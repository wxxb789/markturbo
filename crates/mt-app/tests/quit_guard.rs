//! The in-app Quit action must ask before discarding dirty buffers.

use nsh_engine::{EngineConfig, Points, Root};
use nsh_testing::{DialogAnswer, Headless};

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

    app.on_dialog(|_| DialogAnswer::Press("Cancel".to_owned()));
    app.press("text:Quit").expect("request quit");
    app.settle();

    app.wait_for("label:Markdown source")
        .expect("cancelled quit keeps the window open");
    let text = app
        .screen()
        .input_text("label:Markdown source")
        .expect("read editor text")
        .unwrap_or_default();
    assert!(text.contains("Unsaved edit."));
}
