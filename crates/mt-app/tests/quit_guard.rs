//! The in-app Quit action must ask before discarding dirty buffers.
//!
//! Afterglow: Quit lives in the title menu and raises the guard card; Cancel
//! keeps the window open with the buffer untouched.

use std::time::Duration;

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
}
