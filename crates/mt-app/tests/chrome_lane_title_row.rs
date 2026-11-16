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
    let review = bounds("title-review");
    let settings = bounds("title-settings");
    assert!(
        name.size.width.0 > 0.0,
        "the name has room at {width}: {name:?}"
    );
    assert!(
        name.origin.x.0 + name.size.width.0 <= review.origin.x.0,
        "the name overlaps Review at {width}: {name:?} {review:?}"
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

    // Exactly three controls: the name, Review and the glyph named Settings.
    assert_eq!(
        count(&app, "id:title-row >> role:button"),
        3,
        "the row's controls: {:?}",
        app.screen().texts()
    );
    for control in [
        format!("id:title-row >> role:button[{NAME}]"),
        "id:title-row >> role:button[Review]".to_owned(),
        "id:title-row >> role:button[Settings]".to_owned(),
    ] {
        assert_eq!(count(&app, &control), 1, "`{control}` on the row");
    }
    for moved in [
        "Edit",
        "Preview",
        "Split",
        "Context",
        "Save",
        "Save As\u{2026}",
    ] {
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

    // The five pane modes, each from the menu. The document opens in Split.
    from_menu(&mut app, "title-menu-edit");
    app.wait_for_gone("id:preview-field", Duration::from_secs(5))
        .expect("Edit drops the field's preview");
    app.wait_for("id:source-editor").expect("Edit: the editor");

    from_menu(&mut app, "title-menu-preview");
    app.wait_for("id:preview-column")
        .expect("Preview: the rendered column");
    app.wait_for_gone("id:source-editor", Duration::from_secs(5))
        .expect("Preview replaces the editor");

    from_menu(&mut app, "title-menu-split");
    app.wait_for("id:source-editor").expect("Split: the editor");
    app.wait_for("id:preview-field")
        .expect("Split: the preview in the field");

    from_menu(&mut app, "title-menu-web");
    app.wait_for("id:web-pane").expect("Web preview: the page");
    app.wait_for_gone("id:source-editor", Duration::from_secs(5))
        .expect("Web preview replaces the editor");

    from_menu(&mut app, "title-menu-split-web");
    app.wait_for("id:source-editor")
        .expect("Split \u{00B7} Web: the editor");
    app.wait_for("id:preview-field >> id:web-pane")
        .expect("Split \u{00B7} Web: the page in the field");

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
    app.wait_for("id:title-row >> role:button[copy.md]")
        .expect("the row names the saved copy");
}
