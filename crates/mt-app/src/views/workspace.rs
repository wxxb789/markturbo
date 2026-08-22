//! The workspace: explorer + skills + outline on the left, tabbed documents on
//! the right.
//!
//! Owns the open-document set, the filesystem watcher, the Web preview surface,
//! and the commands (open folder, save, translate). Individual views stay
//! narrow; this is where they are wired together.
//!
//! Two clusters live in submodules because neither belongs to the wiring.
//! `history` is plain data plus the two buttons that read it; `web_surface`
//! is the OS child window and the re-entrancy rules for touching it. Both add
//! their methods to `Workspace` from there, so `self.web_dirty(cx)` reads the
//! same here as it did before the split.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use gpui::prelude::FluentBuilder as _;
use gpui::*;
use gpui_component::{
    ActiveTheme as _, Icon, IconName, Sizable as _, StyledExt as _, TitleBar,
    button::{Button, ButtonVariants as _},
    h_flex,
    list::ListItem,
    menu::ContextMenuExt as _,
    resizable::{h_resizable, resizable_panel},
    tab::{Tab, TabBar},
    tooltip::Tooltip,
    v_flex,
};
use mt_doc::translate::Scope;

use crate::fs;
use crate::i18n;
use crate::metrics;
use crate::renderer::RendererRegistry;
use crate::translate::Provider;
use crate::views::document::{DocumentEvent, DocumentView};
use crate::views::explorer::{Explorer, ExplorerEvent};
use crate::views::harness::{HarnessEvent, HarnessView};
use crate::views::search::{Corpus, SearchEvent, SearchView};
use crate::views::settings_page::{SettingsEvent, SettingsView};
use crate::views::tabs::Tabs;
use crate::watcher::Watcher;

mod history;
mod web_surface;

use self::history::History;
use self::web_surface::WebSurface;

actions!(
    markturbo,
    [
        OpenFolder,
        Save,
        CloseTab,
        OpenSettings,
        TranslateDocument,
        TranslateSelection,
        TranslateBlock,
        CopyPath,
        CopyRelativePath,
        ToggleLeftPanel,
        ToggleRightPanel,
        FocusSearch,
        NavigateBack,
        NavigateForward
    ]
);

/// The longest tab label before it is elided.
///
/// Long enough for `architecture.md` and most agent-artifact names, short
/// enough that six open documents still fit across a laptop window. A tab that
/// grows to its file name pushes every other tab off the bar, which is the
/// failure this bounds — the full path is a hover away.
const TAB_LABEL_MAX: usize = 22;

/// Shorten `name` to [`TAB_LABEL_MAX`], keeping the extension.
///
/// The extension is what distinguishes `notes.md` from `notes.mdx`, so eliding
/// from the end — the obvious implementation — removes exactly the part worth
/// keeping. This elides the stem instead.
fn elide_tab_label(name: &str) -> String {
    let count = name.chars().count();
    if count <= TAB_LABEL_MAX {
        return name.to_string();
    }
    let (stem, ext) = match name.rfind('.') {
        // A leading dot is a hidden file, not an extension.
        Some(ix) if ix > 0 => (&name[..ix], &name[ix..]),
        _ => (name, ""),
    };
    let ext_len = ext.chars().count();
    // Keep at least a few characters of the stem, even beside a long extension.
    let keep = TAB_LABEL_MAX.saturating_sub(ext_len + 1).max(3);
    let head: String = stem.chars().take(keep).collect();
    format!("{head}…{ext}")
}

/// Which left-panel section is showing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SidePanel {
    Files,
    Search,
    Harness,
    Outline,
}

impl SidePanel {
    const ALL: [SidePanel; 4] = [
        SidePanel::Files,
        SidePanel::Search,
        SidePanel::Harness,
        SidePanel::Outline,
    ];

    /// The string key for this panel, resolved against the chosen language
    /// at render time rather than baked in here.
    fn label(self) -> crate::i18n::Key {
        match self {
            SidePanel::Files => crate::i18n::Key::PanelFiles,
            SidePanel::Search => crate::i18n::Key::PanelSearch,
            SidePanel::Harness => crate::i18n::Key::PanelHarness,
            SidePanel::Outline => crate::i18n::Key::PanelOutline,
        }
    }
}

/// How often to drain the filesystem watcher.
///
/// The watcher itself is already debounced; this only governs how quickly a
/// detected change reaches the UI.
const WATCH_POLL: Duration = Duration::from_millis(500);

/// How long a status message stays on the bar.
///
/// Long enough to read a save confirmation without looking for it, short enough
/// that the bar is not still claiming "Saved" when the user comes back.
const STATUS_LINGER: Duration = Duration::from_secs(6);

pub struct Workspace {
    focus_handle: FocusHandle,
    root: Option<PathBuf>,
    explorer: Option<Entity<Explorer>>,
    harness: Option<Entity<HarnessView>>,
    search: Entity<SearchView>,
    /// The settings page, built once rather than per open.
    ///
    /// Eager like [`Self::search`] and for the same reason: the subscription
    /// that carries its events has to be set up somewhere, and a lazily-created
    /// entity would mean re-subscribing on every open — the pattern that leaks
    /// a subscription per click. It is stateless, so an unopened one costs a
    /// focus handle.
    settings: Entity<SettingsView>,
    side_panel: SidePanel,
    /// The open tabs, the active one, the preview slot, and the menu target.
    ///
    /// One field rather than five: the index arithmetic between them is where
    /// closing a tab used to switch documents, leak subscriptions, and strand
    /// the preview slot. [`Tabs`] owns those rules and tests them without a
    /// window.
    tabs: Tabs<DocumentTab>,
    /// Back/forward across visited positions.
    history: History,
    registry: Arc<RendererRegistry>,
    watcher: Option<Watcher>,
    status: Option<String>,
    /// Bumped by every [`Workspace::set_status`], so a timer can tell whether
    /// the message it was started for is still the one on screen.
    status_generation: u64,
    /// The timer that clears the current status message.
    ///
    /// One slot rather than a detached task per message: replacing it cancels
    /// the previous timer, which is the other half of the generation check.
    _status_timer: Option<Task<()>>,
    /// The window's single WebView and what it is showing.
    ///
    /// One field rather than three, and no `#[cfg]` here: the platform split
    /// is inside [`WebSurface`], which is empty on Linux. That is what keeps
    /// the dozen `web_dirty` call sites free of one.
    web: WebSurface,
    /// True while the settings page is showing.
    settings_open: bool,
    /// True while the file/harness/outline panel is showing on the left.
    left_panel_open: bool,
    /// True while the details panel is showing on the right.
    ///
    /// Not derived from whether anything is selected: a panel that appeared and
    /// vanished as the selection changed would resize the document under the
    /// user's cursor.
    right_panel_open: bool,
    /// True while a translation request is in flight.
    ///
    /// The button reads it through `loading`, which also makes it inert — a
    /// second request would overwrite the editor twice with two different
    /// answers to the same text.
    translating: bool,
    _tasks: Vec<Task<()>>,
    /// Subscriptions that live as long as the workspace does.
    ///
    /// Per-document subscriptions are *not* here — they ride in
    /// [`DocumentTab`], so closing a tab drops them with it.
    _subscriptions: Vec<Subscription>,
    /// Subscriptions to the current folder's explorer and skills views,
    /// replaced wholesale when the folder changes.
    _panel_subscriptions: Vec<Subscription>,
}

/// What an open tab carries besides its path.
///
/// The subscriptions live here rather than in a workspace-wide `Vec` because
/// that `Vec` was only ever appended to: closing a tab removed the document and
/// left two subscriptions to it alive for the rest of the session.
struct DocumentTab {
    view: Entity<DocumentView>,
    _subscriptions: [Subscription; 2],
}

impl Workspace {
    /// Every open document, cloned.
    ///
    /// Cloned because every caller is about to `update` each one through the
    /// same `&mut Context` that borrows `self`.
    fn document_views(&self) -> Vec<Entity<DocumentView>> {
        self.tabs.iter().map(|t| t.payload.view.clone()).collect()
    }

    /// The document in the tab at `ix`.
    fn document_at(&self, ix: usize) -> Option<&Entity<DocumentView>> {
        self.tabs.get(ix).map(|t| &t.payload.view)
    }
}

