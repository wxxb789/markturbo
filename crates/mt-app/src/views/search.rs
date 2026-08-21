//! The Search panel.
//!
//! Four scopes, one list. What varies between them is only *which documents*
//! are searched — the matching itself lives in [`mt_doc::search`], so a result
//! row means the same thing no matter where it came from.
//!
//! The scopes are not arbitrary. Each answers a question a person actually has
//! while reading agent artifacts: "where in this file", "which of my open tabs",
//! "anywhere in this project", "which skill or instruction file mentions this".
//! The last is the one no general-purpose editor offers, because it searches
//! directories that are not under the open folder at all.
//!
//! # Cost
//!
//! Everything past the debounce runs on a background task, which is what keeps
//! the window live while it works. Measured on a 6,642-document vault
//! (`cargo test --release -p mt-app --test search_cost -- --ignored`):
//!
//! | Query | Cost |
//! |---|---|
//! | a common word | ~14ms — exits at the cap almost immediately |
//! | a word in ~170 files | ~760ms |
//! | no match at all | ~2.6s — walks and reads everything |
//!
//! The last row is the worst case a user can produce by typing, and it is the
//! one the cap cannot help with: nothing stops early when nothing is found. It
//! is bounded by the debounce instead — one run per settled query, not one per
//! keystroke.

use std::path::PathBuf;
use std::time::Duration;

use gpui::*;
use gpui_component::{
    ActiveTheme as _, Icon, IconName, Sizable as _, h_flex,
    input::{Input, InputEvent, InputState},
    list::ListItem,
    tab::{Tab, TabBar},
    v_flex,
};
use mt_doc::search::{self, Query, Results};

use crate::i18n;
use crate::metrics;

/// Emitted when the user picks a result.
#[derive(Debug, Clone)]
pub enum SearchEvent {
    /// The query or scope changed and the debounce has elapsed.
    ///
    /// The view cannot answer this itself: the open tabs' authoritative text
    /// lives in their editors and the harness paths in the harness view. The
    /// workspace collects a [`Corpus`] and hands it back through [`SearchView::run`].
    Ready,
    /// Open `path` and put the cursor at `offset`.
    ///
    /// Always a preview open: scanning down a result list is exactly the
    /// browsing pattern preview tabs exist for.
    Reveal { path: PathBuf, offset: usize },
}

/// Where to search.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scope {
    /// The active document only.
    Document,
    /// Every open tab.
    OpenTabs,
    /// Every document under the open folder.
    Folder,
    /// Every skill and instruction file the harness scan found, including the
    /// global ones — which is what makes this different from Folder rather
    /// than a slower version of it.
    Harness,
}

impl Scope {
    pub const ALL: [Scope; 4] = [
        Scope::Document,
        Scope::OpenTabs,
        Scope::Folder,
        Scope::Harness,
    ];

    fn label(self) -> i18n::Key {
        match self {
            Scope::Document => i18n::Key::ScopeDocument,
            Scope::OpenTabs => i18n::Key::ScopeOpenTabs,
            Scope::Folder => i18n::Key::ScopeFolder,
            Scope::Harness => i18n::Key::ScopeHarness,
        }
    }
}

/// How long after the last keystroke to run the search.
///
/// Longer than the editor's reparse debounce: a search reads files, and every
/// intermediate prefix of a word the user is typing would read all of them.
const DEBOUNCE: Duration = Duration::from_millis(250);

/// The documents to search, gathered by the workspace.
///
/// The view cannot collect these itself — the open tabs' authoritative text is
/// in their editors, and the harness roots belong to the harness view. Passing
/// a snapshot keeps this view from reaching across the workspace to find them.
///
/// `roots` holds *directories*, not their contents, and that is load-bearing
/// rather than tidy. Walking a real vault takes seconds — measured at **2.4s
/// for 6,642 documents** — and the workspace assembles this on the UI thread,
/// so expanding a root here would freeze the window for that long on every
/// settled keystroke. The expansion happens on the background task instead.
#[derive(Debug, Clone, Default)]
pub struct Corpus {
    /// Documents whose text is already in memory, because they are open and may
    /// hold unsaved edits. Searching the file on disk instead would report
    /// matches that are no longer there.
    pub open: Vec<(PathBuf, String)>,
    /// Individual documents to read from disk.
    pub files: Vec<PathBuf>,
    /// Directories to walk for documents, off the UI thread.
    pub roots: Vec<PathBuf>,
}

