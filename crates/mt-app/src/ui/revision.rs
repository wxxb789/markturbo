//! Owner lane: revision.
//!
//! The field during an approved revision: the proposed hunks, one of them
//! docked in the reading position with its old text struck cold and its new
//! text a hot proposal, the others as single ember lines, and the decision
//! row — `Accept` / `Reject` on the current hunk (`A` / `R`), one pip per hunk
//! reading `n of m decided`, `Undo` (`Ctrl+Z`) and `Done`.
//!
//! A decision writes its text at once. `Accept` puts the replacement in the
//! column in the same frame the proposal extinguishes (the struck words are
//! gone with it); `Reject` writes nothing. `Undo` takes the last decision back
//! together with its text — the reviewed bytes of that hunk, exactly — and
//! repeated `Undo` walks further back. The state's ledger
//! ([`AppState::set_revision_change_applied`]) does every write, keeps later
//! hunks on their own bytes after an earlier one changed length, and follows
//! the user's own edits between decisions: a hunk whose part the user edited
//! says `Can't apply — this part changed` and offers no `Accept`, and an
//! `Undo` whose accepted text was edited since is refused with a message on
//! the status line (the decision stays), never written over the edit.
//! Nothing here saves; the tab only turns dirty.
//!
//! `Ctrl+Z` belongs to whoever has the keyboard: while the source editor (or
//! any text input) has the focus, the editor's own undo wins and takes back
//! its last edit — an accepted replacement included, which the ledger then
//! reads as the user's edit of that part. Everywhere else — the field, its
//! buttons, nothing focused — `Ctrl+Z` is this field's `Undo`. The `Undo`
//! button is always this field's.
//!
//! Contract: [`is_open`] says the field belongs to the revision (it outranks
//! the review field, because a revision runs inside a review); [`field`] draws
//! it. The other half — the removed words struck cold in the column — is
//! [`column_decorations`], which `ui::document` merges into the editor's
//! decorations; it reads [`decision`], because the state's per-change signal
//! alone cannot tell a rejected change from one still to be judged. Accepted
//! words need no mark: once written they are plain ink.
//!
//! `Copy` puts the approved result — the document as it stands, every
//! accepted change in it and nothing else of the proposal — on the clipboard,
//! without touching the editor, the dirty flag or the file.
//!
//! Old and new are always on screen together — no modifier key, no hover — so
//! the hunk is judged without leaving the field. The current proposal is this
//! state's one HOT item ([`Heat`] pinned at 1.0 rather than cooling, then
//! extinguished when it is decided, which leaves it docked and plain until
//! the next undecided hunk takes its place); the other hunks are ember lines.
//! Both sides of the docked hunk are complete: a tall one scrolls in place,
//! so `Accept` never approves text that was hidden. A hunk whose part of the
//! document was edited says `Can't apply — this part changed`; a proposal
//! whose document is no longer the one on screen says `This Revision belongs
//! to another document` with `Review again`. Neither offers an `Accept`, and
//! the field always offers a way out (`Close`, and `Review again` on another
//! document).
//!
//! The decision log and the reading position live here, in a module-local
//! session keyed by the proposal ([`Session`]); a decision is recorded only
//! once the state has written (or refused) its text, so a change's state
//! signal is `true` exactly when the session's last decision on it is an
//! accept. `Done` ([`AppState::finish_revision`]) writes nothing more: the
//! accepted changes are already in, and the undecided ones are discarded —
//! never written — with `n not applied`. Nothing in this file mutates the
//! document except through the state.

use std::{
    collections::hash_map::DefaultHasher,
    hash::{Hash, Hasher},
    time::Duration,
};

use mt_core::settings::Language;
use nsh_base::editor::EditorDecoration;
use nsh_core::{Color, PropertyValue, StyleKey, TextDecoration, element::Style, prelude::*};

use crate::{
    i18n::{self, Key},
    state::{AppState, RevisionHunkView, Tab},
    ui::{
        glow,
        motion::{self, Heat},
        prims, titlebar,
        tokens::{self, SPACE_1, SPACE_2, SPACE_3},
    },
};

/// The tallest the docked hunk's own text gets before it scrolls: about
/// fourteen source lines. The rest is reached by scrolling, never hidden
/// from an `Accept`.
const HUNK_VIEW_MAX_H: f32 = 360.0;
/// The longest one-line digest of a rationale, in characters.
const DIGEST_CHARS: usize = 72;

// ─── copy ─────────────────────────────────────────────────────────────────

/// The revision field's own copy, in English and Simplified Chinese. A
/// missing translation does not compile: both tables are exhaustive matches.
#[derive(Clone, Copy, PartialEq, Eq)]
enum S {
    Accept,
    Reject,
    Accepted,
    Rejected,
    Undo,
    Done,
    Decided(usize, usize),
    NotApplied(usize),
    CannotApply,
    OtherDocument,
    ReviewAgain,
    Move,
    Copy,
    Proposed,
}

impl S {
    fn text(self, language: Language) -> String {
        match language {
            Language::English => match self {
                S::Accept => "Accept".to_owned(),
                S::Reject => "Reject".to_owned(),
                S::Accepted => "Accepted".to_owned(),
                S::Rejected => "Rejected".to_owned(),
                S::Undo => "Undo".to_owned(),
                S::Done => "Done".to_owned(),
                S::Decided(n, m) => format!("{n} of {m} decided"),
                S::NotApplied(n) => format!("{n} not applied"),
                S::CannotApply => "Can't apply — this part changed".to_owned(),
                S::OtherDocument => "This Revision belongs to another document".to_owned(),
                S::ReviewAgain => "Review again".to_owned(),
                S::Move => "Move".to_owned(),
                S::Copy => "Copy".to_owned(),
                S::Proposed => "Proposed text".to_owned(),
            },
            Language::Chinese => match self {
                S::Accept => "接受".to_owned(),
                S::Reject => "拒绝".to_owned(),
                S::Accepted => "已接受".to_owned(),
                S::Rejected => "已拒绝".to_owned(),
                S::Undo => "撤销".to_owned(),
                S::Done => "完成".to_owned(),
                S::Decided(n, m) => format!("已决定 {n} / {m}"),
                S::NotApplied(n) => format!("{n} 项未应用"),
                S::CannotApply => "无法应用——这部分已改动".to_owned(),
                S::OtherDocument => "此修订属于另一个文档".to_owned(),
                S::ReviewAgain => "重新审阅".to_owned(),
                S::Move => "切换".to_owned(),
                S::Copy => "复制".to_owned(),
                S::Proposed => "建议的文本".to_owned(),
            },
        }
    }
}

// ─── the field's session ──────────────────────────────────────────────────

/// One decision, oldest first; the last record of a change id is what it is
/// decided as, and `current_before` restores the reading position on `Undo`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Decision {
    change_id: u32,
    accepted: bool,
    current_before: usize,
}

/// What the field remembers between rebuilds: which proposal the decisions
/// belong to and which hunk holds the reading position. A proposal is its
/// hunks' content, so a new or re-rolled one never inherits a decision.
#[derive(Clone, Debug, Default, PartialEq)]
struct Session {
    proposal: u64,
    current: usize,
    decisions: Vec<Decision>,
}

impl Session {
    /// This session, made for `hunks`: a reset one when they are not the
    /// proposal it belongs to (a new run, whose outcome is the user's to
    /// see).
    fn synced(&self, hunks: &[RevisionHunkView]) -> Session {
        if self.proposal == proposal_id(hunks) {
            self.clone()
        } else {
            Session {
                proposal: proposal_id(hunks),
                ..Session::default()
            }
        }
    }

    /// The hunk in the reading position, clamped to the proposal.
    fn current(&self, hunks: &[RevisionHunkView]) -> Option<usize> {
        (!hunks.is_empty()).then(|| self.current.min(hunks.len() - 1))
    }

    /// What the change is decided as, if it is.
    fn decision(&self, change_id: u32) -> Option<bool> {
        self.decisions
            .iter()
            .rev()
            .find(|decision| decision.change_id == change_id)
            .map(|decision| decision.accepted)
    }

    /// Whether the hunk at `index` has a decided change.
    fn decided(&self, hunks: &[RevisionHunkView], index: usize) -> bool {
        hunks
            .get(index)
            .is_some_and(|hunk| self.decision(hunk.change_id).is_some())
    }

    /// The index of the hunk in the reading position: the field docks the
    /// current hunk whether or not it is decided, so nothing is ever decided
    /// without being on screen.
    fn docked(&self, hunks: &[RevisionHunkView]) -> Option<usize> {
        self.current(hunks)
    }

    /// Whether the docked hunk is one whose change is still to be judged —
    /// the only thing that stays HOT.
    fn undecided(&self, hunks: &[RevisionHunkView]) -> bool {
        self.docked(hunks)
            .is_some_and(|index| !self.decided(hunks, index))
    }

    /// Record a decision about the hunk at `index`, keeping the reading
    /// position on it (the docked proposal extinguishes first; [`advance`]
    /// then brings the next undecided hunk forward).
    ///
    /// [`advance`]: Session::advance
    fn record(&mut self, hunks: &[RevisionHunkView], index: usize, accepted: bool) {
        let Some(hunk) = hunks.get(index) else {
            return;
        };
        self.decisions.push(Decision {
            change_id: hunk.change_id,
            accepted,
            current_before: self.current,
        });
        self.current = index;
    }

    /// Move the reading position to the next undecided hunk after `from`
    /// (wrapping, so hunks left undecided above are still reached), else keep
    /// it on `from`.
    fn advance(&mut self, hunks: &[RevisionHunkView], from: usize) {
        let next = (from + 1..hunks.len())
            .chain(0..from.min(hunks.len()))
            .find(|&index| !self.decided(hunks, index));
        match next {
            Some(next) => self.current = next,
            None if !hunks.is_empty() => self.current = from.min(hunks.len() - 1),
            None => {}
        }
    }

    /// Take the last decision back: the change returns to what it was decided
    /// as before it, and the reading position to where it was.
    fn undo(&mut self) -> Option<u32> {
        let decision = self.decisions.pop()?;
        self.current = decision.current_before;
        Some(decision.change_id)
    }

