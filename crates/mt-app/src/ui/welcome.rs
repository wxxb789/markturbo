//! Owner lane: chrome.
//!
//! The welcome state: an empty room with a blinking caret at the column origin
//! and four plain choices beneath it.
//!
//! Contract: [`welcome`] is the column's content while no document is open;
//! the shell gives it the column's measure and ground. It carries
//! `test_id("welcome")` on its root.
//!
//! Copy is the spec's (`plain_copy`): 'Open folder', 'New file', 'Paste',
//! 'Open sample', and the resting line under them. The chrome lane owns the
//! rest: default focus on 'Open folder', the recent targets, and
//! 'Don't show the welcome again'.

use mt_core::settings::Language;
use nsh_core::{
    element::{Easing, Keyframe, KeyframeTiming},
    file_dialog::OpenDialog,
    prelude::*,
};

use crate::{state::AppState, ui::prims, ui::tokens};

/// The welcome state.
pub fn welcome(state: AppState, language: Language) -> impl IntoElement {
    let open_state = state.clone();
    let new_state = state.clone();
    let paste_state = state.clone();
    let sample_state = state.clone();
    view()
        .test_id("welcome")
        .w_full()
        .flex_col()
        .pt(px(tokens::SPACE_3))
        .child(caret())
        .child(
            view()
                .flex_col()
                .items_start()
                .gap(px(tokens::SPACE_3))
                .pt(px(tokens::SPACE_5))
                .child(prims::primary_action(
                    "welcome-open-folder",
                    s(S::OpenFolder, language),
                    move || choose_folder(open_state.clone()),
                ))
                .child(prims::quiet_action(
                    "welcome-new-file",
                    s(S::NewFile, language),
                    move || new_state.new_document(),
                ))
                .child(prims::quiet_action(
                    "welcome-paste",
                    s(S::Paste, language),
                    move || paste_state.new_from_clipboard(),
                ))
                .child(prims::quiet_action(
                    "welcome-open-sample",
                    s(S::OpenSample, language),
                    move || sample_state.open_bundled_sample(),
                )),
        )
        .child(
            view()
                .pt(px(tokens::SPACE_6))
                .child(prims::faint(s(S::StaysOnThisDevice, language))),
        )
}

/// The blinking caret at the column origin: 2px of ink, 1.06s steps.
fn caret() -> impl IntoElement {
    let opacity = animated_value(1.0);
    let mut timing = KeyframeTiming::new(1060.0);
    timing.easing = Easing::Linear;
    // Blink until the caret is dropped: 1 → 0 → 1, steps, forever.
    timing.iterations = None;
    opacity.keyframes(
        [
            Keyframe::at(0.0, 1.0),
            Keyframe::at(0.5, 0.0),
            Keyframe::at(1.0, 1.0),
        ],
        timing,
    );
    view()
        .test_id("welcome-caret")
        .w(px(2.0))
        .h(px(tokens::TYPE_UI_LINE))
        .bg(move || tokens::palette().ink)
        .bind_opacity(&opacity)
}

/// Shows the system's folder picker and opens what the user picks; a cancel
/// opens nothing. The chrome lane keeps this (the title row will want it too).
fn choose_folder(state: AppState) {
    let picked = pick_paths(OpenDialog::folders().multiple(false));
    spawn(async move {
        match picked.await {
            Ok(Some(paths)) => {
                if let Some(path) = paths.into_iter().next() {
                    state.open_folder(path);
                }
            }
            Ok(None) => {}
            Err(why) => state
                .status
                .set(SharedString::from(format!("Folder picker failed: {why}"))),
        }
    });
}

/// The welcome's copy. New strings live here rather than in `i18n.rs` (a merge
/// hotspot): the match is exhaustive, so a missing translation is a compile
/// error.
#[derive(Clone, Copy)]
enum S {
    OpenFolder,
    NewFile,
    Paste,
    OpenSample,
    StaysOnThisDevice,
}

fn s(key: S, language: Language) -> &'static str {
    match language {
        Language::English => match key {
            S::OpenFolder => "Open folder",
            S::NewFile => "New file",
            S::Paste => "Paste",
            S::OpenSample => "Open sample",
            S::StaysOnThisDevice => "Opening, editing and search stay on this device.",
        },
        Language::Chinese => match key {
            S::OpenFolder => "打开文件夹",
            S::NewFile => "新建文件",
            S::Paste => "粘贴",
            S::OpenSample => "打开示例",
            S::StaysOnThisDevice => "打开、编辑与搜索都在这台设备上完成。",
        },
    }
}