impl Workspace {
    /// Create the workspace, opening `initial` if given.
    pub fn new(initial: Option<PathBuf>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        // Poll the watcher on a timer rather than blocking a thread on it: the
        // receiver is non-blocking and a UI tick is the natural cadence.
        let poll = cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(WATCH_POLL).await;
                if this.upgrade().is_none() {
                    break;
                }
                // A skipped tick is harmless: the watcher queue is drained on
                // the next one. A panic here would take the window with it.
                crate::views::try_update(&this, cx, |this, cx| this.drain_watcher(cx));
            }
        });

        let mut this = Self {
            focus_handle: cx.focus_handle(),
            root: None,
            explorer: None,
            harness: None,
            search: cx.new(|cx| SearchView::new(window, cx)),
            settings: cx.new(SettingsView::new),
            side_panel: SidePanel::Files,
            tabs: Tabs::default(),
            history: History::default(),
            registry: Arc::new(RendererRegistry::with_defaults()),
            watcher: None,
            status: None,
            status_generation: 0,
            _status_timer: None,
            settings_open: false,
            left_panel_open: true,
            right_panel_open: true,
            translating: false,
            web: WebSurface::default(),
            _tasks: vec![poll],
            _subscriptions: Vec::new(),
            _panel_subscriptions: Vec::new(),
        };

        // The saved preference, applied before the first frame so the window
        // never flashes the wrong theme.
        crate::settings::apply_theme(
            crate::settings::AppSettings::global(cx).theme,
            Some(window),
            cx,
        );

        // The search view cannot gather its own corpus — the open tabs' text is
        // in their editors and the harness paths in the harness view — so it
        // asks, and this answers.
        let search = this.search.clone();
        this._subscriptions.push(cx.subscribe_in(
            &search,
            window,
            |this: &mut Self, _, event: &SearchEvent, window, cx| match event {
                SearchEvent::Ready => {
                    let corpus = this.search_corpus(cx);
                    this.search.update(cx, |search, cx| search.run(corpus, cx));
                }
                SearchEvent::Reveal { path, offset } => {
                    this.reveal_in(path.clone(), *offset, window, cx);
                }
            },
        ));
        // The page writes the setting; what it cannot do is repaint the rest of
        // the app. Each event names what changed, and the response is chosen
        // here — the WebView caches HTML with the palette baked in, which is not
        // something a settings page should have to know.
        let settings = this.settings.clone();
        this._subscriptions.push(cx.subscribe(
            &settings,
            |this: &mut Self, _, event: &SettingsEvent, cx| match event {
                SettingsEvent::ThemeChanged => this.reapply_theme(cx),
                SettingsEvent::LanguageChanged => this.relabel(cx),
                SettingsEvent::SkillScopeChanged => this.rescan_harness(cx),
            },
        ));
        // And the backstop, for the writers that are not the settings page.
        //
        // The status bar's watching toggle and the Harness panel's group-by
        // button both call `AppSettings::update` directly, and neither emits a
        // `SettingsEvent` — so before this, a setting changed from anywhere but
        // the settings page repainted only whichever view happened to own the
        // control. `global_mut` already pushes `NotifyGlobalObservers`, so
        // subscribing is the whole of what was missing.
        //
        // A plain redraw, not `relabel`: this fires for *every* settings write,
        // including the ones the subscription above is about to handle
        // specifically, and doing the expensive work twice would make each
        // dropdown change rebuild every open document's HTML.
        this._subscriptions.push(
            cx.observe_global::<crate::settings::AppSettings>(|this, cx| {
                let _ = &this;
                cx.notify();
            }),
        );
        // Following the system means following it *while running*, not only at
        // startup — someone whose OS flips at sunset expects the app to flip
        // with it. An explicit preference ignores the event.
        let handle = cx.entity().downgrade();
        this._subscriptions
            .push(window.observe_window_appearance(move |window, cx| {
                if crate::settings::AppSettings::global(cx).theme
                    != crate::settings::ThemePreference::System
                {
                    return;
                }
                crate::settings::apply_theme(
                    crate::settings::ThemePreference::System,
                    Some(window),
                    cx,
                );
                // The Web preview caches HTML with the scheme baked in, so
                // recoloring GPUI alone would leave it on the old theme.
                if let Some(this) = handle.upgrade() {
                    this.update(cx, |this, cx| {
                        for doc in this.document_views() {
                            doc.update(cx, |doc, cx| doc.theme_changed(cx));
                        }
                        this.web_dirty(cx);
                    });
                }
            }));

        // A path argument may name a file: open its parent as the workspace and
        // the file as the first tab, which is what "open this file with
        // markturbo" should do.
        if let Some(path) = initial {
            let (root, file) = if path.is_dir() {
                (Some(path), None)
            } else {
                (path.parent().map(Path::to_path_buf), Some(path))
            };
            if let Some(root) = root {
                this.open_folder(root, window, cx);
            }
            if let Some(file) = file.filter(|f| f.is_file()) {
                this.open_file(file, window, cx);
            }
        }
        this
    }

    /// Open `path` as the workspace root.
    pub fn open_folder(&mut self, path: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        if !path.is_dir() {
            self.set_status(format!("Not a directory: {}", path.display()), cx);
            return;
        }

        let explorer = cx.new(|cx| Explorer::new(path.clone(), window, cx));
        let harness = cx.new(|cx| HarnessView::new(path.clone(), cx));

        // Kept apart from `_subscriptions`: this set is replaced wholesale on
        // every folder change, and folding it into the general one would take
        // the window's appearance observer and every open document's observer
        // down with it.
        self._panel_subscriptions = vec![
            cx.subscribe_in(
                &explorer,
                window,
                |this: &mut Self, _, event: &ExplorerEvent, window, cx| {
                    let ExplorerEvent::OpenFile { path, preview } = event;
                    this.open_file_as(path.clone(), *preview, window, cx);
                },
            ),
            cx.subscribe_in(
                &harness,
                window,
                |this: &mut Self, _, event: &HarnessEvent, window, cx| {
                    let HarnessEvent::OpenFile { path, preview } = event;
                    this.open_file_as(path.clone(), *preview, window, cx);
                },
            ),
        ];

        self.watcher = match Watcher::new(&path) {
            Ok(watcher) => Some(watcher),
            Err(err) => {
                log::warn!("filesystem watching unavailable: {err}");
                None
            }
        };

        self.explorer = Some(explorer);
        self.harness = Some(harness);
        self.root = Some(path);
        // Any results on screen came from the folder that was open a moment
        // ago. Leaving them would present another project's matches as this
        // one's, which is worse than an empty list.
        let search = self.search.clone();
        search.update(cx, |search, cx| search.rerun(cx));
        cx.notify();
    }

    /// Open a file in a tab, focusing an existing tab if it is already open.
    ///
    /// A pinned open: the tab stays until the user closes it.
    pub fn open_file(&mut self, path: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        self.open_file_as(path, false, window, cx);
    }

    /// Open a file, optionally as a preview.
    ///
    /// A preview reuses one slot: opening another preview replaces it rather
    /// than adding a tab, which is what keeps clicking through a tree from
    /// leaving a bar full of documents nobody asked to keep.
    pub fn open_file_as(
        &mut self,
        path: PathBuf,
        preview: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // Opening the file that is already the preview, by double click, is how
        // it gets promoted — the tab is already right, only its status changes.
        if preview {
            if self.tabs.is_preview(&path) {
                self.focus_path(&path, cx);
                return;
            }
        } else if self.tabs.is_preview(&path) {
            self.tabs.set_preview(None);
            self.focus_path(&path, cx);
            return;
        }

        // Replace the outgoing preview rather than accumulating tabs. Its edits
        // are the one thing that must not be discarded silently, so a dirty
        // preview is kept and simply stops being one.
        if preview
            && let Some(current) = self.tabs.take_preview()
            && let Some(ix) = self.tabs.index_of(&current)
        {
            if self
                .tabs
                .get(ix)
                .is_some_and(|t| t.payload.view.read(cx).is_dirty())
            {
                // Keep it: promoting beats losing unsaved work.
            } else {
                self.close_tab(ix, cx);
            }
        }

        self.open_file_inner(path.clone(), window, cx);
        self.tabs.set_preview(preview.then_some(path));
        cx.notify();
    }

    /// Focus the tab showing `path`, if it is open.
    fn focus_path(&mut self, path: &Path, cx: &mut Context<Self>) {
        if self.tabs.focus_path(path) {
            self.record_visit(path.to_path_buf(), 0);
            self.web_dirty(cx);
            cx.notify();
        }
    }

    fn open_file_inner(&mut self, path: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        // Opening a document while settings are showing has to show the
        // document, or the click in the explorer looks like it did nothing.
        self.settings_open = false;

        if self.tabs.focus_path(&path) {
            self.record_visit(path, 0);
            self.web_dirty(cx);
            cx.notify();
            return;
        }

        let file = match fs::load(&path) {
            Ok(file) => file,
            Err(err) => {
                self.set_status(format!("Cannot open {}: {err}", path.display()), cx);
                return;
            }
        };

        let registry = self.registry.clone();
        let view = cx.new(|cx| DocumentView::new(file, registry, window, cx));

        // Both subscriptions ride with the tab, so closing it drops them.
        let subscriptions = [
            cx.subscribe_in(
                &view,
                window,
                |this: &mut Self, _, event: &DocumentEvent, _, cx| match event {
                    DocumentEvent::Status(message) => this.set_status(message.clone(), cx),
                    DocumentEvent::Conflict => this.set_status(
                        "This file changed on disk. Reload or overwrite from the banner.".into(),
                        cx,
                    ),
                    DocumentEvent::DirtyChanged => cx.notify(),
                },
            ),
            // A document notifies on mode change, trust change, and after a
            // reparse — every event that can alter what the WebView should
            // show. Observing is what replaces the old sync-from-render.
            cx.observe(&view, |this, _, cx| {
                this.web_dirty(cx);
            }),
        ];

        self.tabs.push(
            path.clone(),
            DocumentTab {
                view,
                _subscriptions: subscriptions,
            },
        );
        self.record_visit(path, 0);
        self.web_dirty(cx);
        cx.notify();
    }

    fn close_tab(&mut self, ix: usize, cx: &mut Context<Self>) {
        // ponytail: closing a dirty tab drops its edits. A confirm dialog is
        // the obvious next step; the file on disk is never touched either way.
        //
        // `Tabs::close` also shifts the active index, empties the preview slot
        // if it named this tab, and drops the tab's two subscriptions with it.
        let Some((closed, _dropped)) = self.tabs.close(ix) else {
            return;
        };
        // Otherwise Back reopens the tab that was just closed, which reads as
        // the close button not working.
        self.history.forget(&closed);
        self.web_dirty(cx);
        cx.notify();
    }

    /// Redraw everything after the interface language changed.
    ///
    /// Labels are resolved from the string table during render, so nothing is
    /// cached — but a view only redraws when it is notified, and the panels are
    /// separate entities that did not observe the settings change.
    fn relabel(&mut self, cx: &mut Context<Self>) {
        if let Some(explorer) = &self.explorer {
            explorer.update(cx, |_, cx| cx.notify());
        }
        if let Some(harness) = &self.harness {
            harness.update(cx, |_, cx| cx.notify());
        }
        // The search view resolves its own labels through `i18n::t` at render
        // time — its scope names, its "no matches" line — and it was missing
        // from this list, so switching language left that one panel in the old
        // one until something else happened to redraw it.
        self.search.update(cx, |_, cx| cx.notify());
        for doc in self.document_views() {
            doc.update(cx, |_, cx| cx.notify());
        }
        cx.notify();
    }

    /// Rediscover skills, e.g. after a setting changed what is in scope.
    fn rescan_harness(&mut self, cx: &mut Context<Self>) {
        if let Some(harness) = &self.harness {
            harness.update(cx, |harness, cx| harness.refresh(cx));
        }
    }

    /// Re-resolve the saved theme and repaint everything that caches it.
    ///
    /// The Web preview renders in its own browser context and caches its HTML
    /// with the palette baked in, so it does not pick up a GPUI theme change on
    /// its own. Called for both a mode change and a preset change — the two are
    /// separate settings that land in the same place, and the settings page has
    /// already written whichever one moved.
    fn reapply_theme(&mut self, cx: &mut Context<Self>) {
        let preference = crate::settings::AppSettings::global(cx).theme;
        crate::settings::apply_theme(preference, None, cx);
        for doc in self.document_views() {
            doc.update(cx, |doc, cx| doc.theme_changed(cx));
        }
        self.web_dirty(cx);
        cx.notify();
    }

    fn active_document(&self) -> Option<&Entity<DocumentView>> {
        self.document_at(self.tabs.active_index())
    }

    fn set_status(&mut self, message: String, cx: &mut Context<Self>) {
        // Each message gets its own generation, and only the timer whose
        // generation is still current clears the bar. Without it, two messages
        // inside the window meant the first message's timer wiped the second
        // one off the screen early — and every message spawned a task that
        // outlived its own relevance.
        self.status_generation = self.status_generation.wrapping_add(1);
        let generation = self.status_generation;
        self.status = Some(message);
        cx.notify();
        // Clear after a few seconds so the bar does not hold a stale message.
        self._status_timer = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(STATUS_LINGER).await;
            crate::views::try_update(&this, cx, |this, cx| {
                if this.status_generation == generation {
                    this.status = None;
                    cx.notify();
                }
            });
        }));
    }

    /// Apply pending filesystem changes.
    fn drain_watcher(&mut self, cx: &mut Context<Self>) {
        let Some(watcher) = &self.watcher else { return };
        let changes = watcher.poll();
        if changes.is_empty() {
            return;
        }

        let tree_changed = changes.iter().any(|c| c.affects_tree());
        // Skills live under `skills`-named directories; instruction files are
        // named for what they instruct, so both spellings have to be watched or
        // editing a CLAUDE.md would never refresh the panel listing it.
        let harness_changed = changes.iter().any(|c| {
            let path = c.path().to_string_lossy().to_lowercase();
            path.contains("skill")
                || path.contains("agents.md")
                || path.contains("claude.md")
                || path.contains("instructions.md")
                || path.contains("rules")
        });

        // Every open document whose file changed. With auto-reload off, the
        // flag and its banner are the whole response — the user's unsaved edits
        // are theirs to keep or discard. With it on, a *clean* document is
        // re-read; a dirty one is not, and `reload_if_clean` saying so is
        // exactly the signal that the banner is still needed. Automatic refresh
        // must never discard typed text.
        //
        // `reload_if_clean` returns whether it *started*: the read and the parse
        // run on a background task, because markdown-rs is superlinear and this
        // fires on every external write. A document that goes dirty while that
        // parse runs is flagged by the task itself when the result lands.
        let auto_reload = crate::settings::AppSettings::global(cx).watch_auto_reload;
        for change in &changes {
            let path = change.path();
            // Cloned: `self.documents` cannot stay borrowed across the `&mut cx`
            // that leasing each entity takes.
            for doc in self.document_views() {
                if doc.read(cx).path() != path {
                    continue;
                }
                let reloaded = auto_reload && doc.update(cx, |doc, cx| doc.reload_if_clean(cx));
                if !reloaded {
                    doc.update(cx, |doc, cx| doc.mark_externally_changed(cx));
                }
            }
        }

        if tree_changed && let Some(explorer) = &self.explorer {
            explorer.update(cx, |explorer, cx| explorer.refresh(cx));
        }
        if (tree_changed || harness_changed)
            && let Some(harness) = &self.harness
        {
            harness.update(cx, |harness, cx| harness.refresh(cx));
        }
        cx.notify();
    }

    // --- Actions ----------------------------------------------------------

    fn on_open_folder(&mut self, _: &OpenFolder, window: &mut Window, cx: &mut Context<Self>) {
        let paths = cx.prompt_for_paths(PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: Some("Open workspace folder".into()),
        });
        cx.spawn_in(window, async move |this, cx| {
            // The prompt future nests: cancelled -> failed -> no selection.
            let Some(path) = paths
                .await
                .ok()
                .and_then(Result::ok)
                .flatten()
                .and_then(|paths| paths.first().cloned())
            else {
                return;
            };
            crate::views::try_update_in(&this, cx, |this, window, cx| {
                this.open_folder(path, window, cx);
            });
        })
        .detach();
    }

    fn on_save(&mut self, _: &Save, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(doc) = self.active_document().cloned() {
            doc.update(cx, |doc, cx| doc.save(false, cx));
        }
    }

    fn on_close_tab(&mut self, _: &CloseTab, _: &mut Window, cx: &mut Context<Self>) {
        self.close_tab(self.tabs.active_index(), cx);
    }

    fn on_open_settings(&mut self, _: &OpenSettings, _: &mut Window, cx: &mut Context<Self>) {
        self.settings_open = !self.settings_open;
        // The WebView's visibility depends on this flag, and it is an OS child
        // window that will not notice a re-render on its own.
        self.web_dirty(cx);
        cx.notify();
    }

    fn on_toggle_left_panel(
        &mut self,
        _: &ToggleLeftPanel,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.left_panel_open = !self.left_panel_open;
        // The WebView is an OS child window; it does not notice the document
        // pane resizing under it.
        self.web_dirty(cx);
        cx.notify();
    }

    fn on_toggle_right_panel(
        &mut self,
        _: &ToggleRightPanel,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.right_panel_open = !self.right_panel_open;
        self.web_dirty(cx);
        cx.notify();
    }

    /// Open files dropped onto the window.
    ///
    /// A directory becomes the workspace; documents open as pinned tabs, since
    /// dragging a file in is as deliberate as double-clicking one. Anything the
    /// document pipeline cannot show is reported rather than silently ignored —
    /// a drop that appears to do nothing reads as a broken window.
    fn on_drop_paths(&mut self, paths: &[PathBuf], window: &mut Window, cx: &mut Context<Self>) {
        let mut opened = 0usize;
        let mut skipped: Vec<String> = Vec::new();

        for path in paths {
            if path.is_dir() {
                self.open_folder(path.clone(), window, cx);
                opened += 1;
            } else if crate::workspace::is_openable(path) {
                // With no folder open, adopt the file's parent as the
                // workspace — the same thing a path argument does, and it is
                // what makes the tree useful after a bare drop.
                if self.root.is_none()
                    && let Some(parent) = path.parent()
                {
                    self.open_folder(parent.to_path_buf(), window, cx);
                }
                self.open_file(path.clone(), window, cx);
                opened += 1;
            } else {
                skipped.push(
                    path.file_name()
                        .and_then(|n| n.to_str())
                        .unwrap_or_default()
                        .to_string(),
                );
            }
        }

        if opened == 0 && !skipped.is_empty() {
            self.set_status(format!("Cannot open {}", skipped.join(", ")), cx);
        }
    }

    /// The path of whichever tab the context menu belongs to.
    ///
    /// Falls back to the active tab: the menu is also reachable by keybinding,
    /// where no tab was right-clicked. [`Tabs`] is what makes that fallback
    /// real — it drops the recorded index when the menu closes and when the tab
    /// list changes, so a stale index can never answer here.
    fn menu_target(&self, cx: &App) -> Option<PathBuf> {
        let _ = cx;
        self.tabs.menu_target().map(|t| t.path.clone())
    }

    fn on_copy_path(&mut self, _: &CopyPath, _: &mut Window, cx: &mut Context<Self>) {
        let Some(path) = self.menu_target(cx) else {
            return;
        };
        let text = path.to_string_lossy().replace(char::from(92), "/");
        cx.write_to_clipboard(ClipboardItem::new_string(text.clone()));
        self.set_status(format!("Copied {text}"), cx);
    }

    fn on_copy_relative_path(
        &mut self,
        _: &CopyRelativePath,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(path) = self.menu_target(cx) else {
            return;
        };
        // Without a folder open there is nothing to be relative *to*, so this
        // reports rather than silently copying the absolute path — which would
        // look like the other menu item misbehaving.
        let Some(root) = self.root.clone() else {
            self.set_status("No folder is open, so there is no relative path".into(), cx);
            return;
        };
        let Ok(rest) = path.strip_prefix(&root) else {
            self.set_status("That file is outside the open folder".into(), cx);
            return;
        };
        let text = rest.to_string_lossy().replace(char::from(92), "/");
        cx.write_to_clipboard(ClipboardItem::new_string(text.clone()));
        self.set_status(format!("Copied {text}"), cx);
    }

    fn on_translate_document(
        &mut self,
        _: &TranslateDocument,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // The button goes inert while `translating`, but the keybinding does
        // not — and two requests over the same text race to overwrite the
        // editor with two different answers.
        if self.translating {
            return;
        }
        self.translate(Scope::Document, window, cx);
    }

    fn on_translate_selection(
        &mut self,
        _: &TranslateSelection,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.translating {
            return;
        }
        let Some(doc) = self.active_document() else {
            return;
        };
        let range = doc.read(cx).selection(cx);
        if range.is_empty() {
            self.set_status("Select some text first".into(), cx);
            return;
        }
        self.translate(Scope::Selection(range), window, cx);
    }

    fn on_translate_block(
        &mut self,
        _: &TranslateBlock,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.translating {
            return;
        }
        let Some(doc) = self.active_document() else {
            return;
        };
        let cursor = doc.read(cx).cursor(cx);
        self.translate(Scope::Block(cursor), window, cx);
    }

    /// Translate `scope` of the active document.
    ///
    /// Runs on a background task: a network round-trip must never block the UI
    /// thread. The document engine decides what is translatable; this only
    /// picks the provider.
    fn translate(&mut self, scope: Scope, window: &mut Window, cx: &mut Context<Self>) {
        let Some(doc) = self.active_document().cloned() else {
            return;
        };
        let settings = crate::settings::AppSettings::global(cx).clone();
        let Some(provider) = Provider::resolve(&settings) else {
            // Naming the fix rather than the symptom: "not configured" leaves
            // the user hunting through Settings for which field is missing.
            self.set_status(
                "No translation API key. Set one in Settings (Ctrl/Cmd+,), or export \
                 ANTHROPIC_API_KEY / OPENAI_API_KEY. A local server that wants no key \
                 still needs a placeholder."
                    .into(),
                cx,
            );
            return;
        };
        let service = match provider.build_with(&settings) {
            Ok(service) => service,
            Err(err) => {
                self.set_status(format!("Translation unavailable: {err}"), cx);
                return;
            }
        };

        let target = settings.translate_to.trim().to_string();
        let target = if target.is_empty() {
            "zh".to_string()
        } else {
            target
        };

        // Parse the editor's *current* text rather than reusing
        // `doc.document()`: that parse is debounced by 180ms, so translating
        // right after a keystroke would translate the previous text and then
        // overwrite the editor with it — silently discarding the edit.
        let text = doc.read(cx).text(cx);
        let doc_type = doc.read(cx).document().doc_type();

        self.set_status(format!("Translating via {}…", provider.label()), cx);
        // Set before the spawn, not inside it: the button has to go inert on
        // this frame, or the second click lands before the task even starts.
        self.translating = true;

        cx.spawn_in(window, async move |this, cx| {
            let result = cx
                .background_spawn(async move {
                    let source = mt_doc::Document::with_type(doc_type, text);
                    mt_doc::translate::translate(&source, &scope, &target, service.as_ref())
                })
                .await;

            // `try_update_in` rather than `update_in`: this lands after an
            // await and can arrive mid-draw, where the infallible borrow
            // panics. Skipping costs one frame; panicking costs the session.
            crate::views::try_update_in(&this, cx, |this, window, cx| {
                // Cleared in both arms — a flag left set by the error path is a
                // permanently dead button.
                this.translating = false;
                match result {
                    Ok(translation) => {
                        doc.update(cx, |doc, cx| {
                            doc.replace_text(translation.text, window, cx);
                        });
                        this.set_status(
                            format!(
                                "Translated {} segment(s) via {}",
                                translation
                                    .segments
                                    .iter()
                                    .filter(|s| s.translatable)
                                    .count(),
                                provider.label()
                            ),
                            cx,
                        );
                    }
                    Err(err) => this.set_status(format!("Translation failed: {err}"), cx),
                }
            });
        })
        .detach();
    }

    // --- Rendering --------------------------------------------------------

    /// The details panel, when there is something to show in it.
    ///
    /// `None` rather than an empty panel: a column of blank space next to the
    /// document is worse than no column, and the toggle in the title bar is
    /// what says whether the panel is wanted at all.
    ///
    /// No header row of its own. The panel runs from the top of the window to
    /// the bottom, so a header inside it would sit at the same height as the
    /// title bar and read as a second, competing one.
    fn render_right_panel(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        if !self.right_panel_open || self.settings_open {
            return None;
        }
        let harness = self.harness.clone()?;
        if !harness.read(cx).has_selection() {
            return None;
        }
        // Through `update` rather than `read`: the details carry an Open button
        // whose click handler emits a `HarnessEvent`, which needs the harness
        // view's own `Context`. This is the entity-lease path, not the
        // `AppCell::borrow_mut` one, so it is safe during a draw.
        let details = harness.update(cx, |harness, cx| harness.render_details(cx));
        Some(
            v_flex()
                .size_full()
                .bg(cx.theme().sidebar)
                .border_l_1()
                .border_color(cx.theme().border)
                .child(
                    h_flex()
                        .h(metrics::title_bar())
                        .flex_shrink_0()
                        .px(metrics::inset())
                        .items_center()
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .text_xs()
                                .font_medium()
                                .text_color(cx.theme().muted_foreground)
                                .child(i18n::t(i18n::Key::Details, cx)),
                        )
                        // The collapse control belongs where the panel is, not
                        // across the window in the title bar: the click that
                        // opened it and the click that closes it should land in
                        // the same place. No `stop_propagation` — this panel is
                        // not a `WindowControlArea::Drag` region.
                        .child(self.render_right_toggle(cx)),
                )
                .child(
                    div()
                        .id("details")
                        .flex_1()
                        .min_h_0()
                        .overflow_y_scroll()
                        .child(details),
                )
                .into_any_element(),
        )
    }

    /// The left panel: Files / Harness / Outline.
    ///
    /// Its own tab strip stands in for a header, and the strip is [`TITLE_BAR`]
    /// tall so it lines up with the title bar across the gap — the panel runs
    /// the full height of the window, so the two are side by side rather than
    /// stacked.
    ///
    /// [`TITLE_BAR`]: crate::metrics::TITLE_BAR
    fn render_side_panel(&self, cx: &Context<Self>) -> impl IntoElement {
        v_flex()
            .size_full()
            .bg(cx.theme().sidebar)
            .border_r_1()
            .border_color(cx.theme().border)
            .child(
                div()
                    .h(metrics::title_bar())
                    .flex_shrink_0()
                    .flex()
                    .items_center()
                    .pl(metrics::inset())
                    .gap(metrics::gap())
                    // The collapse control moves in here while the panel is
                    // open, so the click that closes it lands where the click
                    // that opened it did. Left of the strip, on the side it
                    // governs. No `stop_propagation` — this is not a
                    // `WindowControlArea::Drag` region, only the title bar is.
                    .child(self.render_left_toggle(cx))
                    .child(
                        TabBar::new("side-tabs")
                            .underline()
                            // `flex_1`, not `w_full`: the toggle beside it is a
                            // sibling now, and a strip claiming the full width
                            // of the row would push itself off the panel.
                            .flex_1()
                            .min_w_0()
                            // The inset is on the row now, so `Files` still
                            // lines up with everything below it. Only the far
                            // end is padded here — a second left inset would
                            // push the strip a toggle plus an inset in.
                            .pr(metrics::inset())
                            .selected_index(
                                SidePanel::ALL
                                    .iter()
                                    .position(|p| *p == self.side_panel)
                                    .unwrap_or(0),
                            )
                            .on_click(cx.listener(|this, ix: &usize, _, cx| {
                                this.side_panel = SidePanel::ALL[*ix];
                                cx.notify();
                            }))
                            .children(
                                SidePanel::ALL.map(|p| Tab::new().label(i18n::t(p.label(), cx))),
                            ),
                    ),
            )
            .child(div().flex_1().min_h_0().map(|this| match self.side_panel {
                SidePanel::Files => match &self.explorer {
                    Some(explorer) => this.child(explorer.clone()),
                    None => this.child(empty_hint(cx, i18n::t(i18n::Key::OpenFolderToBegin, cx))),
                },
                // No folder needed: "this file" and "open tabs" work the
                // moment a document is open, so gating the whole panel on a
                // workspace would hide two of its four scopes for no reason.
                SidePanel::Search => this.child(self.search.clone()),
                SidePanel::Harness => match &self.harness {
                    Some(harness) => this.child(harness.clone()),
                    None => {
                        this.child(empty_hint(cx, i18n::t(i18n::Key::OpenFolderToDiscover, cx)))
                    }
                },
                SidePanel::Outline => this.child(self.render_outline(cx)),
            }))
    }

    /// Document outline: headings plus MDX structure.
    ///
    /// Every row navigates: an outline you cannot click is a table of contents
    /// with the page numbers torn off.
    fn render_outline(&self, cx: &Context<Self>) -> AnyElement {
        let Some(doc) = self.active_document() else {
            return empty_hint(cx, i18n::t(i18n::Key::OpenDocumentForOutline, cx))
                .into_any_element();
        };
        let doc = doc.read(cx);
        let outline = doc.document().outline();
        if outline.is_empty() {
            return empty_hint(cx, i18n::t(i18n::Key::NoHeadings, cx)).into_any_element();
        }

        v_flex()
            .id("outline")
            .size_full()
            .px(px(metrics::INSET - metrics::ROW_PAD))
            .py_1()
            .gap(metrics::row_gap())
            .overflow_y_scroll()
            .children(outline.headings.iter().enumerate().map(|(ix, h)| {
                let offset = h.offset;
                ListItem::new(("outline-heading", ix))
                    .w_full()
                    .px(metrics::row_pad())
                    .py_0p5()
                    .rounded(cx.theme().radius)
                    .child(
                        div()
                            // Indentation carries the heading level, so the
                            // padding has to be on the content rather than the
                            // row — otherwise the hover highlight steps in with
                            // it and the list looks ragged.
                            .pl(metrics::indent(h.depth.saturating_sub(1) as usize))
                            .text_sm()
                            .truncate()
                            .child(h.text.clone()),
                    )
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.reveal_offset(offset, window, cx)
                    }))
            }))
            .children(outline.structural.iter().enumerate().map(|(ix, entry)| {
                let offset = entry.offset;
                ListItem::new(("outline-structural", ix))
                    .w_full()
                    .px(metrics::row_pad())
                    .py_0p5()
                    .rounded(cx.theme().radius)
                    .child(
                        h_flex()
                            .gap_2()
                            .text_xs()
                            .text_color(cx.theme().muted_foreground)
                            .child(entry.kind.label())
                            .child(div().flex_1().truncate().child(entry.label.clone())),
                    )
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.reveal_offset(offset, window, cx)
                    }))
            }))
            .into_any_element()
    }

    /// Move the active document's cursor to `offset` and show it.
    fn reveal_offset(&mut self, offset: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(doc) = self.active_document().cloned() else {
            return;
        };
        // The outline and the search results both land here, and both are the
        // kind of jump a user expects Back to undo.
        self.record_visit(doc.read(cx).path().to_path_buf(), offset);
        doc.update(cx, |doc, cx| doc.reveal_offset(offset, window, cx));
    }

    /// Open `path` (as a preview) and put the cursor at `offset`.
    ///
    /// The search-result path. A preview open because scanning a result list is
    /// exactly the browsing a preview tab exists for; a double click in the
    /// tree or an edit still promotes it.
    fn reveal_in(
        &mut self,
        path: PathBuf,
        offset: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.open_file_as(path.clone(), true, window, cx);
        let Some(doc) = self.active_document().cloned() else {
            return;
        };
        // Guard against the open having failed — landing the cursor in whatever
        // tab happened to be active would be worse than doing nothing.
        if doc.read(cx).path() != path {
            return;
        }
        self.record_visit(path, offset);
        doc.update(cx, |doc, cx| doc.reveal_offset(offset, window, cx));
    }

    /// Show the Search panel and put the caret in its field.
    fn on_focus_search(&mut self, _: &FocusSearch, window: &mut Window, cx: &mut Context<Self>) {
        self.side_panel = SidePanel::Search;
        // Opening the panel is not enough: the point of the binding is to type
        // a query immediately, and a panel that appears without focus makes the
        // user click into it first.
        self.left_panel_open = true;
        // Cloned first: leasing the entity through `update` takes its own
        // borrow of `cx`, which cannot overlap one held through `self`.
        let search = self.search.clone();
        search.update(cx, |search, cx| search.focus_query(window, cx));
        self.web_dirty(cx);
        cx.notify();
    }

    /// The documents the current search scope covers.
    ///
    /// Built here rather than in the search view because only the workspace can
    /// see all four sources — and because the open tabs must contribute their
    /// *editor* text, not the file on disk, or a search misses everything the
    /// user has typed since the last save.
    ///
    /// Directories are handed over unwalked. This runs on the UI thread, and
    /// walking a real vault is seconds — measured at 2.4s for 6,642 documents
    /// — so expanding a root here would freeze the window on every settled
    /// keystroke. [`Corpus::roots`] is walked on the search's own task.
    fn search_corpus(&self, cx: &App) -> Corpus {
        use crate::views::search::Scope;

        let mut corpus = Corpus::default();
        let add_open_tabs = |corpus: &mut Corpus| {
            for tab in self.tabs.iter() {
                let doc = tab.payload.view.read(cx);
                corpus.open.push((tab.path.clone(), doc.text(cx)));
            }
        };

        match self.search.read(cx).scope() {
            Scope::Document => {
                if let Some(doc) = self.active_document() {
                    let doc = doc.read(cx);
                    corpus.open.push((doc.path().to_path_buf(), doc.text(cx)));
                }
            }
            Scope::OpenTabs => add_open_tabs(&mut corpus),
            Scope::Folder => {
                add_open_tabs(&mut corpus);
                corpus.roots.extend(self.root.clone());
            }
            Scope::Harness => {
                add_open_tabs(&mut corpus);
                // The whole point of this scope: a skill's own directory holds
                // references and scripts beside its SKILL.md, and those are as
                // much a part of the skill as its entry document.
                if let Some(harness) = &self.harness {
                    let harness = harness.read(cx);
                    corpus
                        .roots
                        .extend(harness.skills().iter().map(|s| s.dir.clone()));
                    corpus
                        .files
                        .extend(harness.instructions().iter().map(|i| i.path.clone()));
                }
            }
        }
        // The open tabs are already in `corpus.open` with their unsaved text;
        // reading them again would report every match twice.
        let open = self.open_paths(cx);
        corpus.files.retain(|p| !open.contains(p));
        corpus
    }

    /// Paths of every open tab.
    fn open_paths(&self, cx: &App) -> Vec<PathBuf> {
        let _ = cx;
        self.tabs.paths().map(Path::to_path_buf).collect()
    }

    /// The left panel's toggle.
    ///
    /// One definition with two homes: the title bar carries it while the panel
    /// is collapsed, and the panel's own header takes it back once it is open.
    /// A control that jumps to the far side of the window the moment it is used
    /// makes the user hunt for it to undo the click they just made.
    fn render_left_toggle(&self, cx: &Context<Self>) -> impl IntoElement {
        Button::new("toggle-left-panel")
            .icon(IconName::PanelLeft)
            .xsmall()
            .ghost()
            .when(self.left_panel_open, |b| b.primary())
            .tooltip(i18n::t(i18n::Key::ToggleLeftPanel, cx))
            .on_click(cx.listener(|this, _, window, cx| {
                this.on_toggle_left_panel(&ToggleLeftPanel, window, cx)
            }))
    }

    /// The right panel's toggle, on the same two-homes rule as the left one.
    fn render_right_toggle(&self, cx: &Context<Self>) -> impl IntoElement {
        Button::new("toggle-right-panel")
            .icon(IconName::PanelRight)
            .xsmall()
            .ghost()
            .when(self.right_panel_open, |b| b.primary())
            .tooltip(i18n::t(i18n::Key::ToggleRightPanel, cx))
            .on_click(cx.listener(|this, _, window, cx| {
                this.on_toggle_right_panel(&ToggleRightPanel, window, cx)
            }))
    }

    fn render_tabs(&self, cx: &Context<Self>) -> impl IntoElement {
        let root = self.root.clone();
        TabBar::new("document-tabs")
            // Not `w_full`: the bar sits inside the title bar, and a strip that
            // claims the whole width leaves no slack for dragging the window.
            // Shrink-to-fit means the tabs take what they need and the rest of
            // the bar stays a drag handle.
            .selected_index(self.tabs.active_index())
            .children(self.tabs.iter().enumerate().map(|(ix, tab)| {
                let doc = tab.payload.view.read(cx);
                let path = tab.path.clone();
                let full = path.to_string_lossy().replace('\\', "/");
                // Relative only makes sense with a folder open, and only for a
                // file actually under it — a globally-discovered skill is not.
                let relative = root
                    .as_ref()
                    .and_then(|root| path.strip_prefix(root).ok())
                    .map(|rest| rest.to_string_lossy().replace('\\', "/"));
                let is_preview = self.tabs.is_preview(&path);
                let dirty = doc.is_dirty();

                Tab::new()
                    .label(elide_tab_label(&doc.title(cx)))
                    .icon(if doc.is_externally_changed() {
                        IconName::TriangleAlert
                    } else {
                        IconName::File
                    })
                    .prefix(
                        // The whole tab, wrapped: `Tab` is not interactive, so
                        // the tooltip and the context menu need an element that
                        // is. A zero-width prefix is the seam that gets one
                        // without reimplementing the tab.
                        div()
                            .id(SharedString::from(format!("tab-affordances-{ix}")))
                            .w_0()
                            .h_full()
                            // gpui's own `tooltip`, not gpui-component's
                            // `managed_tooltip`: that extension trait is
                            // private to the crate.
                            .tooltip({
                                let full = full.clone();
                                move |window, cx| Tooltip::new(full.clone()).build(window, cx)
                            })
                            .on_mouse_down(MouseButton::Right, {
                                cx.listener(move |this, _, _, cx| {
                                    // The menu acts on whichever tab was
                                    // right-clicked, which is not necessarily
                                    // the active one.
                                    this.tabs.set_menu(ix);
                                    cx.notify();
                                })
                            })
                            .context_menu({
                                let relative = relative.clone();
                                move |menu, _window, cx| {
                                    let menu = menu
                                        .menu(i18n::t(i18n::Key::CopyPath, cx), Box::new(CopyPath));
                                    // Only offered when there is one: a menu
                                    // item that silently does nothing is worse
                                    // than an absent one.
                                    match relative {
                                        Some(_) => menu.menu(
                                            i18n::t(i18n::Key::CopyRelativePath, cx),
                                            Box::new(CopyRelativePath),
                                        ),
                                        None => menu,
                                    }
                                }
                            }),
                    )
                    // A preview tab is italic, the same signal VS Code uses, so
                    // "this will be replaced by the next click" is visible
                    // before it happens rather than after.
                    .when(is_preview, |tab| tab.italic())
                    .suffix(
                        // Unsaved work shows a dot where the close button
                        // would be, which is the convention every editor uses
                        // and the reason the marker is here rather than
                        // appended to the label: a long name elides, and the
                        // one tab that most needs the warning was the one
                        // losing it.
                        if dirty {
                            div()
                                .id(SharedString::from(format!("dirty-{ix}")))
                                .flex()
                                .items_center()
                                .justify_center()
                                .size(metrics::target())
                                .tooltip({
                                    move |window, cx| {
                                        Tooltip::new(i18n::t(i18n::Key::UnsavedChanges, cx))
                                            .build(window, cx)
                                    }
                                })
                                .child(
                                    div()
                                        .size(metrics::dirty_dot())
                                        .rounded_full()
                                        .bg(cx.theme().primary),
                                )
                                .into_any_element()
                        } else {
                            Button::new(SharedString::from(format!("close-{ix}")))
                                .icon(IconName::Close)
                                .xsmall()
                                .ghost()
                                .on_click(cx.listener(move |this, _, _, cx| this.close_tab(ix, cx)))
                                .into_any_element()
                        },
                    )
            }))
            .on_click(cx.listener(|this, ix: &usize, _, cx| {
                if this.tabs.focus(*ix)
                    && let Some(tab) = this.tabs.get(*ix)
                {
                    let path = tab.path.clone();
                    this.record_visit(path, 0);
                }
                this.web_dirty(cx);
                cx.notify();
            }))
    }

    /// The one bar across the top of the document column.
    ///
    /// Window title row and document tab strip merged: stacked, they spent 80
    /// vertical pixels on chrome and read as two competing headers, and every
    /// modern editor puts the tabs in the title bar. The app name yields to
    /// them — the window title already says what this is.
    ///
    /// It spans the document and the details panel but **not** the left panel,
    /// which runs the full height of the window beside it. That is what makes
    /// the panels read as the main view extending sideways rather than as
    /// content parked under a bar.
    fn render_title_bar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let navigator = self.render_navigator(cx);
        TitleBar::new()
            // `TitleBar` hard-codes its own 34px height; the side panels put a
            // header at [`metrics::TITLE_BAR`] beside it, and two chrome rows
            // that disagree by six pixels is exactly the ragged seam this
            // arrangement exists to avoid.
            .h(metrics::title_bar())
            .child(
                h_flex()
                    .w_full()
                    .h_full()
                    .pl(metrics::inset())
                    .gap(metrics::gap_group())
                    .items_center()
                    // The left panel's toggle sits at the left edge, above the
                    // panel it governs; the right one at the right edge, above
                    // that one. A control whose position contradicts what it
                    // opens is a control the user has to read rather than
                    // recognize. Each is here only while its panel is closed —
                    // an open panel carries its own copy in its header, which
                    // is where the eye already is.
                    .when(!self.left_panel_open, |this| {
                        this.child(
                            h_flex()
                                .flex_shrink_0()
                                .items_center()
                                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                                .child(self.render_left_toggle(cx)),
                        )
                    })
                    .child(
                        h_flex()
                            .flex_1()
                            .min_w_0()
                            .items_center()
                            .gap(metrics::gap())
                            // Back and Forward first, then the tabs — the
                            // arrangement Zed and every browser use, because
                            // navigation is about the strip that follows it.
                            .child(navigator)
                            // The tabs claim the press; the slack beside them does
                            // not. `stop_propagation` on a `flex_1` wrapper covered
                            // the whole bar and killed **both** ways the window
                            // moves: GPUI's, because `TitleBar`'s own
                            // `on_mouse_down` -> `start_window_move` never sees a
                            // stopped event, and Windows', because
                            // `handle_nc_mouse_down_msg` returns `Some(0)` for a
                            // handled press and `DefWindowProc` never gets the
                            // `HTCAPTION`. So the handler goes on a box sized to
                            // the tabs, and what is left over stays a drag handle.
                            .when(!self.tabs.is_empty(), |this| {
                                this.child(
                                    div()
                                        .min_w_0()
                                        .max_w_full()
                                        .on_mouse_down(MouseButton::Left, |_, _, cx| {
                                            cx.stop_propagation()
                                        })
                                        .child(self.render_tabs(cx)),
                                )
                            })
                            // With nothing open the name stands in for the tabs,
                            // and is itself part of the drag handle.
                            .when(self.tabs.is_empty(), |this| {
                                this.child(
                                    div()
                                        .text_sm()
                                        .font_semibold()
                                        .text_color(cx.theme().muted_foreground)
                                        .child("markturbo"),
                                )
                            })
                            // Whatever is left of the bar. No handler, so a press
                            // here reaches the title bar and moves the window.
                            .child(div().flex_1().min_w_0().h_full()),
                    )
                    .child(
                        // The title bar is a `WindowControlArea::Drag` region,
                        // which Windows hit-tests as `HTCAPTION`: a press there
                        // becomes a window drag and never reaches GPUI's mouse
                        // dispatch, so buttons inside it silently do nothing.
                        // Claiming the press back is what upstream's own example
                        // does (gpui-component `story/src/title_bar.rs`).
                        h_flex()
                            .flex_shrink_0()
                            .gap(metrics::gap())
                            .items_center()
                            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                            .child(
                                Button::new("open-folder")
                                    .label(i18n::t(i18n::Key::OpenFolder, cx))
                                    .xsmall()
                                    .ghost()
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.on_open_folder(&OpenFolder, window, cx)
                                    })),
                            )
                            .child(
                                // Translating the selection when there is one is
                                // what a user means by "Translate" with text
                                // highlighted; falling back to the whole document
                                // otherwise avoids a menu for a two-case choice.
                                Button::new("translate")
                                    .label(i18n::t(i18n::Key::Translate, cx))
                                    .xsmall()
                                    .ghost()
                                    // Swaps the icon for a spinner and makes the
                                    // button inert, so the round-trip is visible
                                    // and a second request cannot be started
                                    // over the same text.
                                    .loading(self.translating)
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        let has_selection = this
                                            .active_document()
                                            .is_some_and(|d| !d.read(cx).selection(cx).is_empty());
                                        if has_selection {
                                            this.on_translate_selection(
                                                &TranslateSelection,
                                                window,
                                                cx,
                                            )
                                        } else {
                                            this.on_translate_document(
                                                &TranslateDocument,
                                                window,
                                                cx,
                                            )
                                        }
                                    })),
                            )
                            // Immediately left of Settings while the panel is
                            // closed; the panel's own header carries it once it
                            // is open.
                            .when(!self.right_panel_open, |this| {
                                this.child(self.render_right_toggle(cx))
                            })
                            .child(
                                Button::new("settings")
                                    .icon(IconName::Settings)
                                    .xsmall()
                                    .ghost()
                                    .when(self.settings_open, |b| b.primary())
                                    .tooltip(i18n::t(i18n::Key::Settings, cx))
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.on_open_settings(&OpenSettings, window, cx)
                                    })),
                            ),
                    ),
            )
    }

    fn render_status_bar(&self, cx: &Context<Self>) -> impl IntoElement {
        let renderers = self
            .registry
            .availability_report()
            .into_iter()
            .filter(|(_, a)| !a.is_available())
            .map(|(name, _)| name)
            .collect::<Vec<_>>();
        let watching = crate::settings::AppSettings::global(cx).watch_auto_reload;

        h_flex()
            .w_full()
            .h(metrics::status_bar())
            .px(metrics::inset())
            .gap(metrics::gap_group())
            .items_center()
            .border_t_1()
            .border_color(cx.theme().border)
            .bg(cx.theme().status_bar)
            .text_xs()
            .text_color(cx.theme().muted_foreground)
            .children(self.status.clone().map(|s| div().flex_1().child(s)))
            .when(self.status.is_none(), |this| {
                this.child(
                    div().flex_1().children(
                        self.root
                            .as_ref()
                            .map(|root| div().child(root.to_string_lossy().to_string())),
                    ),
                )
            })
            .when(!renderers.is_empty(), |this| {
                this.child(
                    h_flex()
                        .gap_1()
                        .items_center()
                        .child(Icon::new(IconName::TriangleAlert).xsmall())
                        .child(format!("{} unavailable", renderers.join(", "))),
                )
            })
            // Last, so it lands at the right end of the bar. Watching is a mode
            // rather than a command, and a mode needs somewhere to show that it
            // is on — an eye that is lit is the whole indicator, so the same
            // control both sets and reports it.
            .child(
                Button::new("toggle-auto-refresh")
                    .icon(if watching {
                        IconName::Eye
                    } else {
                        IconName::EyeOff
                    })
                    .xsmall()
                    .ghost()
                    .when(watching, |b| b.primary())
                    .tooltip(i18n::t(
                        if watching {
                            i18n::Key::AutoRefreshOn
                        } else {
                            i18n::Key::AutoRefresh
                        },
                        cx,
                    ))
                    .on_click(cx.listener(|_, _, _, cx| {
                        crate::settings::AppSettings::update(cx, |settings| {
                            settings.watch_auto_reload = !settings.watch_auto_reload
                        });
                        cx.notify();
                    })),
            )
    }
}