    /// The decided hunk count and the hunk count.
    fn counts(&self, hunks: &[RevisionHunkView]) -> (usize, usize) {
        let decided = hunks
            .iter()
            .filter(|hunk| self.decision(hunk.change_id).is_some())
            .count();
        (decided, hunks.len())
    }
}

/// The session signal: one per surface, made on first use, like the shell's
/// width.
fn session() -> Signal<Session> {
    global::<Session>()
}

/// The identity of a proposal: its hunks' content.
fn proposal_id(hunks: &[RevisionHunkView]) -> u64 {
    let mut hasher = DefaultHasher::new();
    hunks.len().hash(&mut hasher);
    for hunk in hunks {
        hunk.ordinal.hash(&mut hasher);
        hunk.change_id.hash(&mut hasher);
        hunk.before.hash(&mut hasher);
        hunk.after.hash(&mut hasher);
        hunk.rationale.hash(&mut hasher);
    }
    hasher.finish()
}

/// The decision on `change_id`, for a surface that draws the revision's other
/// half — the column's cold strikes: `Some(true)` accepted, `Some(false)`
/// rejected (the marks go), `None` still to be judged (the marks stand).
/// Reading it subscribes; a proposal it does not belong to is undecided.
///
/// The state's own per-change signal says only "not accepted", which a
/// rejected and an undecided change share, so the distinction lives here.
pub fn decision(state: &AppState, change_id: u32) -> Option<bool> {
    let hunks = state.revision_hunks.get();
    session().with(|now| now.synced(&hunks).decision(change_id))
}

/// The column's marks for `tab`: every hunk still to be judged has its
/// removed words struck cold — a 1px strike and underline in the palette's
/// removed colour, the glyphs keeping their ink. An accepted hunk is plain
/// text already and a rejected one loses its marks; a hunk an edit touched
/// has no place to mark. The places are the state's ledger's, never a
/// search for the old text. Reads signals only, so the editor's binding
/// follows every decision and edit.
pub fn column_decorations(state: &AppState, tab: &Tab) -> Vec<EditorDecoration> {
    let hunks = state.revision_hunks.get();
    if hunks.is_empty() {
        return Vec::new();
    }
    // Reading the session subscribes, as [`decision`] does.
    let decided = session().with(|now| now.synced(&hunks));
    let removed = removed_style();
    tab.text.with(|text| {
        hunks
            .iter()
            .filter(|hunk| !hunk.accepted.get() && decided.decision(hunk.change_id).is_none())
            .filter_map(|hunk| state.revision_hunk_range(tab.id, hunk.ordinal))
            .filter_map(|range| glow::positions(text, range.start, range.end))
            .map(|range| {
                // `EditorDecoration` is `#[non_exhaustive]`: a default and
                // its fields, not a literal.
                let mut decoration = EditorDecoration::default();
                decoration.range = range;
                decoration.style = Some(removed.clone());
                decoration
            })
            .collect()
    })
}

/// The cold strike on removed words, as declarations the editor replays.
fn removed_style() -> Style {
    let lines = TextDecoration {
        underline: true,
        line_through: true,
    };
    Style::default()
        .with(
            StyleKey::TextDecorationLine,
            PropertyValue::TextDecoration(Some(lines)),
        )
        .with(
            StyleKey::TextDecorationColor,
            PropertyValue::Color(Some(tokens::palette().removed)),
        )
        .with(
            StyleKey::TextDecorationThickness,
            PropertyValue::OptPoints(Some(px(tokens::HAIRLINE))),
        )
}

// ─── the surface ──────────────────────────────────────────────────────────

/// Whether the field is showing a revision.
///
/// An outcome with no hunks on screen — a failure before any proposal, or the
/// confirmation after `Done` — holds the field until it is dismissed, and
/// [`dismiss`] consumes it: the next outcome opens the field again, whether it
/// reads the same as the dismissed one or not. Nothing here remembers a
/// dismissal, so an outcome the state writes again (a second `Prepare
/// revision` failing the same way does not pass through nothing) still shows.
pub fn is_open(state: &AppState) -> bool {
    state.revision_busy.get()
        || state.revision_hunks.with(|hunks| !hunks.is_empty())
        || state.revision_status.get().is_some()
}

/// The revision field.
pub fn field(state: AppState, language: Language) -> impl IntoElement {
    let hunks = state.revision_hunks;
    let status = state.revision_status;
    let busy = state.revision_busy;
    let now = session();
    let heat = Heat::cold();

    // The session is the proposal on screen's; a mount that carries another
    // proposal's hunks (a second revision) starts it over before anything
    // reads it.
    sync_session(&hunks.get_untracked(), now);
    bind_keys(&state, heat);
    watch_proposal(hunks, now);
    watch_heat(hunks, now, heat);

    let stale_state = state.clone();
    let stale_action = state.clone();
    let done_key_state = state.clone();
    let done_key_gate = state.clone();
    let outcome_state = state.clone();
    let rows_state = state.clone();
    let decisions_state = state.clone();

    view()
        .test_id("revision-field")
        .w_full()
        .h(move || px(field_height()))
        .flex_col()
        .gap(px(SPACE_3))
        // The proposal belongs to a tab not on screen: nothing here can be
        // decided into this one.
        .child(when(
            move || {
                !stale_state.revision_hunks.with(|hunks| hunks.is_empty())
                    && stale_state.revision_is_stale()
            },
            move || {
                let state = stale_action.clone();
                view()
                    .test_id("revision-stale")
                    .flex_row()
                    .items_center()
                    .gap(px(SPACE_3))
                    .child(prims::ink(S::OtherDocument.text(language)))
                    .child(prims::primary_action(
                        "revision-review-again",
                        S::ReviewAgain.text(language),
                        move || state.start_review(),
                    ))
            },
        ))
        // The state's own line: ready, failed, applied, refused.
        .child(when(
            move || status.get().is_some(),
            move || outcome(outcome_state.clone(), language),
        ))
        // While the request runs there is nothing to show but that it runs.
        .child(when(
            move || busy.get(),
            move || prims::rest(i18n::t(language, Key::Revising)),
        ))
        // The hunks, the current one docked in the reading position: the
        // whole proposal scrolls, so the rows can always reach the decision
        // row under them.
        .child(when(
            move || hunks.with(|hunks| !hunks.is_empty()),
            move || rows(rows_state.clone(), language, hunks, now, heat),
        ))
        // Pips, the count, Undo and the actions — pinned under the scroll.
        .child(when(
            move || hunks.with(|hunks| !hunks.is_empty()),
            move || decisions(decisions_state.clone(), language, hunks, now),
        ))
        .child(when(
            move || hunks.with(|hunks| !hunks.is_empty()),
            move || key_hints(language),
        ))
        // `Done`'s key (`Ctrl+Enter`, the app's one binding of it), bound
        // again while there is something it could apply, so it is newer than
        // any binding made before the proposal came; in every other state the
        // keystroke is `Review`'s, and a stale proposal has nothing to apply.
        // The binding lives and dies with this region, which is out of the
        // flex flow.
        .child(when(
            move || done_applies(&done_key_gate),
            move || {
                let state = done_key_state.clone();
                effect(move || titlebar::bind_mod_enter(&state));
                view().absolute().pointer_events(PointerEvents::None)
            },
        ))
}

/// The field's height in points: the window under the title row, so the
/// docked reading position can sit at its vertical centre.
fn field_height() -> f32 {
    (window().height.0 - tokens::TITLE_ROW_H).max(0.0)
}

/// The state's outcome line — ready, failed, applied, refused — and, when
/// there are no hunks to look at, the way out of it.
fn outcome(state: AppState, language: Language) -> impl IntoElement {
    let status_state = state.clone();
    let condition_state = state.clone();
    let close_state = state.clone();
    let discarded = state.revision_not_applied;
    view()
        .flex_row()
        .items_center()
        // The outcome, what `Done` discarded and the way out wrap in a
        // narrow field rather than run past its edge.
        .flex_wrap()
        .gap(px(SPACE_3))
        .child(prims::rest(move || {
            status_state.revision_status.get().unwrap_or_default()
        }))
        // What `Done` discarded undecided: never written.
        .child(when(
            move || discarded.get() > 0,
            move || prims::faint(move || S::NotApplied(discarded.get()).text(language)),
        ))
        .child(when(
            move || {
                condition_state
                    .revision_hunks
                    .with(|hunks| hunks.is_empty())
            },
            move || {
                let state = close_state.clone();
                prims::quiet_action(
                    "revision-dismiss",
                    i18n::t(language, Key::Close),
                    move || dismiss(&state),
                )
            },
        ))
}

// ─── the hunks ────────────────────────────────────────────────────────────

/// The hunk rows, in a scroll view that centres them when they fit and keeps
/// the docked one in view when they do not: the docked proposal in the
/// reading position, one line per other hunk above and below it.
fn rows(
    state: AppState,
    language: Language,
    hunks: Signal<Vec<RevisionHunkView>>,
    now: Signal<Session>,
    heat: Heat,
) -> impl IntoElement {
    let list_state = state.clone();
    // The rows sit at the reading position's centre while they fit, and
    // scroll (each row revealed when it is docked) when they do not. The
    // viewport is the scroll view's own.
    let reference = element_ref();
    let viewport: Signal<f32> = signal(0.0);
    let watched = reference;
    watch_scroll(
        move || watched.node(),
        move |event: &ScrollEvent| {
            let height = event.size.height.0;
            if viewport.get_untracked() != height {
                viewport.set(height);
            }
        },
    );
    scroll()
        .test_id("revision-rows")
        .flex_1()
        .min_h(px(0.0))
        .w_full()
        .element_ref(&reference)
        .child(
            view()
                .w_full()
                .min_h(move || px(viewport.get()))
                .flex_col()
                .justify_center()
                .child(view().w_full().flex_col().gap(px(SPACE_2)).child(each(
                    move || hunks.get(),
                    |hunk: &RevisionHunkView| hunk.ordinal,
                    move |hunk: ReadSignal<RevisionHunkView>, index: ReadSignal<usize>| {
                        let docked = move || {
                            hunks.with(|hunks| now.with(|now| now.docked(hunks)))
                                == Some(index.get())
                        };
                        let docked_state = list_state.clone();
                        let line_state = list_state.clone();
                        // The item arrives as a signal that follows its row: a row's
                        // key can outlive one proposal (revision numbering restarts),
                        // so every piece of content is bound, never copied out here.
                        let row = view().w_full().child(
                            when(docked, move || {
                                docked_proposal(docked_state.clone(), language, hunk, heat, now)
                            })
                            .otherwise(move || {
                                one_line(line_state.clone(), language, hunk, now, index)
                            }),
                        );
                        // The reading position is kept in view: the docked row is
                        // asked of the scroll view around it whenever it changes.
                        let row_ref = element_ref();
                        let reveal = move || {
                            hunks.with(|hunks| now.with(|now| now.docked(hunks)))
                                == Some(index.get())
                        };
                        effect(move || {
                            if reveal() {
                                row_ref.reveal(
                                    RevealOptions::default().vertical(ScrollAlignment::Center),
                                );
                            }
                        });
                        row.element_ref(&row_ref)
                    },
                ))),
        )
}

