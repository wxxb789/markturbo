//! Paste creates an editable document from clipboard text.
//!
//! Afterglow: the welcome's Paste choice turns the clipboard into an untitled
//! draft, with the editor holding the clipboard's bytes. CJK text and a
//! surrogate-pair emoji arrive unchanged: the draft saved as a file holds
//! exactly the clipboard's text.

mod support;

use std::time::Duration;

use nsh_engine::{EngineConfig, Points, Root};
use nsh_testing::Headless;

/// CJK text and a surrogate-pair emoji (U+1F980, two UTF-16 units).
const PASTED: &str = "# Pasted draft\n\n粘贴的中文 🦀 stays exact.\n";

#[test]
fn paste_creates_a_document() {
    let config = tempfile::tempdir().expect("config dir");
    let destination = config.path().join("pasted.md");

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

    app.set_clipboard(Some(PASTED.to_owned()));
    // The test id sits on the wrapper; the button inside hears the press, so
    // the press goes through the button's accessibility label.
    app.press("label:Paste").expect("paste into a new document");
    app.wait_for("label:Markdown source")
        .expect("pasted editor");
    app.wait_for("text:# Pasted draft")
        .expect("pasted text in the editor");

    // The editor itself shows the clipboard's bytes (a document opens in
    // Source, so no preview renders a copy).
    assert!(
        app.shows("# Pasted draft") && app.shows("粘贴的中文 🦀 stays exact."),
        "the editor holds the clipboard's lines: {:?}",
        app.screen().texts()
    );

    // Saved, the draft is the clipboard's text byte for byte.
    app.on_pick_save_path(Some(destination.clone()))
        .expect("register save path");
    app.press("id:title-settings").expect("open the menu");
    app.press("id:title-menu-save-as").expect("Save As");
    support::wait_for_path(
        &mut app,
        &destination,
        "Save As never wrote the draft",
        Duration::from_secs(20),
    );
    assert_eq!(
        std::fs::read(&destination).expect("read the saved draft"),
        PASTED.as_bytes(),
        "the pasted text round-trips exactly"
    );
}
