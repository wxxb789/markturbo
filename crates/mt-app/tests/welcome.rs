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
    }

    let mut app =
        Headless::start(EngineConfig::new(Root::of::<mt_app::App>())).expect("start the app");

    app.wait_for("id:welcome").expect("welcome surface");
    app.wait_for("text:Create a new document or open one from your computer.")
        .expect("welcome guidance");
}
