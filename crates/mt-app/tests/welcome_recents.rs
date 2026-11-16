//! A recent target that no longer exists degrades visibly and safely: its row
//! says it is missing, is a disabled button, and a press opens nothing; a
//! target that still exists reopens from its row.

use std::time::Duration;

use nsh_engine::{EngineConfig, Points, Root};
use nsh_testing::Headless;

fn recent(path: &std::path::Path) -> String {
    format!(
        "[[recent-targets]]\npath = {:?}\nkind = \"file\"\ndisplay-name = {:?}\n\n",
        path.display().to_string(),
        path.file_name().unwrap().to_str().unwrap()
    )
}

#[test]
fn a_missing_recent_is_marked_disabled_and_opens_nothing() {
    let config = tempfile::tempdir().expect("config dir");
    let workspace = tempfile::tempdir().expect("workspace");
    let present = workspace.path().join("present.md");
    std::fs::write(&present, "# Present\n").expect("write the present target");
    let missing = workspace.path().join("gone.md");
    std::fs::write(
        config.path().join("settings.toml"),
        format!("{}{}", recent(&missing), recent(&present)),
    )
    .expect("write the settings");

    // SAFETY: this binary has one test.
    unsafe {
        std::env::remove_var("MARKTURBO_OPEN");
        std::env::set_var("MARKTURBO_CONFIG_DIR", config.path());
        std::env::set_var("MARKTURBO_DATA_DIR", config.path().join("data"));
    }

    let mut app =
        Headless::start(EngineConfig::new(Root::of::<mt_app::App>())).expect("start the app");
    app.set_viewport(nsh_core::Size::new(Points(1280.0), Points(800.0)))
        .expect("wide viewport");
    app.wait_for("id:welcome-recents")
        .expect("the recent targets");
    let texts = app.screen().texts().join(" | ");

    // Both rows are buttons sharing the one id, each named by its target.
    assert_eq!(
        app.screen().count("id:welcome-recent").expect("selector"),
        2,
        "two recent rows: {texts}"
    );
    assert_eq!(
        app.screen()
            .count("role:button[File present.md]")
            .expect("selector"),
        1,
        "the present target's row: {texts}"
    );
    assert_eq!(
        app.screen()
            .count("role:button[File gone.md — Missing]")
            .expect("selector"),
        1,
        "the missing target's row names it missing: {texts}"
    );
    assert!(
        app.screen()
            .count("id:welcome-recent >> text:Missing")
            .expect("selector")
            == 1,
        "the missing row reads Missing on screen: {texts}"
    );
    assert!(
        !app.screen()
            .enabled("role:button[File gone.md — Missing]")
            .expect("selector"),
        "the missing row cannot be pressed"
    );
    assert!(
        app.screen()
            .enabled("role:button[File present.md]")
            .expect("selector"),
        "the present row can"
    );

    // A press on the missing row opens nothing and changes no file.
    let _ = app.press("role:button[File gone.md — Missing]");
    app.idle(Duration::from_millis(200)).expect("settle");
    assert!(!missing.exists(), "no file was created for the missing row");
    assert_eq!(
        app.screen().count("id:rail-file-row").expect("selector"),
        0,
        "no document opened"
    );
    app.wait_for("id:welcome-open-folder")
        .expect("the welcome stays");

    // The present row reopens its document.
    app.press("role:button[File present.md]")
        .expect("reopen the present target");
    app.wait_for("label:Markdown source")
        .expect("the reopened document's editor");
    app.wait_for_gone("id:welcome", Duration::from_secs(5))
        .expect("the welcome leaves");
}
