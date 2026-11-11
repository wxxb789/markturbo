//! A new untitled document can be authored and saved through Save As.

use nsh_engine::{EngineConfig, Points, Root};
use nsh_testing::Headless;

#[test]
fn new_document_saves_with_save_as() {
    let dir = tempfile::tempdir().expect("temp dir");
    let config = tempfile::tempdir().expect("config dir");
    let destination = dir.path().join("notes.md");

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
    app.wait_for("text:Start a Markdown document")
        .expect("welcome");
    app.press("text:New").expect("create untitled document");
    app.wait_for("label:Markdown source")
        .expect("untitled editor");
    app.type_text("label:Markdown source", "# Draft")
        .expect("type draft");

    app.on_pick_save_path(Some(destination.clone()))
        .expect("register the save path");
    app.press("text:Save").expect("Save As for untitled");
    app.wait_for("text:notes.md").expect("saved tab title");
    assert_eq!(
        std::fs::read_to_string(&destination).expect("read saved file"),
        "# Draft"
    );
}
