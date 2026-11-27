//! Goal 22, lane 22-B: live disk changes without locks.
//!
//! A clean document reloads from disk with a quiet notice, a dirty one raises
//! the conflict banner at once, the app's own save is never an external
//! change, a delete raises the Save As banner and an older read never clears
//! it, a file opened with no workspace is still watched, a workspace with no
//! tabs keeps its tree current, and a symbolic link's target outside the
//! workspace is watched through the link.

use std::path::Path;
use std::time::Duration;

use nsh_core::element::{Idiom, Os};
use nsh_core::{
    MountItem, NodeId,
    animation::{AnimationOp, AnimationProp, Driver},
};
use nsh_engine::{EngineConfig, Points, Root};
use nsh_testing::{Headless, Screen};

mod support;

use support::wait_for_marker;

const EDITOR: &str = "label:Markdown source";

fn file_uri(path: &Path) -> String {
    url::Url::from_file_path(path)
        .expect("an absolute file path")
        .to_string()
}

/// The app with `MARKTURBO_OPEN` pointed at `open` (a folder or a file).
fn start(config: &Path, open: Option<&Path>) -> Headless {
    // SAFETY: nextest runs each test in its own process.
    unsafe {
        match open {
            Some(path) => std::env::set_var("MARKTURBO_OPEN", path),
            None => std::env::remove_var("MARKTURBO_OPEN"),
        }
        std::env::set_var("MARKTURBO_CONFIG_DIR", config);
        std::env::set_var("MARKTURBO_DATA_DIR", config.join("data"));
        std::env::set_var(
            "MARKTURBO_TEST_LOCK_NAMESPACE",
            std::process::id().to_string(),
        );
    }
    let mut engine = EngineConfig::new(Root::of::<mt_app::App>());
    // A drop is a desktop event: the headless default is an iOS phone.
    engine.environment.os = Os::Windows;
    engine.environment.idiom = Idiom::Desktop;
    let mut app = Headless::start(engine).expect("start the app");
    app.set_viewport(nsh_core::Size::new(Points(1280.0), Points(800.0)))
        .expect("viewport");
    app
}

/// A workspace with no tab open: its home lists the tree, which proves the
/// folder committed (and therefore that the recursive watch is installed)
/// before the test writes anything.
fn start_workspace(config: &Path, workspace: &Path) -> Headless {
    let mut app = start(config, Some(workspace));
    app.wait_for("id:workspace-home")
        .expect("the workspace home");
    app
}

fn open_from_home(app: &mut Headless, name: &str) {
    // The sidebar lists the same file with the same label; the workspace home
    // is the surface this helper opens from.
    app.press(&format!("id:workspace-home >> label:{name}"))
        .unwrap_or_else(|why| panic!("open `{name}` from the workspace home: {why}"));
    app.wait_for(EDITOR).expect("the source editor");
}

fn shows(screen: &Screen, text: &str) -> bool {
    screen
        .texts()
        .iter()
        .any(|shown| shown.trim() == text || shown.contains(text))
}

/// Pump frames until `deadline` of real time passed, so real-clock work (the
/// watcher's debouncer) lands while the app keeps ticking.
fn pump_for(app: &mut Headless, duration: Duration) {
    let deadline = std::time::Instant::now() + duration;
    while std::time::Instant::now() < deadline {
        let _ = app.next_change(Duration::from_millis(25));
    }
}

fn wait_for_text(app: &mut Headless, text: &str, what: &str) {
    wait_for_marker(app, what, Duration::from_secs(20), |screen| {
        shows(screen, text)
    });
}

/// The laid-out view with test id `id`.
fn node_id(app: &Headless, id: &str) -> NodeId {
    app.screen()
        .layout_nodes()
        .into_iter()
        .find(|node| node.test_id.as_deref() == Some(id))
        .unwrap_or_else(|| panic!("no `{id}` on screen: {:?}", app.screen().texts()))
        .id
}

/// The clean-reload slip's own animation values (`ui/document.rs`): the text
/// arrives 6 points off its place and at part ink, and settles to both at
/// once. Its nodes are the ones a batch sets to these values, so the test
/// reads the graph the screen is drawn from, not a rendered frame.
const SLIP_OFFSET_PX: f32 = 6.0;
const SLIP_DIM: f32 = 0.35;

#[derive(Default)]
struct SlipMarks {
    sets: Vec<(NodeId, f32)>,
    starts: Vec<(NodeId, f32)>,
    created: Vec<(NodeId, f32)>,
    bindings: Vec<(NodeId, AnimationProp, NodeId)>,
}

impl SlipMarks {
    fn observe(app: &mut Headless) -> std::sync::Arc<std::sync::Mutex<SlipMarks>> {
        let marks = std::sync::Arc::new(std::sync::Mutex::new(SlipMarks::default()));
        let recorded = marks.clone();
        app.observe(move |_, batch| {
            let mut marks = recorded.lock().unwrap();
            for item in &batch.items {
                match item {
                    MountItem::Animation(AnimationOp::Set { node, value }) => {
                        marks.sets.push((*node, *value));
                    }
                    MountItem::Animation(AnimationOp::Start {
                        node,
                        driver: Driver::Timing { to, .. },
                        ..
                    }) => {
                        marks.starts.push((*node, *to));
                    }
                    MountItem::Animation(AnimationOp::CreateValue { node, value }) => {
                        marks.created.push((*node, *value));
                    }
                    MountItem::AnimationBindings { id, bindings } => {
                        for (prop, node) in bindings {
                            marks.bindings.push((*id, *prop, *node));
                        }
                    }
                    _ => {}
                }
            }
        });
        marks
    }

