//! The title row holds the file's name, `Review` and the settings glyph and
//! nothing else, at any width; every other document command is in the
//! glyph's menu and works from there, and pressing the name opens Context.

mod support;

use std::time::Duration;

use nsh_engine::{EngineConfig, Points, Root};
use nsh_testing::Headless;

/// A name long enough to collide with `Review` if the row did not truncate it.
const NAME: &str = "a-planning-document-whose-name-runs-long-enough-to-reach-review.md";

/// Opens the glyph's menu and presses one of its rows.
fn from_menu(app: &mut Headless, row: &str) {
    app.press("id:title-settings")
        .expect("open the glyph's menu");
    app.press(&format!("id:{row}"))
        .unwrap_or_else(|why| panic!("press `{row}`: {why}"));
    app.wait_for_gone("id:title-menu", Duration::from_secs(5))
        .expect("the row closes the menu");
}

fn count(app: &Headless, selector: &str) -> usize {
    app.screen().count(selector).expect("selector")
}

/// The row's three controls sit left to right, inside the window, without
/// overlapping.
fn assert_row_fits(app: &Headless, width: f32) {
    let nodes = app.screen().layout_nodes();
    let bounds = |id: &str| {
        nodes
            .iter()
            .find(|node| node.test_id.as_deref() == Some(id))
            .map(|node| node.bounds)
            .unwrap_or_else(|| panic!("`{id}` on screen at {width}"))
    };
    let name = bounds("title-file");
    let view = bounds("title-view");
    let review = bounds("title-review");
    let settings = bounds("title-settings");
    assert!(
        name.size.width.0 > 0.0,
        "the name has room at {width}: {name:?}"
    );
    assert!(
        name.origin.x.0 + name.size.width.0 <= view.origin.x.0,
        "the name overlaps the view control at {width}: {name:?} {view:?}"
    );
    assert!(
        view.origin.x.0 + view.size.width.0 <= review.origin.x.0,
        "the view control overlaps Review at {width}: {view:?} {review:?}"
    );
    assert!(
        review.origin.x.0 + review.size.width.0 <= settings.origin.x.0,
        "Review overlaps the glyph at {width}: {review:?} {settings:?}"
    );
    assert!(
        settings.origin.x.0 + settings.size.width.0 <= width,
        "the glyph leaves the window at {width}: {settings:?}"
    );
}

