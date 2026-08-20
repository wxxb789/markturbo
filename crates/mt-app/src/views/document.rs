//! The document view: one open file, in Source / Native / Web / Split.
//!
//! Owns the editor state and the derived [`Document`]. Reparsing is driven by
//! edits, debounced, and the parse result is what every pane reads — one
//! document model driving both rendering paths.

use std::sync::Arc;
use std::time::Duration;

use gpui::prelude::FluentBuilder as _;
use gpui::*;
use gpui_component::{
    ActiveTheme as _, Icon, IconName, Sizable as _,
    button::{Button, ButtonVariants as _},
    h_flex,
    highlighter::Language,
    input::{Editor, EditorState, InputEvent, TabSize},
    resizable::{h_resizable, resizable_panel},
    text::{TextView, TextViewState, TextViewStyle},
    v_flex,
};
use mt_doc::{DocType, Document, Severity};

use crate::fs::{self, LoadedFile, SaveError};
use crate::renderer::RendererRegistry;
use crate::views::{PreviewKind, ViewMode};
use crate::web::{self, Trust};

/// How long after the last keystroke to reparse and refresh the preview.
///
/// Reparsing per keystroke is what makes a 100K-line document unusable; this is
/// short enough to feel live and long enough to coalesce typing.
const REPARSE_DEBOUNCE: Duration = Duration::from_millis(180);

/// Documents above this size skip live preview refresh while typing.
///
/// The preview still updates when the user pauses; what this avoids is
/// re-rendering a megabyte of Markdown on a background task every 180ms.
const LIVE_PREVIEW_LIMIT: usize = 512 * 1024;

/// Events a document view emits to the workspace.
#[derive(Debug, Clone)]
pub enum DocumentEvent {
    /// The dirty flag changed; the tab label needs a refresh.
    DirtyChanged,
    /// A save failed because the file changed on disk.
    Conflict,
    /// Something worth telling the user.
    Status(String),
}

pub struct DocumentView {
    focus_handle: FocusHandle,
    /// The file as loaded, including the stamp used for conflict detection.
    file: LoadedFile,
    /// Parsed view of the editor's current text.
    document: Document,
    editor: Entity<EditorState>,
    /// Native preview state. Rebuilt from `document` on reparse.
    preview: Entity<TextViewState>,
    mode: ViewMode,
    split_preview: PreviewKind,
    trust: Trust,
    dirty: bool,
    /// Set when the file changed on disk while open.
    externally_changed: bool,
    registry: Arc<RendererRegistry>,
    /// Cached WebView HTML, rebuilt on reparse. Held here rather than in the
    /// WebView so switching modes does not re-render.
    web_html: Option<String>,
    _reparse: Option<Task<()>>,
    _subscriptions: Vec<Subscription>,
}

impl DocumentView {
    pub fn new(
        file: LoadedFile,
        registry: Arc<RendererRegistry>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let doc_type = DocType::of(&file.path);
        let document = Document::new(Some(file.path.clone()), file.text.clone());

        let editor = cx.new(|cx| {
            EditorState::new(window, cx)
                .language(editor_language(doc_type))
                .line_number(true)
                .soft_wrap(true)
                .tab_size(TabSize {
                    tab_size: 2,
                    ..Default::default()
                })
                .searchable(true)
                .default_value(file.text.clone())
        });

        let preview = cx.new(|cx| TextViewState::markdown(&file.text, cx).selectable(true));

        let subscriptions = vec![cx.subscribe_in(
            &editor,
            window,
            |this: &mut Self, _, event: &InputEvent, window, cx| {
                if matches!(event, InputEvent::Change) {
                    this.on_edit(window, cx);
                }
            },
        )];

        // Markdown opens in Native: reading is the common case, and it is the
        // fast path.
        let mode = ViewMode::Native;

        let mut this = Self {
            focus_handle: cx.focus_handle(),
            file,
            document,
            editor,
            preview,
            mode,
            split_preview: PreviewKind::Native,
            trust: Trust::Restricted,
            dirty: false,
            externally_changed: false,
            registry,
            web_html: None,
            _reparse: None,
            _subscriptions: subscriptions,
        };
        this.rebuild_derived(cx);
        this
    }