/// The hunk in the reading position: the model's reason, the old text struck
/// cold, the new text (hot while the hunk is undecided, plain ink once it is
/// decided), and the decision on it. The text scrolls; the decision never
/// does.
fn docked_proposal(
    state: AppState,
    language: Language,
    hunk: ReadSignal<RevisionHunkView>,
    heat: Heat,
    now: Signal<Session>,
) -> impl IntoElement {
    let reason = move || hunk.with(|hunk| hunk.rationale.clone());
    let message_condition = {
        let state = state.clone();
        move || hunk.with(|hunk| cannot_apply(&state, hunk))
    };
    let accept_condition = {
        let state = state.clone();
        move || hunk.with(|hunk| state.revision_is_stale() || cannot_apply(&state, hunk))
    };
    let decided_condition =
        move || hunk.with(|hunk| now.with(|now| now.decision(hunk.change_id).is_some()));
    let decision_word = move || match hunk.with(|hunk| now.with(|now| now.decision(hunk.change_id)))
    {
        Some(true) => S::Accepted.text(language),
        _ => S::Rejected.text(language),
    };
    let has_before = move || hunk.with(|hunk| !hunk.before.is_empty());
    let has_after = move || hunk.with(|hunk| !hunk.after.is_empty());
    let before_lines = move || {
        hunk.with(|hunk| {
            hunk_lines(&hunk.before)
                .into_iter()
                .enumerate()
                .collect::<Vec<(usize, SharedString)>>()
        })
    };
    let after_lines = move || {
        hunk.with(|hunk| {
            hunk_lines(&hunk.after)
                .into_iter()
                .enumerate()
                .collect::<Vec<(usize, SharedString)>>()
        })
    };
    let accept_state = state.clone();
    let reject_state = state;
    // The new text's colour follows the one heat: hot while the hunk waits to
    // be judged, and settling through ember to plain ink once it is. Every
    // line owns its binding (a row's element outlives this call, so it cannot
    // borrow one).
    view()
        .test_id("revision-proposal")
        .relative()
        .w_full()
        .flex_col()
        .gap(px(SPACE_3))
        .child(halo_off_the_pointer(heat))
        .child(
            // Both sides are complete; a tall hunk scrolls here rather than
            // hiding text from the Accept under it.
            scroll()
                .test_id("revision-proposal-text")
                .max_h(px(HUNK_VIEW_MAX_H))
                .w_full()
                .child(
                    view()
                        .w_full()
                        .flex_col()
                        .gap(px(SPACE_3))
                        .child(when(decided_condition, move || {
                            view()
                                .test_id("revision-proposal-decision")
                                .child(prims::rest(decision_word))
                        }))
                        .child(prims::rest(reason))
                        .child(when(has_before, move || {
                            view()
                                .test_id("revision-proposal-before")
                                .w_full()
                                .flex_col()
                                .child(each(
                                    before_lines,
                                    |(index, _): &(usize, SharedString)| *index,
                                    move |line: ReadSignal<(usize, SharedString)>, _| {
                                        text(move || line.with(|(_, line)| line.clone()))
                                            .font_family(tokens::FONT_SOURCE)
                                            .text_size(px(tokens::TYPE_SOURCE))
                                            .line_height(px(tokens::TYPE_SOURCE_LINE))
                                            .text_color(move || tokens::palette().removed)
                                            .line_through()
                                            .underline()
                                            .text_decoration_color(move || {
                                                tokens::palette().removed
                                            })
                                            .text_decoration_thickness(px(tokens::HAIRLINE))
                                    },
                                ))
                        }))
                        .child(when(has_after, move || {
                            // A labelled group whose lines are its text
                            // children: what a screen reader reads as the
                            // proposal.
                            view()
                                .test_id("revision-proposal-after")
                                .label(S::Proposed.text(language))
                                .w_full()
                                .flex_col()
                                .child(
                                    view()
                                        .w_full()
                                        .flex_col()
                                        .bg(move || tokens::palette().added_plate)
                                        .child(each(
                                            after_lines,
                                            |(index, _): &(usize, SharedString)| *index,
                                            move |line: ReadSignal<(usize, SharedString)>, _| {
                                                let added = heat.glyph(
                                                    tokens::palette().ink,
                                                    tokens::palette().ember,
                                                    tokens::palette().added,
                                                );
                                                text(move || line.with(|(_, line)| line.clone()))
                                                    .font_family(tokens::FONT_SOURCE)
                                                    .text_size(px(tokens::TYPE_SOURCE))
                                                    .line_height(px(tokens::TYPE_SOURCE_LINE))
                                                    // The palette's added colour,
                                                    // with the settling heat bound
                                                    // over it.
                                                    .text_color(move || tokens::palette().added)
                                                    .bind_color(&added)
                                            },
                                        )),
                                )
                                .child(prims::bloom(heat, 6.0))
                        })),
                ),
        )
        .child(when(message_condition, move || {
            view()
                .test_id("revision-proposal-blocked")
                .child(prims::ink(S::CannotApply.text(language)))
        }))
        .child(
            view()
                .flex_row()
                .items_center()
                .gap(px(SPACE_3))
                .child(when_not(accept_condition, move || {
                    prims::primary_action("revision-accept", S::Accept.text(language), {
                        let state = accept_state.clone();
                        move || decide(&state, heat, true)
                    })
                }))
                .child(prims::quiet_action(
                    "revision-reject",
                    S::Reject.text(language),
                    {
                        let state = reject_state.clone();
                        move || decide(&state, heat, false)
                    },
                )),
        )
}

/// The proposal's halo, off the pointer: it bleeds past this box, and a press
/// in the band it covers belongs to the line under it — the row above the
/// docked hunk stays clickable. The plate carries the glow and takes no
/// presses itself.
fn halo_off_the_pointer(heat: Heat) -> impl IntoElement {
    view()
        .absolute()
        .inset(px(0.0))
        .pointer_events(PointerEvents::None)
        .child(prims::halo(heat))
}

/// The negation of `when`, for a region that must be absent while a condition
/// holds.
fn when_not<E: IntoElements>(
    condition: impl IntoReactive<bool>,
    then: impl Fn() -> E + 'static,
) -> impl IntoElement {
    let condition = condition.into_reactive();
    when(move || !condition.get(), then)
}

/// One hunk that is not in the reading position: its ordinal in the pip row's
/// own order, a single line of the model's reason, and what it is decided as —
/// ember while it still asks to be judged, plain ink once accepted, faint once
/// rejected. Pressing it moves the reading position to its hunk — the mouse's
/// `J` / `K`.
fn one_line(
    state: AppState,
    language: Language,
    hunk: ReadSignal<RevisionHunkView>,
    now: Signal<Session>,
    index: ReadSignal<usize>,
) -> impl IntoElement {
    let word = move || hunk.with(|hunk| format!("{}", hunk.ordinal + 1));
    let label =
        move || hunk.with(|hunk| SharedString::from(format!("Revision hunk {}", hunk.ordinal + 1)));
    let digest_text = move || {
        let decided = hunk.with(|hunk| now.with(|now| now.decision(hunk.change_id)));
        let digest_line = hunk.with(|hunk| digest(&hunk.rationale, DIGEST_CHARS));
        match decided {
            None => digest_line,
            Some(true) => format!("{digest_line} · {}", S::Accepted.text(language)),
            Some(false) => format!("{digest_line} · {}", S::Rejected.text(language)),
        }
    };
    let colour = move || match hunk.with(|hunk| now.with(|now| now.decision(hunk.change_id))) {
        None => tokens::palette().ember,
        Some(true) => tokens::palette().ink_rest,
        Some(false) => tokens::palette().ink_caption,
    };
    view()
        .label(label)
        .focusable(true)
        .cursor_pointer()
        .on_press(move || move_to(&state, index.get()))
        .w_full()
        .flex_row()
        .items_center()
        .gap(px(SPACE_3))
        .child(
            view().w(px(24.0)).child(
                text(word)
                    .font_family(tokens::FONT_UI)
                    .text_size(px(tokens::TYPE_UI))
                    .line_height(px(tokens::TYPE_UI_LINE))
                    .text_color(move || tokens::palette().ink_caption),
            ),
        )
        .child(
            text(digest_text)
                .font_family(tokens::FONT_UI)
                .text_size(px(tokens::TYPE_UI))
                .line_height(px(tokens::TYPE_UI_LINE))
                .text_color(colour),
        )
}

// ─── the decision row ─────────────────────────────────────────────────────

