//! Owner lane: review.
//!
//! The field during review: the lens line, 'Understood: <goal>', the questions
//! (the current one HOT with its `Why it matters:` and its answer field), the
//! cooled question list, `All fields`, loading, stale and the diagnostics.
//!
//! Contract: [`is_open`] says whether the field is the review's; [`field`]
//! draws it. The shell routes to it and gives it the field's box; everything
//! inside — the docked reading position, the ignition order, the stagger, the
//! focus on the answer field — belongs here.
//!
//! The session this file keeps ([`session`], [`emitters`]) is UI state the
//! state has no name for: which question the user is on, which they answered
//! or skipped, which finding they stepped to with `J`/`K`, whether they
//! cancelled the wait, and the [`Heat`] of every model item, so a rebuild of
//! this surface (a keystroke in an answer) never re-ignites a cooled line.
//! [`glow`] draws the other half of every finding in the column; the field
//! tells it which finding is current, and neither surface can invent an
//! anchor.

use std::time::Duration;

use mt_core::{
    model::{EndpointIdentity, Provider},
    review::{ArtifactLens, ClarificationPriority, provider::REVIEW_REQUEST_TIMEOUT},
    settings::Language,
};
use nsh_base::{button, input};
use nsh_core::{Color, EllipsizeMode, element::KeyOptions, keymap::When, prelude::*};

use crate::{
    i18n::{self, Key},
    state::{
        AppState, ReviewFailure, ReviewFindingView, ReviewQuestionView, ReviewUnavailable,
        ReviewView, Tab,
    },
    ui::{
        glow,
        motion::{Heat, IGNITE_MS, REDUCED_CROSSFADE_MS, reduced_motion, stagger},
        prims,
        tokens::{
            self, FOCUS_RING, FOCUS_RING_OFFSET, SPACE_1, SPACE_2, SPACE_3, SPACE_4, SPACE_5,
            SPACE_6, TYPE_MODEL, TYPE_MODEL_LINE, TYPE_UI, TYPE_UI_LINE, WEIGHT_MODEL, with_alpha,
        },
    },
};

/// Whether the field is showing a review, or explaining why it cannot.
pub fn is_open(state: &AppState) -> bool {
    state.review_open.get() || state.review_unavailable.get().is_some()
}

/// The review field.
pub fn field(state: AppState, language: Language) -> impl IntoElement {
    let session = session();
    observe(&state, session);
    loading_hint(&state, session);
    keys(&state, session);
    let lens_state = state.clone();
    let body_state = state;
    view()
        .test_id("review-field")
        .w_full()
        .h(move || px(field_height()))
        .flex_col()
        .flex_shrink_0()
        .pt(px(SPACE_5))
        .child(
            view()
                .test_id("review-lens")
                .flex_shrink_0()
                .child(lens_line(lens_state, language)),
        )
        .child(body(body_state, language, session))
}

/// The field's height: the window's own, less the title row. The field's box
/// in the shell sizes to its content, so the docked position and the footer
/// are placed against the window instead (the prototype lays out against the
/// same number).
fn field_height() -> f32 {
    let title = tokens::TITLE_ROW_H.max(prims::caption_insets().top.0);
    (window().height.0 - title).max(320.0)
}

// ─── the session ─────────────────────────────────────────────────────────

/// What the field remembers between rebuilds. One per window.
#[derive(Clone, Default, PartialEq)]
struct Session {
    /// The result it belongs to; a new one resets the rest.
    view: Option<ReviewView>,
    /// The question ordinals the user answered, and the ones they skipped.
    answered: Vec<usize>,
    skipped: Vec<usize>,
    /// The finding `J`/`K` stepped to.
    focus: Option<usize>,
    /// Whether the docked position is showing that finding rather than the
    /// question to answer.
    showing_finding: bool,
    /// Whether the user expanded `All fields`.
    all_fields: bool,
    /// Whether the user cancelled this run's wait.
    cancelled: bool,
    /// Whether the run has been waiting long enough for the seconds hint.
    slow: bool,
}

/// What takes the keyboard focus once, when its state is entered: the field
/// asks for it here rather than through `autoFocus`, which would take the
/// focus back on every rebuild — and steal it from `J`/`K`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Focus {
    /// The answer field of question `index`.
    Answer(usize),
    /// `Prepare revision`, once every question is answered or skipped.
    Prepare,
    /// `Review again`, on a result that is already stale.
    ReviewAgain,
}

fn focus_request() -> Signal<Option<Focus>> {
    global::<Option<Focus>>()
}

/// A one-shot focus, kept alive until it fires: an effect runs before the
/// commit that carries the control is in, so the ref reaches it only at the
/// next step of the executor.
fn focus_step() -> Signal<Option<Spawned<()>>> {
    global::<Option<Spawned<()>>>()
}

fn focus_soon(reference: ElementRef) {
    let step = set_timeout(Duration::from_millis(0), move || reference.focus());
    focus_step().set(Some(step));
}

fn session() -> Signal<Session> {
    global::<Session>()
}

/// Every model item's heat: the goal, the questions (in the state's order)
/// and the findings. Built once per result, so a rebuild binds the same
/// emitters and an animation never restarts.
#[derive(Clone, Default)]
struct Emitters {
    goal: Heat,
    questions: Vec<Heat>,
    findings: Vec<Heat>,
}

fn emitters() -> Signal<Emitters> {
    global::<Emitters>()
}

/// The step that will show the seconds hint, kept alive until it fires.
fn hint_step() -> Signal<Option<Spawned<()>>> {
    global::<Option<Spawned<()>>>()
}

/// Watches the state's result: a new one resets the session, builds the
/// emitters, ignites them in reading order and hands the column its anchors.
fn observe(state: &AppState, session: Signal<Session>) {
    let state = state.clone();
    effect(move || {
        let view = state.review_view.get();
        // The review belongs to the tab it was made from, whatever tab the
        // user is on when it arrives: its anchors and its `lines N–M` are
        // that document's, and another document's bytes never receive them.
        // The tab list is read untracked — only the result re-runs this.
        let reviewed = view.as_ref().and_then(|view| reviewed_tab(&state, view));
        glow::observe(view.as_ref(), reviewed.as_ref());
        let known = session.get_untracked();
        let arrival = match (&known.view, &view) {
            (Some(known), Some(view)) => !known.same_result(view),
            (None, None) => return,
            _ => true,
        };
        if !arrival {
            return;
        }
        let next = Session {
            view: view.clone(),
            ..Session::default()
        };
        let emitters = emitters();
        match &view {
            Some(view) => {
                let order = priority_order(&view.questions_detail);
                let current = current_question(&next, &order);
                // Under a detached owner: an emitter is a handle held by the
                // owner it is made under, and this effect's owner dies with
                // every re-run of it and with every rebuild of the field. A
                // handle whose owner is gone does nothing, so the heats would
                // stop moving — the field's own lines and the column's
                // docked position alike.
                let built = Owner::detached().run(|| Emitters {
                    goal: Heat::cold(),
                    questions: order.iter().map(|_| Heat::cold()).collect(),
                    findings: view
                        .findings_anchored
                        .iter()
                        .map(|_| Heat::cold())
                        .collect(),
                });
                emitters.set(built.clone());
                // The reading order: the goal, then the question to answer —
                // the one that stays HOT — then the rest of the questions.
                // The column's findings ignite on the same clock, and exactly
                // one thing is HOT by the time the sequence is done.
                built.goal.ignite(stagger(0));
                for (slot, index) in order.iter().enumerate() {
                    let Some(heat) = built.questions.get(slot).copied() else {
                        continue;
                    };
                    if Some(*index) == current {
                        ignite_hold(heat, stagger(1));
                    } else {
                        heat.ignite(stagger(2 + slot));
                    }
                }
                // Nothing is focused yet: the question is the one HOT item
                // until `J`/`K` (or the stepper) steps to a finding.
                glow::focus_finding(None);
                // The first finding ignites with the first question (their
                // simultaneous ignition is the link): the column moves so the
                // anchored lines sit level with the docked question.
                level_finding(&state, view.tab_id, 0);
                focus_request().set(Some(match (view.stale, current) {
                    (true, _) => Focus::ReviewAgain,
                    (false, Some(index)) => Focus::Answer(index),
                    (false, None) => Focus::Prepare,
                }));
            }
            None => {
                emitters.set(Emitters::default());
                focus_request().set(None);
            }
        }
        session.set(next);
    });
}

