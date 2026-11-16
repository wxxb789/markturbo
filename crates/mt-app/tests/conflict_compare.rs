//! The conflict comparison must never show a disk version from an earlier
//! conflict: when the file changes again and the next save is refused, the
//! panel stays folded until Compare is pressed, and then it shows the file as
//! it is now.

use std::time::Duration;

use nsh_engine::{EngineConfig, Points, Root};
use nsh_testing::{DialogAnswer, Headless};

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

    // Compare reads the file: the first disk version is on screen.
    app.press("label:Compare").expect("compare");
    app.wait_for("id:conflict-compare")
        .expect("the compare panel");
    assert!(
        shows_anywhere(&app, "Alpha"),
        "the first disk version shows: {:?}",
        app.screen().texts()
    );

    // Keep mine resolves the conflict; the comparison's read is left behind.
    app.on_dialog(|_| DialogAnswer::Press("Overwrite".to_owned()));
    app.press("label:Keep mine").expect("keep mine");
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
