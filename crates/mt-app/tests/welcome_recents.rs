//! A recent target that no longer exists degrades visibly and safely: its row
//! says it is missing, is a disabled button, and a press opens nothing; a
//! target that still exists reopens from its row. The history holds at most
//! ten targets, on disk and across a restart.

mod support;

use std::sync::Mutex;
use std::time::Duration;

use nsh_engine::{EngineConfig, Points, Root};
use nsh_testing::Headless;

/// The tests in this binary set the process environment the app reads at
/// startup.
static TEST_LOCK: Mutex<()> = Mutex::new(());

fn start(config: &std::path::Path) -> Headless {
    // SAFETY: the tests in this binary serialize app lifetimes and
    // environment changes.
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
    app
}

fn recent(path: &std::path::Path) -> String {
    format!(
        "[[recent-targets]]\npath = {:?}\nkind = \"file\"\ndisplay-name = {:?}\n\n",
        path.display().to_string(),
        path.file_name().unwrap().to_str().unwrap()
    )
}

#[test]
fn a_missing_recent_is_marked_disabled_and_opens_nothing() {
    let _guard = TEST_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let config = tempfile::tempdir().expect("config dir");
    let workspace = tempfile::tempdir().expect("workspace");
    let present = workspace.path().join("present.md");
    std::fs::write(&present, "# Present\n").expect("write the present target");
    let missing = workspace.path().join("gone.md");
    std::fs::write(
        config.path().join("settings.toml"),
        format!("{}{}", recent(&missing), recent(&present)),
    )
    .expect("write the settings");

    let mut app = start(config.path());
    app.wait_for("id:welcome-recents")
        .expect("the recent targets");
    let texts = app.screen().texts().join(" | ");

    // Both rows are buttons sharing the one id, each named by its target.
    assert_eq!(
        app.screen().count("id:welcome-recent").expect("selector"),
        2,
        "two recent rows: {texts}"
    );
    assert_eq!(
        app.screen()
            .count("role:button[File present.md]")
            .expect("selector"),
        1,
        "the present target's row: {texts}"
    );
    assert_eq!(
        app.screen()
            .count("role:button[File gone.md — Missing]")
            .expect("selector"),
        1,
        "the missing target's row names it missing: {texts}"
    );
    assert!(
        app.screen()
            .count("id:welcome-recent-row >> text:Missing \u{00B7} Remove")
            .expect("selector")
            == 1,
        "the missing row offers Missing \u{00B7} Remove on screen: {texts}"
    );
    assert!(
        !app.screen()
            .enabled("role:button[File gone.md — Missing]")
            .expect("selector"),
        "the missing row cannot be pressed"
    );
    assert!(
        app.screen()
            .enabled("role:button[File present.md]")
            .expect("selector"),
        "the present row can"
    );

    // A press on the missing row opens nothing and changes no file.
    let _ = app.press("role:button[File gone.md — Missing]");
    app.idle(Duration::from_millis(200)).expect("settle");
    assert!(!missing.exists(), "no file was created for the missing row");
    assert_eq!(
        app.screen().count("id:rail-file-row").expect("selector"),
        0,
        "no document opened"
    );
    app.wait_for("id:welcome-open-folder")
        .expect("the welcome stays");

    // The missing row's badge is one press to forget the entry.
    app.press("id:welcome-recent-remove-0")
        .expect("remove the missing target");
    app.wait_until("the missing row to go", Duration::from_secs(5), |screen| {
        screen.count("id:welcome-recent").unwrap_or(0) == 1
    })
    .expect("the missing row is removed");
    support::wait_for_file(
        &mut app,
        &config.path().join("settings.toml"),
        |text| !text.contains("gone.md"),
        "the removal never reached settings.toml",
        Duration::from_secs(10),
    );

    // The present row reopens its document.
    app.press("role:button[File present.md]")
        .expect("reopen the present target");
    app.wait_for("label:Markdown source")
        .expect("the reopened document's editor");
    app.wait_for_gone("id:welcome", Duration::from_secs(5))
        .expect("the welcome leaves");
}

