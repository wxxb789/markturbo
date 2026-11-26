//! A folder open carries a request token: a slow first scan held open until
//! after a fast second one committed must write none of the root, files,
//! Context, Recents or status (contract amendment 7).
//!
//! The first scan's completion is controlled by an injectable gate, so the
//! reverse order is forced rather than hoped for.

use std::time::Duration;

use nsh_engine::{EngineConfig, Points, Root};
use nsh_testing::Headless;

mod support;

#[test]
fn a_slow_first_open_never_commits_over_a_fast_second() {
    let slow = tempfile::tempdir().expect("slow workspace");
    let fast = tempfile::tempdir().expect("fast workspace");
    let config = tempfile::tempdir().expect("config dir");
    let codex_home = tempfile::tempdir().expect("codex home");
    let release = config.path().join("release-slow-scan");
    std::fs::write(slow.path().join("slow.md"), "# Slow\n").expect("slow file");
    std::fs::write(fast.path().join("fast.md"), "# Fast\n").expect("fast file");
    std::fs::write(fast.path().join("AGENTS.md"), "# Rules\n").expect("fast instructions");
    let name = |dir: &tempfile::TempDir| {
        dir.path()
            .file_name()
            .expect("a folder name")
            .to_string_lossy()
            .into_owned()
    };
    let slow_name = name(&slow);
    let fast_name = name(&fast);

    // SAFETY: nextest runs each test in its own process.
    unsafe {
        std::env::remove_var("MARKTURBO_OPEN");
        std::env::set_var("MARKTURBO_CONFIG_DIR", config.path());
        std::env::set_var("MARKTURBO_DATA_DIR", config.path().join("data"));
        std::env::set_var(
            "MARKTURBO_TEST_LOCK_NAMESPACE",
            std::process::id().to_string(),
        );
        std::env::set_var("CODEX_HOME", codex_home.path());
    }

    let mut app =
        Headless::start(EngineConfig::new(Root::of::<mt_app::App>())).expect("start the app");
    app.set_viewport(nsh_core::Size::new(Points(1280.0), Points(800.0)))
        .expect("wide viewport");
    app.wait_for("id:welcome-open-folder")
        .expect("the welcome's next action");

    // The slow scan starts first and parks on the gate.
    mt_app::test_vault::gate_read(slow.path(), &release);
    app.on_pick_paths(vec![slow.path().to_path_buf()])
        .expect("answer the first picker");
    app.press("id:welcome-open-folder")
        .expect("open the slow folder");
    app.wait_for("text:Opening folder…")
        .expect("the slow open announces itself");

    // The fast scan starts second and commits first.
    app.on_pick_paths(vec![fast.path().to_path_buf()])
        .expect("answer the second picker");
    app.press("id:welcome-open-folder")
        .expect("open the fast folder");
    app.wait_for("id:workspace-home")
        .expect("the fast root commits");
    app.wait_for("id:workspace-home-file >> text:fast.md")
        .expect("the fast root's files are listed");
    assert!(
        app.screen()
            .texts()
            .iter()
            .any(|text| text.contains(&fast_name)),
        "the fast folder is the root: {:?}",
        app.screen().texts()
    );

    // An open Context resolved for the fast root anchors the check.
    app.press("id:title-settings")
        .expect("open the glyph's menu");
    app.press("id:title-menu-context")
        .expect("open the context");
    app.wait_for("id:context-source-0 >> text:AGENTS.md")
        .expect("the fast root's instructions resolve");

    // The slow scan's source is gone when it finally runs: its answer is
    // an error. Release it and wait until it actually returns: a
    // superseded open changes nothing on screen, so its token check is the
    // only proof the stale scan ran at all (contract amendment 7).
    std::fs::remove_dir_all(slow.path()).expect("remove the superseded workspace");
    std::fs::write(&release, "go").expect("release the slow scan");
    support::wait_for_marker(
        &mut app,
        "the superseded slow open to return",
        Duration::from_secs(10),
        |_| mt_app::test_vault::open_superseded(slow.path()),
    );

    // Now that it returned, none of the stale result's surfaces moved: root
    // and files, Context, Recents and status still name the fast folder, and
    // the stale answer wrote no error either.
    assert!(app.shows(&fast_name), "the fast root stays");
    assert!(
        !app.shows(&slow_name),
        "the stale root never shows: {:?}",
        app.screen().texts()
    );
    assert_eq!(
        app.screen()
            .count("id:workspace-home-file")
            .expect("selector"),
        2,
        "the fast root's two files are listed"
    );
    assert!(
        !app.shows("slow.md"),
        "the stale root's file never joins the listing: {:?}",
        app.screen().texts()
    );
    assert!(
        app.screen()
            .matches("id:context-source-0 >> text:AGENTS.md")
            .expect("selector"),
        "the Context still resolves for the fast root"
    );
    assert!(
        !app.shows(&format!("Opened {slow_name}")),
        "the stale open wrote no status: {:?}",
        app.screen().texts()
    );
    assert!(
        !app.screen()
            .texts()
            .iter()
            .any(|text| text.contains("Open folder failed")),
        "the stale open wrote its failure over the fast one: {:?}",
        app.screen().texts()
    );
    let settings = std::fs::read_to_string(config.path().join("settings.toml")).unwrap_or_default();
    assert!(
        !settings.contains(&slow_name),
        "the stale open entered Recents: {settings}"
    );
}

