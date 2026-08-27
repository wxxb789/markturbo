//! Where the user has been: the Back/Forward pair and the list it walks.
//!
//! Split out of [`super::Workspace`] because none of it needs a window. The
//! list is plain data — a `History` in a test behaves exactly as the one the
//! app walks — so "going back and then somewhere new abandons the branch you
//! left" is an assertion rather than a hope.
//!
//! The navigator renders here too. It is two buttons whose whole state is
//! `can_go_back` / `can_go_forward`, and keeping them beside the rules they
//! read is what stops a disabled button from disagreeing with the list.

use std::path::{Path, PathBuf};

use gpui::prelude::FluentBuilder as _;
use gpui::*;
use gpui_component::{IconName, h_flex};

use super::{ChromeIconButton, NavigateBack, NavigateForward, Workspace};
use crate::i18n;

/// Where the user has been, so Back and Forward mean something.
///
/// Positions rather than tabs: two visits to the same document at different
/// offsets are two entries, which is what makes Back useful after following a
/// search result or an outline click. VS Code and every browser work this way.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Visit {
    path: PathBuf,
    offset: usize,
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
pub(super) struct History {
    visits: Vec<Visit>,
    /// Index of the current position. `None` before anything is visited.
    cursor: Option<usize>,
    /// Set while navigating, so the resulting open does not record itself as a
    /// new visit and truncate the forward half we are moving through.
    navigating: bool,
}