fn empty_hint(cx: &App, text: &str) -> impl IntoElement {
    div()
        .p(metrics::inset())
        .text_sm()
        .text_color(cx.theme().muted_foreground)
        .child(text.to_string())
}

impl Focusable for Workspace {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for Workspace {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // The WebView is deliberately NOT touched here: it is an OS child
        // window, and mutating it during a draw re-enters the window procedure
        // with the App already borrowed. `WebSurface::mark_dirty` runs it after
        // the effect cycle instead.

        // Panel widths are a share of the window rather than a fixed column, so
        // the same layout reads the same on a laptop and on a 4K display. The
        // viewport is only available here, which is why the widths are resolved
        // in `render` rather than baked into a constant.
        let viewport = window.viewport_size().width;

        let content: AnyElement = if self.settings_open {
            self.settings.clone().into_any_element()
        } else {
            match self.active_document() {
                Some(doc) => doc.clone().into_any_element(),
                None => v_flex()
                    .size_full()
                    .items_center()
                    .justify_center()
                    .gap_2()
                    .child(Icon::new(IconName::BookOpen))
                    .child(
                        div()
                            .text_sm()
                            .child(i18n::t(i18n::Key::OpenAMarkdownFile, cx).to_string()),
                    )
                    .into_any_element(),
            }
        };

