//! Numbered untitled drafts: the title names the smallest positive number no
//! other open draft holds, and closing one frees its number again.

use std::time::Duration;

use nsh_engine::{EngineConfig, Points, Root};
use nsh_testing::Headless;

#[test]
fn new_drafts_take_the_smallest_free_number() {
    let config = tempfile::tempdir().expect("config dir");

    // SAFETY: this binary has one test.
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
    app.set_viewport(nsh_core::Size::new(Points(1280.0), Points(800.0)))
        .expect("wide viewport");
    app.wait_for("id:welcome").expect("welcome");

    app.press("label:New file").expect("first draft");
    app.wait_for("id:title-file >> text:Untitled 1")
        .expect("the first draft is Untitled 1");

    app.press_key("mod+n").expect("second draft");
    app.wait_for("id:title-file >> text:Untitled 2")
        .expect("the second draft is Untitled 2");

    app.press_key("mod+n").expect("third draft");
    app.wait_for("id:title-file >> text:Untitled 3")
        .expect("the third draft is Untitled 3");

    // Close the middle draft (clean, so Ctrl+W closes it at once), then make
    // a new one: it takes the number the close freed, not the next internal
    // id.
    app.press("role:button[Untitled 2]")
        .expect("activate the middle draft");
    app.wait_for("id:title-file >> text:Untitled 2")
        .expect("the middle draft is active");
    app.press_key("mod+w").expect("close the middle draft");
    app.wait_for_gone("id:title-file >> text:Untitled 2", Duration::from_secs(5))
        .expect("the middle draft closes");

    app.press_key("mod+n").expect("replacement draft");
    app.wait_for("id:title-file >> text:Untitled 2")
        .expect("the smallest free number is reused");
    assert_eq!(
        app.screen().count("id:rail-file-row").expect("selector"),
        3,
        "still three drafts open"
    );
}
