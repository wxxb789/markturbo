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
        app.screen().count("id:rail-file-row").expect("selector"),
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
    assert_eq!(app.screen().count("id:rail-file-row").expect("selector"), 1);
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
        app.screen().count("id:rail-file-row").expect("selector"),
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
    assert_eq!(
        app.screen().count("id:rail-file-row").expect("selector"),
        0,
        "nothing opened for an unsupported file"
    );
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

    drag_enter(
        &mut app,
        vec![file_uri(&directory.path().join("photo.png"))],
    );
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
