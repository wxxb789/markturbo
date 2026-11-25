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

    // SAFETY: nextest runs each test in its own process.
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
    if cfg!(windows) {
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
}

/// Two Reloads of one tab whose answers come back in reverse order: the
/// later confirmation's disk version lands first, and the older request's
/// answer must not mistake that sync for an edit and re-raise the conflict.
#[test]
fn a_stale_reload_answer_does_not_reconflict_a_synced_tab() {
    let dir = tempfile::tempdir().expect("temp dir");
    let config = tempfile::tempdir().expect("config dir");
    let path = dir.path().join("plan.md");
    let first_release = dir.path().join("release-first");
    let second_release = dir.path().join("release-second");
    std::fs::write(&path, "# Original\n").expect("write fixture");

    // SAFETY: nextest runs each test in its own process.
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

    // An external write makes the save refuse, raising the conflict banner
    // with its Use-disk choice.
    std::fs::write(&path, "# External change\n").expect("replace the file");
    app.press("id:title-settings")
        .expect("open the glyph's menu");
    app.press("id:title-menu-save").expect("attempt save");
    app.wait_for("id:conflict-use-disk")
        .expect("the conflict banner's use-disk choice");

    // Each of the two Reloads parks on its own release, so the second's
    // answer is forced to land before the first's.
    mt_app::test_vault::gate_reads(&path, [first_release.clone(), second_release.clone()]);
    app.press("id:conflict-use-disk")
        .expect("ask to reload from disk");
    app.wait_for("id:guard-reload").expect("the reload guard");
    app.press("id:guard-reload")
        .expect("confirm the first reload");
    app.wait_until(
        "the first reload's read to reach its gate",
        Duration::from_secs(5),
        |_| mt_app::test_vault::gate_reads_started(&path) == 1,
    )
    .expect("the first reload waits on the disk");
    app.press("id:conflict-use-disk")
        .expect("ask to reload from disk again");
    app.wait_for("id:guard-reload")
        .expect("the second reload guard");
    app.press("id:guard-reload")
        .expect("confirm the second reload");
    app.wait_until(
        "the second reload's read to reach its gate",
        Duration::from_secs(5),
        |_| mt_app::test_vault::gate_reads_started(&path) == 2,
    )
    .expect("the second reload waits on the disk");

    // The second request's answer arrives first and syncs the tab.
    std::fs::write(&second_release, "go").expect("release the second reload");
    app.wait_for_gone("id:conflict-banner", Duration::from_secs(20))
        .expect("the disk version lands and the conflict clears");
    assert!(
        editor_text(&app).contains("External change"),
        "the disk version is in the buffer: {:?}",
        editor_text(&app)
    );

    // The first request's answer arrives last. It is stale: the tab is
    // already synced, so no conflict may come back and the buffer stays.
    std::fs::write(&first_release, "go").expect("release the first reload");
    app.wait_until(
        "the stale reload answer to be handled",
        Duration::from_secs(10),
        |_| mt_app::test_vault::reload_superseded(&path),
    )
    .expect("the stale reload answer was handled");
    assert!(
        !app.screen()
            .matches("id:conflict-banner")
            .expect("selector"),
        "the stale reload re-conflicted the synced tab: {:?}",
        app.screen().texts()
    );
    assert!(
        editor_text(&app).contains("External change"),
        "the stale answer wrote over the synced buffer: {:?}",
        editor_text(&app)
    );
}

/// The same reverse order with a *failing* older read: the newer Reload
/// syncs the tab, then the older read's source is gone and its answer is an
/// error. The token check owns that answer before any notice, so the stale
/// failure must not post `Reload failed` over the newer sync.
#[test]
fn a_stale_reload_failure_writes_no_notice() {
    let dir = tempfile::tempdir().expect("temp dir");
    let config = tempfile::tempdir().expect("config dir");
    let path = dir.path().join("plan.md");
    let first_release = dir.path().join("release-first");
    let second_release = dir.path().join("release-second");
    std::fs::write(&path, "# Original\n").expect("write fixture");

    // SAFETY: nextest runs each test in its own process.
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

    // An external write makes the save refuse, raising the conflict banner
    // with its Use-disk choice.
    std::fs::write(&path, "# External change\n").expect("replace the file");
    app.press("id:title-settings")
        .expect("open the glyph's menu");
    app.press("id:title-menu-save").expect("attempt save");
    app.wait_for("id:conflict-use-disk")
        .expect("the conflict banner's use-disk choice");

    // Each of the two Reloads parks on its own release, so the second's
    // answer is forced to land before the first's.
    mt_app::test_vault::gate_reads(&path, [first_release.clone(), second_release.clone()]);
    app.press("id:conflict-use-disk")
        .expect("ask to reload from disk");
    app.wait_for("id:guard-reload").expect("the reload guard");
    app.press("id:guard-reload")
        .expect("confirm the first reload");
    app.wait_until(
        "the first reload's read to reach its gate",
        Duration::from_secs(5),
        |_| mt_app::test_vault::gate_reads_started(&path) == 1,
    )
    .expect("the first reload waits on the disk");
    app.press("id:conflict-use-disk")
        .expect("ask to reload from disk again");
    app.wait_for("id:guard-reload")
        .expect("the second reload guard");
    app.press("id:guard-reload")
        .expect("confirm the second reload");
    app.wait_until(
        "the second reload's read to reach its gate",
        Duration::from_secs(5),
        |_| mt_app::test_vault::gate_reads_started(&path) == 2,
    )
    .expect("the second reload waits on the disk");

    // The second request's answer arrives first and syncs the tab.
    std::fs::write(&second_release, "go").expect("release the second reload");
    app.wait_for_gone("id:conflict-banner", Duration::from_secs(20))
        .expect("the disk version lands and the conflict clears");

    // The first read now fails: its source is gone. The answer is stale,
    // and its handling is the only proof the failure was processed.
    std::fs::remove_file(&path).expect("remove the source of the stale read");
    std::fs::write(&first_release, "go").expect("release the first reload");
    app.wait_until(
        "the stale reload failure to be handled",
        Duration::from_secs(10),
        |_| mt_app::test_vault::reload_superseded(&path),
    )
    .expect("the stale failure was handled");
    assert!(
        !app.screen()
            .texts()
            .iter()
            .any(|text| text.contains("Reload failed")),
        "the stale failure posted over the newer sync: {:?}",
        app.screen().texts()
    );
}
