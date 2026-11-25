//! Headless acceptance for the save-conflict banner: an external write is
//! detected, saving is refused, and the explicit resolution choices appear.
//!
//! Afterglow: the title row's Save refuses the write, the column shows the
//! 'Changed on disk' banner with its Compare, Keep mine, Use disk version
//! choices, and 'Keep mine' still asks through the platform's destructive
//! confirmation before anything on disk changes.

use std::io::Write;
use std::time::Duration;

use nsh_engine::{EngineConfig, Points, Root};
use nsh_testing::Headless;

#[test]
fn external_change_shows_conflict_resolution() {
    let dir = tempfile::tempdir().expect("temp dir");
    let config = tempfile::tempdir().expect("config dir");
    let path = dir.path().join("AGENTS.md");
    let mut file = std::fs::File::create(&path).expect("create fixture");
    // The same length as the external rewrite below.
    writeln!(file, "# Original header").expect("write fixture");
    drop(file);

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
    app.type_text("label:Markdown source", "\n\nLocal edit.")
        .expect("edit the buffer");

    // A same-length rewrite with the original modification time restored:
    // neither the size nor the stamp tells, so only the save's own check of
    // the file can refuse the write.
    let modified = std::fs::metadata(&path)
        .and_then(|metadata| metadata.modified())
        .expect("the fixture's modification time");
    std::fs::write(&path, "# External change\n").expect("replace the file");
    std::fs::OpenOptions::new()
        .write(true)
        .open(&path)
        .and_then(|file| file.set_times(std::fs::FileTimes::new().set_modified(modified)))
        .expect("restore the modification time");
    assert_eq!(
        std::fs::metadata(&path)
            .and_then(|metadata| metadata.modified())
            .expect("the rewritten file's modification time"),
        modified,
        "the test must neutralize the stamp check"
    );
    app.wait_for("id:title-settings")
        .expect("the settings glyph");
    app.press("id:title-settings")
        .expect("open the glyph's menu");
    app.press("id:title-menu-save").expect("attempt save");

    // The refusal is visible with its decisions, and nothing was written.
    app.wait_for("id:conflict-banner").expect("conflict banner");
    app.wait_for("id:conflict-use-disk")
        .expect("use-disk choice");
    app.wait_for("id:conflict-keep-mine")
        .expect("keep-mine choice");
    assert_eq!(
        std::fs::read_to_string(&path).expect("read untouched file"),
        "# External change\n",
        "a refused save must not touch the disk"
    );
    // The editor's whole text (a text selector trims the typed blank lines).
    assert!(
        app.screen()
            .matches("id:source-editor >> text:Local edit.# Original header")
            .expect("selector"),
        "a refused save keeps the edit in the editor: {:?}",
        app.screen().texts()
    );

    // 'Keep mine' opens the quiet in-app guard, not a native alert: nothing
    // is written until its Overwrite is pressed.
    app.press("label:Keep mine").expect("choose keep mine");
    app.wait_for("id:guard-overwrite")
        .expect("the overwrite guard");
    assert_eq!(
        std::fs::read_to_string(&path).expect("read the file while asked"),
        "# External change\n",
        "nothing is written before the overwrite is confirmed"
    );
    app.press("id:guard-overwrite")
        .expect("confirm the overwrite");
    app.wait_for_gone("id:conflict-banner", Duration::from_secs(5))
        .expect("conflict resolved");
    let saved = std::fs::read_to_string(&path).expect("read overwritten file");
    assert!(saved.contains("Local edit."));
    assert!(!saved.contains("External change"));
}