    /// The value node a batch set to `value`, when one did.
    fn node_set_to(&self, value: f32) -> Option<NodeId> {
        self.sets
            .iter()
            .find(|(_, set)| *set == value)
            .map(|(node, _)| *node)
    }

    /// The value node `view` binds to a horizontal translation, as mounted.
    fn translate_of(&self, view: NodeId) -> Option<NodeId> {
        self.bindings
            .iter()
            .find(|(id, prop, _)| *id == view && matches!(prop, AnimationProp::Transform(_)))
            .map(|(_, _, node)| *node)
    }

    /// The initial value a node was created with, when it was created.
    fn created_for(&self, node: NodeId) -> Option<f32> {
        self.created
            .iter()
            .find(|(created, _)| *created == node)
            .map(|(_, value)| *value)
    }

    fn starts_to(&self, node: NodeId, to: f32) -> bool {
        self.starts
            .iter()
            .any(|(started, value)| *started == node && *value == to)
    }

    fn starts_on(&self, node: NodeId) -> bool {
        self.starts.iter().any(|(started, _)| *started == node)
    }
}

#[test]
fn an_external_write_reloads_a_clean_tab_and_says_updated_from_disk() {
    let workspace = tempfile::tempdir().expect("workspace");
    let config = tempfile::tempdir().expect("config dir");
    let path = workspace.path().join("plan.md");
    std::fs::write(&path, "# Original\n").expect("write plan.md");

    let mut app = start_workspace(config.path(), workspace.path());
    open_from_home(&mut app, "plan.md");
    app.wait_for("text:# Original").expect("the document");

    let marks = SlipMarks::observe(&mut app);
    std::fs::write(&path, "# External\n").expect("external write");
    wait_for_text(&mut app, "Updated from disk", "the quiet notice");
    wait_for_text(&mut app, "# External", "the reloaded text");
    // The adopted text slips in: it arrives 6 points off its place and at
    // part ink, and settles to both (decision 6, lane 22-B). Only a
    // successful adoption sets the slip marker, so this is the clean
    // reload's own movement.
    let marks = marks.lock().unwrap();
    let offset = marks
        .node_set_to(SLIP_OFFSET_PX)
        .expect("the slip sets the text off its place");
    assert!(
        marks.starts_to(offset, 0.0),
        "the arriving text settles at its place"
    );
    let ink = marks
        .node_set_to(SLIP_DIM)
        .expect("the slip dims the arriving text");
    assert!(
        marks.starts_to(ink, 1.0),
        "the arriving text fades up to full ink"
    );
    assert_eq!(
        app.screen().count("id:conflict-banner").expect("selector"),
        0,
        "a clean reload raises no banner: {:?}",
        app.screen().texts()
    );
}

#[test]
fn an_external_write_to_a_dirty_tab_raises_the_conflict_banner() {
    let workspace = tempfile::tempdir().expect("workspace");
    let config = tempfile::tempdir().expect("config dir");
    let path = workspace.path().join("plan.md");
    std::fs::write(&path, "# Original\n").expect("write plan.md");

    let mut app = start_workspace(config.path(), workspace.path());
    open_from_home(&mut app, "plan.md");
    app.type_text(EDITOR, "\nLocal edit.")
        .expect("edit the buffer");

    let marks = SlipMarks::observe(&mut app);
    std::fs::write(&path, "# External\n").expect("external write");
    wait_for_text(&mut app, "Changed on disk", "the conflict banner");
    assert!(
        shows(app.screen(), "Local edit."),
        "the edits stay in the buffer: {:?}",
        app.screen().texts()
    );
    assert!(
        !shows(app.screen(), "Updated from disk"),
        "a conflict shows no reload notice: {:?}",
        app.screen().texts()
    );
    // A conflict is not an adoption: the box never slips, even across the
    // debounce and the ticks that follow.
    pump_for(&mut app, Duration::from_millis(2_400));
    assert_eq!(
        marks.lock().unwrap().node_set_to(SLIP_OFFSET_PX),
        None,
        "a conflict plays no slip: {:?}",
        app.screen().texts()
    );
}

#[test]
fn the_apps_own_save_never_reloads_or_conflicts() {
    let workspace = tempfile::tempdir().expect("workspace");
    let config = tempfile::tempdir().expect("config dir");
    let path = workspace.path().join("plan.md");
    std::fs::write(&path, "# Original\n").expect("write plan.md");

    let mut app = start_workspace(config.path(), workspace.path());
    open_from_home(&mut app, "plan.md");
    app.type_text(EDITOR, "\nSaved edit.")
        .expect("edit the buffer");
    app.press_key("mod+s").expect("save");
    wait_for_text(&mut app, "Saved plan.md", "the save notice");
    assert!(
        std::fs::read_to_string(&path)
            .expect("read the saved file")
            .contains("Saved edit."),
        "the save reached the disk"
    );

    // The save's own events reach the watcher: give them more than the debounce
    // and two drain ticks (the drain runs on the app's 1 s ticker), and
    // nothing may reload or conflict.
    pump_for(&mut app, Duration::from_millis(2_400));
    assert_eq!(
        app.screen().count("id:conflict-banner").expect("selector"),
        0,
        "our own save is not an external change: {:?}",
        app.screen().texts()
    );
    assert!(
        !shows(app.screen(), "Updated from disk"),
        "our own save shows no reload notice: {:?}",
        app.screen().texts()
    );
    assert!(
        shows(app.screen(), "Saved edit."),
        "the buffer keeps the saved text: {:?}",
        app.screen().texts()
    );
}

