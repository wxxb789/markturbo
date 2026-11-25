//! The close guard's Save for an untitled draft: a successful Save As closes
//! the tab under its new id, and a failure keeps the guard with the reason
//! the failed save produced (contract amendment 3).

use std::time::Duration;

use nsh_engine::{EngineConfig, Points, Root};
use nsh_testing::Headless;

mod support;

const TIMEOUT: Duration = Duration::from_secs(20);

#[test]
fn saving_an_untitled_close_guard_finishes_or_reports() {
    let dir = tempfile::tempdir().expect("temp dir");
    let config = tempfile::tempdir().expect("config dir");

    // SAFETY: this binary has one test, and it starts one app at a time.
    unsafe {
        std::env::remove_var("MARKTURBO_OPEN");
        std::env::set_var("MARKTURBO_CONFIG_DIR", config.path());
        std::env::set_var("MARKTURBO_DATA_DIR", config.path().join("data"));
        std::env::set_var(
            "MARKTURBO_TEST_LOCK_NAMESPACE",
            std::process::id().to_string(),
        );
    }

    let destination = dir.path().join("saved-draft.md");
    {
        let mut app =
            Headless::start(EngineConfig::new(Root::of::<mt_app::App>())).expect("start the app");
        app.set_viewport(nsh_core::Size::new(Points(1280.0), Points(800.0)))
            .expect("wide viewport");
        app.wait_for("id:welcome").expect("welcome");
        app.press("label:New file").expect("new draft");
        app.wait_for("label:Markdown source")
            .expect("the draft editor");
        app.type_text("label:Markdown source", "Close draft")
            .expect("edit the draft");
        app.wait_for("id:title-dirty").expect("the dirty mark");

        app.type_text("label:Markdown source", "")
            .expect("focus the editor for Ctrl+W");
        app.press_key("mod+w").expect("press Ctrl+W");
        app.wait_for("id:guard-save").expect("the close guard");

        app.on_pick_save_path(Some(destination.clone()))
            .expect("register the save path");
        app.press("id:guard-card >> label:Save")
            .expect("save through the picker");
        support::wait_for_file(
            &mut app,
            &destination,
            |text| text == "Close draft",
            "the close guard's Save As never wrote the draft",
            TIMEOUT,
        );

        // The saved tab is the new tab: the guard closes it, so the window
        // falls back to Welcome instead of staying on the old draft.
        app.wait_for_gone("id:guard-card", TIMEOUT)
            .expect("the guard closes after the save");
        app.wait_for("id:welcome")
            .expect("the saved draft's tab closed");
        assert!(
            !app.screen().matches("id:source-editor").expect("selector"),
            "the closed draft's editor is gone: {:?}",
            app.screen().texts()
        );
    }

    // A failure keeps the guard open with the reason that failed it.
    let open = dir.path().join("open.md");
    std::fs::write(&open, "# Open\n").expect("write the open file");
    // SAFETY: the previous app has stopped; this test binary is alone.
    unsafe {
        std::env::set_var("MARKTURBO_OPEN", &open);
    }
    let mut app =
        Headless::start(EngineConfig::new(Root::of::<mt_app::App>())).expect("start the app");
    app.set_viewport(nsh_core::Size::new(Points(1280.0), Points(800.0)))
        .expect("wide viewport");
    app.wait_for("label:Markdown source")
        .expect("the open file's editor");
    app.press("id:title-settings").expect("open the menu");
    app.press("id:title-menu-new-file").expect("new draft");
    app.type_text("label:Markdown source", "Draft")
        .expect("edit the draft");
    app.wait_for("id:title-dirty")
        .expect("the draft's dirty mark");
    app.press_key("mod+w").expect("press Ctrl+W");
    app.wait_for("id:guard-save").expect("the close guard");

    app.on_pick_save_path(Some(open.clone()))
        .expect("register the already-open path");
    app.press("id:guard-card >> label:Save")
        .expect("save into the open file's path");
    app.wait_for_within(
        "text:\u{201c}open.md\u{201d} is already open. Choose another Save As path.",
        TIMEOUT,
    )
    .expect("the guard shows the failed save's reason");
    assert!(
        app.screen().matches("id:guard-save").expect("selector"),
        "a failed Save As keeps the guard open"
    );
    assert_eq!(
        std::fs::read_to_string(&open).expect("read the open file"),
        "# Open\n",
        "the failed Save As wrote nothing"
    );

    // A write failure keeps the guard open with the I/O reason itself, not
    // only a generic notice on the status line.
    let missing = dir.path().join("missing-dir").join("draft.md");
    app.on_pick_save_path(Some(missing.clone()))
        .expect("register the unwritable destination");
    app.press("id:guard-card >> label:Save")
        .expect("save into a missing directory");
    app.wait_for_within("id:guard-card >> text:/\\(os error \\d+\\)/", TIMEOUT)
        .expect("the guard shows the write failure's real reason");
    assert!(
        app.screen().matches("id:guard-save").expect("selector"),
        "a failed Save As keeps the guard open"
    );
    assert!(!missing.exists(), "the failed Save As wrote nothing");
}
