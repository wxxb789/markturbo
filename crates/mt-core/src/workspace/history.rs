//! Pure Back/Forward history state for workspace navigation.
//!
//! Split out of `Workspace` because none of it needs a window. The
//! list is plain data — a `History` in a test behaves exactly as the one the
//! app walks — so "going back and then somewhere new abandons the branch you
//! left" is an assertion rather than a hope.
//!
use std::path::{Path, PathBuf};

/// Where the user has been, so Back and Forward mean something.
///
/// Positions rather than tabs: two visits to the same document at different
/// offsets are two entries, which is what makes Back useful after following a
/// search result or an outline click. VS Code and every browser work this way.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Visit {
    pub path: PathBuf,
    pub offset: usize,
}

/// How many visits to remember.
///
/// Bounded because this grows with every click in a result list and nobody
/// navigates back through hundreds of them.
const HISTORY_LIMIT: usize = 64;

/// Back/forward over [`Visit`]s.
///
/// A cursor into one list rather than two stacks: the two-stack version is the
/// same thing with more places to forget to clear the forward half.
#[derive(Debug, Default)]
pub struct History {
    visits: Vec<Visit>,
    /// Index of the current position. `None` before anything is visited.
    cursor: Option<usize>,
    /// Set while navigating, so the resulting open does not record itself as a
    /// new visit and truncate the forward half we are moving through.
    navigating: bool,
}

impl History {
    /// Mark whether an open is part of a Back/Forward move.
    ///
    /// While navigating, [`History::push`] ignores the arrival so it does not
    /// truncate the forward branch being followed.
    pub fn set_navigating(&mut self, navigating: bool) {
        self.navigating = navigating;
    }

    /// Record arriving somewhere.
    ///
    /// Everything after the cursor is dropped: going back and then somewhere
    /// new abandons the branch you left, which is what every browser does and
    /// what makes Forward mean "where I was" rather than "where I once was".
    pub fn push(&mut self, visit: Visit) {
        if self.navigating {
            return;
        }
        if self.current() == Some(&visit) {
            return;
        }
        match self.cursor {
            Some(ix) => self.visits.truncate(ix + 1),
            None => self.visits.clear(),
        }
        self.visits.push(visit);
        if self.visits.len() > HISTORY_LIMIT {
            self.visits.remove(0);
        }
        self.cursor = Some(self.visits.len() - 1);
    }

    fn current(&self) -> Option<&Visit> {
        self.visits.get(self.cursor?)
    }

    pub fn can_go_back(&self) -> bool {
        self.cursor.is_some_and(|ix| ix > 0)
    }

    pub fn can_go_forward(&self) -> bool {
        self.cursor.is_some_and(|ix| ix + 1 < self.visits.len())
    }

    pub fn back(&mut self) -> Option<Visit> {
        let ix = self.cursor?.checked_sub(1)?;
        self.cursor = Some(ix);
        self.visits.get(ix).cloned()
    }

    pub fn forward(&mut self) -> Option<Visit> {
        let ix = self.cursor? + 1;
        let visit = self.visits.get(ix).cloned()?;
        self.cursor = Some(ix);
        Some(visit)
    }

    /// Drop every visit to `path`, e.g. when its tab closes.
    ///
    /// Without this, Back reopens a tab the user just closed — which reads as
    /// the close button not working.
    pub fn forget(&mut self, path: &Path) {
        let current = self.current().cloned();
        self.visits.retain(|v| v.path != path);
        self.cursor = match current {
            // Keep pointing at the same visit if it survived; otherwise land on
            // the end, which is where "most recent" lives.
            Some(current) if current.path != path => self.visits.iter().position(|v| *v == current),
            _ => self.visits.len().checked_sub(1),
        };
    }
}

#[cfg(test)]
mod tests {
    /// Closing a tab must not leave it reachable through Back.
    ///
    /// Runtime rather than source-level, because the history is plain data with
    /// no GPUI in it — and the failure is subtle enough to deserve a real
    /// assertion: without `forget`, Back reopens the tab the user just closed,
    /// which reads as the close button not working.
    #[test]
    fn closing_a_document_forgets_its_visits() {
        use super::{History, Visit};
        use std::path::PathBuf;

        let mut history = History::default();
        for (path, offset) in [("a.md", 0), ("b.md", 0), ("a.md", 40), ("c.md", 0)] {
            history.push(Visit {
                path: PathBuf::from(path),
                offset,
            });
        }
        history.forget(std::path::Path::new("a.md"));
        assert!(
            !history
                .visits
                .iter()
                .any(|v| v.path.as_path() == std::path::Path::new("a.md")),
            "every visit to the closed document must go, not just the latest"
        );
        // And the cursor still points inside the list.
        assert!(history.cursor.is_some_and(|ix| ix < history.visits.len()));
    }

    /// Going back and then somewhere new abandons the forward branch.
    ///
    /// The behavior every browser has, and the reason Forward means "where I
    /// was" rather than "where I once was". Getting this wrong produces a
    /// Forward button that jumps somewhere the user never went from here.
    #[test]
    fn a_new_visit_after_going_back_truncates_the_forward_half() {
        use super::{History, Visit};
        use std::path::PathBuf;

        let visit = |name: &str| Visit {
            path: PathBuf::from(name),
            offset: 0,
        };
        let mut history = History::default();
        history.push(visit("a.md"));
        history.push(visit("b.md"));
        history.push(visit("c.md"));

        assert_eq!(history.back().map(|v| v.path), Some(PathBuf::from("b.md")));
        assert!(history.can_go_forward());

        history.push(visit("d.md"));
        assert!(
            !history.can_go_forward(),
            "c.md is on a branch the user left; Forward must not go there"
        );
        assert_eq!(history.back().map(|v| v.path), Some(PathBuf::from("b.md")));
    }

    /// Navigating must not record the navigation as a new visit.
    ///
    /// Without the guard, pressing Back records arriving at the previous entry,
    /// which truncates everything after it — so Back works once and Forward is
    /// dead from then on.
    #[test]
    fn navigating_does_not_rewrite_the_history_it_walks() {
        use super::{History, Visit};
        use std::path::PathBuf;

        let visit = |name: &str| Visit {
            path: PathBuf::from(name),
            offset: 0,
        };
        let mut history = History::default();
        history.push(visit("a.md"));
        history.push(visit("b.md"));
        history.push(visit("c.md"));

        let target = history.back().expect("somewhere to go back to");
        // What `go_to` does around the open it triggers.
        history.set_navigating(true);
        history.push(target);
        history.set_navigating(false);

        assert!(
            history.can_go_forward(),
            "walking back must leave the forward half intact"
        );
        assert_eq!(
            history.forward().map(|v| v.path),
            Some(PathBuf::from("c.md"))
        );
    }
}
