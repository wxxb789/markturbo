//! The Search panel.
//!
//! Four scopes, one list. What varies between them is only *which documents*
//! are searched — the matching itself lives in [`mt_core::workspace::search`], so a result
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

use gpui_kit::component::{
    ActiveTheme as _, Icon, IconName, Sizable as _, h_flex,
    input::{Input, InputEvent, InputState},
    list::ListItem,
    tab::{Tab, TabBar},
    v_flex,
};
use gpui_kit::*;
use mt_core::workspace::search::{self, Query, Results, SearchTarget};

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
    /// Reveal `target` at `offset`, showing `path` in the result list.
    ///
    /// File targets use a preview open; an open-document target resolves to
    /// the existing in-memory document identified by its ID.
    Reveal {
        path: PathBuf,
        target: SearchTarget,
        offset: usize,
    },
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
    /// Open document snapshots in tab order, because that is also the result
    /// priority when the global cap is reached. Searching their in-memory text
    /// avoids reporting stale disk contents for ordinary open files.
    pub open: Vec<OpenSnapshot>,
    /// Individual documents to read from disk.
    pub files: Vec<PathBuf>,
    /// Directories to walk for documents, off the UI thread.
    pub roots: Vec<PathBuf>,
}

/// An in-memory open-document snapshot.
#[derive(Debug, Clone)]
pub struct OpenSnapshot {
    pub path: PathBuf,
    pub text: String,
    /// `File` uses `path` as its navigation target; recovered tabs carry their
    /// document ID here instead.
    pub target: SearchTarget,
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
                .background_spawn(async move { search_corpus(&corpus, &query) })
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

