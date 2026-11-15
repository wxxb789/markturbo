//! The in-app Quit action must ask before discarding dirty buffers.
//!
//! Afterglow: Quit lives in the title menu and raises the guard card; Cancel
//! keeps the window open with the buffer untouched.

use std::time::{Duration, Instant};

use nsh_engine::{EngineConfig, Points, Root};
use nsh_testing::Headless;

#[test]
fn quit_cancel_keeps_the_dirty_document_open() {
    let dir = tempfile::tempdir().expect("temp dir");
    let config = tempfile::tempdir().expect("config dir");
    let path = dir.path().join("notes.md");
    std::fs::write(&path, "# Notes\n").expect("write fixture");

    // SAFETY: this binary has one test.
    unsafe {
        std::env::set_var("MARKTURBO_OPEN", &path);
        std::env::set_var("MARKTURBO_CONFIG_DIR", config.path());
        std::env::set_var("MARKTURBO_DATA_DIR", config.path().join("data"));
    }

    let mut app =
        Headless::start(EngineConfig::new(Root::of::<mt_app::App>())).expect("start the app");
    app.set_viewport(nsh_core::Size::new(Points(1280.0), Points(800.0)))
        .expect("wide viewport");
    app.wait_for("label:Markdown source")
        .expect("source editor");
    app.type_text("label:Markdown source", "\nUnsaved edit.")
        .expect("edit the buffer");
    app.wait_for("id:title-dirty").expect("dirty mark");

    // Quit must raise the guard card, not close silently: the gear opens the
    // title menu, whose Quit row carries its own test id.
    app.press("label:\u{2699}").expect("open the title menu");
    app.wait_for("id:title-menu").expect("the menu");
    app.press("id:title-menu-quit").expect("request quit");
    app.wait_for("id:guard-card").expect("quit guard");

    // Cancel is the guard's safe default; take it.
    app.press("id:guard-card >> label:Cancel")
        .expect("cancel the quit");
    app.wait_for_gone("id:guard-card", Duration::from_secs(5))
        .expect("the guard card goes");

    // Cancelled quit: the window (and the document) stays open, buffer intact.
    app.wait_for("label:Markdown source")
        .expect("cancelled quit keeps the window open");
    app.wait_for("id:title-dirty")
        .expect("the buffer is still dirty");
    let text = app
        .screen()
        .input_text("label:Markdown source")
        .expect("read editor text")
        .unwrap_or_default();
    assert!(text.contains("Unsaved edit."));

    // The guard is keyboard-complete: the card carries its own ring (Enter
    // with the initial selection is Cancel, which must change nothing), and
    // Right moves the ring to Save, whose Enter saves the buffer and quits.
    app.press("label:\u{2699}")
        .expect("open the title menu again");
    app.wait_for("id:title-menu").expect("the menu again");
    app.press("id:title-menu-quit").expect("request quit again");
    app.wait_for("id:guard-card").expect("quit guard again");
    app.press_key("enter")
        .expect("Enter on the default selection");
    app.wait_for_gone("id:guard-card", Duration::from_secs(5))
        .expect("Enter on the default Cancel closes the card");
    assert!(
        !std::fs::read_to_string(&path)
            .unwrap_or_default()
            .contains("Unsaved edit."),
        "Enter on the default Cancel must not write"
    );
    let text = app
        .screen()
        .input_text("label:Markdown source")
        .expect("the editor is still there")
        .unwrap_or_default();
    assert!(text.contains("Unsaved edit."), "nothing was discarded");

    app.press("label:\u{2699}")
        .expect("open the title menu once more");
    app.wait_for("id:title-menu").expect("the menu once more");
    app.press("id:title-menu-quit")
        .expect("request quit once more");
    app.wait_for("id:guard-card").expect("quit guard once more");
    app.press_key("right").expect("move the ring to Save");
    app.press_key("enter").expect("Enter on Save");
    let deadline = Instant::now() + Duration::from_secs(15);
    while !std::fs::read_to_string(&path)
        .unwrap_or_default()
        .contains("Unsaved edit.")
    {
        assert!(
            Instant::now() < deadline,
            "Enter on Save never wrote the buffer: {:?}",
            app.screen().texts()
        );
        app.next_change(Duration::from_millis(25))
            .expect("settle while saving");
    }
}
