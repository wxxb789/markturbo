//! A successful save must advance the loaded file's conflict stamp.

use std::time::{Duration, Instant};

use nsh_engine::{EngineConfig, Points, Root};
use nsh_testing::Headless;

#[test]
fn saving_twice_does_not_report_its_own_first_save_as_a_conflict() {
    let dir = tempfile::tempdir().expect("temp dir");
    let config = tempfile::tempdir().expect("config dir");
    let path = dir.path().join("notes.md");
    std::fs::write(&path, "# Notes\n").expect("write fixture");

    // SAFETY: this test binary has one test, so no other test reads the
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
    app.wait_for("id:title-settings")
        .expect("the settings glyph");
    app.type_text("label:Markdown source", "\nFirst edit.")
        .expect("first edit");
    app.press("id:title-settings")
        .expect("open the glyph's menu");
    app.press("id:title-menu-save").expect("first save");
    app.type_text("label:Markdown source", "\nSecond edit.")
        .expect("second edit");
    app.press("id:title-settings")
        .expect("open the glyph's menu");
    app.press("id:title-menu-save").expect("second save");

    // Pump the app's frames while the second save lands instead of sleeping in
    // this thread, which never advances the app.
    let deadline = Instant::now() + Duration::from_secs(20);
    while !std::fs::read_to_string(&path)
        .unwrap_or_default()
        .contains("Second edit.")
    {
        assert!(
            Instant::now() < deadline,
            "the second save never landed: {:?}",
            app.screen().texts()
        );
        app.next_change(Duration::from_millis(25))
            .expect("settle while saving");
    }

    let saved = std::fs::read_to_string(&path).expect("read saved file");
    assert!(
        saved.contains("First edit."),
        "first edit was lost: {saved:?}"
    );
    assert!(
        saved.contains("Second edit."),
        "second edit was lost: {saved:?}"
    );
    assert!(
        !app.screen()
            .matches("id:conflict-banner")
            .expect("the conflict selector parses"),
        "the second save was reported as a conflict: {:?}",
        app.screen().texts()
    );
    assert!(
        !app.screen()
            .texts()
            .iter()
            .any(|text| text.contains("Save refused")),
        "the second save was refused: {:?}",
        app.screen().texts()
    );
}
