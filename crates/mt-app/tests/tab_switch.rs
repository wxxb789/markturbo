//! Switching tabs must rebuild the document surface for the newly active tab.

use std::time::Duration;

use nsh_engine::{EngineConfig, Points, Root};
use nsh_testing::Headless;

#[test]
fn switching_tabs_swaps_the_document_surface() {
    let dir = tempfile::tempdir().expect("temp dir");
    let config = tempfile::tempdir().expect("config dir");
    let first = dir.path().join("first.md");
    let second = dir.path().join("second.md");
    std::fs::write(&first, "# First heading\n").expect("write first");
    std::fs::write(&second, "# Second heading\n").expect("write second");

    // SAFETY: this test binary has one test, so no other test reads the
    // environment concurrently.
    unsafe {
        std::env::set_var("MARKTURBO_OPEN", &first);
        std::env::set_var("MARKTURBO_CONFIG_DIR", config.path());
        std::env::set_var("MARKTURBO_DATA_DIR", config.path().join("data"));
    }

    let mut app =
        Headless::start(EngineConfig::new(Root::of::<mt_app::App>())).expect("start the app");
    app.set_viewport(nsh_core::Size::new(Points(1440.0), Points(900.0)))
        .expect("wide viewport");

    app.wait_for("text:# First heading")
        .expect("first document");
    app.wait_for("label:Markdown source")
        .expect("first source editor");

    // The folder's second file opens from the palette (Ctrl+P), the way the
    // rail's file list opens it: one plain press takes the preview slot.
    app.press_key("mod+p").expect("open the palette");
    app.wait_for("id:palette-input")
        .expect("the palette's line");
    app.type_text("id:palette-input", "second")
        .expect("filter to second.md");
    app.submit("id:palette-input").expect("open second.md");
    app.wait_for("text:# Second heading")
        .expect("second document replaces the first");
    app.wait_for_gone("text:# First heading", Duration::from_secs(5))
        .expect("first document surface is gone");
    assert_eq!(
        app.screen().count("id:rail-file-row").expect("selector"),
        2,
        "both open files hold a rail dot"
    );

    // The title row is mounted once, so the pane commands in its menu must
    // resolve the active tab when pressed rather than capture the startup
    // `None`.
    app.press("id:title-settings")
        .expect("open the glyph's menu");
    app.press("id:title-menu-preview")
        .expect("switch to Preview");
    app.wait_for("id:preview-column")
        .expect("the preview takes the column");
    app.wait_for_gone("label:Markdown source", Duration::from_secs(5))
        .expect("source editor hidden by the active tab's pane change");
    app.wait_for("text:Second heading")
        .expect("the active tab's preview");
}
