//! File explorer: the native workspace file tree.
//!
//! Lazy in two senses. A directory's children are read when it expands, so
//! opening a monorepo does not stall on a full walk — and every read happens on
//! a background task, so even a flat folder of five thousand notes (~29ms of
//! `read_dir`) never lands on the UI thread.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use gpui::*;
use gpui_component::{
    ActiveTheme as _, Icon, IconName, StyledExt as _, h_flex,
    list::ListItem,
    tree::{TreeEvent, TreeItem, TreeState, tree},
    v_flex,
};

use crate::metrics;
use crate::workspace::{self, FileNode};

/// Emitted when the user picks a file.
#[derive(Debug, Clone)]
pub enum ExplorerEvent {
    /// Open `path`. `preview` means a single click: the tab is transient and
    /// the next single click replaces it, so clicking through a tree does not
    /// leave forty tabs behind.
    OpenFile { path: PathBuf, preview: bool },
}

pub struct Explorer {
    focus_handle: FocusHandle,
    root: PathBuf,
    tree: Entity<TreeState>,
    /// Loaded children per directory. The tree's own items are rebuilt from
    /// this, which is what makes lazy expansion possible: `TreeState` flattens
    /// eagerly, so we control what it is given.
    loaded: HashMap<PathBuf, Vec<FileNode>>,
    /// Which directories the user has expanded, so a refresh preserves shape.
    expanded: Vec<PathBuf>,
    /// Directories with a read in flight, so an expand/collapse/expand burst
    /// does not queue three reads of the same directory.
    reading: HashSet<PathBuf>,
    /// Background reads, keyed by the directory being read. Held so dropping
    /// the view cancels them rather than leaving tasks writing into an entity
    /// nobody renders, and keyed so a re-read replaces rather than accumulates.
    _reads: HashMap<PathBuf, Task<()>>,
    _subscriptions: Vec<Subscription>,
}