    pub fn path(&self) -> &std::path::Path {
        &self.file.path
    }

    pub fn title(&self) -> String {
        let name = self
            .file
            .path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("untitled")
            .to_string();
        if self.dirty { format!("{name} •") } else { name }
    }

    pub fn is_dirty(&self) -> bool {
        self.dirty
    }

    pub fn document(&self) -> &Document {
        &self.document
    }

    pub fn mode(&self) -> ViewMode {
        self.mode
    }

    pub fn set_mode(&mut self, mode: ViewMode, cx: &mut Context<Self>) {
        if self.mode == mode {
            return;
        }
        self.mode = mode;
        // The WebView HTML is only built when a Web pane is actually visible;
        // switching into one for the first time needs it now.
        if mode.uses_webview(self.split_preview) && self.web_html.is_none() {
            self.rebuild_web(cx);
        }
        cx.notify();
    }

    pub fn set_trust(&mut self, trust: Trust, cx: &mut Context<Self>) {
        if self.trust == trust {
            return;
        }
        self.trust = trust;
        self.rebuild_web(cx);
        cx.notify();
    }

    /// Choose which renderer fills the preview pane in Split mode.
    pub fn set_split_preview(&mut self, kind: PreviewKind, cx: &mut Context<Self>) {
        if self.split_preview == kind {
            return;
        }
        self.split_preview = kind;
        // Switching into a Web preview needs the HTML that was skipped while it
        // was hidden.
        if self.mode.uses_webview(kind) && self.web_html.is_none() {
            self.rebuild_web(cx);
        }
        cx.notify();
    }

    pub fn split_preview(&self) -> PreviewKind {
        self.split_preview
    }

    pub fn trust(&self) -> Trust {
        self.trust
    }

    /// The editor's current text — the authoritative in-memory content.
    pub fn text(&self, cx: &App) -> String {
        self.editor.read(cx).value().to_string()
    }

    /// Selected byte range in the editor, for selection-scoped translation.
    pub fn selection(&self, cx: &App) -> std::ops::Range<usize> {
        self.editor.read(cx).selected_range()
    }

    /// Cursor offset, for block-scoped operations.
    pub fn cursor(&self, cx: &App) -> usize {
        self.editor.read(cx).cursor()
    }

    /// Replace the whole document text, e.g. with a translation result.
    ///
    /// Uses `replace_all` rather than `set_value` so the change is undoable —
    /// a translation the user dislikes must be revertible with Ctrl+Z.
    pub fn replace_text(&mut self, text: String, window: &mut Window, cx: &mut Context<Self>) {
        self.editor.update(cx, |state, cx| {
            state.replace_all(text, window, cx);
        });
        self.on_edit(window, cx);
    }

    /// Note that the file changed on disk. Does not touch editor state.
    pub fn mark_externally_changed(&mut self, cx: &mut Context<Self>) {
        if !self.externally_changed {
            self.externally_changed = true;
            cx.notify();
        }
    }

    pub fn is_externally_changed(&self) -> bool {
        self.externally_changed
    }

    /// Re-read the file from disk, discarding unsaved edits.
    pub fn reload(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        match fs::load(&self.file.path) {
            Ok(file) => {
                let text = file.text.clone();
                self.file = file;
                self.editor.update(cx, |state, cx| {
                    state.set_value(text, window, cx);
                });
                self.dirty = false;
                self.externally_changed = false;
                self.rebuild_derived(cx);
                cx.emit(DocumentEvent::DirtyChanged);
                cx.emit(DocumentEvent::Status("Reloaded from disk".into()));
            }
            Err(err) => cx.emit(DocumentEvent::Status(format!("Reload failed: {err}"))),
        }
        cx.notify();
    }

    /// Save to disk, refusing to clobber an external change unless `force`.
    pub fn save(&mut self, force: bool, cx: &mut Context<Self>) {
        let text = self.text(cx);
        match fs::save(&self.file, &text, force) {
            Ok(stamp) => {
                self.file.stamp = stamp;
                self.file.text = text;
                self.dirty = false;
                self.externally_changed = false;
                cx.emit(DocumentEvent::DirtyChanged);
                cx.emit(DocumentEvent::Status("Saved".into()));
            }
            Err(SaveError::Conflict) => {
                self.externally_changed = true;
                cx.emit(DocumentEvent::Conflict);
            }
            Err(err) => cx.emit(DocumentEvent::Status(format!("Save failed: {err}"))),
        }
        cx.notify();
    }