pub struct SearchView {
    focus_handle: FocusHandle,
    input: Entity<InputState>,
    scope: Scope,
    results: Results,
    /// True while a search is in flight, so an empty list is not mistaken for
    /// "no matches" before the answer arrives.
    running: bool,
    /// The query the current `results` answer, so the row count is never
    /// attributed to a query the user has since changed.
    answered: String,
    _search: Option<Task<()>>,
    _subscriptions: Vec<Subscription>,
}

impl SearchView {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let input = cx.new(|cx| InputState::new(window, cx).placeholder("Search…"));
        let subscriptions = vec![cx.subscribe_in(
            &input,
            window,
            |this: &mut Self, _, event: &InputEvent, _, cx| {
                if matches!(event, InputEvent::Change) {
                    this.schedule(cx);
                }
            },
        )];

        Self {
            focus_handle: cx.focus_handle(),
            input,
            scope: Scope::Document,
            results: Results::default(),
            running: false,
            answered: String::new(),
            _search: None,
            _subscriptions: subscriptions,
        }
    }

    /// Focus the query field, for the keybinding that opens this panel.
    pub fn focus_query(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.input.update(cx, |input, cx| input.focus(window, cx));
    }

    pub fn scope(&self) -> Scope {
        self.scope
    }

    /// The current query text.
    pub fn query(&self, cx: &App) -> String {
        self.input.read(cx).value().to_string()
    }

    fn set_scope(&mut self, scope: Scope, cx: &mut Context<Self>) {
        if self.scope == scope {
            return;
        }
        self.scope = scope;
        // The old results answer a different question now. Clearing rather than
        // leaving them is the honest thing: a list captioned "12 results" that
        // came from a scope the user just changed is a lie with a number on it.
        self.results = Results::default();
        self.answered.clear();
        self.schedule(cx);
    }

    /// Re-run the search, e.g. after the corpus changed underneath it.
    ///
    /// Clears first, for the same reason changing scope does: results from the
    /// folder that was open a moment ago are another project's matches shown
    /// as this one's, and an empty list is the honest state until the new
    /// answer lands.
    pub fn rerun(&mut self, cx: &mut Context<Self>) {
        self.results = Results::default();
        self.answered.clear();
        self.schedule(cx);
    }

    /// Debounce, then ask the workspace for a corpus.
    ///
    /// Two steps rather than one because gathering the corpus is not this
    /// view's to do — see [`SearchEvent::Ready`].
    fn schedule(&mut self, cx: &mut Context<Self>) {
        let query = Query::new(self.query(cx));
        if !query.is_runnable() {
            self.results = Results::default();
            self.answered.clear();
            self.running = false;
            self._search = None;
            cx.notify();
            return;
        }

        self.running = true;
        cx.notify();
        // Replacing the task cancels the previous one, which is the debounce:
        // only the last keystroke in a burst reads any files.
        self._search = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(DEBOUNCE).await;
            crate::views::try_update(&this, cx, |_, cx| cx.emit(SearchEvent::Ready));
        }));
    }

    /// Run `query` against `corpus` off the UI thread and show the result.
    ///
    /// Driven by the workspace rather than by this view: gathering the corpus
    /// needs the open tabs' editor text and the harness view's scan, neither of
    /// which this view can reach.
    pub fn run(&mut self, corpus: Corpus, cx: &mut Context<Self>) {
        let query = Query::new(self.query(cx));
        if !query.is_runnable() {
            self.results = Results::default();
            self.answered.clear();
            self.running = false;
            cx.notify();
            return;
        }
        let text = query.text.clone();
        self.running = true;
        cx.notify();

        self._search = Some(cx.spawn(async move |this, cx| {
            let results = cx
                .background_spawn(async move {
                    let mut out = Results::default();
                    for (path, body) in &corpus.open {
                        search::search_text(path, body, &query, search::DEFAULT_LIMIT, &mut out);
                    }
                    search::search_files(&corpus.files, &query, search::DEFAULT_LIMIT, &mut out);

                    // Walking is done here rather than by the caller: on a real
                    // vault it is seconds, and the caller assembles the corpus
                    // on the UI thread. Skip what is already searched, or every
                    // match in an open document is reported twice.
                    let mut walked: Vec<std::path::PathBuf> = Vec::new();
                    for root in &corpus.roots {
                        // Nothing more to find, so stop before paying for the
                        // walk — the cap is what bounds the worst case and it
                        // has to bound the filesystem work, not just the list.
                        if out.matches.len() >= search::DEFAULT_LIMIT {
                            out.truncated = true;
                            break;
                        }
                        walked.extend(search::document_paths(root));
                    }
                    walked.sort();
                    walked.dedup();
                    walked.retain(|p| {
                        !corpus.files.contains(p) && !corpus.open.iter().any(|(o, _)| o == p)
                    });
                    search::search_files(&walked, &query, search::DEFAULT_LIMIT, &mut out);
                    out
                })
                .await;

            crate::views::try_update(&this, cx, |this, cx| {
                // Discard a stale result: the user may have typed on, and a
                // newer search is already queued.
                if this.query(cx) != text {
                    return;
                }
                this.results = results;
                this.answered = text;
                this.running = false;
                cx.notify();
            });
        }));
    }

    fn render_scopes(&self, cx: &Context<Self>) -> impl IntoElement {
        TabBar::new("search-scopes")
            .underline()
            .w_full()
            .selected_index(
                Scope::ALL
                    .iter()
                    .position(|s| *s == self.scope)
                    .unwrap_or(0),
            )
            .on_click(cx.listener(|this, ix: &usize, _, cx| {
                this.set_scope(Scope::ALL[*ix], cx);
            }))
            .children(Scope::ALL.map(|s| Tab::new().label(i18n::t(s.label(), cx))))
    }

    fn render_results(&self, cx: &Context<Self>) -> AnyElement {
        if self.results.is_empty() {
            let hint = if self.running {
                i18n::t(i18n::Key::Searching, cx).to_string()
            } else if self.answered.is_empty() {
                i18n::t(i18n::Key::TypeToSearch, cx).to_string()
            } else {
                i18n::t(i18n::Key::NoMatches, cx).to_string()
            };
            return div()
                .p(metrics::inset())
                .text_sm()
                .text_color(cx.theme().muted_foreground)
                .child(hint)
                .into_any_element();
        }

        v_flex()
            .id("search-results")
            .size_full()
            .px(px(metrics::INSET - metrics::ROW_PAD))
            .py_1()
            .gap(metrics::row_gap())
            .overflow_y_scroll()
            .children(self.results.matches.iter().enumerate().map(|(ix, m)| {
                let path = m.path.clone();
                let offset = m.offset;
                let name = m
                    .path
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or_default()
                    .to_string();
                ListItem::new(ix)
                    .w_full()
                    .px(metrics::row_pad())
                    .py_1()
                    .rounded(cx.theme().radius)
                    .child(
                        v_flex()
                            .gap_0p5()
                            .child(
                                h_flex()
                                    .gap_2()
                                    .items_center()
                                    .child(Icon::new(IconName::File).small())
                                    .child(div().text_sm().truncate().child(name))
                                    // The line number is what makes two hits in
                                    // one file distinguishable at a glance.
                                    .child(
                                        div()
                                            .text_xs()
                                            .text_color(cx.theme().muted_foreground)
                                            .child(format!(":{}", m.line)),
                                    ),
                            )
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(cx.theme().muted_foreground)
                                    .truncate()
                                    .child(m.line_text.trim().to_string()),
                            ),
                    )
                    .on_click(cx.listener(move |_, _, _, cx| {
                        cx.emit(SearchEvent::Reveal {
                            path: path.clone(),
                            offset,
                        });
                    }))
            }))
            .into_any_element()
    }

    /// The one-line summary above the list.
    fn render_summary(&self, cx: &Context<Self>) -> Option<impl IntoElement> {
        if self.results.is_empty() {
            return None;
        }
        // The truncation notice is not decoration: without it a capped list
        // presents a partial answer as a complete one, which is the failure
        // mode a search must never have.
        let text = if self.results.truncated {
            format!(
                "{}+ in {} file(s) — refine to see the rest",
                self.results.matches.len(),
                self.results.files
            )
        } else {
            format!(
                "{} in {} file(s)",
                self.results.matches.len(),
                self.results.files
            )
        };
        Some(
            div()
                .px(metrics::inset())
                .py_1()
                .text_xs()
                .text_color(cx.theme().muted_foreground)
                .child(text),
        )
    }
}