/// The seconds hint: after a full second of waiting, one resting line.
fn loading_hint(state: &AppState, session: Signal<Session>) {
    let busy = state.review_busy;
    let step = hint_step();
    effect(move || {
        if busy.get() {
            session.update(|session| session.slow = false);
            step.set(Some(set_timeout(Duration::from_secs(1), move || {
                if busy.get_untracked() {
                    session.update(|session| session.slow = true);
                }
            })));
        } else {
            step.set(None);
            session.update(|session| session.slow = false);
        }
    });
}

/// `Esc` cancels the wait; `J`/`K` step the current finding. Both bind while
/// this field's owner lives (the shell keeps it only while the review is the
/// field's surface). `J`/`K` fire only while no text input edits, so typing
/// an answer never steps a finding.
fn keys(state: &AppState, session: Signal<Session>) {
    // `Esc` fires wherever the focus is: the keymap counts it as one an
    // editor uses, and the spec's 'Esc cancels' must reach this handler
    // while the column's editor has the focus.
    on_key_with(
        "escape",
        KeyOptions {
            when: When::Always,
            ..KeyOptions::default()
        },
        {
            let state = state.clone();
            move |_| {
                if state.review_busy.get_untracked() {
                    session.update(|session| session.cancelled = true);
                    state.cancel_review_in_flight();
                } else if session.get_untracked().view.is_none()
                    && state.review_consent.get_untracked().is_none()
                {
                    state.close_review();
                }
            }
        },
    );
    for (key, delta) in [("j", 1i32), ("k", -1i32)] {
        on_key_with(
            key,
            KeyOptions {
                when: When::NotEditing,
                ..KeyOptions::default()
            },
            {
                let state = state.clone();
                move |_| step_finding(&state, session, delta)
            },
        );
    }
}

/// `J`/`K`: the next finding's words go HOT, the previous one cools, and its
/// explanation takes the docked position.
fn step_finding(state: &AppState, session: Signal<Session>, delta: i32) {
    let Some(view) = state.review_view.get_untracked() else {
        return;
    };
    if view.stale {
        return;
    }
    let count = view.findings_anchored.len() as i32;
    if count == 0 {
        return;
    }
    let current = session.get_untracked();
    let last = count as usize - 1;
    // From nothing focused, a step lands on the first (or, backwards, the
    // last) finding: it must not step past the one it meant.
    let (from, to) = match current.focus.map(|from| from.min(last)) {
        Some(from) => (
            Some(from),
            (from as i32 + delta).clamp(0, last as i32) as usize,
        ),
        None => (None, if delta >= 0 { 0 } else { last }),
    };
    session.update(|session| {
        session.focus = Some(to);
        session.showing_finding = true;
    });
    glow::focus_finding(Some(to));
    // The column re-centres so the finding's anchored lines sit level with
    // its explanation, docked in the field (document.rs reveals the line).
    level_finding(state, view.tab_id, to);
    let heats = emitters().get_untracked().findings;
    if let Some(from) = from
        && from != to
        && let Some(heat) = heats.get(from).copied()
    {
        heat.cool();
    }
    if let Some(heat) = heats.get(to).copied() {
        ignite_hold(heat, 0.0);
    }
}

/// Asks the column's camera for the finding the field just made HOT: the
/// source editor levels the anchored line with the docked explanation
/// ([`AppState::focus_document_line`]). Nothing is asked when the finding is
/// not anchored in this document (document-wide, or another file's).
fn level_finding(state: &AppState, tab_id: Option<u64>, finding: usize) {
    if let Some(tab_id) = tab_id
        && let Some(line) = glow::anchor_line(finding)
    {
        state.focus_document_line(tab_id, line);
    }
}

/// Ignites `heat` and holds it HOT: a plain [`Heat::ignite`] would cool to
/// ember, and the one current item must stay hot.
fn ignite_hold(heat: Heat, delay_ms: f32) {
    let value = heat.value();
    value.stop();
    if reduced_motion() {
        let timing = Timing::new(REDUCED_CROSSFADE_MS, Easing::EaseInOut);
        if delay_ms > 0.0 {
            value.timing_with(1.0, Timing { delay_ms, ..timing });
        } else {
            value.timing_with(1.0, timing);
        }
        return;
    }
    let mut timing = Timing::new(IGNITE_MS, Easing::Linear);
    timing.delay_ms = delay_ms;
    value.timing_with(1.0, timing);
}

// ─── the body, by state ──────────────────────────────────────────────────

fn body(state: AppState, language: Language, session: Signal<Session>) -> impl IntoElement {
    view()
        .flex_col()
        .flex_1()
        .min_h(px(0.0))
        .w_full()
        // The body is a region of its own: the field's states are taken
        // again whenever the result, the session or the emitters change, and
        // each element rebinds to the heat that belongs to it (the session
        // keeps the states' one next action honest).
        .child(dynamic(move || {
            if let Some(view) = state.review_view.get() {
                return result(state.clone(), language, session, view).into_element();
            }
            if state.review_busy.get() {
                return loading_line(state.clone(), language).into_element();
            }
            if state.review_consent.get().is_some() {
                return view()
                    .pt(px(SPACE_6))
                    .child(prims::rest(i18n::t(language, Key::ReviewWaiting)))
                    .into_element();
            }
            if let Some(reason) = state.review_unavailable.get() {
                return unavailable(state.clone(), language, reason).into_element();
            }
            if session.get().cancelled {
                return cancelled(state.clone(), language).into_element();
            }
            view().into_element()
        }))
}

/// The result: the intent, the docked position, the questions and the
/// footer.
fn result(
    state: AppState,
    language: Language,
    session: Signal<Session>,
    review: ReviewView,
) -> AnyElement {
    let empty = review.sections.is_empty()
        && review.findings_anchored.is_empty()
        && review.questions_detail.is_empty();
    if empty && (review.failure.is_some() || !review.status.is_empty()) {
        // A result that carries no review at all: say why, calmly, and offer
        // the one way forward. Nothing here is a simulated result.
        return failure_block(state, language, &review).into_element();
    }
    let emitters = emitters().get();
    let order = priority_order(&review.questions_detail);
    let stale = review.stale;
    let mut column = view().flex_col().flex_1().min_h(px(0.0)).w_full();
    if stale {
        column = column.child(stale_banner(state.clone(), language));
    }
    column
        .child(intent(language, session, &review, &emitters, stale))
        .child(spacer())
        .child(docked(
            state.clone(),
            language,
            session,
            &review,
            &emitters,
            stale,
        ))
        .child(questions(
            state.clone(),
            language,
            session,
            &review,
            &order,
            &emitters,
            stale,
        ))
        .child(spacer())
        .child(footer(state, language, session, &review))
        .into_element()
}

/// Why the review failed, with `Try again`: the spec's plain diagnostics,
/// and the localized status with its detail for anything else.
fn failure_block(state: AppState, language: Language, review: &ReviewView) -> impl IntoElement {
    let again = state.clone();
    let (headline, detail, action) = match review.failure {
        Some(ReviewFailure::Timeout) => (timed_out(language), None, s(S::TryAgain, language)),
        Some(ReviewFailure::Malformed) => (
            s(S::CouldNotRead, language).to_owned(),
            None,
            s(S::TryAgain, language),
        ),
        Some(ReviewFailure::Cancelled) => (
            s(S::Cancelled, language).to_owned(),
            None,
            s(S::ReviewAgain, language),
        ),
        _ => (
            review.status.clone(),
            review.diagnostic.clone(),
            s(S::TryAgain, language),
        ),
    };
    let mut block = view()
        .test_id("review-failure")
        .pt(px(SPACE_6))
        .w_full()
        .flex_col()
        .gap(px(SPACE_3))
        .child(prims::ink(headline));
    if let Some(detail) = detail {
        block = block.child(prims::faint(detail));
    }
    block.child(prims::primary_action("review-again", action, move || {
        again.start_review()
    }))
}