        // All three built before the tree, because each takes a borrow of `cx`:
        // the details panel leases the harness entity, and the title bar and
        // side panel read the active document through it. Building them inline
        // would overlap those borrows with the `&mut Context` the element chain
        // already holds.
        let right_panel = self.render_right_panel(cx);
        let title_bar = self.render_title_bar(cx).into_any_element();
        let side_panel = self
            .left_panel_open
            .then(|| self.render_side_panel(cx).into_any_element());

        v_flex()
            .id("workspace")
            // Without a role the whole window is announced instead of the
            // focused element; gpui logs exactly that. `Application` is the
            // right one for a window whose own keybindings drive it.
            .role(gpui::Role::Application)
            .aria_label("markturbo workspace")
            .track_focus(&self.focus_handle)
            .key_context("Workspace")
            .on_action(cx.listener(Self::on_open_folder))
            .on_action(cx.listener(Self::on_save))
            .on_action(cx.listener(Self::on_close_tab))
            .on_action(cx.listener(Self::on_open_settings))
            .on_action(cx.listener(Self::on_copy_path))
            .on_action(cx.listener(Self::on_copy_relative_path))
            .on_action(cx.listener(Self::on_toggle_left_panel))
            .on_action(cx.listener(Self::on_toggle_right_panel))
            .on_action(cx.listener(Self::on_focus_search))
            .on_action(cx.listener(Self::on_navigate_back))
            .on_action(cx.listener(Self::on_navigate_forward))
            // Dropping a file or folder onto the window opens it. The whole
            // window is the target rather than the document area: a drop is
            // aimed at the app, and making the user find a hot zone would be a
            // puzzle rather than a feature.
            .on_drop(cx.listener(|this, paths: &ExternalPaths, window, cx| {
                this.on_drop_paths(paths.paths(), window, cx);
            }))
            .on_action(cx.listener(Self::on_translate_document))
            .on_action(cx.listener(Self::on_translate_selection))
            .on_action(cx.listener(Self::on_translate_block))
            .size_full()
            // The panels run the full height of the window, with the title bar
            // spanning only the column between them. tty7's arrangement, and
            // the reason for it: a bar drawn across the panels too makes them
            // read as content parked underneath it, where this reads as the
            // main view extending sideways.
            .child(
                div().flex_1().min_h_0().child(
                    h_resizable("workspace-split")
                        .when_some(side_panel, |group, panel| {
                            group.child(
                                resizable_panel()
                                    .size(metrics::SIDE_PANEL.resolve(viewport))
                                    .size_range(metrics::SIDE_PANEL.drag_range())
                                    .child(panel),
                            )
                        })
                        .child(
                            resizable_panel().child(
                                v_flex()
                                    .size_full()
                                    .child(title_bar)
                                    .child(div().flex_1().min_h_0().child(content)),
                            ),
                        )
                        // The details of whatever is selected on the left, on
                        // the right. They used to sit under the list in the same
                        // narrow column, which left the list and the details
                        // both too short to read.
                        .when_some(right_panel, |this, panel| {
                            this.child(
                                resizable_panel()
                                    .size(metrics::RIGHT_PANEL.resolve(viewport))
                                    .size_range(metrics::RIGHT_PANEL.drag_range())
                                    .child(panel),
                            )
                        }),
                ),
            )
            .child(self.render_status_bar(cx))
    }
}

