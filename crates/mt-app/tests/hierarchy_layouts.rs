//! Uncommon layouts stay behind one inline disclosure.

use nsh_engine::{EngineConfig, Points, Root};
use nsh_testing::Headless;

#[test]
fn compatibility_layouts_are_progressively_disclosed() {
    let dir = tempfile::tempdir().expect("temp dir");
    let config = tempfile::tempdir().expect("config dir");
    let document = dir.path().join("plan.md");
    std::fs::write(&document, "# Plan\n").expect("write fixture");

    // SAFETY: this binary has one test.
    unsafe {
        std::env::set_var("MARKTURBO_OPEN", &document);
        std::env::set_var("MARKTURBO_CONFIG_DIR", config.path());
        std::env::set_var("MARKTURBO_DATA_DIR", config.path().join("data"));
    }

    let mut app =
        Headless::start(EngineConfig::new(Root::of::<mt_app::App>())).expect("start the app");
    app.set_viewport(nsh_core::Size::new(Points(720.0), Points(600.0)))
        .expect("wide viewport");

    app.wait_for("label:Markdown source")
        .expect("source editor");
    app.wait_for("text:Edit").expect("primary Edit mode");
    app.wait_for("text:Preview").expect("primary Preview mode");
    assert!(
        !app.shows("Compatibility Preview"),
        "advanced layout shown before disclosure"
    );

    app.press("text:More layouts")
        .expect("open layout disclosure");
    app.wait_for("text:Compatibility Preview")
        .expect("compatibility layout disclosed");
    app.press("text:Fewer layouts")
        .expect("close layout disclosure");
    app.wait_for_gone(
        "text:Compatibility Preview",
        std::time::Duration::from_secs(5),
    )
    .expect("compatibility layout hidden again");
}
