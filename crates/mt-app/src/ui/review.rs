//! Owner lane: review.
//!
//! The field during review: the lens line, 'Understood: <goal>', the questions
//! (the current one HOT with its `Why it matters:` and its answer field), the
//! cooled question list, `All fields`, loading, stale and the diagnostics.
//!
//! The shell chooses this field surface and gives it its box;
//! review owns its reading position and contents.
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
use nsh_base::input;
use nsh_core::{
    Color, EllipsizeMode,
    element::{AdaptiveColor, KeyOptions},
    keymap::When,
    prelude::*,
};

use crate::{
    i18n::{self, Key},
    state::{
        AppState, ReviewFailure, ReviewFindingView, ReviewQuestionView, ReviewUnavailable,
        ReviewView, Tab,
    },
    ui::{
        consent, glow,
        motion::{EASE_IN_OUT, Heat, IGNITE_MS, LINEAR, REDUCED_CROSSFADE_MS, reduced_motion},
        prims, titlebar,
        tokens::{
            self, FOCUS_RING, FOCUS_RING_OFFSET, SPACE_1, SPACE_2, SPACE_3, SPACE_4, SPACE_5,
            SPACE_XL, TYPE_MODEL, TYPE_MODEL_LINE, TYPE_UI, TYPE_UI_LINE, WEIGHT_MODEL, with_alpha,
        },
    },
};

/// Whether the field is showing a review, or explaining why it cannot.
pub fn is_open(state: &AppState) -> bool {
    let belongs = state
        .review_view
        .get()
        .is_none_or(|view| view.tab_id.is_none() || view.tab_id == state.active.get());
    (state.review_open.get() && belongs) || state.review_unavailable.get().is_some()
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
        .flex_1()
        .min_h(px(0.0))
        .flex_col()
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
    showing_finding_before_revision: Option<bool>,
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

/// A question never takes the keyboard from the source editor the user is
/// typing in — a stale result's `Review again` included: a background result
/// arriving while the user types must not yank the focus anywhere. Nor does
/// it take the keyboard from an open consent card, which owns it while it is
/// open.
fn arrival_focus(
    stale: bool,
    current: Option<usize>,
    editor_focused: bool,
    consent_open: bool,
) -> Option<Focus> {
    if editor_focused || consent_open {
        return None;
    }
    if stale {
        return Some(Focus::ReviewAgain);
    }
    match current {
        Some(index) => Some(Focus::Answer(index)),
        None => Some(Focus::Prepare),
    }
}

fn focus_soon(reference: ElementRef) {
    let step = set_timeout(Duration::from_millis(0), move || reference.focus());
    focus_step().set(Some(step));
}

fn session() -> Signal<Session> {
    global::<Session>()
}

fn prepare_revision(state: &AppState, session: Signal<Session>) {
    session.update(|session| {
        session.showing_finding_before_revision = Some(session.showing_finding);
        session.showing_finding = false;
    });
    state.start_revision();
}

pub(crate) fn cancel_revision(state: &AppState) {
    restore_showing_finding();
    state.cancel_revision();
}

/// Cancel the Revision the tab on screen owns, restoring the session state
/// the prepare switched away from.
pub(crate) fn cancel_active_revision(state: &AppState) {
    restore_showing_finding();
    state.cancel_active_revision();
}

fn restore_showing_finding() {
    session().update(|session| {
        if let Some(showing_finding) = session.showing_finding_before_revision.take() {
            session.showing_finding = showing_finding;
        }
    });
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
                built.goal.ignite();
                for (slot, index) in order.iter().enumerate() {
                    let Some(heat) = built.questions.get(slot).copied() else {
                        continue;
                    };
                    if Some(*index) == current {
                        ignite_hold(heat);
                    } else {
                        heat.ignite();
                    }
                }
                // Nothing is focused yet: the question is the one HOT item
                // until `J`/`K` (or the stepper) steps to a finding.
                glow::focus_finding(None);
                level_finding(&state, view.tab_id, 0, true);
                // A result must not take the keyboard from the source
                // editor: while the user is typing there, the question waits
                // for a press instead.
                focus_request().set(arrival_focus(
                    view.stale,
                    current,
                    state.source_editor_focused.get_untracked(),
                    state.consent_open(),
                ));
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
/// an answer never steps a finding, and while no consent card owns the
/// keyboard.
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
                if consent::cancel_open(&state) {
                    return;
                }
                if titlebar::escape_closes_menu(&state) {
                    return;
                }
                if state.review_busy.get_untracked() {
                    session.update(|session| session.cancelled = true);
                    state.cancel_review_in_flight();
                    state.focus_editor();
                } else if state.review_consent.get_untracked().is_none()
                    && state.translation_consent.get_untracked().is_none()
                {
                    state.close_review();
                    state.focus_editor();
                }
            }
        },
    );
    let step_state = state.clone();
    effect(move || {
        if step_state.consent_open() {
            return;
        }
        for (key, delta) in [("j", 1i32), ("k", -1i32)] {
            let state = step_state.clone();
            on_key_with(
                key,
                KeyOptions {
                    when: When::NotEditing,
                    ..KeyOptions::default()
                },
                move |_| step_finding(&state, session, delta),
            );
        }
    });
}

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
    level_finding(state, view.tab_id, to, false);
    let heats = emitters().get_untracked().findings;
    if let Some(from) = from
        && from != to
        && let Some(heat) = heats.get(from).copied()
    {
        heat.cool();
    }
    if let Some(heat) = heats.get(to).copied() {
        ignite_hold(heat);
    }
}

fn level_finding(state: &AppState, tab_id: Option<u64>, finding: usize, animated: bool) {
    if let Some(tab_id) = tab_id
        && let Some(line) = glow::anchor_line(finding)
    {
        if animated {
            state.focus_document_line(tab_id, line);
        } else {
            state.focus_document_line_instant(tab_id, line);
        }
    }
}

/// Ignites `heat` and holds it HOT: a plain [`Heat::ignite`] would cool to
/// ember, and the one current item must stay hot.
fn ignite_hold(heat: Heat) {
    let value = heat.value();
    value.stop();
    if reduced_motion() {
        value.timing_with(1.0, Timing::new(REDUCED_CROSSFADE_MS, EASE_IN_OUT));
        return;
    }
    value.timing_with(1.0, Timing::new(IGNITE_MS, LINEAR));
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
                    .pt(px(SPACE_XL))
                    .child(prims::rest(i18n::t(language, Key::ReviewWaiting)))
                    .into_element();
            }
            if let Some(reason) = state.review_unavailable.get() {
                return unavailable(state.clone(), language, reason).into_element();
            }
            if session.get().cancelled {
                return cancelled(state.clone(), language).into_element();
            }
            empty(language).into_element()
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
    let content_empty = review.sections.is_empty()
        && review.findings_anchored.is_empty()
        && review.questions_detail.is_empty();
    if content_empty {
        if review.failure.is_some() || !review.status.is_empty() {
            return failure_block(state, language, &review).into_element();
        }
        return empty(language).into_element();
    }
    let emitters = emitters().get();
    let order = priority_order(&review.questions_detail);
    let stale = review.stale;
    let mut column = view()
        .flex_col()
        .flex_1()
        .min_h(px(0.0))
        .w_full()
        .gap(px(SPACE_XL));
    if stale {
        column = column.child(stale_banner(state.clone(), language));
    }
    if review.partial {
        column = column.child(partial_banner(language));
    }
    column = column.child(intent(language, session, &review, &emitters, stale));
    if let Some(docked) = docked(state.clone(), language, session, &review, &emitters, stale) {
        column = column.child(docked);
    }
    if let Some(questions) = questions(
        state.clone(),
        language,
        session,
        &review,
        &order,
        &emitters,
        stale,
    ) {
        column = column.child(questions);
    }
    column
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
    let headline = match review.failure {
        Some(ReviewFailure::Timeout | ReviewFailure::Malformed | ReviewFailure::Other) => {
            text(headline)
                .font_family(tokens::FONT_UI)
                .text_size(px(TYPE_UI))
                .line_height(px(TYPE_UI_LINE))
                .text_color(move || tokens::palette().danger)
                .into_element()
        }
        _ => prims::ink(headline).into_element(),
    };
    let mut block = view()
        .test_id("review-failure")
        .pt(px(SPACE_XL))
        .w_full()
        .flex_col()
        .gap(px(SPACE_3))
        .child(headline);
    if let Some(detail) = detail {
        block = block.child(prims::faint(detail));
    }
    block.child(prims::primary_action("review-again", action, move || {
        again.start_review()
    }))
}

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