#[test]
fn a_delete_during_a_reloads_read_keeps_the_missing_banner() {
    let workspace = tempfile::tempdir().expect("workspace");
    let config = tempfile::tempdir().expect("config dir");
    let path = workspace.path().join("plan.md");
    let release = config.path().join("release-reload-read");
    std::fs::write(&path, "# Original\n").expect("write plan.md");

    let mut app = start_workspace(config.path(), workspace.path());
    open_from_home(&mut app, "plan.md");
    app.wait_for("text:# Original").expect("the document");

    // The external write starts a reload whose read parks on the gate.
    mt_app::test_vault::gate_read(&path, &release);
    let marks = SlipMarks::observe(&mut app);
    std::fs::write(&path, "# External\n").expect("external write");
    wait_for_marker(
        &mut app,
        "the reloaded read reaches its gate",
        Duration::from_secs(20),
        |_| mt_app::test_vault::gate_reads_started(&path) >= 1,
    );

    // The file is deleted while that read is still in flight: the banner goes
    // up, and the older read's answer must never clear it.
    std::fs::remove_file(&path).expect("delete the file");
    wait_for_text(&mut app, "Moved or deleted on disk", "the Save As banner");

    std::fs::write(&release, b"").expect("let the stale read finish");
    pump_for(&mut app, Duration::from_millis(800));
    assert!(
        shows(app.screen(), "Moved or deleted on disk"),
        "the delete notice survives the older read: {:?}",
        app.screen().texts()
    );
    assert!(
        !shows(app.screen(), "Updated from disk"),
        "the stale read never reloads: {:?}",
        app.screen().texts()
    );
    assert!(
        shows(app.screen(), "# Original"),
        "the buffer is untouched: {:?}",
        app.screen().texts()
    );
    // A failed read is not an adoption either: no slip ever plays.
    assert_eq!(
        marks.lock().unwrap().node_set_to(SLIP_OFFSET_PX),
        None,
        "a failed read plays no slip: {:?}",
        app.screen().texts()
    );
}

#[test]
fn an_edit_during_a_reloads_read_keeps_the_edits_and_raises_the_banner() {
    let workspace = tempfile::tempdir().expect("workspace");
    let config = tempfile::tempdir().expect("config dir");
    let path = workspace.path().join("plan.md");
    let release = config.path().join("release-reload-read");
    std::fs::write(&path, "# Original\n").expect("write plan.md");

    let mut app = start_workspace(config.path(), workspace.path());
    open_from_home(&mut app, "plan.md");
    app.wait_for("text:# Original").expect("the document");

    mt_app::test_vault::gate_read(&path, &release);
    std::fs::write(&path, "# External\n").expect("external write");
    wait_for_marker(
        &mut app,
        "the reloaded read reaches its gate",
        Duration::from_secs(20),
        |_| mt_app::test_vault::gate_reads_started(&path) >= 1,
    );

    // The user edits while the disk is read.
    app.type_text(EDITOR, "\nLocal edit.")
        .expect("edit during the read");
    std::fs::write(&release, b"").expect("let the read finish");

    wait_for_text(&mut app, "Changed on disk", "the conflict banner");
    assert!(
        shows(app.screen(), "Local edit."),
        "the edits stay in the buffer: {:?}",
        app.screen().texts()
    );
    assert!(
        !shows(app.screen(), "# External"),
        "the disk version is not adopted: {:?}",
        app.screen().texts()
    );
    assert!(
        !shows(app.screen(), "Updated from disk"),
        "the refused reload shows no notice: {:?}",
        app.screen().texts()
    );
}

#[test]
fn a_document_opened_without_a_workspace_is_still_watched() {
    let directory = tempfile::tempdir().expect("document dir");
    let config = tempfile::tempdir().expect("config dir");
    let path = directory.path().join("dropped.md");
    std::fs::write(&path, "# Dropped\n").expect("write dropped.md");

    let mut app = start(config.path(), None);
    app.drop_files("id:shell", vec![file_uri(&path)])
        .expect("drop the document");
    app.wait_for("text:# Dropped").expect("the document");

    // No workspace: the only watch is the dropped document's own directory.
    std::fs::write(&path, "# External drop\n").expect("external write");
    wait_for_text(&mut app, "# External drop", "the reloaded text");
    wait_for_text(&mut app, "Updated from disk", "the quiet notice");
}

#[test]
fn a_workspace_with_no_tabs_keeps_its_tree_current() {
    let workspace = tempfile::tempdir().expect("workspace");
    let config = tempfile::tempdir().expect("config dir");
    std::fs::write(workspace.path().join("plan.md"), "# Plan\n").expect("write plan.md");

    let mut app = start_workspace(config.path(), workspace.path());
    app.wait_for("id:workspace-home-file >> text:plan.md")
        .expect("the tree lists the file that was there");

    std::fs::write(workspace.path().join("appeared.md"), "# Appeared\n").expect("create a file");
    wait_for_marker(
        &mut app,
        "the tree lists the new file",
        Duration::from_secs(20),
        |screen| {
            screen
                .matches("id:workspace-home-file >> text:appeared.md")
                .unwrap_or(false)
        },
    );
}

#[cfg(windows)]
fn symlink_file(target: &Path, link: &Path) -> std::io::Result<()> {
    std::os::windows::fs::symlink_file(target, link)
}

#[cfg(unix)]
fn symlink_file(target: &Path, link: &Path) -> std::io::Result<()> {
    std::os::unix::fs::symlink(target, link)
}

/// The linked document and its target, or a skip when this machine cannot
/// create file symbolic links.
fn linked_document(workspace: &Path, outside: &Path) -> Option<std::path::PathBuf> {
    let target = outside.join("target.md");
    std::fs::write(&target, "# Target one\n").expect("write the target");
    let link = workspace.join("link.md");
    if let Err(error) = symlink_file(&target, &link) {
        eprintln!("skipping file-symlink test: {error}");
        return None;
    }
    Some(target)
}