/// The footer, pinned under the hunks' scroll view so it is reachable
/// however many hunks the proposal holds: pips (one per hunk, lit when its
/// change is decided) reading the count with `Undo` and the not-applied
/// hint, then the actions — `Close` always, and `Done` (whose key is
/// `Ctrl+Enter`) unless the proposal is stale.
fn decisions(
    state: AppState,
    language: Language,
    hunks: Signal<Vec<RevisionHunkView>>,
    now: Signal<Session>,
) -> impl IntoElement {
    let undo_state = state.clone();
    let copy_state = state.clone();
    let close_state = state.clone();
    let done_state = state;
    view()
        .test_id("revision-decisions")
        .flex_col()
        .w_full()
        .gap(px(SPACE_2))
        .child(
            view()
                .flex_row()
                .items_center()
                .flex_wrap()
                .w_full()
                .gap(px(SPACE_3))
                .child(
                    view()
                        .test_id("revision-pips")
                        .flex_row()
                        .items_center()
                        .gap(px(SPACE_1))
                        .child(each(
                            move || hunks.get(),
                            |hunk: &RevisionHunkView| hunk.ordinal,
                            move |hunk: ReadSignal<RevisionHunkView>, _| {
                                view().w(px(4.0)).h(px(4.0)).bg(move || {
                                    let decided = hunk.with(|hunk| {
                                        now.with(|now| now.decision(hunk.change_id).is_some())
                                    });
                                    if decided {
                                        tokens::palette().ink
                                    } else {
                                        tokens::palette().ink_faint
                                    }
                                })
                            },
                        )),
                )
                .child(prims::rest(move || {
                    let (decided, all) = counts(&hunks, &now);
                    S::Decided(decided, all).text(language)
                }))
                .child(when(
                    move || now.with(|now| !now.decisions.is_empty()),
                    move || {
                        let state = undo_state.clone();
                        prims::quiet_action("revision-undo", S::Undo.text(language), move || {
                            undo_decision(&state)
                        })
                    },
                ))
                .child(when(
                    move || {
                        let (decided, all) = counts(&hunks, &now);
                        decided < all
                    },
                    move || {
                        prims::faint(move || {
                            let (decided, all) = counts(&hunks, &now);
                            S::NotApplied(all - decided).text(language)
                        })
                    },
                )),
        )
        .child(
            view()
                .flex_row()
                .items_center()
                .justify_end()
                .w_full()
                .gap(px(SPACE_3))
                .child(copy_action(copy_state, language))
                .child(prims::quiet_action(
                    "revision-close",
                    i18n::t(language, Key::Close),
                    move || close_state.close_review(),
                ))
                // `Done` is the one next action only once nothing is left to
                // judge; while hunks wait it stays the quiet one.
                .child(done_action(done_state, language, hunks, now)),
        )
}

/// `Copy`, a quiet action whose pressable view carries the test id itself,
/// so a screen reader (and UI Automation) finds one button by it.
fn copy_action(state: AppState, language: Language) -> impl IntoElement {
    let label = SharedString::from(S::Copy.text(language));
    view()
        .test_id("revision-copy")
        .role(AccessibilityRole::Button)
        .label(label.clone())
        .focusable(true)
        .cursor_pointer()
        .bg(Color::TRANSPARENT)
        .px(px(2.0))
        .py(px(4.0))
        .transition_colors()
        .duration(motion::HOVER_MS)
        .hover(|s| s.text_color(tokens::palette().ink).underline())
        .focus_visible(|s| s.text_color(tokens::palette().ink).underline())
        .on_press(move || copy_approved(&state))
        .child(
            text(label)
                .font_family(tokens::FONT_UI)
                .text_size(px(tokens::TYPE_UI))
                .line_height(px(tokens::TYPE_UI_LINE))
                .text_color(move || tokens::palette().ink_rest),
        )
}

/// `Done`: absent while the proposal is stale (nothing here can apply), the
/// quiet action while hunks wait to be judged, and the field's one next
/// action once every hunk is decided.
fn done_action(
    state: AppState,
    language: Language,
    hunks: Signal<Vec<RevisionHunkView>>,
    now: Signal<Session>,
) -> impl IntoElement {
    let fresh = state.clone();
    let all_decided = move || {
        let (decided, all) = counts(&hunks, &now);
        decided == all
    };
    let quiet_state = state.clone();
    let primary_state = state.clone();
    // The condition is a closure: staleness follows the tab's revision, so an
    // edit while the field is up must take `Done` away with it.
    when(move || fresh.revision_is_stale(), move || view().h(px(0.0))).otherwise(move || {
        // Fresh handles for this call: a region asks for its branch again
        // whenever a condition flips, so the copies it captures stay with it.
        let all_decided = all_decided;
        let primary_state = primary_state.clone();
        let quiet_state = quiet_state.clone();
        view()
            .flex_row()
            .items_center()
            .gap(px(SPACE_2))
            .child(
                when(all_decided, move || {
                    let state = primary_state.clone();
                    prims::primary_action("revision-done", S::Done.text(language), move || {
                        done(&state)
                    })
                })
                .otherwise(move || {
                    let state = quiet_state.clone();
                    prims::quiet_action("revision-done", S::Done.text(language), move || {
                        done(&state)
                    })
                }),
            )
            .child(prims::kbd("Ctrl+Enter"))
    })
}

/// The decided hunk count and the total, without cloning the proposal.
fn counts(hunks: &Signal<Vec<RevisionHunkView>>, now: &Signal<Session>) -> (usize, usize) {
    hunks.with(|hunks| now.with(|now| now.counts(hunks)))
}

/// The key hints, quietly, under the decision row.
fn key_hints(language: Language) -> impl IntoElement {
    let pair = |key: &'static str, label: S| {
        view()
            .flex_row()
            .items_center()
            .gap(px(SPACE_1))
            .child(prims::kbd(key))
            .child(prims::faint(label.text(language)))
    };
    view()
        .test_id("revision-keys")
        .flex_row()
        .items_center()
        .gap(px(SPACE_3))
        .child(pair("A", S::Accept))
        .child(pair("R", S::Reject))
        .child(pair("J / K", S::Move))
        .child(pair("Ctrl+Z", S::Undo))
}

// ─── keys ─────────────────────────────────────────────────────────────────

/// `A` / `R` decide the current hunk, `J` / `K` move between them, and
/// `Ctrl+Z` takes the last decision back — all while no text input has the
/// keyboard focus, so the document stays fully editable. They do nothing
/// without a proposal, so they may stay bound; `Done`'s own key has a side
/// effect with none, and is bound by [`bind_done_key`] only where it applies.
fn bind_keys(state: &AppState, heat: Heat) {
    let keys = state.clone();
    on_key("a", move |_| decide(&keys, heat, true));
    let keys = state.clone();
    on_key("r", move |_| decide(&keys, heat, false));
    let keys = state.clone();
    on_key("j", move |_| move_by(&keys, 1));
    let keys = state.clone();
    on_key("k", move |_| move_by(&keys, -1));
    let keys = state.clone();
    on_key("ctrl+z", move |_| undo_decision(&keys));
}

/// Whether `Ctrl+Enter` is `Done` (else it is `Review`,
/// [`titlebar::bind_mod_enter`]): only while a proposal is on screen and
/// fresh, so the shortcut keeps starting a Review from every other state.
pub(crate) fn done_applies(state: &AppState) -> bool {
    !state.revision_hunks.with(|hunks| hunks.is_empty()) && !state.revision_is_stale()
}

// ─── actions ──────────────────────────────────────────────────────────────

/// Make the current hunk's decision: record it, light its pip, and — after
/// the docked proposal's extinguish — bring the next undecided hunk into the
/// reading position.
fn decide(state: &AppState, heat: Heat, accepted: bool) {
    let hunks = state.revision_hunks.get();
    let signal = session();
    let mut next = signal.with(|now| now.synced(&hunks));
    let Some(index) = next.current(&hunks) else {
        return;
    };
    // A hunk whose part changed offers no Accept; the ledger's own byte check
    // is the second gate.
    if accepted && (state.revision_is_stale() || cannot_apply(state, &hunks[index])) {
        return;
    }
    let change_id = hunks[index].change_id;
    if next.decision(change_id) == Some(accepted) {
        // The same decision again: nothing to record; step on.
        next.advance(&hunks, index);
        signal.set(next);
        return;
    }
    // The text first: a decision whose text the state refuses is not made
    // (the refusal is on the status line).
    if !state.set_revision_change_applied(change_id, accepted) {
        return;
    }
    next.record(&hunks, index, accepted);
    let proposal = next.proposal;
    signal.set(next);
    heat.extinguish();
    let state = state.clone();
    set_timeout(
        Duration::from_millis(motion::EXTINGUISH_MS as u64),
        move || advance_after(&state, proposal, index),
    );
}

/// Bring the next undecided hunk forward once the decided one has
/// extinguished — unless the reading position has moved since (a `J`, another
/// decision, `Done`): then what moved it stands.
fn advance_after(state: &AppState, proposal: u64, decided: usize) {
    let hunks = state.revision_hunks.get();
    let signal = session();
    signal.update(|now| {
        if now.proposal != proposal || now.current != decided {
            return;
        }
        let mut synced = now.synced(&hunks);
        synced.advance(&hunks, decided);
        *now = synced;
    });
}

/// Move the reading position by `step` hunks (`J` / `K`).
fn move_by(state: &AppState, step: isize) {
    let hunks = state.revision_hunks.get();
    let signal = session();
    signal.update(|now| {
        let mut synced = now.synced(&hunks);
        let Some(current) = synced.current(&hunks) else {
            return;
        };
        let last = hunks.len() as isize - 1;
        synced.current = (current as isize + step).clamp(0, last) as usize;
        *now = synced;
    });
}

/// Take the last decision back together with its text: the change returns
/// to what it was decided as before it (its reviewed bytes, unless an earlier
/// decision accepted it). When the state refuses — the accepted text was
/// edited since — the decision stays and the status line says why.
fn undo_decision(state: &AppState) {
    let hunks = state.revision_hunks.get();
    let signal = session();
    let mut taken_back = signal.with(|now| now.synced(&hunks));
    let Some(change_id) = taken_back.undo() else {
        return;
    };
    let applied = taken_back.decision(change_id) == Some(true);
    if state.set_revision_change_applied(change_id, applied) {
        signal.set(taken_back);
    }
}

/// `Done`, with the hunks still undecided counted as discarded.
pub(crate) fn done(state: &AppState) {
    let hunks = state.revision_hunks.get();
    let (decided, all) = session().with(|now| now.synced(&hunks).counts(&hunks));
    state.finish_revision(all - decided);
}

