//! Goal 22, lane 22-B: files and folders dropped on the window.
//!
//! The drop screen says only what a drag's names confirm; a drop opens each
//! supported document in order (the last one active), makes the last folder
//! the workspace while open tabs stay open, and reports what it skipped in
//! one count notice that names no paths. Both drop forms appear: with a
//! position (`FileDropAt`) and without one (Windows' `FileDrop`).

use std::path::Path;
use std::time::Duration;

use nsh_core::{
    EventKind, EventPayload, MountItem, NodeId,
    animation::{AnimationOp, AnimationProp, Driver},
    element::{Idiom, Os},
};
use nsh_engine::{EngineConfig, Points, Root};
use nsh_testing::Headless;

mod support;

fn file_uri(path: &Path) -> String {
    url::Url::from_file_path(path)
        .expect("an absolute file path")
        .to_string()
}

fn folder_uri(path: &Path) -> String {
    url::Url::from_directory_path(path)
        .expect("an absolute directory path")
        .to_string()
}

/// The app, with `MARKTURBO_OPEN` pointed at `open` when there is one.
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

/// The first view with test id `id`, as laid out.
fn node_id(app: &Headless, id: &str) -> NodeId {
    app.screen()
        .layout_nodes()
        .into_iter()
        .find(|node| node.test_id.as_deref() == Some(id))
        .unwrap_or_else(|| panic!("no `{id}` on screen: {:?}", app.screen().texts()))
        .id
}

/// The open tabs the sidebar lists: one close affordance per open row — the
/// tree marks an open file's row, and the outside and unlisted groups list
/// their tabs. These tests run with the sidebar open, where every tab has its
/// row; the collapsed strip's ticks are not part of the count.
fn open_tabs(app: &Headless) -> usize {
    app.screen().count("id:rail-close").expect("selector")
}

/// One raw event on the shell, as a platform would deliver it.
fn dispatch(app: &mut Headless, kind: EventKind, payload: EventPayload) {
    let id = node_id(app, "shell");
    let engine = app.engine().control();
    engine.dispatch_event(id, kind, payload);
    let (send, receive) = std::sync::mpsc::channel();
    engine.flush(move |_| {
        let _ = send.send(());
    });
    receive
        .recv_timeout(Duration::from_secs(5))
        .expect("the event flushed");
    app.catch_up();
}

fn drag_enter(app: &mut Headless, uris: Vec<String>) {
    dispatch(app, EventKind::FileDragEnter, EventPayload::FileDrop(uris));
}

fn drag_leave(app: &mut Headless) {
    dispatch(app, EventKind::FileDragLeave, EventPayload::None);
}

#[test]
fn a_drop_opens_each_document_and_the_last_one_is_active() {
    let directory = tempfile::tempdir().expect("document dir");
    let config = tempfile::tempdir().expect("config dir");
    let first = directory.path().join("a.md");
    let second = directory.path().join("b.md");
    std::fs::write(&first, "# A file\n").expect("write a.md");
    std::fs::write(&second, "# B file\n").expect("write b.md");

    let mut app = start(config.path(), None);
    app.drop_files("id:shell", vec![file_uri(&first), file_uri(&second)])
        .expect("drop both documents");

    app.wait_for("text:# B file")
        .expect("the last dropped document is the active one");
    app.wait_for("text:b.md").expect("the active tab's title");
    assert_eq!(
        open_tabs(&app),
        2,
        "each dropped document holds a tab: {:?}",
        app.screen().texts()
    );
}

#[test]
fn a_drop_with_no_position_opens_documents_too() {
    let directory = tempfile::tempdir().expect("document dir");
    let config = tempfile::tempdir().expect("config dir");
    let path = directory.path().join("notes.md");
    std::fs::write(&path, "# No position\n").expect("write notes.md");

    let mut app = start(config.path(), None);
    // Windows sends `FileDrop(uris)` with no position; `Headless::drop_files`
    // sends the `FileDropAt` form.
    dispatch(
        &mut app,
        EventKind::FileDrop,
        EventPayload::FileDrop(vec![file_uri(&path)]),
    );

    app.wait_for("text:# No position")
        .expect("the dropped document opened");
    assert_eq!(open_tabs(&app), 1);
}

