//! A language change rebuilds labels, not the live credential form state.

use nsh_engine::{EngineConfig, Points, Root};
use nsh_testing::Headless;

#[test]
fn language_switch_keeps_the_unsubmitted_credential() {
    let config = tempfile::tempdir().expect("config dir");

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
    app.wait_for("text:Settings").expect("shell");
    app.press("text:Settings").expect("open settings");
    app.wait_for("id:api-key").expect("credential field");
    app.type_text("id:api-key", "synthetic-key")
        .expect("type credential");

    app.press("text:简体中文").expect("switch language");
    app.wait_for("text:界面语言").expect("Chinese settings");
    let value = app
        .screen()
        .input_text("id:api-key")
        .expect("read credential field")
        .unwrap_or_default();
    assert_eq!(value, "synthetic-key");
}