/// Keybindings for the workspace's actions.
pub fn init(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("cmd-o", OpenFolder, None),
        KeyBinding::new("ctrl-o", OpenFolder, None),
        KeyBinding::new("cmd-s", Save, None),
        KeyBinding::new("ctrl-s", Save, None),
        KeyBinding::new("cmd-w", CloseTab, None),
        KeyBinding::new("ctrl-w", CloseTab, None),
        // The platform convention for preferences on each host.
        KeyBinding::new("cmd-,", OpenSettings, None),
        KeyBinding::new("ctrl-,", OpenSettings, None),
        // All three translation scopes are reachable: document, the editor
        // selection, and the block under the cursor.
        KeyBinding::new("cmd-shift-t", TranslateDocument, None),
        KeyBinding::new("ctrl-shift-t", TranslateDocument, None),
        KeyBinding::new("cmd-shift-l", TranslateSelection, None),
        KeyBinding::new("ctrl-shift-l", TranslateSelection, None),
        KeyBinding::new("cmd-shift-b", TranslateBlock, None),
        KeyBinding::new("ctrl-shift-b", TranslateBlock, None),
        // The panels, on the bindings VS Code uses for the same two.
        KeyBinding::new("cmd-b", ToggleLeftPanel, None),
        KeyBinding::new("ctrl-b", ToggleLeftPanel, None),
        KeyBinding::new("cmd-alt-b", ToggleRightPanel, None),
        KeyBinding::new("ctrl-alt-b", ToggleRightPanel, None),
        // Workspace search. `Ctrl/Cmd+F` stays with the editor's own find —
        // one searches the document you are in, the other searches everywhere,
        // and giving the second the first's binding would take away the more
        // frequently wanted of the two.
        KeyBinding::new("cmd-shift-f", FocusSearch, None),
        KeyBinding::new("ctrl-shift-f", FocusSearch, None),
        // Back and forward, on the bindings every editor and IDE uses.
        KeyBinding::new("ctrl-alt--", NavigateBack, None),
        KeyBinding::new("ctrl-alt-shift--", NavigateForward, None),
        KeyBinding::new("cmd-alt-left", NavigateBack, None),
        KeyBinding::new("cmd-alt-right", NavigateForward, None),
        KeyBinding::new("alt-left", NavigateBack, None),
        KeyBinding::new("alt-right", NavigateForward, None),
    ]);
}