impl EventEmitter<SearchEvent> for SearchView {}

impl Focusable for SearchView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for SearchView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        v_flex()
            .id("search")
            .role(gpui::Role::Search)
            .aria_label("Search documents")
            .track_focus(&self.focus_handle)
            .size_full()
            .child(
                div()
                    .px(metrics::inset())
                    .py(metrics::header_pad_y())
                    .child(Input::new(&self.input).small()),
            )
            .child(self.render_scopes(cx))
            .children(self.render_summary(cx))
            .child(div().flex_1().min_h_0().child(self.render_results(cx)))
    }
}

#[cfg(test)]
mod tests {
    // Import selectively: the `gpui::*` glob above re-exports a `test`
    // attribute macro that shadows the built-in one and blows the recursion
    // limit.
    use super::Scope;

    #[test]
    fn every_scope_is_reachable_and_named_distinctly() {
        use crate::i18n::text;
        use crate::settings::Language;

        for language in Language::ALL {
            let labels: std::collections::HashSet<&str> = Scope::ALL
                .iter()
                .map(|s| text(s.label(), language))
                .collect();
            assert_eq!(
                labels.len(),
                Scope::ALL.len(),
                "two scopes share a label in {}, so one of them is unpickable",
                language.label()
            );
        }
    }

    /// A search must never present a partial answer as a complete one.
    ///
    /// Source-level: the failure needs a corpus big enough to hit the cap. What
    /// makes it correct is that the summary branches on `truncated` — without
    /// that branch a capped list reads as "500 results" when the real number is
    /// unknown and larger, which is the one thing a search must not do.
    #[test]
    fn a_capped_result_list_says_so() {
        let source = include_str!("search.rs");
        let start = source
            .find("fn render_summary")
            .expect("render_summary must exist");
        let body = &source[start..];
        let end = body.find("\nimpl EventEmitter").unwrap_or(body.len());
        let body = &body[..end];

        assert!(
            body.contains("self.results.truncated"),
            "the summary must distinguish a capped list from a complete one"
        );
        assert!(
            body.contains("refine"),
            "and say what to do about it, not merely that it happened"
        );
    }

