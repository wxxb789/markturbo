//! Switching tabs must rebuild the document surface for the newly active tab.

use nsh_engine::{EngineConfig, Root};
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
    app.set_viewport(nsh_core::Size::new(
        nsh_engine::Points(1280.0),
        nsh_engine::Points(800.0),
    ))
    .expect("wide viewport for the toolbar");

    app.wait_for("text:First heading").expect("first document");
    app.press("text:second.md").expect("open second tab");
    app.wait_for("text:Second heading")
        .expect("second document replaces the first");
    app.wait_for_gone("text:First heading", std::time::Duration::from_secs(5))
        .expect("first document surface is gone");

    // The toolbar is mounted once, so its pane controls must resolve the
    // active tab when pressed rather than capture the startup `None`.
    app.press("text:Preview").expect("switch to Preview");
    app.wait_for_gone("label:Markdown source", std::time::Duration::from_secs(5))
        .expect("source editor hidden by the active tab's pane change");
}