#[cfg(test)]
mod tests {
    // Import selectively: the `gpui::*` glob above re-exports a `test`
    // attribute macro that shadows the built-in one and blows the recursion
    // limit.
    use super::{TAB_LABEL_MAX, elide_tab_label};

    #[test]
    fn short_names_are_left_alone() {
        for name in ["a.md", "README.md", "architecture.md"] {
            assert_eq!(elide_tab_label(name), name);
        }
    }

    #[test]
    fn long_names_keep_their_extension() {
        // Eliding from the end is the obvious implementation and removes
        // exactly the part worth keeping: `notes.md` and `notes.mdx` are
        // different documents, and the extension is what says which.
        let long = "a-very-long-document-name-indeed.mdx";
        let out = elide_tab_label(long);
        assert!(out.ends_with(".mdx"), "got {out}");
        assert!(out.contains('…'), "got {out}");
        assert!(out.chars().count() <= TAB_LABEL_MAX, "got {out}");
    }

    #[test]
    fn elision_counts_characters_not_bytes() {
        // A CJK name is well under the limit in characters and well over it in
        // bytes; slicing by byte would also panic mid-codepoint.
        let name = "这是一个很长的中文文档名称.md";
        let out = elide_tab_label(name);
        assert!(out.chars().count() <= TAB_LABEL_MAX, "got {out}");
        // Long enough to survive intact at this limit.
        assert_eq!(out, name);

        let longer = "这是一个非常非常非常长的中文文档名称需要省略.md";
        let out = elide_tab_label(longer);
        assert!(out.chars().count() <= TAB_LABEL_MAX, "got {out}");
        assert!(out.ends_with(".md"), "got {out}");
    }

