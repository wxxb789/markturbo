//! The shell must rebuild its copy when the interface language changes.

use nsh_engine::{EngineConfig, Points, Root};
use nsh_testing::Headless;

#[test]
fn switching_language_relocalizes_the_live_shell() {
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
    app.wait_for("text:Workspace").expect("English shell");
    app.press("text:Settings").expect("open settings");
    app.wait_for("text:Language").expect("English settings");

    app.press("text:简体中文").expect("switch language");
    app.wait_for("text:工作区").expect("Chinese shell");
    app.wait_for("text:界面语言").expect("Chinese settings");
    assert!(!app.shows("Workspace"), "English shell copy remained");
}
