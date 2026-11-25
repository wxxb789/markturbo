//! Compare on a conflicted document whose file vanished reports the failed
//! read instead of doing nothing.

use std::time::Duration;

use nsh_engine::{EngineConfig, Points, Root};
use nsh_testing::Headless;

#[test]
fn compare_reports_a_failed_disk_read() {
    let dir = tempfile::tempdir().expect("temp dir");
    let config = tempfile::tempdir().expect("config dir");
    let path = dir.path().join("notes.md");
    std::fs::write(&path, "# Original\n").expect("write fixture");

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

    let mut app =
        Headless::start(EngineConfig::new(Root::of::<mt_app::App>())).expect("start the app");
    app.set_viewport(nsh_core::Size::new(Points(1280.0), Points(800.0)))
        .expect("wide viewport");
    app.wait_for("label:Markdown source")
        .expect("source editor");

    app.type_text("label:Markdown source", "\nLocal edit.")
        .expect("edit the buffer");
    std::fs::write(&path, "# External change\n").expect("replace the file");
    app.press("id:title-settings")
        .expect("open the glyph's menu");
    app.press("id:title-menu-save").expect("attempt the save");
    app.wait_for("id:conflict-banner")
        .expect("the conflict banner");

    // The file is gone; Compare must say the read failed and stay folded.
    std::fs::remove_file(&path).expect("remove the conflicted file");
    app.press("label:Compare").expect("compare");
    app.wait_until(
        "the failed read to be reported",
        Duration::from_secs(5),
        |screen| {
            screen
                .texts()
                .iter()
                .any(|text| text.starts_with("Reload failed"))
        },
    )
    .expect("Compare reports the failed disk read");
    assert_eq!(
        app.screen()
            .count("id:conflict-previous")
            .expect("selector"),
        0,
        "a failed read opens no comparison: {:?}",
        app.screen().texts()
    );
    assert!(
        app.screen().matches("label:Compare").expect("selector"),
        "the button still offers Compare"
    );
}
