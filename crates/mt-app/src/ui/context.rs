//! Owner lane: context.
//!
//! Effective Agent Context in the field: the ordered sources that apply to this
//! file (global → repo → nested), their precedence, which are verified by the
//! `codex-agents-md` profile and which are only an inventory, plus the
//! skills/instructions lists.
//!
//! Contract: [`is_open`] says the field is the context's; [`field`] draws it.
//! Only outbound data emits — a source marked 'Include in review' is white-hot
//! in consent, never here; everything in this file is ambient ink.
//!
//! What is here now: the resolved sources as paths with their byte counts, the
//! resolution status, and 'Close'. The context lane owns the ordered path
//! strip, the `Include in review` toggles, the applied/precedence/included
//! encodings, the skills and instructions inventory, and the refresh/resolve
//! wiring.

use mt_core::{agent_artifacts::context::SourceOccurrence, settings::Language};
use nsh_core::prelude::*;

use crate::{
    state::AppState,
    ui::{
        prims,
        tokens::{self, FONT_SOURCE, SPACE_2, SPACE_3, SPACE_4, SPACE_6},
    },
};

/// Whether the field is showing the agent context.
pub fn is_open(state: &AppState) -> bool {
    state.context_open.get()
}

/// The context field.
pub fn field(state: AppState, language: Language) -> impl IntoElement {
    let _ = language;
    let rows_state = state.clone();
    let status_state = state.clone();
    let close_state = state.clone();
    view()
        .test_id("context-field")
        .w_full()
        .flex_col()
        .gap(px(SPACE_4))
        .child(
            text("Context")
                .font_family(tokens::FONT_UI)
                .text_size(px(tokens::TYPE_UI))
                .line_height(px(tokens::TYPE_UI_LINE))
                .text_color(move || tokens::palette().ink),
        )
        .child(prims::faint(move || {
            match status_state.context_resolved.get() {
                Some(resolved) => SharedString::from(format!(
                    "{} source(s) apply to this file",
                    resolved.sources.len(),
                )),
                None => SharedString::from("Resolving instructions\u{2026}"),
            }
        }))
        .child(
            view().flex_col().gap(px(SPACE_2)).children(
                rows_state
                    .context_resolved
                    .get()
                    .map(|resolved| {
                        resolved
                            .sources
                            .into_iter()
                            .map(|source| source_row(source).into_element())
                            .collect::<Vec<_>>()
                    })
                    .unwrap_or_default(),
            ),
        )
        .child(
            view()
                .pt(px(SPACE_6))
                .flex_row()
                .gap(px(SPACE_3))
                .child(prims::quiet_action("context-refresh", "Refresh", {
                    let state = state.clone();
                    move || state.refresh_context()
                }))
                .child(prims::quiet_action("context-close", "Close", move || {
                    close_state.close_context()
                })),
        )
}

/// One source: its path and its bytes. The context lane replaces this with the
/// ordered strip, its scope line and its `Include in review` toggle.
fn source_row(source: SourceOccurrence) -> impl IntoElement {
    let path = SharedString::from(source.path.to_string_lossy().to_string());
    let bytes = SharedString::from(format!(
        "{} / {} B",
        source.included_bytes, source.raw_bytes
    ));
    view()
        .test_id("context-source")
        .flex_row()
        .justify_between()
        .gap(px(SPACE_3))
        .child(
            text(path)
                .font_family(FONT_SOURCE)
                .text_size(px(tokens::TYPE_SOURCE))
                .line_height(px(tokens::TYPE_SOURCE_LINE))
                .text_color(move || tokens::palette().ink_rest),
        )
        .child(
            text(bytes)
                .font_family(FONT_SOURCE)
                .text_size(px(tokens::TYPE_SOURCE))
                .line_height(px(tokens::TYPE_SOURCE_LINE))
                .tabular_nums()
                .text_color(move || tokens::palette().ink_faint),
        )
}
