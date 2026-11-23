//! A checkpoint left by an interrupted session comes back through the
//! recovery banner's Restore as a dirty tab holding exactly the
//! checkpointed text, and closing that tab with Don't save retires the
//! record, so the next start offers nothing.

use std::collections::HashSet;
use std::time::Duration;

use mt_core::document::io;
use mt_core::recovery::{RecoveryCheckpoint, RecoveryKey, RecoveryMetadata, RecoveryStore};
use nsh_core::platform::{Idiom, Os};
use nsh_engine::{EngineConfig, Points, Root};
use nsh_testing::Headless;

const EDITOR: &str = "label:Markdown source";
const ON_DISK: &str = "# Notes\n";
/// The unsaved text: CJK and a surrogate-pair emoji, as the native recovery
/// case types.
const CHECKPOINT: &str = "# Notes\n恢复的草稿 🦀 survives.\n";

#[test]
fn restore_reopens_the_checkpoint_and_dont_save_retires_it() {
    if !cfg!(windows) {
        // DPAPI is the production protector; non-Windows builds intentionally
        // have no plaintext recovery fallback.
        return;
    }
    let dir = tempfile::tempdir().expect("temp dir");
    let config = tempfile::tempdir().expect("config dir");
    let path = dir.path().join("notes.md");
    std::fs::write(&path, ON_DISK).expect("write fixture");

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

    // The interrupted session's checkpoint, written through the production
    // store the app opens at startup.
    let key = RecoveryKey::for_path(&path);
    {
        let (store, _maintenance) = RecoveryStore::open().expect("open recovery store");
        let loaded = io::load(&path).expect("load the file");
        store
            .checkpoint(
                &RecoveryCheckpoint {
                    key: key.clone(),
                    text: CHECKPOINT.to_owned(),
                    metadata: RecoveryMetadata::from_loaded_file(&loaded),
                    revision: None,
                },
                &HashSet::new(),
            )
            .expect("seed the checkpoint");
    }

    let mut engine = EngineConfig::new(Root::of::<mt_app::App>());
    engine.environment.os = Os::Windows;
    engine.environment.idiom = Idiom::Desktop;
    let mut app = Headless::start(engine).expect("start the app");
    app.set_viewport(nsh_core::Size::new(Points(1280.0), Points(800.0)))
        .expect("wide viewport");
    app.wait_for("id:recovery-banner")
        .expect("the recovery banner");
    app.press("id:recovery-restore-0").expect("restore");
    app.wait_for(EDITOR).expect("the restored document");
    app.wait_for_gone("id:recovery-banner", Duration::from_secs(5))
        .expect("the banner goes once its only record is restored");
    // The source editor draws the checkpoint's lines (the headless input
    // reports typed text only, so the lines are read as drawn).
    for line in CHECKPOINT.lines() {
        assert!(
            app.screen()
                .matches(&format!("id:source-editor >> text:{line}"))
                .expect("selector"),
            "the editor shows {line:?}: {:?}",
            app.screen().texts()
        );
    }
    app.wait_for("id:title-dirty")
        .expect("the restored tab is dirty");

    // Closing it without saving retires the record and writes nothing.
    app.type_text(EDITOR, "").expect("focus the editor");
    app.press_key("mod+w").expect("press Ctrl+W");
    app.wait_for("id:guard-dont-save").expect("the guard");
    app.press("id:guard-card >> label:Don't save")
        .expect("discard");
    app.wait_for_gone(EDITOR, Duration::from_secs(5))
        .expect("the tab closes");
    drop(app);

    assert_eq!(
        std::fs::read_to_string(&path).expect("read file"),
        ON_DISK,
        "Don't save writes nothing"
    );
    let (store, _maintenance) = RecoveryStore::open().expect("reopen recovery store");
    let scan = store.recover().expect("scan recovery");
    assert!(
        scan.records.iter().all(|record| record.record.key != key),
        "Don't save retired the record"
    );
}
