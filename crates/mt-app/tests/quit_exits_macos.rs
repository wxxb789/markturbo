//! On macOS the app must quit, not only close its window: neither the
//! platform's last-window policy nor the Quit menu is allowed to leave it in
//! the Dock after the guard's Don't save.

use nsh_core::platform::{Idiom, Os};
use nsh_engine::{EngineConfig, Points, Root};
use nsh_testing::Headless;

#[test]
fn quit_dont_save_exits_the_app_on_macos() {
    let dir = tempfile::tempdir().expect("temp dir");
    let config = tempfile::tempdir().expect("config dir");
    let path = dir.path().join("notes.md");
    std::fs::write(&path, "# Notes\n").expect("write fixture");

    // SAFETY: this binary has one test.
    unsafe {
        std::env::set_var("MARKTURBO_OPEN", &path);
        std::env::set_var("MARKTURBO_CONFIG_DIR", config.path());
        std::env::set_var("MARKTURBO_DATA_DIR", config.path().join("data"));
        std::env::set_var(
            "MARKTURBO_TEST_LOCK_NAMESPACE",
            std::process::id().to_string(),
        );
    }

    let mut engine = EngineConfig::new(Root::of::<mt_app::App>());
    engine.environment.os = Os::MacOs;
    engine.environment.idiom = Idiom::Desktop;
    let mut app = Headless::start(engine).expect("start the app");
    app.set_viewport(nsh_core::Size::new(Points(1280.0), Points(800.0)))
        .expect("wide viewport");
    app.wait_for("label:Markdown source")
        .expect("source editor");
    app.type_text("label:Markdown source", "\nUnsaved edit.")
        .expect("edit the buffer");
    app.wait_for("id:title-dirty").expect("dirty mark");

    app.press("label:Settings").expect("open the title menu");
    app.wait_for("id:title-menu").expect("the menu");
    app.press("id:title-menu-quit").expect("request quit");
    app.wait_for("id:guard-dont-save").expect("quit guard");
    app.press("id:guard-card >> label:Don't save")
        .expect("discard and quit");
    app.settle();
    assert!(
        app.quit(),
        "Don't save must end the app on macOS, not leave it in the Dock"
    );
    assert_eq!(
        std::fs::read_to_string(&path).expect("read file"),
        "# Notes\n",
        "Don't save writes nothing"
    );
}
