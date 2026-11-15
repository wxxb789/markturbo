//! Paste creates an editable document from clipboard text.
//!
//! Afterglow: the welcome's Paste choice turns the clipboard into an untitled
//! draft, with the editor holding the clipboard's bytes and the preview
//! rendering them.

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
    app.wait_for("id:welcome").expect("welcome");
    app.wait_for("id:welcome-paste").expect("paste choice");

    app.set_clipboard(Some("# Pasted draft\n".to_owned()));
    // The test id sits on the wrapper; the button inside hears the press, so
    // the press goes through the button's accessibility label.
    app.press("label:Paste").expect("paste into a new document");
    app.wait_for("label:Markdown source")
        .expect("pasted editor");
    app.wait_for("text:Pasted draft")
        .expect("pasted text rendered");

    // The editor itself shows the clipboard's bytes (the preview above shows
    // the rendered copy).
    assert!(
        app.shows("# Pasted draft"),
        "the editor holds the clipboard's bytes: {:?}",
        app.screen().texts()
    );
}
