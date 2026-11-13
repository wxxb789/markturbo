//! Untitled drafts must enter the recovery store while they are dirty.

use nsh_engine::{EngineConfig, Points, Root};
use nsh_testing::Headless;

#[test]
fn untitled_draft_is_checkpointed() {
    if !cfg!(windows) {
        // DPAPI is the production protector; non-Windows builds intentionally
        // have no plaintext recovery fallback.
        return;
    }
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
    let recovery = config.path().join("data").join("recovery");
    let baseline = recovery_entries(&recovery);
    app.wait_for("text:Start a Markdown document")
        .expect("welcome");
    app.press("text:New").expect("new draft");
    app.wait_for("label:Markdown source").expect("draft editor");
    app.type_text("label:Markdown source", "Recover me")
        .expect("edit draft");

    // Pump the app's frames: sleeping in this thread never advances it.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    loop {
        if recovery_entries(&recovery)
            .difference(&baseline)
            .next()
            .is_some()
        {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "draft never reached recovery store"
        );
        app.next_change(std::time::Duration::from_millis(25))
            .expect("settle while the checkpoint lands");
    }
    drop(app);

    let (store, _maintenance) =
        mt_core::recovery::RecoveryStore::open().expect("open recovery store");
    let scan = store.recover().expect("scan recovery");
    let record = scan
        .records
        .iter()
        .find(|record| record.record.text.contains("Recover me"))
        .expect("draft recovery record");
    assert_eq!(record.record.metadata.source_path, None);
    for record in scan.records {
        store
            .invalidate_and_delete(&record.record.key)
            .expect("cleanup recovery");
    }
}

fn recovery_entries(path: &std::path::Path) -> std::collections::HashSet<std::path::PathBuf> {
    std::fs::read_dir(path)
        .map(|entries| {
            entries
                .filter_map(Result::ok)
                .map(|entry| entry.path())
                .collect()
        })
        .unwrap_or_default()
}
