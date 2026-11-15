//! Headless acceptance for the shell: a real Markdown file opens into the
//! source editor and the native preview, a dirty buffer is checkpointed
//! without touching the file, and the context field shows the project
//! instructions that apply.

use std::io::Write;
use std::time::Duration;

use nsh_engine::{EngineConfig, Points, Root};
use nsh_testing::Headless;

#[test]
fn opens_a_markdown_file() {
    let dir = tempfile::tempdir().expect("temp dir");
    let config = tempfile::tempdir().expect("config dir");
    let codex_home = tempfile::tempdir().expect("codex home");
    let path = dir.path().join("AGENTS.md");
    let mut file = std::fs::File::create(&path).expect("create fixture");
    writeln!(file, "# Release notes\n\nHello **preview**.").expect("write fixture");
    drop(file);

    // SAFETY: this test binary has one test, so no other test reads the
    // environment concurrently.
    unsafe {
        std::env::set_var("MARKTURBO_OPEN", &path);
        std::env::set_var("MARKTURBO_CONFIG_DIR", config.path());
        std::env::set_var("MARKTURBO_DATA_DIR", config.path().join("data"));
        // The context chain reads the user's codex home; a fresh one keeps
        // the resolution this fixture's own.
        std::env::set_var("CODEX_HOME", codex_home.path());
    }

    let mut app =
        Headless::start(EngineConfig::new(Root::of::<mt_app::App>())).expect("start the app");
    app.set_viewport(nsh_core::Size::new(Points(1440.0), Points(900.0)))
        .expect("wide viewport");

    // The document opens into the source editor, with the native preview
    // beside it in the field (the default Split pane), and Review is the
    // document's own next action.
    app.wait_for("label:Markdown source")
        .expect("source editor");
    app.wait_for("text:Release notes").expect("preview heading");
    app.wait_for("id:title-review")
        .expect("the document's Review action");

    assert!(app.shows("Hello preview.") || app.shows("Hello"));

    // The editor reports the whole new text; saving through the core keeps
    // the file's own bytes apart from the edit.
    let recovery = config.path().join("data").join("recovery");
    let baseline = recovery_entries(&recovery);
    app.type_text("label:Markdown source", "\n\nAdded by the test.")
        .expect("type into the editor");

    // A dirty buffer is checkpointed after a short idle window. DPAPI only
    // exists on Windows; other targets report recovery unavailable.
    if cfg!(windows) {
        // Pump the app's frames: sleeping in this thread never advances it.
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
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
            app.next_change(Duration::from_millis(25))
                .expect("settle while the checkpoint lands");
        }
    }

    // The context field: the resolved chain for this file. The fixture's
    // project holds exactly one instruction file, AGENTS.md at its root; the
    // row names it inside the row itself (the title row also reads
    // AGENTS.md, so an unscoped text check would prove nothing), and a row
    // that applies carries its Include in review decision and is not in the
    // profile's not-applied inventory.
    app.press("id:title-context").expect("open the context");
    app.wait_for("id:context-field").expect("context field");
    app.wait_for("id:context-source-0")
        .expect("a resolved context source");
    app.wait_for("id:context-source-0 >> text:AGENTS.md")
        .expect("the project's own AGENTS.md is the resolved source");
    assert_eq!(
        app.screen().count("id:context-source-1").expect("selector"),
        0,
        "the fixture's project resolves exactly one source"
    );
    let shown = app.screen().texts().join("\n");
    assert!(
        shown.contains("Applies from the repository root"),
        "{shown}"
    );
    assert!(shown.contains("Include in review"), "{shown}");
    assert!(
        !shown.contains("Not applied by Codex"),
        "a project-root AGENTS.md applies; it is not inventory: {shown}"
    );

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