    /// Called on every keystroke. Marks dirty immediately (cheap) and schedules
    /// a reparse (not cheap).
    fn on_edit(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        if !self.dirty {
            self.dirty = true;
            cx.emit(DocumentEvent::DirtyChanged);
        }

        // Replacing the task cancels the previous one, which is the debounce:
        // only the last keystroke in a burst triggers work.
        self._reparse = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(REPARSE_DEBOUNCE).await;
            let _ = this.update(cx, |this, cx| this.schedule_reparse(cx));
        }));
        cx.notify();
    }

    /// Reparse off the UI thread, then swap in the result.
    ///
    /// Parsing is not cheap and — because markdown-rs is superlinear in the
    /// number of blocks — it is *seconds* on a 100K-line document. Doing it
    /// inline would freeze the window; the whole point of the native path is
    /// that it does not. The editor and the previous parse stay live meanwhile.
    fn schedule_reparse(&mut self, cx: &mut Context<Self>) {
        let text = self.text(cx);
        if text == self.document.source() {
            return;
        }
        let doc_type = self.document.doc_type();

        // The native preview is driven by `TextViewState`, which does its own
        // background parsing, so it can update immediately.
        self.preview.update(cx, |state, cx| state.set_text(&text, cx));

        self._reparse = Some(cx.spawn(async move |this, cx| {
            let parsed = cx
                .background_spawn({
                    let text = text.clone();
                    async move { Document::with_type(doc_type, text) }
                })
                .await;

            let _ = this.update(cx, |this, cx| {
                // Discard a stale result: the user may have typed on while we
                // parsed, and a newer task is already queued.
                if this.text(cx) != parsed.source() {
                    return;
                }
                this.document = parsed;
                this.refresh_web(cx);
                cx.notify();
            });
        }));
    }

    /// Reparse synchronously. Only for load and reload, where there is nothing
    /// on screen to block and the caller needs the result immediately.
    fn rebuild_derived(&mut self, cx: &mut Context<Self>) {
        let text = self.text(cx);
        self.document.set_source(text.clone());
        self.preview.update(cx, |state, cx| {
            state.set_text(&text, cx);
        });
        self.refresh_web(cx);
    }

    /// Rebuild or invalidate the WebView payload, depending on visibility.
    fn refresh_web(&mut self, cx: &mut Context<Self>) {
        // Only build HTML when a Web pane is actually visible; doing it for a
        // hidden pane is pure waste.
        if self.mode.uses_webview(self.split_preview) {
            self.rebuild_web(cx);
        } else {
            // Invalidate so switching to Web later rebuilds rather than showing
            // a stale render.
            self.web_html = None;
        }
    }

    fn rebuild_web(&mut self, _cx: &mut Context<Self>) {
        if self.document.source().len() > LIVE_PREVIEW_LIMIT {
            self.web_html = Some(oversize_notice(self.document.source().len()));
            return;
        }
        self.web_html = Some(web::build_html(&self.document, &self.registry, self.trust));
    }

    /// The HTML currently destined for the WebView, if any.
    pub fn web_html(&self) -> Option<&str> {
        self.web_html.as_deref()
    }

    fn render_toolbar(&self, cx: &Context<Self>) -> impl IntoElement {
        let doc_type = self.document.doc_type();
        let errors = self
            .document
            .diagnostics()
            .iter()
            .filter(|d| d.severity == Severity::Error)
            .count();

        h_flex()
            .w_full()
            .px_2()
            .py_1()
            .gap_2()
            .items_center()
            .border_b_1()
            .border_color(cx.theme().border)
            .child(
                h_flex().gap_1().children(ViewMode::ALL.map(|mode| {
                    Button::new(SharedString::from(format!("mode-{}", mode.label())))
                        .label(mode.label())
                        .xsmall()
                        .when(self.mode == mode, |b| b.primary())
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.set_mode(mode, cx);
                        }))
                })),
            )
            // In Split, which renderer fills the preview pane is a separate
            // choice from the mode. Keeping them separate is what leaves room
            // for `Native | Web` and `Original | Translation` later.
            .when(self.mode == ViewMode::Split, |this| {
                let next = match self.split_preview {
                    PreviewKind::Native => PreviewKind::Web,
                    PreviewKind::Web => PreviewKind::Native,
                };
                this.child(
                    Button::new("split-preview")
                        .label(self.split_preview.label())
                        .xsmall()
                        .ghost()
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.set_split_preview(next, cx);
                        })),
                )
            })
            .child(div().flex_1())
            // Document type is a first-class label: an AGENTS.md is not just
            // "a Markdown file".
            .child(
                div()
                    .px_2()
                    .py_0p5()
                    .rounded(cx.theme().radius)
                    .bg(cx.theme().secondary)
                    .text_xs()
                    .child(doc_type.label()),
            )
            .when(errors > 0, |this| {
                this.child(
                    div()
                        .px_2()
                        .text_xs()
                        .text_color(cx.theme().danger)
                        .child(format!("{errors} issue(s)")),
                )
            })
            .when(doc_type == DocType::Mdx, |this| {
                let trust = self.trust;
                this.child(
                    Button::new("trust")
                        .label(match trust {
                            Trust::Restricted => "Trust this document",
                            Trust::Trusted => "Trusted ✓",
                        })
                        .xsmall()
                        .when(trust == Trust::Trusted, |b| b.primary())
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.set_trust(
                                match trust {
                                    Trust::Restricted => Trust::Trusted,
                                    Trust::Trusted => Trust::Restricted,
                                },
                                cx,
                            );
                        })),
                )
            })
            .child(
                Button::new("save")
                    .label("Save")
                    .xsmall()
                    .when(self.dirty, |b| b.primary())
                    .on_click(cx.listener(|this, _, _, cx| this.save(false, cx))),
            )
    }

    /// The banner shown when the file changed underneath us.
    ///
    /// Deliberately blocking-looking and offering both choices: silently
    /// picking one would be exactly the data loss the goal forbids.
    fn render_conflict_banner(&self, cx: &Context<Self>) -> Option<impl IntoElement> {
        if !self.externally_changed {
            return None;
        }
        Some(
            h_flex()
                .w_full()
                .px_3()
                .py_2()
                .gap_3()
                .items_center()
                .bg(cx.theme().warning.opacity(0.15))
                .border_b_1()
                .border_color(cx.theme().warning)
                .child(Icon::new(IconName::TriangleAlert).small())
                .child(
                    div()
                        .flex_1()
                        .text_sm()
                        .child("This file changed on disk since it was opened."),
                )
                .child(
                    Button::new("reload")
                        .label("Reload from disk")
                        .xsmall()
                        .on_click(cx.listener(|this, _, window, cx| this.reload(window, cx))),
                )
                .child(
                    Button::new("overwrite")
                        .label("Overwrite")
                        .xsmall()
                        .danger()
                        .on_click(cx.listener(|this, _, _, cx| this.save(true, cx))),
                ),
        )
    }

    fn render_editor(&self, cx: &Context<Self>) -> impl IntoElement {
        div()
            .id("source")
            .size_full()
            .font_family(cx.theme().mono_font_family.clone())
            .text_size(cx.theme().mono_font_size)
            .child(Editor::new(&self.editor).h(relative(1.)).p_0().border_0())
    }

    /// The native preview.
    ///
    /// Rendered by `gpui-component`'s Markdown TextView, extended with a block
    /// renderer for diagrams and math. That registry — not a hard-coded match
    /// on "mermaid" — is what makes another technology a registration.
    fn render_native_preview(&self, cx: &Context<Self>) -> impl IntoElement {
        let registry = self.registry.clone();
        let diagnostics = self.render_diagnostics(cx);

        v_flex()
            .id("native-preview")
            .size_full()
            .children(diagnostics)
            .child(
                TextView::new(&self.preview)
                    .style(preview_style(cx))
                    .selectable(true)
                    .scrollable(true)
                    .markdown_extensions(crate::views::document::diagram_extensions(registry))
                    .flex_1()
                    .p_5(),
            )
    }

    /// Inline diagnostics, above the preview.
    ///
    /// Kept out of the rendered Markdown so a diagnostic can never be mistaken
    /// for document content.
    fn render_diagnostics(&self, cx: &Context<Self>) -> Option<impl IntoElement> {
        let diagnostics = self.document.diagnostics();
        if diagnostics.is_empty() {
            return None;
        }
        Some(
            v_flex()
                .w_full()
                .p_2()
                .gap_1()
                .children(diagnostics.iter().take(10).map(|d| {
                    let color = match d.severity {
                        Severity::Error => cx.theme().danger,
                        Severity::Warning => cx.theme().warning,
                        Severity::Info => cx.theme().muted_foreground,
                    };
                    h_flex()
                        .gap_2()
                        .px_2()
                        .py_1()
                        .text_xs()
                        .rounded(cx.theme().radius)
                        .bg(color.opacity(0.1))
                        .child(div().text_color(color).child(d.source.clone()))
                        .when_some(d.line, |this, line| {
                            this.child(
                                div()
                                    .text_color(cx.theme().muted_foreground)
                                    .child(format!("line {line}")),
                            )
                        })
                        .child(div().flex_1().child(d.message.clone()))
                })),
        )
    }

    /// The Web pane.
    ///
    /// The actual `WebView` entity lives in the workspace (one per window,
    /// reused across tabs, because each is an OS-level child window). This
    /// renders the placeholder the workspace overlays it onto, plus the
    /// fallback for platforms where `gpui-wry` has no implementation.
    fn render_web_preview(&self, cx: &Context<Self>) -> impl IntoElement {
        #[cfg(any(target_os = "windows", target_os = "macos"))]
        {
            let _ = cx;
            div().id("web-preview").size_full()
        }
        #[cfg(not(any(target_os = "windows", target_os = "macos")))]
        {
            v_flex()
                .id("web-preview")
                .size_full()
                .items_center()
                .justify_center()
                .gap_2()
                .p_8()
                .child(Icon::new(IconName::Globe))
                .child(div().text_sm().child("WebView is not available on this platform."))
                .child(
                    div()
                        .text_xs()
                        .text_color(cx.theme().muted_foreground)
                        .child(
                            "gpui-wry supports Windows and macOS today. \
                             Native rendering covers Markdown; MDX components \
                             show as placeholders.",
                        ),
                )
        }
    }

    fn render_preview(&self, cx: &Context<Self>) -> AnyElement {
        // One predicate decides this everywhere — here, in `refresh_web`, and
        // in the workspace's WebView sync — so the panes cannot disagree about
        // which renderer is showing.
        if self.mode.uses_webview(self.split_preview) {
            self.render_web_preview(cx).into_any_element()
        } else {
            self.render_native_preview(cx).into_any_element()
        }
    }
}

