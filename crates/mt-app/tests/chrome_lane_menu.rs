//! The title row's menu starts documents: a new file, and the clipboard as
//! one, from the welcome and from an open document alike.

use nsh_engine::{EngineConfig, Points, Root};
use nsh_testing::Headless;

#[test]
fn the_menu_starts_a_new_file_and_a_pasted_document() {
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
    app.wait_for("id:welcome").expect("the welcome page");

    // The glyph's menu carries both, with no document on screen.
    app.press("id:title-settings")
        .expect("open the glyph's menu");
    app.wait_for("id:title-menu-new-file")
        .expect("New file in the menu");
    app.wait_for("id:title-menu-paste")
        .expect("Paste as new document in the menu");
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
        app.screen().count("id:rail-file-row").expect("selector"),
        1,
        "the new document is the one tab"
    );

    // The key reaches it too, with a document already open.
    app.press_key("mod+n").expect("Ctrl+N");
    app.wait_until(
        "a second tab",
        std::time::Duration::from_secs(5),
        |screen| screen.count("id:rail-file-row").unwrap_or(0) == 2,
    )
    .expect("Ctrl+N starts another document");

    // The clipboard becomes its own document, from the same menu.
    app.set_clipboard(Some("# Pasted from the menu\n".to_owned()));
    app.press("id:title-settings")
        .expect("open the glyph's menu");
    app.press("id:title-menu-paste")
        .expect("paste as a new document");
    app.wait_for("text:Pasted from the menu")
        .expect("the pasted text rendered");
    assert!(
        app.shows("# Pasted from the menu"),
        "the editor holds the clipboard's bytes: {:?}",
        app.screen().texts()
    );
    assert_eq!(
        app.screen().count("id:rail-file-row").expect("selector"),
        3,
        "the pasted document is a third tab"
    );
}
