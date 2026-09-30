use super::{
    StartupRecovery, Workspace, open_test_workspace, open_test_workspace_with_welcome_preference,
};
use crate::i18n;
use crate::views::Layout;
use gpui_kit::{
    AppContext as _, ClipboardItem, Focusable as _, TestAppContext, VisualTestContext, point, px,
};
use std::cell::RefCell;
use std::fs;
use std::path::{Path, PathBuf};
use std::rc::Rc;

#[test]
fn welcome_visibility_requires_a_no_argument_launch_and_the_saved_preference() {
    assert!(super::super::welcome::should_show_welcome(None, true));
    assert!(!super::super::welcome::should_show_welcome(
        Some(Path::new("workspace")),
        true
    ));
    assert!(!super::super::welcome::should_show_welcome(None, false));
}

#[gpui_kit::test]
fn no_argument_workspace_starts_on_the_welcome_state(cx: &mut TestAppContext) {
    let (workspace, cx) = open_test_workspace_with_welcome_preference(cx, true);
    workspace.read_with(cx, |workspace, _| {
        assert!(workspace.show_welcome);
        assert!(workspace.tabs.is_empty());
        assert!(workspace.root.is_none());
    });
}

// Exercise rendered controls and native GPUI event dispatch, not handlers.
// This guards the first-use path without requiring a foreground desktop.
#[gpui_kit::test]
fn kit_welcome_new_click_opens_one_editable_document(cx: &mut TestAppContext) {
    use gpui_kit::test::TestWindowExt as _;

    let (workspace, cx) = open_test_workspace_with_welcome_preference(cx, true);
    cx.update(|window, app| {
        window.render_frame(app);
        assert!(window.find("welcome-new").visible());
        window.click("welcome-new", app);
    });
    cx.run_until_parked();
    workspace.read_with(cx, |workspace, app| {
        assert!(!workspace.show_welcome);
        assert_eq!(workspace.tabs.len(), 1);
        let document = workspace.document_at(0).unwrap().read(app);
        assert_eq!(document.source_path(), None);
        assert_eq!(document.text(app), "");
        assert!(!document.is_dirty());
    });
    cx.update(|window, app| {
        window.render_frame(app);
        assert!(window.try_find("welcome-new").is_none());
        window.click("source", app);
        assert_eq!(window.find("source").focused(), Some(true));
        window.input("# Headless edit", app);
    });
    cx.run_until_parked();
    workspace.read_with(cx, |workspace, app| {
        assert_eq!(workspace.tabs.len(), 1);
        let document = workspace.document_at(0).unwrap().read(app);
        assert_eq!(document.text(app), "# Headless edit");
        assert!(document.is_dirty());
    });
}

#[gpui_kit::test]
fn explicit_path_bypasses_welcome_and_records_its_file_target(cx: &mut TestAppContext) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("opened.md");
    fs::write(&path, "# Opened\n").unwrap();
    let (workspace, cx) = open_test_workspace(cx, path.clone());
    workspace.read_with(cx, |workspace, app| {
        assert!(!workspace.show_welcome);
        assert_eq!(workspace.root.as_deref(), path.parent());
        assert_eq!(
            crate::settings::AppSettings::global(app)
                .recent_targets
                .first()
                .map(|target| target.path.as_path()),
            Some(path.as_path())
        );
    });
}

#[gpui_kit::test]
fn paste_creates_an_exact_dirty_memory_document(cx: &mut TestAppContext) {
    let text = "# \u{7cbe}\u{8d34} \u{1f680}\nexact clipboard text\n";
    let (workspace, cx) = open_test_workspace_with_welcome_preference(cx, true);
    cx.update(|window, app| {
        app.write_to_clipboard(ClipboardItem::new_string(text.to_string()));
        workspace.update(app, |workspace, cx| {
            workspace.on_paste_into_new(&super::super::PasteIntoNew, window, cx);
        });
    });
    workspace.read_with(cx, |workspace, app| {
        assert!(!workspace.show_welcome);
        let document = workspace.document_at(0).unwrap().read(app);
        assert_eq!(document.text(app), text);
        assert!(document.is_dirty());
        assert_eq!(document.layout(), Layout::Source);
    });
}

