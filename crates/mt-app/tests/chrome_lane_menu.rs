//! The title row's menu starts documents: a new file, and the clipboard as
//! one, from the welcome and from an open document alike.

use std::{sync::Mutex, time::Duration};

use nsh_engine::{EngineConfig, Points, Root};
use nsh_testing::Headless;

static TEST_LOCK: Mutex<()> = Mutex::new(());

#[test]
fn the_menu_starts_a_new_file_and_a_pasted_document() {
    let _guard = TEST_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    let config = tempfile::tempdir().expect("config dir");

    // SAFETY: TEST_LOCK serializes app lifetimes and environment changes.
    unsafe {
        std::env::remove_var("MARKTURBO_OPEN");
        std::env::set_var("MARKTURBO_CONFIG_DIR", config.path());
        std::env::set_var("MARKTURBO_DATA_DIR", config.path().join("data"));
        std::env::set_var(
            "MARKTURBO_TEST_LOCK_NAMESPACE",
            std::process::id().to_string(),
        );
    }

    let mut app =
        Headless::start(EngineConfig::new(Root::of::<mt_app::App>())).expect("start the app");
    app.set_viewport(nsh_core::Size::new(Points(1280.0), Points(800.0)))
        .expect("wide viewport");
    app.wait_for("id:welcome").expect("the welcome page");

    // The glyph's menu carries both, with no document on screen.
    app.press("id:title-settings")
        .expect("open the glyph's menu");
    app.wait_for("id:title-menu-new-file")
        .expect("New file in the menu");
    app.wait_for("id:title-menu-paste")
        .expect("Paste as new document in the menu");
    app.wait_for("id:title-menu-open-sample")
        .expect("Open sample in the menu");
    assert_eq!(
        app.screen()
            .count("id:title-menu-context")
            .expect("selector"),
        0,
        "Context is hidden on an empty welcome"
    );
    app.wait_for("text:New file").expect("its copy");
    app.wait_for("text:Paste as new document")
        .expect("its copy");

    app.press("id:title-menu-new-file")
        .expect("start a new file");
    app.wait_for("label:Markdown source")
        .expect("the new document's editor");
    app.wait_for_gone("id:welcome", std::time::Duration::from_secs(5))
        .expect("the welcome leaves");
    assert_eq!(
        app.screen().count("id:rail-close").expect("selector"),
        1,
        "the new document is the one tab"
    );

    // With a document open, the same menu offers Context and the sample.
    app.press("id:title-settings")
        .expect("reopen the glyph's menu");
    app.wait_for("id:title-menu-context")
        .expect("Context with a document open");
    app.wait_for("id:title-menu-open-sample")
        .expect("the sample stays reachable from a workspace");
    app.press("id:title-settings")
        .expect("close the glyph's menu");

    // The key reaches it too, with a document already open.
    app.press_key("mod+n").expect("Ctrl+N");
    app.wait_until(
        "a second tab",
        std::time::Duration::from_secs(5),
        |screen| screen.count("id:rail-close").unwrap_or(0) == 2,
    )
    .expect("Ctrl+N starts another document");

    // The clipboard becomes its own document, from the same menu.
    app.set_clipboard(Some("# Pasted from the menu\n".to_owned()));
    app.press("id:title-settings")
        .expect("open the glyph's menu");
    app.press("id:title-menu-paste")
        .expect("paste as a new document");
    app.wait_for("text:# Pasted from the menu")
        .expect("the pasted text in the editor");
    assert!(
        app.shows("# Pasted from the menu"),
        "the editor holds the clipboard's bytes: {:?}",
        app.screen().texts()
    );
    assert_eq!(
        app.screen().count("id:rail-close").expect("selector"),
        3,
        "the pasted document is a third tab"
    );
}

#[test]
fn the_welcome_menu_collapses_empty_groups() {
    let _guard = TEST_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    let config = tempfile::tempdir().expect("config dir");

    // SAFETY: TEST_LOCK serializes app lifetimes and environment changes.
    unsafe {
        std::env::remove_var("MARKTURBO_OPEN");
        std::env::set_var("MARKTURBO_CONFIG_DIR", config.path());
        std::env::set_var("MARKTURBO_DATA_DIR", config.path().join("data"));
        std::env::set_var(
            "MARKTURBO_TEST_LOCK_NAMESPACE",
            std::process::id().to_string(),
        );
    }

    let mut app =
        Headless::start(EngineConfig::new(Root::of::<mt_app::App>())).expect("start the app");
    app.set_viewport(nsh_core::Size::new(Points(1280.0), Points(800.0)))
        .expect("wide viewport");
    app.wait_for("id:welcome").expect("the welcome page");
    app.press("id:title-settings").expect("open the menu");
    app.wait_for("id:title-menu").expect("the menu");
    app.settle();

    // The empty welcome: the document and context groups collapse, so one
    // divider separates the start rows from Settings and Quit.
    assert_eq!(
        app.screen()
            .count("id:title-menu-divider")
            .expect("selector"),
        1,
        "the welcome menu shows exactly one divider: {:?}",
        app.screen().texts()
    );

    // With a document, the two groups exist and both dividers separate
    // content.
    app.press("id:title-menu-new-file")
        .expect("start a new file");
    app.wait_for("label:Markdown source").expect("the editor");
    app.press("id:title-settings").expect("reopen the menu");
    app.wait_for("id:title-menu").expect("the menu");
    app.settle();
    assert_eq!(
        app.screen()
            .count("id:title-menu-divider")
            .expect("selector"),
        2,
        "the document menu shows two dividers: {:?}",
        app.screen().texts()
    );
    let dividers: Vec<f32> = app
        .screen()
        .layout_nodes()
        .into_iter()
        .filter(|node| node.test_id.as_deref() == Some("title-menu-divider"))
        .map(|node| node.bounds.origin.y.0)
        .collect();
    let save = app
        .screen()
        .layout_nodes()
        .into_iter()
        .find(|node| node.test_id.as_deref() == Some("title-menu-save"))
        .expect("the Save row")
        .bounds
        .origin
        .y
        .0;
    assert!(
        dividers[0] < save && save < dividers[1],
        "the dividers must have content between them: {dividers:?}, save at {save}"
    );
    app.press_key("escape").expect("close the menu");
    app.wait_for_gone("id:title-menu", Duration::from_secs(5))
        .expect("the menu closes");
}
