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
    }

    let mut app =
        Headless::start(EngineConfig::new(Root::of::<mt_app::App>())).expect("start the app");
    app.set_viewport(nsh_core::Size::new(Points(1440.0), Points(900.0)))
        .expect("wide viewport");

    app.wait_for("label:Markdown source")
        .expect("source editor");
    // The three modes a roomy window holds are primary row actions.
    for mode in ["id:title-edit", "id:title-preview", "id:title-split"] {
        app.wait_for(mode).expect("primary mode on the row");
    }
    // The web compatibility pane is not: it waits behind the glyph's menu.
    assert!(
        !app.shows("Web preview"),
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
    app.wait_for("text:Web preview").expect("its copy");

    // Closing the disclosure hides the layout again: the menu carries it,
    // and nothing stays on the row.
    app.press("id:title-settings")
        .expect("close the glyph's menu");
    app.wait_for_gone("id:title-menu-web", Duration::from_secs(5))
        .expect("the compatibility layout is hidden again");
    assert!(!app.shows("Web preview"), "the disclosed layout stayed");

    // Reopened, the disclosed layout works: Web shows the document as a page
    // with its scripts restricted until the user trusts them.
    app.press("id:title-settings")
        .expect("open the glyph's menu");
    app.press("id:title-menu-web")
        .expect("switch to the web pane");
    app.wait_for("id:web-pane")
        .expect("the web compatibility pane");
    app.wait_for("text:Restricted: scripts disabled")
        .expect("the trust level on the pane");
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

    // A narrow window folds the modes into the menu: never a lost action.
    app.set_viewport(nsh_core::Size::new(Points(900.0), Points(700.0)))
        .expect("narrow viewport");
    app.wait_for_gone("id:title-edit", Duration::from_secs(5))
        .expect("the modes leave the row");
    app.press("id:title-settings")
        .expect("open the glyph's menu");
    app.wait_for("id:title-menu-edit")
        .expect("Edit in the menu");
    app.wait_for("id:title-menu-preview")
        .expect("Preview in the menu");
    app.wait_for("id:title-menu-split")
        .expect("Split in the menu");
}