#[test]
fn a_superseded_successful_open_commits_nothing() {
    let slow = tempfile::tempdir().expect("slow workspace");
    let fast = tempfile::tempdir().expect("fast workspace");
    let config = tempfile::tempdir().expect("config dir");
    let codex_home = tempfile::tempdir().expect("codex home");
    let release = config.path().join("release-slow-scan");
    std::fs::write(slow.path().join("slow.md"), "# Slow\n").expect("slow file");
    std::fs::write(fast.path().join("fast.md"), "# Fast\n").expect("fast file");
    std::fs::write(fast.path().join("AGENTS.md"), "# Rules\n").expect("fast instructions");
    let name = |dir: &tempfile::TempDir| {
        dir.path()
            .file_name()
            .expect("a folder name")
            .to_string_lossy()
            .into_owned()
    };
    let slow_name = name(&slow);
    let fast_name = name(&fast);

    // SAFETY: nextest runs each test in its own process.
    unsafe {
        std::env::remove_var("MARKTURBO_OPEN");
        std::env::set_var("MARKTURBO_CONFIG_DIR", config.path());
        std::env::set_var("MARKTURBO_DATA_DIR", config.path().join("data"));
        std::env::set_var(
            "MARKTURBO_TEST_LOCK_NAMESPACE",
            std::process::id().to_string(),
        );
        std::env::set_var("CODEX_HOME", codex_home.path());
    }

    let mut app =
        Headless::start(EngineConfig::new(Root::of::<mt_app::App>())).expect("start the app");
    app.set_viewport(nsh_core::Size::new(Points(1280.0), Points(800.0)))
        .expect("wide viewport");
    app.wait_for("id:welcome-open-folder")
        .expect("the welcome's next action");

    // The slow scan starts first and parks on the gate.
    mt_app::test_vault::gate_read(slow.path(), &release);
    app.on_pick_paths(vec![slow.path().to_path_buf()])
        .expect("answer the first picker");
    app.press("id:welcome-open-folder")
        .expect("open the slow folder");
    app.wait_for("text:Opening folder…")
        .expect("the slow open announces itself");

    // The fast scan starts second and commits first.
    app.on_pick_paths(vec![fast.path().to_path_buf()])
        .expect("answer the second picker");
    app.press("id:welcome-open-folder")
        .expect("open the fast folder");
    app.wait_for("id:workspace-home")
        .expect("the fast root commits");
    app.wait_for("id:workspace-home-file >> text:fast.md")
        .expect("the fast root's files are listed");
    assert!(
        app.screen()
            .texts()
            .iter()
            .any(|text| text.contains(&fast_name)),
        "the fast folder is the root: {:?}",
        app.screen().texts()
    );

    // An open Context resolved for the fast root anchors the check.
    app.press("id:title-settings")
        .expect("open the glyph's menu");
    app.press("id:title-menu-context")
        .expect("open the context");
    app.wait_for("id:context-source-0 >> text:AGENTS.md")
        .expect("the fast root's instructions resolve");

    // The slow folder still exists, so its scan succeeds. Release it and
    // wait until it actually returned: a superseded open changes nothing on
    // screen, so its token check is the only proof the stale scan ran at
    // all (contract amendment 7).
    std::fs::write(&release, "go").expect("release the slow scan");
    support::wait_for_marker(
        &mut app,
        "the superseded slow open to return",
        Duration::from_secs(10),
        |_| mt_app::test_vault::open_superseded(slow.path()),
    );

    // None of the stale result's surfaces moved: root and files, Context,
    // Recents and status still name the fast folder.
    assert!(app.shows(&fast_name), "the fast root stays");
    assert!(
        !app.shows(&slow_name),
        "the stale root never shows: {:?}",
        app.screen().texts()
    );
    assert_eq!(
        app.screen()
            .count("id:workspace-home-file")
            .expect("selector"),
        2,
        "the fast root's two files are listed"
    );
    assert!(
        !app.shows("slow.md"),
        "the stale root's file never joins the listing: {:?}",
        app.screen().texts()
    );
    assert!(
        app.screen()
            .matches("id:context-source-0 >> text:AGENTS.md")
            .expect("selector"),
        "the Context still resolves for the fast root"
    );
    assert!(
        !app.shows(&format!("Opened {slow_name}")),
        "the stale open wrote no status: {:?}",
        app.screen().texts()
    );
    let settings = std::fs::read_to_string(config.path().join("settings.toml")).unwrap_or_default();
    assert!(
        !settings.contains(&slow_name),
        "the stale open entered Recents: {settings}"
    );
}