impl Explorer {
    pub fn new(root: PathBuf, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let tree = cx.new(|cx| TreeState::new(cx));

        let subscriptions = vec![cx.subscribe_in(
            &tree,
            window,
            |this: &mut Self, _, event: &TreeEvent, _, cx| match event {
                TreeEvent::Expanded(id) => this.on_expand(PathBuf::from(id.as_ref()), cx),
                TreeEvent::Collapsed(id) => {
                    let path = PathBuf::from(id.as_ref());
                    this.expanded.retain(|p| p != &path);
                }
            },
        )];

        let mut this = Self {
            focus_handle: cx.focus_handle(),
            root: root.clone(),
            tree,
            loaded: HashMap::new(),
            expanded: Vec::new(),
            reading: HashSet::new(),
            _reads: HashMap::new(),
            _subscriptions: subscriptions,
        };
        // Two levels up front so the first screen is useful without paying for
        // a deep walk — off-thread, so `Explorer::new` returns immediately and
        // the window draws with an empty tree that fills in a frame later.
        this.load_dir(root, 1, cx);
        this
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Re-read the tree from disk, keeping expansion state.
    ///
    /// The old entries stay until each replacement lands. Clearing first would
    /// blank the tree on every filesystem tick, which is a flicker on a watcher
    /// that fires whenever an agent writes.
    pub fn refresh(&mut self, cx: &mut Context<Self>) {
        let dirs: Vec<PathBuf> = std::iter::once(self.root.clone())
            .chain(self.expanded.iter().cloned())
            .collect();
        for dir in dirs {
            self.load_dir(dir, 0, cx);
        }
    }

    fn on_expand(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        if !self.expanded.contains(&path) {
            self.expanded.push(path.clone());
        }
        // Load one level below the newly expanded node, so its children show a
        // disclosure triangle if they in turn have children.
        if !self.loaded.contains_key(&path) {
            self.load_dir(path, 1, cx);
        }
    }

    /// Read `dir` (and `depth` levels below it) on a background task.
    ///
    /// Returns immediately. `read_dir` is a syscall per entry — 29ms for five
    /// thousand files — and a folder of notes is exactly the shape that makes
    /// it expensive, so it never runs on the thread that draws.
    fn load_dir(&mut self, dir: PathBuf, depth: usize, cx: &mut Context<Self>) {
        if !self.reading.insert(dir.clone()) {
            // Already in flight. A second read would produce the same answer
            // and race the first to write it.
            return;
        }
        let key = dir.clone();
        let task = cx.spawn(async move |this, cx| {
            let dir_for_read = dir.clone();
            // One task per level rather than a recursive walk: the top level is
            // what the user sees first, so it should land as soon as it is
            // read rather than waiting on its children.
            let nodes = cx
                .background_spawn(
                    async move { workspace::read_dir(&dir_for_read).unwrap_or_default() },
                )
                .await;

            crate::views::try_update(&this, cx, |this, cx| {
                this.reading.remove(&dir);
                let children: Vec<PathBuf> = if depth > 0 {
                    nodes
                        .iter()
                        .filter(|n| n.is_dir)
                        .map(|n| n.path.clone())
                        .collect()
                } else {
                    Vec::new()
                };
                this.loaded.insert(dir, nodes);
                this.rebuild(cx);
                for child in children {
                    this.load_dir(child, depth - 1, cx);
                }
            });
        });
        // Keyed by directory rather than pushed onto a list: a re-read of the
        // same directory replaces its predecessor, so a watcher firing on every
        // agent write cannot accumulate handles. Dropping the view drops the
        // map, which cancels every read still in flight.
        self._reads.insert(key, task);
    }

    /// Rebuild the tree items from `loaded`.
    fn rebuild(&mut self, cx: &mut Context<Self>) {
        let items = self.build_items(&self.root.clone());
        self.tree.update(cx, |state, cx| state.set_items(items, cx));
        cx.notify();
    }

    fn build_items(&self, dir: &Path) -> Vec<TreeItem> {
        let Some(nodes) = self.loaded.get(dir) else {
            return Vec::new();
        };
        nodes
            .iter()
            .map(|node| {
                let id = node.path.to_string_lossy().to_string();
                let item = TreeItem::new(id, node.name.clone());
                if !node.is_dir {
                    return item;
                }
                let children = self.build_items(&node.path);
                let expanded = self.expanded.contains(&node.path);
                if children.is_empty() {
                    // A directory we have not read yet still needs to look
                    // expandable, otherwise the user can never open it. One
                    // placeholder child does that; expanding replaces it.
                    item.child(TreeItem::new(
                        format!("{}\0loading", node.path.to_string_lossy()),
                        "…",
                    ))
                    .expanded(expanded)
                } else {
                    item.children(children).expanded(expanded)
                }
            })
            .collect()
    }

    fn on_click(&mut self, id: &str, preview: bool, cx: &mut Context<Self>) {
        let path = PathBuf::from(id);
        if path.is_dir() || id.ends_with("\0loading") {
            return;
        }
        // Sniffed here rather than in `read_dir` because this is one file the
        // user just picked, not every entry in the directory: the same check
        // costs one read instead of thousands. It has to happen somewhere —
        // `fs` round-trips through a `String`, so a binary that reaches the
        // editor is re-encoded on save and every unmappable byte is lost. The
        // stamp check in `fs::save` does not help: nothing changed on disk, so
        // the write is authorized and destroys the file anyway.
        if workspace::is_openable(&path) {
            cx.emit(ExplorerEvent::OpenFile { path, preview });
        }
    }
}

/// Icon for a tree entry.
fn icon_for(path: &Path, is_dir: bool, expanded: bool) -> IconName {
    if is_dir {
        return if expanded {
            IconName::FolderOpen
        } else {
            IconName::Folder
        };
    }
    // Agent artifacts get a distinct icon: recognizing them at a glance is the
    // point of treating them as first-class.
    match mt_doc::DocType::of(path) {
        mt_doc::DocType::Skill => IconName::Bot,
        mt_doc::DocType::Agents | mt_doc::DocType::Claude | mt_doc::DocType::CursorRule => {
            IconName::BookOpen
        }
        mt_doc::DocType::Instructions => IconName::BookOpen,
        _ => IconName::File,
    }
}

impl EventEmitter<ExplorerEvent> for Explorer {}

impl Focusable for Explorer {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for Explorer {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let view = cx.entity();