impl History {
    /// Record arriving somewhere.
    ///
    /// Everything after the cursor is dropped: going back and then somewhere
    /// new abandons the branch you left, which is what every browser does and
    /// what makes Forward mean "where I was" rather than "where I once was".
    fn push(&mut self, visit: Visit) {
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

    fn can_go_back(&self) -> bool {
        self.cursor.is_some_and(|ix| ix > 0)
    }

    fn can_go_forward(&self) -> bool {
        self.cursor.is_some_and(|ix| ix + 1 < self.visits.len())
    }

    fn back(&mut self) -> Option<Visit> {
        let ix = self.cursor?.checked_sub(1)?;
        self.cursor = Some(ix);
        self.visits.get(ix).cloned()
    }

    fn forward(&mut self) -> Option<Visit> {
        let ix = self.cursor? + 1;
        let visit = self.visits.get(ix).cloned()?;
        self.cursor = Some(ix);
        Some(visit)
    }

    /// Drop every visit to `path`, e.g. when its tab closes.
    ///
    /// Without this, Back reopens a tab the user just closed — which reads as
    /// the close button not working.
    pub(super) fn forget(&mut self, path: &Path) {
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

impl Workspace {
    /// Note that the user is now at `offset` in `path`.
    pub(super) fn record_visit(&mut self, path: PathBuf, offset: usize) {
        self.history.push(Visit { path, offset });
    }

    /// Go to `visit`, without recording the move as a new visit.
    fn go_to(&mut self, visit: Visit, window: &mut Window, cx: &mut Context<Self>) {
        // The flag is what keeps Back from truncating the forward half it is
        // walking through.
        self.history.navigating = true;
        self.open_file_as(visit.path.clone(), true, window, cx);
        if let Some(doc) = self.active_document().cloned()
            && doc.read(cx).path() == visit.path
        {
            doc.update(cx, |doc, cx| doc.reveal_offset(visit.offset, window, cx));
        }
        self.history.navigating = false;
        cx.notify();
    }

    pub(super) fn on_navigate_back(
        &mut self,
        _: &NavigateBack,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(visit) = self.history.back() {
            self.go_to(visit, window, cx);
        }
    }

    pub(super) fn on_navigate_forward(
        &mut self,
        _: &NavigateForward,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(visit) = self.history.forward() {
            self.go_to(visit, window, cx);
        }
    }

    /// Back and Forward, at the far left of the bar.
    ///
    /// A disabled button rather than a hidden one: the pair is a fixed landmark
    /// that the tabs start after, and one that appears and disappears would
    /// shift every tab sideways as the user navigates. Disabled says "there is
    /// nowhere to go" — which is the actual state — where absent says nothing.
    pub(super) fn render_navigator(&self, tooltips: bool, cx: &Context<Self>) -> impl IntoElement {
        h_flex()
            .flex_shrink_0()
            .items_center()
            .gap_0p5()
            // The bar is a `WindowControlArea::Drag` region, so a press here
            // becomes a window drag unless it is claimed back.
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .child(
                ChromeIconButton::new(
                    "nav-back",
                    IconName::ArrowLeft,
                    i18n::t(i18n::Key::NavigateBack, cx),
                )
                .disabled(!self.history.can_go_back())
                .when(tooltips, |button| {
                    button.tooltip(i18n::t(i18n::Key::NavigateBack, cx))
                })
                .on_click(cx.listener(|this, _, window, cx| {
                    this.on_navigate_back(&NavigateBack, window, cx)
                })),
            )
            .child(
                ChromeIconButton::new(
                    "nav-forward",
                    IconName::ArrowRight,
                    i18n::t(i18n::Key::NavigateForward, cx),
                )
                .disabled(!self.history.can_go_forward())
                .when(tooltips, |button| {
                    button.tooltip(i18n::t(i18n::Key::NavigateForward, cx))
                })
                .on_click(cx.listener(|this, _, window, cx| {
                    this.on_navigate_forward(&NavigateForward, window, cx)
                })),
            )
    }
}

#[cfg(test)]
mod tests {
    // Nothing is glob-imported: the `gpui::*` above re-exports a `test`
    // attribute macro that shadows the built-in one and blows the recursion
    // limit.

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
        history.navigating = true;
        history.push(target);
        history.navigating = false;

        assert!(
            history.can_go_forward(),
            "walking back must leave the forward half intact"
        );
        assert_eq!(
            history.forward().map(|v| v.path),
            Some(PathBuf::from("c.md"))
        );
    }

    /// The navigator leads the tab strip, and its buttons disable rather than
    /// disappear in every document layout.
    ///
    /// A hidden button would shift every tab sideways as the user navigates,
    /// which is exactly the kind of motion that makes a strip hard to aim at.
    #[test]
    fn the_navigator_leads_the_tabs_and_never_vanishes() {
        let source = include_str!("history.rs");
        let start = source
            .find("fn render_navigator")
            .expect("render_navigator must exist");
        let body = &source[start..];
        let end = body.find("\n#[cfg(test)]").unwrap_or(body.len());
        let nav = &body[..end];

        assert!(
            nav.contains("can_go_back()") && nav.contains("can_go_forward()"),
            "both directions must reflect whether there is anywhere to go"
        );
        assert_eq!(
            nav.matches(".disabled(").count(),
            2,
            "both buttons must disable rather than be conditionally rendered"
        );
        assert_eq!(
            nav.matches(".when(tooltips, |button|").count(),
            2,
            "Web mode keeps the fixed buttons but suppresses popup tooltips"
        );
        assert_eq!(
            nav.matches("ChromeIconButton::new(").count(),
            2,
            "both fixed navigation buttons must remain in the element tree"
        );

        // And it is childed before the tab strip, which is a fact about the
        // title bar rather than about this file.
        let workspace = crate::views::production_source(include_str!("../workspace.rs"));
        let bar_start = workspace
            .find("fn render_document_title_controls")
            .expect("render_document_title_controls");
        let bar = &workspace[bar_start..];
        let bar = bar
            .split("\n    fn render_right_title_bar")
            .next()
            .unwrap_or(bar);
        let chrome = workspace
            .split_once("impl RenderOnce for ChromeIconButton")
            .expect("the shared icon-button behavior")
            .1;
        let chrome = chrome
            .split("/// Full-width sidebar navigation")
            .next()
            .unwrap_or(chrome);
        assert_eq!(
            chrome.matches(".accessibility_label(self.label)").count(),
            2,
            "both push and toggle variants need names even when WebView \
             suppresses popup tooltips"
        );
        assert_eq!(
            chrome.matches(".size(metrics::target())").count(),
            2,
            "both variants must keep the repository's 24px pointer target"
        );
        assert_eq!(
            chrome.matches("window.prevent_default()").count(),
            2,
            "pointer activation must preserve editor focus for both variants"
        );
        assert!(
            chrome.contains("let inert = disabled || loading")
                && chrome.matches(".disabled(inert)").count() == 2,
            "loading and disabled chrome commands must be inert to keyboard \
             and accessibility clients, not only missing a click handler"
        );
        assert!(
            bar.contains("self.render_navigator(!web_active, cx)"),
            "Web layouts must render the fixed navigator without tooltips"
        );
        let navigator = bar.find(".child(navigator)").expect("the navigator");
        let tabs = bar.find("self.render_tabs(cx)").expect("the tab strip");
        assert!(navigator < tabs, "Back/Forward come before the tabs");
    }
}