/// A flex spacer that collapses when the content needs the room.
fn spacer() -> impl IntoElement {
    view().flex_1().min_h(px(SPACE_2))
}

/// 'Stale — edited after review' with `Review again`.
fn stale_banner(state: AppState, language: Language) -> impl IntoElement {
    let again = state.clone();
    // 'Review again' takes the focus when the result goes stale: it is the
    // state's one next action.
    let reference = element_ref();
    let focus_reference = reference;
    effect(move || {
        if focus_request().get() == Some(Focus::ReviewAgain) {
            focus_request().set(None);
            focus_soon(focus_reference);
        }
    });
    view()
        .test_id("review-stale")
        .w_full()
        .flex_col()
        .gap(px(SPACE_2))
        .pt(px(SPACE_3))
        .child(
            view()
                .flex_row()
                .items_center()
                .justify_between()
                .child(prims::ink(s(S::Stale, language)))
                .child(
                    prims::primary_action("review-again", s(S::ReviewAgain, language), move || {
                        again.start_review()
                    })
                    .element_ref(&reference),
                ),
        )
}

/// `Lens: <kind>`.
fn lens_line(state: AppState, language: Language) -> impl IntoElement {
    view()
        .flex_row()
        .items_center()
        .gap(px(SPACE_1))
        .child(prims::rest(format!("{}:", s(S::Lens, language))))
        .child(prims::ink(move || {
            // The reviewed tab's, not whichever tab is open now: the lens is
            // what the review was made of.
            let lens = state
                .review_view
                .get_untracked()
                .and_then(|view| reviewed_tab(&state, &view))
                .map(|tab| ArtifactLens::infer_from_path(&tab.path))
                .unwrap_or_else(ArtifactLens::default_lens);
            SharedString::from(lens_label(lens, language))
        }))
}

/// The tab a review belongs to, while it is open: the one it was made from,
/// whatever tab the user is on. Untracked — a caller in a render pass must
/// not rebuild with every keystroke in another document.
fn reviewed_tab(state: &AppState, review: &ReviewView) -> Option<Tab> {
    let id = review.tab_id?;
    state
        .tabs
        .get_untracked()
        .iter()
        .find(|tab| tab.id == id)
        .cloned()
}

/// The understood intent: the goal in the model's voice, then what the model
/// understood beyond it behind `All fields`.
fn intent(
    language: Language,
    session: Signal<Session>,
    review: &ReviewView,
    emitters: &Emitters,
    stale: bool,
) -> impl IntoElement {
    let goal = review
        .sections
        .first()
        .and_then(|section| section.items.first())
        .cloned()
        .unwrap_or_default();
    let sections: Vec<(String, Vec<String>)> = review
        .sections
        .iter()
        .skip(1)
        .map(|section| (section.title.clone(), section.items.clone()))
        .collect();
    let toggle = session;
    let mut block = view()
        .w_full()
        .flex_col()
        .gap(px(SPACE_2))
        .pt(px(SPACE_4))
        .child(prims::rest(s(S::Understood, language)))
        .child(voice(goal, emitters.goal, language, drained(stale)));
    if !sections.is_empty() {
        let mut disclosure = view()
            .flex_col()
            // The control hugs its label: a quiet action is a word, not a
            // band across the field.
            .items_start()
            .gap(px(SPACE_2))
            .child(prims::quiet_action(
                "review-all-fields",
                s(S::AllFields, language),
                move || {
                    toggle.update(|session| session.all_fields = !session.all_fields);
                },
            ));
        if session.get().all_fields {
            disclosure = disclosure.child(
                // The fields scroll inside their own room: the question to
                // answer is the field's one next action, and it must stay in
                // the window however much the intent holds (the prototype's
                // own fallback is a scrollable field).
                scroll()
                    .test_id("review-fields")
                    .w_full()
                    .max_h(move || px(sections_max_h()))
                    .child(
                        view()
                            .w_full()
                            .flex_col()
                            .gap(px(SPACE_3))
                            .pt(px(SPACE_2))
                            .children(sections.into_iter().map(|(title, items)| {
                                view()
                                    .test_id("review-section")
                                    .flex_col()
                                    .gap(px(SPACE_1))
                                    .child(prims::rest(title))
                                    .children(items.into_iter().map(prims::ink))
                                    .into_element()
                            })),
                    ),
            );
        }
        block = block.child(disclosure);
    }
    block
}

/// The room `All fields` may take: a quarter of the field, so the question,
/// the legend and the footer stay in the window at any size.
fn sections_max_h() -> f32 {
    (field_height() * 0.25).clamp(120.0, 260.0)
}

/// The docked reading position: the finding `J`/`K` stepped to, with its
/// kind and its line range. The question to answer is drawn below it.
fn docked(
    state: AppState,
    language: Language,
    session: Signal<Session>,
    review: &ReviewView,
    emitters: &Emitters,
    stale: bool,
) -> AnyElement {
    let selected = session.get();
    let Some((index, finding)) = selected.focus.and_then(|index| {
        review
            .findings_anchored
            .get(index)
            .map(|finding| (index, finding.clone()))
    }) else {
        return view().into_element();
    };
    if !selected.showing_finding {
        return view().into_element();
    }
    let heat = emitters
        .findings
        .get(index)
        .copied()
        .unwrap_or_else(Heat::default);
    // The reviewed document's text: `lines N–M` is that document's, and
    // another tab's text would name the wrong lines.
    let lines = reviewed_tab(&state, review)
        .map(|tab| tab.text.get_untracked())
        .as_ref()
        .and_then(|text| glow::finding_lines(&finding, text))
        .map(|(first, last)| lines_label(first, last));
    let mut block = view().test_id("review-docked").w_full().flex_col().child(
        view()
            .test_id("review-finding")
            .w_full()
            .flex_col()
            .gap(px(SPACE_2))
            .child(
                view()
                    .flex_row()
                    .items_center()
                    .child(prims::rest(finding_kind(&finding, language)))
                    .child(
                        view()
                            .child(prims::faint(match lines {
                                Some(lines) => format!(" · {lines}"),
                                None => String::new(),
                            }))
                            .into_element(),
                    ),
            )
            .child(voice(finding.text, heat, language, drained(stale))),
    );
    if !stale {
        block = block.child(prims::bloom(heat, 6.0));
    }
    block.into_element()
}

/// The questions: the one to answer HOT with its `Why it matters:` and its
/// answer field, the others as single ember lines.
fn questions(
    state: AppState,
    language: Language,
    session: Signal<Session>,
    review: &ReviewView,
    order: &[usize],
    emitters: &Emitters,
    stale: bool,
) -> impl IntoElement {
    let session_now = session.get();
    let current = current_question(&session_now, order);
    let mut block = view().w_full().flex_col().gap(px(SPACE_3));
    if let Some(index) = current
        && let Some(question) = review.questions_detail.get(index).cloned()
    {
        let heat = question_heat(emitters, order, index);
        block = block.child(current_question_block(
            state, language, session, order, question, index, heat, stale,
        ));
    }
    let others: Vec<usize> = order
        .iter()
        .copied()
        .filter(|index| Some(*index) != current)
        .collect();
    block = block.child(
        view()
            .flex_col()
            .gap(px(SPACE_2))
            .children(others.into_iter().map(|index| {
                let question = review.questions_detail.get(index);
                let heat = question_heat(emitters, order, index);
                let answered = session_now.answered.contains(&index);
                let skipped = session_now.skipped.contains(&index);
                let text = question
                    .map(|question| question.question.clone())
                    .unwrap_or_default();
                let priority = question
                    .map(|question| priority_label(question.priority, language))
                    .unwrap_or_default();
                let row = cooled_row(&text, priority, heat, language, stale, answered, skipped);
                // A cooled question is not dead: pressing it (or reaching it
                // with Tab) makes it the current one again, with its answer
                // as it was left, so a typo is fixable without re-running
                // the whole review. While the result is stale there is
                // nothing to edit, and the line stays plain text.
                if stale {
                    return view()
                        .test_id("review-question-line")
                        .w_full()
                        .child(row)
                        .into_element();
                }
                let reopen = {
                    let order = order.to_vec();
                    move || reopen_question(session, &order, index)
                };
                view()
                    .test_id("review-question-line")
                    .w_full()
                    .on_press(reopen.clone())
                    .child(
                        button()
                            .accessibility_label(Some(SharedString::from(text.clone())))
                            .w_full()
                            .p(px(0.0))
                            .bg(Color::TRANSPARENT)
                            .cursor_pointer()
                            .focus_visible(|style| style.text_color(tokens::palette().ink))
                            .on_press(reopen)
                            .child(row),
                    )
                    .into_element()
            })),
    );
    block
}

