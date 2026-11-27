//! A dirty tab's own close goes through the 'Close without saving?' guard,
//! from the pointer and from the keyboard.
//!
//! The panel exposes the close button, while the collapsed strip only
//! switches tabs. The guard's Cancel keeps the tab, text and dirty mark.
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

/// Whether the close button lies inside the panel's clipping frame.
fn close_is_inside_panel(app: &Headless) -> bool {
    let panel = bounds(app, "sidebar-panel-clip");
    let close = bounds(app, "rail-close");
    panel.contains(&close.center())
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

    app.press_key("mod+b").expect("collapse the sidebar");
    app.wait_for_gone("id:sidebar-panel", Duration::from_secs(5))
        .expect("the panel closes");
    assert!(
        !app.screen().matches("id:rail-close").unwrap(),
        "the strip has no close button"
    );
    app.press_key("mod+b").expect("expand the sidebar");
    app.wait_until("panel expanded", Duration::from_secs(5), |screen| {
        screen.layout_nodes().iter().any(|node| {
            node.test_id.as_deref() == Some("sidebar")
                && node.bounds.size.width.0 == mt_app::ui::tokens::SIDEBAR_W
        })
    })
    .expect("the panel opens");
    assert!(
        node(&app, "sidebar-panel-clip").clips,
        "the panel is clipped to its track"
    );
    assert!(
        close_is_inside_panel(&app),
        "the tab's close button is in the panel"
    );
    let close_opacity = |app: &Headless| {
        app.screen()
            .view_style("id:rail-close")
            .expect("select the close control's style")
            .expect("the close control is mounted")
            .effective_opacity
    };
    assert_eq!(
        close_opacity(&app),
        0.0,
        "the close control is hidden at rest"
    );

    // The panel's close raises the guard; Cancel keeps everything.
    app.hover("id:sidebar-panel >> role:button[notes.md]")
        .expect("reveal the close control");
    app.settle();
    assert!(close_opacity(&app) > 0.0, "hover reveals the close control");
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
        app.screen().count("id:rail-close").expect("selector"),
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
        app.screen().count("id:rail-close").expect("selector"),
        0,
        "no tab is left"
    );
    assert_eq!(
        std::fs::read_to_string(&path).expect("read file"),
        ORIGINAL,
        "Don't save writes nothing"
    );
}