fn partial_banner(language: Language) -> impl IntoElement {
    view()
        .test_id("review-partial")
        .w_full()
        .flex_col()
        .gap(px(SPACE_1))
        .child(prims::ink(i18n::t(language, Key::ReviewPartial)))
        .child(prims::faint(s(S::PartialHint, language)))
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
        .test_id("review-intent")
        .w_full()
        .flex_col()
        .gap(px(SPACE_2))
        .pt(px(if stale || review.partial {
            0.0
        } else {
            SPACE_4
        }))
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
) -> Option<AnyElement> {
    let selected = session.get();
    let (index, finding) = selected.focus.and_then(|index| {
        review
            .findings_anchored
            .get(index)
            .map(|finding| (index, finding.clone()))
    })?;
    if !selected.showing_finding {
        return None;
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
                    .child(
                        view()
                            .test_id("review-finding-kind")
                            .child(prims::rest(finding_kind(&finding, language))),
                    )
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
    Some(block.into_element())
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
) -> Option<AnyElement> {
    if review.questions_detail.is_empty() {
        return None;
    }
    let session_now = session.get();
    let current = current_question(&session_now, order);
    let mut block = view()
        .test_id("review-questions")
        .w_full()
        .flex_col()
        .gap(px(SPACE_3));
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
    if !others.is_empty() {
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
                    let row = view()
                        .test_id("review-question-line")
                        .w_full()
                        .child(cooled_row(
                            &text, priority, heat, language, stale, answered, skipped,
                        ));
                    if stale {
                        return row.into_element();
                    }
                    let reopen = {
                        let order = order.to_vec();
                        move || reopen_question(session, &order, index)
                    };
                    // A pressable wrapper would add an unnamed UIA button.
                    prims::press_feedback(
                        row.role(AccessibilityRole::Button)
                            .optional_label(Some(SharedString::from(text)))
                            .cursor_pointer()
                            .focus_visible(|style| style.text_color(|| tokens::palette().ink))
                            .on_press(reopen),
                    )
                    .into_element()
                })),
        );
    }
    Some(block.into_element())
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
                    tokens::adaptive_color(|palette| palette.ember),
                    tokens::adaptive_color(|palette| palette.ember),
                    tokens::adaptive_color(|palette| palette.hot),
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
                state.source_editor_focused,
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
    ignite_hold(question_heat(&emitters, order, index));
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
        ignite_hold(question_heat(&emitters, order, next));
    }
    // The next thing to do takes the focus: the next answer field, or
    // `Prepare revision` once every question is done.
    focus_request().set(Some(match next {
        Some(index) => Focus::Answer(index),
        None => Focus::Prepare,
    }));
}

// ─── the footer ──────────────────────────────────────────────────────────

fn findings_stepper(
    state: &AppState,
    session: Signal<Session>,
    language: Language,
    findings: usize,
) -> AnyElement {
    let mut row = view()
        .test_id("review-findings-stepper")
        .flex_row()
        .items_center()
        .gap(px(SPACE_2))
        .child(
            view()
                .test_id("review-findings-count")
                .child(tokens::mono_count(prims::rest(step_hint(
                    findings, language,
                )))),
        );
    if findings > 0 {
        row = row
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
            .child(prims::rest(s(S::StepThrough, language)));
    }
    row.into_element()
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
    // A pressable wrapper would add an unnamed UIA button.
    prims::press_feedback(
        view()
            .test_id(id)
            .role(AccessibilityRole::Button)
            .optional_label(Some(SharedString::from(s(label, language))))
            .cursor_pointer()
            .focus_visible(|style| style.text_color(|| tokens::palette().ink))
            .on_press(step),
    )
    .child(prims::kbd(key))
    .into_element()
}

/// The legend, and the state's one next action once every question is
/// answered or skipped. A stale result offers no `Prepare revision`: the
/// banner's `Review again` is its only next step.
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
    let stale = review.stale;
    let prepare_state = state.clone();
    let prepare_session = session;
    let close_state = state.clone();
    view()
        .test_id("review-footer")
        .w_full()
        .flex_col()
        .gap(px(SPACE_3))
        .pb(px(SPACE_2))
        .child(
            view()
                .test_id("review-legend")
                .w_full()
                .flex_col()
                .gap(px(SPACE_2))
                .child(
                    view()
                        .flex_row()
                        .items_center()
                        .flex_wrap()
                        .gap(px(SPACE_4))
                        .child(
                            view()
                                .test_id("review-legend-stated")
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
                                .test_id("review-legend-inferred")
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
                .test_id("review-actions")
                .w_full()
                .flex_row()
                .items_center()
                .gap(px(SPACE_2))
                .child(if stale {
                    view().into_element()
                } else if done {
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
                        move || {
                            prepare_session.update(|session| {
                                session.showing_finding_before_revision = None;
                            });
                            prepare_state.start_revision();
                        },
                    )
                    .element_ref(&reference)
                    .into_element()
                } else {
                    prims::quiet_action(
                        "review-prepare",
                        s(S::PrepareRevision, language),
                        move || prepare_revision(&prepare_state, prepare_session),
                    )
                    .into_element()
                })
                .child(prims::quiet_action(
                    "review-close",
                    s(S::Close, language),
                    move || {
                        close_state.close_review();
                        close_state.focus_editor();
                    },
                )),
        )
}

/// The 2px edge and haze an inferred finding shows in the column.
fn haze_swatch() -> impl IntoElement {
    view()
        .test_id("review-legend-inferred-swatch")
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
        .pt(px(SPACE_XL))
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
        .pt(px(SPACE_XL))
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

fn empty(language: Language) -> impl IntoElement {
    view()
        .test_id("review-empty")
        .w_full()
        .pt(px(SPACE_XL))
        .child(
            view()
                .test_id("review-empty-copy")
                .child(prims::rest(s(S::Empty, language))),
        )
}

/// Why Review cannot run, with the one way forward that can help.
fn unavailable(state: AppState, language: Language, reason: ReviewUnavailable) -> impl IntoElement {
    let action_state = state.clone();
    let retry_state = state.clone();
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
    let mut actions =
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
            ));
    if reason == ReviewUnavailable::NotSetUp {
        actions = actions.child(prims::quiet_action(
            "review-again",
            s(S::TryAgain, language),
            move || retry_state.start_review(),
        ));
    }
    view()
        .test_id("review-unavailable")
        .pt(px(SPACE_XL))
        .w_full()
        .flex_col()
        .gap(px(SPACE_3))
        .child(prims::ink(headline))
        .child(prims::rest(hint))
        .child(actions.child(prims::quiet_action(
            "review-close",
            s(S::Close, language),
            move || {
                close.close_review();
                close.focus_editor();
            },
        )))
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
    PartialHint,
    Empty,
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
            S::Stale => "Text changed since this review",
            S::NoModelSetUp => "No model set up",
            S::NoModelHint => {
                "Choose a wire format and enter an API credential in Settings, then run Review again."
            }
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
            S::CouldNotRead => {
                "The model returned an invalid response. Nothing was changed — try again."
            }
            S::PartialHint => {
                "A supporting file of the package was not sent; this result does not cover it."
            }
            S::Empty => "No review result yet. Run Review to inspect this document.",
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
            S::Stale => "审查后文本已更改",
            S::NoModelSetUp => "未配置模型",
            S::NoModelHint => "请在“设置”中选择协议格式并配置 API 凭据，然后重新运行审查。",
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
            S::CouldNotRead => "模型返回了无效响应，文档未做任何更改——请重试。",
            S::PartialHint => "包中有支持文件未发送；此结果未涵盖该文件。",
            S::Empty => "暂无审查结果。运行“审查”以检查此文档。",
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

