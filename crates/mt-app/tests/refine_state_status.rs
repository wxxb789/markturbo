//! The status line names a file by its name, never by the folder it sits in;
//! a done message fades after a moment, an error stays.

use std::time::{Duration, SystemTime};

use nsh_engine::{EngineConfig, Points, Root};
use nsh_testing::Headless;

/// Any status text that is not blank.
const STATUS_TEXT: &str = r"id:status >> text:/\S/";

#[test]
fn the_status_line_names_the_file_and_fades_while_an_error_stays() {
    let dir = tempfile::tempdir().expect("temp dir");
    let config = tempfile::tempdir().expect("config dir");
    let document = dir.path().join("plan.md");
    std::fs::write(&document, "# Plan\n").expect("write fixture");

    // SAFETY: this test binary has one test, so no other test reads the
    // environment concurrently.
    unsafe {
        std::env::set_var("MARKTURBO_OPEN", &document);
        std::env::set_var("MARKTURBO_CONFIG_DIR", config.path());
        std::env::set_var("MARKTURBO_DATA_DIR", config.path().join("data"));
    }

    let mut engine = EngineConfig::new(Root::of::<mt_app::App>());
    engine.clock = nsh_core::time::Clock::manual(SystemTime::now());
    let mut app = Headless::start(engine).expect("start the app");
    app.set_viewport(nsh_core::Size::new(Points(1280.0), Points(800.0)))
        .expect("wide viewport");
    app.wait_for("label:Markdown source")
        .expect("source editor");

    // The whole line is the file's name: the folder's path is not in it.
    app.wait_for("id:status >> text:Opened plan.md")
        .expect("the status names the opened file");
    let shown = app.screen().texts().join("\n");
    assert!(
        !shown.contains(&*dir.path().to_string_lossy()),
        "a local path reached the screen: {shown}"
    );

    // A done message stays readable for a moment, then leaves the line.
    app.run_for(Duration::from_millis(2_000)).expect("a moment");
    assert!(
        app.shows("Opened plan.md"),
        "the done message left too early"
    );
    app.run_for(Duration::from_millis(1_500)).expect("the fade");
    assert_eq!(
        app.screen().count(STATUS_TEXT).expect("selector"),
        0,
        "the done message is still on the line: {:?}",
        app.screen().texts()
    );

    // A save that fails says why, and the line holds it.
    std::fs::remove_file(&document).expect("remove the file under the tab");
    app.press("id:title-settings")
        .expect("open the glyph's menu");
    app.press("id:title-menu-save").expect("save");
    app.wait_for("id:status >> text:/^Save/")
        .expect("the save failure on the line");
    let failure = app.screen().texts().join("\n");
    assert!(
        !failure.contains("Missing") && !failure.contains("Io("),
        "the error is shown in its Debug form: {failure}"
    );
    app.run_for(Duration::from_millis(6_000))
        .expect("well past the fade");
    assert_eq!(
        app.screen()
            .count("id:status >> text:/^Save/")
            .expect("selector"),
        1,
        "the error left the line: {:?}",
        app.screen().texts()
    );
}