/// The question to answer: its priority, its words, `Why it matters:` and
/// the answer field with `Answer` and `Skip`.
#[allow(clippy::too_many_arguments)]
fn current_question_block(
    state: AppState,
    language: Language,
    session: Signal<Session>,
    order: &[usize],
    question: ReviewQuestionView,
    index: usize,
    heat: Heat,
    stale: bool,
) -> impl IntoElement {
    let answer = state
        .revision_questions
        .get()
        .get(index)
        .map(|question| question.answer);
    let submit = {
        let state = state.clone();
        let order = order.to_vec();
        move || answer_question(state.clone(), session, &order, index)
    };
    let skip = {
        let state = state.clone();
        let order = order.to_vec();
        move || skip_question(state.clone(), session, &order, index)
    };
    let question_text = question.question.clone();
    let impact = question.impact.clone();
    let mut block = view()
        .test_id("review-question")
        .w_full()
        .flex_col()
        .gap(px(SPACE_2))
        .child(
            text(priority_label(question.priority, language))
                // The medium instance's own family; the three Grotesk faces
                // are one family each.
                .font_family(tokens::FONT_MEDIUM)
                .text_size(px(TYPE_UI))
                .line_height(move || px(ui_line(language)))
                .font_weight(tokens::WEIGHT_MEDIUM)
                .text_color(move || tokens::palette().ink)
                .bind_color(&heat.glyph(
                    tokens::palette().ember,
                    tokens::palette().ember,
                    tokens::palette().hot,
                )),
        )
        .child(voice(question_text, heat, language, drained(stale)));
    if let Some(impact) = impact {
        block = block.child(
            view()
                .flex_col()
                .child(prims::rest(s(S::WhyItMatters, language)))
                .child(heat_line(impact, heat, language, stale, false)),
        );
    }
    if let Some(answer) = answer
        && !stale
    {
        block = block
            .child(answer_field(
                language,
                SharedString::from(question.question),
                answer,
                index,
                submit.clone(),
            ))
            .child(
                view()
                    .flex_row()
                    .items_center()
                    .gap(px(SPACE_4))
                    .pt(px(SPACE_2))
                    .child(prims::primary_action(
                        "review-answer-submit",
                        s(S::Answer, language),
                        submit,
                    ))
                    .child(prims::quiet_action(
                        "review-skip",
                        s(S::Skip, language),
                        skip,
                    )),
            );
    }
    block
}

/// One cooled question's row: its priority, its words and its state marker —
/// one line, the question's own words giving way when the room is short.
fn cooled_row(
    text: &str,
    priority: &str,
    heat: Heat,
    language: Language,
    stale: bool,
    answered: bool,
    skipped: bool,
) -> impl IntoElement {
    let mut row = view()
        .flex_row()
        .items_center()
        .gap(px(SPACE_3))
        .child(heat_line(priority.to_owned(), heat, language, stale, false))
        .child(heat_line(text.to_owned(), heat, language, stale, true));
    if answered {
        row = row.child(prims::rest(s(S::Answered, language)));
    }
    if skipped {
        row = row.child(prims::faint(s(S::Skipped, language)));
    }
    row
}

/// Makes a cooled question the current one again, its answer as it was left.
fn reopen_question(session: Signal<Session>, order: &[usize], index: usize) {
    let emitters = emitters().get_untracked();
    let previous = current_question(&session.get_untracked(), order);
    session.update(|session| {
        session.answered.retain(|answered| *answered != index);
        session.skipped.retain(|skipped| *skipped != index);
        session.showing_finding = false;
        session.focus = None;
    });
    if let Some(previous) = previous
        && previous != index
    {
        question_heat(&emitters, order, previous).cool();
    }
    ignite_hold(question_heat(&emitters, order, index), 0.0);
    // The question is the HOT item again, and the column's light goes back to
    // the question's own finding.
    glow::focus_finding(None);
    focus_request().set(Some(Focus::Answer(index)));
}

/// Marks the question answered, cools it and ignites the next one.
fn answer_question(state: AppState, session: Signal<Session>, order: &[usize], index: usize) {
    if answer_of(&state, index).trim().is_empty() {
        // An empty answer is what the state sends as `Unanswered`: it is a
        // skip, and the row must not claim the question was answered.
        skip_question(state, session, order, index);
        return;
    }
    session.update(|session| {
        if !session.answered.contains(&index) {
            session.answered.push(index);
        }
        session.showing_finding = false;
    });
    advance_question(session, order, index);
}

/// Marks the question skipped, drops whatever was typed for it — a skip means
/// no answer, and `start_revision` sends what the answer signal holds — cools
/// it and ignites the next one.
fn skip_question(state: AppState, session: Signal<Session>, order: &[usize], index: usize) {
    if let Some(question) = state.revision_questions.get_untracked().get(index) {
        question.answer.set(String::new());
    }
    session.update(|session| {
        if !session.skipped.contains(&index) {
            session.skipped.push(index);
        }
        session.showing_finding = false;
    });
    advance_question(session, order, index);
}

/// What the user typed for question `index`, if the review has it at all.
fn answer_of(state: &AppState, index: usize) -> String {
    state
        .revision_questions
        .get_untracked()
        .get(index)
        .map(|question| question.answer.get_untracked())
        .unwrap_or_default()
}

/// Cools `index` and ignites the question that becomes current.
fn advance_question(session: Signal<Session>, order: &[usize], index: usize) {
    let emitters = emitters().get_untracked();
    question_heat(&emitters, order, index).cool();
    let next = current_question(&session.get_untracked(), order);
    if let Some(next) = next {
        ignite_hold(question_heat(&emitters, order, next), 0.0);
    }
    // The next thing to do takes the focus: the next answer field, or
    // `Prepare revision` once every question is done.
    focus_request().set(Some(match next {
        Some(index) => Focus::Answer(index),
        None => Focus::Prepare,
    }));
}

// ─── the footer ──────────────────────────────────────────────────────────

/// The pointer's way through the findings: `J`/`K` fire only while no text
/// input edits, and the answer field holds the focus by design, so each key is
/// its own control — a press (or Tab and Enter) steps that way. `J` walks
/// forward, `K` back, both clamped at the ends ([`step_finding`]).
fn findings_stepper(
    state: &AppState,
    session: Signal<Session>,
    language: Language,
    findings: usize,
) -> AnyElement {
    view()
        .flex_row()
        .items_center()
        .gap(px(SPACE_2))
        .child(prims::rest(step_hint(findings, language)))
        .child(step_control(
            "review-findings-step",
            S::StepFindings,
            "J",
            1,
            state,
            session,
            language,
        ))
        .child(prims::rest("/"))
        .child(step_control(
            "review-findings-back",
            S::StepFindingsBack,
            "K",
            -1,
            state,
            session,
            language,
        ))
        .child(prims::rest(s(S::StepThrough, language)))
        .into_element()
}

/// One key of [`findings_stepper`]: the `kbd` plate is the target, so the hint
/// reads the same whether it is typed or pressed.
fn step_control(
    id: &'static str,
    label: S,
    key: &'static str,
    delta: i32,
    state: &AppState,
    session: Signal<Session>,
    language: Language,
) -> AnyElement {
    let state = state.clone();
    let step = move || step_finding(&state, session, delta);
    view()
        .test_id(id)
        .on_press(step.clone())
        .child(
            button()
                .accessibility_label(Some(SharedString::from(s(label, language))))
                .p(px(0.0))
                .bg(Color::TRANSPARENT)
                .cursor_pointer()
                .focus_visible(|style| style.text_color(tokens::palette().ink))
                .on_press(step)
                .child(prims::kbd(key)),
        )
        .into_element()
}