    /// A stale result must never overwrite a newer query's.
    ///
    /// Source-level because reproducing it needs two searches racing. Without
    /// the guard, typing `ab` then `abc` can land `ab`'s slower result last and
    /// leave the list disagreeing with the field above it — which reads as the
    /// search being wrong rather than late.
    #[test]
    fn a_slow_search_cannot_overwrite_a_newer_one() {
        let source = include_str!("search.rs");
        let start = source.find("pub fn run").expect("run must exist");
        let body = &source[start..];
        let end = body.find("\n    fn render_scopes").unwrap_or(body.len());
        let body = &body[..end];

        assert!(
            body.contains("if this.query(cx) != text"),
            "the result must be discarded when the query has moved on"
        );
        assert!(
            body.contains("background_spawn"),
            "searching reads files; on the UI thread that is a frozen window"
        );
    }

    /// Typing must not read the corpus once per keystroke.
    #[test]
    fn the_query_is_debounced() {
        let source = include_str!("search.rs");
        let start = source.find("fn schedule").expect("schedule must exist");
        let body = &source[start..];
        let end = body.find("\n    /// Run `query`").unwrap_or(body.len());
        let body = &body[..end];

        assert!(
            body.contains("timer(DEBOUNCE)"),
            "every intermediate prefix of a word would otherwise read every file"
        );
        assert!(
            body.contains("self._search = Some("),
            "the task must be replaced, which is what cancels the previous one"
        );
    }
}
