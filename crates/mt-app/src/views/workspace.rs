//! The workspace: explorer + skills + outline on the left, tabbed documents on
//! the right.
//!
//! Owns the open-document set, the filesystem watcher, the Web preview surface,
//! and the commands (open folder, save, translate). Individual views stay
//! narrow; this is where they are wired together.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use gpui::prelude::FluentBuilder as _;
use gpui::*;
use gpui_component::{
    ActiveTheme as _, Icon, IconName, Sizable as _, StyledExt as _, TitleBar,
    button::{Button, ButtonVariants as _},
    h_flex,
    resizable::{h_resizable, resizable_panel},
    tab::{Tab, TabBar},
    v_flex,
};
use mt_doc::translate::Scope;

use crate::renderer::RendererRegistry;
use crate::translate::Provider;
use crate::views::document::{DocumentEvent, DocumentView};
use crate::views::explorer::{Explorer, ExplorerEvent};
use crate::views::skills::{SkillsEvent, SkillsView};
use crate::watcher::Watcher;
use crate::fs;

actions!(
    markturbo,
    [OpenFolder, Save, CloseTab, TranslateDocument, TranslateSelection, TranslateBlock]
);

/// Which left-panel section is showing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SidePanel {
    Files,
    Skills,
    Outline,
}

impl SidePanel {
    const ALL: [SidePanel; 3] = [SidePanel::Files, SidePanel::Skills, SidePanel::Outline];

    fn label(self) -> &'static str {
        match self {
            SidePanel::Files => "Files",
            SidePanel::Skills => "Skills",
            SidePanel::Outline => "Outline",
        }
    }
}

/// How often to drain the filesystem watcher.
///
/// The watcher itself is already debounced; this only governs how quickly a
/// detected change reaches the UI.
const WATCH_POLL: Duration = Duration::from_millis(500);

/// What the WebView should be showing.
///
/// `Unchanged` is not "do nothing because nothing happened" — it means the
/// answer is not known yet (a Web pane is wanted but its HTML has not been
/// built), and leaving the WebView alone beats flashing the previous document.
#[cfg(any(target_os = "windows", target_os = "macos"))]
#[derive(Debug, Clone, PartialEq, Eq)]
enum WebIntent {
    Hide,
    Show(String),
    Unchanged,
}

pub struct Workspace {
    focus_handle: FocusHandle,
    root: Option<PathBuf>,
    explorer: Option<Entity<Explorer>>,
    skills: Option<Entity<SkillsView>>,
    side_panel: SidePanel,
    documents: Vec<Entity<DocumentView>>,
    active: usize,
    registry: Arc<RendererRegistry>,
    watcher: Option<Watcher>,
    status: Option<String>,
    /// The single WebView for this window. It is an OS-level child window, so
    /// one is shared by every tab rather than created per document.
    #[cfg(any(target_os = "windows", target_os = "macos"))]
    webview: Option<Entity<gpui_wry::WebView>>,
    /// Set while a deferred WebView sync is queued, so a burst of notifications
    /// coalesces into one.
    #[cfg(any(target_os = "windows", target_os = "macos"))]
    webview_sync_pending: bool,
    /// What the WebView is currently showing, so we do not reload identical
    /// content on every frame.
    web_current: Option<String>,
    _tasks: Vec<Task<()>>,
    _subscriptions: Vec<Subscription>,
}

impl Workspace {
    /// Create the workspace, opening `initial` if given.
    pub fn new(
        initial: Option<PathBuf>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
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
            skills: None,
            side_panel: SidePanel::Files,
            documents: Vec::new(),
            active: 0,
            registry: Arc::new(RendererRegistry::with_defaults()),
            watcher: None,
            status: None,
            #[cfg(any(target_os = "windows", target_os = "macos"))]
            webview: None,
            #[cfg(any(target_os = "windows", target_os = "macos"))]
            webview_sync_pending: false,
            web_current: None,
            _tasks: vec![poll],
            _subscriptions: Vec::new(),
        };

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
        let skills = cx.new(|cx| SkillsView::new(path.clone(), cx));