#[gpui_kit::test]
fn unavailable_welcome_clipboard_preserves_the_surface_and_reports_the_reason(
    cx: &mut TestAppContext,
) {
    let (workspace, cx) = open_test_workspace_with_welcome_preference(cx, true);

    cx.update(|window, app| {
        workspace.update(app, |workspace, cx| {
            workspace.on_paste_into_new(&super::super::PasteIntoNew, window, cx);
        });
    });

    workspace.read_with(cx, |workspace, app| {
        assert!(workspace.show_welcome);
        assert!(workspace.root.is_none());
        assert!(workspace.tabs.is_empty());
        assert_eq!(
            workspace.status.as_deref(),
            Some(i18n::t(i18n::Key::ClipboardTextUnavailable, app))
        );
    });
}

#[gpui_kit::test]
fn welcome_ctrl_v_pastes_into_a_new_document(cx: &mut TestAppContext) {
    let text = "# Clipboard shortcut\nexact \u{4e2d}\u{6587} \u{1f680}\n";
    let (workspace, cx) = open_test_workspace_with_welcome_preference(cx, true);
    cx.write_to_clipboard(ClipboardItem::new_string(text.to_string()));

    cx.simulate_keystrokes("ctrl-v");
    cx.run_until_parked();

    workspace.read_with(cx, |workspace, app| {
        assert!(!workspace.show_welcome);
        assert_eq!(workspace.tabs.len(), 1);
        let document = workspace.document_at(0).unwrap().read(app);
        assert_eq!(document.text(app), text);
        assert!(document.is_dirty());
    });
}

#[gpui_kit::test]
fn welcome_paste_shortcut_is_inactive_while_settings_is_visible(cx: &mut TestAppContext) {
    let (workspace, cx) = open_test_workspace_with_welcome_preference(cx, true);
    cx.update(|window, app| {
        app.write_to_clipboard(ClipboardItem::new_string("settings input".to_string()));
        workspace.update(app, |workspace, cx| {
            workspace.on_open_settings(&super::super::OpenSettings, window, cx);
        });
    });

    cx.simulate_keystrokes("ctrl-v");
    cx.run_until_parked();

    workspace.read_with(cx, |workspace, _| {
        assert!(workspace.settings_open);
        assert!(workspace.show_welcome);
        assert!(workspace.tabs.is_empty());
    });
}

#[cfg(target_os = "windows")]
#[gpui_kit::test]
fn failed_file_open_keeps_welcome_root_tabs_and_focus_unchanged(cx: &mut TestAppContext) {
    use std::os::windows::fs::OpenOptionsExt as _;

    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("locked.md");
    fs::write(&path, "locked\n").unwrap();
    let _lock = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .share_mode(0)
        .open(&path)
        .unwrap();
    let (workspace, cx) = open_test_workspace_with_welcome_preference(cx, true);

    let opened = cx.update(|window, app| {
        workspace.update(app, |workspace, cx| {
            workspace.open_file_target(path.clone(), window, cx)
        })
    });

    assert!(!opened);
    cx.update(|window, app| {
        let workspace = workspace.read(app);
        assert!(workspace.show_welcome);
        assert!(workspace.root.is_none());
        assert!(workspace.tabs.is_empty());
        assert!(workspace.focus_handle.is_focused(window));
    });
}

#[gpui_kit::test]
fn cancelling_file_and_folder_pickers_preserves_the_welcome_state_and_focus(
    cx: &mut TestAppContext,
) {
    let (workspace, cx) = open_test_workspace_with_welcome_preference(cx, true);

    cx.update(|window, app| {
        workspace.update(app, |workspace, cx| {
            workspace.on_open_file(&super::super::OpenFile, window, cx);
        });
    });
    assert!(cx.did_prompt_for_paths());
    cx.simulate_path_prompt_response(|options| {
        assert!(options.files);
        assert!(!options.directories);
        None
    });
    cx.run_until_parked();

    cx.update(|window, app| {
        workspace.update(app, |workspace, cx| {
            workspace.on_open_folder(&super::super::OpenFolder, window, cx);
        });
    });
    assert!(cx.did_prompt_for_paths());
    cx.simulate_path_prompt_response(|options| {
        assert!(!options.files);
        assert!(options.directories);
        None
    });
    cx.run_until_parked();

    cx.update(|window, app| {
        let workspace = workspace.read(app);
        assert!(workspace.show_welcome);
        assert!(workspace.root.is_none());
        assert!(workspace.tabs.is_empty());
        assert!(workspace.status.is_none());
        assert!(workspace.focus_handle.is_focused(window));
    });
}

