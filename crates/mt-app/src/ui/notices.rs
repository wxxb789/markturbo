//! Owner lane: notices.
//!
//! Everything the app says when things are not calm: the recovery banner for a
//! checkpointed buffer, the external-change conflict, the destructive close and
//! quit guard, and the quiet status line at the foot of the column.
//!
//! Contract: the shell asks `has_*` before it places a banner, draws
//! [`recovery_banner`] and [`conflict_banner`] at the top of the column and
//! [`status_line`] at its foot, and puts [`guard`] in the overlay layer when
//! [`guard_is_open`] says so. The notices lane owns the copy, the dashed
//! warm-grey conflict underline, and 'Cancel has the focus' on the guard.
//!
//! What is here now: the two banners and the status line, wired to the state
//! methods the old UI used. The guard returns nothing yet: `state.rs` still
//! closes through the platform's own dialog, and replacing it needs an additive
//! signal in the state (the notices lane's job) — a guard that cannot save is
//! worse than no guard.

use mt_core::settings::Language;
use nsh_core::prelude::*;

use crate::{
    i18n::{self, Key},
    state::AppState,
    ui::{
        prims,
        tokens::{self, SPACE_1, SPACE_3, SPACE_4},
    },
};

/// Whether a recovered checkpoint is waiting.
pub fn has_recovery(state: &AppState) -> bool {
    !state.recovery_records.get().is_empty()
}

/// The recovery banner: unsaved work survived a crash or a quit.
pub fn recovery_banner(state: AppState, language: Language) -> impl IntoElement {
    let count_state = state.clone();
    let restore_state = state.clone();
    let discard_state = state.clone();
    view()
        .test_id("recovery-banner")
        .w_full()
        .flex_row()
        .items_center()
        .gap(px(SPACE_3))
        .pb(px(SPACE_3))
        .child(view().flex_1().child(prims::rest(move || {
            SharedString::from(format!(
                "{} ({})",
                i18n::t(language, Key::RestoredDocuments),
                count_state.recovery_records.get().len()
            ))
        })))
        .child(prims::quiet_action(
            "recovery-restore",
            i18n::t(language, Key::RestoreAll),
            move || restore_state.restore_all_recovered(),
        ))
        .child(prims::quiet_action(
            "recovery-discard",
            i18n::t(language, Key::DiscardAll),
            move || discard_state.discard_all_recovered(),
        ))
}

/// Whether the active document differs from what is on disk.
pub fn has_conflict(state: &AppState) -> bool {
    state.active_tab().is_some_and(|tab| tab.conflict.get())
}

/// The external-change banner: your file is not what it was on disk.
pub fn conflict_banner(state: AppState, language: Language) -> impl IntoElement {
    let reload_state = state.clone();
    let overwrite_state = state.clone();
    let save_as_state = state.clone();
    view()
        .test_id("conflict-banner")
        .w_full()
        .flex_row()
        .items_center()
        .gap(px(SPACE_3))
        .pb(px(SPACE_3))
        .child(
            view()
                .flex_1()
                .child(prims::ink(i18n::t(language, Key::SaveConflict))),
        )
        .child(prims::quiet_action(
            "conflict-reload",
            i18n::t(language, Key::ReloadFromDisk),
            move || reload_state.reload_active(),
        ))
        .child(prims::quiet_action(
            "conflict-overwrite",
            i18n::t(language, Key::Overwrite),
            move || overwrite_state.overwrite_active(),
        ))
        .child(prims::quiet_action(
            "conflict-save-as",
            i18n::t(language, Key::SaveAsPicker),
            move || save_as_state.save_as_active(),
        ))
}

/// The quiet line at the foot of the column: what the app last did.
pub fn status_line(state: AppState, language: Language) -> impl IntoElement {
    let _ = language;
    view().test_id("status").w_full().pt(px(SPACE_3)).child(
        text(move || state.status.get())
            .font_family(tokens::FONT_UI)
            .text_size(px(tokens::TYPE_UI))
            .line_height(px(tokens::TYPE_UI_LINE))
            .text_color(move || tokens::palette().ink_rest),
    )
}

/// Whether the destructive close or quit guard is on screen.
///
/// `state.rs` still runs that flow through the platform's dialog, so this is
/// false: the guard card arrives with the state signal the notices lane adds.
pub fn guard_is_open(state: &AppState) -> bool {
    let _ = state;
    false
}

/// The guard card, in the overlay layer: 'Close without saving?' with Save,
/// Don't save and Cancel, Cancel focused.
pub fn guard(state: AppState, language: Language) -> impl IntoElement {
    let _ = (state, language);
    view().test_id("guard-card").child(prims::rest("")).child(
        view()
            .pt(px(SPACE_1))
            .child(prims::faint("Closing with unsaved changes asks first.")),
    )
}

/// The hairline a banner uses to separate itself from the document.
pub fn banner_rule() -> impl IntoElement {
    view().w_full().pt(px(SPACE_4)).child(prims::hairline())
}
