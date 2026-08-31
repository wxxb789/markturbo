//! The set of open tabs: which documents, which one is active, which is the
//! preview slot.
//!
//! Split out of [`crate::views::workspace::Workspace`] because nine separate
//! concerns reached into `documents` and `active` there, and every bug that
//! lived in that reaching needed a real window to reproduce. Here the indexing
//! rules are plain data — `Tabs<()>` in a test behaves exactly as
//! `Tabs<DocumentTab>` does in the app, so "closing the tab left of the active
//! one must not switch documents" is an assertion rather than a hope.
//!
//! `T` is whatever a tab needs to carry beyond its path. The app puts the
//! document view and its subscriptions there; a test puts `()`. That is the
//! whole reason for the parameter — two real instantiations, not a hypothetical
//! seam.

use std::path::{Path, PathBuf};

/// One open tab.
#[derive(Debug)]
pub struct Tab<T> {
    /// The document's path, cached here so index arithmetic never needs an
    /// `App` to answer "which tab is this file in?".
    pub path: PathBuf,
    pub payload: T,
}

/// The open tabs and the cursor into them.
#[derive(Debug)]
pub struct Tabs<T> {
    tabs: Vec<Tab<T>>,
    /// Index of the active tab. Meaningless when `tabs` is empty; every reader
    /// goes through [`Tabs::active`], which returns `None` then.
    active: usize,
    /// The tab opened as a preview, if any.
    ///
    /// VS Code's rule: a single click opens a document in italics and reuses
    /// that slot for the next single click, so browsing a tree does not leave
    /// forty tabs behind. A double click, an edit, or an explicit open promotes
    /// it. Exactly one slot, identified by path rather than index so it
    /// survives tabs closing to its left.
    preview: Option<PathBuf>,
    /// Which tab the context menu was opened on.
    ///
    /// Not the active tab: right-clicking a tab opens its menu without
    /// selecting it, so the copy actions would otherwise act on whichever
    /// document happened to be focused. Cleared when the menu closes, because a
    /// value that outlived its menu made every later keyboard invocation act on
    /// a tab the user right-clicked minutes ago.
    menu: Option<usize>,
}

impl<T> Default for Tabs<T> {
    fn default() -> Self {
        Self {
            tabs: Vec::new(),
            active: 0,
            preview: None,
            menu: None,
        }
    }
}

impl<T> Tabs<T> {
    pub fn is_empty(&self) -> bool {
        self.tabs.is_empty()
    }

    pub fn len(&self) -> usize {
        self.tabs.len()
    }

    pub fn iter(&self) -> impl Iterator<Item = &Tab<T>> {
        self.tabs.iter()
    }

    pub fn get(&self, ix: usize) -> Option<&Tab<T>> {
        self.tabs.get(ix)
    }

    /// The active tab, or `None` when nothing is open.
    pub fn active(&self) -> Option<&Tab<T>> {
        self.tabs.get(self.active)
    }

    /// The active tab's index. Only meaningful alongside [`Tabs::active`].
    pub fn active_index(&self) -> usize {
        self.active
    }

    pub fn index_of(&self, path: &Path) -> Option<usize> {
        self.tabs.iter().position(|t| t.path == path)
    }

    pub fn paths(&self) -> impl Iterator<Item = &Path> {
        self.tabs.iter().map(|t| t.path.as_path())
    }

    /// Focus an existing tab. Returns false when the index is out of range.
    pub fn focus(&mut self, ix: usize) -> bool {
        if ix >= self.tabs.len() {
            return false;
        }
        self.active = ix;
        true
    }

    /// Focus the tab showing `path`, if it is open.
    pub fn focus_path(&mut self, path: &Path) -> bool {
        match self.index_of(path) {
            Some(ix) => self.focus(ix),
            None => false,
        }
    }

    /// Append a tab and make it active. Returns its index.
    pub fn push(&mut self, path: PathBuf, payload: T) -> usize {
        self.tabs.push(Tab { path, payload });
        self.active = self.tabs.len() - 1;
        self.active
    }

    /// Update a tab's cached path after Save As.
    pub fn replace_path(&mut self, ix: usize, path: PathBuf) -> bool {
        let Some(tab) = self.tabs.get_mut(ix) else {
            return false;
        };
        if self.preview.as_deref() == Some(tab.path.as_path()) {
            self.preview = Some(path.clone());
        }
        tab.path = path;
        self.menu = None;
        true
    }