/// Markdown extensions that render diagram and math fences through the
/// registry.
///
/// One parser + one renderer handles every registered technology, so adding
/// Graphviz means adding a `DiagramKind` — not editing this function.
pub fn diagram_extensions(
    registry: Arc<RendererRegistry>,
) -> gpui_component::text::MarkdownExtensions {
    use gpui_component::text::{MarkdownExtensions, MarkdownNode, markdown_ast};

    let parse_registry = registry.clone();
    MarkdownExtensions::default()
        .block_parser(move |node, _cx| {
            let markdown_ast::Node::Code(code) = node else {
                return None;
            };
            let lang = code.lang.as_deref().unwrap_or("").trim();
            let id = match mt_doc::DiagramKind::from_lang(lang) {
                Some(kind) => kind.id().to_string(),
                None if matches!(
                    lang.to_ascii_lowercase().as_str(),
                    "math" | "latex" | "tex"
                ) =>
                {
                    "math".to_string()
                }
                None => return None,
            };
            // Rendering happens here, on the background parse task, so a
            // shell-out never blocks the UI thread.
            let outcome = parse_registry.render(&id, &code.value);
            Some(
                MarkdownNode::new("mt-block", RenderedBlock {
                    id,
                    outcome,
                    source: code.value.clone(),
                })
                .markdown(format!("```{lang}\n{}\n```", code.value)),
            )
        })
        .block_renderer("mt-block", move |node, _window, cx| {
            let Some(block) = node.data::<RenderedBlock>() else {
                return div().into_any_element();
            };
            render_block(block, cx)
        })
}

