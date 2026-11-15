//! Owner lane: revision.
//!
//! The field during an approved revision: the proposed hunks, one of them
//! docked in the reading position with its old text struck cold and its new
//! text a hot proposal, the others as single ember lines, and the decision
//! row — `Accept` / `Reject` on the current hunk (`A` / `R`), one pip per hunk
//! reading `n of m decided`, `Undo` (`Ctrl+Z`) and `Done`.
//!
//! Contract: [`is_open`] says the field belongs to the revision (it outranks
//! the review field, because a revision runs inside a review); [`field`] draws
//! it. The other half — the removed words struck cold in the column, the
//! accepted words settling in as ambient ink — belongs to whatever draws the
//! column's decorations; [`decision`] is the seam it reads, because the
//! state's per-change signal alone cannot tell a rejected change from one
//! still to be judged.
//!
//! Old and new are always on screen together — no modifier key, no hover — so
//! the hunk is judged without leaving the field. The current proposal is this
//! state's one HOT item ([`Heat`] pinned at 1.0 rather than cooling, then
//! extinguished when it is decided); decided hunks and the rest cool to ember
//! or plain ink. A hunk whose expected text no longer appears in the document
//! says `Can't apply — this part changed` and offers no `Accept`.
//!
//! Decisions live here, in a module-local session keyed by the proposal
//! ([`Session`]), and are mirrored into the state's per-change signals through
//! [`AppState::toggle_revision_hunk`]: a change the user accepted is exactly a
//! change `apply_revision` composes. `Done` performs the state's one validated
//! write for every accepted change and discards the rest — undecided hunks are
//! never written. Nothing in this file mutates the document.

use std::{
    collections::hash_map::DefaultHasher,
    hash::{Hash, Hasher},
    time::Duration,
};

use mt_core::settings::Language;
use nsh_core::prelude::*;

use crate::{
    i18n::{self, Key},
    state::{AppState, RevisionHunkView},
    ui::{
        motion::{self, Heat},
        prims,
        tokens::{self, SPACE_1, SPACE_2, SPACE_3},
    },
};

/// The most lines one side of a hunk shows in the docked position; a longer
/// one ends in an ellipsis line, so the docked proposal stays a proposal.
const HUNK_LINES: usize = 12;
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
    Move,
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
                S::Move => "Move".to_owned(),
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
                S::Move => "切换".to_owned(),
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
/// belong to, which hunk holds the reading position, and which outcome the
/// user already dismissed. A proposal is its hunks' content, so a new or
/// re-rolled one never inherits a decision.
#[derive(Clone, Debug, Default, PartialEq)]
struct Session {
    proposal: u64,
    current: usize,
    decisions: Vec<Decision>,
    dismissed: Option<u64>,
}

