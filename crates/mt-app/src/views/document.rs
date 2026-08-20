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
    menu::DropdownMenu as _,
    resizable::{h_resizable, resizable_panel},
    text::{TextView, TextViewState, TextViewStyle},
    v_flex,
};
use mt_doc::{DocType, Document, Severity};

use crate::fs::{self, LoadedFile, SaveError};
use crate::i18n;
use crate::metrics;
use crate::renderer::RendererRegistry;
use crate::views::{Layout, PreviewKind};
use crate::web::{self, Trust};

// One action per layout.
//
// Unit actions rather than one carrying the layout: `PopupMenu` items are
// actions, and a payload-carrying action needs `schemars` derives this crate
// does not otherwise depend on. Five names is the cheaper trade, and it makes
// each layout independently bindable to a key.
actions!(
    markturbo,
    [
        ViewSource,
        ViewNative,
        ViewWeb,
        ViewSplitNative,
        ViewSplitWeb
    ]
);

/// The action that selects `layout`.
fn layout_action(layout: Layout) -> Box<dyn gpui::Action> {
    match layout {
        Layout::Source => Box::new(ViewSource),
        Layout::Native => Box::new(ViewNative),
        Layout::Web => Box::new(ViewWeb),
        Layout::SplitNative => Box::new(ViewSplitNative),
        Layout::SplitWeb => Box::new(ViewSplitWeb),
    }
}

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
    layout: Layout,
    trust: Trust,
    dirty: bool,
    /// Set when the file changed on disk while open.
    externally_changed: bool,
    registry: Arc<RendererRegistry>,
    /// Cached WebView HTML, rebuilt on reparse. Held here rather than in the
    /// WebView so switching modes does not re-render.
    web_html: Option<String>,
    /// The first visible editor row the last time the preview was synced.
    ///
    /// Sync is driven from render, which runs on every frame — without this the
    /// preview would be told to scroll to where it already is, sixty times a
    /// second, and each of those is a script evaluation in another process.
    synced_row: Option<usize>,
    /// The window's single WebView, lent to this tab while it is the active one
    /// showing a Web pane.
    ///
    /// It has to be *in this element tree* rather than merely alive: the OS
    /// child window's bounds are set by `WebViewElement::prepaint`, and a
    /// `WebView` that is never rendered keeps the `Rect::default()` it was
    /// constructed with — 0x0 at the origin, which is exactly "the Web view does
    /// not work".
    #[cfg(any(target_os = "windows", target_os = "macos"))]
    webview: Option<Entity<gpui_wry::WebView>>,
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
        let layout = Layout::Native;

        let mut this = Self {
            focus_handle: cx.focus_handle(),
            file,
            document,
            editor,
            preview,
            layout,
            trust: Trust::Restricted,
            dirty: false,
            externally_changed: false,
            registry,
            web_html: None,
            synced_row: None,
            #[cfg(any(target_os = "windows", target_os = "macos"))]
            webview: None,
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
        if self.dirty {
            format!("{name} •")
        } else {
            name
        }
    }

    pub fn is_dirty(&self) -> bool {
        self.dirty
    }

    pub fn document(&self) -> &Document {
        &self.document
    }

    pub fn layout(&self) -> Layout {
        self.layout
    }

    pub fn set_layout(&mut self, layout: Layout, cx: &mut Context<Self>) {
        if self.layout == layout {
            return;
        }
        self.layout = layout;
        // The WebView HTML is only built when a Web pane is actually visible;
        // switching into one for the first time needs it now.
        if layout.uses_webview() && self.web_html.is_none() {
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

    /// Put the cursor at `offset` and scroll it into view.
    ///
    /// Used by the outline, where clicking a heading has to actually go there.
    /// A preview-only mode has no cursor to move, so this switches into one that
    /// shows the editor — Split rather than Source, so the reader keeps the
    /// rendered document they were navigating.
    /// Scroll the preview to match the editor, when the setting asks for it.
    ///
    /// Proportional rather than positional: the editor measures in source lines
    /// and the preview in rendered pixels, and one source line can render as a
    /// heading, a paragraph, or an entire diagram. Mapping "fraction of the way
    /// through the source" to "fraction of the way through the render" is the
    /// approximation every split-pane Markdown editor makes, and it is stable
    /// under exactly the thing that breaks line-mapping: a block whose rendered
    /// height has nothing to do with its source height.
    ///
    /// Only ever driven from the editor. Two-way sync means each pane's scroll
    /// event moves the other, which moves the first — and the loop is only not
    /// infinite because of rounding.
    fn sync_preview_scroll(&mut self, cx: &mut Context<Self>) {
        if !crate::settings::AppSettings::global(cx).split_sync_scroll {
            return;
        }
        if !self.layout.is_split() {
            return;
        }
        let Some(visible) = self.editor.read(cx).visible_row_range() else {
            return;
        };
        let row = visible.start;
        if self.synced_row == Some(row) {
            return;
        }
        self.synced_row = Some(row);

        let total = self.line_count(cx);
        // A document short enough to fit needs no sync, and would divide by a
        // near-zero denominator if it tried.
        if total <= visible.len() {
            return;
        }
        let fraction = row as f32 / total.saturating_sub(visible.len()).max(1) as f32;
        self.scroll_preview_to(fraction.clamp(0., 1.), cx);
    }

    /// Number of lines in the editor's current text.
    fn line_count(&self, cx: &App) -> usize {
        self.text(cx).lines().count().max(1)
    }

    /// Scroll whichever preview is showing to `fraction` of its height.
    fn scroll_preview_to(&mut self, fraction: f32, cx: &mut Context<Self>) {
        match self.layout.preview().unwrap_or(PreviewKind::Native) {
            PreviewKind::Web => {
                #[cfg(any(target_os = "windows", target_os = "macos"))]
                if let Some(webview) = &self.webview {
                    // The preview is a separate browser context, so the only
                    // way in is a script. `scrollingElement` covers both quirks
                    // and standards mode; the guard makes a document that has
                    // not finished loading a no-op rather than an exception.
                    let script = format!(
                        "(function(){{var e=document.scrollingElement||document.body;\
                         if(!e)return;var h=e.scrollHeight-e.clientHeight;\
                         if(h>0)e.scrollTop=h*{fraction};}})()"
                    );
                    let _ = webview.read(cx).raw().evaluate_script(&script);
                }
                #[cfg(not(any(target_os = "windows", target_os = "macos")))]
                let _ = fraction;
            }
            // ponytail: the native preview is a `TextView`, which owns its
            // scroll handle and does not expose it. Syncing it needs an upstream
            // accessor; the Web preview is the one that can be driven today.
            PreviewKind::Native => {}
        }
    }

    pub fn reveal_offset(&mut self, offset: usize, window: &mut Window, cx: &mut Context<Self>) {
        // Clamp against the *editor's* text, not the parsed document: the parse
        // is debounced, so an outline built moments ago can name an offset past
        // the end of a document the user has since shortened.
        let text = self.text(cx);
        let offset = offset.min(text.len());
        let fraction = if text.is_empty() {
            0.
        } else {
            offset as f32 / text.len() as f32
        };

        // A preview-only layout has no caret to move, so the jump has to reach
        // the preview instead. Only the Web preview can be scrolled today —
        // the native one is a `TextView` that owns its scroll handle without
        // exposing it — so Native falls back to opening the editor beside it,
        // keeping the renderer the user chose.
        let can_scroll_preview = self.layout.preview() == Some(PreviewKind::Web);
        if !self.layout.shows_editor() && !can_scroll_preview {
            self.set_layout(self.layout.with_editor(), cx);
        }

        if self.layout.shows_editor() {
            self.editor.update(cx, |state, cx| {
                // `set_selected_range` routes through `move_to`, which is what
                // scrolls the viewport; setting the cursor without it would move
                // an invisible caret.
                state.set_selected_range(offset..offset, cx);
                state.focus(window, cx);
            });
        }

        // Scroll the preview when it is the only pane — a jump that moved
        // nothing visible is a click that did nothing — and in Split only when
        // the user asked the panes to stay together.
        let preview_only = !self.layout.shows_editor();
        let sync_split =
            self.layout.is_split() && crate::settings::AppSettings::global(cx).split_sync_scroll;
        if preview_only || sync_split {
            // The render-driven sync would otherwise see the same first visible
            // row it last recorded and skip the update.
            self.synced_row = None;
            self.scroll_preview_to(fraction.clamp(0., 1.), cx);
        }
        cx.notify();
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
            crate::views::try_update(&this, cx, |this, cx| this.schedule_reparse(cx));
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
        self.preview
            .update(cx, |state, cx| state.set_text(&text, cx));

        self._reparse = Some(cx.spawn(async move |this, cx| {
            let parsed = cx
                .background_spawn({
                    let text = text.clone();
                    async move { Document::with_type(doc_type, text) }
                })
                .await;

            crate::views::try_update(&this, cx, |this, cx| {
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
        if self.layout.uses_webview() {
            self.rebuild_web(cx);
        } else {
            // Invalidate so switching to Web later rebuilds rather than showing
            // a stale render.
            self.web_html = None;
        }
    }

    fn rebuild_web(&mut self, cx: &mut Context<Self>) {
        if self.document.source().len() > LIVE_PREVIEW_LIMIT {
            self.web_html = Some(oversize_notice(self.document.source().len()));
            return;
        }
        // Paint the preview with the app's own preset rather than letting the
        // browser follow the OS: otherwise an explicit Nord shows a generic dark
        // preview next to Nord-colored chrome.
        let preset = crate::settings::active_preset(cx);
        self.web_html = Some(web::build_html_themed(
            &self.document,
            &self.registry,
            self.trust,
            Some(preset),
        ));
    }

    /// Rebuild the Web payload after something outside this view changed how it
    /// should look — today, the theme.
    pub fn theme_changed(&mut self, cx: &mut Context<Self>) {
        if self.web_html.is_some() {
            self.rebuild_web(cx);
        }
        cx.notify();
    }

    /// The HTML currently destined for the WebView, if any.
    pub fn web_html(&self) -> Option<&str> {
        self.web_html.as_deref()
    }

    /// Lend this tab the window's WebView, or take it back.
    ///
    /// The workspace owns it — it is one OS child window per window, not per
    /// document — but only the tab currently rendering a Web pane may put it in
    /// its element tree, or two tabs would fight over its bounds.
    #[cfg(any(target_os = "windows", target_os = "macos"))]
    pub fn set_webview(
        &mut self,
        webview: Option<Entity<gpui_wry::WebView>>,
        cx: &mut Context<Self>,
    ) {
        let same = match (&self.webview, &webview) {
            (Some(current), Some(next)) => current == next,
            (None, None) => true,
            _ => false,
        };
        if same {
            return;
        }
        self.webview = webview;
        cx.notify();
    }

    /// Whether this tab currently holds the window's WebView.
    #[cfg(any(target_os = "windows", target_os = "macos"))]
    pub fn has_webview(&self) -> bool {
        self.webview.is_some()
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
            .px(metrics::inset())
            .py(metrics::header_pad_y())
            .gap(metrics::gap_group())
            .items_center()
            .border_b_1()
            .border_color(cx.theme().border)
            // One dropdown rather than four buttons and a conditional toggle.
            // The five layouts are mutually exclusive, so a control that shows
            // one value and opens to reveal the rest is the honest shape — and
            // the old toggle only appeared once Split was selected, which hid
            // half the choices behind the other half.
            .child(
                Button::new("layout")
                    .label(i18n::t(self.layout.label_key(), cx))
                    .icon(IconName::ChevronDown)
                    .xsmall()
                    .ghost()
                    .tooltip(i18n::t(i18n::Key::ViewLayout, cx))
                    .dropdown_menu({
                        let current = self.layout;
                        move |menu, _window, _cx| {
                            Layout::ALL.iter().fold(menu, |menu, layout| {
                                menu.menu_with_check(
                                    i18n::text(
                                        layout.label_key(),
                                        crate::settings::Language::default(),
                                    ),
                                    *layout == current,
                                    layout_action(*layout),
                                )
                            })
                        }
                    }),
            )
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
                            Trust::Restricted => i18n::t(i18n::Key::TrustThisDocument, cx),
                            Trust::Trusted => i18n::t(i18n::Key::Trusted, cx),
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
                    .label(i18n::t(i18n::Key::Save, cx))
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
                .px(metrics::inset())
                .py(metrics::header_pad_y())
                .gap(metrics::gap_group())
                .items_center()
                .bg(cx.theme().warning.opacity(0.15))
                .border_b_1()
                .border_color(cx.theme().warning)
                .child(Icon::new(IconName::TriangleAlert).small())
                .child(
                    div()
                        .flex_1()
                        .text_sm()
                        .child(i18n::t(i18n::Key::FileChangedOnDisk, cx)),
                )
                .child(
                    Button::new("reload")
                        .label(i18n::t(i18n::Key::ReloadFromDisk, cx))
                        .xsmall()
                        .on_click(cx.listener(|this, _, window, cx| this.reload(window, cx))),
                )
                .child(
                    Button::new("overwrite")
                        .label(i18n::t(i18n::Key::Overwrite, cx))
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
    /// The `WebView` entity itself lives in the workspace (one per window,
    /// reused across tabs, because each is an OS-level child window) and is
    /// lent to the active tab via [`Self::set_webview`]. Childing it here is
    /// load-bearing rather than decorative: `WebViewElement::prepaint` is the
    /// only thing that ever calls `set_bounds` on the child window, so a
    /// `WebView` outside the element tree stays 0x0 no matter what it loads.
    fn render_web_preview(&self, cx: &Context<Self>) -> impl IntoElement {
        #[cfg(any(target_os = "windows", target_os = "macos"))]
        {
            let _ = cx;
            div()
                .id("web-preview")
                .size_full()
                .children(self.webview.clone())
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
                .child(
                    div()
                        .text_sm()
                        .child("WebView is not available on this platform."),
                )
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
        if self.layout.uses_webview() {
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
                None if matches!(lang.to_ascii_lowercase().as_str(), "math" | "latex" | "tex") => {
                    "math".to_string()
                }
                None => return None,
            };
            // Rendering happens here, on the background parse task, so a
            // shell-out never blocks the UI thread.
            let outcome = parse_registry.render(&id, &code.value);
            Some(
                MarkdownNode::new(
                    "mt-block",
                    RenderedBlock {
                        id,
                        outcome,
                        source: code.value.clone(),
                    },
                )
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
        let body = if self.layout.is_split() {
            h_resizable("split")
                .child(resizable_panel().child(self.render_editor(cx)))
                .child(resizable_panel().child(self.render_preview(cx)))
                .into_any_element()
        } else if self.layout.shows_editor() {
            self.render_editor(cx).into_any_element()
        } else {
            self.render_preview(cx)
        };

        // After the panes are built, so the editor has a layout to report a
        // visible range from. Cheap and self-debouncing: it returns immediately
        // unless the first visible row actually moved.
        self.sync_preview_scroll(cx);

        v_flex()
            .id("document")
            // One handler per layout action, so the dropdown items work and each
            // layout is independently bindable.
            .on_action(
                cx.listener(|this, _: &ViewSource, _, cx| this.set_layout(Layout::Source, cx)),
            )
            .on_action(
                cx.listener(|this, _: &ViewNative, _, cx| this.set_layout(Layout::Native, cx)),
            )
            .on_action(cx.listener(|this, _: &ViewWeb, _, cx| this.set_layout(Layout::Web, cx)))
            .on_action(cx.listener(|this, _: &ViewSplitNative, _, cx| {
                this.set_layout(Layout::SplitNative, cx)
            }))
            .on_action(
                cx.listener(|this, _: &ViewSplitWeb, _, cx| this.set_layout(Layout::SplitWeb, cx)),
            )
            // A focusable element with an id but no role makes assistive
            // technology announce the whole window instead of the document —
            // gpui logs exactly that. `Group` is the right one for a container
            // holding a toolbar, an editor and a preview.
            .role(gpui::Role::Group)
            .aria_label(self.title())
            .track_focus(&self.focus_handle)
            .size_full()
            .child(self.render_toolbar(cx))
            .children(self.render_conflict_banner(cx))
            .child(div().flex_1().min_h_0().child(body))
    }
}

#[cfg(test)]
mod tests {
    // Import selectively: the `gpui::*` glob above re-exports a `test` attribute
    // macro that shadows the built-in one and blows the recursion limit.

    /// The Web pane must place the `WebView` in the element tree.
    ///
    /// A source-level check, like `workspace.rs`'s companion, and for the same
    /// reason: the failure needs a real window with a real WebView2 runtime, so
    /// it is not reachable from a unit test — but it is trivially reintroducible
    /// by anyone who reads `render_web_preview`'s empty div as the whole story.
    ///
    /// What broke: the WebView was created, shown, and given a `data:` URL, but
    /// never childed anywhere. `WebViewElement::prepaint` is the only code that
    /// calls `set_bounds` on the OS child window, so it kept the 0x0
    /// `Rect::default()` from `WebView::new` — loaded, visible, and invisible.
    #[test]
    fn the_web_pane_renders_the_webview_entity() {
        let source = include_str!("document.rs");
        let start = source
            .find("fn render_web_preview")
            .expect("the Web pane renderer");
        let body = &source[start..];
        let end = body.find("\n    fn render_preview").unwrap_or(body.len());
        let body = &body[..end];

        assert!(
            body.contains("self.webview"),
            "`render_web_preview` must child the WebView entity; without it the \
             OS child window is never laid out and stays 0x0 no matter what it \
             loads"
        );
        assert!(
            source.contains("pub fn set_webview"),
            "the workspace needs a way to lend the WebView to the active tab"
        );
    }

    /// Revealing an offset must move something visible, in every layout.
    ///
    /// The bug this replaces: a jump from the outline forced the document into
    /// Split — discarding the user's chosen renderer — because moving a caret
    /// was the only thing it knew how to do. Every layout has to respond, and
    /// none may silently switch to a different preview.
    #[test]
    fn reveal_offset_moves_something_in_every_layout() {
        let source = include_str!("document.rs");
        let start = source.find("pub fn reveal_offset").expect("reveal_offset");
        let body = &source[start..];
        let end = body
            .find("\n    /// Replace the whole")
            .unwrap_or(body.len());
        let body = &body[..end];

        assert!(
            body.contains("set_selected_range"),
            "must move the cursor through the path that also scrolls it into view"
        );
        assert!(
            body.contains("scroll_preview_to"),
            "a preview-only layout has no caret, so the preview has to move"
        );
        assert!(
            body.contains("with_editor()"),
            "a layout whose preview cannot be scrolled must open the editor \
             beside it rather than switching renderers"
        );
        assert!(
            !body.contains("Layout::SplitNative") && !body.contains("Layout::Split)"),
            "the layout must be derived from the current one, not hard-coded — \
             hard-coding is what discarded the user's renderer"
        );
    }

    /// Scroll sync must be one-way, driven from the editor.
    ///
    /// Source-level because the failure needs two laid-out panes and a real
    /// scroll event: two-way sync means each pane's movement moves the other,
    /// which moves the first, and the loop only terminates because of rounding.
    /// It reads as a preview that drifts or judders and is very hard to
    /// attribute after the fact.
    #[test]
    fn scroll_sync_is_driven_only_from_the_editor() {
        let source = include_str!("document.rs");
        let start = source
            .find("fn sync_preview_scroll")
            .expect("sync_preview_scroll");
        let body = &source[start..];
        let end = body.find("\n    /// Number of lines").unwrap_or(body.len());
        let body = &body[..end];

        assert!(
            body.contains("visible_row_range"),
            "the editor's visible range is what drives the mapping"
        );
        assert!(
            !body.contains("set_scroll_offset"),
            "syncing back into the editor closes the feedback loop"
        );
        assert!(
            body.contains("split_sync_scroll"),
            "sync must be off unless the setting asks for it"
        );
        assert!(
            body.contains("self.synced_row"),
            "render runs every frame; without the guard this evaluates a script \
             in another process sixty times a second"
        );
    }

    /// The injected script must tolerate a document that has not loaded.
    #[test]
    fn the_scroll_script_is_guarded() {
        let source = include_str!("document.rs");
        let start = source
            .find("fn scroll_preview_to")
            .expect("scroll_preview_to");
        let body = &source[start..];
        let end = body
            .find("\n    pub fn reveal_offset")
            .unwrap_or(body.len());
        let body = &body[..end];

        assert!(
            body.contains("scrollingElement"),
            "quirks-mode documents scroll on `body`, standards on `documentElement`"
        );
        assert!(
            body.contains("if(!e)return"),
            "a document mid-load has no scrolling element; without the guard \
             this throws inside the WebView"
        );
        assert!(
            body.contains("if(h>0)"),
            "a preview shorter than its viewport has nothing to scroll, and \
             dividing by its zero height would be a NaN offset"
        );
    }
}
