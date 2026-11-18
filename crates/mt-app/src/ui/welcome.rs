//! Owner lane: chrome.
//!
//! The welcome state: an empty room with a blinking caret at the column origin
//! and four plain choices beneath it — Open folder, New file, Paste, Open
//! sample — with the paths opened before under them, and one quiet line that
//! says the local work stays local.
//!
//! Contract: [`welcome`] is the column's content while no document is open;
//! the shell gives it the column's measure and ground. Until a workspace is
//! open it is the welcome, `test_id("welcome")` on its root; with one open it
//! is the workspace's home, `test_id("workspace-home")`, listing its first
//! files.
//!
//! There is no 'Don't show the welcome again' control: the setting that would
//! back it (`show_welcome_on_startup`) is read by no launch path yet, so the
//! control would write a value nothing reads. The chrome lane adds it when the
//! startup path honours it.

use mt_core::{
    settings::{Language, RecentTarget, RecentTargetKind},
    workspace::tree,
};
use nsh_core::{
    Color,
    element::{Easing, Keyframe, KeyframeTiming},
    file_dialog::OpenDialog,
    prelude::*,
};

use crate::{
    app_paths,
    i18n::{self, Key},
    state::{AppState, Entry},
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

/// The workspace's files the column lists while none is open; Ctrl+P
/// finds the rest.
const WORKSPACE_FILES: usize = 12;

/// The column while no document is open: the welcome until a workspace is
/// open, then that workspace's home — a folder picked, the sample, a directory
/// given on the command line or a recent workspace all leave the welcome.
pub fn welcome(state: AppState, language: Language) -> impl IntoElement {
    let no_workspace = state.clone();
    let start_state = state.clone();
    when(
        move || no_workspace.root.get().is_none(),
        move || start(start_state.clone(), language).into_element(),
    )
    .otherwise(move || workspace_home(state.clone(), language).into_element())
}

/// The welcome state.
fn start(state: AppState, language: Language) -> impl IntoElement {
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

/// An open workspace with no document on screen: its name, and its first
/// files as quiet rows that open on a press.
fn workspace_home(state: AppState, language: Language) -> impl IntoElement {
    let name_state = state.clone();
    let files_state = state.clone();
    // One read of the tree feeds both the rows and the empty line.
    let files = computed(move || workspace_files(&files_state));
    view()
        .test_id("workspace-home")
        .w_full()
        .flex_col()
        .gap(px(SPACE_1))
        .pt(px(SPACE_5))
        .child(
            view()
                .test_id("workspace-home-name")
                .pb(px(SPACE_2))
                .child(prims::ink(move || {
                    let root = name_state.root.get().unwrap_or_default();
                    SharedString::from(
                        root.file_name()
                            .map(|name| name.to_string_lossy().into_owned())
                            .unwrap_or_else(|| root.display().to_string()),
                    )
                })),
        )
        .child(each(
            move || files.get(),
            |entry: &Entry| entry.path.clone(),
            move |entry: ReadSignal<Entry>, _| {
                let entry = entry.get();
                let state = state.clone();
                let label = state
                    .root
                    .get_untracked()
                    .map(|root| tree::display_relative(&root, &entry.path))
                    .unwrap_or_else(|| entry.name.clone());
                let path = entry.path.clone();
                view()
                    .test_id("workspace-home-file")
                    .role(AccessibilityRole::Button)
                    .optional_label(Some(SharedString::from(label.clone())))
                    .window_drag(false)
                    .cursor_pointer()
                    .pl(px(SPACE_3))
                    .py(px(SPACE_1))
                    .hover(|s| s.text_color(tokens::palette().ink))
                    .focus_visible(|s| s.text_color(tokens::palette().ink).underline())
                    .on_press(move || state.open_file(path.clone()))
                    .child(
                        text(label)
                            .font_family(FONT_UI)
                            .text_size(px(TYPE_UI))
                            .line_height(px(TYPE_UI_LINE))
                            .text_color(move || tokens::palette().ink_rest)
                            .truncate(),
                    )
            },
        ))
        .child(when(
            move || files.with(Vec::is_empty),
            move || {
                view()
                    .test_id("workspace-home-empty")
                    .child(prims::faint(s(S::NoFiles, language)))
                    .into_element()
            },
        ))
        .child(
            view().pt(px(SPACE_6)).child(
                view()
                    .test_id("workspace-home-find")
                    .child(prims::faint(s(S::FindAFile, language))),
            ),
        )
}

/// The files [`workspace_home`] lists: the workspace's files, in the
/// tree's order, capped.
fn workspace_files(state: &AppState) -> Vec<Entry> {
    state
        .entries
        .get()
        .into_iter()
        .filter(|entry| !entry.is_dir)
        .take(WORKSPACE_FILES)
        .collect()
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
                let kind = match target.kind {
                    RecentTargetKind::Workspace => i18n::t(language, Key::Workspace),
                    RecentTargetKind::File => i18n::t(language, Key::File),
                };
                // A target that is gone stays listed, marked and disabled: the
                // row says what happened rather than failing on a press.
                let missing = !target_exists(&target);
                let label = if missing {
                    format!(
                        "{kind} {} \u{2014} {}",
                        target.display_name,
                        s(S::Missing, language)
                    )
                } else {
                    format!("{kind} {}", target.display_name)
                };
                let target_for_open = target.clone();
                view()
                    .test_id("welcome-recent")
                    .role(AccessibilityRole::Button)
                    .optional_label(Some(SharedString::from(label)))
                    .disabled(missing)
                    .flex_row()
                    .items_center()
                    .gap(px(SPACE_2))
                    .pl(px(SPACE_3))
                    .py(px(SPACE_1))
                    .when(!missing, |row| {
                        row.cursor_pointer()
                            .hover(|s| s.text_color(tokens::palette().ink))
                            .focus_visible(|s| s.text_color(tokens::palette().ink).underline())
                    })
                    .on_press(move || {
                        // Checked again at the press: the target may have gone
                        // since the row was drawn.
                        if target_exists(&target_for_open) {
                            state.open_recent(&target_for_open);
                        }
                    })
                    .child(
                        text(kind)
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
                            .text_color(move || {
                                if missing {
                                    tokens::palette().ink_faint
                                } else {
                                    tokens::palette().ink_rest
                                }
                            })
                            .truncate(),
                    )
                    .when(missing, |row| {
                        row.child(prims::faint(s(S::Missing, language)))
                    })
            },
        ))
}