impl Session {
    /// This session, made for `hunks`: a reset one when they are not the
    /// proposal it belongs to. Dismissals survive (they name one outcome).
    fn synced(&self, hunks: &[RevisionHunkView]) -> Session {
        if self.proposal == proposal_id(hunks) {
            self.clone()
        } else {
            Session {
                proposal: proposal_id(hunks),
                dismissed: self.dismissed,
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

    /// The index of the hunk in the reading position, while it is one whose
    /// change is still to be judged.
    fn docked(&self, hunks: &[RevisionHunkView]) -> Option<usize> {
        let index = self.current(hunks)?;
        (!self.decided(hunks, index)).then_some(index)
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

/// The identity of one outcome text.
fn text_id(text: &str) -> u64 {
    let mut hasher = DefaultHasher::new();
    text.hash(&mut hasher);
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

// ─── the surface ──────────────────────────────────────────────────────────

/// Whether the field is showing a revision.
///
/// An outcome with no hunks on screen — a failure before any proposal, or the
/// confirmation after `Done` — holds the field until it is dismissed; a new
/// outcome opens it again.
pub fn is_open(state: &AppState) -> bool {
    if state.revision_busy.get() || state.revision_hunks.with(|hunks| !hunks.is_empty()) {
        return true;
    }
    let Some(status) = state.revision_status.get() else {
        return false;
    };
    session().get().dismissed != Some(text_id(&status))
}

/// The revision field.
pub fn field(state: AppState, language: Language) -> impl IntoElement {
    let hunks = state.revision_hunks;
    let status = state.revision_status;
    let busy = state.revision_busy;
    let now = session();
    let heat = Heat::cold();

    bind_keys(&state, heat);
    watch_heat(hunks, now, heat);

    let outcome_state = state.clone();
    let rows_state = state.clone();
    let decisions_state = state.clone();

    view()
        .test_id("revision-field")
        .w_full()
        .h(move || px(field_height()))
        .flex_col()
        .gap(px(SPACE_3))
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
        // The hunks, the current one docked in the reading position.
        .child(when(
            move || hunks.with(|hunks| !hunks.is_empty()),
            move || rows(rows_state.clone(), language, hunks, now, heat),
        ))
        // Pips, the count, Undo and Done.
        .child(when(
            move || hunks.with(|hunks| !hunks.is_empty()),
            move || decisions(decisions_state.clone(), language, hunks, now),
        ))
        .child(when(
            move || hunks.with(|hunks| !hunks.is_empty()),
            move || key_hints(language),
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
    view()
        .flex_row()
        .items_center()
        .gap(px(SPACE_3))
        .child(prims::rest(move || {
            status_state.revision_status.get().unwrap_or_default()
        }))
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

/// The hunk rows, vertically centred: the docked proposal in the reading
/// position, one ember line per other hunk above and below it.
fn rows(
    state: AppState,
    language: Language,
    hunks: Signal<Vec<RevisionHunkView>>,
    now: Signal<Session>,
    heat: Heat,
) -> impl IntoElement {
    let list_state = state.clone();
    view()
        .test_id("revision-rows")
        .flex_1()
        .w_full()
        .flex_col()
        .justify_center()
        .child(view().w_full().flex_col().gap(px(SPACE_2)).child(each(
            move || hunks.get(),
            |hunk: &RevisionHunkView| hunk.ordinal,
            move |hunk: ReadSignal<RevisionHunkView>, index: ReadSignal<usize>| {
                // A proposal is never replaced while its hunks are on
                // screen (a new one clears them first and the field
                // remounts), so the snapshot a row is built with stays
                // the hunk it draws.
                let snapshot = hunk.get();
                let docked = move || {
                    hunks.with(|hunks| now.with(|now| now.docked(hunks))) == Some(index.get())
                };
                let docked_state = list_state.clone();
                let docked_hunk = snapshot.clone();
                let line_hunk = snapshot;
                view().w_full().child(
                    when(docked, move || {
                        docked_proposal(docked_state.clone(), language, docked_hunk.clone(), heat)
                    })
                    .otherwise(move || one_line(language, line_hunk.clone(), now)),
                )
            },
        )))
}

/// The hunk in the reading position: the model's reason, the old text struck
/// cold, the new text as the hot proposal, and the decision on it.
fn docked_proposal(
    state: AppState,
    language: Language,
    hunk: RevisionHunkView,
    heat: Heat,
) -> impl IntoElement {
    let reason = hunk.rationale.clone();
    let before = hunk_lines(&hunk.before, HUNK_LINES);
    let after = hunk_lines(&hunk.after, HUNK_LINES);
    let message_condition = {
        let state = state.clone();
        let before = hunk.before.clone();
        move || base_changed(&state, &before)
    };
    let accept_condition = {
        let state = state.clone();
        let before = hunk.before.clone();
        move || base_changed(&state, &before)
    };
    let accept_state = state.clone();
    let reject_state = state;
    view()
        .test_id("revision-proposal")
        .relative()
        .w_full()
        .flex_col()
        .gap(px(SPACE_3))
        .child(prims::halo(heat))
        .child(prims::rest(reason))
        .child(when(!before.is_empty(), move || {
            let lines = before.clone();
            view()
                .test_id("revision-proposal-before")
                .w_full()
                .flex_col()
                .children(lines.into_iter().map(|line| {
                    text(line)
                        .font_family(tokens::FONT_SOURCE)
                        .text_size(px(tokens::TYPE_SOURCE))
                        .line_height(px(tokens::TYPE_SOURCE_LINE))
                        .text_color(move || tokens::palette().removed)
                        .line_through()
                        .underline()
                        .text_decoration_color(move || tokens::palette().removed)
                        .text_decoration_thickness(px(tokens::HAIRLINE))
                }))
        }))
        .child(when(!after.is_empty(), move || {
            let lines = after.clone();
            view()
                .test_id("revision-proposal-after")
                .w_full()
                .flex_col()
                .child(
                    view()
                        .w_full()
                        .flex_col()
                        .bg(move || tokens::palette().added_plate)
                        .children(lines.into_iter().map(|line| {
                            text(line)
                                .font_family(tokens::FONT_SOURCE)
                                .text_size(px(tokens::TYPE_SOURCE))
                                .line_height(px(tokens::TYPE_SOURCE_LINE))
                                .text_color(move || tokens::palette().added)
                        })),
                )
                .child(prims::bloom(heat, 6.0))
        }))
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
/// rejected.
fn one_line(language: Language, hunk: RevisionHunkView, now: Signal<Session>) -> impl IntoElement {
    let ordinal = hunk.ordinal;
    let change_id = hunk.change_id;
    let word = format!("{}", ordinal + 1);
    let digest_line = digest(&hunk.rationale, DIGEST_CHARS);
    let digest_text = move || {
        let decided = now.with(|now| now.decision(change_id));
        match decided {
            None => digest_line.clone(),
            Some(true) => format!("{digest_line} · {}", S::Accepted.text(language)),
            Some(false) => format!("{digest_line} · {}", S::Rejected.text(language)),
        }
    };
    let colour = move || match now.with(|now| now.decision(change_id)) {
        None => tokens::palette().ember,
        Some(true) => tokens::palette().ink_rest,
        Some(false) => tokens::palette().ink_faint,
    };
    view()
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
                    .text_color(move || tokens::palette().ink_faint),
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

/// Pips (one per hunk, lit when its change is decided), the count, `Undo`,
/// the not-applied hint, and `Done`.
fn decisions(
    state: AppState,
    language: Language,
    hunks: Signal<Vec<RevisionHunkView>>,
    now: Signal<Session>,
) -> impl IntoElement {
    let undo_state = state.clone();
    let done_state = state.clone();
    view()
        .test_id("revision-decisions")
        .flex_row()
        .items_center()
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
        ))
        .child(view().flex_1())
        .child(prims::primary_action(
            "revision-done",
            S::Done.text(language),
            move || done_state.apply_revision(),
        ))
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
/// `Ctrl+Z` takes the last decision back — while no text input has the
/// keyboard focus, so the document stays fully editable.
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
    // A hunk whose base changed offers no Accept; the state's snapshot check
    // at `Done` is the second gate.
    if accepted && base_changed(state, &hunks[index].before) {
        return;
    }
    let change_id = hunks[index].change_id;
    if next.decision(change_id) == Some(accepted) {
        // The same decision again: nothing to record; step on.
        next.advance(&hunks, index);
        signal.set(next);
        return;
    }
    next.record(&hunks, index, accepted);
    let proposal = next.proposal;
    signal.set(next);
    set_change_accepted(state, change_id, accepted);
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

/// Take the last decision back, and its change's signal with it.
fn undo_decision(state: &AppState) {
    let hunks = state.revision_hunks.get();
    let signal = session();
    let mut restored = None;
    signal.update(|now| {
        let mut synced = now.synced(&hunks);
        let Some(change_id) = synced.undo() else {
            return;
        };
        restored = Some((change_id, synced.decision(change_id).unwrap_or(false)));
        *now = synced;
    });
    if let Some((change_id, accepted)) = restored {
        set_change_accepted(state, change_id, accepted);
    }
}

/// Dismiss the outcome text on screen; a new one opens the field again.
fn dismiss(state: &AppState) {
    let Some(status) = state.revision_status.get() else {
        return;
    };
    session().update(|now| now.dismissed = Some(text_id(&status)));
}

/// Make the state's signal for `change_id` say `accepted`: every hunk of one
/// change carries the same signal, so one toggle sets them all.
fn set_change_accepted(state: &AppState, change_id: u32, accepted: bool) {
    let hunks = state.revision_hunks.get();
    if let Some(index) = hunks
        .iter()
        .position(|hunk| hunk.change_id == change_id && hunk.accepted.get() != accepted)
    {
        state.toggle_revision_hunk(index);
    }
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

/// Keeps the heat on what the field is asking to be judged: ignite when a new
/// hunk is docked (`J` / `K`, or the advance after a decision), extinguish
/// when nothing is docked any more.
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
        let docked = hunks.with(|hunks| now.with(|now| now.docked(hunks).is_some()));
        let previous = last.replace(key);
        if !docked {
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

/// Whether the part of the document the hunk expects to replace is no longer
/// there: the proposal was validated against the reviewed bytes, and a hunk
/// whose base moved cannot apply. (The whole proposal is one snapshot; the
/// state refuses a stale one at `Done` and says why.)
fn base_changed(state: &AppState, before: &str) -> bool {
    match state.active_tab() {
        Some(tab) => tab.text.with(|text| base_moved(text, before)),
        None => true,
    }
}

/// [`base_changed`] against a text: an insertion (nothing to replace) can
/// always apply.
fn base_moved(text: &str, before: &str) -> bool {
    !before.is_empty() && !text.contains(before)
}

// ─── text helpers ─────────────────────────────────────────────────────────

/// The lines of one side of a hunk, at most `max` of them; a longer side ends
/// in an ellipsis line. Nothing to show (an insertion has no old text) is an
/// empty vector.
fn hunk_lines(text: &str, max: usize) -> Vec<SharedString> {
    if text.is_empty() {
        return Vec::new();
    }
    let mut lines: Vec<SharedString> = text
        .lines()
        .take(max)
        .map(|line| SharedString::from(line.to_owned()))
        .collect();
    if text.lines().count() > max {
        lines.push(SharedString::from("…"));
    }
    lines
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
            None,
            "a decided hunk is not the docked proposal"
        );
        session.advance(&hunks, 0);
        assert_eq!(session.docked(&hunks), Some(1));
        assert_eq!(session.current, 1);

        session.record(&hunks, 1, false);
        session.advance(&hunks, 1);
        assert_eq!(session.counts(&hunks), (2, 2));
        assert_eq!(session.docked(&hunks), None);

        assert_eq!(session.undo(), Some(1));
        assert_eq!(session.current, 1, "undo restores the reading position");
        assert_eq!(session.docked(&hunks), Some(1));

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

    #[test]
    fn a_changed_base_refuses_the_hunk() {
        assert!(base_moved("the new text", "compiles but"));
        assert!(!base_moved("compiles but it does", "compiles but"));
        assert!(
            !base_moved("anything", ""),
            "an insertion has no base to match"
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
    fn hunk_lines_elide_past_the_cap() {
        assert!(hunk_lines("", 12).is_empty());
        assert_eq!(hunk_lines("one\ntwo", 12).len(), 2);
        let many = (0..20)
            .map(|i| i.to_string())
            .collect::<Vec<_>>()
            .join("\n");
        let lines = hunk_lines(&many, 12);
        assert_eq!(lines.len(), 13);
        assert_eq!(lines.last().unwrap().as_str(), "…");
    }

    /// A surface with a synthetic proposal: the field alone, as `app.rs`
    /// mounts it through the shell.
    #[component]
    fn ProposalProbe() -> impl IntoElement {
        let state = AppState::new();
        open_probe_document(&state);
        state.revision_hunks.set(two_hunks());
        state.revision_status.set(Some(SharedString::from(
            "Revision ready. Approve individual changes, then apply.",
        )));
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
            "the proposal is the same appearance's added colour"
        );

        app.press_key("a").expect("press A");
        app.wait_for("text:1 of 2 decided")
            .expect("the decision is recorded");
        assert!(
            app.shows("no check"),
            "the next undecided hunk is docked: {:?}",
            app.screen().texts()
        );
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
        assert!(
            !app.screen()
                .matches("id:revision-accept")
                .expect("the selector"),
            "no proposal is left to accept: {:?}",
            app.screen().texts()
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
    }

    /// A surface that routes on `is_open`, as the shell does.
    #[component]
    fn OutcomeProbe() -> impl IntoElement {
        let state = AppState::new();
        state.revision_status.set(Some(SharedString::from(
            "Revision failed: Timed out after 60 s",
        )));
        let show_state = state.clone();
        view()
            .child(when(
                move || is_open(&show_state),
                move || field(state.clone(), Language::English),
            ))
            .child(text("probe-tail"))
    }

    /// The whole room, with a synthetic proposal: the field box as `app.rs`
    /// mounts it.
    #[component]
    fn ShellProbe() -> impl IntoElement {
        let state = AppState::new();
        open_probe_document(&state);
        state.revision_hunks.set(two_hunks());
        state.revision_status.set(Some(SharedString::from(
            "Revision ready. Approve individual changes, then apply.",
        )));
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
    fn an_outcome_without_hunks_holds_the_field_until_dismissed() {
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
    }

    /// One untitled document holding the hunks' expected text, so the base
    /// checks answer as they would for a real proposal.
    fn open_probe_document(state: &AppState) {
        state.new_document();
        if let Some(tab) = state.active_tab() {
            tab.text.set(SharedString::from("compiles but no check\n"));
        }
    }
}
