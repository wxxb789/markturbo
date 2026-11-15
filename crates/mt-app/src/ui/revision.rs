//! Owner lane: revision.
//!
//! The field during an approved revision: the proposed hunks, one at a time in
//! the docked reading position, with `Accept` / `Reject` and the decided count.
//!
//! Contract: [`is_open`] says the field belongs to the revision (it outranks
//! the review field, because a revision runs inside a review); [`field`] draws
//! it. The document lane shows the other half in the column: the removed words
//! struck cold, the accepted words settling in as ambient ink.
//!
//! What is here now: the status text, the hunk count and `Done` (which applies
//! exactly the hunks the state holds as accepted — none, until the revision
//! lane adds the per-hunk controls). The revision lane owns the accept/reject
//! keys, the counter, the base-changed wording and the extinguish-to-ink
//! transition.

use mt_core::settings::Language;
use nsh_core::prelude::*;

use crate::{
    state::AppState,
    ui::{
        prims,
        tokens::{self, SPACE_2, SPACE_3, SPACE_4, SPACE_6},
    },
};

/// Whether the field is showing a revision.
pub fn is_open(state: &AppState) -> bool {
    state.revision_busy.get()
        || state.revision_status.get().is_some()
        || !state.revision_hunks.get().is_empty()
}

/// The revision field.
pub fn field(state: AppState, language: Language) -> impl IntoElement {
    let _ = language;
    let status_state = state.clone();
    let count_state = state.clone();
    let apply_state = state.clone();
    let cancel_state = state.clone();
    view()
        .test_id("revision-field")
        .w_full()
        .flex_col()
        .gap(px(SPACE_4))
        .child(
            text("Revision")
                .font_family(tokens::FONT_MODEL)
                .font_weight(tokens::WEIGHT_MODEL)
                .text_size(px(tokens::TYPE_MODEL))
                .line_height(px(tokens::TYPE_MODEL_LINE))
                .text_color(move || tokens::palette().ink),
        )
        .child(prims::rest(move || {
            status_state
                .revision_status
                .get()
                .unwrap_or_else(|| SharedString::from("No proposed hunks"))
        }))
        .child(prims::faint(move || {
            let state = count_state.clone();
            SharedString::from(format!(
                "{} proposed hunk(s)",
                state.revision_hunks.get().len()
            ))
        }))
        .child(
            view()
                .pt(px(SPACE_6))
                .flex_row()
                .gap(px(SPACE_3))
                .child(prims::quiet_action(
                    "revision-cancel",
                    "Cancel",
                    move || cancel_state.cancel_revision(),
                ))
                .child(prims::primary_action("revision-done", "Done", move || {
                    apply_state.apply_revision()
                })),
        )
        .child(view().pt(px(SPACE_2)).child(prims::faint(
            "Old text stays in the column; proposals read here.",
        )))
}
