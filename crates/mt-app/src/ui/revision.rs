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
//! extinguished when it is decided, which leaves it docked and plain until
//! the next undecided hunk takes its place); the other hunks are ember lines.
//! Both sides of the docked hunk are complete: a tall one scrolls in place,
//! so `Accept` never approves text that was hidden. A hunk whose expected
//! text no longer appears in the document says `Can't apply — this part
//! changed`; a proposal whose reviewed snapshot moved says `Stale — edited
//! after review` with `Review again`. Neither offers an `Accept`, and the
//! field always offers a way out (`Close`, and `Review again` once stale).
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
    Stale,
    ReviewAgain,
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
                S::Stale => "Stale — edited after review".to_owned(),
                S::ReviewAgain => "Review again".to_owned(),
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
                S::Stale => "已过期——审阅后文档有改动".to_owned(),
                S::ReviewAgain => "重新审阅".to_owned(),
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
/// confirmation after `Done` — holds the field until it is dismissed; a run
/// that starts after that (the status goes back to nothing while one is
/// prepared) opens it again, with the same outcome text or another.
pub fn is_open(state: &AppState) -> bool {
    if state.revision_busy.get() || state.revision_hunks.with(|hunks| !hunks.is_empty()) {
        return true;
    }
    let Some(status) = state.revision_status.get() else {
        // No outcome yet: the dismissed one is done with, so the next run's
        // outcome shows however its text reads.
        forget_dismissal();
        return false;
    };
    session().get().dismissed != Some(text_id(&status))
}

