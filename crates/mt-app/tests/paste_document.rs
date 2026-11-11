//! Paste creates an editable document from clipboard text.

use nsh_engine::{EngineConfig, Points, Root};
use nsh_testing::Headless;

#[test]
fn paste_creates_a_document() {
    let config = tempfile::tempdir().expect("config dir");

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
    app.set_clipboard(Some("# Pasted draft\n".to_owned()));
    app.press("text:Paste").expect("paste into a new document");
    app.wait_for("label:Markdown source")
        .expect("pasted editor");
    app.wait_for("text:Pasted draft")
        .expect("pasted text rendered");
}
