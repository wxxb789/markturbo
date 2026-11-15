//! Owner lane: chrome.
//!
//! The welcome state: an empty room with a blinking caret at the column origin
//! and four plain choices beneath it — Open folder, New file, Paste, Open
//! sample — with the paths opened before under them, and one quiet line that
//! says the local work stays local.
//!
//! Contract: [`welcome`] is the column's content while no document is open;
//! the shell gives it the column's measure and ground. It carries
//! `test_id("welcome")` on its root.
//!
//! There is no 'Don't show the welcome again' control: the setting that would
//! back it (`show_welcome_on_startup`) is read by no launch path yet, so the
//! control would write a value nothing reads. The chrome lane adds it when the
//! startup path honours it.

use mt_core::settings::{Language, RecentTarget, RecentTargetKind};
use nsh_core::{
    Color,
    element::{Easing, Keyframe, KeyframeTiming},
    file_dialog::OpenDialog,
    prelude::*,
};

use crate::{
    app_paths,
    i18n::{self, Key},
    state::AppState,
    ui::{
        motion::HOVER_MS,
        prims,
        tokens::{
            self, FONT_UI, HAIRLINE, SPACE_1, SPACE_2, SPACE_3, SPACE_5, SPACE_6, TYPE_UI,
            TYPE_UI_LINE,
        },
    },
};

/// The paths the welcome lists under the four choices.
const RECENTS: usize = 5;

/// The welcome state.
pub fn welcome(state: AppState, language: Language) -> impl IntoElement {
    let new_state = state.clone();
    let paste_state = state.clone();
    let sample_state = state.clone();
    view()
        .test_id("welcome")
        .w_full()
        .flex_col()
        .pt(px(SPACE_3))
        .child(caret())
        .child(
            view()
                .flex_col()
                .items_start()
                .gap(px(SPACE_3))
                .pt(px(SPACE_5))
                .child(open_folder_action(
                    "welcome-open-folder",
                    s(S::OpenFolder, language),
                    state.clone(),
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
                .child(when(app_paths::bundled_sample_available, move || {
                    let sample_state = sample_state.clone();
                    prims::quiet_action(
                        "welcome-open-sample",
                        s(S::OpenSample, language),
                        move || sample_state.open_bundled_sample(),
                    )
                    .into_element()
                })),
        )
        .child(when(
            {
                let state = state.clone();
                move || !state.settings.get().recent_targets.is_empty()
            },
            {
                let state = state.clone();
                move || recents(state.clone(), language).into_element()
            },
        ))
        .child(
            view().pt(px(SPACE_6)).child(
                view()
                    .test_id("welcome-local-only")
                    .child(prims::faint(s(S::StaysOnThisDevice, language))),
            ),
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
        .h(px(TYPE_UI_LINE))
        .bg(move || tokens::palette().ink)
        .bind_opacity(&opacity)
}

/// 'Open folder', the state's one next action: full ink, a hairline plate, and
/// the focus it asks for when the state appears (the platform serves the
/// request for the views it can focus; the plate carries it otherwise).
fn open_folder_action(
    test_id: &str,
    label: &'static str,
    state: AppState,
) -> impl Interactive + ParentElement {
    let handle = element_ref();
    effect(move || handle.focus());
    view()
        .test_id(test_id)
        .role(AccessibilityRole::Button)
        .optional_label(Some(SharedString::from(label)))
        .window_drag(false)
        .cursor_pointer()
        .bg(Color::TRANSPARENT)
        .px(px(SPACE_3))
        .py(px(SPACE_2))
        .border(px(HAIRLINE))
        .border_color(move || tokens::palette().ink_faint)
        .transition_colors()
        .duration(HOVER_MS)
        .hover(|s| s.border_color(tokens::palette().ink))
        .focus_visible(|s| s.border_color(tokens::palette().ink))
        .on_press(move || choose_folder(state.clone()))
        .child(
            text(SharedString::from(label))
                .font_family(FONT_UI)
                .text_size(px(TYPE_UI))
                .line_height(px(TYPE_UI_LINE))
                .text_color(move || tokens::palette().ink),
        )
        .element_ref(&handle)
}

/// The paths opened before, quiet under the four choices: a person who comes
/// back to one workspace should not walk the folder picker again.
fn recents(state: AppState, language: Language) -> impl IntoElement {
    let list_state = state.clone();
    view()
        .test_id("welcome-recents")
        .flex_col()
        .gap(px(SPACE_1))
        .pt(px(SPACE_5))
        .child(
            view()
                .test_id("welcome-recents-title")
                .child(prims::faint(i18n::t(language, Key::Recent))),
        )
        .child(each(
            // The cap is applied to the list itself: a row's index is read
            // when it is built, and the list moves under it as new targets
            // are recorded.
            move || {
                list_state
                    .settings
                    .get()
                    .recent_targets
                    .into_iter()
                    .take(RECENTS)
                    .collect::<Vec<_>>()
            },
            |target: &RecentTarget| target.path.clone(),
            move |target: ReadSignal<RecentTarget>, _| {
                let target = target.get();
                let state = state.clone();
                let target_for_open = target.clone();
                view()
                    .test_id("welcome-recent")
                    .flex_row()
                    .items_center()
                    .gap(px(SPACE_2))
                    .pl(px(SPACE_3))
                    .py(px(SPACE_1))
                    .cursor_pointer()
                    .hover(|s| s.text_color(tokens::palette().ink))
                    .focus_visible(|s| s.text_color(tokens::palette().ink).underline())
                    .on_press(move || state.open_recent(&target_for_open))
                    .child(
                        text(match target.kind {
                            RecentTargetKind::Workspace => i18n::t(language, Key::Workspace),
                            RecentTargetKind::File => i18n::t(language, Key::File),
                        })
                        .font_family(FONT_UI)
                        .text_size(px(TYPE_UI))
                        .line_height(px(TYPE_UI_LINE))
                        .text_color(move || tokens::palette().ink_faint),
                    )
                    .child(
                        text(target.display_name.clone())
                            .font_family(FONT_UI)
                            .text_size(px(TYPE_UI))
                            .line_height(px(TYPE_UI_LINE))
                            .text_color(move || tokens::palette().ink_rest)
                            .truncate(),
                    )
            },
        ))
}

/// Shows the system's folder picker and opens what the user picks; a cancel
/// opens nothing. The chrome lane's title menu shares this flow.
pub(crate) fn choose_folder(state: AppState) {
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