#[test]
fn an_external_write_to_a_symlink_target_outside_the_workspace_reloads_the_clean_tab() {
    let workspace = tempfile::tempdir().expect("workspace");
    let outside = tempfile::tempdir().expect("outside dir");
    let config = tempfile::tempdir().expect("config dir");
    let Some(target) = linked_document(workspace.path(), outside.path()) else {
        return;
    };

    let mut app = start_workspace(config.path(), workspace.path());
    open_from_home(&mut app, "link.md");
    app.wait_for("text:# Target one")
        .expect("the linked document is open");

    std::fs::write(&target, "# Target two\n").expect("external write to the target");
    wait_for_text(&mut app, "# Target two", "the reloaded text");
    wait_for_text(&mut app, "Updated from disk", "the quiet notice");
}

#[test]
fn an_external_write_to_a_symlink_target_raises_the_banner_on_a_dirty_tab() {
    let workspace = tempfile::tempdir().expect("workspace");
    let outside = tempfile::tempdir().expect("outside dir");
    let config = tempfile::tempdir().expect("config dir");
    let Some(target) = linked_document(workspace.path(), outside.path()) else {
        return;
    };

    let mut app = start_workspace(config.path(), workspace.path());
    open_from_home(&mut app, "link.md");
    app.wait_for("text:# Target one")
        .expect("the linked document is open");
    app.type_text(EDITOR, "\nLocal edit.")
        .expect("edit the buffer");

    std::fs::write(&target, "# Target two\n").expect("external write to the target");
    wait_for_text(&mut app, "Changed on disk", "the conflict banner");
    assert!(
        shows(app.screen(), "Local edit."),
        "the edits stay in the buffer: {:?}",
        app.screen().texts()
    );
}

/// A link whose target lives inside the workspace: deleting the target must
/// raise the missing banner. The target's resolved spelling is the canonical
/// one (verbatim on Windows), the event's is the watch's, and only the one
/// normalized spelling matches them after the target is gone.
#[test]
fn deleting_a_symlink_target_inside_the_workspace_raises_the_missing_banner() {
    let workspace = tempfile::tempdir().expect("workspace");
    let config = tempfile::tempdir().expect("config dir");
    let link = workspace.path().join("link.md");
    let target = workspace.path().join("real.md");
    std::fs::write(&target, "# Real\n").expect("write the target");
    if let Err(error) = symlink_file(&target, &link) {
        eprintln!("skipping file-symlink test: {error}");
        return;
    }

    let mut app = start_workspace(config.path(), workspace.path());
    open_from_home(&mut app, "link.md");
    app.wait_for("text:# Real")
        .expect("the linked document is open");

    std::fs::remove_file(&target).expect("delete the target");
    wait_for_text(&mut app, "Moved or deleted on disk", "the missing banner");
}

#[test]
fn a_retargeted_symlink_watch_follows_the_new_target() {
    let workspace = tempfile::tempdir().expect("workspace");
    let first = tempfile::tempdir().expect("first target dir");
    let second = tempfile::tempdir().expect("second target dir");
    let config = tempfile::tempdir().expect("config dir");
    let target_one = first.path().join("target.md");
    std::fs::write(&target_one, "# Target one\n").expect("write the first target");
    let target_two = second.path().join("target.md");
    std::fs::write(&target_two, "# Target two\n").expect("write the second target");
    let link = workspace.path().join("link.md");
    if let Err(error) = symlink_file(&target_one, &link) {
        eprintln!("skipping file-symlink test: {error}");
        return;
    }

    let mut app = start_workspace(config.path(), workspace.path());
    open_from_home(&mut app, "link.md");
    app.wait_for("text:# Target one")
        .expect("the linked document is open");

    // The link is retargeted outside the app; the reload adopts the new
    // target, and the watch must follow it to its own directory.
    std::fs::remove_file(&link).expect("unlink");
    symlink_file(&target_two, &link).expect("retarget the link");
    wait_for_text(&mut app, "# Target two", "the adopted new target");

    std::fs::write(&target_two, "# Target two changed\n").expect("write the new target");
    wait_for_text(
        &mut app,
        "# Target two changed",
        "the new target's own event",
    );
}

#[test]
fn typing_right_after_a_save_does_not_raise_a_conflict() {
    let workspace = tempfile::tempdir().expect("workspace");
    let config = tempfile::tempdir().expect("config dir");
    let path = workspace.path().join("plan.md");
    std::fs::write(&path, "# Original\n").expect("write plan.md");

    let mut app = start_workspace(config.path(), workspace.path());
    open_from_home(&mut app, "plan.md");
    app.type_text(EDITOR, "\nSaved edit.")
        .expect("edit the buffer");
    app.press_key("mod+s").expect("save");
    wait_for_text(&mut app, "Saved plan.md", "the save notice");
    // The user keeps typing before the save's own event is drained.
    app.type_text(EDITOR, "\nTyped on.")
        .expect("keep typing after the save");

    pump_for(&mut app, Duration::from_millis(2_400));
    assert_eq!(
        app.screen().count("id:conflict-banner").expect("selector"),
        0,
        "the save's own event is not a conflict: {:?}",
        app.screen().texts()
    );
    assert!(
        shows(app.screen(), "Typed on."),
        "the newer typing stays in the buffer: {:?}",
        app.screen().texts()
    );
    assert!(
        !shows(app.screen(), "Updated from disk"),
        "the save reloads nothing: {:?}",
        app.screen().texts()
    );
}

