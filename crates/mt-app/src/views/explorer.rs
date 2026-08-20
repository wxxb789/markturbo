//! File explorer: the native workspace file tree.
//!
//! Lazy: a directory's children are read when it expands, so opening a monorepo
//! does not stall on a full walk.

use std::collections::HashMap;
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
    OpenFile(PathBuf),
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
            _subscriptions: subscriptions,
        };
        // Two levels up front so the first screen is useful without paying for
        // a deep walk.
        this.load_dir(&root, 1);
        this.rebuild(cx);
        this
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Re-read the tree from disk, keeping expansion state.
    pub fn refresh(&mut self, cx: &mut Context<Self>) {
        let dirs: Vec<PathBuf> = std::iter::once(self.root.clone())
            .chain(self.expanded.iter().cloned())
            .collect();
        self.loaded.clear();
        for dir in dirs {
            self.load_dir(&dir, 0);
        }
        self.rebuild(cx);
    }

    fn on_expand(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        if !self.expanded.contains(&path) {
            self.expanded.push(path.clone());
        }
        // Load one level below the newly expanded node, so its children show a
        // disclosure triangle if they in turn have children.
        if !self.loaded.contains_key(&path) {
            self.load_dir(&path, 1);
            self.rebuild(cx);
        }
    }

    fn load_dir(&mut self, dir: &Path, depth: usize) {
        let Ok(nodes) = workspace::read_dir(dir) else {
            return;
        };
        if depth > 0 {
            for child in nodes.iter().filter(|n| n.is_dir) {
                self.load_dir(&child.path, depth - 1);
            }
        }
        self.loaded.insert(dir.to_path_buf(), nodes);
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

    fn on_click(&mut self, id: &str, cx: &mut Context<Self>) {
        let path = PathBuf::from(id);
        if path.is_dir() || id.ends_with("\0loading") {
            return;
        }
        if mt_doc::DocType::of(&path).is_document() {
            cx.emit(ExplorerEvent::OpenFile(path));
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
                            .on_click(move |_, _, cx| {
                                view.update(cx, |this, cx| this.on_click(&id, cx));
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
}