    /// Close the tab at `ix`, returning its path and payload.
    ///
    /// The active index *shifts* rather than clamping. Clamping was the bug:
    /// with `[A, B, C]` and B active, closing A left `active = min(1, 1) = 1`,
    /// which is now C — closing a tab on the left silently switched the user to
    /// a different document. The rule is the one every editor uses: a tab
    /// closing to the left of the active one pulls it along, and closing the
    /// active one lands on its right-hand neighbour (or the new last tab).
    pub fn close(&mut self, ix: usize) -> Option<(PathBuf, T)> {
        if ix >= self.tabs.len() {
            return None;
        }
        let Tab { path, payload } = self.tabs.remove(ix);

        if self.tabs.is_empty() {
            self.active = 0;
        } else if ix < self.active {
            self.active -= 1;
        } else if ix == self.active {
            self.active = self.active.min(self.tabs.len() - 1);
        }

        // A preview slot naming the tab that just closed would be handed to the
        // next preview open, which would then fail to find it and leave the
        // slot pointing at nothing.
        if self.preview.as_deref() == Some(path.as_path()) {
            self.preview = None;
        }
        // Indices after `ix` all moved down by one, so a menu index recorded
        // before this close now names a different tab.
        self.menu = None;

        Some((path, payload))
    }

    /// The path of the tab opened as a preview, if any.
    pub fn preview(&self) -> Option<&Path> {
        self.preview.as_deref()
    }

    pub fn is_preview(&self, path: &Path) -> bool {
        self.preview.as_deref() == Some(path)
    }

    /// Set the preview slot, or clear it with `None`.
    pub fn set_preview(&mut self, path: Option<PathBuf>) {
        self.preview = path;
    }

    /// Take the preview slot, leaving it empty.
    pub fn take_preview(&mut self) -> Option<PathBuf> {
        self.preview.take()
    }

    /// Record which tab a context menu was opened on.
    pub fn set_menu(&mut self, ix: usize) {
        self.menu = Some(ix);
    }

    /// The tab a menu action should act on: the right-clicked one, else the
    /// active one.
    ///
    /// [`Tabs::clear_menu`] is what makes the fallback real. Without it the
    /// field stayed `Some` forever after the first right-click, so every later
    /// keyboard invocation acted on that tab instead of the active one.
    pub fn menu_target(&self) -> Option<&Tab<T>> {
        self.tabs.get(self.menu.unwrap_or(self.active))
    }

