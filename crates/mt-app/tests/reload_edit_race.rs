//! A reload confirmation is bound to the revision it was confirmed for: an
//! edit made while the disk was read keeps the buffer, its recovery record
//! and the conflict instead of being replaced by the disk version.

use std::time::Duration;

use mt_core::recovery::RecoveryKey;
use nsh_engine::{EngineConfig, Points, Root};
use nsh_testing::Headless;

fn editor_text(app: &Headless) -> String {
    app.screen()
        .input_text("label:Markdown source")
        .expect("read editor text")
        .unwrap_or_default()
        .to_owned()
}

#[test]
fn edits_during_the_reload_read_stay_in_the_buffer() {
    let dir = tempfile::tempdir().expect("temp dir");
    let config = tempfile::tempdir().expect("config dir");
    let path = dir.path().join("plan.md");
    let release = dir.path().join("release-reload");
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
    app.type_text("label:Markdown source", "\n\nLocal edit.")
        .expect("edit the buffer");

    // An external write makes the save refuse, raising the conflict banner
    // with its Use-disk choice.
    std::fs::write(&path, "# External change\n").expect("replace the file");
    app.press("id:title-settings")
        .expect("open the glyph's menu");
    app.press("id:title-menu-save").expect("attempt save");
    app.wait_for("id:conflict-use-disk")
        .expect("the conflict banner's use-disk choice");

    // The reload's disk read waits on the test's gate: the user edits again
    // while the confirmation is being carried out.
    mt_app::test_vault::gate_read(&path, &release);
    app.press("id:conflict-use-disk")
        .expect("ask to reload from disk");
    app.wait_for("id:guard-reload").expect("the reload guard");
    app.press("id:guard-reload").expect("confirm the reload");
    app.type_text("label:Markdown source", "\n\nWhile reading.")
        .expect("edit during the disk read");
    std::fs::write(&release, "go").expect("release the disk read");
    app.wait(Duration::from_secs(1))
        .expect("let the reload task finish");

    // The confirmed revision moved: the buffer, its dirty mark and the
    // conflict stay; the disk text never replaces the newer edit.
    assert!(
        editor_text(&app).contains("While reading."),
        "the reload dropped the edit made during the read: {:?}",
        editor_text(&app)
    );
    assert!(
        !editor_text(&app).contains("External change"),
        "the disk version replaced the newer edit: {:?}",
        editor_text(&app)
    );
    assert!(
        app.screen().matches("id:title-dirty").expect("selector"),
        "the newer edit stays dirty"
    );
    assert!(
        app.screen()
            .matches("id:conflict-banner")
            .expect("selector"),
        "the conflict is still shown"
    );

    // The dirty buffer keeps its crash-recovery record.
    app.wait(Duration::from_secs(3))
        .expect("let the checkpoint tick write");
    let record = mt_core::runtime_paths::recovery_dir()
        .expect("the recovery directory")
        .join(format!(
            "{}.mtrecovery",
            RecoveryKey::for_path(&path).as_str()
        ));
    assert!(
        record.exists(),
        "the reload retired the recovery record of the kept edits"
    );
}