#[test]
fn a_folder_drop_opens_it_as_the_workspace_and_keeps_the_tabs() {
    let workspace = tempfile::tempdir().expect("workspace dir");
    let dropped = tempfile::tempdir().expect("dropped folder");
    let config = tempfile::tempdir().expect("config dir");
    let open_file = workspace.path().join("open.md");
    std::fs::write(&open_file, "# Already open\n").expect("write open.md");
    std::fs::write(dropped.path().join("inside.md"), "# Inside\n").expect("write inside.md");

    let mut app = start(config.path(), Some(&open_file));
    app.wait_for("text:# Already open")
        .expect("the file is open");

    app.drop_files("id:shell", vec![folder_uri(dropped.path())])
        .expect("drop the folder");

    // The dropped folder is the workspace: the palette lists its file.
    app.press_key("mod+p").expect("open the palette");
    app.wait_for("id:palette-input")
        .expect("the palette's line");
    app.wait_for("text:inside.md")
        .expect("the dropped folder became the workspace");
    assert_eq!(
        open_tabs(&app),
        1,
        "the open tab stays open: {:?}",
        app.screen().texts()
    );
}

#[test]
fn an_unsupported_drop_shows_one_count_notice_and_names_no_path() {
    let directory = tempfile::tempdir().expect("document dir");
    let config = tempfile::tempdir().expect("config dir");
    let picture = directory.path().join("photo.png");
    std::fs::write(&picture, b"\x89PNG\r\n\x1a\n").expect("write photo.png");

    let mut app = start(config.path(), None);
    app.drop_files("id:shell", vec![file_uri(&picture)])
        .expect("drop the picture");

    app.wait_for("text:Skipped 1 file that can't be opened")
        .expect("one count notice");
    assert!(
        !app.screen()
            .texts()
            .iter()
            .any(|text| text.contains("photo.png")),
        "the notice never names the skipped path: {:?}",
        app.screen().texts()
    );
    assert_eq!(open_tabs(&app), 0, "nothing opened for an unsupported file");
}

#[test]
fn the_drop_screen_follows_the_confirmed_unknown_and_unsupported_cases() {
    let directory = tempfile::tempdir().expect("document dir");
    let config = tempfile::tempdir().expect("config dir");
    let document = directory.path().join("plan.md");
    let second = directory.path().join("notes.md");
    std::fs::write(&document, "# plan\n").expect("write plan.md");
    std::fs::write(&second, "# notes\n").expect("write notes.md");

    let mut app = start(config.path(), None);

    drag_enter(&mut app, vec![file_uri(&document)]);
    app.wait_for("text:Open plan.md")
        .expect("one confirmed document is named");
    drag_leave(&mut app);
    app.wait_for_gone("id:drop-screen", Duration::from_secs(5))
        .expect("a leave clears the screen");

    drag_enter(&mut app, vec![file_uri(&document), file_uri(&second)]);
    app.wait_for("text:Open 2 files")
        .expect("several confirmed documents are counted");
    drag_leave(&mut app);
    app.wait_for_gone("id:drop-screen", Duration::from_secs(5))
        .expect("a leave clears the screen");

    // No extension and no document name: it might be a folder the event did
    // not type.
    drag_enter(&mut app, vec!["file:///tmp/binary-blob".to_owned()]);
    app.wait_for("text:Drop to open")
        .expect("an unconfirmed name offers the plain line");
    drag_leave(&mut app);
    app.wait_for_gone("id:drop-screen", Duration::from_secs(5))
        .expect("a leave clears the screen");

    // The hover reads names and types only: a folder named like a file,
    // without a trailing slash and without a type, is not confirmed here
    // (the drop's own metadata settles it), and is never called
    // unsupported.
    let folder = directory.path().join("archive.v1");
    std::fs::create_dir(&folder).expect("create archive.v1");
    let folder_uri = folder_uri(&folder);
    drag_enter(&mut app, vec![folder_uri.trim_end_matches('/').to_owned()]);
    app.wait_for("text:Drop to open")
        .expect("a folder named like a file stays unconfirmed while it hovers");
    drag_leave(&mut app);
    app.wait_for_gone("id:drop-screen", Duration::from_secs(5))
        .expect("a leave clears the screen");

    // The event's trailing slash is what marks the item as a folder.
    drag_enter(&mut app, vec![folder_uri]);
    app.wait_for("text:Open archive.v1")
        .expect("a folder the event marked is openable");
    drag_leave(&mut app);
    app.wait_for_gone("id:drop-screen", Duration::from_secs(5))
        .expect("a leave clears the screen");

    let picture = directory.path().join("photo.png");
    std::fs::write(&picture, b"\x89PNG\r\n\x1a\n").expect("write photo.png");
    drag_enter(&mut app, vec![file_uri(&picture)]);
    app.wait_for("text:Can't open this file type")
        .expect("every offered item is a confirmed unsupported file");
}