#[test]
fn the_title_row_holds_the_name_review_and_settings() {
    let dir = tempfile::tempdir().expect("temp dir");
    let config = tempfile::tempdir().expect("config dir");
    let codex_home = tempfile::tempdir().expect("codex home");
    let path = dir.path().join(NAME);
    std::fs::write(&path, "# Plan\n").expect("write fixture");
    let save_as = dir.path().join("copy.md");

    // SAFETY: this binary has one test.
    unsafe {
        std::env::set_var("MARKTURBO_OPEN", &path);
        std::env::set_var("MARKTURBO_CONFIG_DIR", config.path());
        std::env::set_var("MARKTURBO_DATA_DIR", config.path().join("data"));
        std::env::set_var(
            "MARKTURBO_TEST_LOCK_NAMESPACE",
            std::process::id().to_string(),
        );
        // The context chain reads the user's codex home; a fresh one keeps
        // the resolution this fixture's own.
        std::env::set_var("CODEX_HOME", codex_home.path());
    }

    let mut app =
        Headless::start(EngineConfig::new(Root::of::<mt_app::App>())).expect("start the app");
    app.set_viewport(nsh_core::Size::new(Points(1440.0), Points(900.0)))
        .expect("wide viewport");
    app.wait_for("label:Markdown source")
        .expect("source editor");

    // Exactly four controls: the name, the view button, Review and Settings.
    assert_eq!(
        count(&app, "id:title-row >> role:button"),
        4,
        "the row's controls: {:?}",
        app.screen().texts()
    );
    for control in [
        format!("id:title-row >> role:button[Show agent context for {NAME}]"),
        "id:title-row >> role:button[View: Edit — switch to Split]".to_owned(),
        "id:title-row >> role:button[Review]".to_owned(),
        "id:title-row >> role:button[Settings]".to_owned(),
    ] {
        assert_eq!(count(&app, &control), 1, "`{control}` on the row");
    }
    for moved in ["Preview", "Split", "Context", "Save", "Save As\u{2026}"] {
        assert_eq!(
            count(&app, &format!("id:title-row >> text:{moved}")),
            0,
            "`{moved}` left on the row"
        );
    }

    // The row does not overlap at the spec's 1068 client width, nor below it.
    for width in [1068.0, 900.0, 640.0] {
        app.set_viewport(nsh_core::Size::new(Points(width), Points(800.0)))
            .expect("viewport");
        app.wait_for("id:title-review").expect("Review on the row");
        assert_row_fits(&app, width);
    }
    // Windows reports its caption buttons only after the first frame (and
    // again when the window is maximized): the row must make room for them
    // whenever they change, not keep the area it was built with.
    const CAPTION: f32 = 138.0;
    app.set_environment("the caption buttons arrive", |env| {
        env.title_bar = nsh_core::Edges {
            top: Points(32.0),
            left: Points(0.0),
            right: Points(CAPTION),
            bottom: Points(0.0),
        };
    })
    .expect("caption buttons");
    for width in [1440.0, 640.0] {
        app.set_viewport(nsh_core::Size::new(Points(width), Points(800.0)))
            .expect("viewport");
        app.wait_for("id:title-review").expect("Review on the row");
        assert_row_fits(&app, width - CAPTION);
    }
    app.set_viewport(nsh_core::Size::new(Points(1440.0), Points(900.0)))
        .expect("wide viewport");

    // Pressing the name opens Context; the menu's Context row closes it.
    app.press("id:title-file").expect("press the file's name");
    app.wait_for("id:context-field")
        .expect("the name opens Context");
    from_menu(&mut app, "title-menu-context");
    app.wait_for_gone("id:context-field", Duration::from_secs(5))
        .expect("the menu's Context closes it");

    // The view button cycles three modes. The menu toggles the renderer
    // independently, and both Web views keep the source mounted.
    app.press("id:title-view").expect("choose Split");
    app.wait_for("id:preview-field")
        .expect("Split: the preview in the field");
    app.press("id:title-view").expect("choose Preview");
    app.wait_for("id:preview-column")
        .expect("Preview: the rendered screen");
    from_menu(&mut app, "title-menu-web");
    app.wait_for("id:stage-dock >> id:web-pane")
        .expect("Web preview: the page");
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
    app.press("id:title-view").expect("return to Edit");
    app.wait_for_gone("id:web-pane", Duration::from_secs(5))
        .expect("Edit closes the reading screen");
    app.wait_for("id:source-editor").expect("Edit: the editor");
    app.press("id:title-view").expect("choose Split again");
    app.wait_for("id:preview-field")
        .expect("the Split caption returns");
    app.wait_for("id:stage-dock >> id:web-pane")
        .expect("the compatibility preference also applies to Split");

    // Save writes the buffer; Save As writes it to the picked path.
    app.type_text("label:Markdown source", "\nSaved from the menu.")
        .expect("edit the buffer");
    from_menu(&mut app, "title-menu-save");
    support::wait_for_file(
        &mut app,
        &path,
        |text| text.contains("Saved from the menu."),
        "Save from the menu",
        Duration::from_secs(10),
    );

    app.on_pick_save_path(Some(save_as.clone()))
        .expect("register the save path");
    from_menu(&mut app, "title-menu-save-as");
    support::wait_for_file(
        &mut app,
        &save_as,
        |text| text.contains("Saved from the menu."),
        "Save As from the menu",
        Duration::from_secs(10),
    );
    app.wait_for("id:title-row >> role:button[Show agent context for copy.md]")
        .expect("the row names the saved copy");
}