/// A block after the registry has had a go at it.
#[derive(Clone)]
struct RenderedBlock {
    id: String,
    outcome: crate::renderer::RenderOutcome,
    source: String,
}

fn render_block(block: &RenderedBlock, cx: &mut App) -> AnyElement {
    use crate::renderer::RenderOutcome;

    match &block.outcome {
        // SVG renders natively via resvg.
        RenderOutcome::Svg(markup) if markup.contains("<svg") => div()
            .w_full()
            .flex()
            .justify_center()
            .py_2()
            .child(
                img(Arc::new(Image::from_bytes(
                    ImageFormat::Svg,
                    markup.clone().into_bytes(),
                )))
                .object_fit(ObjectFit::Contain)
                .max_w_full(),
            )
            .into_any_element(),
        // MathML: resvg cannot draw it, so show the formula source in a math
        // style rather than an empty box. The Web pane renders it properly.
        RenderOutcome::Svg(_) => div()
            .w_full()
            .flex()
            .justify_center()
            .py_2()
            .child(
                div()
                    .px_3()
                    .py_1()
                    .rounded(cx.theme().radius)
                    .bg(cx.theme().secondary)
                    .font_family(cx.theme().mono_font_family.clone())
                    .child(block.source.trim().to_string()),
            )
            .into_any_element(),
        // Failure: the diagnostic plus the untouched source, never a crash and
        // never lost content.
        RenderOutcome::Failed(diag) => v_flex()
            .w_full()
            .my_2()
            .gap_1()
            .p_3()
            .rounded(cx.theme().radius)
            .border_1()
            .border_color(cx.theme().danger.opacity(0.6))
            .child(
                h_flex()
                    .gap_2()
                    .text_sm()
                    .text_color(cx.theme().danger)
                    .child(format!("{} rendering failed", block.id))
                    .when_some(diag.line, |this, line| {
                        this.child(
                            div()
                                .text_xs()
                                .text_color(cx.theme().muted_foreground)
                                .child(format!("line {line}")),
                        )
                    }),
            )
            .child(div().text_xs().child(diag.message.clone()))
            .child(
                div()
                    .mt_1()
                    .p_2()
                    .w_full()
                    .rounded(cx.theme().radius)
                    .bg(cx.theme().secondary)
                    .font_family(cx.theme().mono_font_family.clone())
                    .text_xs()
                    .child(block.source.clone()),
            )
            .into_any_element(),
    }
}