/// The legend, and the state's one next action once every question is
/// answered or skipped.
fn footer(
    state: AppState,
    language: Language,
    session: Signal<Session>,
    review: &ReviewView,
) -> impl IntoElement {
    let session_now = session.get();
    let order = priority_order(&review.questions_detail);
    let done =
        review.questions_detail.is_empty() || current_question(&session_now, &order).is_none();
    let prepare_state = state.clone();
    let prepare_session = session;
    let close_state = state.clone();
    view()
        .w_full()
        .flex_col()
        .gap(px(SPACE_3))
        .pb(px(SPACE_3))
        .child(
            view()
                .test_id("review-legend")
                .flex_col()
                .gap(px(SPACE_2))
                .child(
                    view()
                        .flex_row()
                        .items_center()
                        .gap(px(SPACE_4))
                        .child(
                            view()
                                .flex_row()
                                .items_center()
                                .gap(px(SPACE_2))
                                .child(prims::emitter_plate(
                                    text(s(S::WordsSwatch, language))
                                        .font_family(tokens::FONT_SOURCE)
                                        .text_size(px(12.5))
                                        .line_height(px(16.0))
                                        .text_color(move || tokens::palette().ember)
                                        .bg_linear_to_b()
                                        .gradient_from(Color::TRANSPARENT)
                                        .gradient_via(move || tokens::palette().ember_bloom)
                                        .gradient_to(Color::TRANSPARENT),
                                ))
                                .child(prims::rest(s(S::StatedInTheText, language))),
                        )
                        .child(
                            view()
                                .flex_row()
                                .items_center()
                                .gap(px(SPACE_2))
                                .child(haze_swatch())
                                .child(prims::rest(s(S::InferredLegend, language))),
                        ),
                )
                .child(findings_stepper(
                    &state,
                    session,
                    language,
                    review.findings_anchored.len(),
                )),
        )
        .child(
            view()
                .flex_row()
                .items_center()
                .justify_between()
                .child(if done {
                    // The state's one next action takes the focus once, when
                    // every question is done.
                    let reference = element_ref();
                    let focus_reference = reference;
                    effect(move || {
                        if focus_request().get() == Some(Focus::Prepare) {
                            focus_request().set(None);
                            focus_soon(focus_reference);
                        }
                    });
                    prims::primary_action(
                        "review-prepare",
                        s(S::PrepareRevision, language),
                        move || prepare_state.start_revision(),
                    )
                    .element_ref(&reference)
                    .into_element()
                } else {
                    prims::quiet_action(
                        "review-prepare",
                        s(S::PrepareRevision, language),
                        move || {
                            prepare_session.update(|session| session.showing_finding = false);
                            prepare_state.start_revision();
                        },
                    )
                    .into_element()
                })
                .child(prims::quiet_action(
                    "review-close",
                    s(S::Close, language),
                    move || close_state.close_review(),
                )),
        )
}

/// The 2px edge and haze an inferred finding shows in the column.
fn haze_swatch() -> impl IntoElement {
    view()
        .w(px(26.0))
        .h(px(16.0))
        .border_l(px(2.0))
        .border_l_color(move || tokens::palette().haze_edge)
        .bg_linear_to_r()
        .gradient_from(move || tokens::palette().haze)
        .gradient_to(Color::TRANSPARENT)
}

// ─── the other states ────────────────────────────────────────────────────

/// One resting line while the model works, with `Cancel`, and — after a
/// second — the seconds hint.
fn loading_line(state: AppState, language: Language) -> impl IntoElement {
    let cancel_state = state.clone();
    let host = reviewing_with(&state, language);
    view()
        .pt(px(SPACE_6))
        .w_full()
        .flex_col()
        .gap(px(SPACE_3))
        .child(
            view()
                .flex_row()
                .items_center()
                .justify_between()
                .child(prims::rest(host))
                .child(prims::quiet_action(
                    "review-cancel",
                    s(S::Cancel, language),
                    move || {
                        session().update(|session| session.cancelled = true);
                        cancel_state.cancel_review_in_flight();
                    },
                )),
        )
        .child(when(
            move || session().get().slow,
            move || prims::faint(s(S::UsuallySeconds, language)).into_element(),
        ))
}

/// The wait was cancelled: calm text, and the way back to a review.
fn cancelled(state: AppState, language: Language) -> impl IntoElement {
    let again = state.clone();
    view()
        .test_id("review-cancelled")
        .pt(px(SPACE_6))
        .w_full()
        .flex_col()
        .gap(px(SPACE_3))
        .child(prims::rest(s(S::Cancelled, language)))
        .child(prims::quiet_action(
            "review-again",
            s(S::ReviewAgain, language),
            move || again.start_review(),
        ))
}

/// Why Review cannot run, with the one way forward that can help.
fn unavailable(state: AppState, language: Language, reason: ReviewUnavailable) -> impl IntoElement {
    let action_state = state.clone();
    let close = state.clone();
    let (headline, hint, label, opens_settings) = match reason {
        ReviewUnavailable::NotSetUp => (
            s(S::NoModelSetUp, language),
            s(S::NoModelHint, language),
            s(S::OpenSettings, language),
            true,
        ),
        ReviewUnavailable::Unpreparable => (
            s(S::CouldNotPrepare, language),
            s(S::CouldNotPrepareHint, language),
            s(S::TryAgain, language),
            false,
        ),
    };
    view()
        .test_id("review-unavailable")
        .pt(px(SPACE_6))
        .w_full()
        .flex_col()
        .gap(px(SPACE_3))
        .child(prims::ink(headline))
        .child(prims::rest(hint))
        .child(
            view()
                .flex_row()
                .items_center()
                .gap(px(SPACE_4))
                .child(prims::primary_action(
                    "review-open-settings",
                    label,
                    move || {
                        if opens_settings {
                            action_state.open_settings();
                        } else {
                            action_state.start_review();
                        }
                    },
                ))
                .child(prims::quiet_action(
                    "review-close",
                    s(S::Close, language),
                    move || close.close_review(),
                )),
        )
}

// ─── copy ────────────────────────────────────────────────────────────────

/// The field's own copy. New strings live here rather than in `i18n.rs` (a
/// merge hotspot): the match is exhaustive, so a missing translation is a
/// compile error.
#[derive(Clone, Copy, PartialEq, Eq)]
enum S {
    Lens,
    Understood,
    AllFields,
    WhyItMatters,
    Answer,
    Skip,
    Answered,
    Skipped,
    PrepareRevision,
    ReviewAgain,
    Close,
    Cancel,
    ReviewingWith,
    UsuallySeconds,
    Cancelled,
    Stale,
    NoModelSetUp,
    NoModelHint,
    OpenSettings,
    CouldNotPrepare,
    CouldNotPrepareHint,
    TryAgain,
    StatedInTheText,
    InferredLegend,
    WordsSwatch,
    TypeYourAnswer,
    StepThrough,
    StepFindings,
    StepFindingsBack,
    CouldNotRead,
}

