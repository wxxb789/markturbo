//! Untitled drafts must enter the recovery store while they are dirty.

use nsh_engine::{EngineConfig, Points, Root};
use nsh_testing::Headless;

#[test]
fn untitled_draft_is_checkpointed() {
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

    let mut checkpointed = false;
    for _ in 0..20 {
        std::thread::sleep(std::time::Duration::from_millis(250));
        if recovery_entries(&recovery)
            .difference(&baseline)
            .next()
            .is_some()
        {
            checkpointed = true;
            break;
        }
    }
    assert!(checkpointed, "draft never reached recovery store");
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