/// Eleven recorded targets: the history keeps ten, a reopen writes exactly
/// ten back to `settings.toml` (the oldest dropped, the reopened target
/// first), and a restarted app reads those ten back. The welcome lists the
/// five most recent of them.
#[test]
fn a_recent_that_vanishes_after_render_flips_to_missing() {
    let _guard = TEST_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let config = tempfile::tempdir().expect("config dir");
    let workspace = tempfile::tempdir().expect("workspace");
    let vanish = workspace.path().join("vanish.md");
    std::fs::write(&vanish, "# Vanish\n").expect("write the target");
    std::fs::write(config.path().join("settings.toml"), recent(&vanish))
        .expect("write the settings");

    let mut app = start(config.path());
    app.wait_for("id:welcome-recents")
        .expect("the recent target");
    // The row is drawn as openable; the file goes missing behind it.
    std::fs::remove_file(&vanish).expect("remove the target after the render");
    app.press("role:button[File vanish.md]")
        .expect("press the stale row");
    app.wait_until(
        "the stale row to flip to missing",
        Duration::from_secs(5),
        |screen| {
            screen
                .count("role:button[File vanish.md — Missing]")
                .unwrap_or(0)
                == 1
        },
    )
    .expect("a press on a vanished recent says so");
    assert_eq!(
        app.screen()
            .count("id:welcome-recent-row >> text:Missing \u{00B7} Remove")
            .expect("selector"),
        1,
        "the flipped row offers Missing \u{00B7} Remove: {:?}",
        app.screen().texts()
    );
    assert!(
        !app.screen()
            .enabled("role:button[File vanish.md — Missing]")
            .expect("selector"),
        "the flipped row is no longer pressable"
    );
    assert_eq!(
        app.screen().count("id:rail-file-row").expect("selector"),
        0,
        "the vanished target opened no document"
    );

    // The badge still forgets the entry.
    app.press("id:welcome-recent-remove-0")
        .expect("remove the vanished target");
    app.wait_until("the row to go", Duration::from_secs(5), |screen| {
        screen.count("id:welcome-recent").unwrap_or(0) == 0
    })
    .expect("the vanished row is removed");
}

#[test]
fn the_recent_history_holds_ten_targets_across_a_restart() {
    let _guard = TEST_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let config = tempfile::tempdir().expect("config dir");
    let workspace = tempfile::tempdir().expect("workspace");
    let targets: Vec<_> = (0..11)
        .map(|index| {
            let path = workspace.path().join(format!("note-{index:02}.md"));
            std::fs::write(&path, format!("# Note {index}\n")).expect("write a target");
            path
        })
        .collect();
    let settings = config.path().join("settings.toml");
    std::fs::write(
        &settings,
        targets.iter().map(|path| recent(path)).collect::<String>(),
    )
    .expect("write the settings");
    let persisted = || {
        let text = std::fs::read_to_string(&settings).expect("read the settings");
        (
            text.matches("[[recent-targets]]").count(),
            reopened_first(&text),
            text.contains("note-10.md"),
        )
    };
    let listed = |app: &Headless| {
        assert_eq!(
            app.screen().count("id:welcome-recent").expect("selector"),
            5,
            "the five most recent targets are listed: {:?}",
            app.screen().texts()
        );
    };

    let mut app = start(config.path());
    app.wait_for("id:welcome-recents")
        .expect("the recent targets");
    listed(&app);
    app.press("role:button[File note-03.md]")
        .expect("reopen a listed target");
    app.wait_for("label:Markdown source")
        .expect("the reopened document's editor");
    support::wait_for_file(
        &mut app,
        &settings,
        reopened_first,
        "the reopen never moved its target first",
        Duration::from_secs(10),
    );
    assert_eq!(
        persisted(),
        (10, true, false),
        "exactly ten targets are written back, the reopened first, the oldest gone"
    );
    drop(app);

    let mut app = start(config.path());
    app.wait_for("id:welcome-recents")
        .expect("the recent targets after the restart");
    listed(&app);
    assert_eq!(
        app.screen()
            .count("role:button[File note-03.md]")
            .expect("selector"),
        1,
        "the reopened target is listed after the restart"
    );
    assert_eq!(
        persisted(),
        (10, true, false),
        "the restart keeps ten on disk"
    );
}

/// Whether the settings list the reopened `note-03.md` before `note-00.md`,
/// the target that was first before the reopen.
fn reopened_first(settings: &str) -> bool {
    matches!(
        (settings.find("note-03.md"), settings.find("note-00.md")),
        (Some(reopened), Some(first)) if reopened < first
    )
}

/// Two recents with the same file name carry their parent folders, so the
/// rows tell the files apart.
#[test]
fn shared_recent_filenames_carry_their_parent_folders() {
    let _guard = TEST_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let config = tempfile::tempdir().expect("config dir");
    let workspace = tempfile::tempdir().expect("workspace");
    let drafts = workspace.path().join("drafts");
    let archive = workspace.path().join("archive");
    std::fs::create_dir(&drafts).expect("drafts dir");
    std::fs::create_dir(&archive).expect("archive dir");
    let first = drafts.join("plan.md");
    let second = archive.join("plan.md");
    std::fs::write(&first, "# Drafts plan\n").expect("write first");
    std::fs::write(&second, "# Archive plan\n").expect("write second");
    std::fs::write(
        config.path().join("settings.toml"),
        format!("{}{}", recent(&first), recent(&second)),
    )
    .expect("write the settings");

    let mut app = start(config.path());
    app.wait_for("id:welcome-recents")
        .expect("the recent targets");
    assert!(
        app.shows("drafts/plan.md"),
        "the drafts plan is named by its folder: {:?}",
        app.screen().texts()
    );
    assert!(
        app.shows("archive/plan.md"),
        "the archive plan is named by its folder: {:?}",
        app.screen().texts()
    );
}
