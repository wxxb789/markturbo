//! Owner lane: chrome.
//!
//! The welcome state: an empty room with four plain choices at the column
//! origin — Open folder, New file, Paste, Open sample — with the paths opened
//! before under them, and one quiet line that says the local work stays local.
//! It draws no caret: nothing here takes typing, so nothing pretends to.
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
use nsh_core::{Color, file_dialog::OpenDialog, prelude::*};

use crate::{
    app_paths,
    i18n::{self, Key},
    state::{AppState, Entry},
    ui::{
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
                prims::press_feedback(
                    view()
                        .test_id("workspace-home-file")
                        .role(AccessibilityRole::Button)
                        .optional_label(Some(SharedString::from(label.clone())))
                        .window_drag(false)
                        .cursor_pointer()
                        .pl(px(SPACE_3))
                        .py(px(SPACE_1))
                        .hover(|s| s.text_color(|| tokens::palette().ink))
                        .focus_visible(|s| s.text_color(|| tokens::palette().ink).underline())
                        .on_press(move || state.open_file(path.clone())),
                )
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
                    .child(prims::faint(find_a_file(language))),
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
    prims::press_feedback(
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
            .border_color(move || tokens::palette().ink_caption)
            .hover(|s| s.border_color(|| tokens::palette().ink))
            .focus_visible(|s| s.border_color(|| tokens::palette().ink))
            .on_press(move || choose_folder(state.clone())),
    )
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
            move |target: ReadSignal<RecentTarget>, index: ReadSignal<usize>| {
                let target = target.get();
                let index = index.get();
                let state = state.clone();
                let kind = match target.kind {
                    RecentTargetKind::Workspace => i18n::t(language, Key::Workspace),
                    RecentTargetKind::File => i18n::t(language, Key::File),
                };
                let name = recent_name(&target, &state.settings.get().recent_targets);
                let label = format!("{kind} {name}");
                let missing_label = format!("{label} \u{2014} {}", s(S::Missing, language));
                // A target that vanished after the row was drawn flips to
                // missing on the press instead of doing nothing.
                let missing = signal(!target_exists(&target));
                let target_for_open = target.clone();
                view()
                    .test_id("welcome-recent-row")
                    .flex_row()
                    .items_center()
                    .child(
                        prims::press_feedback(
                            view()
                                .test_id("welcome-recent")
                                .role(AccessibilityRole::Button)
                                .optional_label(move || {
                                    Some(SharedString::from(if missing.get() {
                                        missing_label.clone()
                                    } else {
                                        label.clone()
                                    }))
                                })
                                .disabled(move || missing.get())
                                .flex_row()
                                .items_center()
                                .gap(px(SPACE_2))
                                .pl(px(SPACE_3))
                                .py(px(SPACE_1))
                                .cursor_pointer()
                                .hover(|s| s.text_color(|| tokens::palette().ink))
                                .focus_visible(|s| {
                                    s.text_color(|| tokens::palette().ink).underline()
                                })
                                .on_press({
                                    let open_state = state.clone();
                                    move || {
                                        if target_exists(&target_for_open) {
                                            open_state.open_recent(&target_for_open);
                                        } else {
                                            missing.set(true);
                                        }
                                    }
                                }),
                        )
                        .child(
                            text(kind)
                                .font_family(FONT_UI)
                                .text_size(px(TYPE_UI))
                                .line_height(px(TYPE_UI_LINE))
                                .text_color(move || tokens::palette().ink_caption),
                        )
                        .child(
                            text(name)
                                .font_family(FONT_UI)
                                .text_size(px(TYPE_UI))
                                .line_height(px(TYPE_UI_LINE))
                                .text_color(move || {
                                    if missing.get() {
                                        tokens::palette().ink_caption
                                    } else {
                                        tokens::palette().ink_rest
                                    }
                                })
                                .truncate(),
                        ),
                    )
                    .child(dynamic({
                        let remove_state = state.clone();
                        let remove_target = target.clone();
                        let remove_id = format!("welcome-recent-remove-{index}");
                        move || {
                            if missing.get() {
                                view()
                                    .pr(px(SPACE_3))
                                    .child(prims::quiet_action(
                                        &remove_id,
                                        s(S::MissingRemove, language),
                                        {
                                            let remove_state = remove_state.clone();
                                            let remove_target = remove_target.clone();
                                            move || {
                                                remove_state
                                                    .remove_recent_target(&remove_target.path)
                                            }
                                        },
                                    ))
                                    .into_element()
                            } else {
                                view().into_element()
                            }
                        }
                    }))
            },
        ))
}

fn recent_name(target: &RecentTarget, entries: &[RecentTarget]) -> String {
    let shared = entries
        .iter()
        .filter(|other| other.display_name == target.display_name)
        .count();
    if shared > 1
        && let Some(folder) = target.path.parent().and_then(|parent| parent.file_name())
    {
        return format!("{}/{}", folder.to_string_lossy(), target.display_name);
    }
    target.display_name.clone()
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
    let language = state.settings.get().language;
    let picked = pick_paths(OpenDialog::folders().multiple(false));
    spawn(async move {
        match picked.await {
            Ok(Some(paths)) => {
                if let Some(path) = paths.into_iter().next() {
                    state.open_folder(path);
                }
            }
            Ok(None) => {}
            Err(why) => state.notify_error(picker_failed(language, &why)),
        }
    });
}

fn find_a_file(language: Language) -> String {
    let key = crate::ui::titlebar::shortcut(false, "P");
    match language {
        Language::English => format!("{key} finds any file in this folder."),
        Language::Chinese => format!("按 {key} 查找此文件夹中的任意文件。"),
    }
}

fn picker_failed(language: Language, why: &dyn std::fmt::Display) -> String {
    match language {
        Language::English => format!("Could not open the folder dialog: {why}. Try again."),
        Language::Chinese => format!("无法打开文件夹对话框：{why}。请重试。"),
    }
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
    MissingRemove,
    NoFiles,
}

fn s(key: S, language: Language) -> &'static str {
    match language {
        Language::English => match key {
            S::OpenFolder => "Open folder\u{2026}",
            S::NewFile => "New file",
            S::Paste => "Paste as new document",
            S::OpenSample => "Open sample",
            S::StaysOnThisDevice => "Opening, editing and search stay on this device.",
            S::Missing => "Missing",
            S::MissingRemove => "Missing \u{00B7} Remove",
            S::NoFiles => "No files in this folder yet.",
        },
        Language::Chinese => match key {
            S::OpenFolder => "打开文件夹\u{2026}",
            S::NewFile => "新建文件",
            S::Paste => "粘贴为新文档",
            S::OpenSample => "打开示例",
            S::StaysOnThisDevice => "打开、编辑与搜索都在这台设备上完成。",
            S::Missing => "已不存在",
            S::MissingRemove => "已不存在 \u{00B7} 移除",
            S::NoFiles => "这个文件夹里还没有文件。",
        },
    }
}
