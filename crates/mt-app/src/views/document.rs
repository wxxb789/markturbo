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
    tab::{Tab, TabBar},
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

#[cfg(target_os = "windows")]
use crate::views::workspace::web_surface::WindowsWebView;

// One unit action per layout keeps every mode independently bindable without a
// payload-carrying action and its otherwise-unused `schemars` dependency.
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

/// The document's real layouts on every platform.
///
/// Windows WebView2 stays above GPUI only inside the bounds assigned to its
/// preview element. A split editor and the workspace side panels occupy
/// disjoint bounds, so removing `SplitWeb` was a broader restriction than the
/// child-HWND constraint requires. The current editor's search UI stays inside
/// its pane, its context menu is native, and no floating LSP providers are
/// installed. Adding one must revisit this boundary rather than assuming it can
/// paint over the child window.
fn available_layouts(doc_type: DocType) -> &'static [Layout] {
    Layout::available_for(doc_type)
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
    /// Scroll the worker-owned window WebView after the current draw.
    ScrollWebPreview(f32),
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
    /// The diagram/math block extensions handed to the preview's `TextView`.
    ///
    /// Built once and cloned per frame, never rebuilt in `render`. Upstream
    /// stamps every `MarkdownExtensions` with a process-global revision on
    /// construction (`markdown_ext.rs`, an `AtomicU64::fetch_add` inside
    /// `push_block_parser`), and `TextViewState::set_markdown_extensions`
    /// short-circuits only when the revision it already holds matches. A fresh
    /// one per frame therefore never matches: every frame reparsed the whole
    /// document and re-ran the registry over every fence.
    ///
    /// Above upstream's 4 KiB `MAX_SYNC_FULL_REPLACE_BYTES` the reparse goes
    /// async and ends with a `cx.notify()`, which redraws, which reparses —
    /// a self-sustaining loop. Measured on the release binary with no user
    /// input, a 4,200-byte document burned 251% of a core indefinitely while a
    /// 4,000-byte one sat at 0.2%.
    ///
    /// `Clone` copies the revision rather than minting a new one, which is what
    /// makes the guard match from the second frame onwards.
    preview_extensions: gpui_component::text::MarkdownExtensions,
    layout: Layout,
    trust: Trust,
    dirty: bool,
    /// Set when the file changed on disk while open.
    externally_changed: bool,
    registry: Arc<RendererRegistry>,
    /// Cached WebView payload, rebuilt on reparse. Held here rather than in the
    /// WebView so switching modes does not re-render.
    ///
    /// Usually HTML, which the workspace turns into a `data:` URL. A trusted
    /// HTML file instead holds the `file://` URL itself — `to_data_url` passes
    /// one through — because loading that file from disk is the only way its
    /// relative images and stylesheets can resolve.
    web_html: Option<String>,
    /// Changes only when `web_html` changes or is invalidated.
    ///
    /// The workspace compares this before cloning the HTML, so notifications
    /// unrelated to the preview stay a small-integer no-op.
    web_revision: u64,
    /// The first visible editor row the last time the preview was synced.
    ///
    /// Sync is driven from render, which runs on every frame — without this the
    /// preview would be told to scroll to where it already is, sixty times a
    /// second, and each of those is a script evaluation in another process.
    synced_row: Option<usize>,
    /// The window's single WebView, lent to this tab while it is active.
    ///
    /// It has to be *in this element tree* rather than merely alive: the OS
    /// child window's bounds are set by `WebViewElement::prepaint`, and a
    /// `WebView` that is never rendered keeps the `Rect::default()` it was
    /// constructed with — 0x0 at the origin, which is exactly "the Web view does
    /// not work".
    #[cfg(target_os = "windows")]
    webview: Option<WindowsWebView>,
    #[cfg(target_os = "macos")]
    webview: Option<Entity<gpui_wry::WebView>>,
    _reparse: Option<Task<()>>,
    /// The in-flight background reload, deliberately not sharing `_reparse`.
    ///
    /// Sharing the slot would let a keystroke cancel a reload, and a reload
    /// that never lands never runs its second dirty check — which is the thing
    /// that raises the conflict banner for a document the user started typing
    /// into while the parse was still running.
    _reload: Option<Task<()>>,
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
                .language(editor_language(&file.path))
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

        // Markdown opens in Native and HTML in Web: reading is the common case.
        // A `.rs` has no rendered form, so it opens in the editor rather than
        // in a preview pane that would have nothing to draw.
        let layout = Layout::default_for(doc_type);

        let mut this = Self {
            focus_handle: cx.focus_handle(),
            file,
            document,
            editor,
            preview,
            preview_extensions: diagram_extensions(registry.clone()),
            layout,
            trust: Trust::Restricted,
            dirty: false,
            externally_changed: false,
            registry,
            web_html: None,
            web_revision: 0,
            synced_row: None,
            #[cfg(any(target_os = "windows", target_os = "macos"))]
            webview: None,
            _reparse: None,
            _reload: None,
            _subscriptions: subscriptions,
        };
        this.rebuild_derived(cx);
        this
    }

    pub fn path(&self) -> &std::path::Path {
        &self.file.path
    }

    /// Whether this document exists on disk.
    ///
    /// A tab for a file that was never saved has no name to show, so the label
    /// falls back to the first line — see [`Self::title`].
    pub fn is_on_disk(&self) -> bool {
        self.file.path.is_file()
    }

    /// The tab label.
    ///
    /// A file on disk is named by its file name. A buffer that is not is named
    /// by its first line, the way every note-taking app does it — the first
    /// line of a new document is almost always its heading, and `Untitled 3`
    /// tells the reader nothing about which of three drafts it is. An empty or
    /// blank first line has nothing to offer, so that falls back to `Untitled`.
    ///
    /// The dirty marker is **not** here. It used to be appended as ` •`, which
    /// made it part of the string that gets elided — a long name pushed the
    /// marker out of the label entirely, so the one tab that most needed the
    /// warning was the one that lost it. The tab draws it as its own element
    /// now; see `workspace::render_tabs`.
    pub fn title(&self, cx: &App) -> String {
        if self.is_on_disk() {
            return self
                .file
                .path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("untitled")
                .to_string();
        }
        first_line_title(&self.text(cx))
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
                cx.emit(DocumentEvent::ScrollWebPreview(fraction));
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
    ///
    /// Synchronous, and deliberately so: this is the conflict banner's button.
    /// The user asked for it, it happens once, and a freeze they initiated is
    /// one they can attribute. The *automatic* path is
    /// [`Self::reload_if_clean`], which is involuntary and repeats on every
    /// external write — that one must not block the window.
    pub fn reload(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        match fs::load(&self.file.path) {
            Ok(file) => {
                let document = Document::with_type(self.document.doc_type(), file.text.clone());
                self.apply_reload(file, document, window, cx);
            }
            Err(err) => {
                cx.emit(DocumentEvent::Status(format!("Reload failed: {err}")));
                cx.notify();
            }
        }
    }

    /// Swap in a freshly loaded file and the parse that goes with it.
    ///
    /// Takes the `Document` rather than deriving it: the background path has
    /// already paid for the parse, and [`Self::rebuild_derived`] would repeat it
    /// on the UI thread — which is the whole thing that path exists to avoid.
    fn apply_reload(
        &mut self,
        file: LoadedFile,
        document: Document,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let text = file.text.clone();
        self.file = file;
        // `set_value` suppresses the `Change` event, so this does not re-enter
        // `on_edit` and mark the document dirty again.
        self.editor.update(cx, |state, cx| {
            state.set_value(text.clone(), window, cx);
        });
        self.dirty = false;
        self.externally_changed = false;
        self.document = document;
        self.preview.update(cx, |state, cx| {
            state.set_text(&text, cx);
        });
        self.refresh_web(cx);
        cx.emit(DocumentEvent::DirtyChanged);
        cx.emit(DocumentEvent::Status("Reloaded from disk".into()));
        cx.notify();
    }

    /// Start an automatic reload, if there is nothing to lose. Returns whether
    /// it started.
    ///
    /// This is the data-loss guard for auto-refresh: [`Self::reload`] discards
    /// unsaved edits, which is the right answer when the user clicked the
    /// banner's button and the wrong one when a file watcher fired. A dirty
    /// document keeps its text and gets `false`, which is the caller's signal to
    /// raise the banner and let the user choose.
    ///
    /// **The read and the parse both run off the UI thread.** The watcher fires
    /// on every external write, so this runs involuntarily and repeatedly — and
    /// `Document::with_type` is markdown-rs, measured at 23.4s on the 100K-line
    /// fixture. Inline, an agent rewriting a file in a loop would freeze the
    /// window once per write. See `schedule_reparse`, which is the same
    /// hazard reached from the other direction.
    ///
    /// `true` means *started*, not *finished*: the user can begin typing during
    /// the parse, so the result checks the dirty flag a second time on landing
    /// and raises the banner itself if it has to.
    pub fn reload_if_clean(&mut self, cx: &mut Context<Self>) -> bool {
        if self.dirty {
            return false;
        }
        let path = self.file.path.clone();
        let doc_type = self.document.doc_type();

        self._reload = Some(cx.spawn(async move |this, cx| {
            let loaded = cx
                .background_spawn(async move {
                    let file = fs::load(&path)?;
                    let document = Document::with_type(doc_type, file.text.clone());
                    Ok::<_, std::io::Error>((file, document))
                })
                .await;

            crate::views::try_update_in(&this, cx, |this, window, cx| match loaded {
                Ok((file, document)) => this.finish_reload(file, document, window, cx),
                Err(err) => cx.emit(DocumentEvent::Status(format!("Reload failed: {err}"))),
            });
        }));
        true
    }

    /// Apply a background reload, or decline to.
    ///
    /// Two things can have changed while the parse ran, and each has exactly one
    /// safe answer:
    ///
    /// * The user started typing. Applying would discard their text, which is
    ///   the one outcome auto-refresh must never produce — so the banner goes up
    ///   instead and they choose.
    /// * The file was written again. The parse is then of a version that no
    ///   longer exists; the watcher has already queued that write, so dropping
    ///   this result costs one poll tick and avoids showing text that is not on
    ///   disk.
    fn finish_reload(
        &mut self,
        file: LoadedFile,
        document: Document,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.dirty {
            self.mark_externally_changed(cx);
            return;
        }
        if !file.stamp.matches(&file.path) {
            return;
        }
        self.apply_reload(file, document, window, cx);
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
            if self.web_html.take().is_some() {
                self.web_revision = self.web_revision.wrapping_add(1);
            }
        }
    }

    fn rebuild_web(&mut self, cx: &mut Context<Self>) {
        self.web_revision = self.web_revision.wrapping_add(1);
        if self.document.source().len() > LIVE_PREVIEW_LIMIT {
            self.web_html = Some(oversize_notice(self.document.source().len()));
            return;
        }
        if self.document.doc_type() == DocType::Html {
            // **The sandbox boundary.** A `file://` document has a real origin,
            // so `<img src="logo.png">` and `<link rel=stylesheet>` resolve
            // against the file's own directory — and so does everything else
            // the user can read. That is why it is gated behind an explicit
            // Trust action, exactly as MDX script execution is.
            //
            // Restricted stores the file's own text instead, which the
            // workspace encodes as a `data:` URL like every other document:
            // that origin is opaque and cannot reach the filesystem at all.
            // Encoding it here as well would percent-encode the payload twice.
            //
            // A revision still reloads this URL after an edit, but `file://`
            // shows what is on disk rather than the unsaved editor buffer. A
            // truly live trusted preview would need a temporary-file protocol,
            // not a different cache key.
            self.web_html = Some(match self.trust {
                Trust::Trusted => web::to_file_url(&self.file.path),
                Trust::Restricted => web::build_html_raw(&self.document, self.trust),
            });
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

    /// The HTML — or, for a trusted HTML file, the `file://` URL — currently
    /// destined for the WebView.
    pub fn web_html(&self) -> Option<&str> {
        self.web_html.as_deref()
    }

    pub(crate) fn web_payload(&self) -> Option<(&str, u64)> {
        self.web_html
            .as_deref()
            .map(|html| (html, self.web_revision))
    }

    /// Lend this tab the window's WebView, or take it back.
    ///
    /// The workspace owns it — it is one OS child window per window, not per
    /// document — but only the tab currently rendering a Web pane may put it in
    /// its element tree, or two tabs would fight over its bounds.
    #[cfg(target_os = "windows")]
    pub(crate) fn set_webview(&mut self, webview: Option<WindowsWebView>, cx: &mut Context<Self>) {
        if self.webview == webview {
            return;
        }
        self.webview = webview;
        cx.notify();
    }

    #[cfg(target_os = "macos")]
    pub(crate) fn set_webview(
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
            .child({
                let current = self.layout;
                let available = available_layouts(doc_type);
                TabBar::new("layout-modes")
                    .segmented()
                    .selected_index(
                        available
                            .iter()
                            .position(|layout| *layout == current)
                            .unwrap_or(0),
                    )
                    .on_click(cx.listener(move |this, ix: &usize, _, cx| {
                        if let Some(layout) = available.get(*ix) {
                            this.set_layout(*layout, cx);
                        }
                    }))
                    .children(
                        available
                            .iter()
                            .map(|layout| Tab::new().label(i18n::t(layout.label_key(), cx))),
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
            // The two types Trust means something for, and it means a different
            // thing for each: MDX gains script execution, HTML gains filesystem
            // access. Both are things a cloned repository must not get without
            // the user asking.
            .when(matches!(doc_type, DocType::Mdx | DocType::Html), |this| {
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
                    // Cloned, never rebuilt: a fresh `MarkdownExtensions` here
                    // carries a new revision and defeats upstream's guard, which
                    // reparses the whole document every frame. See the field.
                    .markdown_extensions(self.preview_extensions.clone())
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
/// Give a rendered SVG the theme's foreground colour.
///
/// Renderers emit `fill="currentColor"` so one cached SVG can serve twelve
/// themes and follow the OS light/dark switch. That works in the Web pane
/// because `web.rs` sets `color` on the body — but the native pane hands the
/// markup to `Image::from_bytes(ImageFormat::Svg, …)`, which rasterizes through
/// usvg, and usvg resolves `currentColor` by walking for a `color` attribute and
/// **falling back to black** when it finds none (`parser/style.rs`). Measured:
/// an SVG with no `color` rasterizes to `(0, 0, 0)` and is invisible on the six
/// dark presets, which is what a first launch shows, since the default theme
/// preference is `System`.
///
/// gpui's `svg()` element tints through `style.text.color`, but `img()` — the
/// element that can display arbitrary SVG markup — has no colour handling at
/// all, so the colour has to be in the document.
///
/// Injected here rather than at render time on purpose: the renderer cache is
/// keyed on `(id, source)` with no theme in it, so baking a colour into the
/// cached string would serve the previous theme's colour after a switch. This
/// runs per frame on an already-rendered string and costs one `replacen`.
fn themed_svg(markup: &str, cx: &App) -> String {
    let fg = cx.theme().foreground.to_rgb();
    let ch = |c: f32| (c.clamp(0., 1.) * 255.).round() as u8;
    let color = format!("#{:02x}{:02x}{:02x}", ch(fg.r), ch(fg.g), ch(fg.b));
    // Only the root element, and only when it does not already say: a renderer
    // that sets its own `color` has made a deliberate choice.
    match markup.find("<svg") {
        Some(_) if markup[..markup.find('>').unwrap_or(markup.len())].contains("color=") => {
            markup.to_string()
        }
        Some(start) => {
            let insert = start + "<svg".len();
            format!(
                "{} color=\"{color}\"{}",
                &markup[..insert],
                &markup[insert..]
            )
        }
        None => markup.to_string(),
    }
}

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
                    themed_svg(markup, cx).into_bytes(),
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

/// A tab label derived from a buffer's first line.
///
/// Markdown heading markers are stripped: a document whose first line is
/// `# Design notes` is called "Design notes", not "# Design notes". The hash is
/// syntax, and a tab strip full of them reads as noise.
fn first_line_title(text: &str) -> String {
    let first = text
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or_default();
    let stripped = first.trim_start_matches('#').trim();
    if stripped.is_empty() {
        // Not "untitled": this is a label a person reads, and it is the same
        // word every editor uses for the same state.
        return "Untitled".to_string();
    }
    stripped.to_string()
}

/// Which syntax-highlighting language the editor uses for a file.
///
/// Keyed on the path, not the [`DocType`]: the doc type answers "how is this
/// document rendered", and every text file shares one answer to that while
/// needing a different grammar here.
fn editor_language(path: &std::path::Path) -> Language {
    // The Markdown family reaches this by name as often as by extension —
    // `AGENTS.md`, `*.instructions.md`, a `.mdc` cursor rule — and
    // gpui-component maps "mdx" onto the Markdown grammar anyway, so the whole
    // family resolves to the one language.
    if DocType::of(path).renders() {
        return Language::Markdown;
    }
    // No extension table of our own: `Language::from_str` already is one, over
    // exactly the grammars the workspace manifest enables, and it falls back to
    // `Plain` for anything it does not know. The file name is the second try
    // because `Path::extension` is `None` for a `Makefile`.
    //
    // That list is short by design and is chosen for *fence* languages — see
    // the comment on `gpui-component` in the workspace `Cargo.toml`. A source
    // file whose grammar is not compiled in opens in Source with no
    // highlighting, which is the intended outcome for a Markdown workspace:
    // highlighting other languages' files is not this app's job, but
    // highlighting a ```rust block inside a README is.
    let name = |part: Option<&std::ffi::OsStr>| {
        part.and_then(|p| p.to_str())
            .map(str::to_ascii_lowercase)
            .unwrap_or_default()
    };
    match Language::from_str(&name(path.extension())) {
        Language::Plain => Language::from_str(&name(path.file_name())),
        language => language,
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
        // Before the element chain: `title` reads the editor through `&App`,
        // which cannot overlap the `&mut Context` the chain holds.
        let title = self.title(cx);

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
            .aria_label(title)
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
    use super::{Layout, available_layouts, editor_language, first_line_title};
    use gpui_component::highlighter::Language;
    use mt_doc::DocType;
    use std::path::Path;

    /// This file's source between `signature` and the next `end` marker.
    ///
    /// The source-level checks below all need one function's body, and the
    /// hand-rolled `find`/slice pair was already repeated once per test.
    fn fn_body(signature: &str, end: &str) -> &'static str {
        let source = crate::views::production_source(include_str!("document.rs"));
        let start = source
            .find(signature)
            .unwrap_or_else(|| panic!("{signature} must exist"));
        let body = &source[start..];
        &body[..body.find(end).unwrap_or(body.len())]
    }

    #[test]
    fn a_buffer_is_named_by_its_first_line() {
        assert_eq!(first_line_title("Design notes\nbody\n"), "Design notes");
    }

    #[test]
    fn heading_markers_are_not_part_of_the_name() {
        // `# ` is syntax, and a tab strip full of hashes reads as noise.
        assert_eq!(first_line_title("# Design notes\n"), "Design notes");
        assert_eq!(first_line_title("### Deep\n"), "Deep");
    }

    #[test]
    fn leading_blank_lines_are_skipped() {
        // A buffer that starts with a blank line still has a name; taking
        // `lines().next()` literally would call it "Untitled".
        assert_eq!(first_line_title("\n\n  Real title\n"), "Real title");
    }

    #[test]
    fn a_buffer_with_nothing_to_say_is_untitled() {
        for text in ["", "\n\n\n", "   \n\t\n", "#\n", "###   \n"] {
            assert_eq!(
                first_line_title(text),
                "Untitled",
                "{text:?} should have no name to show"
            );
        }
    }

    /// The Web pane must place the `WebView` in the element tree.
    ///
    /// A source-level check, because the failure needs a real window with a
    /// real WebView2 runtime and is otherwise easy to reintroduce.
    #[test]
    fn the_web_pane_renders_the_webview_entity() {
        let source = crate::views::production_source(include_str!("document.rs"));
        let start = source
            .find("fn render_web_preview")
            .expect("the Web pane renderer");
        let body = &source[start..];
        let end = body.find("\n    fn render_preview").unwrap_or(body.len());
        let body = &body[..end];

        assert!(body.contains("self.webview"));
        assert!(source.contains("fn set_webview("));
    }

    /// The native preview must reuse one `MarkdownExtensions`, never build one
    /// in `render`.
    ///
    /// A source-level check because the failure needs a real window and a
    /// document over 4 KiB: below that threshold upstream parses synchronously
    /// and the waste is merely a full reparse per frame; above it the parse goes
    /// async, ends in `cx.notify()`, and the notify schedules the frame that
    /// starts the next parse. Measured on the release binary with no user input
    /// at all, a 4,200-byte document held 251% of a core indefinitely while a
    /// 4,000-byte one sat at 0.2%.
    ///
    /// The mechanism is upstream and invisible from here:
    /// `MarkdownExtensions::push_block_parser` calls `bump_revision`, which is
    /// `MARKDOWN_EXTENSIONS_REVISION.fetch_add(1, Relaxed)` on a process-global
    /// `AtomicU64`. `TextViewState::set_markdown_extensions` returns early only
    /// when the revision matches the one it holds, so a value built fresh in
    /// `render` can never match — while a `Clone` of one built once copies the
    /// revision and matches from the second frame on.
    /// The native pane must give a `currentColor` SVG a colour to inherit.
    ///
    /// usvg resolves `currentColor` by walking for a `color` attribute and
    /// falling back to **black** when it finds none, so an SVG that relies on
    /// inheritance rasterizes invisible on the six dark presets — which is what
    /// a first launch shows, since the default theme preference is `System`.
    /// Measured with usvg 0.45: no `color` gives `(0, 0, 0)`; `color="#e6edf3"`
    /// gives the light foreground, and an explicit `fill="#ff0000"` still wins.
    ///
    /// The Web pane is unaffected — `web.rs` sets `color` on the body — which is
    /// exactly why this was invisible in review: the same SVG is correct on one
    /// path and black on the other.
    #[test]
    fn the_native_pane_gives_a_currentcolor_svg_a_colour() {
        let source = crate::views::production_source(include_str!("document.rs"));

        let start = source
            .find("fn render_block")
            .expect("render_block must exist");
        let body = &source[start..];
        let end = body.find("\n/// ").unwrap_or(body.len());
        let body = &body[..end];
        assert!(
            body.contains("themed_svg(markup, cx)"),
            "`render_block` must theme the markup before handing it to \
             `Image::from_bytes`; usvg has no other way to resolve \
             `currentColor` and defaults it to black"
        );
        assert!(
            !body.contains("markup.clone().into_bytes()"),
            "handing the raw markup straight to usvg is the bug this replaces"
        );

        // And the injection itself must not clobber a renderer that already
        // chose a colour, and must not corrupt markup that has no `<svg` at all.
        let start = source.find("fn themed_svg").expect("themed_svg must exist");
        let body = &source[start..];
        let end = body.find("\n#[derive").unwrap_or(body.len());
        let body = &body[..end];
        assert!(
            body.contains(r#"contains("color=")"#),
            "an SVG that already carries a `color` has made a deliberate \
             choice and must be left alone"
        );
        assert!(
            body.contains("None => markup.to_string()"),
            "markup with no root element must pass through untouched rather \
             than being mangled by an offset into it"
        );
    }

    #[test]
    fn the_native_preview_does_not_rebuild_its_extensions_per_frame() {
        let source = crate::views::production_source(include_str!("document.rs"));
        let start = source
            .find("fn render_native_preview")
            .expect("the native preview renderer");
        let body = &source[start..];
        let end = body
            .find("\n    /// Inline diagnostics")
            .unwrap_or(body.len());
        let body = &body[..end];

        assert!(
            !body.contains("diagram_extensions("),
            "`render_native_preview` runs every frame and must not call \
             `diagram_extensions`: each call mints a new global revision, which \
             defeats upstream's `set_markdown_extensions` guard and reparses the \
             whole document every frame. Clone `self.preview_extensions` instead."
        );
        assert!(
            body.contains("self.preview_extensions.clone()"),
            "the preview must reuse the extensions built in `new`; `Clone` \
             copies the revision, which is what lets the guard match"
        );
        // And it is built exactly once, where the cost is paid per document
        // rather than per frame.
        assert_eq!(
            source.matches("diagram_extensions(registry").count(),
            1,
            "`diagram_extensions` must be called once, from `DocumentView::new`"
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
        let source = crate::views::production_source(include_str!("document.rs"));
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
        let source = crate::views::production_source(include_str!("document.rs"));
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
        let surface = crate::views::production_source(include_str!("workspace/web_surface.rs"));
        let start = surface.find("fn scroll_script").expect("scroll_script");
        let body = &surface[start..];
        let end = body.find("\n}").unwrap_or(body.len());
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
        let document = crate::views::production_source(include_str!("document.rs"));
        let start = document
            .find("fn scroll_preview_to")
            .expect("scroll_preview_to");
        let document = &document[start..];
        let end = document
            .find("\n    pub fn reveal_offset")
            .unwrap_or(document.len());
        let document = &document[..end];
        assert!(document.contains("DocumentEvent::ScrollWebPreview"));
        assert!(
            !document.contains("evaluate_script"),
            "render may queue a scroll, never call WebView2 directly"
        );
        assert!(
            body.contains("if(h>0)"),
            "a preview shorter than its viewport has nothing to scroll, and \
             dividing by its zero height would be a NaN offset"
        );
    }

    #[test]
    fn the_editor_highlights_a_file_by_its_extension() {
        // The bug this replaces: every file was opened with the Markdown
        // grammar, so a `.rs` was syntax-highlighted as prose.
        assert_eq!(editor_language(Path::new("src/main.rs")), Language::Rust);
        assert_eq!(editor_language(Path::new("app.py")), Language::Python);
        assert_eq!(
            editor_language(Path::new("pkg/package.json")),
            Language::Json
        );
        assert_eq!(editor_language(Path::new("Cargo.toml")), Language::Toml);
        // Uppercase on disk is the same language.
        assert_eq!(editor_language(Path::new("Main.RS")), Language::Rust);
    }

    #[test]
    fn the_whole_markdown_family_keeps_the_markdown_grammar() {
        // These are Markdown by name rather than by extension, and `.mdc` has
        // no grammar of its own at all — falling through to `from_str` would
        // highlight a cursor rule as plain text.
        for path in [
            "README.md",
            "AGENTS.md",
            "page.mdx",
            "x/rust.instructions.md",
            "repo/.cursor/rules/style.mdc",
        ] {
            assert_eq!(
                editor_language(Path::new(path)),
                Language::Markdown,
                "{path}"
            );
        }
    }

    #[test]
    fn a_file_with_no_grammar_falls_back_to_plain() {
        // Never a panic and never a wrong grammar: an unknown extension gets
        // no highlighting rather than someone else's.
        assert_eq!(editor_language(Path::new("data.qqq")), Language::Plain);
        assert_eq!(editor_language(Path::new("no_extension")), Language::Plain);
        // A conventional name the extension lookup cannot see, because
        // `Path::extension` is `None` for it.
        assert_eq!(editor_language(Path::new("Makefile")), Language::Make);
    }

    /// The fence languages a Markdown document actually contains must have a
    /// grammar compiled in.
    ///
    /// This is the test the manifest's grammar list exists for, and it guards a
    /// non-obvious dependency: a ```rust block inside a README is highlighted
    /// through the *same* cfg-gated grammars as a `.rs` file. `CodeBlock::styles`
    /// reaches `SyntaxHighlighter::new(lang)`, which asks
    /// `LanguageRegistry::singleton()`, which is built from `Language::all()`.
    /// So trimming a grammar because "markturbo does not highlight other
    /// languages' files" also un-highlights every fence in every document — the
    /// opposite of what a Markdown workspace wants.
    ///
    /// The list below is the fence languages measured across this repository's
    /// own Markdown, most frequent first. `Language::from_name` is `pub(crate)`
    /// upstream, so this goes through `from_str`, which is the same table.
    ///
    /// A failure here means someone removed a grammar feature from
    /// `Cargo.toml`. Restore it, or delete the fence language from this list
    /// deliberately — do not "fix" it by asserting `Plain`.
    #[test]
    fn every_fence_language_used_in_documents_has_a_grammar() {
        for (fence, expected) in [
            ("rust", Language::Rust),
            ("rs", Language::Rust),
            ("bash", Language::Bash),
            ("sh", Language::Bash),
            ("toml", Language::Toml),
            ("yaml", Language::Yaml),
            ("yml", Language::Yaml),
            ("json", Language::Json),
            ("python", Language::Python),
            ("py", Language::Python),
            ("js", Language::JavaScript),
            ("ts", Language::TypeScript),
            ("html", Language::Html),
            ("css", Language::Css),
            ("md", Language::Markdown),
        ] {
            assert_eq!(
                Language::from_str(fence),
                expected,
                "```{fence} lost its grammar — a fence in every document that \
                 uses it now renders as flat text. See the grammar list in the \
                 workspace Cargo.toml."
            );
        }

        // `text` is the deliberate no-highlighting fence and must stay Plain,
        // which is also what proves the assertions above are not vacuous.
        assert_eq!(Language::from_str("text"), Language::Plain);
    }

    /// A text file must open in the editor, not in an empty preview.
    ///
    /// Source-level: constructing a `DocumentView` needs a `Window`. What is
    /// being asserted is the wiring — `Layout::available_for` and
    /// `default_for` are unit-tested in `views/mod.rs`, and this is the call
    /// site that was hard-coded to `Layout::Native` for every document.
    #[test]
    fn a_document_opens_in_the_layout_its_type_defaults_to() {
        let source = crate::views::production_source(include_str!("document.rs"));
        let start = source.find("pub fn new(").expect("DocumentView::new");
        let body = &source[start..];
        let end = body.find("\n    pub fn path(").unwrap_or(body.len());
        let body = &body[..end];

        assert!(
            body.contains("Layout::default_for(doc_type)"),
            "hard-coding the opening layout opens a `.rs` in a preview pane \
             that has nothing to draw"
        );
        // And the type it defaults to for a text file is the editor.
        assert_eq!(Layout::default_for(DocType::Text), Layout::Source);
    }

    /// The fixed layout selector must not offer layouts that show nothing.
    #[test]
    fn the_layout_selector_is_fixed_and_offers_only_supported_modes() {
        let source = crate::views::production_source(include_str!("document.rs"));
        let start = source.find("fn render_toolbar").expect("render_toolbar");
        let body = &source[start..];
        let end = body
            .find("\n    /// The banner shown")
            .unwrap_or(body.len());
        let body = &body[..end];

        assert!(
            body.contains("available_layouts(doc_type)"),
            "iterating `Layout::ALL` offers a text file four layouts that \
             render an empty pane"
        );
        assert!(
            !body.contains("Layout::ALL"),
            "one of the two lists has to go, or they drift"
        );
        assert!(
            body.contains("TabBar::new(\"layout-modes\")") && body.contains(".segmented()"),
            "Web mode needs a fixed selector; a popup can be covered by the child HWND"
        );
        assert!(
            !body.contains(".dropdown_menu(") && !body.contains(".tooltip("),
            "the document toolbar must not create an overlay above the Web preview"
        );
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn windows_keeps_split_web_available() {
        assert!(available_layouts(DocType::Markdown).contains(&Layout::SplitWeb));
        let source = crate::views::production_source(include_str!("document.rs"));
        let start = source.find("pub fn set_layout").expect("set_layout");
        let body = &source[start..];
        let end = body.find("pub fn set_trust").unwrap_or(body.len());
        assert!(!body[..end].contains("platform_layout"));
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn windows_split_web_has_no_floating_editor_provider() {
        let source = crate::views::production_source(include_str!("document.rs"));
        assert!(source.contains(".searchable(true)"));
        for provider in ["CompletionProvider", "HoverProvider", "CodeActionProvider"] {
            assert!(
                !source.contains(provider),
                "{provider} needs a WebView overlay strategy before Windows SplitWeb can use it"
            );
        }
    }

    /// Trust is offered for both things it can unlock.
    ///
    /// Source-level for the same reason as the others — this is a render body.
    /// The two meanings are different (MDX gains scripts, HTML gains the
    /// filesystem) but the control is one, and HTML's was simply missing.
    #[test]
    fn the_trust_button_is_offered_to_html_as_well_as_mdx() {
        let source = crate::views::production_source(include_str!("document.rs"));
        let start = source.find("fn render_toolbar").expect("render_toolbar");
        let body = &source[start..];
        let end = body
            .find("\n    /// The banner shown")
            .unwrap_or(body.len());
        let body = &body[..end];

        assert!(
            body.contains("DocType::Mdx | DocType::Html"),
            "without the button an HTML file's relative images can never load"
        );
    }

    /// `file://` is reachable only for a document the user trusted.
    ///
    /// This is the security boundary, and it is the kind that is easy to widen
    /// by accident while refactoring: a `file://` page can read anything the
    /// user can. Source-level because the alternative needs a real WebView.
    #[test]
    fn only_a_trusted_document_is_given_filesystem_access() {
        let source = crate::views::production_source(include_str!("document.rs"));
        let start = source.find("fn rebuild_web").expect("rebuild_web");
        let body = &source[start..];
        let end = body
            .find("\n    /// Rebuild the Web payload")
            .unwrap_or(body.len());
        let body = &body[..end];

        let file_url = body.find("to_file_url").expect("the trusted path");
        let trusted = body.find("Trust::Trusted =>").expect("the trust match");
        assert!(
            trusted < file_url,
            "`to_file_url` must sit under the `Trusted` arm"
        );
        assert!(
            body.contains("Trust::Restricted =>") && body.contains("build_html_raw"),
            "restricted HTML must be served as text for the workspace to turn \
             into an opaque-origin `data:` URL, which cannot reach the \
             filesystem at all"
        );
        assert!(
            !body.contains("to_data_url"),
            "the workspace already encodes the payload; doing it here too \
             percent-encodes it twice and shows the user a URL as text"
        );
        assert!(
            body.contains("DocType::Html"),
            "HTML must not go through the themed shell, which would nest a \
             whole document inside another one"
        );
    }

    /// Auto-refresh must never discard typed text.
    ///
    /// Source-level: the guard's whole job is to return *before* it starts any
    /// work, and there is no observable state to assert that against — a
    /// document that refused is byte-identical to one nothing happened to.
    #[test]
    fn reload_if_clean_refuses_to_touch_a_dirty_document() {
        let body = fn_body(
            "pub fn reload_if_clean",
            "\n    /// Apply a background reload",
        );

        let guard = body.find("if self.dirty").expect("the dirty guard");
        let spawn = body.find("cx.spawn(").expect("the reload task");
        assert!(
            guard < spawn,
            "the guard must come first, or an automatic refresh discards \
             unsaved edits before anyone can object"
        );
        assert!(
            body[guard..spawn].contains("return false"),
            "a dirty document must change nothing and say so, which is what \
             leaves the banner up"
        );
    }

    /// The automatic reload must not parse on the UI thread.
    ///
    /// The defect this replaces: `drain_watcher` -> `reload_if_clean` ->
    /// `reload` -> `Document::set_source` -> `reparse`, all inline. That parse
    /// is markdown-rs and superlinear — **measured at 23.4s on
    /// `fixtures/perf/huge-100k.md`** in a release build, 665ms on a 1MB file —
    /// and it froze the window for the whole of it. The watcher fires on every
    /// external write, so an agent rewriting a file in a loop froze the window
    /// once per poll tick. `mt-doc/tests/performance.rs`'s
    /// `a_huge_document_is_slow_enough_to_require_background_parsing` asserts
    /// the parse stays slow precisely so nobody moves it back here.
    ///
    /// Source-level for the same reason as its neighbours: reproducing it needs
    /// a real window, a 100K-line file, and a stopwatch on the frame time.
    #[test]
    fn the_automatic_reload_parses_off_the_ui_thread() {
        let body = fn_body(
            "pub fn reload_if_clean",
            "\n    /// Apply a background reload",
        );

        assert!(
            body.contains("background_spawn"),
            "the read and the parse must run off the UI thread; inline they \
             freeze the window for 23.4s on the 100K-line fixture, once per \
             external write"
        );
        assert!(
            !body.contains("self.reload("),
            "`reload` is the synchronous, user-initiated path — routing the \
             watcher through it is the freeze this test exists to prevent"
        );
        assert!(
            body.contains("try_update_in"),
            "the result lands after an await, where the infallible borrow \
             panics mid-draw"
        );
    }

    /// A background reload lands into a document that may have moved on.
    ///
    /// Both checks are one-line and both are load-bearing: without the second
    /// dirty check a reload started while clean clobbers text typed during the
    /// parse — the exact data loss `reload_if_clean` exists to prevent — and
    /// without the stamp check the editor shows a version that is no longer on
    /// disk.
    #[test]
    fn a_landing_reload_rechecks_the_document_and_the_file() {
        let body = fn_body("fn finish_reload", "\n    /// Save to disk");

        let dirty = body.find("if self.dirty").expect("the second dirty check");
        let apply = body.find("self.apply_reload").expect("the apply");
        assert!(
            dirty < apply,
            "the user can start typing during the parse; applying without \
             re-checking discards what they typed"
        );
        assert!(
            body[dirty..apply].contains("mark_externally_changed"),
            "a document that went dirty mid-parse must still get its banner, \
             or the change goes unnoticed"
        );
        assert!(
            body.contains("stamp.matches"),
            "a result parsed from a version that has since been overwritten is \
             stale; the watcher has already queued the newer write"
        );
    }

    /// The manual reload stays synchronous, and must stay cheap to tell apart.
    #[test]
    fn the_manual_reload_is_the_one_the_banner_button_calls() {
        let source = crate::views::production_source(include_str!("document.rs"));
        let start = source
            .find("fn render_conflict_banner")
            .expect("the banner");
        let body = &source[start..];
        let end = body.find("\n    fn render_editor").unwrap_or(body.len());
        assert!(
            body[..end].contains("this.reload(window, cx)"),
            "the banner's button is user-initiated and one-shot, so it keeps \
             the synchronous path; the watcher is the one that must not"
        );
    }
}