fn s(key: S, language: Language) -> &'static str {
    match language {
        Language::English => match key {
            S::Lens => "Lens",
            S::Understood => "Understood:",
            S::AllFields => "All fields",
            S::WhyItMatters => "Why it matters:",
            S::Answer => "Answer",
            S::Skip => "Skip",
            S::Answered => "Answered",
            S::Skipped => "Skipped",
            S::PrepareRevision => "Prepare revision",
            S::ReviewAgain => "Review again",
            S::Close => "Close",
            S::Cancel => "Cancel",
            S::ReviewingWith => "Reviewing with {host}…",
            S::UsuallySeconds => "Usually 10–30 seconds. Esc cancels.",
            S::Cancelled => "Cancelled — nothing changed.",
            S::Stale => "Stale — edited after review",
            S::NoModelSetUp => "No model set up",
            S::NoModelHint => "Add a provider and an API key in Settings, then run Review again.",
            S::OpenSettings => "Open settings",
            S::CouldNotPrepare => "Couldn't prepare the request",
            S::CouldNotPrepareHint => {
                "This document and its context could not form a review request."
            }
            S::TryAgain => "Try again",
            S::StatedInTheText => "Stated in the text",
            S::InferredLegend => "Inferred",
            S::WordsSwatch => "words",
            S::TypeYourAnswer => "Type your answer",
            S::StepThrough => "to step through",
            S::StepFindings => "Next finding",
            S::StepFindingsBack => "Previous finding",
            S::CouldNotRead => "Couldn't read the response — Try again",
        },
        Language::Chinese => match key {
            S::Lens => "视角",
            S::Understood => "理解：",
            S::AllFields => "全部字段",
            S::WhyItMatters => "为何重要：",
            S::Answer => "回答",
            S::Skip => "跳过",
            S::Answered => "已回答",
            S::Skipped => "已跳过",
            S::PrepareRevision => "准备修订",
            S::ReviewAgain => "重新审查",
            S::Close => "关闭",
            S::Cancel => "取消",
            S::ReviewingWith => "正在审查 · {host}…",
            S::UsuallySeconds => "通常需要 10–30 秒。按 Esc 取消。",
            S::Cancelled => "已取消——没有任何更改。",
            S::Stale => "已过期——审查后文档被编辑",
            S::NoModelSetUp => "未配置模型",
            S::NoModelHint => "请在设置中添加服务商和 API 密钥，然后重新运行审查。",
            S::OpenSettings => "打开设置",
            S::CouldNotPrepare => "无法准备请求",
            S::CouldNotPrepareHint => "此文档及其上下文无法构成审查请求。",
            S::TryAgain => "重试",
            S::StatedInTheText => "文本中陈述",
            S::InferredLegend => "推断",
            S::WordsSwatch => "词语",
            S::TypeYourAnswer => "输入回答",
            S::StepThrough => "切换",
            S::StepFindings => "下一个发现",
            S::StepFindingsBack => "上一个发现",
            S::CouldNotRead => "无法读取响应——请重试",
        },
    }
}

/// `Reviewing with api.openai.com…`: the configured endpoint's host, as the
/// consent disclosure names it.
fn reviewing_with(state: &AppState, language: Language) -> SharedString {
    let settings = state.settings.get();
    let host = Provider::from_key(settings.model_provider.trim())
        .and_then(|provider| {
            EndpointIdentity::parse(provider, Some(settings.model_base_url.as_str())).ok()
        })
        .map(|endpoint| endpoint.host().to_owned())
        .unwrap_or_else(|| settings.model_provider.clone());
    SharedString::from(s(S::ReviewingWith, language).replace("{host}", &host))
}

/// `3 findings · J / K to step through` opens with this, then the keys.
fn step_hint(findings: usize, language: Language) -> String {
    match language {
        Language::English => match findings {
            0 => "No findings ·".to_owned(),
            1 => "1 finding ·".to_owned(),
            _ => format!("{findings} findings ·"),
        },
        Language::Chinese => format!("{findings} 个发现 ·"),
    }
}

/// `Timed out after 120 s`: the seconds are the core's own request timeout,
/// so the copy cannot drift from what actually happens. `Try again` is the
/// button under it, not a second invitation in the sentence.
fn timed_out(language: Language) -> String {
    let seconds = REVIEW_REQUEST_TIMEOUT.as_secs();
    match language {
        Language::English => format!("Timed out after {seconds} s"),
        Language::Chinese => format!("{seconds} 秒后超时"),
    }
}

/// `lines 14–16`.
fn lines_label(first: u64, last: u64) -> String {
    if first == last {
        format!("line {first}")
    } else {
        format!("lines {first}–{last}")
    }
}

/// The kind a finding's light says it has.
fn finding_kind(finding: &ReviewFindingView, language: Language) -> &'static str {
    if finding.stated {
        s(S::StatedInTheText, language)
    } else {
        s(S::InferredLegend, language)
    }
}

/// The localized priority word (the i18n keys the old panel used).
fn priority_label(priority: ClarificationPriority, language: Language) -> &'static str {
    match priority {
        ClarificationPriority::Critical => i18n::t(language, Key::ReviewPriorityCritical),
        ClarificationPriority::High => i18n::t(language, Key::ReviewPriorityHigh),
        ClarificationPriority::Medium => i18n::t(language, Key::ReviewPriorityMedium),
        ClarificationPriority::Low => i18n::t(language, Key::ReviewPriorityLow),
    }
}

/// The lens kind, as the existing labels name it.
fn lens_label(lens: ArtifactLens, language: Language) -> &'static str {
    match lens {
        ArtifactLens::Prompt => i18n::t(language, Key::ReviewLensPrompt),
        ArtifactLens::Specification => i18n::t(language, Key::UsageLensSpecification),
        ArtifactLens::Plan => i18n::t(language, Key::UsageLensPlan),
        ArtifactLens::AgentInstructions => i18n::t(language, Key::ReviewLensAgentInstructions),
        ArtifactLens::AgentSkill => i18n::t(language, Key::ReviewLensAgentSkill),
    }
}

// ─── order and heat helpers ──────────────────────────────────────────────

/// The question ordinals in display order: priority first (Critical down),
/// then the model's own order.
fn priority_order(questions: &[ReviewQuestionView]) -> Vec<usize> {
    let mut order: Vec<usize> = (0..questions.len()).collect();
    order.sort_by_key(|index| (questions[*index].priority, *index));
    order
}

/// The question to answer: the first in display order that is neither
/// answered nor skipped.
fn current_question(session: &Session, order: &[usize]) -> Option<usize> {
    order
        .iter()
        .copied()
        .find(|index| !session.answered.contains(index) && !session.skipped.contains(index))
}

/// The heat of the question `index`, whatever its display slot.
fn question_heat(emitters: &Emitters, order: &[usize], index: usize) -> Heat {
    order
        .iter()
        .position(|slot| *slot == index)
        .and_then(|slot| emitters.questions.get(slot).copied())
        .unwrap_or_default()
}

/// The afterimage ink, once the source has moved on.
fn drained(stale: bool) -> Option<Color> {
    stale.then(|| tokens::palette().afterimage_ink)
}

/// The model's voice: 22/30, and 1.75 line height for Chinese. With
/// `drained`, the colour is the afterimage's neutral ink instead of heat.
fn voice(
    content: impl IntoReactive<SharedString>,
    heat: Heat,
    language: Language,
    drained: Option<Color>,
) -> impl IntoElement {
    let palette = tokens::palette();
    let (ink, ember, hot) = match drained {
        Some(afterimage) => (afterimage, afterimage, afterimage),
        None => (palette.ink, palette.ember, palette.hot),
    };
    text(content)
        .font_family(tokens::FONT_MODEL)
        .font_weight(WEIGHT_MODEL)
        .text_size(px(TYPE_MODEL))
        .line_height(move || px(model_line(language)))
        .text_color(move || tokens::palette().ink)
        .bind_color(&heat.glyph(ink, ember, hot))
}

/// A UI-size line that follows a heat: the priority word, the `Why it
/// matters:` line and the cooled question lines. `one_line` pins a cooled
/// question's words to one row, giving way when the row needs the room for
/// its state marker.
fn heat_line(
    content: impl IntoReactive<SharedString>,
    heat: Heat,
    language: Language,
    drained: bool,
    one_line: bool,
) -> impl IntoElement {
    let palette = tokens::palette();
    let (ink, ember, hot) = if drained {
        (
            palette.afterimage_ink,
            palette.afterimage_ink,
            palette.afterimage_ink,
        )
    } else {
        (palette.ink, palette.ember, palette.hot)
    };
    let line = text(content)
        .font_family(tokens::FONT_UI)
        .text_size(px(TYPE_UI))
        .line_height(move || px(ui_line(language)))
        .text_color(move || tokens::palette().ink)
        .bind_color(&heat.glyph(ink, ember, hot));
    if one_line {
        line.flex_1()
            .min_w(px(0.0))
            .number_of_lines(1u32)
            .text_overflow(EllipsizeMode::Tail)
    } else {
        line
    }
}