    #[test]
    fn a_dotfile_is_not_all_extension() {
        // `.gitignore` has no stem before the dot; treating the whole name as
        // an extension would leave nothing to elide and produce just "…".
        let out = elide_tab_label(".a-really-long-dotfile-name-here");
        assert!(!out.starts_with('…'), "got {out}");
        assert!(out.chars().count() <= TAB_LABEL_MAX, "got {out}");
    }

    #[test]
    fn a_name_with_no_extension_still_elides() {
        let out = elide_tab_label("LICENSE-WITH-A-VERY-LONG-SUFFIX");
        assert!(out.chars().count() <= TAB_LABEL_MAX, "got {out}");
        assert!(out.ends_with('…'), "got {out}");
    }

    /// A preview tab must be replaced, not accumulated.
    ///
    /// Source-level: the behavior needs a window and a real click sequence, but
    /// what makes it work is that `open_file_as` takes the outgoing preview and
    /// closes it. Someone simplifying that away gets forty tabs back.
    ///
    /// The *slot's* own rules — closing the preview tab empties it, closing a
    /// different one leaves it — are plain data and are tested for real in
    /// [`crate::views::tabs`].
    #[test]
    fn opening_a_preview_replaces_the_previous_one() {
        let source = include_str!("workspace.rs");
        let start = source
            .find("pub fn open_file_as")
            .expect("open_file_as must exist");
        let body = &source[start..];
        let end = body.find("\n    /// Focus the tab").unwrap_or(body.len());
        let body = &body[..end];

        assert!(
            body.contains("take_preview()"),
            "the outgoing preview must be taken, or previews accumulate"
        );
        assert!(
            body.contains("close_tab"),
            "and closed, or the slot is only forgotten rather than freed"
        );
        assert!(
            body.contains("is_dirty"),
            "a preview with unsaved edits must be kept, not silently discarded"
        );
    }

    /// Per-document subscriptions must die with their tab.
    ///
    /// Source-level because the leak is invisible to any assertion: the
    /// subscriptions stayed alive and simply fired against a document nobody
    /// could see. What prevents it is that they are *owned by the tab* rather
    /// than pushed onto a workspace-wide `Vec` that only ever grows.
    #[test]
    fn a_closed_tab_takes_its_subscriptions_with_it() {
        let source = include_str!("workspace.rs");
        let start = source
            .find("fn open_file_inner")
            .expect("open_file_inner must exist");
        let body = &source[start..];
        let end = body.find("\n    fn close_tab").unwrap_or(body.len());
        let body = &body[..end];

        assert!(
            !body.contains("_subscriptions.push"),
            "a per-document subscription must not go into the workspace-wide \
             list: that list is never pruned, so closing the tab left two \
             subscriptions to a released document alive for the session"
        );
        assert!(
            body.contains("DocumentTab {"),
            "they belong to the tab, which drops them when it closes"
        );
    }

    /// A status message must not be cleared by the previous message's timer.
    ///
    /// Source-level: the failure needs six seconds of wall clock and two
    /// messages, which is not a test worth having. What prevents it is the
    /// generation check plus a single timer slot — a detached task per message
    /// meant every one of them cleared the bar unconditionally when it fired,
    /// so the second message vanished on the first one's schedule.
    #[test]
    fn a_status_message_outlives_the_one_before_it() {
        let source = include_str!("workspace.rs");
        let start = source.find("fn set_status").expect("set_status must exist");
        let body = &source[start..];
        let end = body.find("\n    /// Apply pending").unwrap_or(body.len());
        let body = &body[..end];

        assert!(
            !body.contains(".detach()"),
            "the timer must be held in a slot, so a new message cancels the \
             old one's timer rather than leaving it to fire"
        );
        assert!(
            body.contains("status_generation == generation"),
            "and the timer must check it is still clearing its own message"
        );
    }

    ///
    /// Source-level: a real drop needs a window and an OS drag. What makes the
    /// feature work rather than appear to is that a directory becomes the
    /// workspace, a document opens, and anything else is *reported* — a drop
    /// that silently does nothing reads as a broken window.
    #[test]
    fn dropping_paths_handles_all_three_cases() {
        let source = include_str!("workspace.rs");
        let start = source
            .find("fn on_drop_paths")
            .expect("on_drop_paths must exist");
        let body = &source[start..];
        let end = body
            .find("\n    /// The path of whichever tab")
            .unwrap_or(body.len());
        let body = &body[..end];

        assert!(
            body.contains("is_dir()"),
            "a folder must open as a workspace"
        );
        assert!(
            body.contains("is_openable("),
            "a dropped file must go through the same gate the file tree uses. \
             `DocType::of(..).is_document()` is not that gate: it judges the \
             extension alone, so a NUL-filled `.log` passes, is decoded into \
             the editor's `String`, and the first Ctrl+S re-encodes from that \
             `String` — dropping every byte the decoder could not map. The \
             conflict check in `fs::save` guards a different failure: nothing \
             changed on disk, so that write is authorized"
        );
        assert!(
            body.contains("set_status"),
            "an unopenable drop must say so rather than doing nothing visible"
        );
        assert!(
            body.contains("self.root.is_none()"),
            "a bare file drop should adopt its parent as the workspace"
        );
    }

    /// The title bar must leave somewhere to grab the window.
    ///
    /// Source-level because reproducing it needs a real `WM_NCHITTEST` against
    /// a laid-out bar. What broke: the tab strip was wrapped in a `flex_1` div
    /// carrying `stop_propagation`, so the wrapper covered the whole bar and
    /// killed *both* drag paths — GPUI's, because `TitleBar`'s own
    /// `on_mouse_down` never saw a stopped event, and Windows', because
    /// `handle_nc_mouse_down_msg` returns `Some(0)` for a handled press and
    /// `DefWindowProc` never receives the `HTCAPTION`. The window simply stopped
    /// moving, with nothing in the log.
    #[test]
    fn the_title_bar_keeps_a_drag_handle() {
        let source = include_str!("workspace.rs");
        let start = source
            .find("fn render_title_bar")
            .expect("render_title_bar must exist");
        let body = &source[start..];
        let end = body
            .find("\n    fn render_status_bar")
            .unwrap_or(body.len());
        let body = &body[..end];

        // Every press-claiming region must be sized to its own content. The
        // count is not the invariant — the bar legitimately grew a third claim
        // when the navigator arrived — what matters is that no claim sits on a
        // box that grows to fill the bar, because that box is the drag handle.
        //
        // Checked by walking backwards from each handler to the `h_flex()` or
        // `div()` that opens its element, rather than by matching indentation:
        // `cargo fmt` rewrites whitespace, and an assertion pinned to it
        // silently becomes an assertion about nothing.
        let claims: Vec<usize> = body
            .match_indices("cx.stop_propagation()")
            .map(|(at, _)| at)
            .collect();
        assert!(
            claims.len() >= 2,
            "the buttons must claim their presses back, or they do nothing \
             inside a WindowControlArea::Drag region"
        );
        for at in claims {
            let opener = body[..at]
                .rfind("h_flex()")
                .into_iter()
                .chain(body[..at].rfind("div()"))
                .max()
                .expect("every handler is on some element");
            let element = &body[opener..at];
            assert!(
                !element.contains(".flex_1()"),
                "a press-claiming region sits on a flex_1 box, which covers the \
                 whole bar and kills both drag paths: GPUI's, because \
                 `TitleBar`'s own `on_mouse_down` never sees a stopped event, \
                 and Windows', because `handle_nc_mouse_down_msg` returns \
                 `Some(0)` for a handled press so `DefWindowProc` never gets \
                 the `HTCAPTION`.\nOffending element: {element}"
            );
        }

        assert!(
            body.contains("div().flex_1().min_w_0().h_full()"),
            "the bar needs a handler-free filler, or every pixel beside the \
             tabs is claimed and the window cannot be dragged"
        );

        // And the tab strip itself must shrink to its content rather than
        // claiming the width.
        let start = source.find("fn render_tabs").expect("render_tabs");
        let tabs = &source[start..];
        let end = tabs.find("\n    /// The one bar").unwrap_or(tabs.len());
        let tabs = &tabs[..end];
        assert!(
            !tabs.contains(".w_full()"),
            "a full-width tab strip leaves no slack in the title bar to drag"
        );
    }