#[test]
fn a_manual_reload_answer_after_a_delete_keeps_the_missing_banner() {
    let workspace = tempfile::tempdir().expect("workspace");
    let config = tempfile::tempdir().expect("config dir");
    let path = workspace.path().join("plan.md");
    let release = config.path().join("release-commit");
    std::fs::write(&path, "# Original\n").expect("write plan.md");

    let mut app = start_workspace(config.path(), workspace.path());
    open_from_home(&mut app, "plan.md");
    app.type_text(EDITOR, "\nLocal edit.")
        .expect("edit the buffer");
    std::fs::write(&path, "# External\n").expect("external write");
    wait_for_text(&mut app, "Changed on disk", "the conflict banner");

    // The manual reload's snapshot is read, then held before it commits.
    mt_app::test_vault::gate_commit(&path, &release);
    app.press("id:conflict-use-disk")
        .expect("ask for the disk version");
    app.wait_for("id:guard-reload").expect("the reload guard");
    app.press("id:guard-reload").expect("confirm the reload");
    wait_for_marker(
        &mut app,
        "the manual read to reach its commit",
        Duration::from_secs(20),
        |_| mt_app::test_vault::commit_gate_reached(&path),
    );

    // The file disappears while the read's answer waits to commit.
    std::fs::remove_file(&path).expect("delete the file");
    wait_for_text(&mut app, "Moved or deleted on disk", "the Save As banner");

    std::fs::write(&release, b"").expect("let the stale answer commit");
    pump_for(&mut app, Duration::from_millis(800));
    assert!(
        shows(app.screen(), "Moved or deleted on disk"),
        "the newer delete notice survives the older read: {:?}",
        app.screen().texts()
    );
    assert!(
        !shows(app.screen(), "Updated from disk"),
        "the stale read never reloads: {:?}",
        app.screen().texts()
    );
    assert!(
        shows(app.screen(), "Local edit."),
        "the buffer keeps the edit: {:?}",
        app.screen().texts()
    );
}

#[test]
fn a_newer_disk_version_after_the_overwrite_card_is_not_clobbered() {
    let workspace = tempfile::tempdir().expect("workspace");
    let config = tempfile::tempdir().expect("config dir");
    let path = workspace.path().join("plan.md");
    std::fs::write(&path, "# Original\n").expect("write plan.md");

    let mut app = start_workspace(config.path(), workspace.path());
    open_from_home(&mut app, "plan.md");
    app.type_text(EDITOR, "\nLocal edit.")
        .expect("edit the buffer");
    std::fs::write(&path, "# Alpha disk\n").expect("external write");
    wait_for_text(&mut app, "Changed on disk", "the conflict banner");

    // The user compares Alpha and opens the overwrite confirmation bound to
    // it.
    app.press("label:Compare").expect("compare");
    app.wait_for("id:conflict-previous")
        .expect("the comparison panel");
    app.press("label:Keep mine").expect("keep mine");
    app.wait_for("id:guard-overwrite")
        .expect("the overwrite guard");

    // A newer version lands after the card was shown: the confirmation is
    // bound to the version the user compared, so Beta must survive.
    std::fs::write(&path, "# Beta disk\n").expect("a newer external write");
    app.wait_for_gone("id:conflict-previous", Duration::from_secs(20))
        .expect("the newer disk version closes the comparison");

    app.press("id:guard-overwrite")
        .expect("confirm the overwrite");
    pump_for(&mut app, Duration::from_millis(800));
    assert_eq!(
        std::fs::read_to_string(&path).expect("read the file"),
        "# Beta disk\n",
        "the newer disk version is not overwritten"
    );
    assert!(
        shows(app.screen(), "Changed on disk"),
        "the tab stays in a fresh conflict: {:?}",
        app.screen().texts()
    );
}

#[test]
fn a_pending_reload_never_overwrites_a_just_kept_version() {
    let workspace = tempfile::tempdir().expect("workspace");
    let config = tempfile::tempdir().expect("config dir");
    let path = workspace.path().join("plan.md");
    let release = config.path().join("release-kept-read");
    std::fs::write(&path, "# Original\n").expect("write plan.md");

    let mut app = start_workspace(config.path(), workspace.path());
    open_from_home(&mut app, "plan.md");
    app.type_text(EDITOR, "\nLocal edit.")
        .expect("edit the buffer");

    // An external write starts a reload whose read of Alpha completes and
    // parks before it commits. The save attempt raises the conflict while
    // that answer waits.
    mt_app::test_vault::gate_commit(&path, &release);
    std::fs::write(&path, "# Alpha disk\n").expect("external write");
    wait_for_marker(
        &mut app,
        "the reloaded answer reaches its commit",
        Duration::from_secs(20),
        |_| mt_app::test_vault::commit_gate_reached(&path),
    );
    app.press("id:title-settings")
        .expect("open the glyph's menu");
    app.press("id:title-menu-save").expect("attempt the save");
    wait_for_text(&mut app, "Changed on disk", "the conflict banner");
    app.press("label:Keep mine").expect("keep mine");
    app.wait_for("id:guard-overwrite")
        .expect("the overwrite guard");
    app.press("id:guard-overwrite")
        .expect("confirm the overwrite");
    wait_for_text(&mut app, "Saved plan.md", "the kept save");

    // Only now does the older, already-read answer commit: it read the
    // version the kept decision replaced, so it must be dropped.
    std::fs::write(&release, b"").expect("let the stale answer commit");
    pump_for(&mut app, Duration::from_millis(800));
    assert!(
        shows(app.screen(), "Local edit."),
        "the kept text stays in the editor: {:?}",
        app.screen().texts()
    );
    assert!(
        !shows(app.screen(), "Updated from disk"),
        "the stale read never reloads: {:?}",
        app.screen().texts()
    );
    assert!(
        std::fs::read_to_string(&path)
            .expect("read the file")
            .contains("Local edit."),
        "the kept text reached the disk"
    );
}