        v_flex()
            .id("explorer")
            .role(gpui::Role::Tree)
            .aria_label("Workspace files")
            .track_focus(&self.focus_handle)
            .size_full()
            .child(
                div()
                    .px(metrics::inset())
                    .py(metrics::header_pad_y())
                    .text_xs()
                    .font_medium()
                    .text_color(cx.theme().muted_foreground)
                    .child(
                        self.root
                            .file_name()
                            .and_then(|n| n.to_str())
                            .unwrap_or("workspace")
                            .to_uppercase(),
                    ),
            )
            .child(
                div().flex_1().min_h_0().child(
                    tree(&self.tree, move |ix, entry, _selected, _window, cx| {
                        let item = entry.item();
                        let id = item.id.to_string();
                        let path = PathBuf::from(&id);
                        let is_dir = entry.is_folder();
                        let icon = icon_for(&path, is_dir, entry.is_expanded());
                        let view = view.clone();

                        ListItem::new(ix)
                            .w_full()
                            .h(metrics::row())
                            .px(metrics::row_pad())
                            // Indentation is padding on the row rather than a
                            // spacer inside it, so the hover highlight still
                            // spans the full width at every depth.
                            .pl(metrics::indent(entry.depth()) + metrics::row_pad())
                            .rounded(cx.theme().radius)
                            .child(
                                h_flex()
                                    .gap(metrics::gap())
                                    .items_center()
                                    .child(Icon::new(icon).text_color(if is_dir {
                                        cx.theme().muted_foreground
                                    } else {
                                        cx.theme().foreground
                                    }))
                                    .child(div().text_sm().child(item.label.clone())),
                            )
                            .on_click(move |event: &ClickEvent, _, cx| {
                                // Single click previews, double click pins —
                                // the rule every file tree in an editor uses.
                                let preview = event.click_count() < 2;
                                view.update(cx, |this, cx| this.on_click(&id, preview, cx));
                            })
                    })
                    .size_full(),
                ),
            )
    }
}

#[cfg(test)]
mod tests {
    // Import selectively: `gpui::*` in the parent re-exports a `test` attribute
    // macro that shadows the built-in one and blows the recursion limit.
    use super::icon_for;
    use gpui_component::IconNamed as _;
    use std::path::Path;

    /// `IconName` is macro-generated and implements neither `PartialEq` nor
    /// `Debug`, so compare the SVG paths it resolves to.
    fn icon(path: &str, is_dir: bool, expanded: bool) -> String {
        icon_for(Path::new(path), is_dir, expanded)
            .path()
            .to_string()
    }

    #[test]
    fn agent_artifacts_get_distinct_icons() {
        let skill = icon("s/SKILL.md", false, false);
        let agents = icon("AGENTS.md", false, false);
        let plain = icon("README.md", false, false);
        assert_ne!(skill, plain, "a skill must be distinguishable at a glance");
        assert_ne!(agents, plain);
        assert_eq!(agents, icon("repo/CLAUDE.md", false, false));
    }

    #[test]
    fn directory_icons_reflect_expansion() {
        assert_ne!(icon("d", true, false), icon("d", true, true));
        assert_ne!(icon("d", true, false), icon("f.md", false, false));
    }

    /// The click handler is the only production open gate in this view, so the
    /// content sniff has to sit on it or it protects nothing.
    ///
    /// What broke before: `on_click` gated on `DocType::of(..).is_document()`
    /// alone, which is an extension allowlist, so a NUL-filled `.log` reached
    /// the editor as a decoded `String` — and saving re-encodes from that
    /// `String`, so every byte the decoder could not map was gone. `fs::save`
    /// does refuse a write when the file changed on disk, which is a different
    /// failure and no help here: nothing changed, so the write goes through.
    /// `workspace::is_openable` was computed for every tree entry and read by
    /// nobody. Asserted against the source because reaching the handler needs
    /// a real window.
    #[test]
    fn opening_a_file_goes_through_the_binary_check() {
        let source = crate::views::production_source(include_str!("explorer.rs"));
        let start = source.find("fn on_click").expect("the click handler");
        let body = &source[start..];
        let end = body.find("\n}").unwrap_or(body.len());
        let body = &body[..end];

        assert!(
            body.contains("workspace::is_openable"),
            "the open gate must sniff contents; `DocType::of` alone is an \
             extension allowlist and admits a binary wearing a text extension"
        );
    }
}
