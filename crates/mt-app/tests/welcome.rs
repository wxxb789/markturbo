//! Clean-profile first use must show a real welcome surface, not a blank pane.

use nsh_engine::{EngineConfig, Root};
use nsh_testing::Headless;

#[test]
fn clean_launch_shows_welcome() {
    let config = tempfile::tempdir().expect("config dir");

    // SAFETY: this test binary has one test, so no other test reads the
    // environment concurrently.
    unsafe {
        std::env::remove_var("MARKTURBO_OPEN");
        std::env::set_var("MARKTURBO_CONFIG_DIR", config.path());
        std::env::set_var("MARKTURBO_DATA_DIR", config.path().join("data"));
        std::env::set_var(
            "MARKTURBO_TEST_LOCK_NAMESPACE",
            std::process::id().to_string(),
        );
    }

    let mut app =
        Headless::start(EngineConfig::new(Root::of::<mt_app::App>())).expect("start the app");

    app.wait_for("id:welcome").expect("welcome surface");
    app.wait_for("text:Opening, editing and search stay on this device.")
        .expect("welcome guidance");

    // The welcome states its one status line once: the four choices already
    // say how to begin, so no second line repeats them under it.
    let texts = app.screen().texts();
    assert_eq!(
        app.screen()
            .count("text:Opening, editing and search stay on this device.")
            .expect("selector"),
        1,
        "the local-only line, once: {texts:?}"
    );
    assert!(
        !app.shows("Open a folder to begin."),
        "a second status line under the welcome: {texts:?}"
    );

    // On the welcome, Settings stays available and both document controls
    // are present but disabled until a document opens.
    assert_eq!(
        app.screen()
            .count("id:title-row >> role:button")
            .expect("selector"),
        3,
        "the welcome's title row: {texts:?}"
    );
    assert_eq!(
        app.screen()
            .count("id:title-row >> role:button[Settings]")
            .expect("selector"),
        1,
        "the Settings action"
    );
    assert_eq!(
        app.screen()
            .accessibility(Some("id:title-view"))
            .expect("select the view control")
            .expect("View stays in the row")
            .name,
        "View: Edit — switch to Split"
    );
    for selector in ["id:title-view", "id:title-review"] {
        assert!(
            app.screen()
                .accessibility_state(selector)
                .expect("the control stays in the row")
                .disabled,
            "{selector} is disabled without a document"
        );
        assert_eq!(
            app.screen().press_target(selector).unwrap(),
            None,
            "{selector} cannot be pressed without a document"
        );
    }
}
