//! Tier 2: `Discard` on the recovery banner soft-deletes. The rows leave the
//! banner at once, the store only lets go after `UNDO_HOLD_MS`, the status
//! line's `Undo` puts them back until then, and a second action takes the one
//! pending slot by committing the first hold at once. With two or more rows
//! the banner also offers `Restore all` and `Discard all`.

mod support;

use std::collections::HashSet;
use std::time::Duration;

use mt_core::document::io;
use mt_core::recovery::{RecoveryCheckpoint, RecoveryKey, RecoveryMetadata, RecoveryStore};
use nsh_core::platform::{Idiom, Os};
use nsh_engine::{EngineConfig, Points, Root};
use nsh_testing::Headless;

const HOLD_MS: u64 = 5_000;

/// How many of the seeded checkpoint files the recovery directory holds
/// right now. The store itself cannot be opened while the app owns it, so the
/// test reads the files the store writes.
fn checkpoint_files(keys: &[RecoveryKey]) -> usize {
    let directory = mt_core::runtime_paths::recovery_dir().expect("the recovery directory");
    keys.iter()
        .filter(|key| {
            directory
                .join(format!("{}.mtrecovery", key.as_str()))
                .exists()
        })
        .count()
}

/// The app over two crash checkpoints whose files still hold the checkpointed
/// text; the recovery banner lists both.
struct Recovered {
    app: Headless,
    keys: Vec<RecoveryKey>,
    _dir: tempfile::TempDir,
    _config: tempfile::TempDir,
}

fn two_recovered() -> Recovered {
    let dir = tempfile::tempdir().expect("temp dir");
    let config = tempfile::tempdir().expect("config dir");
    let first = dir.path().join("first.md");
    let second = dir.path().join("second.md");
    std::fs::write(&first, "# First\n").expect("write first");
    std::fs::write(&second, "# Second\n").expect("write second");

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

    let mut keys = Vec::new();
    for path in [&first, &second] {
        let key = RecoveryKey::for_path(path);
        let loaded = io::load(path).expect("load the file");
        let checkpoint = RecoveryCheckpoint {
            key: key.clone(),
            text: loaded.text.clone(),
            metadata: RecoveryMetadata::from_loaded_file(&loaded),
            revision: None,
        };
        let (store, _maintenance) = RecoveryStore::open().expect("open recovery store");
        store
            .checkpoint(&checkpoint, &HashSet::new())
            .expect("seed the checkpoint");
        keys.push(key);
    }
    assert_eq!(checkpoint_files(&keys), 2, "two checkpoints are seeded");

    let mut engine = EngineConfig::new(Root::of::<mt_app::App>());
    engine.environment.os = Os::Windows;
    engine.environment.idiom = Idiom::Desktop;
    engine.clock = nsh_core::time::Clock::manual(std::time::SystemTime::now());
    let mut app = Headless::start(engine).expect("start the app");
    app.set_viewport(nsh_core::Size::new(Points(1280.0), Points(800.0)))
        .expect("wide viewport");
    app.wait_for("id:recovery-banner")
        .expect("the recovery banner");
    Recovered {
        app,
        keys,
        _dir: dir,
        _config: config,
    }
}

#[test]
fn tier_two_discard_holds_and_commit_boundaries() {
    if !cfg!(windows) {
        // DPAPI is the production protector; non-Windows builds intentionally
        // have no plaintext recovery fallback.
        return;
    }
    discard_all_stays_reversible_until_the_hold_expires();
    a_second_discard_commits_the_first_hold_and_takes_its_slot();
    a_newer_notice_does_not_hide_the_pending_undo();
}