    pub fn clear_menu(&mut self) {
        self.menu = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(name: &str) -> PathBuf {
        PathBuf::from(name)
    }

    /// Three tabs, `active` left where `push` put it: on the last one.
    fn abc() -> Tabs<()> {
        let mut tabs = Tabs::default();
        tabs.push(p("a.md"), ());
        tabs.push(p("b.md"), ());
        tabs.push(p("c.md"), ());
        tabs
    }

    #[test]
    fn closing_a_tab_to_the_left_keeps_the_same_document_active() {
        // The bug: `active = active.min(len - 1)` clamped instead of shifting,
        // so `[a, b, c]` with b active and a closed left `active = 1` — which
        // after the removal is c. Clicking a close button switched documents.
        let mut tabs = abc();
        tabs.focus(1);
        assert_eq!(tabs.active().unwrap().path, p("b.md"));

        tabs.close(0);
        assert_eq!(
            tabs.active().unwrap().path,
            p("b.md"),
            "closing a.md must leave b.md active, not jump to c.md"
        );
    }

    #[test]
    fn closing_the_active_tab_lands_on_its_neighbour() {
        let mut tabs = abc();
        tabs.focus(1);
        tabs.close(1);
        assert_eq!(
            tabs.active().unwrap().path,
            p("c.md"),
            "the tab that slid into the closed slot"
        );

        // Closing the last tab has no right-hand neighbour, so it falls back.
        let mut tabs = abc();
        tabs.focus(2);
        tabs.close(2);
        assert_eq!(tabs.active().unwrap().path, p("b.md"));
    }

    #[test]
    fn closing_a_tab_to_the_right_does_not_move_the_cursor() {
        let mut tabs = abc();
        tabs.focus(0);
        tabs.close(2);
        assert_eq!(tabs.active().unwrap().path, p("a.md"));
    }

    #[test]
    fn the_active_index_is_never_out_of_range() {
        // Whatever order tabs close in, `active()` must either name a real tab
        // or be `None` — an index past the end is what renders a blank pane.
        for order in [[0, 0, 0], [2, 1, 0], [1, 1, 0], [2, 0, 0]] {
            let mut tabs = abc();
            tabs.focus(1);
            for ix in order {
                tabs.close(ix);
                assert!(
                    tabs.active_index() < tabs.len().max(1),
                    "active {} out of range for {} tabs",
                    tabs.active_index(),
                    tabs.len()
                );
            }
            assert!(tabs.is_empty() && tabs.active().is_none());
        }
    }

    #[test]
    fn closing_the_preview_tab_empties_the_slot() {
        // Left set, the next preview open takes a path whose tab is gone, then
        // fails to find it — so the outgoing preview is never replaced and the
        // bar accumulates tabs, which is the one thing preview mode prevents.
        let mut tabs = abc();
        tabs.set_preview(Some(p("b.md")));
        tabs.close(1);
        assert_eq!(tabs.preview(), None);
    }

    #[test]
    fn closing_a_different_tab_leaves_the_preview_slot_alone() {
        let mut tabs = abc();
        tabs.set_preview(Some(p("b.md")));
        tabs.close(0);
        assert_eq!(tabs.preview(), Some(p("b.md").as_path()));
    }

    #[test]
    fn a_menu_index_does_not_outlive_the_tab_list_it_indexed() {
        // Two failures in one field. Stale: indices shift on close, so a menu
        // index recorded before it names a different document after. Sticky:
        // the field was never cleared, so `menu_target`'s documented fallback
        // to the active tab stopped happening after the first right-click.
        let mut tabs = abc();
        tabs.set_menu(2);
        assert_eq!(tabs.menu_target().unwrap().path, p("c.md"));

        tabs.close(0);
        tabs.focus(0);
        assert_eq!(
            tabs.menu_target().unwrap().path,
            p("b.md"),
            "after a close the menu index is dropped and the active tab answers"
        );

        // And closing the menu restores the fallback without needing a close.
        let mut tabs = abc();
        tabs.focus(0);
        tabs.set_menu(2);
        tabs.clear_menu();
        assert_eq!(tabs.menu_target().unwrap().path, p("a.md"));
    }

    #[test]
    fn the_menu_target_falls_back_to_the_active_tab() {
        let mut tabs = abc();
        tabs.focus(1);
        assert_eq!(tabs.menu_target().unwrap().path, p("b.md"));
        assert!(Tabs::<()>::default().menu_target().is_none());
    }

    #[test]
    fn closing_returns_the_payload_so_its_subscriptions_drop() {
        // The leak this closes: subscriptions were pushed into a Vec on the
        // workspace and the entity was removed from another, so closing a tab
        // freed the document and kept two subscriptions to it forever. Carrying
        // them in the payload means `close` is the only place they can go.
        let mut tabs: Tabs<&'static str> = Tabs::default();
        tabs.push(p("a.md"), "subscriptions");
        let (path, payload) = tabs.close(0).expect("the tab exists");
        assert_eq!(path, p("a.md"));
        assert_eq!(payload, "subscriptions");
        assert!(tabs.is_empty());
    }

    #[test]
    fn closing_out_of_range_is_a_no_op() {
        let mut tabs = abc();
        assert!(tabs.close(9).is_none());
        assert_eq!(tabs.len(), 3);
    }

    #[test]
    fn focus_rejects_an_index_it_cannot_honor() {
        let mut tabs = abc();
        tabs.focus(0);
        assert!(!tabs.focus(9));
        assert_eq!(tabs.active_index(), 0, "the cursor did not move");
        assert!(tabs.focus_path(&p("c.md")));
        assert_eq!(tabs.active().unwrap().path, p("c.md"));
        assert!(!tabs.focus_path(&p("nope.md")));
    }

    #[test]
    fn save_as_updates_the_tab_and_preview_identity() {
        let mut tabs = Tabs::default();
        tabs.push(p("old.md"), ());
        tabs.set_preview(Some(p("old.md")));

        assert!(tabs.replace_path(0, p("new.md")));
        assert_eq!(tabs.active().unwrap().path, p("new.md"));
        assert_eq!(tabs.preview(), Some(Path::new("new.md")));
        assert!(!tabs.replace_path(1, p("missing.md")));
    }
}
