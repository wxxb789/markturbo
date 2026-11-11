//! A successful save must advance the loaded file's conflict stamp.

use nsh_engine::{EngineConfig, Root};
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
    }

    let mut app =
        Headless::start(EngineConfig::new(Root::of::<mt_app::App>())).expect("start the app");

    app.wait_for("label:Markdown source")
        .expect("source editor");
    app.type_text("label:Markdown source", "\nFirst edit.")
        .expect("first edit");
    app.press("text:Save").expect("first save");
    app.type_text("label:Markdown source", "\nSecond edit.")
        .expect("second edit");
    app.press("text:Save").expect("second save");
    app.settle();

    let saved = std::fs::read_to_string(&path).expect("read saved file");
    assert!(
        saved.contains("First edit."),
        "first edit was lost: {saved:?}"
    );
    assert!(
        saved.contains("Second edit."),
        "second save was refused: {saved:?}"
    );
    assert!(!app.shows("Save conflict"));
}