fn discard_all_stays_reversible_until_the_hold_expires() {
    let mut seeded = two_recovered();
    let keys = seeded.keys.clone();
    let app = &mut seeded.app;
    app.wait_for("id:recovery-restore-all")
        .expect("Restore all with two rows");
    app.wait_for("id:recovery-discard-all")
        .expect("Discard all with two rows");

    // Discard all: the rows leave at once and the status offers Undo.
    app.press("id:recovery-discard-all").expect("discard all");
    app.wait_until("the rows to go", Duration::from_secs(5), |screen| {
        screen.count("id:recovery-banner").unwrap_or(1) == 0
    })
    .expect("the banner leaves");
    app.wait_for("id:status-action")
        .expect("the status line offers Undo");
    assert!(app.shows("Discarded recovered draft"));
    assert_eq!(
        checkpoint_files(&keys),
        2,
        "nothing is deleted before the hold expires"
    );

    // Undo restores the rows; the action goes with the hold.
    app.press("id:status-action").expect("undo the discard");
    app.wait_for("id:recovery-banner")
        .expect("the rows come back");
    app.wait_for_gone("id:status-action", Duration::from_secs(5))
        .expect("Undo is spent");
    assert_eq!(checkpoint_files(&keys), 2);

    // Discard again and let the clock pass the hold: only then is the store
    // cleaned, and the spent action disappears with the commit.
    app.press("id:recovery-discard-all")
        .expect("discard all again");
    app.wait_until("the rows to go", Duration::from_secs(5), |screen| {
        screen.count("id:recovery-banner").unwrap_or(1) == 0
    })
    .expect("the banner leaves");
    app.wait(Duration::from_millis(HOLD_MS - 1_000))
        .expect("advance before the hold");
    assert_eq!(
        checkpoint_files(&keys),
        2,
        "the store still holds the records inside the hold"
    );
    app.wait(Duration::from_millis(1_500))
        .expect("advance past the hold");
    app.wait_until("the store to fall empty", Duration::from_secs(5), |_| {
        checkpoint_files(&keys) == 0
    })
    .expect("the commit deletes after the hold");
    app.wait_for_gone("id:status-action", Duration::from_secs(5))
        .expect("the committed action is gone");
    assert_eq!(
        checkpoint_files(&keys),
        0,
        "the discarded checkpoints are deleted"
    );
}

fn a_second_discard_commits_the_first_hold_and_takes_its_slot() {
    let mut seeded = two_recovered();
    let keys = seeded.keys.clone();
    let app = &mut seeded.app;

    // First discard: one row leaves, its Undo holds the row and the entry.
    app.press("id:recovery-discard-0")
        .expect("discard the first row");
    app.wait_for("id:status-action").expect("the Undo");
    assert_eq!(checkpoint_files(&keys), 2, "nothing is deleted yet");

    // The row left names the file the first discard held; the other one is
    // committed the moment the second discard takes the slot.
    let remaining = [0usize, 1]
        .into_iter()
        .find(|index| {
            let name = ["first.md", "second.md"][*index];
            app.screen().texts().iter().any(|text| text.ends_with(name))
        })
        .unwrap_or_else(|| panic!("one row stays: {:?}", app.screen().texts()));
    let committed = 1 - remaining;

    app.press("label:Discard").expect("discard the second row");
    app.wait_until("the first hold to commit", Duration::from_secs(5), |_| {
        checkpoint_files(std::slice::from_ref(&keys[committed])) == 0
    })
    .expect("taking the slot commits the first hold");
    assert_eq!(
        checkpoint_files(std::slice::from_ref(&keys[remaining])),
        1,
        "the second hold keeps its entry"
    );

    // The Undo on the line now owns only the second discard: it brings back
    // one row, and the committed draft stays gone.
    app.press("id:status-action").expect("undo the second");
    app.wait_for("id:recovery-row-0").expect("one row returns");
    assert_eq!(
        checkpoint_files(&keys),
        1,
        "the committed draft does not come back"
    );
}

fn a_newer_notice_does_not_hide_the_pending_undo() {
    let mut seeded = two_recovered();
    let keys = seeded.keys.clone();
    let app = &mut seeded.app;

    app.press("id:recovery-discard-all").expect("discard all");
    app.wait_for("id:status-action").expect("the Undo");
    assert_eq!(checkpoint_files(&keys), 2);

    // A done message overwrites the notice; the Undo stays on the line and
    // outlives the done message's own fade window.
    app.press("id:welcome-open-sample")
        .expect("open the sample folder");
    app.wait_for("id:workspace-home")
        .expect("the sample folder opens");
    app.wait(Duration::from_millis(3_500))
        .expect("advance past the done message's hold");
    assert!(
        app.screen().matches("id:status-action").expect("selector"),
        "the Undo survives the newer notice: {:?}",
        app.screen().texts()
    );

    // It still restores the held rows and their entries.
    app.press("id:status-action")
        .expect("undo after the overwrite");
    app.wait_for("id:recovery-banner")
        .expect("the rows come back");
    assert_eq!(checkpoint_files(&keys), 2, "nothing was deleted");
}
