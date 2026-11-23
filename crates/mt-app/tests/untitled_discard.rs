//! An untitled draft goes through the close decision, and discarding it
//! leaves nothing behind.
//!
//! Afterglow: the welcome's Paste turns the clipboard into an untitled
//! draft; `Ctrl+W` raises the guard card, whose Don't save closes the draft
//! without asking for a path, back to a welcome that lists no recent target,
//! and `settings.toml` records none.

use std::time::Duration;

use nsh_engine::{EngineConfig, Points, Root};
use nsh_testing::Headless;

#[test]
fn discarding_an_untitled_draft_records_no_recent() {
    let config = tempfile::tempdir().expect("config dir");

    // SAFETY: this binary has one test.
    unsafe {
        std::env::remove_var("MARKTURBO_OPEN");
        std::env::set_var("MARKTURBO_CONFIG_DIR", config.path());
        std::env::set_var("MARKTURBO_DATA_DIR", config.path().join("data"));
        std::env::set_var(
            "MARKTURBO_TEST_LOCK_NAMESPACE",
            std::process::id().to_string(),
        );
    }

    let mut app =
        Headless::start(EngineConfig::new(Root::of::<mt_app::App>())).expect("start the app");
    app.set_viewport(nsh_core::Size::new(Points(1280.0), Points(800.0)))
        .expect("wide viewport");
    app.wait_for("id:welcome-paste").expect("paste choice");

    app.set_clipboard(Some("# Pasted draft\n".to_owned()));
    app.press("label:Paste").expect("paste into a new document");
    app.wait_for("label:Markdown source")
        .expect("the pasted draft's editor");

    // Ctrl+W from the focused editor asks first.
    app.type_text("label:Markdown source", "")
        .expect("focus the editor");
    app.press_key("mod+w").expect("press Ctrl+W");
    app.wait_for("id:guard-card")
        .expect("Ctrl+W raises the guard for the draft");
    app.press("id:guard-dont-save").expect("discard the draft");
    app.wait_for("id:welcome-open-folder")
        .expect("the welcome is back");
    app.wait_for_gone("id:guard-card", Duration::from_secs(5))
        .expect("the guard goes");

    assert_eq!(
        app.screen().count("id:welcome-recent").expect("selector"),
        0,
        "a discarded draft is no recent target: {:?}",
        app.screen().texts()
    );
    let settings = std::fs::read_to_string(config.path().join("settings.toml")).unwrap_or_default();
    assert!(
        !settings.contains("[[recent-targets]]"),
        "a discarded draft records no recent target"
    );
    assert!(
        !app.screen()
            .file_requests()
            .iter()
            .any(|request| matches!(request, nsh_core::file_dialog::FileRequest::SaveDialog(_))),
        "Don't save never asks where to write the draft"
    );
}