/// `Copy`: the approved result on the clipboard. Nothing else changes — not
/// the editor, not the dirty flag, not the file.
fn copy_approved(state: &AppState) {
    if let Some(text) = state.revision_approved_text() {
        set_clipboard(text.to_string());
    }
}

/// Dismiss the outcome text on screen: it is consumed, so the next outcome
/// opens the field again however it reads — an identical failure included
/// (the state writes it again without ever going back to nothing).
fn dismiss(state: &AppState) {
    state.revision_status.set(None);
    state.revision_not_applied.set(0);
}

/// The reading position to hunk `index` (a pressed row).
fn move_to(state: &AppState, index: usize) {
    let hunks = state.revision_hunks.get();
    let signal = session();
    signal.update(|now| {
        let mut synced = now.synced(&hunks);
        let Some(current) = synced.current(&hunks) else {
            return;
        };
        synced.current = index.min(hunks.len() - 1);
        if synced.current == current {
            return;
        }
        *now = synced;
    });
}

/// Bind the session to the proposal `hunks` hold: a reset one when they are
/// not the proposal it belongs to.
fn sync_session(hunks: &[RevisionHunkView], now: Signal<Session>) {
    now.update(|session| {
        let synced = session.synced(hunks);
        if synced != *session {
            *session = synced;
        }
    });
}

/// Keep the session the one the proposal on screen's: a proposal that
/// arrives without the field remounting (which the shell's routing does, but
/// nothing here may assume) still starts the decisions over.
fn watch_proposal(hunks: Signal<Vec<RevisionHunkView>>, now: Signal<Session>) {
    effect(move || {
        let current = hunks.get();
        sync_session(&current, now);
    });
}

// ─── heat ─────────────────────────────────────────────────────────────────

/// The docked proposal ignites and stays hot: the ignition's rise without its
/// decay — this state's one HOT item, like the current question in a review.
fn ignite_hot(heat: Heat) {
    let value = heat.value();
    value.stop();
    let (ms, easing) = if motion::reduced_motion() {
        (motion::REDUCED_CROSSFADE_MS, Easing::EaseInOut)
    } else {
        (motion::IGNITE_MS, Easing::Linear)
    };
    value.timing(1.0, ms, easing);
}

/// Keeps the heat on what the field is asking to be judged: ignite when an
/// undecided hunk comes into the reading position (`J` / `K`, a row press, or
/// the advance after a decision), extinguish the moment it is decided — the
/// settled docked block then reads through ember to plain ink.
fn watch_heat(hunks: Signal<Vec<RevisionHunkView>>, now: Signal<Session>, heat: Heat) {
    let last = std::rc::Rc::new(std::cell::Cell::new((usize::MAX, usize::MAX)));
    effect(move || {
        let key = hunks.with(|hunks| {
            now.with(|now| {
                (
                    now.current(hunks).unwrap_or(usize::MAX),
                    now.counts(hunks).0,
                )
            })
        });
        let hot = hunks.with(|hunks| now.with(|now| now.undecided(hunks)));
        let previous = last.replace(key);
        if !hot {
            heat.extinguish();
            return;
        }
        if previous == key {
            return;
        }
        ignite_hot(heat);
    });
}

// ─── the document ─────────────────────────────────────────────────────────

/// Whether the hunk cannot be accepted because the user edited the part of
/// the document it covers (the state's ledger says so). An accepted hunk is
/// not refused here: its text is already in.
fn cannot_apply(state: &AppState, hunk: &RevisionHunkView) -> bool {
    !hunk.accepted.get() && state.revision_hunk_part_changed(hunk.ordinal)
}

// ─── text helpers ─────────────────────────────────────────────────────────

/// Every line of one side of a hunk: nothing is cut here, a tall side
/// scrolls in the field. Nothing to show (an insertion has no old text) is an
/// empty vector.
fn hunk_lines(text: &str) -> Vec<SharedString> {
    text.lines()
        .map(|line| SharedString::from(line.to_owned()))
        .collect()
}

/// The first line of `text`, cut to `max` characters on a character boundary.
fn digest(text: &str, max: usize) -> String {
    let line = text.lines().next().unwrap_or_default().trim();
    if line.chars().count() <= max {
        return line.to_owned();
    }
    let mut cut: String = line.chars().take(max.saturating_sub(1)).collect();
    cut.push('…');
    cut
}

#[cfg(test)]
mod tests {
    use super::*;
    use nsh_engine::{EngineConfig, Root};
    use nsh_testing::Headless;

    fn hunk(
        ordinal: usize,
        change_id: u32,
        rationale: &str,
        before: &str,
        after: &str,
    ) -> RevisionHunkView {
        RevisionHunkView {
            ordinal,
            change_id,
            rationale: rationale.to_owned(),
            before: before.to_owned(),
            after: after.to_owned(),
            accepted: signal(false),
        }
    }

    fn two_hunks() -> Vec<RevisionHunkView> {
        vec![
            hunk(
                0,
                0,
                "Tighten the promise.",
                "compiles but",
                "compiles, but",
            ),
            hunk(1, 1, "Name the check.", "no check", "one check"),
        ]
    }

    /// The probe document every two-change proposal reads.
    const PROBE_TEXT: &str = "compiles but no check\n";

    /// `two_hunks` as a proposal over `PROBE_TEXT`: (reason, old, new).
    const TWO_CHANGES: &[(&str, &str, &str)] = &[
        ("Tighten the promise.", "compiles but", "compiles, but"),
        ("Name the check.", "no check", "one check"),
    ];

    /// A second proposal over the document after the first one's accept:
    /// same ordinals and change ids (the core numbers them per proposal from
    /// zero), other content.
    const REPLACEMENT_CHANGES: &[(&str, &str, &str)] = &[
        ("Another take.", "compiles, but", "compiles cleanly"),
        ("Second take.", "no check", "two checks"),
    ];

    /// Both sides of one hunk with more lines than the docked position shows
    /// at once.
    fn tall_sides() -> (String, String) {
        let side = |word: &str| {
            (1..=20)
                .map(|line| format!("{word} line {line}"))
                .collect::<Vec<_>>()
                .join("\n")
        };
        (side("old"), side("new"))
    }

    /// A proposal as large as the core allows a list of rows to be: the
    /// document and its changes.
    fn many_changes() -> (String, Vec<(String, String, String)>) {
        let source = (0..40).map(|index| format!("old {index}\n")).collect();
        let changes = (0..40)
            .map(|index| {
                (
                    format!("Reason {index}."),
                    format!("old {index}\n"),
                    format!("new {index}\n"),
                )
            })
            .collect();
        (source, changes)
    }

    /// Present a validated proposal over `source` — one change per
    /// `(reason, old, new)`, at the old text's first place — as a real
    /// Revision arrives: through the state, which places it in the tab on
    /// screen.
    fn present(state: &AppState, source: &str, changes: &[(&str, &str, &str)]) {
        use mt_core::review::{
            ByteRange, SourceSnapshot,
            revision::{RevisionChange, RevisionEdit, RevisionLimits, RevisionProposal},
        };
        let changes = changes
            .iter()
            .map(|&(reason, before, after)| {
                let start = source.find(before).expect("the old text is in the source") as u64;
                let range = ByteRange::new(start, start + before.len() as u64).expect("a range");
                RevisionChange::new(reason, vec![RevisionEdit::new(range, before, after)])
            })
            .collect();
        let proposal = RevisionProposal::validate(
            source,
            SourceSnapshot::new(0, 0),
            changes,
            RevisionLimits::default(),
        )
        .expect("a valid proposal");
        state.present_revision(proposal);
    }

    #[test]
    fn decisions_advance_undo_and_reset_with_the_proposal() {
        // The hunks carry signals; the model below never reads them, but
        // making them needs a reactive runtime (what a component runs in).
        let runtime = nsh_core::element::Runtime::default();
        let _entered = runtime.enter();
        let hunks = two_hunks();
        let mut session = Session::default().synced(&hunks);
        assert_eq!(session.proposal, proposal_id(&hunks));
        assert_eq!(session.docked(&hunks), Some(0));
        assert_eq!(session.counts(&hunks), (0, 2));

        session.record(&hunks, 0, true);
        assert_eq!(session.decision(0), Some(true));
        assert_eq!(session.counts(&hunks), (1, 2));
        assert_eq!(
            session.docked(&hunks),
            Some(0),
            "a decided hunk stays in the reading position until the next one"
        );
        assert!(!session.undecided(&hunks), "it is not what stays HOT");
        session.advance(&hunks, 0);
        assert_eq!(session.docked(&hunks), Some(1));
        assert!(session.undecided(&hunks));
        assert_eq!(session.current, 1);

        session.record(&hunks, 1, false);
        session.advance(&hunks, 1);
        assert_eq!(session.counts(&hunks), (2, 2));
        assert_eq!(
            session.docked(&hunks),
            Some(1),
            "with nothing undecided left, the last one stays docked"
        );
        assert!(!session.undecided(&hunks));

        assert_eq!(session.undo(), Some(1));
        assert_eq!(session.current, 1, "undo restores the reading position");
        assert_eq!(session.docked(&hunks), Some(1));
        assert!(session.undecided(&hunks));

        // A re-decision overrides; the last record of a change id wins.
        session.record(&hunks, 1, true);
        assert_eq!(session.decision(1), Some(true));
        assert_eq!(session.counts(&hunks), (2, 2));

        // Another proposal never inherits a decision.
        let other = vec![hunk(
            0,
            0,
            "Another take.",
            "compiles but",
            "compiles cleanly",
        )];
        let fresh = session.synced(&other);
        assert_eq!(fresh.counts(&other), (0, 1));
        assert_eq!(fresh.docked(&other), Some(0));
        assert_ne!(fresh.proposal, session.proposal);
    }

    /// The two-change proposal, then an edit inside the first change's old
    /// text, made as the editor makes one.
    #[component]
    fn ChangedBaseProbe() -> impl IntoElement {
        let state = AppState::new();
        open_probe_document(&state, PROBE_TEXT);
        present(&state, PROBE_TEXT, TWO_CHANGES);
        if let Some(tab) = state.active_tab() {
            tab.text.set(SharedString::from(
                "compiles, yet no check
",
            ));
            tab.revision.update(|revision| *revision += 1);
            state.note_edit(tab.id);
        }
        field(state, Language::English)
    }