#[gpui_kit::test]
fn bundled_sample_opens_from_welcome_and_becomes_the_recent_workspace(cx: &mut TestAppContext) {
    let sample = crate::app_paths::bundled_sample_dir().expect("the debug sample");
    let (workspace, cx) = open_test_workspace_with_welcome_preference(cx, true);

    cx.update(|window, app| {
        workspace.update(app, |workspace, cx| {
            workspace.open_bundled_sample(window, cx);
        });
    });

    workspace.read_with(cx, |workspace, app| {
        assert!(!workspace.show_welcome);
        assert_eq!(workspace.root.as_deref(), Some(sample.as_path()));
        let recent = crate::settings::AppSettings::global(app)
            .recent_targets
            .first()
            .expect("the sample recent target");
        assert_eq!(recent.path, sample);
        assert_eq!(recent.kind, mt_core::settings::RecentTargetKind::Workspace);
    });
}

#[gpui_kit::test]
fn unavailable_bundled_sample_keeps_welcome_visible_and_reports_status(cx: &mut TestAppContext) {
    let (workspace, cx) = open_test_workspace_with_welcome_preference(cx, true);

    cx.update(|window, app| {
        workspace.update(app, |workspace, cx| {
            workspace.open_bundled_sample_result(
                Err(std::io::Error::other("test materialization failure")),
                window,
                cx,
            );
        });
    });

    workspace.read_with(cx, |workspace, app| {
        assert!(workspace.show_welcome);
        assert!(workspace.root.is_none());
        assert!(workspace.tabs.is_empty());
        assert_eq!(
            workspace.status.as_deref(),
            Some(i18n::t(i18n::Key::BundledSampleUnavailable, app))
        );
    });
}

#[gpui_kit::test]
fn missing_recent_target_is_disabled_and_removable_without_opening_anything(
    cx: &mut TestAppContext,
) {
    let missing = PathBuf::from("Q:/definitely/not/here/markturbo-missing.md");
    let (workspace, cx) = open_test_workspace_with_welcome_preference(cx, true);
    cx.update(|window, app| {
        crate::settings::AppSettings::update(app, |settings| {
            settings.record_recent_target(mt_core::settings::RecentTarget::new(
                missing.clone(),
                mt_core::settings::RecentTargetKind::File,
                "missing.md",
            ));
        });
        workspace.update(app, |workspace, cx| {
            assert!(!workspace.open_recent_target(&missing, window, cx));
            workspace.remove_recent_target(&missing, cx);
        });
    });
    workspace.read_with(cx, |workspace, app| {
        assert!(workspace.tabs.is_empty());
        assert!(
            crate::settings::AppSettings::global(app)
                .recent_targets
                .is_empty()
        );
    });
}

#[test]
fn recent_target_validation_distinguishes_missing_and_mismatched_entries() {
    let dir = tempfile::tempdir().unwrap();
    let directory = mt_core::settings::RecentTarget::new(
        dir.path(),
        mt_core::settings::RecentTargetKind::File,
        "directory",
    );
    assert_eq!(
        super::super::welcome::recent_target_issue(&directory),
        Some(i18n::Key::RecentUnavailable)
    );
    let missing = mt_core::settings::RecentTarget::new(
        dir.path().join("missing.md"),
        mt_core::settings::RecentTargetKind::File,
        "missing.md",
    );
    assert_eq!(
        super::super::welcome::recent_target_issue(&missing),
        Some(i18n::Key::RecentMissing)
    );
}

#[gpui_kit::test]
fn valid_recent_file_reopens_through_the_shared_target_path(cx: &mut TestAppContext) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("recent.md");
    let text = "# Recent \u{4e2d}\u{6587} \u{1f680}\nexact text\n";
    fs::write(&path, text).unwrap();
    let (workspace, cx) = open_test_workspace_with_welcome_preference(cx, true);
    cx.update(|window, app| {
        crate::settings::AppSettings::update(app, |settings| {
            settings.record_recent_target(mt_core::settings::RecentTarget::new(
                path.clone(),
                mt_core::settings::RecentTargetKind::File,
                "recent.md",
            ));
        });
        workspace.update(app, |workspace, cx| {
            assert!(workspace.open_recent_target(&path, window, cx));
        });
    });
    workspace.read_with(cx, |workspace, app| {
        assert_eq!(workspace.root.as_deref(), path.parent());
        assert_eq!(workspace.tabs.len(), 1);
        assert_eq!(workspace.document_at(0).unwrap().read(app).text(app), text);
        assert_eq!(
            crate::settings::AppSettings::global(app)
                .recent_targets
                .first()
                .map(|target| target.path.as_path()),
            Some(path.as_path())
        );
    });
}

