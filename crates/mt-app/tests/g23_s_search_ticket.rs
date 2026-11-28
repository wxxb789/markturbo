//! A superseded search writes nothing: a slow earlier walk can neither
//! overwrite a newer query's results nor refill a cleared palette.

use std::{sync::Mutex, time::Duration};

use nsh_engine::{EngineConfig, Points, Root};
use nsh_testing::Headless;

mod support;

const TIMEOUT: Duration = Duration::from_secs(20);

static TEST_LOCK: Mutex<()> = Mutex::new(());

fn start_app(workspace: &std::path::Path, config: &std::path::Path) -> Headless {
    // SAFETY: TEST_LOCK serializes app lifetimes and environment changes.
    unsafe {
        std::env::remove_var("MARKTURBO_OPEN");
        std::env::set_var("MARKTURBO_CONFIG_DIR", config);
        std::env::set_var("MARKTURBO_DATA_DIR", config.join("data"));
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
    app.on_pick_paths(vec![workspace.to_path_buf()])
        .expect("answer the folder picker");
    app.press("id:welcome-open-folder")
        .expect("open the workspace");
    app.wait_for("id:workspace-home").expect("the root commits");
    app.wait_for("id:workspace-home-file >> text:first.md")
        .expect("the workspace files are listed");
    app
}

fn submit_search(app: &mut Headless, query: &str) {
    app.press_key("shift+mod+f").expect("open the search");
    app.wait_for("id:palette-input").expect("the search line");
    app.type_text("id:palette-input", query)
        .expect("type the query");
    app.submit("id:palette-input").expect("run the search");
}

fn workspace_with_two_documents() -> (tempfile::TempDir, tempfile::TempDir) {
    let workspace = tempfile::tempdir().expect("workspace");
    let config = tempfile::tempdir().expect("config dir");
    std::fs::write(workspace.path().join("first.md"), "# first\nalpha\n").expect("write first");
    std::fs::write(workspace.path().join("second.md"), "# second\nbeta\n").expect("write second");
    (workspace, config)
}

#[test]
fn a_stale_search_never_overwrites_a_newer_result() {
    let _guard = TEST_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    let (workspace, config) = workspace_with_two_documents();
    let mut app = start_app(workspace.path(), config.path());

    let old_release = config.path().join("release-old-search");
    let new_release = config.path().join("release-new-search");
    std::fs::write(&new_release, "go").expect("the newer search proceeds");
    mt_app::test_vault::gate_reads(workspace.path(), [old_release.clone(), new_release]);
    assert_eq!(
        mt_app::test_vault::gate_reads_started(workspace.path()),
        0,
        "the folder open was supposed to pass its gate before the searches"
    );
    let before = mt_app::state::search_superseded();

    submit_search(&mut app, "alpha");
    support::wait_for_marker(&mut app, "the old walk to reach its gate", TIMEOUT, |_| {
        mt_app::test_vault::gate_reads_started(workspace.path()) == 1
    });

    app.clear("id:palette-input").expect("clear the old query");
    app.type_text("id:palette-input", "beta")
        .expect("type the newer query");
    app.submit("id:palette-input")
        .expect("run the newer search");
    app.wait_for("text:/beta/")
        .expect("the newer query's result");

    std::fs::write(&old_release, "go").expect("release the old search");
    support::wait_for_marker(&mut app, "the stale walk to be dropped", TIMEOUT, |_| {
        mt_app::state::search_superseded() > before
    });

    assert!(
        app.screen()
            .matches("id:palette-rows >> text:/beta/")
            .expect("selector"),
        "the newer result survives the stale answer: {:?}",
        app.screen().texts()
    );
    assert!(
        !app.screen().matches("text:/alpha/").expect("selector"),
        "the stale search replaced the newer result: {:?}",
        app.screen().texts()
    );
}

#[test]
fn a_stale_search_never_refills_a_cleared_palette() {
    let _guard = TEST_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    let (workspace, config) = workspace_with_two_documents();
    let mut app = start_app(workspace.path(), config.path());

    let release = config.path().join("release-old-search");
    mt_app::test_vault::gate_read(workspace.path(), &release);
    let before = mt_app::state::search_superseded();

    submit_search(&mut app, "alpha");
    support::wait_for_marker(&mut app, "the old walk to reach its gate", TIMEOUT, |_| {
        mt_app::test_vault::gate_reads_started(workspace.path()) == 1
    });

    app.press_key("escape").expect("close the search");
    app.wait_for_gone("id:palette", TIMEOUT)
        .expect("the palette closes");

    std::fs::write(&release, "go").expect("release the old search");
    support::wait_for_marker(&mut app, "the stale walk to be dropped", TIMEOUT, |_| {
        mt_app::state::search_superseded() > before
    });

    app.press_key("shift+mod+f").expect("reopen the search");
    app.wait_for("id:palette-input").expect("the search line");
    assert_eq!(
        app.screen().count("id:palette-row").expect("selector"),
        0,
        "the cleared search shows no rows: {:?}",
        app.screen().texts()
    );
    assert!(
        !app.screen().matches("text:/alpha/").expect("selector"),
        "the stale search refilled the cleared palette: {:?}",
        app.screen().texts()
    );
}