    #[test]
    fn a_changed_base_refuses_the_hunk() {
        let mut app = Headless::start(EngineConfig::new(Root::of::<ChangedBaseProbe>()))
            .expect("start the field");
        app.wait_for("id:revision-proposal-blocked")
            .expect("the edited hunk says it cannot apply");
        assert!(app.shows("Can't apply — this part changed"));
        assert!(
            !app.screen()
                .matches("id:revision-accept")
                .expect("the selector"),
            "no Accept on a hunk whose base changed: {:?}",
            app.screen().texts()
        );
        app.press_key("a").expect("press A");
        app.wait(Duration::from_millis(50)).expect("a frame");
        assert!(app.shows("0 of 2 decided"), "A decided nothing");

        // The untouched hunk is still where the edit left it, and accepts.
        app.press_key("j").expect("press J");
        app.wait_for("id:revision-accept")
            .expect("the untouched hunk can be accepted");
        assert!(!app.shows("Can't apply — this part changed"));
    }

    /// The two-change proposal, then another document comes on screen.
    #[component]
    fn OtherDocumentProbe() -> impl IntoElement {
        let state = AppState::new();
        open_probe_document(&state, PROBE_TEXT);
        present(&state, PROBE_TEXT, TWO_CHANGES);
        open_probe_document(&state, PROBE_TEXT);
        field(state, Language::English)
    }

    #[test]
    fn a_proposal_for_another_document_says_so_and_offers_no_accept() {
        let mut app = Headless::start(EngineConfig::new(Root::of::<OtherDocumentProbe>()))
            .expect("start the field");
        app.wait_for("id:revision-stale")
            .expect("the field says the proposal is not this document's");
        assert!(app.shows("This Revision belongs to another document"));
        assert!(
            !app.shows("Stale — edited after review"),
            "nothing was edited after the review"
        );
        assert!(
            !app.screen()
                .matches("id:revision-accept")
                .expect("the selector"),
            "no Accept into a document the proposal was not made for: {:?}",
            app.screen().texts()
        );
    }

    #[test]
    fn digests_stay_one_line() {
        assert_eq!(digest("First line.\nSecond line.", 72), "First line.");
        assert_eq!(digest("  padded  ", 72), "padded");
        let long = "x".repeat(100);
        let cut = digest(&long, 10);
        assert_eq!(cut.chars().count(), 10);
        assert!(cut.ends_with('…'));
    }

    #[test]
    fn hunk_lines_keep_every_line() {
        assert!(hunk_lines("").is_empty());
        assert_eq!(hunk_lines("one\ntwo").len(), 2);
        // Nothing is cut: the docked hunk scrolls instead (finding 4).
        let many = (0..20)
            .map(|i| i.to_string())
            .collect::<Vec<_>>()
            .join("\n");
        let lines = hunk_lines(&many);
        assert_eq!(lines.len(), 20);
        assert_eq!(lines.last().unwrap().as_str(), "19");
    }

    /// A surface with a synthetic proposal: the field alone, as `app.rs`
    /// mounts it through the shell.
    #[component]
    fn ProposalProbe() -> impl IntoElement {
        let state = AppState::new();
        open_probe_document(&state, PROBE_TEXT);
        present(&state, PROBE_TEXT, TWO_CHANGES);
        field(state, Language::English)
    }

    #[test]
    fn the_keys_decide_the_docked_hunk_and_advance() {
        let mut app = Headless::start(EngineConfig::new(Root::of::<ProposalProbe>()))
            .expect("start the field");
        app.wait_for("id:revision-field").expect("the field");
        app.wait_for("text:Accept").expect("the docked proposal");

        assert!(app.shows("0 of 2 decided"));
        assert!(app.shows("compiles but"), "the old text is on screen");
        assert!(app.shows("compiles, but"), "the new text is on screen");
        assert!(app.shows("Tighten the promise."));

        // The docked proposal ignites (90ms); let the heat settle before the
        // colours are read.
        app.wait(Duration::from_millis(250)).expect("the ignition");
        // The old text is the palette's cold strike, the new text the same
        // appearance's hot proposal — drawn from `tokens`, not invented here.
        let strike = app
            .screen()
            .span_style("text:compiles but")
            .expect("the old text's span");
        let added = app
            .screen()
            .span_style("text:compiles, but")
            .expect("the new text's span");
        assert!(strike.decoration.line_through, "struck: {strike:?}");
        assert!(strike.decoration.underline, "and underlined: {strike:?}");
        assert_eq!(
            strike.decoration_color,
            Some(strike.color),
            "the strike and its underline are the cold colour"
        );
        let dark = strike.color == tokens::DARK.removed;
        assert!(
            dark || strike.color == tokens::LIGHT.removed,
            "the cold strike is the palette's removed: {:?}",
            strike.color
        );
        assert_eq!(
            added.color,
            if dark {
                tokens::DARK.added
            } else {
                tokens::LIGHT.added
            },
            "the proposal's colour is the same appearance's added one (the \
             settling heat is bound over it, which a resolved span does not \
             show)"
        );
        // While hunks wait to be judged, `Accept` is the field's one next
        // action and `Done` stays quiet (finding 7).
        let done = app
            .screen()
            .span_style("text:Done")
            .expect("the Done label");
        assert_eq!(
            done.color,
            if dark {
                tokens::DARK.ink_rest
            } else {
                tokens::LIGHT.ink_rest
            },
            "Done is the quiet action while hunks wait"
        );

        app.press_key("a").expect("press A");
        app.wait_for("text:1 of 2 decided")
            .expect("the decision is recorded");
        // The decided hunk stays in the reading position until its ignition
        // extinguishes; then the next undecided one comes forward.
        app.wait_for("text:no check")
            .expect("the next undecided hunk is docked");
        assert!(
            !app.shows("compiles but"),
            "the decided hunk left the reading position"
        );
        assert!(
            app.shows("1 not applied"),
            "the undecided hunk is counted: {:?}",
            app.screen().texts()
        );

        // The docked position advances only after the extinguish, which the
        // pumps above let pass.
        app.press_key("r").expect("press R");
        app.wait_for("text:2 of 2 decided").expect("both decided");
        // Nothing is left to judge: the last decided hunk stays docked with
        // its decision, so a re-decision (or `Undo`) happens on screen.
        assert!(
            app.shows("no check") && app.shows("Rejected"),
            "the decided hunk stays docked: {:?}",
            app.screen().texts()
        );
        assert!(
            app.screen()
                .matches("id:revision-accept")
                .expect("the selector"),
            "it can be decided again: {:?}",
            app.screen().texts()
        );
        // Every hunk judged: `Done` becomes the field's one next action.
        let done = app
            .screen()
            .span_style("text:Done")
            .expect("the Done label");
        assert_eq!(
            done.color,
            if dark {
                tokens::DARK.ink
            } else {
                tokens::LIGHT.ink
            },
            "Done is the primary action once every hunk is decided"
        );

        app.press_key("ctrl+z").expect("undo");
        app.wait_for("text:1 of 2 decided")
            .expect("the decision is back");
        assert!(
            app.screen()
                .matches("id:revision-accept")
                .expect("the selector"),
            "the docked proposal is back: {:?}",
            app.screen().texts()
        );

        // A row is a control: pressing it moves the reading position there,
        // so the mouse can reach every hunk.
        app.press("label:Revision hunk 1").expect("press the row");
        app.wait_for("text:compiles but")
            .expect("the first hunk docks on the mouse press");

        // `Done` is reachable without the mouse: the key runs it, keeping
        // the accepted change and discarding the one still undecided.
        app.press_key("ctrl+enter").expect("press Ctrl+Enter");
        app.wait_for("text:Approved changes applied to the buffer.")
            .expect("Done ran from the keyboard");
        app.wait_for("text:1 not applied")
            .expect("the undecided hunk was discarded, not written");
    }

    /// A surface that routes on `is_open`, as the shell does.
    #[component]
    fn OutcomeProbe() -> impl IntoElement {
        let state = AppState::new();
        state.revision_status.set(Some(failure_text()));
        let show_state = state.clone();
        let fail_state = state.clone();
        view()
            .child(when(
                move || is_open(&show_state),
                move || field(state.clone(), Language::English),
            ))
            .child(
                text("fail-again")
                    .on_press(move || fail_state.revision_status.set(Some(failure_text()))),
            )
            .child(text("probe-tail"))
    }

    fn failure_text() -> SharedString {
        SharedString::from("Revision failed: Timed out after 60 s")
    }

    /// A field that stays mounted while a second proposal arrives in place —
    /// the case where nothing may inherit the first proposal's decisions.
    #[component]
    fn SwapProbe() -> impl IntoElement {
        let state = AppState::new();
        open_probe_document(&state, PROBE_TEXT);
        present(&state, PROBE_TEXT, TWO_CHANGES);
        let swap_state = state.clone();
        view()
            .child(text("swap").on_press(move || {
                // A second proposal, read from the text as it is now (the
                // first proposal's accepted change included).
                let current = swap_state
                    .active_tab()
                    .expect("the probe document")
                    .text
                    .get()
                    .to_string();
                present(&swap_state, &current, REPLACEMENT_CHANGES);
            }))
            .child(field(state, Language::English))
    }

    /// A proposed line that would run as script in a web view.
    const SCRIPT_LINE: &str = "<script>alert('proposal')</script>";

    /// A proposal whose hunk is taller than the docked position shows at
    /// once, its last proposed line markup that would run as script.
    #[component]
    fn TallHunkProbe() -> impl IntoElement {
        let state = AppState::new();
        let (before, after) = tall_sides();
        let after = format!("{after}\n{SCRIPT_LINE}");
        let source = format!("{before}\n");
        open_probe_document(&state, &source);
        present(&state, &source, &[("Rewrite the block.", &before, &after)]);
        field(state, Language::English)
    }

    /// A proposal with more rows than the field can hold.
    #[component]
    fn ManyHunksProbe() -> impl IntoElement {
        let state = AppState::new();
        let (source, changes) = many_changes();
        open_probe_document(&state, &source);
        let changes: Vec<(&str, &str, &str)> = changes
            .iter()
            .map(|(reason, before, after)| (reason.as_str(), before.as_str(), after.as_str()))
            .collect();
        present(&state, &source, &changes);
        field(state, Language::English)
    }

