//! Uncommon layouts stay behind one inline disclosure.

use std::time::Duration;

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
        std::env::set_var(
            "MARKTURBO_TEST_LOCK_NAMESPACE",
            std::process::id().to_string(),
        );
    }

    let mut app =
        Headless::start(EngineConfig::new(Root::of::<mt_app::App>())).expect("start the app");
    app.set_viewport(nsh_core::Size::new(Points(1440.0), Points(900.0)))
        .expect("wide viewport");

    app.wait_for("label:Markdown source")
        .expect("source editor");
    app.wait_for("id:title-view >> text:Edit")
        .expect("the primary view control");
    app.press("id:title-settings").expect("open the menu");
    app.wait_for("id:title-menu-web")
        .expect("the separate compatibility toggle");
    for mode in [
        "id:title-menu-edit",
        "id:title-menu-preview",
        "id:title-menu-split",
    ] {
        assert_eq!(app.screen().count(mode).expect("selector"), 0);
    }
    app.press("id:title-settings").expect("close the menu");
    app.wait_for_gone("id:title-menu", Duration::from_secs(5))
        .expect("the menu closes");
    // The web compatibility pane waits behind the glyph's menu too.
    assert!(
        !app.shows("Compatibility preview"),
        "advanced layout shown before disclosure"
    );
    assert_eq!(
        app.screen().count("id:title-menu-web").expect("selector"),
        0,
        "the compatibility layout is only in the menu"
    );

    app.press("id:title-settings")
        .expect("open the glyph's menu");
    app.wait_for("id:title-menu").expect("menu disclosed");
    app.wait_for("id:title-menu-web")
        .expect("compatibility layout disclosed");
    app.wait_for("text:Compatibility preview")
        .expect("its copy");

    // Closing the disclosure hides the layout again: the menu carries it,
    // and nothing stays on the row.
    app.press("id:title-settings")
        .expect("close the glyph's menu");
    app.wait_for_gone("id:title-menu-web", Duration::from_secs(5))
        .expect("the compatibility layout is hidden again");
    assert!(
        !app.shows("Compatibility preview"),
        "the disclosed layout stayed"
    );

    app.press("id:title-view").expect("choose Split");
    app.press("id:title-view").expect("choose Preview");
    app.press("id:title-settings")
        .expect("open the glyph's menu");
    app.press("id:title-menu-web")
        .expect("turn on compatibility");
    app.wait_for("id:web-pane")
        .expect("the web compatibility pane");
    app.wait_for("id:web-view").expect("the document WebView");
    app.wait_for("id:title-view >> text:Preview")
        .expect("changing renderer keeps the Preview stop");
    assert!(
        !app.screen().matches("id:preview-field").unwrap(),
        "Preview has no Split caption"
    );
    assert!(
        !app.screen()
            .matches("id:preview-field >> id:web-pane")
            .unwrap()
    );
    assert_eq!(
        app.screen().count("id:web-banner").expect("selector"),
        0,
        "no trust banner in the Web pane"
    );
    app.wait_until("source screen closes", Duration::from_secs(5), |screen| {
        screen.layout_nodes().iter().any(|node| {
            node.test_id.as_deref() == Some("stage-source") && node.bounds.size.width.0 == 0.0
        })
    })
    .expect("the web reading screen fills the stage");

    // The view button returns to Edit; compatibility never adds a stop.
    app.press("id:title-view").expect("return to Edit");
    app.wait_for("label:Markdown source").expect("editor again");
    app.wait_for_gone("id:web-pane", Duration::from_secs(5))
        .expect("the web pane left");

    // A narrow window retains both controls without exposing the Web toggle.
    app.set_viewport(nsh_core::Size::new(Points(900.0), Points(700.0)))
        .expect("narrow viewport");
    app.wait_for("id:title-view >> text:Edit")
        .expect("the view button stays on the row");
    app.press("id:title-settings")
        .expect("open the glyph's menu");
    app.wait_for("id:title-menu-web >> text:On")
        .expect("the toggle is still on in the menu");
}