/// The animation graph items of every view, recorded as the batches come: the
/// drop screen's own view id is only known while it is up, so the bindings it
/// declared are resolved after the fact.
#[derive(Default)]
struct ScreenBindings {
    bindings: Vec<(NodeId, AnimationProp, NodeId)>,
    sets: Vec<(NodeId, f32)>,
    starts: Vec<(NodeId, f32)>,
}

impl ScreenBindings {
    fn offsets(&self, translate: NodeId) -> Vec<f32> {
        self.sets
            .iter()
            .filter(|(node, _)| *node == translate)
            .map(|(_, value)| *value)
            .collect()
    }

    fn starts_to(&self, translate: NodeId) -> Vec<f32> {
        self.starts
            .iter()
            .filter(|(node, _)| *node == translate)
            .map(|(_, value)| *value)
            .collect()
    }

    /// The value node `view` binds to a translation, as mounted.
    fn translate_of(&self, view: NodeId) -> Option<NodeId> {
        self.bindings
            .iter()
            .find(|(id, prop, _)| *id == view && matches!(prop, AnimationProp::Transform(_)))
            .map(|(_, _, node)| *node)
    }

    /// The value node `view` binds to its opacity, as mounted.
    fn opacity_of(&self, view: NodeId) -> Option<NodeId> {
        self.bindings
            .iter()
            .find(|(id, prop, _)| *id == view && matches!(prop, AnimationProp::Opacity))
            .map(|(_, _, node)| *node)
    }

    fn observe(app: &mut Headless) -> std::sync::Arc<std::sync::Mutex<ScreenBindings>> {
        let seen = std::sync::Arc::new(std::sync::Mutex::new(ScreenBindings::default()));
        let recorded = seen.clone();
        app.observe(move |_, batch| {
            let mut seen = recorded.lock().unwrap();
            for item in &batch.items {
                match item {
                    MountItem::AnimationBindings { id, bindings } => {
                        for (prop, node) in bindings {
                            seen.bindings.push((*id, *prop, *node));
                        }
                    }
                    MountItem::Animation(AnimationOp::Set { node, value }) => {
                        seen.sets.push((*node, *value));
                    }
                    MountItem::Animation(AnimationOp::Start {
                        node,
                        driver: Driver::Timing { to, .. },
                        ..
                    }) => {
                        seen.starts.push((*node, *to));
                    }
                    _ => {}
                }
            }
        });
        seen
    }
}

#[test]
fn the_drop_screen_slides_in_from_the_right_edge_unless_motion_is_reduced() {
    let directory = tempfile::tempdir().expect("document dir");
    let config = tempfile::tempdir().expect("config dir");
    let document = directory.path().join("plan.md");
    std::fs::write(&document, "# plan\n").expect("write plan.md");
    let uri = file_uri(&document);

    let mut app = start(config.path(), None);
    let seen = ScreenBindings::observe(&mut app);
    drag_enter(&mut app, vec![uri.clone()]);
    app.wait_for("id:drop-screen").expect("the screen is up");
    let screen = node_id(&app, "drop-screen");
    drag_leave(&mut app);
    app.wait_for_gone("id:drop-screen", Duration::from_secs(5))
        .expect("the screen leaves");

    let seen = seen.lock().unwrap();
    let translate = seen
        .translate_of(screen)
        .expect("the screen binds a translation");
    assert!(
        seen.offsets(translate).contains(&320.0),
        "the first frame starts behind the right edge: {:?}",
        seen.offsets(translate)
    );
    assert!(
        seen.starts_to(translate).contains(&0.0),
        "the screen moves to the edge: {:?}",
        seen.starts_to(translate)
    );
}

