//! A clean profile shows one welcome surface, not an empty editor toolbar.

use nsh_engine::{EngineConfig, Points, Root};
use nsh_testing::Headless;

#[test]
fn welcome_has_one_primary_action_and_no_document_toolbar() {
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
        .expect("welcome title");
    app.wait_for("text:Open Folder")
        .expect("primary open action");
    app.wait_for("text:Review")
        .expect("Review remains discoverable");

    assert!(!app.shows("Save"), "document toolbar leaked into Welcome");
    assert!(
        !app.shows("Compatibility Preview"),
        "advanced layouts leaked into Welcome"
    );
}