    /// The whole room, with a synthetic proposal: the field box as `app.rs`
    /// mounts it.
    #[component]
    fn ShellProbe() -> impl IntoElement {
        let state = AppState::new();
        open_probe_document(&state, PROBE_TEXT);
        present(&state, PROBE_TEXT, TWO_CHANGES);
        crate::ui::shell(state, Language::English, crate::ui::tokens::Layout::Wide)
    }

    #[test]
    fn the_docked_proposal_sits_near_the_fields_middle_with_the_rows_around_it() {
        let mut app =
            Headless::start(EngineConfig::new(Root::of::<ShellProbe>())).expect("start the room");
        app.set_viewport(Size {
            width: Points(1440.0),
            height: Points(900.0),
        })
        .expect("a 1440x900 window");
        app.first_render().expect("first render");
        app.wait_for("id:revision-proposal")
            .expect("the docked proposal");

        let nodes = app.screen().layout_nodes();
        let bounds = |test_id: &str| {
            nodes
                .iter()
                .find(|node| node.test_id.as_deref() == Some(test_id))
                .map(|node| node.bounds)
                .unwrap_or_else(|| panic!("no {test_id} on screen: {:?}", app.screen().texts()))
        };
        let field = bounds("revision-field");
        let proposal = bounds("revision-proposal");
        let rows = bounds("revision-rows");
        let decisions = bounds("revision-decisions");
        let keys = bounds("revision-keys");

        // The field is the window under the 32px title row, in the field's
        // own box (the 440px docked reading position).
        let expected = 900.0 - tokens::TITLE_ROW_H;
        assert!(
            (field.size.height.0 - expected).abs() < 2.0,
            "field height {} instead of {expected}",
            field.size.height.0
        );
        assert!(
            field.size.width.0 <= tokens::FIELD_READING_MAX_W + 1.0,
            "field width {}",
            field.size.width.0
        );

        // The docked proposal is at the field's vertical centre, and the
        // other hunk, the pips and the key hints are ordered around it.
        let middle = |b: &Bounds<Points>| b.origin.y.0 + b.size.height.0 / 2.0;
        assert!(
            (middle(&proposal) - middle(&field)).abs() < 80.0,
            "docked at {} of the field's {}",
            middle(&proposal),
            middle(&field)
        );
        assert!(rows.origin.y.0 <= proposal.origin.y.0);
        assert!(
            decisions.origin.y.0 > proposal.origin.y.0,
            "the pips and Done are under the docked proposal"
        );
        assert!(keys.origin.y.0 > decisions.origin.y.0);
        assert!(
            decisions.origin.y.0 + decisions.size.height.0
                <= field.origin.y.0 + field.size.height.0
        );
    }

    #[test]
    fn a_dismissed_outcome_never_hides_the_next_one() {
        let mut app =
            Headless::start(EngineConfig::new(Root::of::<OutcomeProbe>())).expect("start");
        app.wait_for("text:Revision failed: Timed out after 60 s")
            .expect("the failure is on screen");
        app.wait_for("id:revision-dismiss").expect("a way out");
        app.press("text:Close").expect("dismiss");
        app.wait_for("text:probe-tail").expect("the probe's tail");
        assert!(
            !app.shows("Revision failed: Timed out after 60 s"),
            "the dismissed outcome hides the field"
        );

        // A second attempt failing the very same way writes the same outcome
        // without ever going back to nothing (what `Prepare revision` does):
        // a failure is never silently hidden behind the dismissal.
        app.press("text:fail-again").expect("it fails the same way");
        app.wait_for("text:Revision failed: Timed out after 60 s")
            .expect("the same failure shows again");
        assert!(
            app.screen()
                .matches("id:revision-dismiss")
                .expect("the selector"),
            "and can be dismissed again: {:?}",
            app.screen().texts()
        );
    }

    #[test]
    fn done_key_stays_unbound_where_nothing_applies() {
        let mut app =
            Headless::start(EngineConfig::new(Root::of::<OutcomeProbe>())).expect("start");
        app.wait_for("text:Revision failed: Timed out after 60 s")
            .expect("the failure is on screen");
        // `Ctrl+Enter` is `Done` only while a proposal could apply: with none
        // it belongs to the title row's `mod+enter` (`Review`), and the field
        // must not write `Done`'s own refusal over the outcome on screen.
        app.press_key("ctrl+enter").expect("press Ctrl+Enter");
        app.wait_for("text:probe-tail").expect("the probe's tail");
        assert!(
            app.shows("Revision failed: Timed out after 60 s"),
            "the outcome is untouched: {:?}",
            app.screen().texts()
        );
        assert!(
            !app.shows("Run Review first to prepare a Revision."),
            "and `Done` did not claim the key: {:?}",
            app.screen().texts()
        );
    }

    #[test]
    fn a_second_proposal_starts_the_decisions_over() {
        let mut app =
            Headless::start(EngineConfig::new(Root::of::<SwapProbe>())).expect("start the field");
        app.wait_for("id:revision-field").expect("the field");
        app.press_key("a").expect("press A");
        app.wait_for("text:1 of 2 decided").expect("one decided");
        // The decided row appears once the dock has extinguished (a timer),
        // so it is waited for rather than read in the same frame.
        app.wait_for("text:Tighten the promise. · Accepted")
            .expect("the accepted hunk's row");

        // The proposal is replaced in place: same ordinals, same change ids,
        // other content. Nothing of the first proposal's decisions may stick.
        app.press("text:swap").expect("a second proposal");
        app.wait_for("text:0 of 2 decided")
            .expect("the decisions started over");
        assert!(
            app.shows("compiles cleanly") && app.shows("Another take."),
            "the second proposal's docked hunk: {:?}",
            app.screen().texts()
        );
        assert!(
            !app.shows("· Accepted") && !app.shows("· Rejected"),
            "nothing is shown as decided: {:?}",
            app.screen().texts()
        );
        assert!(
            app.screen()
                .matches("id:revision-accept")
                .expect("the selector"),
            "and its hunk can be accepted: {:?}",
            app.screen().texts()
        );
    }

    #[test]
    fn a_tall_hunk_shows_every_line() {
        let mut app = Headless::start(EngineConfig::new(Root::of::<TallHunkProbe>()))
            .expect("start the field");
        app.wait_for("id:revision-proposal-text")
            .expect("the docked text region");
        // Nothing is cut out of an Accept: the whole side is on screen (the
        // region scrolls).
        for line in ["old line 1", "old line 20", "new line 1", "new line 20"] {
            assert!(
                app.shows(line),
                "{line} is on screen: {:?}",
                app.screen().texts()
            );
        }
    }

    #[test]
    fn a_huge_proposal_keeps_the_decision_row_in_the_field() {
        let mut app = Headless::start(EngineConfig::new(Root::of::<ManyHunksProbe>()))
            .expect("start the field");
        app.set_viewport(Size {
            width: Points(1440.0),
            height: Points(900.0),
        })
        .expect("a 1440x900 window");
        app.first_render().expect("first render");
        app.wait_for("id:revision-decisions")
            .expect("the decision row");

        let nodes = app.screen().layout_nodes();
        let bounds = |test_id: &str| {
            nodes
                .iter()
                .find(|node| node.test_id.as_deref() == Some(test_id))
                .map(|node| node.bounds)
                .unwrap_or_else(|| panic!("no {test_id} on screen"))
        };
        let field = bounds("revision-field");
        let decisions = bounds("revision-decisions");
        let keys = bounds("revision-keys");
        let rows = bounds("revision-rows");
        assert!(
            rows.size.height.0 > 0.0,
            "the rows get the room left: {:?}",
            rows.size
        );
        assert!(
            decisions.origin.y.0 >= field.origin.y.0
                && decisions.origin.y.0 + decisions.size.height.0
                    <= field.origin.y.0 + field.size.height.0,
            "the pips and Done stay inside the field: {decisions:?} in {field:?}"
        );
        assert!(
            keys.origin.y.0 + keys.size.height.0 <= field.origin.y.0 + field.size.height.0,
            "and so do the key hints: {keys:?}"
        );

        // The reading position reaches the end of a huge proposal: `J` walks
        // there, and the docked hunk comes into view inside the rows' own
        // scroll view.
        for _ in 0..39 {
            app.press_key("j").expect("press J");
        }
        app.wait_for("text:Reason 39.")
            .expect("the last hunk docks");
        let nodes = app.screen().layout_nodes();
        let bounds = |test_id: &str| {
            nodes
                .iter()
                .find(|node| node.test_id.as_deref() == Some(test_id))
                .map(|node| node.bounds)
                .unwrap_or_else(|| panic!("no {test_id} on screen"))
        };
        let rows = bounds("revision-rows");
        let docked = bounds("revision-proposal");
        assert!(
            docked.origin.y.0 >= rows.origin.y.0
                && docked.origin.y.0 + docked.size.height.0 <= rows.origin.y.0 + rows.size.height.0,
            "the docked hunk is kept in view: {docked:?} in {rows:?}"
        );
    }

    /// The two-change proposal with the column's marks for the tab on screen
    /// written out, and the controls that change them from outside the
    /// field: an edit inside the first hunk's old text, and another document.
    #[component]
    fn ColumnMarksProbe() -> impl IntoElement {
        let state = AppState::new();
        open_probe_document(&state, PROBE_TEXT);
        present(&state, PROBE_TEXT, TWO_CHANGES);
        let marks_state = state.clone();
        let edit_state = state.clone();
        let other_state = state.clone();
        view()
            .child(text(move || {
                marks_state.active_tab().map_or_else(
                    || "marks: no tab".to_owned(),
                    |tab| describe_marks(&column_decorations(&marks_state, &tab)),
                )
            }))
            .child(text("edit-first").on_press(move || {
                if let Some(tab) = edit_state.active_tab() {
                    let edited = tab.text.get().replacen("compiles but", "compiles, yet", 1);
                    tab.text.set(SharedString::from(edited));
                    tab.revision.update(|revision| *revision += 1);
                    edit_state.note_edit(tab.id);
                }
            }))
            .child(text("other-document").on_press(move || other_state.new_document()))
            .child(field(state, Language::English))
    }

