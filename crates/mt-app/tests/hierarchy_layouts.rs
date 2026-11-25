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
    // The row holds no mode, even in a roomy window: the three modes are the
    // glyph's menu's first pane commands.
    for mode in ["id:title-edit", "id:title-preview", "id:title-split"] {
        assert_eq!(
            app.screen().count(mode).expect("selector"),
            0,
            "`{mode}` on the title row"
        );
    }
    app.press("id:title-settings")
        .expect("open the glyph's menu");
    for mode in [
        "id:title-menu-edit",
        "id:title-menu-preview",
        "id:title-menu-split",
    ] {
        app.wait_for(mode).expect("primary mode in the menu");
    }
    app.press("id:title-settings")
        .expect("close the glyph's menu");
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

    app.press("id:title-settings")
        .expect("open the glyph's menu");
    app.press("id:title-menu-web")
        .expect("switch to the web pane");
    app.wait_for("id:web-pane")
        .expect("the web compatibility pane");
    app.wait_for("id:web-view").expect("the document WebView");
    assert_eq!(
        app.screen().count("id:web-banner").expect("selector"),
        0,
        "no trust banner in the Web pane"
    );
    app.wait_for_gone("label:Markdown source", Duration::from_secs(5))
        .expect("the web pane replaces the editor");

    // The primary Edit mode comes back from the same menu.
    app.press("id:title-settings")
        .expect("open the glyph's menu");
    app.press("id:title-menu-edit")
        .expect("switch back to Edit");
    app.wait_for("label:Markdown source").expect("editor again");
    app.wait_for_gone("id:web-pane", Duration::from_secs(5))
        .expect("the web pane left");

    // A narrow window keeps the modes in the menu: never a lost action.
    app.set_viewport(nsh_core::Size::new(Points(900.0), Points(700.0)))
        .expect("narrow viewport");
    app.wait_for_gone("id:title-edit", Duration::from_secs(5))
        .expect("no mode on the row");
    app.press("id:title-settings")
        .expect("open the glyph's menu");
    app.wait_for("id:title-menu-edit")
        .expect("Edit in the menu");
    app.wait_for("id:title-menu-preview")
        .expect("Preview in the menu");
    app.wait_for("id:title-menu-split")
        .expect("Split in the menu");
}
