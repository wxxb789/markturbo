//! The window's declared minimum: at it the title row's controls stay inside
//! the client area and the editor column's Undo stays pressable in Split.

use std::time::Duration;

use nsh_engine::{EngineConfig, Points, Root};
use nsh_testing::Headless;

fn bounds(app: &Headless, id: &str) -> nsh_core::geometry::Bounds<Points> {
    app.screen()
        .layout_nodes()
        .into_iter()
        .find(|node| node.test_id.as_deref() == Some(id))
        .unwrap_or_else(|| panic!("`{id}` on screen"))
        .bounds
}

#[test]
fn the_minimum_window_keeps_the_title_row_and_the_editor_undo_usable() {
    let config = tempfile::tempdir().expect("config dir");
    // SAFETY: nextest runs each test in its own process; this test owns its
    // environment.
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
        .expect("a wide start");
    app.wait_for("id:shell").expect("the shell");

    // A document with a live Undo on the editor's own status line.
    app.press_key("mod+n").expect("a new document");
    app.wait_for("label:Markdown source").expect("the editor");
    app.press("id:title-settings").expect("open the title menu");
    app.wait_for("id:title-menu").expect("the menu");
    app.press("id:title-menu-settings").expect("open Settings");
    app.wait_for("id:settings-page").expect("the settings page");
    app.press("id:usage-clear").expect("clear the counts");
    app.wait_for("id:status-action").expect("the Undo");
    app.press("id:settings-close").expect("close Settings");
    app.wait_for("id:status-action")
        .expect("the Undo follows the editor surface");

    // Split: the editor column holds the Undo at its foot.
    for _ in 0..3 {
        if app
            .screen()
            .matches("id:title-view >> text:Split")
            .expect("a valid selector")
        {
            break;
        }
        app.press("id:title-view").expect("cycle the view");
        app.settle();
    }
    assert!(
        app.screen()
            .matches("id:title-view >> text:Split")
            .expect("a valid selector"),
        "the view is Split: {:?}",
        app.screen().texts()
    );

    let minimum = nsh_core::Size::new(
        Points(mt_app::ui::tokens::WINDOW_MIN_W),
        Points(mt_app::ui::tokens::WINDOW_MIN_H),
    );
    app.set_viewport(minimum).expect("the minimum viewport");
    app.settle();

    // Every title-row control is inside the row and the window, side by side.
    let row = bounds(&app, "title-row");
    assert_eq!(
        row.size.width.0,
        mt_app::ui::tokens::WINDOW_MIN_W,
        "the shell fills the minimum width"
    );
    let controls: Vec<(String, nsh_core::geometry::Bounds<Points>)> = [
        "title-view",
        "title-review",
        "title-settings",
        "title-menu-anchor",
    ]
    .into_iter()
    .map(|id| (id.to_owned(), bounds(&app, id)))
    .collect();
    for (id, at) in &controls {
        assert!(
            at.origin.x.0 >= -0.5
                && at.origin.y.0 >= -0.5
                && at.origin.x.0 + at.size.width.0 <= mt_app::ui::tokens::WINDOW_MIN_W + 0.5
                && at.origin.y.0 + at.size.height.0 <= row.size.height.0 + 0.5,
            "{id} is outside the title row at the minimum: {at:?}"
        );
    }
    let mut leaves: Vec<(String, nsh_core::geometry::Bounds<Points>)> =
        ["title-view", "title-review", "title-menu-anchor"]
            .into_iter()
            .map(|id| (id.to_owned(), bounds(&app, id)))
            .collect();
    leaves.sort_by(|a, b| a.1.origin.x.0.total_cmp(&b.1.origin.x.0));
    for pair in leaves.windows(2) {
        let (left, right) = (&pair[0], &pair[1]);
        assert!(
            left.1.origin.x.0 + left.1.size.width.0 <= right.1.origin.x.0 + 0.5,
            "{} overlaps {} at the minimum: {:?} / {:?}",
            left.0,
            right.0,
            left.1,
            right.1
        );
    }

    // The Undo is on screen, reachable, and restores.
    let undo = bounds(&app, "status-action");
    assert!(
        undo.origin.x.0 >= -0.5
            && undo.origin.y.0 >= -0.5
            && undo.origin.x.0 + undo.size.width.0 <= mt_app::ui::tokens::WINDOW_MIN_W + 0.5
            && undo.origin.y.0 + undo.size.height.0 <= mt_app::ui::tokens::WINDOW_MIN_H + 0.5,
        "the Undo is outside the minimum window: {undo:?}"
    );
    let undo_control = app.screen().id("id:status-undo").expect("the Undo control");
    assert_eq!(
        app.screen()
            .press_target("id:status-action")
            .expect("a valid selector"),
        Some(undo_control),
        "the Undo's own control hears the press at the minimum"
    );
    app.press("id:status-action").expect("undo the clear");
    app.wait_for_gone("id:status-action", Duration::from_secs(5))
        .expect("the Undo is spent");
}
