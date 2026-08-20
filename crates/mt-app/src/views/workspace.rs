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
                if this.update(cx, |this, cx| this.drain_watcher(cx)).is_err() {
                    break;
                }
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

        let subscription = cx.subscribe_in(
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
        );
        self._subscriptions.push(subscription);

        self.documents.push(view);
        self.active = self.documents.len() - 1;
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
        cx.notify();
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
            let _ = this.update(cx, |this, cx| {
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

    /// Keep the WebView in sync with the active document.
    ///
    /// Called from render because the WebView is an OS child window positioned
    /// by the element tree; it must be created after the window exists and
    /// updated whenever the visible content changes.
    #[cfg(any(target_os = "windows", target_os = "macos"))]
    fn sync_webview(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let wants_web = self
            .active_document()
            .map(|d| {
                let doc = d.read(cx);
                doc.mode().uses_webview(doc.split_preview())
            })
            .unwrap_or(false);

        if !wants_web {
            if let Some(webview) = &self.webview {
                webview.update(cx, |webview, _| webview.hide());
            }
            self.web_current = None;
            return;
        }

        let Some(html) = self
            .active_document()
            .and_then(|d| d.read(cx).web_html().map(str::to_string))
        else {
            return;
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
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        #[cfg(any(target_os = "windows", target_os = "macos"))]
        self.sync_webview(window, cx);

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
                            Button::new("open-folder")
                                .label("Open Folder")
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
                                .label("Translate")
                                .xsmall()
                                .ghost()
                                .on_click(cx.listener(|this, _, window, cx| {
                                    let has_selection = this
                                        .active_document()
                                        .is_some_and(|d| !d.read(cx).selection(cx).is_empty());
                                    if has_selection {
                                        this.on_translate_selection(&TranslateSelection, window, cx)
                                    } else {
                                        this.on_translate_document(&TranslateDocument, window, cx)
                                    }
                                })),
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
