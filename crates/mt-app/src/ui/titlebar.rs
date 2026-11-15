//! Owner lane: chrome.
//!
//! The 32px opaque title row: the file's name at the left (aligned with the
//! text column), and 'Review' plus the settings glyph at the right.
//!
//! Contract: [`title_row`] fills the row and owns its height and its padding.
//! The window runs with the system title bar hidden (`shell` sets it), so the
//! row must reserve the native controls' area — `prims::caption_insets()` —
//! and mark itself draggable, or the window cannot be moved.
//!
//! The chrome lane owns everything here: the dirty marker, the Context entry,
//! the Edit/Preview/Split actions, the Review shortcut (Ctrl+Enter), the
//! settings glyph and the title-bar menu. What is here now is the row and its
//! two named controls, wired to the state methods the old header used.

use mt_core::settings::Language;
use nsh_core::prelude::*;

use crate::{
    i18n::{self, Key},
    state::AppState,
    ui::{
        prims,
        tokens::{self, COLUMN_GAP, RAIL_W, TITLE_ROW_H},
    },
};

/// The title row. `state` is the shell's handle on the app; `language` is the
/// language the shell was re-keyed with.
pub fn title_row(state: AppState, language: Language) -> impl IntoElement {
    let review_state = state.clone();
    let settings_state = state.clone();
    let insets = prims::caption_insets();
    log::debug!(
        "title row: caption insets top {} left {} right {}",
        insets.top.0,
        insets.left.0,
        insets.right.0
    );
    let height = TITLE_ROW_H.max(insets.top.0);
    view()
        .test_id("title-row")
        .h(px(height))
        .w_full()
        .flex_row()
        .items_center()
        .justify_between()
        .pl(px(RAIL_W + COLUMN_GAP))
        .pr(px(insets.right.0 + tokens::SPACE_3))
        .bg(move || tokens::palette().room)
        .window_drag(true)
        .child(
            view()
                .flex_row()
                .items_center()
                .gap(px(tokens::SPACE_2))
                .child(
                    text(move || match state.active_tab() {
                        Some(tab) => tab.title.clone(),
                        None => SharedString::from(""),
                    })
                    .font_family(tokens::FONT_UI)
                    .text_size(px(tokens::TYPE_UI))
                    .line_height(px(tokens::TYPE_UI_LINE))
                    .text_color(move || tokens::palette().ink_rest),
                ),
        )
        .child(
            view()
                .flex_row()
                .items_center()
                .gap(px(tokens::SPACE_3))
                .child(prims::quiet_action(
                    "title-review",
                    i18n::t(language, Key::Review),
                    move || {
                        if review_state.active_tab().is_none() {
                            return;
                        }
                        review_state.start_review();
                    },
                ))
                .child(prims::quiet_action(
                    "title-settings",
                    "\u{2699}",
                    move || {
                        settings_state.open_settings();
                    },
                )),
        )
}