/// The answer field: a boxed input that takes the focus when it appears,
/// with the designed ring while it edits. It shows the state's answer signal
/// and writes back to it, so `Prepare revision` carries what was typed.
fn answer_field(
    language: Language,
    label: SharedString,
    answer: Signal<String>,
    index: usize,
    submit: impl Fn() + 'static,
) -> impl ParentElement {
    // The field takes the focus when its state is entered — never on a plain
    // rebuild, which would take it from `J`/`K` (or the stepper) on every
    // step.
    let reference = element_ref();
    let focus_reference = reference;
    effect(move || {
        if focus_request().get() == Some(Focus::Answer(index)) {
            focus_request().set(None);
            focus_soon(focus_reference);
        }
    });
    view()
        .test_id("review-answer")
        .w_full()
        .p(px(FOCUS_RING_OFFSET))
        .border(px(FOCUS_RING))
        .border_color(Color::TRANSPARENT)
        .focus_within(|style| style.border_color(tokens::palette().ink))
        .child(
            input()
                .test_id("review-answer-input")
                .label(label)
                .element_ref(&reference)
                .value(answer)
                .on_change(move |text: &str| answer.set(text.to_owned()))
                .on_submit(move |_: &str| submit())
                .placeholder(s(S::TypeYourAnswer, language))
                .font_family(tokens::FONT_UI)
                .text_size(px(TYPE_UI))
                .line_height(px(TYPE_UI_LINE))
                .h(px(40.0))
                .px(px(SPACE_3))
                .bg(move || tokens::palette().room)
                .border(px(tokens::HAIRLINE))
                .border_color(move || with_alpha(tokens::palette().ink, 0.16))
                .focus_visible(|style| style.border_color(tokens::palette().ink)),
        )
}

/// The UI text line height: 12.5/18, at 1.75 for Chinese.
fn ui_line(language: Language) -> f32 {
    if language == Language::Chinese {
        TYPE_UI * tokens::CJK_LINE_FACTOR
    } else {
        TYPE_UI_LINE
    }
}

/// The model's voice line height: 22/30, at 1.75 for Chinese.
fn model_line(language: Language) -> f32 {
    if language == Language::Chinese {
        TYPE_MODEL * tokens::CJK_LINE_FACTOR
    } else {
        TYPE_MODEL_LINE
    }
}

#[cfg(test)]
mod tests {
    use mt_core::review::SourceLocation;
    use nsh_engine::{EngineConfig, Root};
    use nsh_testing::Headless;

    use super::*;

    fn question(priority: ClarificationPriority, text: &str) -> ReviewQuestionView {
        ReviewQuestionView {
            priority,
            question: text.to_owned(),
            impact: None,
        }
    }

    #[test]
    fn questions_are_ordered_critical_first() {
        let questions = vec![
            question(ClarificationPriority::Low, "third"),
            question(ClarificationPriority::Critical, "first"),
            question(ClarificationPriority::Medium, "second"),
        ];
        assert_eq!(priority_order(&questions), vec![1, 2, 0]);
    }

    #[test]
    fn the_current_question_is_the_first_unanswered_and_unskipped() {
        let questions = vec![
            question(ClarificationPriority::Critical, "first"),
            question(ClarificationPriority::High, "second"),
        ];
        let order = priority_order(&questions);
        let mut session = Session::default();
        assert_eq!(current_question(&session, &order), Some(0));
        session.answered.push(0);
        assert_eq!(current_question(&session, &order), Some(1));
        session.skipped.push(1);
        assert_eq!(current_question(&session, &order), None);
    }

    #[test]
    fn every_copy_key_is_translated() {
        for key in [
            S::Lens,
            S::Understood,
            S::AllFields,
            S::WhyItMatters,
            S::Answer,
            S::Skip,
            S::Answered,
            S::Skipped,
            S::PrepareRevision,
            S::ReviewAgain,
            S::Close,
            S::Cancel,
            S::ReviewingWith,
            S::UsuallySeconds,
            S::Cancelled,
            S::Stale,
            S::NoModelSetUp,
            S::NoModelHint,
            S::OpenSettings,
            S::CouldNotPrepare,
            S::CouldNotPrepareHint,
            S::TryAgain,
            S::StatedInTheText,
            S::InferredLegend,
            S::WordsSwatch,
            S::TypeYourAnswer,
            S::StepThrough,
            S::StepFindings,
            S::StepFindingsBack,
            S::CouldNotRead,
        ] {
            assert!(!s(key, Language::English).is_empty());
            assert!(!s(key, Language::Chinese).is_empty());
        }
    }

    #[test]
    fn the_line_label_reads_as_the_spec_writes_it() {
        assert_eq!(lines_label(14, 16), "lines 14–16");
        assert_eq!(lines_label(7, 7), "line 7");
    }

    #[test]
    fn the_timeout_copy_is_the_core_request_timeout() {
        let seconds = REVIEW_REQUEST_TIMEOUT.as_secs().to_string();
        for language in [Language::English, Language::Chinese] {
            let copy = timed_out(language);
            assert!(copy.contains(&seconds), "{copy} names the timeout");
            // `Try again` is the button under it, not a second invitation.
            assert!(
                !copy.contains("Try again") && !copy.contains("重试"),
                "{copy}"
            );
        }
    }

    #[test]
    fn the_step_hint_counts_findings() {
        assert_eq!(step_hint(0, Language::English), "No findings ·");
        assert_eq!(step_hint(1, Language::English), "1 finding ·");
        assert_eq!(step_hint(4, Language::English), "4 findings ·");
    }

