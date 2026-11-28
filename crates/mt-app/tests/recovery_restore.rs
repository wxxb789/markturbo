//! A checkpoint left by an interrupted session comes back through the
//! recovery banner's Restore as a dirty tab holding exactly the
//! checkpointed text, and closing that tab with Don't save retires the
//! record, so the next start offers nothing.

use std::collections::HashSet;
use std::sync::Mutex;
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
/// The two recovery drafts differ, so a restored document proves which row
/// its Restore opened.
const FIRST_DRAFT: &str = "Recovered first draft alpha.";
const SECOND_DRAFT: &str = "Recovered second draft beta.";

static TEST_LOCK: Mutex<()> = Mutex::new(());

#[test]
fn restore_reopens_the_checkpoint_and_dont_save_retires_it() {
    if !cfg!(windows) {
        // DPAPI is the production protector; non-Windows builds intentionally
        // have no plaintext recovery fallback.
        return;
    }
    let _guard = TEST_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    let dir = tempfile::tempdir().expect("temp dir");
    let config = tempfile::tempdir().expect("config dir");
    let path = dir.path().join("notes.md");
    std::fs::write(&path, ON_DISK).expect("write fixture");

    // SAFETY: TEST_LOCK serializes app lifetimes and environment changes.
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
    let loaded = io::load(&path).expect("load the file");
    let key = RecoveryKey::for_path(&loaded.path);
    {
        let (store, _maintenance) = RecoveryStore::open().expect("open recovery store");
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

fn seed_two(dir: &std::path::Path) -> tempfile::TempDir {
    let config = tempfile::tempdir().expect("config dir");
    // SAFETY: TEST_LOCK serializes app lifetimes and environment changes.
    unsafe {
        std::env::remove_var("MARKTURBO_OPEN");
        std::env::set_var("MARKTURBO_CONFIG_DIR", config.path());
        std::env::set_var("MARKTURBO_DATA_DIR", config.path().join("data"));
        std::env::set_var(
            "MARKTURBO_TEST_LOCK_NAMESPACE",
            std::process::id().to_string(),
        );
    }
    for (name, draft) in [("first.md", FIRST_DRAFT), ("second.md", SECOND_DRAFT)] {
        let path = dir.join(name);
        std::fs::write(&path, ON_DISK).expect("write seed");
        let loaded = io::load(&path).expect("load seed");
        let key = RecoveryKey::for_path(&loaded.path);
        let (store, _maintenance) = RecoveryStore::open().expect("open recovery store");
        store
            .checkpoint(
                &RecoveryCheckpoint {
                    key,
                    text: draft.to_owned(),
                    metadata: RecoveryMetadata::from_loaded_file(&loaded),
                    revision: None,
                },
                &HashSet::new(),
            )
            .expect("seed checkpoint");
    }
    config
}

fn bounds(app: &Headless, id: &str) -> nsh_core::geometry::Bounds<Points> {
    app.screen()
        .layout_nodes()
        .into_iter()
        .find(|node| node.test_id.as_deref() == Some(id))
        .unwrap_or_else(|| panic!("`{id}` on screen"))
        .bounds
}

/// The row text truncates before the actions; the hit test presses Restore
/// and Discard at 1000 (Source and Split) and 1100 px, and the Restore on
/// the remaining row opens that row's own draft.
#[test]
fn recovery_actions_stay_pressable_at_narrow_widths() {
    if !cfg!(windows) {
        return;
    }
    let _guard = TEST_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    for (width, split) in [(1000.0, false), (1000.0, true), (1100.0, true)] {
        let work = tempfile::tempdir().expect("workspace");
        let _config = seed_two(work.path());
        let mut app =
            Headless::start(EngineConfig::new(Root::of::<mt_app::App>())).expect("start the app");
        app.set_viewport(nsh_core::Size::new(Points(width), Points(800.0)))
            .expect("viewport");
        app.wait_for("id:recovery-banner")
            .expect("the recovery banner");
        // A document first: the Source state with a document is what the
        // walk found the actions clipped in.
        app.press("id:title-settings").expect("open the title menu");
        app.press("id:title-menu-new-file").expect("new document");
        app.wait_for(EDITOR).expect("the editor");
        if split {
            app.press("id:title-view").expect("choose Split");
            app.wait_for("id:preview-field").expect("the preview");
        }
        app.settle();

        let overlay_left = if width < 1040.0 {
            app.screen()
                .layout_nodes()
                .into_iter()
                .find(|node| node.test_id.as_deref() == Some("field"))
                .map(|node| node.bounds.origin.x.0 - mt_app::ui::tokens::FIELD_GAP)
        } else {
            None
        };
        let visible_right = overlay_left.unwrap_or(width);
        for id in ["recovery-restore-0", "recovery-discard-0"] {
            let action = bounds(&app, id);
            assert!(
                action.origin.x.0 + action.size.width.0 <= visible_right + 0.5,
                "{id} clips past {visible_right}px at {width}px split={split}: {action:?}"
            );
        }

        let row0 = ["first.md", "second.md"]
            .into_iter()
            .find(|name| {
                app.screen()
                    .matches(&format!("id:recovery-row-0 >> text:/{name}/"))
                    .expect("selector")
            })
            .expect("row 0 names its file");
        let (discarded, expected) = if row0 == "first.md" {
            (FIRST_DRAFT, SECOND_DRAFT)
        } else {
            (SECOND_DRAFT, FIRST_DRAFT)
        };

        app.press("id:recovery-discard-0")
            .expect("Discard is hit at its visible position");
        app.wait_for("id:status-action")
            .expect("the discarded row leaves");
        app.press("label:Restore")
            .expect("Restore is hit at its visible position");
        app.wait_for_gone("id:recovery-banner", Duration::from_secs(5))
            .expect("the restored row leaves the banner");
        app.wait_for(&format!("id:source-editor >> text:{expected}"))
            .expect("the restored document shows the remaining row's draft");
        assert!(
            !app.shows(discarded),
            "the discarded row's draft was restored: {:?}",
            app.screen().texts()
        );
    }
}
