//! The conflict comparison must never show a disk version from an earlier
//! conflict: when the file changes again and the next save is refused, the
//! panel stays folded until Compare is pressed, and then it shows the file as
//! it is now.

use std::time::Duration;

use nsh_engine::{EngineConfig, Points, Root};
use nsh_testing::Headless;

/// Whether any text on screen holds `needle` (the panel draws one span per
/// word, so a phrase is not one text).
fn shows_anywhere(app: &Headless, needle: &str) -> bool {
    app.screen()
        .texts()
        .iter()
        .any(|text| text.contains(needle))
}

#[test]
fn a_second_conflict_reads_the_disk_again() {
    let dir = tempfile::tempdir().expect("temp dir");
    let config = tempfile::tempdir().expect("config dir");
    let path = dir.path().join("notes.md");
    std::fs::write(&path, "# Original\n").expect("write fixture");

    // SAFETY: this binary has one test, so no other test reads the
    // environment concurrently.
    unsafe {
        std::env::set_var("MARKTURBO_OPEN", &path);
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
    app.wait_for("label:Markdown source")
        .expect("source editor");

    // The first conflict: an external write, an edit, a refused save.
    app.type_text("label:Markdown source", "\nLocal edit.")
        .expect("edit the buffer");
    std::fs::write(&path, "# Alpha disk\n").expect("replace the file");
    app.press("id:title-settings")
        .expect("open the glyph's menu");
    app.press("id:title-menu-save").expect("attempt the save");
    app.wait_for("id:conflict-banner")
        .expect("the conflict banner");

    // Compare reads the file: the first disk version is on screen, and the
    // control shows its open state with the selected underline.
    let closed_style = app
        .screen()
        .span_style("id:conflict-compare >> text:Compare")
        .expect("the closed Compare label");
    assert!(
        closed_style.decoration.is_none(),
        "the closed Compare is not underlined"
    );
    app.press("label:Compare").expect("compare");
    app.wait_for("id:conflict-compare")
        .expect("the compare panel");
    app.wait_for("text:Hide comparison")
        .expect("the control names its open state");
    let open_style = app
        .screen()
        .span_style("id:conflict-compare >> text:Hide comparison")
        .expect("the open Compare label");
    assert!(
        open_style.decoration.underline,
        "the open Compare shows its selected underline"
    );
    assert!(
        shows_anywhere(&app, "Alpha"),
        "the first disk version shows: {:?}",
        app.screen().texts()
    );

    // Escape alone closes the comparison.
    app.press_key("escape").expect("close the comparison");
    app.wait_for_gone("id:conflict-previous", Duration::from_secs(5))
        .expect("the comparison closes");
    app.press("label:Compare").expect("reopen the comparison");
    app.wait_for("id:conflict-previous")
        .expect("the comparison is open again");

    // Escape dismisses the topmost layer only: with the palette over the
    // comparison, it closes the palette and leaves the comparison.
    app.press_key("mod+p")
        .expect("open the palette over the comparison");
    app.wait_for("id:palette-input").expect("the palette");
    app.press_key("escape").expect("close the palette");
    app.wait_for_gone("id:palette", Duration::from_secs(5))
        .expect("the palette closes");
    app.settle();
    assert!(
        app.screen()
            .matches("id:conflict-previous")
            .expect("selector"),
        "the comparison below stays open: {:?}",
        app.screen().texts()
    );

    // Escape then closes the comparison and returns the keyboard to the
    // editor: the next plain key types into the document.
    app.press_key("escape").expect("close the comparison");
    app.wait_for_gone("id:conflict-previous", Duration::from_secs(5))
        .expect("the comparison closes");
    app.press_key("x").expect("type without clicking");
    app.wait_for(r"id:source-editor >> text:/Local edit\.x/")
        .expect("the keystroke landed in the editor");

    // Keep mine opens the in-app overwrite guard; the comparison's read is
    // left behind.
    app.press("label:Keep mine").expect("keep mine");
    app.wait_for("id:guard-overwrite")
        .expect("the overwrite guard");
    app.press("id:guard-overwrite")
        .expect("confirm the overwrite");
    app.wait_for_gone("id:conflict-banner", Duration::from_secs(5))
        .expect("the conflict is resolved");
    assert!(
        std::fs::read_to_string(&path)
            .expect("read the overwritten file")
            .contains("Local edit.")
    );

    // A second external change, an edit, and another refused save: the same
    // document is in conflict again, and nothing was read for it yet.
    std::fs::write(&path, "# Beta disk\n").expect("replace the file again");
    app.type_text("label:Markdown source", "\nSecond local edit.")
        .expect("edit the buffer again");
    app.press("id:title-settings")
        .expect("open the glyph's menu");
    app.press("id:title-menu-save")
        .expect("attempt the save again");
    app.wait_for("id:conflict-banner")
        .expect("the conflict banner again");

    // The stale comparison must not open itself, and must not be on screen.
    assert!(
        !shows_anywhere(&app, "Difference"),
        "the panel stayed folded: {:?}",
        app.screen().texts()
    );
    assert!(
        !shows_anywhere(&app, "Alpha"),
        "the earlier disk version is gone: {:?}",
        app.screen().texts()
    );

    // Pressing Compare reads the file as it is now.
    app.press("label:Compare").expect("compare again");
    app.wait_for("id:conflict-compare")
        .expect("the compare panel again");
    assert!(
        shows_anywhere(&app, "Beta"),
        "the fresh disk version shows: {:?}",
        app.screen().texts()
    );
    assert!(
        !shows_anywhere(&app, "Alpha"),
        "the earlier disk version never shows: {:?}",
        app.screen().texts()
    );
}

/// The walk's N2: with Context open and the comparison open, the editor
/// column narrows. The banner's label keeps its longest word whole and
/// every action stays pressable at each width the walk crossed.
#[test]
fn a_narrow_conflict_banner_keeps_its_label_and_actions_whole() {
    let dir = tempfile::tempdir().expect("temp dir");
    let config = tempfile::tempdir().expect("config dir");
    let path = dir.path().join("notes.md");
    std::fs::write(&path, "# Original\n").expect("write fixture");
    std::fs::write(dir.path().join("AGENTS.md"), "# Rules\n").expect("write a context source");

    // SAFETY: nextest runs each test in its own process, and the
    // environment names this test's own directories.
    unsafe {
        std::env::set_var("MARKTURBO_OPEN", &path);
        std::env::set_var("MARKTURBO_CONFIG_DIR", config.path());
        std::env::set_var("MARKTURBO_DATA_DIR", config.path().join("data"));
        std::env::set_var(
            "MARKTURBO_TEST_LOCK_NAMESPACE",
            std::process::id().to_string(),
        );
    }

    let mut app =
        Headless::start(EngineConfig::new(Root::of::<mt_app::App>())).expect("start the app");
    app.set_viewport(nsh_core::Size::new(Points(1400.0), Points(900.0)))
        .expect("the walk's viewport");
    app.wait_for("label:Markdown source")
        .expect("source editor");

    // The walk's order: the conflict and its comparison first, then Context.
    app.type_text("label:Markdown source", "\nLocal edit.")
        .expect("edit the buffer");
    std::fs::write(&path, "# Alpha disk\n").expect("replace the file");
    app.press("id:title-settings")
        .expect("open the glyph's menu");
    app.press("id:title-menu-save").expect("attempt the save");
    app.wait_for("id:conflict-banner")
        .expect("the conflict banner");
    app.press("label:Compare").expect("compare");
    app.wait_for("id:conflict-previous")
        .expect("the comparison panel");
    app.press("id:title-settings").expect("open the title menu");
    app.press("id:title-menu-context").expect("open Context");
    app.wait_for("id:context-field").expect("the context field");
    app.settle();

    let bounds = |app: &Headless, id: &str| {
        app.screen()
            .layout_nodes()
            .into_iter()
            .find(|node| node.test_id.as_deref() == Some(id))
            .unwrap_or_else(|| panic!("`{id}` on screen"))
            .bounds
    };
    for width in [1400.0_f32, 1280.0, 1100.0, 1040.0] {
        app.set_viewport(nsh_core::Size::new(Points(width), Points(900.0)))
            .expect("a width the walk crossed");
        app.settle();
        let label = bounds(&app, "conflict-label");
        assert!(
            label.size.width.0 >= mt_app::ui::tokens::CONFLICT_LABEL_MIN_W - 0.5,
            "at {width} the label crushed to {} wide",
            label.size.width.0
        );
        for (name, selector) in [
            ("conflict-compare", "label:Hide comparison"),
            ("conflict-keep-mine", "label:Keep mine"),
            ("conflict-use-disk", "label:Use disk version"),
            ("conflict-save-as", "label:Save As\u{2026}"),
        ] {
            let named = app.screen().id(selector).expect("the action");
            let target = app
                .screen()
                .press_target(selector)
                .expect("a valid selector");
            assert_eq!(
                target,
                Some(named),
                "{name} is not reachable under the label at {width}"
            );
        }
    }
}