fn preview_style(_cx: &App) -> TextViewStyle {
    // Tables scroll horizontally rather than wrapping: a wide table in an
    // agent instruction file is common and wrapping makes it unreadable.
    let mut table = StyleRefinement::default();
    table.overflow.x = Some(Overflow::Scroll);
    TextViewStyle::default().table(table)
}

fn oversize_notice(len: usize) -> String {
    format!(
        "<!doctype html><html><body style=\"font-family:system-ui;padding:2rem\">\
         <h3>Web preview paused</h3>\
         <p>This document is {} MB. Rendering it through the WebView on every edit \
         would block the UI. Native preview and the editor remain fully live.</p>\
         </body></html>",
        len / (1024 * 1024)
    )
}

/// Which syntax-highlighting language the editor uses for a document type.
fn editor_language(doc_type: DocType) -> Language {
    match doc_type {
        // gpui-component maps "mdx" onto its Markdown grammar; MDX-specific
        // highlighting would need a grammar this build does not ship.
        DocType::Mdx => Language::Markdown,
        _ => Language::Markdown,
    }
}

impl EventEmitter<DocumentEvent> for DocumentView {}

impl Focusable for DocumentView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for DocumentView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let body = match self.mode {
            ViewMode::Source => self.render_editor(cx).into_any_element(),
            ViewMode::Native | ViewMode::Web => self.render_preview(cx),
            ViewMode::Split => h_resizable("split")
                .child(resizable_panel().child(self.render_editor(cx)))
                .child(resizable_panel().child(self.render_preview(cx)))
                .into_any_element(),
        };

        v_flex()
            .id("document")
            .track_focus(&self.focus_handle)
            .size_full()
            .child(self.render_toolbar(cx))
            .children(self.render_conflict_banner(cx))
            .child(div().flex_1().min_h_0().child(body))
    }
}