    /// The panels run the full height of the window, beside the title bar.
    ///
    /// Source-level: this is pure layout, invisible to any runtime assertion.
    /// What it guards is the arrangement itself — a title bar drawn across the
    /// panels makes them read as content parked underneath it, where this reads
    /// as the main view extending sideways, which is the whole point.
    #[test]
    fn the_side_panels_span_the_full_window_height() {
        let source = include_str!("workspace.rs");
        let render = source
            .split_once("impl Render for Workspace")
            .expect("the Render impl")
            .1;
        let body = render.split("\n/// Keybindings").next().unwrap_or(render);

        let split = body.find("h_resizable(").expect("the workspace split");
        let title = body.find(".child(title_bar)").expect("the title bar");
        assert!(
            split < title,
            "the title bar must be built inside the resizable split, not above \
             it — above it, the bar spans the panels and they stop looking like \
             the main view extending sideways"
        );

        // Both panels are collapsible, and the split is what makes them
        // resizable rather than fixed.
        assert!(
            body.contains("when_some(side_panel"),
            "the left panel must be omittable, not merely narrow"
        );
        assert!(
            body.contains("when_some(right_panel"),
            "the right panel must be omittable"
        );
    }

    /// Each panel's toggle sits above the panel it governs, and moves inside it.
    ///
    /// Source-level: pure layout, invisible to any runtime assertion. Two things
    /// are guarded. The sides — put together, a control's position contradicts
    /// what it opens, and the user reads the icon instead of recognizing the
    /// side. And the handoff — a toggle rendered in both places at once is two
    /// buttons with the same element id, and one rendered in neither leaves an
    /// open panel with no way to close it.
    #[test]
    fn each_panel_toggle_sits_on_its_own_side() {
        let source = include_str!("workspace.rs");
        let start = source
            .find("fn render_title_bar")
            .expect("render_title_bar must exist");
        let body = &source[start..];
        let end = body
            .find("\n    fn render_status_bar")
            .unwrap_or(body.len());
        let body = &body[..end];

        let left = body
            .find("render_left_toggle")
            .expect("the left panel toggle");
        let right = body
            .find("render_right_toggle")
            .expect("the right panel toggle");
        let tabs = body.find("self.render_tabs(cx)").expect("the tab strip");
        assert!(
            left < tabs,
            "the left panel's toggle must come before the tabs, at the left edge"
        );
        assert!(
            right > tabs,
            "the right panel's toggle must come after the tabs, at the right edge"
        );
        // The user asked for this position exactly: immediately left of
        // Settings, not after it at the very corner.
        let settings = body
            .find("Button::new(\"settings\")")
            .expect("the settings button");
        assert!(
            right < settings,
            "the right panel's toggle must sit immediately left of Settings"
        );

        // Each is in the bar only while its panel is closed. The guard is
        // checked against the toggle that follows it, so swapping the two
        // conditions cannot pass.
        for (guard, toggle) in [
            ("when(!self.left_panel_open", "render_left_toggle"),
            ("when(!self.right_panel_open", "render_right_toggle"),
        ] {
            let at = body.find(guard).unwrap_or_else(|| {
                panic!("`{guard}` must gate the bar's copy, or the toggle is drawn twice")
            });
            let after = &body[at..];
            let next = after
                .find("render_left_toggle")
                .into_iter()
                .chain(after.find("render_right_toggle"))
                .min()
                .expect("a toggle after its guard");
            assert!(
                after[next..].starts_with(toggle),
                "`{guard}` must gate `{toggle}`, not the other side's"
            );
        }

        // And each open panel carries its own copy, or opening a panel hides
        // the only control that closes it again.
        for (renderer, toggle) in [
            ("fn render_side_panel", "render_left_toggle"),
            ("fn render_right_panel", "render_right_toggle"),
        ] {
            let start = source
                .find(renderer)
                .unwrap_or_else(|| panic!("{renderer}"));
            let body = &source[start..];
            let end = body.find("\n    /// ").unwrap_or(body.len());
            assert!(
                body[..end].contains(toggle),
                "{renderer} must carry `{toggle}` in its header"
            );
        }
    }

    /// Nothing in this file may reach the App through the infallible windowed
    /// borrow.
    ///
    /// Source-level because the failure only reproduces when the platform
    /// re-enters the window procedure mid-draw. `update_in` bottoms out in
    /// `AppCell::borrow_mut`, which panics on that re-entrant borrow; every
    /// caller here runs after an `await`, so every caller here can land in one.
    /// `try_update_in` takes the same borrow through `try_borrow_mut` and skips
    /// the frame instead. The literal in the log is `RefCell already borrowed`.
    ///
    /// Checked over the whole file rather than per spawn body: there is no
    /// legitimate `update_in` here, and matching async block extents textually
    /// is how a source check quietly stops asserting anything.
    #[test]
    fn no_async_task_updates_through_the_infallible_borrow() {
        let source = include_str!("workspace.rs");
        // Only the code, not this test's own description of it.
        let code = &source[..source.find("\n#[cfg(test)]").unwrap_or(source.len())];

        assert!(
            !code.contains(".update_in("),
            "an update goes through the infallible windowed borrow, which \
             panics when it lands mid-draw. Use `crate::views::try_update_in`."
        );
        assert_eq!(
            code.matches("try_update_in(&this").count(),
            2,
            "the folder prompt and the translation both land after an await and \
             both need the fallible path"
        );
        assert!(
            code.contains("try_update(&this, cx, |this, cx| this.drain_watcher(cx))"),
            "the watcher poll lands after an await too; it needs no `Window`, \
             so it takes the windowless fallible path"
        );
    }

    /// Auto-refresh must never discard typed text.
    ///
    /// Source-level: reproducing it needs a window, a real editor and a file
    /// changing underneath it. What makes it safe rather than merely working is
    /// that a document refusing to reload — which is what a dirty one does —
    /// still gets flagged, so the conflict banner appears instead. Someone
    /// simplifying the `if !reloaded` away silently loses the banner on exactly
    /// the documents that need it.
    #[test]
    fn auto_refresh_never_reloads_a_document_with_unsaved_edits() {
        let source = include_str!("workspace.rs");
        let start = source
            .find("fn drain_watcher")
            .expect("drain_watcher must exist");
        let body = &source[start..];
        let end = body.find("\n    // --- Actions").unwrap_or(body.len());
        let body = &body[..end];

        assert!(
            body.contains("watch_auto_reload"),
            "reloading must be opt-in, not the default"
        );
        assert!(
            body.contains("reload_if_clean"),
            "the reload must go through the guard that refuses a dirty document"
        );
        assert!(
            body.contains("if !reloaded") && body.contains("mark_externally_changed"),
            "a document that refused to reload must still be flagged, or the \
             conflict banner never appears and the change goes unnoticed"
        );
    }

    /// A translation in flight must show it and refuse a second request.
    ///
    /// Source-level: the failure needs a real network round-trip. Two requests
    /// over the same text race to overwrite the editor with two different
    /// answers, and the flag is only half the guard — the keybindings bypass
    /// the button entirely, and a flag left set by the error path is a
    /// permanently dead button.
    #[test]
    fn a_translation_in_flight_blocks_a_second_one() {
        let source = include_str!("workspace.rs");
        // Only the code: this test names the literals it looks for, and would
        // otherwise count itself.
        let code = &source[..source.find("\n#[cfg(test)]").unwrap_or(source.len())];
        let start = code.find("fn translate(").expect("translate must exist");
        let body = &code[start..];
        let end = body.find("\n    // --- Rendering").unwrap_or(body.len());
        let body = &body[..end];

        assert!(
            body.contains("self.translating = true"),
            "the flag must be set before the spawn, or the second click lands \
             before the task starts"
        );
        assert!(
            body.contains("this.translating = false"),
            "and cleared when the result arrives"
        );
        // Cleared once, above the match on the result, rather than per arm —
        // which is what keeps the error path from leaving a dead button.
        let cleared = body.find("this.translating = false").expect("the clear");
        let matched = body.find("match result").expect("the result match");
        assert!(
            cleared < matched,
            "the flag must be cleared for both outcomes; an error path that \
             leaves it set is a permanently dead button"
        );

        // The button goes inert, and so do the keybindings that bypass it.
        assert!(
            code.contains(".loading(self.translating)"),
            "the Translate button must show the request and go inert"
        );
        assert_eq!(
            code.matches("if self.translating {").count(),
            3,
            "all three translate actions must return early, or a keybinding \
             starts a second request the inert button cannot"
        );
    }

    /// Search must never search a stale copy of an open document, and must
    /// never walk the filesystem on the UI thread.
    ///
    /// Source-level: reproducing either needs a window, an editor and a real
    /// vault. Both failures are real and were measured — walking the 6,642-file
    /// vault takes 2.4s, which as a UI-thread cost is a frozen window on every
    /// settled keystroke; and reading an open tab from disk misses everything
    /// typed since the last save.
    #[test]
    fn the_search_corpus_is_cheap_to_build_and_prefers_editor_text() {
        let source = include_str!("workspace.rs");
        let start = source
            .find("fn search_corpus")
            .expect("search_corpus must exist");
        let body = &source[start..];
        let end = body
            .find("\n    /// Paths of every open tab")
            .unwrap_or(body.len());
        let body = &body[..end];

        assert!(
            body.contains("corpus.open.push"),
            "open tabs must contribute their editor text"
        );
        assert!(
            !body.contains("document_paths("),
            "walking a directory here runs on the UI thread — measured at 2.4s \
             on a 6,642-document vault. Hand the directory over in \
             `Corpus::roots` and let the background task walk it."
        );
        assert!(
            body.contains("corpus.roots"),
            "directories must be passed unwalked"
        );
        assert!(
            body.contains("!open.contains(p)"),
            "files already contributed as editor text must be filtered out, or \
             every match in an open document is reported twice"
        );
        assert!(
            body.contains("skills().iter().map(|s| s.dir.clone())"),
            "the Harness scope must cover a skill's whole directory, not only \
             its SKILL.md — references and scripts are part of the skill"
        );
    }
}