        let recovered_label = i18n::t(i18n::Key::RecoveredSnapshot, cx);
        v_flex()
            .id("search-results")
            .size_full()
            .px(px(metrics::INSET - metrics::ROW_PAD))
            .py_1()
            .gap(metrics::row_gap())
            .overflow_y_scroll()
            .children(self.results.matches.iter().enumerate().map(|(ix, m)| {
                let path = m.path.clone();
                let target = m.target;
                let offset = m.offset;
                let name = m
                    .path
                    .as_ref()
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
                                    )
                                    .children(
                                        matches!(target, SearchTarget::OpenDocument(_)).then(
                                            || {
                                                div()
                                                    .text_xs()
                                                    .text_color(cx.theme().muted_foreground)
                                                    .child(recovered_label)
                                            },
                                        ),
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
                            path: path.as_ref().clone(),
                            target,
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

fn search_corpus(corpus: &Corpus, query: &Query) -> Results {
    let mut out = Results::default();
    for snapshot in &corpus.open {
        match snapshot.target {
            SearchTarget::File => search::search_text(
                &snapshot.path,
                &snapshot.text,
                query,
                search::DEFAULT_LIMIT,
                &mut out,
            ),
            SearchTarget::OpenDocument(id) => search::search_open_document(
                id,
                &snapshot.path,
                &snapshot.text,
                query,
                search::DEFAULT_LIMIT,
                &mut out,
            ),
        }
    }
    search::search_files(&corpus.files, query, search::DEFAULT_LIMIT, &mut out);

    // Walking is done here rather than by the caller: on a real vault it is
    // seconds, and the caller assembles the corpus on the UI thread. Skip what
    // is already searched, or every match in an open document is reported twice.
    let mut walked: Vec<std::path::PathBuf> = Vec::new();
    for root in &corpus.roots {
        // Nothing more to find, so stop before paying for the walk — the cap is
        // what bounds the filesystem work, not just the result list.
        if out.matches.len() >= search::DEFAULT_LIMIT {
            out.truncated = true;
            break;
        }
        walked.extend(search::document_paths(root));
    }
    walked.sort();
    walked.dedup();
    walked.retain(|path| {
        !corpus.files.contains(path)
            && !corpus.open.iter().any(|snapshot| {
                snapshot.target == SearchTarget::File && snapshot.path.as_path() == path.as_path()
            })
    });
    search::search_files(&walked, query, search::DEFAULT_LIMIT, &mut out);
    out
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
            .role(gpui_kit::Role::Search)
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
    // Import selectively: the `gpui_kit::*` glob above re-exports a `test`
    // attribute macro that shadows the built-in one and blows the recursion
    // limit.
    use super::{Corpus, OpenSnapshot, Query, Scope, SearchTarget, search_corpus};
    use mt_core::document::lifecycle::DocumentId;

    #[test]
    fn a_recovered_snapshot_does_not_hide_a_disk_file_with_the_same_path() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("same-source.md");
        std::fs::write(&path, "needle on disk\n").unwrap();
        let id = DocumentId::next();
        let corpus = Corpus {
            open: vec![OpenSnapshot {
                path: path.clone(),
                text: "needle in recovered buffer\n".to_string(),
                target: SearchTarget::OpenDocument(id),
            }],
            roots: vec![directory.path().to_path_buf()],
            ..Corpus::default()
        };

        let results = search_corpus(&corpus, &Query::new("needle"));

        assert_eq!(results.matches.len(), 2);
        assert!(
            results
                .matches
                .iter()
                .all(|found| found.path.as_path() == path.as_path())
        );
        assert_eq!(results.matches[0].line_text, "needle in recovered buffer");
        assert_eq!(results.matches[0].target, SearchTarget::OpenDocument(id));
        assert_eq!(results.matches[1].line_text, "needle on disk");
        assert_eq!(results.matches[1].target, SearchTarget::File);
        assert_eq!(results.files, 2);
    }

    #[test]
    fn open_snapshot_tab_order_controls_results_at_the_global_cap() {
        let recovered_id = DocumentId::next();
        let corpus = Corpus {
            open: vec![
                OpenSnapshot {
                    path: "recovered.md".into(),
                    text: "needle\n".repeat(mt_core::workspace::search::DEFAULT_LIMIT - 1),
                    target: SearchTarget::OpenDocument(recovered_id),
                },
                OpenSnapshot {
                    path: "ordinary.md".into(),
                    text: "needle\n".into(),
                    target: SearchTarget::File,
                },
            ],
            ..Corpus::default()
        };

        let results = search_corpus(&corpus, &Query::new("needle"));

        assert_eq!(
            results.matches.len(),
            mt_core::workspace::search::DEFAULT_LIMIT
        );
        assert_eq!(
            results.matches[mt_core::workspace::search::DEFAULT_LIMIT - 2].target,
            SearchTarget::OpenDocument(recovered_id)
        );
        assert_eq!(
            results.matches[mt_core::workspace::search::DEFAULT_LIMIT - 1].target,
            SearchTarget::File
        );
        assert!(results.truncated);
        assert_eq!(results.files, 2);
    }

    #[test]
    fn identical_result_rows_keep_distinct_navigation_targets() {
        let path = std::path::PathBuf::from("same-source.md");
        let id = DocumentId::next();
        let corpus = Corpus {
            open: vec![
                OpenSnapshot {
                    path: path.clone(),
                    text: "same needle line\n".into(),
                    target: SearchTarget::File,
                },
                OpenSnapshot {
                    path: path.clone(),
                    text: "same needle line\n".into(),
                    target: SearchTarget::OpenDocument(id),
                },
            ],
            ..Corpus::default()
        };

        let results = search_corpus(&corpus, &Query::new("needle"));

        assert_eq!(results.matches.len(), 2);
        assert_eq!(results.matches[0].path.as_path(), path.as_path());
        assert_eq!(results.matches[1].path.as_path(), path.as_path());
        assert_eq!(results.matches[0].line, results.matches[1].line);
        assert_eq!(results.matches[0].line_text, results.matches[1].line_text);
        assert_eq!(results.matches[0].target, SearchTarget::File);
        assert_eq!(results.matches[1].target, SearchTarget::OpenDocument(id));
    }

    #[test]
    fn every_scope_is_reachable_and_named_distinctly() {
        use crate::i18n::text;
        use mt_core::settings::Language;

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
}