#[test]
fn a_dirty_retargeted_symlink_keeps_watching_the_new_target() {
    let workspace = tempfile::tempdir().expect("workspace");
    let first = tempfile::tempdir().expect("first target dir");
    let second = tempfile::tempdir().expect("second target dir");
    let config = tempfile::tempdir().expect("config dir");
    let target_one = first.path().join("target.md");
    std::fs::write(&target_one, "# Target one\n").expect("write the first target");
    let target_two = second.path().join("target.md");
    std::fs::write(&target_two, "# Target two\n").expect("write the second target");
    let link = workspace.path().join("link.md");
    if let Err(error) = symlink_file(&target_one, &link) {
        eprintln!("skipping file-symlink test: {error}");
        return;
    }

    let mut app = start_workspace(config.path(), workspace.path());
    open_from_home(&mut app, "link.md");
    app.wait_for("text:# Target one")
        .expect("the linked document is open");
    app.type_text(EDITOR, "\nLocal edit.")
        .expect("edit the buffer");

    // Retarget while dirty: the conflict goes up, and the new target's
    // parent becomes watched at detection time, not at adoption.
    std::fs::remove_file(&link).expect("unlink");
    symlink_file(&target_two, &link).expect("retarget the link");
    wait_for_text(&mut app, "Changed on disk", "the conflict banner");
    // The unlink and the re-link can arrive as two debounced batches; let
    // the second reload's answer land before the comparison opens, so the
    // newer target's own watch is what closes it, not an answer already in
    // flight.
    pump_for(&mut app, Duration::from_millis(1_500));

    app.press("label:Compare").expect("compare");
    app.wait_for("id:conflict-previous")
        .expect("the comparison panel");
    assert!(
        shows(app.screen(), "Target two"),
        "the comparison reads the new target: {:?}",
        app.screen().texts()
    );

    // A later write to the new target must reach the app: the comparison
    // closes because its version is no longer the disk's, which only the
    // watched new parent can say.
    std::fs::write(&target_two, "# Target two changed\n").expect("write the new target");
    app.wait_for_gone("id:conflict-previous", Duration::from_secs(20))
        .expect("the newer target write closes the comparison");
}

#[test]
fn a_newer_external_write_closes_an_open_comparison() {
    let workspace = tempfile::tempdir().expect("workspace");
    let config = tempfile::tempdir().expect("config dir");
    let path = workspace.path().join("plan.md");
    std::fs::write(&path, "# Original\n").expect("write plan.md");

    let mut app = start_workspace(config.path(), workspace.path());
    open_from_home(&mut app, "plan.md");
    app.type_text(EDITOR, "\nLocal edit.")
        .expect("edit the buffer");
    std::fs::write(&path, "# Alpha disk\n").expect("external write");
    wait_for_text(&mut app, "Changed on disk", "the conflict banner");

    app.press("label:Compare").expect("compare");
    app.wait_for("id:conflict-previous")
        .expect("the comparison panel");
    assert!(
        shows(app.screen(), "Alpha"),
        "the first disk version shows: {:?}",
        app.screen().texts()
    );

    // A newer version on disk: the comparison may not keep the older one as
    // the current disk version.
    std::fs::write(&path, "# Beta disk has more words\n").expect("a newer external write");
    app.wait_for_gone("id:conflict-previous", Duration::from_secs(20))
        .expect("the older comparison closes on the newer disk version");
    assert!(
        !shows(app.screen(), "Alpha"),
        "the compared version is gone: {:?}",
        app.screen().texts()
    );

    app.press("label:Compare").expect("compare again");
    app.wait_for("id:conflict-previous")
        .expect("the fresh comparison panel");
    assert!(
        shows(app.screen(), "Beta"),
        "the fresh disk version shows: {:?}",
        app.screen().texts()
    );
}

#[test]
fn a_relative_workspace_path_still_matches_its_own_tree_events() {
    let parent = tempfile::tempdir().expect("workspace parent");
    let config = tempfile::tempdir().expect("config dir");
    let workspace = parent.path().join("notes");
    std::fs::create_dir(&workspace).expect("create the workspace");
    std::fs::write(workspace.join("plan.md"), "# Plan\n").expect("write plan.md");

    // SAFETY: nextest runs each test in its own process.
    unsafe {
        std::env::set_var(
            "MARKTURBO_TEST_LOCK_NAMESPACE",
            std::process::id().to_string(),
        );
    }
    std::env::set_current_dir(parent.path()).expect("enter the parent");
    let mut app = start(config.path(), Some(Path::new("notes")));
    app.wait_for("id:workspace-home-file >> text:plan.md")
        .expect("the relative workspace lists its file");

    std::fs::write(workspace.join("appeared.md"), "# Appeared\n").expect("create a file");
    wait_for_marker(
        &mut app,
        "the tree lists the new file",
        Duration::from_secs(20),
        |screen| {
            screen
                .matches("id:workspace-home-file >> text:appeared.md")
                .unwrap_or(false)
        },
    );
}

#[test]
fn a_normal_open_leaves_the_editor_at_its_place() {
    let workspace = tempfile::tempdir().expect("workspace");
    let config = tempfile::tempdir().expect("config dir");
    std::fs::write(workspace.path().join("plan.md"), "# Original\n").expect("write plan.md");

    let mut app = start_workspace(config.path(), workspace.path());
    let marks = SlipMarks::observe(&mut app);
    open_from_home(&mut app, "plan.md");
    app.wait_for("text:# Original").expect("the document");

    let editor = node_id(&app, "source-editor");
    let marks = marks.lock().unwrap();
    let offset = marks
        .translate_of(editor)
        .expect("the editor binds a slide offset");
    assert_eq!(
        marks.created_for(offset),
        Some(0.0),
        "a normal open starts the text at its place"
    );
    assert!(
        !marks.starts_on(offset),
        "a normal open starts no slide: {:?}",
        app.screen().texts()
    );
}

