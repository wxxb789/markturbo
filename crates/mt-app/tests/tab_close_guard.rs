//! A dirty tab's own close goes through the 'Close without saving?' guard,
//! from the pointer and from the keyboard.
//!
//! The rail's close button lies outside the resting strip's clip: only the
//! pointer over the strip widens it far enough to show (and reach) the
//! button. Its guard's Cancel keeps the tab, the text and the dirty mark.
//! `Ctrl+W` from the focused source editor raises the same guard, whose
//! Don't save closes the tab and leaves the file as it was.

use std::time::Duration;

use nsh_core::platform::{Idiom, Os};
use nsh_engine::{EngineConfig, Points, Root};
use nsh_testing::Headless;

const EDITOR: &str = "label:Markdown source";
const ORIGINAL: &str = "# Notes\n";

/// The one view with test id `id`, as layout placed it.
fn node(app: &Headless, id: &str) -> nsh_testing::LayoutNode {
    let nodes = app.screen().layout_nodes();
    let mut matching = nodes
        .iter()
        .filter(|node| node.test_id.as_deref() == Some(id));
    let node = matching
        .next()
        .unwrap_or_else(|| panic!("no `{id}` on screen"))
        .clone();
    assert!(matching.next().is_none(), "one `{id}`");
    node
}

/// The bounds of the one view with test id `id`.
fn bounds(app: &Headless, id: &str) -> nsh_core::Bounds<Points> {
    node(app, id).bounds
}

/// Whether the close button lies inside the strip that clips it.
fn close_is_inside_strip(app: &Headless) -> bool {
    let strip = bounds(app, "rail-hover");
    let close = bounds(app, "rail-close");
    close.origin.x >= strip.origin.x
        && close.origin.x + close.size.width <= strip.origin.x + strip.size.width
}

/// Whether the source editor draws the edited line (typed at the start of
/// the file's first line).
fn editor_shows_edit(app: &Headless) -> bool {
    app.screen()
        .matches("id:source-editor >> text:Unsaved edit. # Notes")
        .expect("selector")
}

#[test]
fn a_dirty_tab_close_is_guarded_from_the_rail_and_from_ctrl_w() {
    let dir = tempfile::tempdir().expect("temp dir");
    let config = tempfile::tempdir().expect("config dir");
    let path = dir.path().join("notes.md");
    std::fs::write(&path, ORIGINAL).expect("write fixture");

    // SAFETY: this binary has one test.
    unsafe {
        std::env::set_var("MARKTURBO_OPEN", &path);
        std::env::set_var("MARKTURBO_CONFIG_DIR", config.path());
        std::env::set_var("MARKTURBO_DATA_DIR", config.path().join("data"));
        std::env::set_var(
            "MARKTURBO_TEST_LOCK_NAMESPACE",
            std::process::id().to_string(),
        );
    }
    let mut engine = EngineConfig::new(Root::of::<mt_app::App>());
    engine.environment.os = Os::Windows;
    engine.environment.idiom = Idiom::Desktop;
    let mut app = Headless::start(engine).expect("start the app");
    app.set_viewport(nsh_core::Size::new(Points(1280.0), Points(800.0)))
        .expect("wide viewport");
    app.wait_for(EDITOR).expect("source editor");
    app.type_text(EDITOR, "Unsaved edit. ")
        .expect("edit the buffer");
    app.wait_for("id:title-dirty").expect("dirty mark");
    assert!(editor_shows_edit(&app), "{:?}", app.screen().texts());

    // At rest the strip clips the close button away; the pointer over the
    // strip widens it over the button.
    assert!(
        node(&app, "rail-hover").clips,
        "the strip clips what lies outside it"
    );
    assert!(
        !close_is_inside_strip(&app),
        "the resting strip shows no close button"
    );
    app.hover("id:rail-hover").expect("hover the rail");
    app.settle();
    assert!(
        close_is_inside_strip(&app),
        "the widened strip shows the close button"
    );

    // The rail's close raises the guard; Cancel keeps everything.
    app.press("id:rail-close").expect("close the tab");
    app.wait_for("id:guard-cancel")
        .expect("the close raises the guard");
    app.press("id:guard-card >> label:Cancel")
        .expect("cancel the close");
    app.wait_for_gone("id:guard-cancel", Duration::from_secs(5))
        .expect("the guard goes");
    assert!(editor_shows_edit(&app), "Cancel keeps the text");
    app.wait_for("id:title-dirty")
        .expect("Cancel keeps the dirty mark");
    assert_eq!(
        app.screen().count("id:rail-file-row").expect("selector"),
        1,
        "Cancel keeps the tab"
    );

    // Ctrl+W from the focused editor: the same guard; Don't save closes the
    // tab without writing.
    app.type_text(EDITOR, "").expect("focus the editor");
    app.press_key("mod+w").expect("press Ctrl+W");
    app.wait_for("id:guard-dont-save")
        .expect("Ctrl+W raises the guard");
    app.press("id:guard-card >> label:Don't save")
        .expect("discard");
    app.wait_for_gone(EDITOR, Duration::from_secs(5))
        .expect("Don't save closes the tab");
    assert_eq!(
        app.screen().count("id:rail-file-row").expect("selector"),
        0,
        "no tab is left"
    );
    assert_eq!(
        std::fs::read_to_string(&path).expect("read file"),
        ORIGINAL,
        "Don't save writes nothing"
    );
}
