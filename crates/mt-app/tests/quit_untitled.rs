//! Quit's Save must route untitled drafts through Save As.
//!
//! Afterglow: Quit raises the guard card; its Save saves the dirty draft
//! through the Save As picker before the window closes.

use std::time::{Duration, Instant};

use nsh_engine::{EngineConfig, Points, Root};
use nsh_testing::Headless;

#[test]
fn quit_save_all_saves_an_untitled_draft() {
    let dir = tempfile::tempdir().expect("temp dir");
    let config = tempfile::tempdir().expect("config dir");
    let destination = dir.path().join("quit-draft.md");

    // SAFETY: this binary has one test.
    unsafe {
        std::env::remove_var("MARKTURBO_OPEN");
        std::env::set_var("MARKTURBO_CONFIG_DIR", config.path());
        std::env::set_var("MARKTURBO_DATA_DIR", config.path().join("data"));
    }

    let mut app =
        Headless::start(EngineConfig::new(Root::of::<mt_app::App>())).expect("start the app");
    app.set_viewport(nsh_core::Size::new(Points(1280.0), Points(800.0)))
        .expect("wide viewport");
    app.wait_for("id:welcome").expect("welcome");
    app.press("label:New file").expect("new draft");
    app.wait_for("label:Markdown source").expect("draft editor");
    app.type_text("label:Markdown source", "Quit draft")
        .expect("edit draft");
    app.wait_for("id:title-dirty").expect("dirty mark");

    app.on_pick_save_path(Some(destination.clone()))
        .expect("register save path");
    app.press("label:Settings").expect("open the title menu");
    app.wait_for("id:title-menu").expect("the menu");
    app.press("id:title-menu-quit").expect("quit");
    app.wait_for("id:guard-card").expect("quit guard");
    // Each control's id names the control itself — the element a press
    // reaches and UI Automation exposes as its AutomationId.
    for (id, label) in [
        ("guard-save", "Save"),
        ("guard-dont-save", "Don't save"),
        ("guard-cancel", "Cancel"),
    ] {
        let control = app
            .screen()
            .press_target(&format!("id:guard-card >> role:button[{label}]"))
            .expect("selector");
        assert!(control.is_some(), "the {label} control");
        assert_eq!(
            app.screen()
                .press_target(&format!("id:{id}"))
                .expect("selector"),
            control,
            "`{id}` is the {label} control"
        );
    }
    app.press("id:guard-card >> label:Save")
        .expect("save before quitting");

    // Quit saves the draft through Save As from a task: pump the app's frames
    // and answer for it instead of sleeping in this thread, which never
    // advances the app.
    let deadline = Instant::now() + Duration::from_secs(20);
    while std::fs::read_to_string(&destination).unwrap_or_default() != "Quit draft" {
        assert!(
            Instant::now() < deadline,
            "quit did not save the untitled draft: {:?}",
            app.screen().texts()
        );
        app.next_change(Duration::from_millis(25))
            .expect("settle while quitting");
    }
    assert_eq!(
        std::fs::read_to_string(&destination).expect("read saved draft"),
        "Quit draft"
    );
}