#[test]
fn under_reduced_motion_a_reload_does_not_slide_the_text() {
    let workspace = tempfile::tempdir().expect("workspace");
    let config = tempfile::tempdir().expect("config dir");
    let path = workspace.path().join("plan.md");
    std::fs::write(&path, "# Original\n").expect("write plan.md");

    // SAFETY: nextest runs each test in its own process.
    unsafe {
        std::env::set_var("MARKTURBO_OPEN", workspace.path());
        std::env::set_var("MARKTURBO_CONFIG_DIR", config.path());
        std::env::set_var("MARKTURBO_DATA_DIR", config.path().join("data"));
        std::env::set_var(
            "MARKTURBO_TEST_LOCK_NAMESPACE",
            std::process::id().to_string(),
        );
    }
    let mut engine = EngineConfig::new(Root::of::<mt_app::App>());
    engine.environment.os = Os::Windows;
    engine.environment.idiom = Idiom::Desktop;
    engine.environment.reduce_motion = true;
    let mut app = Headless::start(engine).expect("start the app");
    app.set_viewport(nsh_core::Size::new(Points(1280.0), Points(800.0)))
        .expect("viewport");
    app.wait_for("id:workspace-home")
        .expect("the workspace home");
    let marks = SlipMarks::observe(&mut app);
    open_from_home(&mut app, "plan.md");
    app.wait_for("text:# Original").expect("the document");

    std::fs::write(&path, "# External\n").expect("external write");
    wait_for_text(&mut app, "# External", "the reloaded text");

    let editor = node_id(&app, "source-editor");
    let marks = marks.lock().unwrap();
    assert!(
        marks.translate_of(editor).is_none(),
        "reduced motion binds no slide offset for the editor"
    );
    assert!(
        marks.node_set_to(SLIP_OFFSET_PX).is_none(),
        "reduced motion never moves the text"
    );
    // The fade alone still carries the slip after a successful adoption.
    let ink = marks
        .node_set_to(SLIP_DIM)
        .expect("reduced motion dims the arriving text");
    assert!(
        marks.starts_to(ink, 1.0),
        "reduced motion fades the arriving text back up"
    );
}

#[cfg(windows)]
#[test]
fn a_differently_cased_workspace_still_sees_its_symlink_target_deleted() {
    let parent = tempfile::tempdir().expect("workspace parent");
    let config = tempfile::tempdir().expect("config dir");
    let real = parent.path().join("Notes");
    std::fs::create_dir(&real).expect("the real workspace name");
    let flipped = parent.path().join("notes");
    let target = real.join("real.md");
    std::fs::write(&target, "# Real\n").expect("write the target");
    let link = real.join("link.md");
    if let Err(error) = symlink_file(&target, &link) {
        eprintln!("skipping file-symlink test: {error}");
        return;
    }

    let mut app = start(config.path(), Some(&flipped));
    app.wait_for("id:workspace-home")
        .expect("the differently cased workspace opens");
    open_from_home(&mut app, "link.md");
    app.wait_for("text:# Real")
        .expect("the linked document is open");

    std::fs::remove_file(&target).expect("delete the target");
    wait_for_text(&mut app, "Moved or deleted on disk", "the missing banner");
}

#[test]
fn a_deleted_file_tab_moves_to_outside_folder() {
    let workspace = tempfile::tempdir().expect("workspace");
    let config = tempfile::tempdir().expect("config dir");
    std::fs::write(workspace.path().join("plan.md"), "# Plan\n").expect("write plan.md");

    let mut app = start_workspace(config.path(), workspace.path());
    open_from_home(&mut app, "plan.md");
    app.wait_for("text:# Plan").expect("the document");

    std::fs::remove_file(workspace.path().join("plan.md")).expect("delete plan.md");
    wait_for_text(&mut app, "Moved or deleted on disk", "the missing banner");

    wait_for_marker(
        &mut app,
        "the deleted tab in Outside folder",
        Duration::from_secs(20),
        |screen| {
            screen
                .matches("id:sidebar-outside >> id:sidebar-outside-row")
                .unwrap_or(false)
        },
    );
    assert_eq!(
        app.screen().count("id:sidebar-unlisted").expect("selector"),
        0,
        "a deleted tab is not in Unlisted: {:?}",
        app.screen().texts()
    );
}

#[test]
fn a_symlink_tab_whose_target_vanished_leaves_the_tree_marking() {
    let workspace = tempfile::tempdir().expect("workspace");
    let outside = tempfile::tempdir().expect("outside dir");
    let config = tempfile::tempdir().expect("config dir");
    let Some(target) = linked_document(workspace.path(), outside.path()) else {
        return;
    };

    let mut app = start_workspace(config.path(), workspace.path());
    open_from_home(&mut app, "link.md");
    app.wait_for("text:# Target one")
        .expect("the linked document");

    std::fs::remove_file(&target).expect("delete the target");
    wait_for_text(&mut app, "Moved or deleted on disk", "the missing banner");

    wait_for_marker(
        &mut app,
        "the vanished target's tab in Outside folder and not the tree",
        Duration::from_secs(20),
        |screen| {
            screen
                .matches("id:sidebar-outside >> id:sidebar-outside-row")
                .unwrap_or(false)
                && screen
                    .count("id:sidebar-file >> id:sidebar-tree-open")
                    .unwrap_or(1)
                    == 0
        },
    );
}

