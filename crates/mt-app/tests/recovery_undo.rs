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

    // SAFETY: nextest runs each test in its own process.
    unsafe {
        std::env::remove_var("MARKTURBO_OPEN");
        // No environment credential may make Review preparable: the
        // amendments' Review scenarios want the `No model set up` panel.
        std::env::remove_var("ANTHROPIC_API_KEY");
        std::env::remove_var("OPENAI_API_KEY");
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

/// Opens the fixture's `first.md` as a document over `workspace`, so a
/// Review can be prepared while a live Undo holds its rows.
fn open_first_document(app: &mut Headless, workspace: &std::path::Path) {
    app.on_pick_paths(vec![workspace.to_path_buf()])
        .expect("answer the folder picker");
    app.press("id:welcome-open-folder")
        .expect("open the workspace");
    app.wait_for("id:workspace-home").expect("the workspace");
    app.press_key("mod+p").expect("open the palette");
    app.wait_for("id:palette-input").expect("the palette input");
    app.type_text("id:palette-input", "first")
        .expect("filter to first.md");
    app.submit("id:palette-input").expect("open first.md");
    app.wait_for("label:Markdown source")
        .expect("the source editor");
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
    a_live_undo_stays_reachable_on_settings();
    an_expired_discard_keeps_a_newer_checkpoint_for_the_same_path();
    an_unreadable_record_survives_a_delayed_discard();
    a_live_undo_stays_reachable_when_a_background_notice_arrives();
}

/// Opening Settings unmounts the editor's status line: a live Undo has to
/// travel to the visible surface and still restore its rows there.
fn a_live_undo_stays_reachable_on_settings() {
    let mut seeded = two_recovered();
    let keys = seeded.keys.clone();
    let app = &mut seeded.app;
    app.press("id:recovery-discard-all").expect("discard all");
    app.wait_for("id:status-action").expect("the Undo");
    assert_eq!(
        checkpoint_files(&keys),
        2,
        "nothing commits inside the hold"
    );

    app.press("id:title-settings").expect("open the title menu");
    app.wait_for("id:title-menu").expect("the menu");
    app.press("id:title-menu-settings").expect("open Settings");
    app.wait_for("id:settings-page").expect("the settings page");
    app.wait_for("id:status-action")
        .expect("the Undo follows the user to Settings");
    assert_eq!(
        app.screen().count("id:status-action").expect("selector"),
        1,
        "the Undo renders on exactly one status line"
    );

    app.press("id:status-action")
        .expect("undo on the settings surface");
    app.wait_for_gone("id:status-action", Duration::from_secs(5))
        .expect("the Undo is spent");
    assert_eq!(
        checkpoint_files(&keys),
        2,
        "the returned rows keep their store entries"
    );

    app.press("id:settings-close").expect("close Settings");
    app.wait_for("id:recovery-banner")
        .expect("the rows are back in the banner");
}

/// A live Undo stays reachable while Settings is open even when a later
/// background notice lands: the notice follows the Undo to the visible
/// surface instead of routing it back to the unmounted editor line.
fn a_live_undo_stays_reachable_when_a_background_notice_arrives() {
    let mut seeded = two_recovered();
    let keys = seeded.keys.clone();
    let workspace = tempfile::tempdir().expect("workspace dir");
    let workspace_name = workspace
        .path()
        .file_name()
        .expect("a folder name")
        .to_string_lossy()
        .into_owned();
    let release = workspace.path().join("release-scan");
    let app = &mut seeded.app;

    app.press("id:recovery-discard-all").expect("discard all");
    app.wait_for("id:status-action").expect("the Undo");

    // A slow folder open starts first; its completion posts the background
    // notice while Settings is the visible surface.
    mt_app::test_vault::gate_read(workspace.path(), &release);
    app.on_pick_paths(vec![workspace.path().to_path_buf()])
        .expect("answer the folder picker");
    app.press("id:welcome-open-folder")
        .expect("start the slow open");
    app.wait_for("text:Opening folder…")
        .expect("the open announces itself");

    // Settings takes the surface and carries the Undo there.
    app.press("id:title-settings").expect("open the title menu");
    app.press("id:title-menu-settings").expect("open Settings");
    app.wait_for("id:settings-page").expect("the settings page");
    app.wait_for("id:settings-page >> id:status-action")
        .expect("the Undo follows the user to Settings");
    assert_eq!(
        app.screen().count("id:status-action").expect("selector"),
        1,
        "the Undo renders on exactly one status line"
    );

    // The scan completes: `Opened …` must not hide the Undo. The commit's
    // Recents write proves the notice landed while the checks run.
    std::fs::write(&release, "go").expect("release the slow scan");
    support::wait_for_file(
        app,
        &seeded._config.path().join("settings.toml"),
        |text| text.contains(&workspace_name),
        "the slow folder never committed its background notice",
        Duration::from_secs(10),
    );
    assert!(
        app.screen()
            .matches("id:settings-page >> id:status-action")
            .expect("selector"),
        "the background notice hid the Undo: {:?}",
        app.screen().texts()
    );
    assert_eq!(
        app.screen().count("id:status-action").expect("selector"),
        1,
        "the background notice put the Undo on a second line: {:?}",
        app.screen().texts()
    );
    assert_eq!(
        checkpoint_files(&keys),
        2,
        "nothing was deleted while the Undo had to stay reachable"
    );

    // The carried Undo still restores the rows.
    app.press("id:settings-page >> id:status-action")
        .expect("undo from the settings surface");
    app.press("id:settings-close").expect("close Settings");
    app.wait_for("id:recovery-banner")
        .expect("the rows are back in the banner");
    assert_eq!(checkpoint_files(&keys), 2, "nothing was deleted");
}

/// A live Undo and an open Review: starting a Review with no model set up
/// posts its own notice, and the Undo renders on the review line; closing
/// the panel must carry it back to the editor's line (contract amendment
/// 18).
#[test]
fn closing_the_review_panel_carries_the_live_undo_back_to_the_editor() {
    if !cfg!(windows) {
        // DPAPI is the production protector; non-Windows builds intentionally
        // have no plaintext recovery fallback.
        return;
    }
    let mut seeded = two_recovered();
    let keys = seeded.keys.clone();
    let app = &mut seeded.app;
    open_first_document(app, seeded._dir.path());

    // The discard's Undo must stay reachable through the Review panel.
    app.press("id:recovery-discard-all").expect("discard all");
    app.wait_for("id:status-action").expect("the Undo");

    // Review cannot be prepared without a model; its error notice carries
    // the live Undo, and the review line is where that Undo renders.
    app.press("id:title-review").expect("start Review");
    app.wait_for("id:review-unavailable")
        .expect("Review says no model is set up");
    app.wait_for("id:field >> id:status-action")
        .expect("the Undo renders on the review line");
    assert_eq!(
        app.screen().count("id:status-action").expect("selector"),
        1,
        "the Undo renders on exactly one status line"
    );

    // Closing the panel unmounts the review line; the Undo must move to the
    // editor's line, not vanish with the panel.
    app.press("id:review-close")
        .expect("close the review panel");
    app.wait_for_gone("id:review-unavailable", Duration::from_secs(5))
        .expect("the panel closes");
    app.wait_for("id:status-action")
        .expect("the Undo is back on the editor line");
    assert_eq!(
        app.screen().count("id:status-action").expect("selector"),
        1,
        "the Undo moved, it was not duplicated"
    );

    // It still restores the held rows and their store entries.
    app.press("id:status-action").expect("undo from the editor");
    app.wait_for("id:recovery-banner").expect("the rows return");
    assert_eq!(checkpoint_files(&keys), 2, "nothing was deleted");
}

/// Settings opened from the Review panel's own way forward: the one visible
/// status line is Settings', so the live Undo renders there (contract
/// amendment 18).
#[test]
fn settings_opened_from_the_review_panel_keeps_the_live_undo() {
    if !cfg!(windows) {
        // DPAPI is the production protector; non-Windows builds intentionally
        // have no plaintext recovery fallback.
        return;
    }
    let mut seeded = two_recovered();
    let keys = seeded.keys.clone();
    let app = &mut seeded.app;
    open_first_document(app, seeded._dir.path());

    // The discard's Undo must stay reachable through the Review panel.
    app.press("id:recovery-discard-all").expect("discard all");
    app.wait_for("id:status-action").expect("the Undo");
    app.press("id:title-review").expect("start Review");
    app.wait_for("id:review-unavailable")
        .expect("Review says no model is set up");
    app.wait_for("id:field >> id:status-action")
        .expect("the Undo renders on the review line");

    // Review's way forward opens Settings; the Undo follows to the one
    // status line still on screen.
    app.press("id:review-open-settings")
        .expect("open Settings from Review");
    app.wait_for("id:settings-page").expect("the settings page");
    app.wait_for("id:settings-page >> id:status-action")
        .expect("the Undo follows the user to Settings");
    assert_eq!(
        app.screen().count("id:status-action").expect("selector"),
        1,
        "the Undo renders on exactly one status line"
    );

    app.press("id:settings-page >> id:status-action")
        .expect("undo from the settings surface");
    app.wait_for_gone("id:status-action", Duration::from_secs(5))
        .expect("the Undo is spent");
    assert_eq!(checkpoint_files(&keys), 2, "nothing was deleted");
}

/// Switching tabs unmounts the Review panel the previous tab owned: a live
/// Undo must move to the editor line of the tab now on screen (contract
/// amendment 18, the tab-switch path).
#[test]
fn switching_tabs_carries_the_live_undo_to_the_visible_editor() {
    if !cfg!(windows) {
        // DPAPI is the production protector; non-Windows builds intentionally
        // have no plaintext recovery fallback.
        return;
    }
    let mut seeded = two_recovered();
    let keys = seeded.keys.clone();
    let app = &mut seeded.app;
    open_first_document(app, seeded._dir.path());

    // A second document, so the switch has a tab without a Review panel.
    app.press_key("mod+p").expect("open the palette");
    app.wait_for("id:palette-input").expect("the palette input");
    app.type_text("id:palette-input", "second")
        .expect("filter to second.md");
    app.press("id:palette-row").expect("open second.md");
    app.wait_for("id:title-file >> text:second.md")
        .expect("B's tab");

    // Back on A, open its Review panel: with no model set up the panel is
    // the unavailable state, and it still owns the review status line.
    app.press_key("mod+p").expect("open the palette");
    app.type_text("id:palette-input", "first")
        .expect("filter to first.md");
    app.press("id:palette-row").expect("open first.md");
    app.wait_for("id:title-file >> text:first.md")
        .expect("A's tab");
    app.press("id:title-review").expect("open A's Review");
    app.wait_for_within("id:review-unavailable", Duration::from_secs(20))
        .expect("A's Review panel");

    // The discard's Undo renders on the review line while A owns it.
    app.press("id:recovery-discard-all").expect("discard all");
    app.wait_for("id:field >> id:status-action")
        .expect("the Undo on A's review line");

    // Back to B: B has no Review panel, so the Undo must land on B's editor
    // line instead of unmounting with A's panel.
    app.press_key("mod+p").expect("open the palette");
    app.type_text("id:palette-input", "second")
        .expect("filter to second.md");
    app.press("id:palette-row").expect("open second.md");
    app.wait_for("id:title-file >> text:second.md")
        .expect("B's tab");
    app.wait_for_gone("id:review-unavailable", Duration::from_secs(5))
        .expect("A's Review panel closes");
    app.wait_for("id:status-action")
        .expect("the Undo is on B's editor line");
    assert_eq!(
        app.screen().count("id:status-action").expect("selector"),
        1,
        "the Undo renders on exactly one status line"
    );

    // It still restores the held rows and their store entries.
    app.press("id:status-action").expect("undo from B's editor");
    app.wait_for("id:recovery-banner").expect("the rows return");
    assert_eq!(checkpoint_files(&keys), 2, "nothing was deleted");
}

/// For the file a rejoined path's checkpoint is named by.
fn record_path(key: &RecoveryKey) -> std::path::PathBuf {
    mt_core::runtime_paths::recovery_dir()
        .expect("the recovery directory")
        .join(format!("{}.mtrecovery", key.as_str()))
}

/// The discard's commit is bound to the draft it captured: a checkpoint the
/// same path wrote inside the hold is a newer draft and survives.
fn an_expired_discard_keeps_a_newer_checkpoint_for_the_same_path() {
    let mut seeded = two_recovered();
    let keys = seeded.keys.clone();
    let app = &mut seeded.app;
    app.wait_for("id:recovery-discard-all")
        .expect("Discard all with two rows");
    app.press("id:recovery-discard-all").expect("discard all");
    app.wait_until("the rows to go", Duration::from_secs(5), |screen| {
        screen.count("id:recovery-banner").unwrap_or(1) == 0
    })
    .expect("the banner leaves");

    // Reopen the same path and edit it: a fresh checkpoint lands under the
    // same key while the old discard is still holding.
    app.on_pick_paths(vec![seeded._dir.path().to_path_buf()])
        .expect("answer the folder picker");
    app.press("id:welcome-open-folder")
        .expect("open the workspace");
    app.wait_for("id:workspace-home").expect("the workspace");
    app.press_key("mod+p").expect("open the palette");
    app.wait_for("id:palette-input").expect("the palette input");
    app.type_text("id:palette-input", "first")
        .expect("filter to first.md");
    app.submit("id:palette-input").expect("open first.md");
    app.wait_for("label:Markdown source")
        .expect("the source editor");
    app.type_text("label:Markdown source", "\nA newer draft.")
        .expect("edit the reopened file");
    app.wait(Duration::from_millis(3_500))
        .expect("advance to the fresh checkpoint");
    let first = record_path(&keys[0]);
    let second = record_path(&keys[1]);
    assert!(first.exists(), "the newer first.md checkpoint is written");
    let refreshed = std::fs::metadata(&first)
        .and_then(|metadata| metadata.modified())
        .expect("the fresh checkpoint's stamp");
    assert!(second.exists(), "the second hold is still waiting");

    // Let the old discard expire: its commit may only delete the drafts it
    // captured, never the checkpoint that replaced one of them.
    app.wait_for_gone("id:status-action", Duration::from_secs(5))
        .expect("the hold commits");
    app.wait(Duration::from_millis(500))
        .expect("advance past the commit");
    assert!(
        first.exists(),
        "the expired discard deleted the newer checkpoint for the same path"
    );
    assert_eq!(
        std::fs::metadata(&first)
            .and_then(|metadata| metadata.modified())
            .expect("the surviving checkpoint's stamp"),
        refreshed,
        "the newer checkpoint was rewritten by the commit"
    );
    assert!(
        !second.exists(),
        "the captured second.md draft is still deleted"
    );
}

/// The commit may only delete the record it captured. When the file under
/// the same key cannot be read, the scan lists it as an issue instead of a
/// record: it cannot be confirmed, so the commit leaves it alone.
fn an_unreadable_record_survives_a_delayed_discard() {
    let mut seeded = two_recovered();
    let keys = seeded.keys.clone();
    let app = &mut seeded.app;
    app.wait_for("id:recovery-discard-all")
        .expect("Discard all with two rows");
    app.press("id:recovery-discard-all").expect("discard all");
    app.wait_until("the rows to go", Duration::from_secs(5), |screen| {
        screen.count("id:recovery-banner").unwrap_or(1) == 0
    })
    .expect("the banner leaves");

    // The first record becomes unreadable while the hold runs.
    let first = record_path(&keys[0]);
    let second = record_path(&keys[1]);
    std::fs::write(&first, b"not a recovery record").expect("corrupt the checkpoint");

    // The commit can only prove the second record: the first survives.
    app.wait_for_gone("id:status-action", Duration::from_secs(5))
        .expect("the hold commits");
    app.wait(Duration::from_millis(500))
        .expect("advance past the commit");
    assert!(
        first.exists(),
        "the commit deleted a record it could not confirm"
    );
    assert!(!second.exists(), "the confirmed record is still deleted");
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