/// Whether a recent target is still there, as the kind it was recorded as.
fn target_exists(target: &RecentTarget) -> bool {
    match target.kind {
        RecentTargetKind::Workspace => target.path.is_dir(),
        RecentTargetKind::File => target.path.is_file(),
    }
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
    Missing,
    NoFiles,
    FindAFile,
}

fn s(key: S, language: Language) -> &'static str {
    match language {
        Language::English => match key {
            S::OpenFolder => "Open folder",
            S::NewFile => "New file",
            S::Paste => "Paste",
            S::OpenSample => "Open sample",
            S::StaysOnThisDevice => "Opening, editing and search stay on this device.",
            S::Missing => "Missing",
            S::NoFiles => "No files in this folder yet.",
            S::FindAFile => "Ctrl+P finds any file in this folder.",
        },
        Language::Chinese => match key {
            S::OpenFolder => "打开文件夹",
            S::NewFile => "新建文件",
            S::Paste => "粘贴",
            S::OpenSample => "打开示例",
            S::StaysOnThisDevice => "打开、编辑与搜索都在这台设备上完成。",
            S::Missing => "已不存在",
            S::NoFiles => "这个文件夹里还没有文件。",
            S::FindAFile => "按 Ctrl+P 查找这个文件夹中的任意文件。",
        },
    }
}
