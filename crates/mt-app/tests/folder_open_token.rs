//! A folder open carries a request token: a slow first scan must not commit
//! its root, files, context, recents or status over a faster second one that
//! finished first (contract amendment 7).

use std::time::Duration;

use nsh_engine::{EngineConfig, Points, Root};
use nsh_testing::Headless;

#[test]
fn a_slow_first_open_never_commits_over_a_fast_second() {
    let slow = tempfile::tempdir().expect("slow workspace");
    let fast = tempfile::tempdir().expect("fast workspace");
    let config = tempfile::tempdir().expect("config dir");
    std::fs::write(fast.path().join("fast.md"), "# Fast\n").expect("fast file");
    // Enough nested directories that the first scan cannot beat the second.
    for index in 0..2_000 {
        std::fs::create_dir_all(slow.path().join(format!("slow-{index}/nested")))
            .expect("slow tree");
    }
    let name = |dir: &tempfile::TempDir| {
        dir.path()
            .file_name()
            .expect("a folder name")
            .to_string_lossy()
            .into_owned()
    };
    let slow_name = name(&slow);
    let fast_name = name(&fast);

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

    let mut app =
        Headless::start(EngineConfig::new(Root::of::<mt_app::App>())).expect("start the app");
    app.set_viewport(nsh_core::Size::new(Points(1280.0), Points(800.0)))
        .expect("wide viewport");
    app.wait_for("id:welcome-open-folder")
        .expect("the welcome's next action");

    // The slow scan starts first; the fast one, started right after, finishes
    // first and owns the root.
    app.on_pick_paths(vec![slow.path().to_path_buf()])
        .expect("answer the first picker");
    app.press("id:welcome-open-folder")
        .expect("open the slow folder");
    app.on_pick_paths(vec![fast.path().to_path_buf()])
        .expect("answer the second picker");
    app.press("id:welcome-open-folder")
        .expect("open the fast folder");
    app.wait_for("id:workspace-home")
        .expect("the fast root commits");
    assert!(app.shows(&fast_name), "the fast folder is the root");

    // Give the superseded scan more time than its walk can need to finish:
    // its commit must not touch root, entries, context, recents or status.
    let superseded = app
        .wait_until(
            "the superseded slow open to land",
            Duration::from_secs(6),
            |screen| screen.texts().iter().any(|text| text == &slow_name),
        )
        .is_ok();
    assert!(
        !superseded,
        "the slow first open committed over the fast one: {:?}",
        app.screen().texts()
    );
    assert!(app.shows(&fast_name), "the fast root stays");
    assert!(
        !app.shows(&slow_name),
        "the superseded root never shows: {:?}",
        app.screen().texts()
    );
}