fn step_hint(findings: usize, language: Language) -> String {
    match language {
        Language::English => match findings {
            0 => "No findings".to_owned(),
            1 => "1 finding".to_owned(),
            _ => format!("{findings} findings"),
        },
        Language::Chinese => match findings {
            0 => "没有发现".to_owned(),
            _ => format!("{findings} 个发现"),
        },
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
fn drained(stale: bool) -> Option<AdaptiveColor> {
    stale.then(|| tokens::adaptive_color(|palette| palette.afterimage_ink))
}

/// The model's voice: 22/30, and 1.75 line height for Chinese. With
/// `drained`, the colour is the afterimage's neutral ink instead of heat.
fn voice(
    content: impl IntoReactive<SharedString>,
    heat: Heat,
    language: Language,
    drained: Option<AdaptiveColor>,
) -> impl IntoElement {
    #[cfg(test)]
    let optics = global::<Option<tokens::CjkOptics>>()
        .get_untracked()
        .unwrap_or(tokens::CJK_OPTICS);
    #[cfg(not(test))]
    let optics = tokens::CJK_OPTICS;
    let (ink, ember, hot) = match drained {
        Some(afterimage) => (afterimage, afterimage, afterimage),
        None => (
            tokens::adaptive_color(|palette| palette.ink),
            tokens::adaptive_color(|palette| palette.ember),
            tokens::adaptive_color(|palette| palette.hot),
        ),
    };
    prims::script_text(content, TYPE_MODEL, WEIGHT_MODEL, optics)
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
    let (ink, ember, hot) = if drained {
        let afterimage = tokens::adaptive_color(|palette| palette.afterimage_ink);
        (afterimage, afterimage, afterimage)
    } else {
        (
            tokens::adaptive_color(|palette| palette.ink),
            tokens::adaptive_color(|palette| palette.ember),
            tokens::adaptive_color(|palette| palette.hot),
        )
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
    editor_focused: Signal<bool>,
    submit: impl Fn() + 'static,
) -> impl ParentElement {
    // The field takes the focus when its state is entered — never on a plain
    // rebuild, which would take it from `J`/`K` (or the stepper) on every
    // step — and never while the user is typing in the source editor.
    let reference = element_ref();
    let focus_reference = reference;
    effect(move || {
        if focus_request().get() == Some(Focus::Answer(index)) {
            focus_request().set(None);
            if editor_focused.get_untracked() {
                return;
            }
            focus_soon(focus_reference);
        }
    });
    view()
        .test_id("review-answer")
        .w_full()
        .p(px(FOCUS_RING_OFFSET))
        .border(px(FOCUS_RING))
        .border_color(Color::TRANSPARENT)
        .focus_within(|style| style.border_color(|| tokens::palette().ink))
        .child(
            input()
                .test_id("review-answer-input")
                .label(label)
                .element_ref(&reference)
                .value(answer)
                .on_change(move |text: &str| answer.set(text.to_owned()))
                .on_submit(move |_: &str| submit())
                .placeholder(s(S::TypeYourAnswer, language))
                .font_family(tokens::FONT_INPUT)
                .text_size(px(TYPE_UI))
                .line_height(px(TYPE_UI_LINE))
                .h(px(40.0))
                .px(px(SPACE_3))
                .bg(move || tokens::palette().room)
                .border(px(tokens::HAIRLINE))
                .border_color(move || with_alpha(tokens::palette().ink, 0.16))
                .focus_visible(|style| style.border_color(|| tokens::palette().ink)),
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
            S::PartialHint,
            S::Empty,
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
        assert_eq!(step_hint(0, Language::English), "No findings");
        assert_eq!(step_hint(1, Language::English), "1 finding");
        assert_eq!(step_hint(4, Language::English), "4 findings");
    }

    #[test]
    fn an_idle_review_field_has_an_empty_state() {
        let mut app = Headless::start(EngineConfig::new(Root::of::<EmptyReviewProbe>()))
            .expect("start the empty review probe");
        app.wait_for("id:review-empty-copy")
            .expect("the empty state copy");
        let nodes = app.screen().layout_nodes();
        assert!(
            nodes
                .iter()
                .any(|node| node.test_id.as_deref() == Some("review-empty"))
                && nodes.iter().any(|node| {
                    node.text
                        .as_ref()
                        .is_some_and(|text| text.text.text == s(S::Empty, Language::English))
                }),
            "the idle review field names its empty state"
        );
    }

    #[test]
    fn an_empty_review_result_uses_the_empty_state() {
        let mut app = Headless::start(EngineConfig::new(Root::of::<EmptyReviewResultProbe>()))
            .expect("start the empty result probe");
        app.wait_for("id:review-empty-copy")
            .expect("the empty state copy");
        let nodes = app.screen().layout_nodes();
        assert!(
            nodes
                .iter()
                .any(|node| node.test_id.as_deref() == Some("review-empty"))
                && nodes.iter().any(|node| {
                    node.text
                        .as_ref()
                        .is_some_and(|text| text.text.text == s(S::Empty, Language::English))
                })
                && !nodes
                    .iter()
                    .any(|node| node.test_id.as_deref() == Some("review-prepare")),
            "a content-free result shows the empty state without revision actions"
        );
    }

    #[test]
    fn zero_findings_hide_the_step_hint_and_controls() {
        let mut app = Headless::start(EngineConfig::new(Root::of::<ZeroFindingsStepperProbe>()))
            .expect("start the zero-findings probe");
        app.wait_for("id:review-findings-count")
            .expect("the zero count");
        let nodes = app.screen().layout_nodes();
        assert!(
            nodes.iter().any(|node| {
                node.text
                    .as_ref()
                    .is_some_and(|text| text.text.text == "No findings")
            }) && !nodes.iter().any(|node| {
                matches!(
                    node.test_id.as_deref(),
                    Some("review-findings-step" | "review-findings-back")
                ) || node
                    .text
                    .as_ref()
                    .is_some_and(|text| text.text.text == "to step through")
            }),
            "zero findings keep the count and hide J/K stepping"
        );
    }

    #[test]
    fn findings_count_uses_the_mono_family() {
        let mut app = Headless::start(EngineConfig::new(Root::of::<ReviewLayoutProbe>()))
            .expect("start the review layout probe");
        app.wait_for("id:review-findings-count")
            .expect("the findings count");
        assert_eq!(
            app.screen()
                .span_style("id:review-findings-count >> text:1 finding")
                .expect("the count text")
                .font_family
                .expect("the count font")
                .name()
                .as_ref(),
            tokens::FONT_SOURCE
        );
    }

    #[test]
    fn review_voice_emits_model_styles_for_latin_and_cjk() {
        let mut app = Headless::start(EngineConfig::new(
            Root::of::<ReviewVoiceProbe>().tokens(tokens::theme()),
        ))
        .expect("start the Review voice probe");
        app.wait_for("text:Review voice: Latin, 中文.")
            .expect("the mixed-script model voice");

        for (selector, expected) in [
            (
                "text:Latin",
                (
                    TYPE_MODEL,
                    TYPE_MODEL_LINE,
                    WEIGHT_MODEL.0,
                    tokens::FONT_MODEL,
                ),
            ),
            (
                "text:中文",
                (
                    TYPE_MODEL + 1.0,
                    TYPE_MODEL_LINE,
                    WEIGHT_MODEL.0 + 100,
                    tokens::FONT_UI,
                ),
            ),
        ] {
            let style = app.screen().span_style(selector).expect(selector);
            assert_eq!(
                (
                    style.font_size.0,
                    style.line_height.0,
                    style.font_weight,
                    style.font_family.unwrap().name().to_string(),
                ),
                (expected.0, expected.1, expected.2, expected.3.to_owned(),),
                "{selector}"
            );
        }
    }

    #[test]
    fn production_cjk_optics_step_the_size_and_weight_of_cjk_only() {
        let optics = tokens::CJK_OPTICS;
        assert_eq!(optics.size(TYPE_MODEL, false), TYPE_MODEL);
        assert_eq!(optics.weight(WEIGHT_MODEL, false), WEIGHT_MODEL);
        assert_eq!(optics.size(TYPE_MODEL, true), TYPE_MODEL + 1.0);
        assert_eq!(optics.weight(WEIGHT_MODEL, true).0, WEIGHT_MODEL.0 + 100);
        assert!(optics.changes_weight());
    }

    #[test]
    fn review_sections_share_left_edges_without_large_gaps() {
        let mut app = Headless::start(EngineConfig::new(Root::of::<ReviewLayoutProbe>()))
            .expect("start the review layout probe");
        app.wait_for("id:review-findings-step")
            .expect("the findings stepper");

        let intent_label = s(S::Understood, Language::English);
        let legend_label = s(S::WordsSwatch, Language::English);
        let question_priority = priority_label(ClarificationPriority::Critical, Language::English);
        let action = VisibleContent::Control("review-prepare");
        let initial_sections_fit =
            section_gaps_fit(&app, &["review-intent", "review-legend", "review-actions"]);
        let initial_nodes = app.screen().layout_nodes();
        let legend_wraps_left = match (
            text_position(&app, intent_label),
            text_position(&app, legend_label),
            content_position(
                &app,
                VisibleContent::Control("review-legend-inferred-swatch"),
            ),
        ) {
            (Some((intent_x, _)), Some((stated_x, stated_y)), Some((inferred_x, inferred_y))) => {
                (stated_x - intent_x).abs() <= 0.5
                    && (inferred_x - intent_x).abs() <= 0.5
                    && inferred_y - stated_y > 0.5
            }
            _ => false,
        };
        let empty_sections_omitted = !initial_nodes.iter().any(|node| {
            matches!(
                node.test_id.as_deref(),
                Some("review-docked" | "review-questions")
            )
        });
        let initial_content_lefts = content_lefts(
            &app,
            &[
                VisibleContent::Text(intent_label),
                VisibleContent::Text(legend_label),
                action,
            ],
        );

        app.press("id:layout-show-questions")
            .expect("show a question section");
        app.wait_for("id:review-questions")
            .expect("the questions section");
        let question_content_lefts = content_lefts(
            &app,
            &[
                VisibleContent::Text(intent_label),
                VisibleContent::Text(question_priority),
                VisibleContent::Text(legend_label),
                action,
            ],
        );
        let question_sections_fit = section_gaps_fit(
            &app,
            &[
                "review-intent",
                "review-questions",
                "review-legend",
                "review-actions",
            ],
        );

        app.press("id:review-findings-step")
            .expect("show the selected finding");
        app.settle();
        let stepped_sections_fit = section_gaps_fit(
            &app,
            &[
                "review-intent",
                "review-docked",
                "review-questions",
                "review-legend",
                "review-actions",
            ],
        );
        let stepped_content_lefts = content_lefts(
            &app,
            &[
                VisibleContent::Text(intent_label),
                VisibleContent::Control("review-finding-kind"),
                VisibleContent::Text(question_priority),
                VisibleContent::Text(legend_label),
                action,
            ],
        );
        let nodes = app.screen().layout_nodes();
        let stepped_sections_present = nodes
            .iter()
            .any(|node| node.test_id.as_deref() == Some("review-docked"))
            && nodes
                .iter()
                .any(|node| node.test_id.as_deref() == Some("review-questions"));
        assert!(
            content_lefts_align(&initial_content_lefts),
            "intent, legend words, and Prepare left edges: {initial_content_lefts:?}"
        );
        assert!(
            content_lefts_align(&question_content_lefts),
            "intent, question, legend words, and Prepare left edges: {question_content_lefts:?}"
        );
        assert!(
            content_lefts_align(&stepped_content_lefts),
            "intent, finding kind, question, legend words, and Prepare left edges: {stepped_content_lefts:?}"
        );
        assert!(
            initial_sections_fit
                && legend_wraps_left
                && empty_sections_omitted
                && question_sections_fit
                && stepped_sections_fit
                && stepped_sections_present,
            "section gaps are at most {SPACE_XL}px; the legend wraps at its visible left edge"
        );
    }

    #[test]
    fn review_section_gaps_include_notice_padding_in_each_state() {
        let mut app = Headless::start(EngineConfig::new(Root::of::<ReviewSpacingProbe>()))
            .expect("start the review spacing probe");
        app.wait_for("id:review-intent")
            .expect("the intent section");
        let intent_label = s(S::Understood, Language::English);
        let partial_label = i18n::t(Language::English, Key::ReviewPartial);
        let partial_hint = s(S::PartialHint, Language::English);
        let legend_label = s(S::StatedInTheText, Language::English);
        let mut gaps = Vec::new();
        gaps.push((
            "empty result: intent to legend",
            vertical_gap(
                section_extent(&app, "review-intent"),
                text_extent(&app, legend_label),
            ),
        ));

        app.press("id:spacing-questions")
            .expect("show a question section");
        app.wait_for("id:review-questions")
            .expect("question section");
        gaps.push((
            "intent to question",
            vertical_gap(
                section_extent(&app, "review-intent"),
                section_extent(&app, "review-questions"),
            ),
        ));
        gaps.push((
            "question to legend",
            vertical_gap(
                section_extent(&app, "review-questions"),
                section_extent(&app, "review-legend"),
            ),
        ));

        app.press("id:spacing-partial").expect("show partial state");
        app.wait_for("id:review-partial")
            .expect("partial notice detail");
        gaps.push((
            "partial notice to intent",
            vertical_gap(
                text_extent(&app, partial_hint),
                text_extent(&app, intent_label),
            ),
        ));
        gaps.push((
            "partial result sections",
            vertical_gap(
                section_extent(&app, "review-intent"),
                text_extent(&app, legend_label),
            ),
        ));

        app.press("id:spacing-stale").expect("show stale state");
        app.wait_for("id:review-stale").expect("stale notice");
        gaps.push((
            "stale notice to intent",
            vertical_gap(
                section_extent(&app, "review-stale"),
                text_extent(&app, intent_label),
            ),
        ));
        gaps.push((
            "stale result sections",
            vertical_gap(
                section_extent(&app, "review-intent"),
                text_extent(&app, legend_label),
            ),
        ));

        app.press("id:spacing-both")
            .expect("show both notice states");
        app.wait_for("id:review-partial")
            .expect("partial notice beside stale notice");
        gaps.push((
            "stale notice to partial notice",
            vertical_gap(
                section_extent(&app, "review-stale"),
                text_extent(&app, partial_label),
            ),
        ));
        gaps.push((
            "partial notice to intent",
            vertical_gap(
                text_extent(&app, partial_hint),
                text_extent(&app, intent_label),
            ),
        ));
        gaps.push((
            "combined result sections",
            vertical_gap(
                section_extent(&app, "review-intent"),
                text_extent(&app, legend_label),
            ),
        ));

        app.press("id:spacing-findings")
            .expect("show a result with findings");
        app.wait_for("id:review-findings-step")
            .expect("finding step control");
        app.press("id:review-findings-step")
            .expect("show the selected finding");
        app.wait_for("id:review-docked").expect("docked finding");
        gaps.push((
            "intent to finding",
            vertical_gap(
                section_extent(&app, "review-intent"),
                section_extent(&app, "review-docked"),
            ),
        ));
        gaps.push((
            "finding to legend",
            vertical_gap(
                section_extent(&app, "review-docked"),
                section_extent(&app, "review-legend"),
            ),
        ));
        let over_limit: Vec<_> = gaps
            .iter()
            .copied()
            .filter(|(_, gap)| !(-0.5..=SPACE_XL + 0.5).contains(gap))
            .collect();
        assert!(
            over_limit.is_empty(),
            "visible section gaps are at most {SPACE_XL}px: {over_limit:?}; measured={gaps:?}"
        );
    }

    #[test]
    fn prepare_revision_and_close_share_the_action_row() {
        let mut app = Headless::start(EngineConfig::new(Root::of::<ReviewLayoutProbe>()))
            .expect("start the review layout probe");
        app.wait_for("id:review-prepare")
            .expect("the prepare action");
        let nodes = app.screen().layout_nodes();
        let node = |id: &str| {
            nodes
                .iter()
                .find(|node| node.test_id.as_deref() == Some(id))
                .unwrap_or_else(|| panic!("the `{id}` control is laid out"))
        };
        let row = node("review-actions");
        let prepare = node("review-prepare");
        let close = node("review-close");
        let prepare_center = prepare.bounds.origin.y.0 + prepare.bounds.size.height.0 / 2.0;
        let close_center = close.bounds.origin.y.0 + close.bounds.size.height.0 / 2.0;
        assert!(
            prepare.parent == row.id
                && close.parent == row.id
                && (prepare_center - close_center).abs() <= 0.5
                && prepare.bounds.origin.x.0 < close.bounds.origin.x.0,
            "prepare and close are adjacent actions in one row"
        );
    }

    #[test]
    fn review_failures_use_danger_while_cancellation_stays_neutral() {
        let mut app = Headless::start(EngineConfig::new(Root::of::<FailureColorProbe>()))
            .expect("start the review failure probe");
        app.set_environment("light mode", |env| {
            env.color_scheme = nsh_core::platform::ColorScheme::Light
        })
        .expect("light mode");
        app.wait_for("text:Provider failed")
            .expect("the provider failure");
        for (selector, color) in [
            ("text:/^Provider failed$/", tokens::LIGHT.danger),
            ("text:/^Cancelled — nothing changed\\.$/", tokens::LIGHT.ink),
        ] {
            assert_eq!(
                app.screen().span_style(selector).expect(selector).color,
                color
            );
        }
    }

    /// The document the camera probes review: three known lines.
    fn camera_fixture() -> &'static std::path::Path {
        static FIXTURE: std::sync::LazyLock<(tempfile::TempDir, std::path::PathBuf)> =
            std::sync::LazyLock::new(|| {
                let dir = tempfile::tempdir().expect("temp dir");
                let path = dir.path().join("camera.md");
                let mut text = String::from("alpha\nbeta\ngamma\n");
                for line in 3..64 {
                    text.push_str(&format!("line {line}\n"));
                }
                std::fs::write(&path, text).expect("write the fixture");
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

    fn layout_review() -> ReviewView {
        ReviewView {
            status: "Review ready".to_owned(),
            sections: vec![
                crate::state::ReviewSection {
                    title: "Intent".to_owned(),
                    items: vec!["The document's intent".to_owned()],
                },
                crate::state::ReviewSection {
                    title: "Scope".to_owned(),
                    items: vec!["A detail".to_owned()],
                },
            ],
            findings_anchored: vec![ReviewFindingView {
                stated: false,
                text: "One finding".to_owned(),
                location: None,
            }],
            ..ReviewView::default()
        }
    }

    fn spacing_review(partial: bool, stale: bool, with_findings: bool) -> ReviewView {
        let mut review = layout_review();
        review.partial = partial;
        review.stale = stale;
        if !with_findings {
            review.findings_anchored.clear();
        }
        review
    }

    fn with_question(mut review: ReviewView) -> ReviewView {
        review.questions_detail.push(question(
            ClarificationPriority::Critical,
            "Clarify the expected outcome",
        ));
        review
    }

    fn review_spacing_control(
        id: &'static str,
        state: &AppState,
        partial: bool,
        stale: bool,
        with_findings: bool,
    ) -> impl IntoElement {
        let state = state.clone();
        view().test_id(id).w(px(8.0)).h(px(8.0)).on_press(move || {
            state
                .review_view
                .set(Some(spacing_review(partial, stale, with_findings)))
        })
    }

    fn review_state_control(
        id: &'static str,
        state: &AppState,
        review: ReviewView,
    ) -> impl IntoElement {
        let state = state.clone();
        view()
            .test_id(id)
            .w(px(8.0))
            .h(px(8.0))
            .on_press(move || state.review_view.set(Some(review.clone())))
    }

    #[component]
    fn ReviewVoiceProbe() -> impl IntoElement {
        global::<Option<tokens::CjkOptics>>()
            .set(Some(tokens::CJK_OPTICS_CAPTURE_VARIANTS[2].optics));
        let state = AppState::new();
        state.review_view.set(Some(ReviewView {
            sections: vec![crate::state::ReviewSection {
                title: "Intent".to_owned(),
                items: vec!["Review voice: Latin, 中文.".to_owned()],
            }],
            ..ReviewView::default()
        }));
        view()
            .w(px(760.0))
            .h(px(720.0))
            .child(field(state, Language::English))
    }

    #[component]
    fn ReviewLayoutProbe() -> impl IntoElement {
        let state = AppState::new();
        state.review_view.set(Some(layout_review()));
        let question_state = with_question(layout_review());
        view()
            .test_id("review-layout-probe")
            .flex_col()
            .w(px(220.0))
            .h(px(720.0))
            .child(review_state_control(
                "layout-show-questions",
                &state,
                question_state,
            ))
            .child(field(state, Language::English))
    }

    #[component]
    fn ReviewSpacingProbe() -> impl IntoElement {
        let state = AppState::new();
        state
            .review_view
            .set(Some(spacing_review(false, false, false)));
        view()
            .flex_col()
            .w(px(760.0))
            .h(px(720.0))
            .child(
                view()
                    .flex_row()
                    .child(review_spacing_control(
                        "spacing-partial",
                        &state,
                        true,
                        false,
                        false,
                    ))
                    .child(review_spacing_control(
                        "spacing-stale",
                        &state,
                        false,
                        true,
                        false,
                    ))
                    .child(review_spacing_control(
                        "spacing-both",
                        &state,
                        true,
                        true,
                        false,
                    ))
                    .child(review_spacing_control(
                        "spacing-findings",
                        &state,
                        false,
                        false,
                        true,
                    ))
                    .child(review_state_control(
                        "spacing-questions",
                        &state,
                        with_question(spacing_review(false, false, false)),
                    )),
            )
            .child(field(state, Language::English))
    }

    #[component]
    fn EmptyReviewProbe() -> impl IntoElement {
        let state = AppState::new();
        state.review_open.set(true);
        view()
            .flex_col()
            .w(px(760.0))
            .h(px(720.0))
            .child(field(state, Language::English))
    }

    #[component]
    fn EmptyReviewResultProbe() -> impl IntoElement {
        let state = AppState::new();
        state.review_view.set(Some(ReviewView::default()));
        view()
            .flex_col()
            .w(px(760.0))
            .h(px(720.0))
            .child(field(state, Language::English))
    }

    #[component]
    fn ZeroFindingsStepperProbe() -> impl IntoElement {
        let state = AppState::new();
        view()
            .test_id("zero-findings-stepper-probe")
            .child(findings_stepper(&state, session(), Language::English, 0))
    }

    #[component]
    fn FailureColorProbe() -> impl IntoElement {
        let state = AppState::new();
        let failed = ReviewView {
            status: "Provider failed".to_owned(),
            diagnostic: Some("The request failed.".to_owned()),
            failure: Some(ReviewFailure::Other),
            ..ReviewView::default()
        };
        let cancelled = ReviewView {
            failure: Some(ReviewFailure::Cancelled),
            ..ReviewView::default()
        };
        view()
            .flex_col()
            .w(px(760.0))
            .h(px(720.0))
            .child(failure_block(state.clone(), Language::English, &failed))
            .child(failure_block(state, Language::English, &cancelled))
    }

    #[derive(Clone, Copy)]
    enum VisibleContent<'a> {
        Text(&'a str),
        Control(&'a str),
    }

    fn section_gaps_fit(app: &Headless, ids: &[&str]) -> bool {
        let nodes = app.screen().layout_nodes();
        let mut sections = Vec::new();
        for id in ids {
            let Some(bounds) = nodes
                .iter()
                .find(|node| node.test_id.as_deref() == Some(id))
                .map(|node| node.bounds)
            else {
                return false;
            };
            sections.push((bounds.origin.y.0, bounds.size.height.0));
        }
        sections
            .windows(2)
            .all(|pair| pair[1].0 - pair[0].0 - pair[0].1 <= SPACE_XL)
    }

    fn content_lefts(app: &Headless, contents: &[VisibleContent<'_>]) -> Vec<Option<f32>> {
        contents
            .iter()
            .map(|content| content_position(app, *content).map(|position| position.0))
            .collect()
    }

    fn content_lefts_align(positions: &[Option<f32>]) -> bool {
        let Some(Some(left)) = positions.first() else {
            return false;
        };
        positions.len() >= 2
            && positions
                .iter()
                .all(|x| x.as_ref().is_some_and(|x| (*x - *left).abs() <= 0.5))
    }

    fn content_position(app: &Headless, content: VisibleContent<'_>) -> Option<(f32, f32)> {
        let nodes = app.screen().layout_nodes();
        let node = match content {
            VisibleContent::Text(content) => nodes.iter().find(|node| {
                node.text
                    .as_ref()
                    .is_some_and(|text| text.text.text == content)
            }),
            VisibleContent::Control(id) => nodes
                .iter()
                .find(|node| node.test_id.as_deref() == Some(id)),
        }?;
        Some((node.bounds.origin.x.0, node.bounds.origin.y.0))
    }

    fn text_position(app: &Headless, content: &str) -> Option<(f32, f32)> {
        content_position(app, VisibleContent::Text(content))
    }

    fn section_extent(app: &Headless, id: &str) -> (f32, f32) {
        let nodes = app.screen().layout_nodes();
        let bounds = nodes
            .iter()
            .find(|node| node.test_id.as_deref() == Some(id))
            .unwrap_or_else(|| panic!("the `{id}` section is laid out"))
            .bounds;
        (bounds.origin.y.0, bounds.origin.y.0 + bounds.size.height.0)
    }

    fn text_extent(app: &Headless, content: &str) -> (f32, f32) {
        let nodes = app.screen().layout_nodes();
        let bounds = nodes
            .iter()
            .find(|node| {
                node.text
                    .as_ref()
                    .is_some_and(|text| text.text.text == content)
            })
            .unwrap_or_else(|| panic!("the text `{content}` is laid out"))
            .bounds;
        (bounds.origin.y.0, bounds.origin.y.0 + bounds.size.height.0)
    }

    fn vertical_gap(after: (f32, f32), before: (f32, f32)) -> f32 {
        before.0 - after.1
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

    #[component]
    fn KeyboardCameraProbe() -> impl IntoElement {
        let state = AppState::new();
        state.open_file(camera_fixture().to_path_buf());
        state.review_open.set(true);
        load_result(state.clone());
        keys(&state, session());
        view()
            .test_id("keyboard-camera-probe")
            .flex_col()
            .w(px(760.0))
            .h(px(700.0))
            .child(camera_tab(&state))
            .child(camera_request(&state))
            .child(crate::ui::document::column(state, Language::English))
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

    /// The text of the node `id` on `screen`, if it is drawn.
    fn text_on(screen: &nsh_testing::Screen, id: &str) -> Option<String> {
        screen
            .layout_nodes()
            .iter()
            .find(|node| node.test_id.as_deref() == Some(id))
            .and_then(|node| node.text.as_ref().map(|text| text.text.text.to_string()))
    }

    /// The text of the node `id`, once the probe renders it.
    fn text_of(app: &Headless, id: &str) -> String {
        text_on(app.screen(), id)
            .unwrap_or_else(|| panic!("no `{id}` text on screen: {:?}", app.screen().texts()))
    }

    /// Waits until the node `id` reads a text that `holds`: the fixture's
    /// file loads on a blocking thread, in its own time.
    fn wait_for_text(app: &mut Headless, id: &str, what: &str, holds: impl Fn(&str) -> bool) {
        app.wait_until(what, Duration::from_secs(5), |screen| {
            text_on(screen, id).is_some_and(|text| holds(&text))
        })
        .expect(what);
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
        wait_for_text(&mut app, "camera-tab", "the fixture's tab lands", |tab| {
            !tab.is_empty()
        });

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

    #[test]
    fn keyboard_finding_step_levels_the_line_on_the_next_frame() {
        let mut config = EngineConfig::new(Root::of::<KeyboardCameraProbe>());
        config.clock = nsh_core::time::Clock::manual(std::time::SystemTime::now());
        let mut app = Headless::start(config).expect("start the probe");
        app.set_environment("motion enabled", |env| env.reduce_motion = false)
            .expect("enable motion");
        app.wait_for("label:Markdown source")
            .expect("the source editor renders");
        app.settle();

        let tab = text_of(&app, "camera-tab");
        let tab = (!tab.is_empty())
            .then_some(tab)
            .expect("the fixture's tab opens");
        let start = camera_level_offset(&app, "alpha");

        let frame_start = app.clock().now();
        app.press_key("j").expect("step to the first finding");
        let request = text_of(&app, "camera-request");
        let elapsed = app.clock().now().saturating_duration_since(frame_start);
        if elapsed.is_zero() {
            app.next_frame(Duration::from_millis(17))
                .expect("the key's next frame");
        }

        let elapsed = app.clock().now().saturating_duration_since(frame_start);
        let offset = camera_level_offset(&app, "alpha");
        assert!(
            start.abs() > 2.0
                && request == format!("{tab}:0")
                && elapsed <= Duration::from_millis(17)
                && offset.abs() <= tokens::TYPE_SOURCE_LINE,
            "J must level its finding within one frame; start {start}, request {request}, elapsed {elapsed:?}, offset {offset}"
        );
    }

    fn camera_level_offset(app: &Headless, line: &str) -> f32 {
        let nodes = app.screen().layout_nodes();
        let editor = nodes
            .iter()
            .find(|node| node.test_id.as_deref() == Some("source-editor"))
            .expect("the source editor is laid out")
            .bounds;
        let row = nodes
            .iter()
            .find(|node| {
                node.text
                    .as_ref()
                    .is_some_and(|text| text.text.text == line)
            })
            .unwrap_or_else(|| panic!("the row `{line}` is in view"))
            .bounds;
        let row_middle = row.origin.y.0 + row.size.height.0 / 2.0;
        let editor_middle = editor.origin.y.0 + editor.size.height.0 / 2.0;
        row_middle - editor_middle
    }

    #[test]
    fn the_arrival_levels_the_first_finding() {
        let mut app = Headless::start(EngineConfig::new(Root::of::<ArrivalProbe>()))
            .expect("start the probe");
        app.wait_for("id:camera-loaded")
            .expect("the fixture's tab lands");
        // Tab ids start at 1: a request for tab 0 is the default, none yet.
        wait_for_text(
            &mut app,
            "camera-request",
            "the result watcher runs",
            |request| !request.starts_with("0:"),
        );

        let tab = text_of(&app, "camera-tab");
        assert!(!tab.is_empty(), "the fixture's tab must be open");
        assert_eq!(
            text_of(&app, "camera-request"),
            format!("{tab}:0"),
            "the arrival must level the first finding's line"
        );
    }

    /// A result with a question arrives while a consent card waits: the
    /// arrival must leave every focus request untouched.
    #[component]
    fn ConsentArrivalProbe() -> impl IntoElement {
        let state = AppState::new();
        state.open_file(camera_fixture().to_path_buf());
        state.review_open.set(true);
        focus_request().set(None);
        session().set(Session::default());
        state
            .translation_consent
            .set(Some(SharedString::from("disclosure text")));
        observe(&state, session());
        let loaded_state = state.clone();
        effect(move || {
            if loaded_state.review_view.get_untracked().is_some() {
                return;
            }
            let Some(tab) = loaded_state.active_tab() else {
                return;
            };
            let mut loaded = camera_view(tab.id);
            loaded.questions_detail =
                vec![question(ClarificationPriority::High, "Which audience?")];
            loaded_state.review_view.set(Some(loaded));
        });
        view()
            .test_id("consent-arrival-probe")
            .w(px(400.0))
            .h(px(200.0))
            .child(text(move || {
                format!("arrived: {}", session().get().view.is_some())
            }))
            .child(text(move || format!("focus: {:?}", focus_request().get())))
    }

    #[test]
    fn an_open_consent_card_leaves_the_arrival_focus_empty() {
        let mut app = Headless::start(EngineConfig::new(Root::of::<ConsentArrivalProbe>()))
            .expect("start the probe");
        app.wait_for("id:consent-arrival-probe").expect("the probe");
        app.wait_for("text:arrived: true").expect("the arrival ran");
        assert!(
            app.shows("focus: None"),
            "an open consent card took a focus request: {:?}",
            app.screen().texts()
        );
    }

    /// A resting review with two findings and controls that open and clear a
    /// consent card: the Review step keys are inert while one waits.
    #[component]
    fn ConsentGatedReviewKeysProbe() -> impl IntoElement {
        let state = AppState::new();
        state.open_file(camera_fixture().to_path_buf());
        state.review_open.set(true);
        session().set(Session::default());
        load_result(state.clone());
        let open_state = state.clone();
        let clear_state = state.clone();
        view()
            .test_id("consent-review-keys-probe")
            .flex_col()
            .w(px(400.0))
            .h(px(200.0))
            .child(
                view()
                    .test_id("probe-waiting-consent")
                    .focusable(false)
                    .on_press(move |_: &PressEvent| {
                        open_state
                            .translation_consent
                            .set(Some(SharedString::from("disclosure text")));
                    })
                    .child(prims::rest("Wait for consent")),
            )
            .child(
                view()
                    .test_id("probe-consent-gone")
                    .focusable(false)
                    .on_press(move |_: &PressEvent| clear_state.translation_consent.set(None))
                    .child(prims::rest("Drop consent")),
            )
            .child(camera_tab(&state))
            .child(camera_request(&state))
            .child(field(state.clone(), Language::English))
    }

    #[test]
    fn review_step_keys_wait_for_a_consent_card_and_come_back() {
        let mut app = Headless::start(EngineConfig::new(Root::of::<ConsentGatedReviewKeysProbe>()))
            .expect("start the probe");
        app.wait_for("id:consent-review-keys-probe")
            .expect("the probe");
        wait_for_text(&mut app, "camera-tab", "the fixture's tab lands", |tab| {
            !tab.is_empty()
        });
        let tab = text_of(&app, "camera-tab");
        wait_for_text(&mut app, "camera-request", "the arrival level", |request| {
            request == format!("{tab}:0")
        });

        // While the card waits, two steps move nothing.
        app.press("id:probe-waiting-consent")
            .expect("wait for consent");
        app.press_key("j").expect("step under the card");
        app.press_key("j").expect("step under the card again");
        app.settle();
        assert_eq!(
            text_of(&app, "camera-request"),
            format!("{tab}:0"),
            "a step under the card moved the finding"
        );

        // The card goes: the steps move the finding again.
        app.press("id:probe-consent-gone")
            .expect("drop the consent");
        app.press_key("j").expect("step with the card gone");
        app.press_key("j").expect("step with the card gone again");
        app.settle();
        assert_eq!(
            text_of(&app, "camera-request"),
            format!("{tab}:1"),
            "the steps came back"
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
        wait_for_text(&mut app, "camera-tab", "the fixture's tab lands", |tab| {
            !tab.is_empty()
        });

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
        state.new_document();
        let tab = state.active.get().expect("the probe document");
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
                    .on_press(move || arm_state.arm_review_in_flight(tab)),
            )
            .child(
                view()
                    .test_id("busy-start")
                    .w_full()
                    .h(px(24.0))
                    .on_press(move || start_state.start_review()),
            )
    }

    fn questioned_view() -> ReviewView {
        ReviewView {
            sections: vec![crate::state::ReviewSection {
                title: "Stated goal".to_owned(),
                items: vec!["one".to_owned()],
            }],
            questions_detail: vec![ReviewQuestionView {
                priority: ClarificationPriority::High,
                question: "Which one?".to_owned(),
                impact: None,
            }],
            ..ReviewView::default()
        }
    }

    /// The whole room over a document; the result arrives the moment the
    /// source editor takes the keyboard, as a slow provider's answer would.
    #[component]
    fn ArrivalDuringTypingProbe() -> impl IntoElement {
        let state = AppState::new();
        state.new_document();
        if let Some(tab) = state.active_tab() {
            tab.text.set(SharedString::from(
                "probe
",
            ));
        }
        state.review_open.set(true);
        state
            .revision_questions
            .set(vec![crate::state::RevisionQuestionView {
                ordinal: 0,
                question: "Which one?".to_owned(),
                priority: format!("{:?}", ClarificationPriority::High),
                answer: signal(String::new()),
            }]);
        let arrive = state.clone();
        effect(move || {
            if arrive.source_editor_focused.get() {
                arrive.review_view.set(Some(questioned_view()));
            }
        });
        let text_state = state.clone();
        view()
            .flex_col()
            .h(relative(1.0))
            .child(
                text(move || {
                    if text_state
                        .active_tab()
                        .is_some_and(|tab| tab.text.get().contains('q'))
                    {
                        "typed"
                    } else {
                        "untouched"
                    }
                })
                .test_id("editor-state"),
            )
            .child(crate::ui::shell::shell(
                state,
                Language::English,
                crate::ui::tokens::Layout::Wide,
            ))
    }

    #[test]
    fn an_arriving_result_does_not_steal_the_keyboard_from_the_editor() {
        let mut app = Headless::start(EngineConfig::new(Root::of::<ArrivalDuringTypingProbe>()))
            .expect("start the shell");
        app.set_viewport(Size {
            width: Points(1440.0),
            height: Points(900.0),
        })
        .expect("a 1440x900 window");
        app.first_render().expect("first render");
        app.wait_for("label:Markdown source").expect("the editor");
        app.type_text("label:Markdown source", "x")
            .expect("type in the editor");
        app.wait_for("id:review-question")
            .expect("the result arrives while the editor holds the keyboard");
        app.settle();
        app.press_key("q").expect("keep typing");
        app.wait_for("text:typed")
            .expect("the editor kept the keyboard");
        let answer = app
            .screen()
            .input_text("id:review-answer-input")
            .expect("the answer input")
            .expect("the answer field is on screen");
        assert!(
            answer.is_empty(),
            "the question did not take the character: {answer:?}"
        );
    }

    // The room during the reported sequence: a Revision is busy, a Review
    // consent card waits, and then the Revision's answer arrives, so the
    // Review field mounts after the card with a newer Escape binding.
    #[component]
    fn EscapeAfterTheCardProbe() -> impl IntoElement {
        let state = AppState::new();
        state.new_document();
        let tab = state.active_tab().expect("the tab");
        state.review_view.set(Some(ReviewView {
            status: "kept".to_owned(),
            tab_id: Some(tab.id),
            ..ReviewView::default()
        }));
        state.review_open.set(true);
        state.arm_revision_in_flight(tab.id);
        state
            .review_consent
            .set(Some(SharedString::from("disclosure text")));
        let arrive = state.clone();
        effect(move || {
            let arrive = arrive.clone();
            let _timer = set_timeout(Duration::from_millis(50), move || {
                arrive.finish_revision_in_flight(tab.id);
            });
        });
        crate::ui::shell::shell(state, Language::English, crate::ui::tokens::Layout::Wide)
    }

    #[test]
    fn escape_cancels_the_card_after_the_review_field_mounts() {
        let mut app = Headless::start(EngineConfig::new(Root::of::<EscapeAfterTheCardProbe>()))
            .expect("start the shell");
        app.set_viewport(Size {
            width: Points(1440.0),
            height: Points(900.0),
        })
        .expect("a 1440x900 window");
        app.first_render().expect("first render");
        app.wait_for("id:consent-card").expect("the card");
        // The Revision's answer arrives and the tab shows no proposal: the
        // Review field mounts after the card, its Escape binding newer.
        app.wait_for("id:review-field")
            .expect("the Review field mounted over the waiting card");
        app.press_key("escape").expect("escape");
        app.wait_for_gone("id:consent-card", Duration::from_secs(2))
            .expect("the card's cancel ran");
    }

    /// The whole room over a document; a result with no questions arrives the
    /// moment the source editor takes the keyboard.
    #[component]
    fn ArrivalWithoutQuestionsProbe() -> impl IntoElement {
        let state = AppState::new();
        state.new_document();
        if let Some(tab) = state.active_tab() {
            tab.text.set(SharedString::from("probe\n"));
        }
        state.review_open.set(true);
        let arrive = state.clone();
        effect(move || {
            if arrive.source_editor_focused.get() {
                arrive.review_view.set(Some(ReviewView {
                    sections: vec![crate::state::ReviewSection {
                        title: "Stated goal".to_owned(),
                        items: vec!["one".to_owned()],
                    }],
                    ..ReviewView::default()
                }));
            }
        });
        let text_state = state.clone();
        view()
            .flex_col()
            .h(relative(1.0))
            .child(
                text(move || {
                    if text_state
                        .active_tab()
                        .is_some_and(|tab| tab.text.get().contains('q'))
                    {
                        "typed"
                    } else {
                        "untouched"
                    }
                })
                .test_id("editor-state"),
            )
            .child(crate::ui::shell::shell(
                state,
                Language::English,
                crate::ui::tokens::Layout::Wide,
            ))
    }

    #[test]
    fn an_arriving_result_without_questions_does_not_steal_the_keyboard() {
        let mut app =
            Headless::start(EngineConfig::new(Root::of::<ArrivalWithoutQuestionsProbe>()))
                .expect("start the shell");
        app.set_viewport(Size {
            width: Points(1440.0),
            height: Points(900.0),
        })
        .expect("a 1440x900 window");
        app.first_render().expect("first render");
        app.wait_for("label:Markdown source").expect("the editor");
        app.type_text("label:Markdown source", "x")
            .expect("type in the editor");
        app.wait_for("id:review-prepare")
            .expect("the result arrives while the editor holds the keyboard");
        app.settle();
        app.press_key("q").expect("keep typing");
        app.wait_for("text:typed")
            .expect("the editor kept the keyboard");
        assert!(
            app.screen().matches("id:review-answer-input") == Ok(false),
            "no question input exists to take the keyboard"
        );
    }

    #[test]
    fn arrival_focus_never_takes_the_keyboard_from_the_editor() {
        assert_eq!(arrival_focus(false, Some(2), true, false), None);
        assert_eq!(
            arrival_focus(false, Some(2), false, false),
            Some(Focus::Answer(2))
        );
        assert_eq!(
            arrival_focus(true, Some(2), true, false),
            None,
            "a stale background result must not take the keyboard either"
        );
        assert_eq!(
            arrival_focus(true, Some(2), false, false),
            Some(Focus::ReviewAgain),
            "a stale result focuses Review again only without the editor"
        );
        assert_eq!(
            arrival_focus(false, None, true, false),
            None,
            "a background result with no questions must not take the keyboard"
        );
        assert_eq!(
            arrival_focus(false, None, false, false),
            Some(Focus::Prepare)
        );
        assert_eq!(
            arrival_focus(false, Some(2), false, true),
            None,
            "an open consent card keeps the keyboard from every arrival"
        );
        assert_eq!(
            arrival_focus(true, Some(2), false, true),
            None,
            "a card keeps the keyboard from a stale arrival too"
        );
        assert_eq!(
            arrival_focus(false, None, false, true),
            None,
            "a card keeps the keyboard from the Prepare action too"
        );
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

    /// The unavailable state with no configured model: Settings leads, and
    /// `Try again` stays reachable beside it.
    #[component]
    fn NotSetUpReviewProbe() -> impl IntoElement {
        let state = AppState::new();
        state.review_open.set(true);
        state
            .review_unavailable
            .set(Some(ReviewUnavailable::NotSetUp));
        view()
            .flex_col()
            .w(px(760.0))
            .h(px(720.0))
            .child(field(state, Language::English))
    }

    #[test]
    fn no_model_set_up_offers_try_again_and_settings() {
        let mut app = Headless::start(EngineConfig::new(Root::of::<NotSetUpReviewProbe>()))
            .expect("start the probe");
        app.wait_for("id:review-open-settings")
            .expect("the Settings action");
        app.wait_for("id:review-again")
            .expect("the Try again action");
        assert!(
            app.shows(s(S::TryAgain, Language::English)),
            "Try again is rendered"
        );
    }

    /// The whole room over a review result, for the Escape focus round-trip.
    #[component]
    fn EscapeFocusProbe() -> impl IntoElement {
        let state = AppState::new();
        state.new_document();
        if let Some(tab) = state.active_tab() {
            tab.text.set(SharedString::from("probe\n"));
        }
        state.review_view.set(Some(ReviewView::default()));
        state.review_open.set(true);
        let text_state = state.clone();
        view()
            .flex_col()
            .h(relative(1.0))
            .child(
                text(move || {
                    if text_state
                        .active_tab()
                        .is_some_and(|tab| tab.text.get().contains('q'))
                    {
                        "typed"
                    } else {
                        "untouched"
                    }
                })
                .test_id("escape-editor-state"),
            )
            .child(crate::ui::shell::shell(
                state,
                Language::English,
                crate::ui::tokens::Layout::Wide,
            ))
    }

    #[test]
    fn escape_closes_the_result_and_returns_focus_to_the_editor() {
        let mut app = Headless::start(EngineConfig::new(Root::of::<EscapeFocusProbe>()))
            .expect("start the shell");
        app.set_viewport(Size {
            width: Points(1440.0),
            height: Points(900.0),
        })
        .expect("a 1440x900 window");
        app.first_render().expect("first render");
        app.wait_for("id:review-empty").expect("the result field");
        app.press_key("escape").expect("close the review");
        app.wait_for_gone("id:review-field", Duration::from_secs(2))
            .expect("the field closed");
        app.press_key("q").expect("type into the focused editor");
        app.wait_for("text:typed")
            .expect("the source buffer receives the character");
    }
}