#[test]
fn a_file_created_between_the_workspace_scan_and_its_watch_still_reaches_the_tree() {
    let workspace = tempfile::tempdir().expect("workspace");
    let config = tempfile::tempdir().expect("config dir");
    std::fs::write(workspace.path().join("plan.md"), "# Plan\n").expect("write plan.md");
    let scan_root = std::path::absolute(workspace.path()).expect("absolute root");
    let release = config.path().join("release-first-scan");

    // The first scan reads the folder and parks before committing.
    mt_app::test_vault::gate_commit(&scan_root, &release);
    let mut app = start(config.path(), Some(workspace.path()));
    wait_for_marker(
        &mut app,
        "the first workspace scan to reach its commit",
        Duration::from_secs(20),
        |_| mt_app::test_vault::commit_gate_reached(&scan_root),
    );

    // The file lands after the scan read but before the root watch.
    std::fs::write(workspace.path().join("appeared.md"), "# Appeared\n").expect("create a file");
    std::fs::write(&release, b"").expect("let the first scan commit");

    wait_for_marker(
        &mut app,
        "the re-scan lists the appeared file",
        Duration::from_secs(20),
        |screen| {
            screen
                .matches("id:workspace-home-file >> text:appeared.md")
                .unwrap_or(false)
        },
    );
}

#[test]
fn a_write_between_the_open_read_and_the_watch_still_reloads() {
    let workspace = tempfile::tempdir().expect("workspace");
    let other = tempfile::tempdir().expect("document dir");
    let config = tempfile::tempdir().expect("config dir");
    std::fs::write(workspace.path().join("plan.md"), "# Plan\n").expect("write plan.md");
    // The window exists only outside the workspace: the root watch already
    // covers a workspace file, but an outside document's own watch is
    // installed after its first read.
    let path = other.path().join("outside.md");
    std::fs::write(&path, "# Original\n").expect("write outside.md");
    let release = config.path().join("release-open-commit");

    let mut app = start_workspace(config.path(), workspace.path());
    mt_app::test_vault::gate_commit(&path, &release);
    app.drop_files("id:shell", vec![file_uri(&path)])
        .expect("drop the outside document");
    wait_for_marker(
        &mut app,
        "the opening read to reach its commit",
        Duration::from_secs(20),
        |_| mt_app::test_vault::commit_gate_reached(&path),
    );

    std::fs::write(&path, "# External\n").expect("external write before the watch");
    std::fs::write(&release, b"").expect("let the opening read finish");

    wait_for_text(&mut app, "# External", "the re-checked text");
}

#[test]
fn a_tree_row_that_becomes_a_directory_renders_the_folder_control() {
    let workspace = tempfile::tempdir().expect("workspace");
    let config = tempfile::tempdir().expect("config dir");
    let path = workspace.path().join("notes.md");
    std::fs::write(&path, "# Notes\n").expect("write notes.md");

    let mut app = start_workspace(config.path(), workspace.path());
    wait_for_marker(
        &mut app,
        "the file row",
        Duration::from_secs(20),
        |screen| {
            screen
                .matches("id:sidebar-file >> text:notes.md")
                .unwrap_or(false)
        },
    );

    // Replace the file with a directory of the same name inside one debounce
    // window: the entry flips kind without ever leaving the tree.
    std::fs::remove_file(&path).expect("remove the file");
    std::fs::create_dir(&path).expect("create the directory");
    std::fs::write(path.join("inside.md"), "# Inside\n").expect("write inside.md");

    wait_for_marker(
        &mut app,
        "the row becomes the folder control",
        Duration::from_secs(20),
        |screen| {
            screen
                .matches("id:sidebar-folder >> text:notes.md")
                .unwrap_or(false)
        },
    );
}

#[test]
fn a_drop_and_a_tree_event_resolve_no_paths_on_the_ui_thread() {
    let workspace = tempfile::tempdir().expect("workspace");
    let other = tempfile::tempdir().expect("document dir");
    let config = tempfile::tempdir().expect("config dir");
    std::fs::write(workspace.path().join("plan.md"), "# Plan\n").expect("write plan.md");
    let dropped = other.path().join("dropped.md");
    std::fs::write(&dropped, "# Dropped\n").expect("write dropped.md");

    let mut app = start_workspace(config.path(), workspace.path());
    open_from_home(&mut app, "plan.md");
    app.wait_for("text:# Plan").expect("the document");
    pump_for(&mut app, Duration::from_millis(600));

    let before_drop = mt_core::test_probe::path_resolutions();
    app.drop_files("id:shell", vec![file_uri(&dropped)])
        .expect("drop the document");
    app.wait_for("text:# Dropped")
        .expect("the dropped document");
    pump_for(&mut app, Duration::from_millis(600));
    assert_eq!(
        mt_core::test_probe::path_resolutions(),
        before_drop,
        "drop dispatch resolves no paths while the UI thread handles it"
    );

    let before_event = mt_core::test_probe::path_resolutions();
    std::fs::write(workspace.path().join("appeared.md"), "# Appeared\n").expect("create a file");
    wait_for_marker(
        &mut app,
        "the sidebar tree lists the created file",
        Duration::from_secs(20),
        |screen| {
            screen
                .matches("id:sidebar-file >> text:appeared.md")
                .unwrap_or(false)
        },
    );
    pump_for(&mut app, Duration::from_millis(600));
    assert_eq!(
        mt_core::test_probe::path_resolutions(),
        before_event,
        "watcher dispatch resolves no paths while the UI thread handles it"
    );
}
