//! Owner lane: chrome.
//!
//! The rail: a 44px strip at the far left with one 6px dot per open file
//! (plain ink; the dot glows ember only while the file holds live model
//! output), widening to 240px on hover to show names.
//!
//! Contract: [`rail`] is the strip's whole column — the shell places it beside
//! the text column and gives it no width of its own, so the width, the hover
//! widening (160ms `cubic-bezier(0.2, 0, 0, 1)`) and the names all belong
//! here. The chrome lane also owns what sits on the rail beyond the dots: the
//! workspace tree and search as one input line at the top of the field on
//! Ctrl+P / Ctrl+Shift+F, and the preview-slot semantics.

use mt_core::settings::Language;
use nsh_core::prelude::*;

use crate::{
    state::{AppState, Tab},
    ui::tokens::{self, RAIL_DOT, RAIL_W},
};

/// The rail: one dot per open file, the active file's at full ink.
///
/// Not yet here (chrome lane): the hover widening, the file names, the ember
/// dot for a file that holds live model output, and search.
pub fn rail(state: AppState, language: Language) -> impl IntoElement {
    let _ = language;
    view()
        .test_id("rail")
        .w(px(RAIL_W))
        .h(relative(1.0))
        .flex_col()
        .items_center()
        .pt(px(tokens::SPACE_5))
        .gap(px(tokens::SPACE_3))
        .bg(move || tokens::palette().room)
        .child(each(
            {
                let state = state.clone();
                move || state.tabs.get()
            },
            |tab: &Tab| tab.id,
            {
                let state = state.clone();
                move |tab: ReadSignal<Tab>, _| {
                    let id = tab.get().id;
                    let active_state = state.clone();
                    view()
                        .test_id("rail-dot")
                        .w(px(RAIL_DOT))
                        .h(px(RAIL_DOT))
                        .rounded(px(RAIL_DOT / 2.0))
                        .bg(move || {
                            if active_state.active.get() == Some(id) {
                                tokens::palette().ink
                            } else {
                                tokens::palette().ink_faint
                            }
                        })
                }
            },
        ))
}