#[gpui_kit::test]
fn dont_show_welcome_again_persists_and_starts_an_empty_memory_document(cx: &mut TestAppContext) {
    let (workspace, cx) = open_test_workspace_with_welcome_preference(cx, true);
    cx.update(|window, app| {
        workspace.update(app, |workspace, cx| {
            workspace.dont_show_welcome_again(window, cx);
        });
    });
    workspace.read_with(cx, |workspace, app| {
        assert!(!workspace.show_welcome);
        assert!(!crate::settings::AppSettings::global(app).show_welcome_on_startup);
        let document = workspace.document_at(0).unwrap().read(app);
        assert_eq!(document.source_path(), None);
        assert_eq!(document.text(app), "");
        assert!(!document.is_dirty());
    });
}

#[gpui_kit::test]
fn disabled_welcome_starts_future_no_argument_workspaces_with_a_new_buffer(
    cx: &mut TestAppContext,
) {
    let (workspace, cx) = open_test_workspace_with_welcome_preference(cx, false);
    workspace.read_with(cx, |workspace, app| {
        assert!(!workspace.show_welcome);
        assert_eq!(workspace.tabs.len(), 1);
        let document = workspace.document_at(0).unwrap().read(app);
        assert_eq!(document.source_path(), None);
        assert_eq!(document.text(app), "");
        assert!(!document.is_dirty());
    });
}

#[gpui_kit::test]
fn ten_recent_targets_scroll_into_view_at_the_minimum_window_size(cx: &mut TestAppContext) {
    let dir = tempfile::tempdir().unwrap();
    let mut targets = Vec::new();
    for ix in 0..10 {
        let path = dir.path().join(format!(
            "{ix:02}-a-very-long-recent-document-name-for-layout-\u{4e2d}\u{6587}.md"
        ));
        if ix < 8 {
            fs::write(&path, format!("# Recent {ix}\n")).unwrap();
        }
        targets.push(path);
    }

    cx.update(|app| {
        gpui_kit::init(app);
        crate::settings::AppSettings::init(app);
        super::super::init(app);
    });
    let captured = Rc::new(RefCell::new(None));
    let window = cx.open_window(gpui_kit::size(px(720.), px(480.)), {
        let captured = captured.clone();
        move |window, app| {
            let workspace = app.new(|cx| {
                Workspace::new_with_startup_recovery(None, StartupRecovery::default, window, cx)
            });
            *captured.borrow_mut() = Some(workspace.clone());
            gpui_kit::component::Root::new(workspace, window, app)
        }
    });
    let mut cx = VisualTestContext::from_window(window.into(), cx);
    let workspace = captured.borrow().clone().expect("the Workspace entity");
    cx.update(|window, app| {
        crate::settings::AppSettings::update(app, |settings| {
            for path in &targets {
                settings.record_recent_target(mt_core::settings::RecentTarget::new(
                    path.clone(),
                    mt_core::settings::RecentTargetKind::File,
                    path.file_name().unwrap().to_string_lossy(),
                ));
            }
        });
        let handle = workspace.read(app).focus_handle(app);
        window.focus(&handle, app);
        window.draw(app).clear(app);
    });
    cx.run_until_parked();
    cx.update(|window, app| window.draw(app).clear(app));

    let (before, max, bounds) = workspace.read_with(&cx, |workspace, _| {
        (
            workspace.welcome.scroll.offset(),
            workspace.welcome.scroll.max_offset(),
            workspace.welcome.scroll.bounds(),
        )
    });
    assert!(
        max.y > px(0.),
        "ten recent targets must overflow vertically"
    );
    assert!(bounds.top() >= crate::metrics::title_bar());
    assert!(bounds.bottom() <= px(480.) - crate::metrics::status_bar());

    cx.simulate_event(gpui_kit::ScrollWheelEvent {
        position: point(px(360.), px(240.)),
        delta: gpui_kit::ScrollDelta::Pixels(point(px(0.), px(-2_000.))),
        ..Default::default()
    });
    cx.update(|window, app| window.draw(app).clear(app));

    let after = workspace.read_with(&cx, |workspace, _| workspace.welcome.scroll.offset());
    assert!(
        after.y < before.y,
        "the welcome page must respond to scrolling"
    );
    assert_eq!(after.y, -max.y, "the full recent list must be reachable");
}