        self._subscriptions = vec![
            cx.subscribe_in(
                &explorer,
                window,
                |this: &mut Self, _, event: &ExplorerEvent, window, cx| {
                    let ExplorerEvent::OpenFile(path) = event;
                    this.open_file(path.clone(), window, cx);
                },
            ),
            cx.subscribe_in(
                &skills,
                window,
                |this: &mut Self, _, event: &SkillsEvent, window, cx| {
                    let SkillsEvent::OpenFile(path) = event;
                    this.open_file(path.clone(), window, cx);
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
        self.skills = Some(skills);
        self.root = Some(path);
        cx.notify();
    }

    /// Open a file in a tab, focusing an existing tab if it is already open.
    pub fn open_file(&mut self, path: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(ix) = self
            .documents
            .iter()
            .position(|d| d.read(cx).path() == path)
        {
            self.active = ix;
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

        self._subscriptions.push(cx.subscribe_in(
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
        ));
        // A document notifies on mode change, trust change, and after a
        // reparse — every event that can alter what the WebView should show.
        // Observing is what replaces the old sync-from-render.
        self._subscriptions.push(cx.observe(&view, |this, _, cx| {
            this.web_dirty(cx);
        }));

        self.documents.push(view);
        self.active = self.documents.len() - 1;
        self.web_dirty(cx);
        cx.notify();
    }

    fn close_tab(&mut self, ix: usize, cx: &mut Context<Self>) {
        if ix >= self.documents.len() {
            return;
        }
        // ponytail: closing a dirty tab drops its edits. A confirm dialog is
        // the obvious next step; the file on disk is never touched either way.
        self.documents.remove(ix);
        self.active = self.active.min(self.documents.len().saturating_sub(1));
        self.web_dirty(cx);
        cx.notify();
    }

    /// Note that the WebView may need to change. Cheap and idempotent.
    fn web_dirty(&mut self, cx: &mut Context<Self>) {
        #[cfg(any(target_os = "windows", target_os = "macos"))]
        self.schedule_webview_sync(cx);
        #[cfg(not(any(target_os = "windows", target_os = "macos")))]
        let _ = cx;
    }

    fn active_document(&self) -> Option<&Entity<DocumentView>> {
        self.documents.get(self.active)
    }

    fn set_status(&mut self, message: String, cx: &mut Context<Self>) {
        self.status = Some(message);
        cx.notify();
        // Clear after a few seconds so the bar does not hold a stale message.
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(Duration::from_secs(6)).await;
            crate::views::try_update(&this, cx, |this, cx| {
                this.status = None;
                cx.notify();
            });
        })
        .detach();
    }

    /// Apply pending filesystem changes.
    fn drain_watcher(&mut self, cx: &mut Context<Self>) {
        let Some(watcher) = &self.watcher else { return };
        let changes = watcher.poll();
        if changes.is_empty() {
            return;
        }

        let tree_changed = changes.iter().any(|c| c.affects_tree());
        let skills_changed = changes
            .iter()
            .any(|c| c.path().to_string_lossy().to_lowercase().contains("skill"));

        // Flag every open document whose file changed. Never reload silently:
        // the user's unsaved edits are theirs to keep or discard.
        for change in &changes {
            let path = change.path();
            for doc in &self.documents {
                if doc.read(cx).path() == path {
                    doc.update(cx, |doc, cx| doc.mark_externally_changed(cx));
                }
            }
        }

        if tree_changed && let Some(explorer) = &self.explorer {
            explorer.update(cx, |explorer, cx| explorer.refresh(cx));
        }
        if (tree_changed || skills_changed) && let Some(skills) = &self.skills {
            skills.update(cx, |skills, cx| skills.refresh(cx));
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
            let _ = this.update_in(cx, |this, window, cx| {
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
        self.close_tab(self.active, cx);
    }

    fn on_translate_document(
        &mut self,
        _: &TranslateDocument,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.translate(Scope::Document, window, cx);
    }

    fn on_translate_selection(
        &mut self,
        _: &TranslateSelection,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
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
        let Some(provider) = Provider::available().into_iter().next() else {
            self.set_status("No translation provider is configured".into(), cx);
            return;
        };
        let service = match provider.build() {
            Ok(service) => service,
            Err(err) => {
                self.set_status(format!("Translation unavailable: {err}"), cx);
                return;
            }
        };

        let target = std::env::var("MARKTURBO_TRANSLATE_TO").unwrap_or_else(|_| "zh".into());
        let source = doc.read(cx).document().clone();
        self.set_status(format!("Translating via {}…", provider.label()), cx);

        cx.spawn_in(window, async move |this, cx| {
            let result = cx
                .background_spawn(async move {
                    mt_doc::translate::translate(&source, &scope, &target, service.as_ref())
                })
                .await;

            let _ = this.update_in(cx, |this, window, cx| match result {
                Ok(translation) => {
                    doc.update(cx, |doc, cx| {
                        doc.replace_text(translation.text, window, cx);
                    });
                    this.set_status("Translated".into(), cx);
                }
                Err(err) => this.set_status(format!("Translation failed: {err}"), cx),
            });
        })
        .detach();
    }

    // --- WebView ----------------------------------------------------------

    /// What the WebView should be showing, as a pure read of current state.
    ///
    /// Separated from applying it because deciding is safe during a draw and
    /// applying is not — see [`Self::sync_webview`].
    #[cfg(any(target_os = "windows", target_os = "macos"))]
    fn webview_intent(&self, cx: &App) -> WebIntent {
        let Some(doc) = self.active_document() else {
            return WebIntent::Hide;
        };
        let doc = doc.read(cx);
        if !doc.mode().uses_webview(doc.split_preview()) {
            return WebIntent::Hide;
        }
        match doc.web_html() {
            Some(html) => WebIntent::Show(html.to_string()),
            // The mode wants the WebView but the HTML has not been built yet;
            // leaving it as-is avoids a flash of the previous document.
            None => WebIntent::Unchanged,
        }
    }

    /// Schedule a WebView sync for after the current effect cycle.
    ///
    /// **Never call the sync itself from `render`.** The WebView is an OS child
    /// window driven by `wry`, and on Windows WebView2 pumps messages: touching
    /// it re-enters the window procedure while the `App` is already mutably
    /// borrowed for the draw, which panics in `AppCell::borrow_mut`. `defer`
    /// runs the work at the end of the effect cycle, with no borrow held.
    #[cfg(any(target_os = "windows", target_os = "macos"))]
    fn schedule_webview_sync(&mut self, cx: &mut Context<Self>) {
        if self.webview_sync_pending {
            return;
        }
        self.webview_sync_pending = true;
        let this = cx.entity().downgrade();
        let entity_id = cx.entity_id();
        cx.defer(move |cx| {
            cx.with_window(entity_id, |window, cx| {
                this.update(cx, |this, cx| {
                    this.webview_sync_pending = false;
                    this.sync_webview(window, cx);
                })
                .ok();
            });
        });
    }

    /// Apply the current intent to the WebView. Must run outside a draw.
    #[cfg(any(target_os = "windows", target_os = "macos"))]
    fn sync_webview(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let intent = self.webview_intent(cx);

        let html = match intent {
            WebIntent::Unchanged => return,
            WebIntent::Hide => {
                if let Some(webview) = &self.webview {
                    webview.update(cx, |webview, _| webview.hide());
                }
                self.web_current = None;
                return;
            }
            WebIntent::Show(html) => html,
        };

        let webview = match &self.webview {
            Some(webview) => webview.clone(),
            None => {
                let Some(webview) = create_webview(window, cx) else {
                    return;
                };
                self.webview = Some(webview.clone());
                webview
            }
        };

        webview.update(cx, |webview, _| webview.show());
        if self.web_current.as_deref() != Some(html.as_str()) {
            let url = crate::web::to_data_url(&html);
            webview.update(cx, |webview, _| webview.load_url(&url));
            self.web_current = Some(html);
        }
    }

    // --- Rendering --------------------------------------------------------

    fn render_side_panel(&self, cx: &Context<Self>) -> impl IntoElement {
        v_flex()
            .size_full()
            .border_r_1()
            .border_color(cx.theme().border)
            .child(
                TabBar::new("side-tabs")
                    .underline()
                    .w_full()
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
                    .children(SidePanel::ALL.map(|p| Tab::new().label(p.label()))),
            )
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .map(|this| match self.side_panel {
                        SidePanel::Files => match &self.explorer {
                            Some(explorer) => this.child(explorer.clone()),
                            None => this.child(empty_hint(cx, "Open a folder to begin.")),
                        },
                        SidePanel::Skills => match &self.skills {
                            Some(skills) => this.child(skills.clone()),
                            None => this.child(empty_hint(cx, "Open a folder to discover skills.")),
                        },
                        SidePanel::Outline => this.child(self.render_outline(cx)),
                    }),
            )
    }

    /// Document outline: headings plus MDX structure.
    fn render_outline(&self, cx: &Context<Self>) -> AnyElement {
        let Some(doc) = self.active_document() else {
            return empty_hint(cx, "Open a document to see its outline.").into_any_element();
        };
        let doc = doc.read(cx);
        let outline = doc.document().outline();
        if outline.is_empty() {
            return empty_hint(cx, "This document has no headings.").into_any_element();
        }

        v_flex()
            .id("outline")
            .size_full()
            .p_1()
            .gap_0p5()
            .overflow_y_scroll()
            .children(outline.headings.iter().map(|h| {
                div()
                    .px_2()
                    .py_0p5()
                    .pl(px(8.) + px(10.) * (h.depth.saturating_sub(1)) as f32)
                    .text_sm()
                    .rounded(cx.theme().radius)
                    .child(h.text.clone())
            }))
            .children(outline.structural.iter().map(|entry| {
                h_flex()
                    .px_2()
                    .py_0p5()
                    .gap_2()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child(entry.kind.label())
                    .child(div().flex_1().truncate().child(entry.label.clone()))
            }))
            .into_any_element()
    }

    fn render_tabs(&self, cx: &Context<Self>) -> impl IntoElement {
        TabBar::new("document-tabs")
            .w_full()
            .selected_index(self.active)
            .children(self.documents.iter().enumerate().map(|(ix, doc)| {
                let doc = doc.read(cx);
                Tab::new()
                    .label(doc.title())
                    .icon(if doc.is_externally_changed() {
                        IconName::TriangleAlert
                    } else {
                        IconName::File
                    })
                    .suffix(
                        Button::new(SharedString::from(format!("close-{ix}")))
                            .icon(IconName::Close)
                            .xsmall()
                            .ghost()
                            .on_click(cx.listener(move |this, _, _, cx| this.close_tab(ix, cx))),
                    )
            }))
            .on_click(cx.listener(|this, ix: &usize, _, cx| {
                this.active = *ix;
                this.web_dirty(cx);
                cx.notify();
            }))
    }

    fn render_status_bar(&self, cx: &Context<Self>) -> impl IntoElement {
        let renderers = self
            .registry
            .availability_report()
            .into_iter()
            .filter(|(_, a)| !a.is_available())
            .map(|(name, _)| name)
            .collect::<Vec<_>>();

        h_flex()
            .w_full()
            .px_3()
            .py_1()
            .gap_3()
            .items_center()
            .border_t_1()
            .border_color(cx.theme().border)
            .text_xs()
            .text_color(cx.theme().muted_foreground)
            .children(self.status.clone().map(|s| div().flex_1().child(s)))
            .when(self.status.is_none(), |this| {
                this.child(div().flex_1().children(self.root.as_ref().map(|root| {
                    div().child(root.to_string_lossy().to_string())
                })))
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
    }
}

fn empty_hint(cx: &App, text: &str) -> impl IntoElement {
    div()
        .p_4()
        .text_sm()
        .text_color(cx.theme().muted_foreground)
        .child(text.to_string())
}

/// Create the window's WebView.
#[cfg(any(target_os = "windows", target_os = "macos"))]
fn create_webview(window: &mut Window, cx: &mut App) -> Option<Entity<gpui_wry::WebView>> {
    use raw_window_handle::HasWindowHandle as _;

    let handle = window.window_handle().ok()?;
    let builder = wry::WebViewBuilder::new();
    #[cfg(debug_assertions)]
    let builder = builder.with_devtools(true);
    let webview = builder.build_as_child(&handle).ok()?;
    Some(cx.new(|cx| gpui_wry::WebView::new(webview, window, cx)))
}

impl Focusable for Workspace {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for Workspace {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // The WebView is deliberately NOT touched here: it is an OS child
        // window, and mutating it during a draw re-enters the window procedure
        // with the App already borrowed. `schedule_webview_sync` runs it after
        // the effect cycle instead.

        let content: AnyElement = match self.active_document() {
            Some(doc) => doc.clone().into_any_element(),
            None => v_flex()
                .size_full()
                .items_center()
                .justify_center()
                .gap_2()
                .child(Icon::new(IconName::BookOpen))
                .child(div().text_sm().child("Open a Markdown file to begin."))
                .into_any_element(),
        };

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
            .on_action(cx.listener(Self::on_translate_document))
            .on_action(cx.listener(Self::on_translate_selection))
            .on_action(cx.listener(Self::on_translate_block))
            .size_full()
            .child(
                TitleBar::new().child(
                    h_flex()
                        .w_full()
                        .gap_2()
                        .items_center()
                        .child(div().text_sm().font_bold().child("markturbo"))
                        .child(div().flex_1())
                        .child(
                            // The title bar is a `WindowControlArea::Drag`
                            // region, which Windows hit-tests as `HTCAPTION`:
                            // a press there becomes a window drag and never
                            // reaches GPUI's mouse dispatch, so buttons inside
                            // it silently do nothing. Claiming the press back
                            // is what upstream's own example does
                            // (gpui-component `story/src/title_bar.rs`).
                            h_flex()
                                .gap_2()
                                .items_center()
                                .on_mouse_down(MouseButton::Left, |_, _, cx| {
                                    cx.stop_propagation()
                                })
                                .child(
                                    Button::new("open-folder")
                                        .label("Open Folder")
                                        .xsmall()
                                        .ghost()
                                        .on_click(cx.listener(|this, _, window, cx| {
                                            this.on_open_folder(&OpenFolder, window, cx)
                                        })),
                                )
                                .child(
                                    // Translating the selection when there is
                                    // one is what a user means by "Translate"
                                    // with text highlighted; falling back to
                                    // the whole document otherwise avoids a
                                    // menu for a two-case choice.
                                    Button::new("translate")
                                        .label("Translate")
                                        .xsmall()
                                        .ghost()
                                        .on_click(cx.listener(|this, _, window, cx| {
                                            let has_selection =
                                                this.active_document().is_some_and(|d| {
                                                    !d.read(cx).selection(cx).is_empty()
                                                });
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
                                ),
                        ),
                ),
            )
            .child(
                div().flex_1().min_h_0().child(
                    h_resizable("workspace-split")
                        .child(
                            resizable_panel()
                                .size(px(260.))
                                .child(self.render_side_panel(cx)),
                        )
                        .child(
                            resizable_panel().child(
                                v_flex()
                                    .size_full()
                                    .when(!self.documents.is_empty(), |this| {
                                        this.child(self.render_tabs(cx))
                                    })
                                    .child(div().flex_1().min_h_0().child(content)),
                            ),
                        ),
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
        // All three translation scopes are reachable: document, the editor
        // selection, and the block under the cursor.
        KeyBinding::new("cmd-shift-t", TranslateDocument, None),
        KeyBinding::new("ctrl-shift-t", TranslateDocument, None),
        KeyBinding::new("cmd-shift-l", TranslateSelection, None),
        KeyBinding::new("ctrl-shift-l", TranslateSelection, None),
        KeyBinding::new("cmd-shift-b", TranslateBlock, None),
        KeyBinding::new("ctrl-shift-b", TranslateBlock, None),
    ]);
}

#[cfg(test)]
mod tests {
    // Import selectively: the `gpui::*` glob above re-exports a `test`
    // attribute macro that shadows the built-in one and blows the recursion
    // limit.

    /// `Workspace::render` must not mutate the WebView.
    ///
    /// This is a source-level check rather than a runtime one on purpose: the
    /// failure it guards is a `RefCell` panic that only reproduces when the
    /// platform re-enters the window procedure mid-draw (WebView2 pumping
    /// messages, a screen reader attaching). It is not reliably reachable from
    /// a test, but it is trivially reintroducible by someone "simplifying" the
    /// deferred sync back into `render` — which is exactly what this catches.
    #[test]
    fn render_does_not_touch_the_webview() {
        // `include_str!` resolves relative to this file at compile time, so it
        // works regardless of the test runner's working directory.
        let source = include_str!("workspace.rs");
        let render = source
            .split_once("impl Render for Workspace")
            .expect("the Render impl")
            .1;
        // Stop at the next top-level item so this only reads `render`'s body.
        let body = render.split("\n/// Keybindings").next().unwrap_or(render);

        for forbidden in ["sync_webview(", "webview.update(", "create_webview("] {
            assert!(
                !body.contains(forbidden),
                "`{forbidden}` is called from render; the WebView is an OS child \
                 window and touching it during a draw re-enters the window \
                 procedure with the App already borrowed. Use \
                 `schedule_webview_sync` instead."
            );
        }
        assert!(
            source.contains("fn schedule_webview_sync"),
            "the deferred path must still exist"
        );
    }
}
