//! Headless acceptance for the save-conflict banner: an external write is
//! detected, saving is refused, and the explicit resolution choices appear.

use std::io::Write;

use nsh_engine::{EngineConfig, Root};
use nsh_testing::Headless;

#[test]
fn external_change_shows_conflict_resolution() {
    let dir = tempfile::tempdir().expect("temp dir");
    let config = tempfile::tempdir().expect("config dir");
    let path = dir.path().join("AGENTS.md");
    let mut file = std::fs::File::create(&path).expect("create fixture");
    writeln!(file, "# Original").expect("write fixture");
    drop(file);

    // SAFETY: this binary has one test, so no other test reads the
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
    app.type_text("label:Markdown source", "\n\nLocal edit.")
        .expect("edit the buffer");

    std::fs::write(&path, "# External change\n").expect("replace the file");
    app.press("text:Save").expect("attempt save");

    app.wait_for("text:Reload").expect("reload choice");
    app.wait_for("text:Overwrite").expect("overwrite choice");

    app.on_dialog(|_| nsh_testing::DialogAnswer::Press("Overwrite".to_owned()));
    app.press("text:Overwrite").expect("choose overwrite");
    app.wait_for_gone("text:Reload", std::time::Duration::from_secs(5))
        .expect("conflict resolved");
    let saved = std::fs::read_to_string(&path).expect("read overwritten file");
    assert!(saved.contains("Local edit."));
    assert!(!saved.contains("External change"));
}