#[test]
fn under_reduced_motion_the_drop_screen_fades_without_translating() {
    let directory = tempfile::tempdir().expect("document dir");
    let config = tempfile::tempdir().expect("config dir");
    let document = directory.path().join("plan.md");
    std::fs::write(&document, "# plan\n").expect("write plan.md");

    // SAFETY: nextest runs each test in its own process.
    unsafe {
        std::env::remove_var("MARKTURBO_OPEN");
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

    let seen = ScreenBindings::observe(&mut app);
    drag_enter(&mut app, vec![file_uri(&document)]);
    app.wait_for("text:Open plan.md")
        .expect("the screen is up with its title");
    let screen = node_id(&app, "drop-screen");

    let seen = seen.lock().unwrap();
    let translate = seen
        .translate_of(screen)
        .expect("the screen binds a translation");
    assert!(
        seen.offsets(translate).iter().all(|offset| *offset == 0.0),
        "reduced motion never moves the screen: {:?}",
        seen.offsets(translate)
    );
    assert!(
        seen.starts_to(translate).is_empty(),
        "reduced motion starts no translation: {:?}",
        seen.starts_to(translate)
    );
    assert!(
        seen.opacity_of(screen).is_some(),
        "the screen still fades in: {:?}",
        app.screen().texts()
    );
}

#[test]
fn a_drop_behind_a_card_changes_nothing() {
    let directory = tempfile::tempdir().expect("document dir");
    let config = tempfile::tempdir().expect("config dir");
    let open_path = directory.path().join("open.md");
    std::fs::write(&open_path, "# Open\n").expect("write open.md");
    let dropped = directory.path().join("dropped.md");
    std::fs::write(&dropped, "# Dropped\n").expect("write dropped.md");

    let mut app = start(config.path(), Some(&open_path));
    app.wait_for("text:# Open").expect("the document");
    app.type_text("label:Markdown source", "\nLocal edit.")
        .expect("edit the buffer");
    std::fs::write(&open_path, "# External\n").expect("external write");
    app.wait_for("text:Changed on disk")
        .expect("the conflict banner");

    // The reload guard is up: it owns the interaction.
    app.press("id:conflict-use-disk")
        .expect("ask for the disk version");
    app.wait_for("id:guard-reload").expect("the guard card");

    drag_enter(&mut app, vec![file_uri(&dropped)]);
    app.wait(Duration::from_millis(300))
        .expect("let a screen appear");
    assert_eq!(
        app.screen().count("id:drop-screen").expect("selector"),
        0,
        "no drop screen behind the card: {:?}",
        app.screen().texts()
    );

    app.drop_files("id:shell", vec![file_uri(&dropped)])
        .expect("drop behind the card");
    app.wait(Duration::from_millis(800))
        .expect("let a drop commit");
    assert_eq!(
        open_tabs(&app),
        1,
        "no tab opens behind the card: {:?}",
        app.screen().texts()
    );
    assert!(
        !app.screen()
            .texts()
            .iter()
            .any(|text| text.contains("# Dropped")),
        "the dropped document stays closed: {:?}",
        app.screen().texts()
    );
}

#[test]
fn a_drop_that_lands_behind_settings_does_not_open() {
    let directory = tempfile::tempdir().expect("document dir");
    let config = tempfile::tempdir().expect("config dir");
    let dropped = directory.path().join("slow.md");
    let release = config.path().join("release-drop-read");
    std::fs::write(&dropped, "# Slow\n").expect("write slow.md");

    let mut app = start(config.path(), None);
    app.wait_for("id:welcome-open-folder").expect("the welcome");

    // The drop is accepted while nothing blocks it; its read parks.
    mt_app::test_vault::gate_read(&dropped, &release);
    app.drop_files("id:shell", vec![file_uri(&dropped)])
        .expect("drop the slow document");
    support::wait_for_marker(
        &mut app,
        "the dropped read to reach its gate",
        Duration::from_secs(20),
        |_| mt_app::test_vault::gate_reads_started(&dropped) >= 1,
    );

    // Settings opens while the read is in flight, before the commit.
    app.press("id:title-settings")
        .expect("open the glyph's menu");
    app.wait_for("id:title-menu-settings")
        .expect("the Settings item");
    app.press("id:title-menu-settings").expect("open Settings");

    std::fs::write(&release, b"").expect("let the read finish");
    app.wait(Duration::from_millis(800))
        .expect("let the drop commit");
    app.press_key("escape").expect("close Settings");
    app.wait_for("id:welcome-open-folder")
        .expect("the drop opened nothing: the welcome is back");
    assert_eq!(
        open_tabs(&app),
        0,
        "the drop commits nothing behind Settings: {:?}",
        app.screen().texts()
    );
}

#[test]
fn a_folder_scan_that_lands_behind_settings_does_not_open() {
    let dropped = tempfile::tempdir().expect("dropped folder");
    let config = tempfile::tempdir().expect("config dir");
    std::fs::write(dropped.path().join("inside.md"), "# Inside\n").expect("write inside.md");
    let release = config.path().join("release-folder-scan");
    let scan_path = std::path::absolute(dropped.path()).expect("absolute scan path");

    let mut app = start(config.path(), None);
    app.wait_for("id:welcome-open-folder").expect("the welcome");

    // The drop is accepted while nothing blocks it; its folder scan parks.
    mt_app::test_vault::gate_read(&scan_path, &release);
    app.drop_files("id:shell", vec![folder_uri(dropped.path())])
        .expect("drop the folder");
    support::wait_for_marker(
        &mut app,
        "the folder scan to reach its gate",
        Duration::from_secs(20),
        |_| mt_app::test_vault::gate_reads_started(&scan_path) >= 1,
    );

    // Settings opens while the scan is in flight, before the commit.
    app.press("id:title-settings")
        .expect("open the glyph's menu");
    app.wait_for("id:title-menu-settings")
        .expect("the Settings item");
    app.press("id:title-menu-settings").expect("open Settings");

    std::fs::write(&release, b"").expect("let the scan finish");
    app.wait(Duration::from_millis(800))
        .expect("let the drop commit");
    app.press_key("escape").expect("close Settings");
    app.wait_for("id:welcome-open-folder")
        .expect("the workspace stayed closed: the welcome is back");
    assert_eq!(
        app.screen()
            .count("id:workspace-home-file")
            .expect("selector"),
        0,
        "the dismissed scan never becomes the workspace: {:?}",
        app.screen().texts()
    );
}

#[test]
fn a_folder_drop_blocked_by_a_card_clears_its_opening_notice() {
    let work = tempfile::tempdir().expect("workspace");
    let dropped = tempfile::tempdir().expect("dropped folder");
    let config = tempfile::tempdir().expect("config dir");
    let path = work.path().join("plan.md");
    std::fs::write(&path, "# Original\n").expect("write plan.md");
    std::fs::write(dropped.path().join("inside.md"), "# Inside\n").expect("write inside.md");
    let release = config.path().join("release-folder-scan");
    let scan_path = std::path::absolute(dropped.path()).expect("absolute scan path");

    let mut app = start(config.path(), Some(&path));
    app.wait_for("text:# Original").expect("the document");
    app.type_text("label:Markdown source", "\nLocal edit.")
        .expect("edit the buffer");
    std::fs::write(&path, "# External\n").expect("external write");
    app.wait_for("text:Changed on disk")
        .expect("the conflict banner");

    // The drop is accepted while nothing blocks it; its folder scan parks.
    mt_app::test_vault::gate_read(&scan_path, &release);
    app.drop_files("id:shell", vec![folder_uri(dropped.path())])
        .expect("drop the folder");
    support::wait_for_marker(
        &mut app,
        "the folder scan to reach its gate",
        Duration::from_secs(20),
        |_| mt_app::test_vault::gate_reads_started(&scan_path) >= 1,
    );
    app.wait_for("text:Opening folder…")
        .expect("the drop announces the folder open");

    // A card opens while the scan is in flight: the scan commits nothing,
    // and the held line belongs to the cancelled open, not to the card.
    app.press("id:conflict-use-disk")
        .expect("ask for the disk version");
    app.wait_for("id:guard-reload").expect("the reload guard");
    std::fs::write(&release, b"").expect("let the scan finish");
    app.wait(Duration::from_millis(800))
        .expect("let the drop land");
    assert!(
        !app.screen()
            .texts()
            .iter()
            .any(|text| text.contains("Opening folder")),
        "a blocked folder drop clears its held notice: {:?}",
        app.screen().texts()
    );
}

#[test]
fn a_superseded_mixed_drop_publishes_no_skipped_notice() {
    let first = tempfile::tempdir().expect("dropped folder");
    let other = tempfile::tempdir().expect("picked folder");
    let config = tempfile::tempdir().expect("config dir");
    let doc = first.path().join("slow.md");
    let picture = first.path().join("photo.png");
    let release = config.path().join("release-slow-drop-read");
    std::fs::write(&doc, "# Slow\n").expect("write slow.md");
    std::fs::write(&picture, b"\x89PNG").expect("write photo.png");
    std::fs::write(other.path().join("other.md"), "# Other\n").expect("write other.md");

    let mut app = start(config.path(), None);
    app.wait_for("id:welcome-open-folder").expect("the welcome");

    // The drop names the slow document, a skipped picture and folder A; its
    // document read parks.
    mt_app::test_vault::gate_read(&doc, &release);
    app.drop_files(
        "id:shell",
        vec![file_uri(&doc), file_uri(&picture), folder_uri(first.path())],
    )
    .expect("drop the mixed set");
    support::wait_for_marker(
        &mut app,
        "the dropped read to reach its gate",
        Duration::from_secs(20),
        |_| mt_app::test_vault::gate_reads_started(&doc) >= 1,
    );

    // While the document loads the user opens folder B.
    app.on_pick_paths(vec![other.path().to_path_buf()])
        .expect("answer the picker");
    app.press("id:welcome-open-folder").expect("open folder B");
    app.wait_for("id:workspace-home-file >> text:other.md")
        .expect("B is the workspace");

    std::fs::write(&release, b"").expect("let the slow read finish");
    app.wait(Duration::from_millis(1_500))
        .expect("let the drop land");
    assert!(
        !app.screen()
            .texts()
            .iter()
            .any(|text| text.contains("Skipped")),
        "a superseded drop publishes no skipped count: {:?}",
        app.screen().texts()
    );
}

#[test]
fn a_drop_behind_the_review_consent_changes_nothing() {
    let directory = tempfile::tempdir().expect("document dir");
    let config = tempfile::tempdir().expect("config dir");
    let open_path = directory.path().join("open.md");
    std::fs::write(&open_path, "# Open\n").expect("write open.md");
    let dropped = directory.path().join("dropped.md");
    std::fs::write(&dropped, "# Dropped\n").expect("write dropped.md");

    let base_url = "http://127.0.0.1:1/v1/";
    let endpoint = mt_core::model::EndpointIdentity::parse(
        mt_core::model::Provider::OpenAiChat,
        Some(base_url),
    )
    .expect("endpoint");
    std::fs::write(
        config.path().join("settings.toml"),
        format!(
            "model-provider = \"openai-chat\"\nmodel-name = \"drop-model\"\nmodel-base-url = \"{base_url}\"\nmodel-environment-key-identity = \"{}\"\n",
            endpoint.credential_target()
        ),
    )
    .expect("write settings");
    // SAFETY: nextest runs each test in its own process.
    unsafe {
        std::env::set_var("OPENAI_API_KEY", "loopback-test-key");
    }

    let mut app = start(config.path(), Some(&open_path));
    app.wait_for("text:# Open").expect("the document");
    app.press("id:title-review").expect("open Review");
    app.wait_for("id:consent-card")
        .expect("the Review disclosure");

    drag_enter(&mut app, vec![file_uri(&dropped)]);
    app.wait(Duration::from_millis(300))
        .expect("let a screen appear");
    assert_eq!(
        app.screen().count("id:drop-screen").expect("selector"),
        0,
        "no drop screen behind the consent: {:?}",
        app.screen().texts()
    );

    app.drop_files("id:shell", vec![file_uri(&dropped)])
        .expect("drop behind the consent");
    app.wait(Duration::from_millis(800))
        .expect("let a drop commit");
    assert_eq!(
        open_tabs(&app),
        1,
        "no tab opens behind the consent: {:?}",
        app.screen().texts()
    );
    assert!(
        !app.screen()
            .texts()
            .iter()
            .any(|text| text.contains("# Dropped")),
        "the dropped document stays closed: {:?}",
        app.screen().texts()
    );
}

#[test]
fn a_slow_drop_never_overrides_a_newer_workspace_choice() {
    let first = tempfile::tempdir().expect("dropped folder");
    let other = tempfile::tempdir().expect("picked folder");
    let config = tempfile::tempdir().expect("config dir");
    let doc = first.path().join("slow.md");
    let release = config.path().join("release-slow-drop-read");
    std::fs::write(&doc, "# Slow\n").expect("write slow.md");
    std::fs::write(other.path().join("other.md"), "# Other\n").expect("write other.md");

    let mut app = start(config.path(), None);
    app.wait_for("id:welcome-open-folder").expect("the welcome");

    // The drop names the slow document and folder A; its read parks.
    mt_app::test_vault::gate_read(&doc, &release);
    app.drop_files("id:shell", vec![file_uri(&doc), folder_uri(first.path())])
        .expect("drop the slow document and folder A");
    support::wait_for_marker(
        &mut app,
        "the dropped read to reach its gate",
        Duration::from_secs(20),
        |_| mt_app::test_vault::gate_reads_started(&doc) >= 1,
    );

    // While the document loads the user opens folder B.
    app.on_pick_paths(vec![other.path().to_path_buf()])
        .expect("answer the picker");
    app.press("id:welcome-open-folder").expect("open folder B");
    app.wait_for("id:workspace-home-file >> text:other.md")
        .expect("B is the workspace");

    // The older drop's folder must not become the workspace now: the palette
    // still lists B's files and none of A's.
    std::fs::write(&release, b"").expect("let the slow read finish");
    app.wait(Duration::from_millis(1_500))
        .expect("let the drop land");
    app.press_key("mod+p").expect("open the palette");
    app.wait_for("id:palette-input")
        .expect("the palette's line");
    app.type_text("id:palette-input", "slow")
        .expect("filter to A's file");
    app.settle();
    assert_eq!(
        app.screen().count("id:palette-row").expect("selector"),
        0,
        "A's file is not a workspace entry: {:?}",
        app.screen().texts()
    );
    app.press_key("escape").expect("close the palette");
    app.wait_for_gone("id:palette", Duration::from_secs(5))
        .expect("the palette closes");
    app.press_key("mod+p").expect("open the palette again");
    app.wait_for("id:palette-input")
        .expect("the palette's line again");
    app.type_text("id:palette-input", "other")
        .expect("filter to B's file");
    app.wait_for("text:other.md")
        .expect("B stays the workspace");
}

#[test]
fn a_file_dropped_twice_opens_one_tab() {
    let directory = tempfile::tempdir().expect("document dir");
    let config = tempfile::tempdir().expect("config dir");
    let path = directory.path().join("notes.md");
    let release = config.path().join("release-double-drop");
    std::fs::write(&path, "# Notes\n").expect("write notes.md");

    let mut app = start(config.path(), None);
    app.wait_for("id:welcome-open-folder").expect("the welcome");

    // Two drops of one slow file: both reads are in flight before either
    // commits.
    mt_app::test_vault::gate_reads(&path, [release.clone(), release.clone()]);
    app.drop_files("id:shell", vec![file_uri(&path)])
        .expect("first drop");
    app.drop_files("id:shell", vec![file_uri(&path)])
        .expect("second drop");
    support::wait_for_marker(
        &mut app,
        "both dropped reads to reach their gates",
        Duration::from_secs(20),
        |_| mt_app::test_vault::gate_reads_started(&path) >= 2,
    );

    std::fs::write(&release, b"").expect("let both reads finish");
    app.wait_for("text:# Notes").expect("the document opened");
    app.wait(Duration::from_millis(800))
        .expect("let the second drop commit");
    assert_eq!(
        open_tabs(&app),
        1,
        "one file is one tab: {:?}",
        app.screen().texts()
    );
}