    /// The document the camera probes review: three known lines.
    fn camera_fixture() -> &'static std::path::Path {
        static FIXTURE: std::sync::LazyLock<(tempfile::TempDir, std::path::PathBuf)> =
            std::sync::LazyLock::new(|| {
                let dir = tempfile::tempdir().expect("temp dir");
                let path = dir.path().join("camera.md");
                std::fs::write(&path, "alpha\nbeta\ngamma\n").expect("write the fixture");
                (dir, path)
            });
        FIXTURE.1.as_path()
    }

    /// A result whose findings are anchored to the fixture's first and second
    /// lines (the 1-based `lines` the model writes).
    fn camera_view(tab_id: u64) -> ReviewView {
        ReviewView {
            findings_anchored: vec![
                ReviewFindingView {
                    stated: false,
                    text: "the first".to_owned(),
                    location: Some(SourceLocation::lines(1, 1)),
                },
                ReviewFindingView {
                    stated: false,
                    text: "the second".to_owned(),
                    location: Some(SourceLocation::lines(2, 2)),
                },
            ],
            tab_id: Some(tab_id),
            ..ReviewView::default()
        }
    }

    /// The camera request the state holds, drawn as `<tab>:<line>`: a signal
    /// is not readable from the test thread, its render is.
    fn camera_request(state: &AppState) -> impl IntoElement {
        let state = state.clone();
        text(move || {
            let focus = state.document_focus.get();
            format!("{}:{}", focus.tab, focus.line)
        })
        .test_id("camera-request")
    }

    /// The reviewed tab's id, drawn so a request can be checked against it.
    fn camera_tab(state: &AppState) -> impl IntoElement {
        let state = state.clone();
        text(move || {
            state
                .active_tab()
                .map(|tab| tab.id.to_string())
                .unwrap_or_default()
        })
        .test_id("camera-tab")
    }

    /// Hand the fixture's tab to `glow` and to the state once it lands, as
    /// the field's own result watcher does.
    fn load_result(state: AppState) {
        effect(move || {
            if state.review_view.get_untracked().is_some() {
                return;
            }
            let Some(tab) = state.active_tab() else {
                return;
            };
            let view = camera_view(tab.id);
            glow::observe(Some(&view), Some(&tab));
            state.review_view.set(Some(view));
        });
    }

    /// A live review and one control that steps through findings exactly as
    /// `J` does; what it asks the column's camera for is on screen.
    #[component]
    fn StepProbe() -> impl IntoElement {
        let state = AppState::new();
        state.open_file(camera_fixture().to_path_buf());
        state.review_open.set(true);
        load_result(state.clone());
        let session = session();
        let step_state = state.clone();
        view()
            .test_id("camera-probe")
            .flex_col()
            .w(px(400.0))
            .h(px(200.0))
            .child(camera_tab(&state))
            .child(camera_request(&state))
            .child(
                view()
                    .test_id("camera-step")
                    .w_full()
                    .h(px(24.0))
                    .on_press(move || step_finding(&step_state, session, 1)),
            )
    }

    /// A review that arrives through the field's own result watcher, so the
    /// camera request comes from the arrival path itself.
    #[component]
    fn ArrivalProbe() -> impl IntoElement {
        let state = AppState::new();
        state.open_file(camera_fixture().to_path_buf());
        state.review_open.set(true);
        observe(&state, session());
        let loaded_state = state.clone();
        effect(move || {
            if loaded_state.review_view.get_untracked().is_some() {
                return;
            }
            let Some(tab) = loaded_state.active_tab() else {
                return;
            };
            loaded_state.review_view.set(Some(camera_view(tab.id)));
        });
        let marker_state = state.clone();
        view()
            .test_id("camera-arrival-probe")
            .flex_col()
            .w(px(400.0))
            .h(px(200.0))
            .child(camera_tab(&state))
            .child(camera_request(&state))
            .child(when(
                move || marker_state.active_tab().is_some(),
                || view().test_id("camera-loaded").w(px(1.0)).h(px(1.0)),
            ))
    }

    /// The text of the node `id`, once the probe renders it.
    fn text_of(app: &Headless, id: &str) -> String {
        app.screen()
            .layout_nodes()
            .iter()
            .find(|node| node.test_id.as_deref() == Some(id))
            .and_then(|node| node.text.as_ref().map(|text| text.text.text.to_string()))
            .unwrap_or_else(|| panic!("no `{id}` text on screen: {:?}", app.screen().texts()))
    }

    /// `J`/`K` (and the stepper) ask the column's camera for the stepped
    /// finding's anchored start line, of the reviewed tab.
    ///
    /// The probe's second finding is anchored to the fixture's second line, so
    /// a request that follows the step reads `<tab>:1` — not the first
    /// finding's line, and not another tab's.
    #[test]
    fn stepping_asks_the_camera_for_the_findings_line() {
        let mut app =
            Headless::start(EngineConfig::new(Root::of::<StepProbe>())).expect("start the probe");
        app.wait_for("id:camera-step").expect("the probe renders");
        app.settle();
        app.wait(Duration::from_millis(200))
            .expect("the fixture's tab lands");
        app.settle();

        let tab = text_of(&app, "camera-tab");
        assert!(!tab.is_empty(), "the fixture's tab must be open");
        app.press("id:camera-step").expect("step to the first");
        app.settle();
        assert_eq!(
            text_of(&app, "camera-request"),
            format!("{tab}:0"),
            "the first step must level the first finding's line"
        );
        app.press("id:camera-step").expect("step to the second");
        app.settle();
        assert_eq!(
            text_of(&app, "camera-request"),
            format!("{tab}:1"),
            "the step must level the stepped finding's own line"
        );
    }

    /// A result's arrival asks the camera for the first finding's line: its
    /// simultaneous ignition with the first question is the link.
    #[test]
    fn the_arrival_levels_the_first_finding() {
        let mut app = Headless::start(EngineConfig::new(Root::of::<ArrivalProbe>()))
            .expect("start the probe");
        app.wait_for("id:camera-loaded")
            .expect("the fixture's tab lands");
        app.settle();
        app.wait(Duration::from_millis(200))
            .expect("the result watcher runs");
        app.settle();

        let tab = text_of(&app, "camera-tab");
        assert!(!tab.is_empty(), "the fixture's tab must be open");
        assert_eq!(
            text_of(&app, "camera-request"),
            format!("{tab}:0"),
            "the arrival must level the first finding's line"
        );
    }

    /// The footer's own stepper, drawn with the fixture's two findings: what
    /// each press asks the column's camera for is on screen.
    #[component]
    fn StepperProbe() -> impl IntoElement {
        let state = AppState::new();
        state.open_file(camera_fixture().to_path_buf());
        state.review_open.set(true);
        load_result(state.clone());
        view()
            .test_id("stepper-probe")
            .flex_col()
            .w(px(400.0))
            .h(px(200.0))
            .child(camera_tab(&state))
            .child(camera_request(&state))
            .child(findings_stepper(&state, session(), Language::English, 2))
    }

    /// The pointer walks the findings both ways and stops at either end: the
    /// `kbd` plates are targets, so pressing `J`'s takes the next finding and
    /// pressing `K`'s takes the previous one back.
    #[test]
    fn the_pointer_steps_the_findings_both_ways() {
        let mut app = Headless::start(EngineConfig::new(Root::of::<StepperProbe>()))
            .expect("start the probe");
        app.wait_for("id:review-findings-step")
            .expect("the stepper renders");
        app.settle();
        app.wait(Duration::from_millis(200))
            .expect("the fixture's tab lands");
        app.settle();

        let tab = text_of(&app, "camera-tab");
        assert!(!tab.is_empty(), "the fixture's tab must be open");
        app.press("id:review-findings-step")
            .expect("step to the first");
        app.settle();
        assert_eq!(
            text_of(&app, "camera-request"),
            format!("{tab}:0"),
            "a forward step from nothing lands on the first finding"
        );
        app.press("id:review-findings-step")
            .expect("step to the second");
        app.settle();
        assert_eq!(
            text_of(&app, "camera-request"),
            format!("{tab}:1"),
            "a forward step takes the next finding"
        );
        app.press("id:review-findings-step")
            .expect("step past the last");
        app.settle();
        assert_eq!(
            text_of(&app, "camera-request"),
            format!("{tab}:1"),
            "a forward step stops at the last finding"
        );
        app.press("id:review-findings-back")
            .expect("back to the first");
        app.settle();
        assert_eq!(
            text_of(&app, "camera-request"),
            format!("{tab}:0"),
            "the back control takes the previous finding"
        );
        app.press("id:review-findings-back")
            .expect("back past the first");
        app.settle();
        assert_eq!(
            text_of(&app, "camera-request"),
            format!("{tab}:0"),
            "the back control stops at the first finding"
        );
    }

    /// The loading flag and the review generation, drawn for the test.
    #[component]
    fn BusyProbe() -> impl IntoElement {
        let state = AppState::new();
        let arm_state = state.clone();
        let start_state = state.clone();
        view()
            .test_id("busy-probe")
            .flex_col()
            .w(px(400.0))
            .h(px(200.0))
            .child(
                text({
                    let state = state.clone();
                    move || {
                        format!(
                            "{}:{}",
                            state.review_busy.get(),
                            state.review_generation.get()
                        )
                    }
                })
                .test_id("busy-state"),
            )
            .child(
                view()
                    .test_id("busy-arm")
                    .w_full()
                    .h(px(24.0))
                    .on_press(move || arm_state.review_busy.set(true)),
            )
            .child(
                view()
                    .test_id("busy-start")
                    .w_full()
                    .h(px(24.0))
                    .on_press(move || start_state.start_review()),
            )
    }

    /// Preparing a review supersedes whatever was in flight: the loading flag
    /// its task would have cleared goes straight away, so an early return
    /// (no request can be prepared in this probe) cannot leave the field
    /// saying `Reviewing…` with nothing in flight.
    #[test]
    fn preparing_a_review_clears_the_loading_state_it_supersedes() {
        let mut app =
            Headless::start(EngineConfig::new(Root::of::<BusyProbe>())).expect("start the probe");
        app.wait_for("id:busy-state").expect("the probe renders");
        app.settle();
        assert_eq!(text_of(&app, "busy-state"), "false:0");
        app.press("id:busy-arm").expect("arm the loading state");
        app.settle();
        assert_eq!(text_of(&app, "busy-state"), "true:0");
        app.press("id:busy-start").expect("prepare another review");
        app.settle();
        assert_eq!(
            text_of(&app, "busy-state"),
            "false:1",
            "the superseded loading state must go with its request"
        );
    }
}