/// Forget which outcome was dismissed. Written from [`is_open`], which the
/// shell asks on every commit (its answer is a `when` condition), so a
/// dismissal never outlives the outcome it named.
fn forget_dismissal() {
    let Some(now) = try_global::<Session>() else {
        return;
    };
    now.update(|session| {
        if session.dismissed.is_some() {
            session.dismissed = None;
        }
    });
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
    let outcome_state = state.clone();
    let rows_state = state.clone();
    let decisions_state = state.clone();

    view()
        .test_id("revision-field")
        .w_full()
        .h(move || px(field_height()))
        .flex_col()
        .gap(px(SPACE_3))
        // The proposal's base moved: nothing here can apply any more.
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
                    .child(prims::ink(S::Stale.text(language)))
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
        move || hunk.with(|hunk| base_changed(&state, &hunk.before))
    };
    let accept_condition = {
        let state = state.clone();
        move || hunk.with(|hunk| state.revision_is_stale() || base_changed(&state, &hunk.before))
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
                            view()
                                .test_id("revision-proposal-after")
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
        Some(false) => tokens::palette().ink_faint,
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
                        state.apply_revision()
                    })
                })
                .otherwise(move || {
                    let state = quiet_state.clone();
                    prims::quiet_action("revision-done", S::Done.text(language), move || {
                        state.apply_revision()
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
/// `Ctrl+Z` takes the last decision back; `Ctrl+Enter` is `Done` — all while
/// no text input has the keyboard focus, so the document stays fully
/// editable.
fn bind_keys(state: &AppState, heat: Heat) {
    let keys = state.clone();
    on_key("a", move |_| decide(&keys, heat, true));
    let keys = state.clone();
    on_key("r", move |_| decide(&keys, heat, false));
    let keys = state.clone();
    on_key("ctrl+enter", move |_| keys.apply_revision());
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
    if accepted && (state.revision_is_stale() || base_changed(state, &hunks[index].before)) {
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

    /// A second proposal over the same document: same ordinals and change
    /// ids (the core numbers them per proposal from zero), other content.
    fn replacement_hunks() -> Vec<RevisionHunkView> {
        vec![
            hunk(0, 0, "Another take.", "compiles but", "compiles cleanly"),
            hunk(1, 1, "Second take.", "no check", "two checks"),
        ]
    }

    /// One hunk with more lines than the docked position shows at once.
    fn tall_hunk() -> RevisionHunkView {
        let before = (1..=20)
            .map(|line| format!("old line {line}"))
            .collect::<Vec<_>>()
            .join("\n");
        let after = (1..=20)
            .map(|line| format!("new line {line}"))
            .collect::<Vec<_>>()
            .join("\n");
        hunk(0, 0, "Rewrite the block.", &before, &after)
    }

    /// A proposal as large as the core allows a list of rows to be.
    fn many_hunks() -> Vec<RevisionHunkView> {
        (0..40)
            .map(|index| {
                hunk(
                    index,
                    index as u32,
                    &format!("Reason {index}."),
                    &format!("old {index}"),
                    &format!("new {index}"),
                )
            })
            .collect()
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

        // `Done` is reachable without the mouse: the key runs it. (The
        // proposal here is synthetic — no Review prepared one — so the state
        // refuses to apply, which is what proves the key reached `Done`.)
        app.press_key("ctrl+enter").expect("press Ctrl+Enter");
        app.wait_for("text:Run Review first to prepare a Revision.")
            .expect("Done ran from the keyboard");
    }

    /// A surface that routes on `is_open`, as the shell does.
    #[component]
    fn OutcomeProbe() -> impl IntoElement {
        let state = AppState::new();
        state.revision_status.set(Some(failure_text()));
        let show_state = state.clone();
        let clear_state = state.clone();
        let fail_state = state.clone();
        view()
            .child(when(
                move || is_open(&show_state),
                move || field(state.clone(), Language::English),
            ))
            .child(text("clear").on_press(move || clear_state.revision_status.set(None)))
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
        open_probe_document(&state);
        state.revision_hunks.set(two_hunks());
        state.revision_status.set(Some(SharedString::from(
            "Revision ready. Approve individual changes, then apply.",
        )));
        let swap_state = state.clone();
        view()
            .child(
                text("swap").on_press(move || swap_state.revision_hunks.set(replacement_hunks())),
            )
            .child(field(state, Language::English))
    }

    /// A proposal whose hunk is taller than the docked position shows at
    /// once.
    #[component]
    fn TallHunkProbe() -> impl IntoElement {
        let state = AppState::new();
        open_probe_document(&state);
        state.revision_hunks.set(vec![tall_hunk()]);
        state.revision_status.set(Some(SharedString::from(
            "Revision ready. Approve individual changes, then apply.",
        )));
        field(state, Language::English)
    }

    /// A proposal with more rows than the field can hold.
    #[component]
    fn ManyHunksProbe() -> impl IntoElement {
        let state = AppState::new();
        open_probe_document(&state);
        state.revision_hunks.set(many_hunks());
        state.revision_status.set(Some(SharedString::from(
            "Revision ready. Approve individual changes, then apply.",
        )));
        field(state, Language::English)
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

        // A run that starts after the dismissal (the status goes back to
        // nothing while one is prepared) opens the field again, even for the
        // very same outcome text: a failure is never silently hidden.
        app.press("text:clear").expect("a new run starts");
        app.press("text:fail-again").expect("it fails the same way");
        app.wait_for("text:Revision failed: Timed out after 60 s")
            .expect("the same failure shows again");
        assert!(
            app.screen()
                .matches("id:revision-dismiss")
                .expect("the selector"),
            "and can be dismissed again"
        );
    }

    #[test]
    fn a_second_proposal_starts_the_decisions_over() {
        let mut app =
            Headless::start(EngineConfig::new(Root::of::<SwapProbe>())).expect("start the field");
        app.wait_for("id:revision-field").expect("the field");
        app.press_key("a").expect("press A");
        app.wait_for("text:1 of 2 decided").expect("one decided");
        assert!(app.shows("Tighten the promise. · Accepted"));

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

    /// One untitled document holding the hunks' expected text, so the base
    /// checks answer as they would for a real proposal.
    fn open_probe_document(state: &AppState) {
        state.new_document();
        if let Some(tab) = state.active_tab() {
            tab.text.set(SharedString::from("compiles but no check\n"));
        }
    }
}
