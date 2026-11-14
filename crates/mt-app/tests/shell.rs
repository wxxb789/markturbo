//! Headless acceptance for the first nsh surface: the shell renders, and a
//! real Markdown file opens into the source editor and native preview.

use std::io::Write;

use nsh_engine::{EngineConfig, Root};
use nsh_testing::Headless;

#[test]
fn opens_a_markdown_file() {
    let dir = tempfile::tempdir().expect("temp dir");
    let config = tempfile::tempdir().expect("config dir");
    let path = dir.path().join("AGENTS.md");
    let mut file = std::fs::File::create(&path).expect("create fixture");
    writeln!(file, "# Release notes\n\nHello **preview**.").expect("write fixture");
    drop(file);

    // SAFETY: this binary has one test, so no other test reads the
    // environment concurrently.
    unsafe {
        std::env::set_var("MARKTURBO_OPEN", &path);
        std::env::set_var("MARKTURBO_CONFIG_DIR", config.path());
        std::env::set_var("MARKTURBO_DATA_DIR", config.path().join("data"));
    }

    let mut app =
        Headless::start(EngineConfig::new(Root::of::<mt_app::App>())).expect("start the app");

    app.wait_for("text:Release notes").expect("preview heading");
    app.wait_for("label:Markdown source")
        .expect("source editor");

    assert!(app.shows("Hello preview.") || app.shows("Hello"));

    // The editor reports the whole new text; saving through the core keeps the
    // file's own bytes apart from the edit.
    let recovery = config.path().join("data").join("recovery");
    let baseline = recovery_entries(&recovery);
    app.type_text("label:Markdown source", "\n\nAdded by the test.")
        .expect("type into the editor");

    // A dirty buffer is checkpointed after a short idle window. DPAPI only
    // exists on Windows; other targets report recovery unavailable.
    if cfg!(windows) {
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
                "expected a recovery checkpoint for the dirty buffer"
            );
            app.next_change(std::time::Duration::from_millis(25))
                .expect("settle while the checkpoint lands");
        }
    }

    app.press("text:Agent Context").expect("open context");
    app.wait_for("text:Effective Agent Context")
        .expect("context panel heading");
    app.wait_for("text:Workspace / ProjectAgents")
        .expect("project instruction source");

    // The checkpoint must not modify the source file. Reopen the store as a
    // fresh process would and verify the recovered text is the edited buffer.
    app.settle();
    drop(app);
    assert!(
        !std::fs::read_to_string(&path)
            .expect("read source")
            .contains("Added by the test.")
    );
    if cfg!(windows) {
        let (store, _maintenance) =
            mt_core::recovery::RecoveryStore::open().expect("open recovery store");
        let scan = store.recover().expect("scan recovery records");
        assert!(
            scan.records
                .iter()
                .any(|record| { record.record.text.contains("Added by the test.") })
        );
        for record in scan.records {
            store
                .invalidate_and_delete(&record.record.key)
                .expect("delete test recovery record");
        }
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
