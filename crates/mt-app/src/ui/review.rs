//! Owner lane: review.
//!
//! The field during review: the lens line, 'Understood: <goal>', the questions
//! (the current one HOT with its `Why it matters:` and its answer field), the
//! cooled question list, `All fields`, loading, stale and the diagnostics.
//!
//! Contract: [`is_open`] says whether the field is the review's;
//! [`field`] draws it. The shell routes to it and gives it the field's box;
//! everything inside — the docked reading position, the ignition order, the
//! stagger, the focus on the answer field — belongs here.
//!
//! What is here now is the honest skeleton: the model-voice title ignites on
//! arrival and cools to ember (the [`crate::ui::motion::Heat`] proof), the
//! state's status text while a review runs or has run, and a quiet 'Close'
//! wired to the state. The review lane adds the copy, the questions, the
//! `All fields` disclosure, `J`/`K`, and `Prepare revision`.

use mt_core::settings::Language;
use nsh_core::prelude::*;

use crate::{
    state::AppState,
    ui::{
        motion::Heat,
        prims,
        tokens::{SPACE_2, SPACE_3, SPACE_4, SPACE_6},
    },
};

/// Whether the field is showing a review.
pub fn is_open(state: &AppState) -> bool {
    state.review_open.get()
}

/// The review field.
pub fn field(state: AppState, language: Language) -> impl IntoElement {
    let _ = language;
    let status_state = state.clone();
    let close_state = state.clone();
    let heat = Heat::cold();
    // Ignite once, when the field first renders: HOT, then cooling to ember.
    effect(move || heat.ignite(0.0));
    view()
        .test_id("review-field")
        .w_full()
        .flex_col()
        .gap(px(SPACE_4))
        .child(
            view()
                .flex_col()
                .gap(px(SPACE_3))
                .child(prims::model_voice("Review", heat))
                .child(prims::bloom(heat, 6.0)),
        )
        .child(
            view()
                .flex_col()
                .gap(px(SPACE_2))
                .child(prims::rest(move || {
                    if status_state.review_busy.get() {
                        SharedString::from("Reviewing\u{2026}")
                    } else if let Some(view) = status_state.review_view.get() {
                        SharedString::from(view.status)
                    } else if status_state.review_consent.get().is_some() {
                        SharedString::from("Waiting for your consent")
                    } else {
                        SharedString::from("")
                    }
                }))
                .child(prims::faint(
                    "Understood: the goal line, the questions and the findings live here.",
                )),
        )
        .child(view().pt(px(SPACE_6)).child(prims::quiet_action(
            "review-close",
            "Close",
            move || close_state.close_review(),
        )))
        .child(view().pt(px(SPACE_3)).child(prims::kbd("J / K")))
}