    /// The marks as `line:column-line:column` ranges, each flagged when its
    /// style is anything but the cold strike: a 1px strike and underline in
    /// the palette's removed colour, the glyphs left alone, nothing on the
    /// line or in the gutter.
    fn describe_marks(decorations: &[EditorDecoration]) -> String {
        use nsh_core::StyleDecl;
        let cold = [
            StyleDecl::Set(
                StyleKey::TextDecorationLine,
                PropertyValue::TextDecoration(Some(TextDecoration {
                    underline: true,
                    line_through: true,
                })),
            ),
            StyleDecl::Set(
                StyleKey::TextDecorationColor,
                PropertyValue::Color(Some(tokens::palette().removed)),
            ),
            StyleDecl::Set(
                StyleKey::TextDecorationThickness,
                PropertyValue::OptPoints(Some(px(1.0))),
            ),
        ];
        let marks: Vec<String> = decorations
            .iter()
            .map(|decoration| {
                let struck_cold = decoration
                    .style
                    .as_ref()
                    .is_some_and(|style| style.decls() == &cold[..])
                    && decoration.line_style.is_none()
                    && decoration.gutter.is_none()
                    && decoration.class.is_none();
                let range = decoration.range;
                format!(
                    "{}:{}-{}:{}{}",
                    range.start.line,
                    range.start.column,
                    range.end.line,
                    range.end.column,
                    if struck_cold { "" } else { " (not cold)" }
                )
            })
            .collect();
        if marks.is_empty() {
            "marks: none".to_owned()
        } else {
            format!("marks: {}", marks.join(" "))
        }
    }

    #[test]
    fn the_column_strikes_the_undecided_hunks_old_words_cold() {
        let mut app = Headless::start(EngineConfig::new(Root::of::<ColumnMarksProbe>()))
            .expect("start the probe");
        // `compiles but no check`: both hunks wait, both old texts struck.
        app.wait_for("text:marks: 0:0-0:12 0:13-0:21")
            .expect("each undecided hunk's old words are struck cold");

        // Accept: the replacement is plain text, its strike gone in the same
        // step; the other hunk's mark follows its bytes one column on.
        app.press_key("a").expect("accept the first hunk");
        app.wait_for("text:marks: 0:14-0:22")
            .expect("the accepted hunk loses its mark, the other one moves");
        app.wait_for("text:Tighten the promise. · Accepted")
            .expect("the second hunk is docked");

        // Reject: the cold marks go.
        app.press_key("r").expect("reject the second hunk");
        app.wait_for("text:marks: none")
            .expect("a rejected hunk loses its marks");

        // Undo walks back: undecided again, struck again.
        app.press_key("ctrl+z").expect("undo the rejection");
        app.wait_for("text:marks: 0:14-0:22")
            .expect("the undecided hunk is struck again");
        app.press_key("ctrl+z").expect("undo the accept");
        app.wait_for("text:marks: 0:0-0:12 0:13-0:21")
            .expect("the reviewed words are back, and struck");

        // An edit inside the first hunk's old words: it has no place left to
        // mark (and no Accept); the untouched hunk keeps its own bytes.
        app.press("text:edit-first")
            .expect("edit inside the first hunk");
        app.wait_for("text:marks: 0:14-0:22")
            .expect("only the untouched hunk is marked, where it moved");

        // Another document on screen: the proposal's marks stay in its own.
        app.press("text:other-document")
            .expect("open another document");
        app.wait_for("text:marks: none")
            .expect("another document carries no marks");
    }

    #[test]
    fn the_docked_proposal_reads_as_a_labelled_group_of_lines() {
        let mut app = Headless::start(EngineConfig::new(Root::of::<TallHunkProbe>()))
            .expect("start the field");
        app.wait_for("id:revision-proposal-after")
            .expect("the proposal's lines");
        let screen = app.screen();
        assert_eq!(
            screen.count("label:Proposed text").expect("the selector"),
            1,
            "one labelled proposal: {:?}",
            screen.texts()
        );
        // Every proposed line is a text inside the labelled group, in the
        // group the harness reads by its id.
        for group in ["label:Proposed text", "id:revision-proposal-after"] {
            assert_eq!(
                screen
                    .count(&format!("{group} >> text:/^new line [0-9]+$/"))
                    .expect("the selector"),
                20,
                "every proposed line is a text of {group}: {:?}",
                screen.texts()
            );
            assert!(
                !screen
                    .matches(&format!("{group} >> text:old line 1"))
                    .expect("the selector"),
                "the old text is not part of the proposal"
            );
        }
        // Proposed markup is inert: its script is a line of text in the
        // proposal, and nothing in the field renders it as a page.
        assert_eq!(
            screen
                .count(&format!("id:revision-proposal-after >> text:{SCRIPT_LINE}"))
                .expect("the selector"),
            1,
            "the script reads as text: {:?}",
            screen.texts()
        );
        assert!(
            screen
                .layout_nodes()
                .iter()
                .all(|node| node.kind != nsh_core::ViewKind::WebView),
            "the field holds no web view"
        );
    }

    /// A credential store with nothing in it: no probe reaches the
    /// platform's secure store.
    struct EmptyStore;

    impl mt_core::credentials::SecureCredentialStore for EmptyStore {
        fn is_supported(&self) -> bool {
            false
        }
        fn read(
            &self,
            _: &str,
        ) -> Result<Option<mt_core::credentials::Secret>, mt_core::credentials::CredentialError>
        {
            Ok(None)
        }
        fn write(
            &self,
            _: &str,
            _: &mt_core::credentials::Secret,
        ) -> Result<(), mt_core::credentials::CredentialError> {
            Ok(())
        }
        fn delete(&self, _: &str) -> Result<(), mt_core::credentials::CredentialError> {
            Ok(())
        }
    }

    /// The file the save-conflict probe opens.
    static CONFLICT_DOCUMENT: std::sync::Mutex<Option<std::path::PathBuf>> =
        std::sync::Mutex::new(None);

    /// The source on disk the save-conflict probe opens.
    const CONFLICT_SOURCE: &str = "Original\nUnchanged\n";

    /// The field over a document opened from disk, with the title row's Save
    /// and the banner the column shows when a save is refused. The proposal
    /// arrives on `propose`, once the file has loaded; no Review runs.
    #[component]
    fn SaveConflictProbe() -> impl IntoElement {
        let path = CONFLICT_DOCUMENT
            .lock()
            .expect("the document path")
            .clone()
            .expect("the test set the document path");
        let state = AppState::with_vault(mt_core::credentials::CredentialVault::with_test_store(
            std::sync::Arc::new(EmptyStore),
        ));
        state.open_file(path);
        let propose_state = state.clone();
        let save_state = state.clone();
        let conflict_state = state.clone();
        let banner_state = state.clone();
        let tabs = state.tabs;
        view()
            .child(text("propose").on_press(move || {
                present(
                    &propose_state,
                    CONFLICT_SOURCE,
                    &[("Revise the word.", "Original", "Revised")],
                );
            }))
            .child(text("save").on_press(move || save_state.save_active()))
            .child(when(
                move || crate::ui::notices::has_conflict(&conflict_state),
                move || {
                    crate::ui::notices::conflict_banner(banner_state.clone(), Language::English)
                },
            ))
            .child(text(move || {
                let tabs = tabs.get();
                let tab = tabs.first();
                format!(
                    "Buffer: {:?} dirty={}",
                    tab.map(|tab| tab.text.get().to_string())
                        .unwrap_or_default(),
                    tab.is_some_and(|tab| tab.dirty.get())
                )
            }))
            .child(field(state, Language::English))
    }

    /// An accepted change, applied with `Done`, saves through the same
    /// conflict check as any edit: after another writer changes the file,
    /// Save is refused with the conflict banner and the disk keeps the other
    /// writer's bytes; the accepted text stays in the buffer, unsaved.
    #[test]
    fn an_applied_change_does_not_overwrite_a_file_changed_on_disk() {
        let dir = tempfile::tempdir().expect("a temp dir");
        let path = dir.path().join("plan.md");
        std::fs::write(&path, CONFLICT_SOURCE).expect("the file on disk");
        *CONFLICT_DOCUMENT.lock().expect("the document path") = Some(path.clone());

        let mut app = Headless::start(EngineConfig::new(Root::of::<SaveConflictProbe>()))
            .expect("start the probe");
        let opened = format!("Buffer: {CONFLICT_SOURCE:?} dirty=false");
        app.wait_for(&format!("text:{opened}"))
            .expect("the file is open");
        app.press("text:propose").expect("present the proposal");
        app.wait_for("id:revision-field").expect("the field");
        app.press_key("a").expect("accept the hunk");
        let applied = format!("Buffer: {:?} dirty=true", "Revised\nUnchanged\n");
        app.wait_for(&format!("text:{applied}"))
            .expect("the accepted hunk is in the buffer");
        app.press_key("ctrl+enter").expect("press Ctrl+Enter");
        app.wait_for("text:Approved changes applied to the buffer.")
            .expect("Done ran");
        assert_eq!(
            app.screen()
                .count("id:conflict-banner")
                .expect("the selector"),
            0,
            "no conflict before the other writer"
        );

        let external = "Changed elsewhere, at another length.\n";
        std::fs::write(&path, external).expect("the other writer's change");
        app.press("text:save").expect("save");
        app.wait_for("id:conflict-banner")
            .expect("the save is refused as a conflict");
        assert_eq!(
            std::fs::read_to_string(&path).expect("read the file"),
            external,
            "a refused save leaves the disk alone"
        );
        assert!(
            app.shows(&applied),
            "the accepted text stays, unsaved: {:?}",
            app.screen().texts()
        );
    }

    /// One untitled document holding `text`, the source a probe's proposal
    /// is read from.
    fn open_probe_document(state: &AppState, text: &str) {
        state.new_document();
        if let Some(tab) = state.active_tab() {
            tab.text.set(SharedString::from(text.to_owned()));
        }
    }
}
