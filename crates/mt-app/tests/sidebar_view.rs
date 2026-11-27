//! The Shoji sidebar and view track, exercised through the mounted app.

use std::{
    collections::HashSet,
    path::PathBuf,
    sync::{Arc, Mutex, MutexGuard, mpsc},
    time::{Duration, SystemTime},
};

use mt_app::ui::{motion, tokens};
use nsh_core::{
    AccessibilityRole, Bounds, Command, EventKind, EventPayload, MountItem, NodeId, PayloadValue,
    ROOT_ID, Size,
    animation::{AnimationOp, AnimationProp, Animator, Driver},
    geometry::{point, px},
    keymap::{Keymap, Keystroke},
    platform::{Idiom, Os},
    time::Clock,
};
use nsh_engine::{EngineConfig, Points, Root};
use nsh_testing::{DragOptions, Headless, LayoutNode, Screen};

const WAIT: Duration = Duration::from_secs(5);
const WIDE: Size<Points> = Size::new(Points(1440.0), Points(900.0));
const NARROW: Size<Points> = Size::new(Points(900.0), Points(700.0));
const COLLAPSED: f32 = 44.0;
const EDIT: &str = "View: Edit — switch to Split";
const SPLIT: &str = "View: Split — switch to Preview";
const PREVIEW: &str = "View: Preview — switch to Edit";
const DIVIDER: &str = "id:stage-divider";

struct Fixture {
    dir: tempfile::TempDir,
    _environment: MutexGuard<'static, ()>,
}

impl Fixture {
    fn new(startup: &str, body: &str) -> Self {
        static ENVIRONMENT: Mutex<()> = Mutex::new(());
        let fixture = Self {
            dir: tempfile::tempdir().expect("test directory"),
            _environment: ENVIRONMENT.lock().unwrap_or_else(|err| err.into_inner()),
        };
        let path = fixture.write(startup, body);
        let config = fixture.dir.path().join("config");
        // SAFETY: nextest runs each suite test in its own process. If this
        // file runs as a standalone test binary, the guard serializes its
        // apps and is held until the app has dropped.
        unsafe {
            std::env::set_var("MARKTURBO_OPEN", path);
            std::env::set_var("MARKTURBO_CONFIG_DIR", &config);
            std::env::set_var("MARKTURBO_DATA_DIR", config.join("data"));
            std::env::set_var(
                "MARKTURBO_TEST_LOCK_NAMESPACE",
                std::process::id().to_string(),
            );
        }
        fixture
    }

    fn notes() -> Self {
        Self::new("workspace/notes.md", "# Notes\n")
    }

    fn write(&self, relative: &str, body: &str) -> PathBuf {
        let path = self.dir.path().join(relative);
        std::fs::create_dir_all(path.parent().expect("document directory"))
            .expect("create document directory");
        std::fs::write(&path, body).expect("write Markdown fixture");
        path
    }
}

fn start(reduce_motion: bool) -> Headless {
    let mut config = EngineConfig::new(Root::of::<mt_app::App>());
    config.clock = Clock::manual(SystemTime::now());
    config.environment.os = Os::Windows;
    config.environment.idiom = Idiom::Desktop;
    config.environment.reduce_motion = reduce_motion;
    Headless::start(config).expect("start the app")
}

fn open_document_at(app: &mut Headless, viewport: Size<Points>) {
    // Until the app's own window mounts, Headless would resize its hidden host.
    app.wait_for("id:title-view").expect("mount the app window");
    app.set_viewport(viewport).expect("set the viewport");
    app.wait_until("workspace fills the requested viewport", WAIT, |screen| {
        node(screen, "workspace")
            .is_some_and(|workspace| workspace.bounds.size.width == viewport.width)
    })
    .expect("resize the visible app window");
    app.wait_for("id:source-editor").expect("open the document");
}

fn start_in(viewport: Size<Points>) -> Headless {
    let mut app = start(false);
    open_document_at(&mut app, viewport);
    app
}

fn node(screen: &Screen, id: &str) -> Option<LayoutNode> {
    screen
        .layout_nodes()
        .into_iter()
        .find(|node| node.test_id.as_deref() == Some(id))
}

fn bounds(screen: &Screen, id: &str) -> Bounds<Points> {
    node(screen, id)
        .unwrap_or_else(|| panic!("no `{id}` on screen"))
        .bounds
}

fn wait_for_sidebar_width(app: &mut Headless, width: f32) {
    app.wait_until(&format!("sidebar width is {width}"), WAIT, |screen| {
        node(screen, "sidebar").is_some_and(|sidebar| sidebar.bounds.size.width.0 == width)
    })
    .expect("the sidebar reaches its expected width");
}

fn title_is(screen: &Screen, title: &str) -> bool {
    node(screen, "title-file-name")
        .and_then(|node| node.text)
        .is_some_and(|text| text.text.text == title)
}

fn wait_for_title(app: &mut Headless, title: &str) {
    app.wait_until(&format!("active file is {title}"), WAIT, |screen| {
        title_is(screen, title)
    })
    .expect("the title row names the active document");
}

fn assert_row(app: &Headless, selector: &str, id: &str) {
    let screen = app.screen();
    let row = screen.id(selector).expect("the named button exists");
    assert!(
        screen
            .layout_nodes()
            .iter()
            .any(|node| node.id == row && node.test_id.as_deref() == Some(id)),
        "{selector} must be a `{id}` button"
    );
}

fn assert_view(app: &Headless, name: &str) {
    let button = app
        .screen()
        .accessibility(Some("id:title-view"))
        .expect("one view button")
        .expect("the view button is mounted");
    assert_eq!(button.role, Some(AccessibilityRole::Button));
    assert_eq!(button.name, name);
}

fn divider_x(app: &Headless) -> f32 {
    let screen = app.screen();
    bounds(screen, "stage-divider").center().x.0 - bounds(screen, "stage").origin.x.0
}

fn stage_width(app: &Headless) -> f32 {
    bounds(app.screen(), "stage").size.width.0
}

fn assert_stop(app: &Headless, ratio: f32) {
    let actual = divider_x(app);
    let expected = stage_width(app) * ratio;
    assert!(
        (actual - expected).abs() <= 1.0,
        "divider at {actual:.2}, expected {expected:.2} points (stop {ratio})"
    );
}

fn finish_camera(app: &mut Headless) {
    app.run_for(Duration::from_millis(motion::CAMERA_MS as u64 + 32))
        .expect("finish the camera tween");
}

fn step_view(app: &mut Headless) {
    app.press("id:title-view").expect("press the view button");
    finish_camera(app);
}

#[test]
fn the_sidebar_opens_a_file_from_the_tree_in_one_click() {
    let fixture = Fixture::new("workspace/first.md", "# First heading\n");
    fixture.write("workspace/second.md", "# Second heading\n");
    let mut app = start_in(WIDE);
    wait_for_title(&mut app, "first.md");
    wait_for_sidebar_width(&mut app, tokens::SIDEBAR_W);

    let second = "id:sidebar-tree >> role:button[second.md]";
    app.wait_for(second).expect("the second file in the tree");
    assert_row(&app, second, "sidebar-file");
    app.press(second).expect("one press opens second.md");
    wait_for_title(&mut app, "second.md");
    app.wait_for("id:source-editor >> text:# Second heading")
        .expect("the source editor shows the second document");
}

#[test]
fn ctrl_b_remembers_the_collapsed_sidebar() {
    let _fixture = Fixture::notes();
    let mut app = start_in(WIDE);
    wait_for_sidebar_width(&mut app, tokens::SIDEBAR_W);
    app.press_key("mod+b").expect("collapse the sidebar");
    wait_for_sidebar_width(&mut app, COLLAPSED);
    drop(app);

    let mut restarted = start_in(WIDE);
    wait_for_sidebar_width(&mut restarted, COLLAPSED);
}

#[test]
fn the_narrow_layout_collapses_the_sidebar_and_overlays_it_without_pushing() {
    let _fixture = Fixture::notes();
    let mut app = start_in(NARROW);
    let started = app.clock().now();
    app.wait_until("the narrow stage starts at x=44", WAIT, |screen| {
        node(screen, "stage").is_some_and(|stage| stage.bounds.origin.x.0 == COLLAPSED)
    })
    .unwrap_or_else(|err| {
        panic!(
            "narrow layout after {:?} of manual frame time: sidebar width {}, stage x {}; {err}",
            app.clock().now().saturating_duration_since(started),
            bounds(app.screen(), "sidebar").size.width.0,
            bounds(app.screen(), "stage").origin.x.0,
        );
    });
    let assert_stage_stays = |screen: &Screen| {
        assert_eq!(bounds(screen, "stage").origin.x.0, COLLAPSED);
    };
    assert_stage_stays(app.screen());
    assert!(node(app.screen(), "sidebar-panel").is_none());

    let stage = node(app.screen(), "stage").expect("the narrow stage").id;
    let (positions, stage_xs) = mpsc::channel();
    app.observe(move |_, batch| {
        for item in &batch.items {
            if let MountItem::Frame { id, frame } = item
                && *id == stage
            {
                let _ = positions.send(frame.origin.x.0);
            }
        }
    });
    app.press_key("mod+b").expect("open the sidebar overlay");
    app.wait_until("the panel is fully open over the stage", WAIT, |screen| {
        assert_stage_stays(screen);
        node(screen, "sidebar-panel").is_some_and(|panel| {
            bounds(screen, "sidebar-panel-clip").size.width.0 == tokens::SIDEBAR_W
                && panel.bounds.intersects(&bounds(screen, "stage"))
        })
    })
    .expect("the open panel overlaps the stage without pushing it");

    app.press_key("escape")
        .expect("dismiss the sidebar overlay");
    app.wait_until("the overlay closes", WAIT, |screen| {
        assert_stage_stays(screen);
        node(screen, "sidebar-panel").is_none()
    })
    .expect("Escape removes the overlay without shifting the stage");
    for x in stage_xs.try_iter() {
        assert_eq!(x, COLLAPSED, "the overlay moved the stage during a frame");
    }
}

#[test]
fn the_view_button_cycles_in_order_and_the_shortcut_cycles_too() {
    let _fixture = Fixture::notes();
    let mut app = start_in(WIDE);
    assert_view(&app, EDIT);
    for next in [SPLIT, PREVIEW, EDIT] {
        app.press("id:title-view").expect("press the view button");
        assert_view(&app, next);
    }
    app.press_key("mod+e").expect("cycle with Ctrl+E");
    assert_view(&app, SPLIT);
}

#[test]
fn cycling_moves_the_divider_to_the_matching_stop() {
    let _fixture = Fixture::notes();
    let mut app = start_in(WIDE);
    wait_for_sidebar_width(&mut app, tokens::SIDEBAR_W);
    assert_stop(&app, 1.0);
    for ratio in [0.5, 0.0, 1.0] {
        step_view(&mut app);
        assert_stop(&app, ratio);
    }
}

#[test]
fn dragging_the_divider_snaps_to_the_nearest_stop() {
    let _fixture = Fixture::notes();
    let mut app = start_in(WIDE);
    step_view(&mut app);
    assert_stop(&app, 0.5);
    let width = stage_width(&app);

    app.drag_and_hold(
        DIVIDER,
        point(px(0.2 * width), px(0.0)),
        DragOptions::default(),
    )
    .expect("drag to 0.7 without releasing");
    assert_stop(&app, 0.7);
    app.drop_held().expect("release at 0.7");
    finish_camera(&mut app);
    assert_stop(&app, 0.5);

    app.drag(
        DIVIDER,
        point(px(-0.3 * width), px(0.0)),
        DragOptions::default(),
    )
    .expect("drag to 0.2 and release");
    finish_camera(&mut app);
    assert_stop(&app, 0.0);

    step_view(&mut app); // Preview -> Edit -> Split.
    step_view(&mut app);
    assert_stop(&app, 0.5);
    app.drag(
        DIVIDER,
        point(px(0.3 * width), px(0.0)),
        DragOptions::default(),
    )
    .expect("drag past 0.75 and release");
    finish_camera(&mut app);
    assert_stop(&app, 1.0);
}

#[derive(Default)]
struct MountTrace {
    keymap: Option<Arc<Keymap>>,
    bindings: Vec<(NodeId, AnimationProp, u32)>,
    animation: Vec<MountItem>,
}

fn traced_document(reduce_motion: bool) -> (Headless, Arc<Mutex<MountTrace>>) {
    let mut app = start(reduce_motion);
    let trace = Arc::new(Mutex::new(MountTrace::default()));
    app.observe({
        let trace = trace.clone();
        move |_, batch| {
            let mut trace = trace.lock().expect("mount trace");
            for item in &batch.items {
                match item {
                    MountItem::Keymap(keymap) => trace.keymap = Some(keymap.clone()),
                    MountItem::AnimationBindings { id, bindings } => trace
                        .bindings
                        .extend(bindings.iter().map(|&(prop, node)| (*id, prop, node))),
                    MountItem::Animation(_) => trace.animation.push(item.clone()),
                    _ => {}
                }
            }
        }
    });
    open_document_at(&mut app, WIDE);
    (app, trace)
}

/// Dispatch the mounted shortcut as the native keymap does, but don't let
/// Headless's normal `press_key` settle advance the manual clock to the stop.
fn cycle_without_advancing(app: &mut Headless, trace: &Mutex<MountTrace>) {
    let stroke = Keystroke::parse("mod+e", Os::Windows).expect("Ctrl+E keystroke");
    let id = trace
        .lock()
        .expect("mount trace")
        .keymap
        .as_ref()
        .expect("mounted keymap")
        .bindings()
        .iter()
        .rev()
        .find(|binding| binding.keystroke == stroke)
        .expect("Ctrl+E binding")
        .id;
    let engine = app.engine().control();
    engine.dispatch_event(
        ROOT_ID,
        EventKind::KeyCommand,
        EventPayload::Value(PayloadValue::Number(f64::from(id))),
    );
    let (sent, received) = mpsc::channel();
    engine.flush(move |_| {
        let _ = sent.send(());
    });
    received.recv_timeout(WAIT).expect("apply the shortcut");
    app.catch_up();
}

#[test]
fn a_second_press_mid_slide_retargets_from_the_current_position() {
    let _fixture = Fixture::notes();
    let (mut app, trace) = traced_document(false);
    let width = stage_width(&app);
    let sample = Duration::from_millis(120);

    for (before, intermediate, target) in [(1.0, 0.5, 0.0), (0.5, 0.0, 1.0)] {
        if before == 0.5 {
            step_view(&mut app); // Preview -> Edit -> Split.
            step_view(&mut app);
        }
        assert_stop(&app, before);
        cycle_without_advancing(&mut app, &trace);
        app.wait(sample).expect("advance part of the slide");
        let x1 = divider_x(&app);
        assert!(
            x1 > intermediate * width && x1 < before * width,
            "first slide is in flight: x={x1:.2}, from {} to {} points",
            before * width,
            intermediate * width
        );

        cycle_without_advancing(&mut app, &trace);
        let x2 = divider_x(&app);
        let remaining = target * width - x1;
        let jump = (x2 - x1) * remaining.signum();
        assert!(
            jump >= 0.0 && jump <= remaining.abs() * 0.1,
            "retarget toward {} points must not jump: {x1:.2} -> {x2:.2}",
            target * width
        );
        app.wait(sample).expect("advance the retargeted slide");
        assert!(
            (divider_x(&app) - x2) * remaining.signum() > 0.0,
            "the retargeted slide moves toward its new stop"
        );
        finish_camera(&mut app);
        assert_stop(&app, target);
    }
}

fn stage_subtree(screen: &Screen) -> HashSet<NodeId> {
    let mut ids = HashSet::new();
    // layout_nodes visits each parent before its children.
    for node in screen.layout_nodes() {
        if node.test_id.as_deref() == Some("stage") || ids.contains(&node.parent) {
            ids.insert(node.id);
        }
    }
    ids
}

fn dock_opacity_animation(app: &Headless, trace: &MountTrace) -> u32 {
    let dock = node(app.screen(), "stage-dock").expect("the dock").id;
    trace
        .bindings
        .iter()
        .rev()
        .find_map(|&(id, prop, animation)| {
            (id == dock && prop == AnimationProp::Opacity).then_some(animation)
        })
        .expect("the dock is bound to an opacity value")
}

fn assert_dock_fade(app: &Headless, trace: &Mutex<MountTrace>, transition: &str) {
    let trace = trace.lock().expect("mount trace");
    let node = dock_opacity_animation(app, &trace);
    // Replay the operations the press emitted against the native animator.
    // Start opaque: without an explicit reset on this press, sampling from
    // the dock's settled opacity would never dip below one.
    let mut animator = Animator::default();
    animator.apply(&MountItem::Animation(AnimationOp::CreateValue {
        node,
        value: 1.0,
    }));
    for item in &trace.animation {
        animator.apply(item);
    }
    let duration = f64::from(motion::REDUCED_CROSSFADE_MS) / 1000.0;
    animator.tick(1.0);
    assert_eq!(
        animator.value(node),
        0.0,
        "{transition} resets the displayed dock to transparent"
    );
    animator.tick(1.0 + duration / 2.0);
    let middle = animator.value(node);
    assert!(
        0.0 < middle && middle < 1.0,
        "{transition} fades through an intermediate opacity: {middle}"
    );
    animator.tick(1.0 + duration + 0.01);
    assert_eq!(animator.value(node), 1.0, "{transition} ends opaque");
}

#[test]
fn reduced_motion_fades_the_new_screen_without_sliding_it() {
    let _fixture = Fixture::notes();
    let (mut app, trace) = traced_document(true);
    assert_stop(&app, 1.0);
    let mut ids = stage_subtree(app.screen());
    trace.lock().expect("mount trace").animation.clear();

    cycle_without_advancing(&mut app, &trace);
    assert_stop(&app, 0.5);
    assert_eq!(
        bounds(app.screen(), "stage-source").size.width.0,
        stage_width(&app) * 0.5
    );
    ids.extend(stage_subtree(app.screen()));
    assert_dock_fade(&app, &trace, "Edit to Split");
    trace.lock().expect("mount trace").animation.clear();

    cycle_without_advancing(&mut app, &trace);
    assert_stop(&app, 0.0);
    assert_eq!(bounds(app.screen(), "stage-source").size.width.0, 0.0);
    assert_dock_fade(&app, &trace, "Split to Preview");
    ids.extend(stage_subtree(app.screen()));
    let trace = trace.lock().expect("mount trace");
    let transforms: Vec<_> = trace
        .bindings
        .iter()
        .filter(|(id, prop, _)| ids.contains(id) && matches!(prop, AnimationProp::Transform(_)))
        .collect();
    assert!(
        transforms.is_empty(),
        "the stage subtree has no animated transforms under reduced motion; got {transforms:?}"
    );
}

#[test]
fn reduced_motion_does_not_fade_a_context_dock_that_did_not_change() {
    let _fixture = Fixture::notes();
    let (mut app, trace) = traced_document(true);
    app.press("id:title-file").expect("open Context over Edit");
    app.wait_for("id:context-field")
        .expect("Context occupies the dock");
    trace.lock().expect("mount trace").animation.clear();
    cycle_without_advancing(&mut app, &trace);
    assert_view(&app, SPLIT);
    app.wait_for("id:context-field")
        .expect("the same surface still occupies the dock");
    let trace = trace.lock().expect("mount trace");
    let dock_opacity = dock_opacity_animation(&app, &trace);
    assert!(!trace.animation.iter().any(|item| matches!(item,
        MountItem::Animation(AnimationOp::Start { node, driver: Driver::Timing { to: 1.0, .. }, .. })
            if *node == dock_opacity)), "an unchanged Context dock must not fade");
}

#[test]
fn opening_settings_settles_a_captured_divider_drag() {
    let _fixture = Fixture::notes();
    let mut app = start_in(WIDE);
    let dx = -0.3 * stage_width(&app);
    app.drag_and_hold(DIVIDER, point(px(dx), px(0.0)), DragOptions::default())
        .expect("drag Edit to 0.7");
    assert_stop(&app, 0.7);
    app.press_key("mod+,")
        .expect("open Settings without releasing the pointer");
    app.wait_for("id:settings-page")
        .expect("the stage unmounts");
    app.drop_held()
        .expect("lift the pointer after the stage went away");
    app.press_key("escape").expect("close Settings");
    app.wait_for(DIVIDER).expect("the divider returns");
    finish_camera(&mut app);
    assert_stop(&app, 0.5);
    assert_view(&app, SPLIT);
}

#[test]
fn save_as_keeps_the_tab_switchable_until_the_tree_is_refreshed() {
    let fixture = Fixture::notes();
    let root = fixture.dir.path().join("workspace");
    let destination = root.join("saved.md");
    let release = fixture.dir.path().join("release-refresh");
    let mut app = start_in(WIDE);
    let notes = "id:sidebar-tree >> role:button[notes.md]";
    app.wait_for(notes).expect("the workspace's initial tree");
    mt_app::test_vault::gate_read(&root, &release);
    app.press_key("mod+n").expect("new draft");
    wait_for_title(&mut app, "Untitled 1");
    app.type_text("label:Markdown source", "# New file")
        .expect("edit draft");
    app.on_pick_save_path(Some(destination))
        .expect("save into the workspace");
    app.press_key("mod+shift+s").expect("Save As");
    wait_for_title(&mut app, "saved.md");
    let temporary = "role:button[saved.md]";
    app.wait_for(temporary)
        .expect("the saved tab must have a sidebar row before the tree refresh");
    assert_row(&app, temporary, "sidebar-outside-row");
    app.press(notes)
        .expect("switch away from the newly saved tab");
    app.press(temporary)
        .expect("switch back to the saved tab in one click");
    wait_for_title(&mut app, "saved.md");
    std::fs::write(&release, "go").expect("release the tree refresh");
    app.wait_for("id:sidebar-tree >> role:button[saved.md]")
        .expect("Save As refreshes the tree");
    assert_eq!(
        app.screen().count(temporary).unwrap(),
        1,
        "the tab now has only its tree row"
    );
}

#[test]
fn recovery_and_conflict_actions_remain_visible_in_full_preview() {
    use mt_core::{
        document::io,
        recovery::{RecoveryCheckpoint, RecoveryKey, RecoveryMetadata, RecoveryStore},
    };

    let fixture = Fixture::notes();
    let notes = fixture.dir.path().join("workspace/notes.md");
    if cfg!(windows) {
        let draft = fixture.write("workspace/draft.md", "# On disk\n");
        let loaded = io::load(&draft).expect("load the recovery fixture");
        let (store, _) = RecoveryStore::open().expect("open recovery storage");
        store
            .checkpoint(
                &RecoveryCheckpoint {
                    key: RecoveryKey::for_path(&draft),
                    text: "# From recovery\n".to_owned(),
                    metadata: RecoveryMetadata::from_loaded_file(&loaded),
                    revision: None,
                },
                &HashSet::new(),
            )
            .expect("seed a recovery record");
    }
    let mut app = start_in(WIDE);
    if cfg!(windows) {
        app.wait_for("id:recovery-restore-0")
            .expect("recovery is offered in Edit");
    }
    step_view(&mut app);
    step_view(&mut app);
    assert_view(&app, PREVIEW);
    std::fs::write(&notes, "# Changed externally\n").expect("replace the document on disk");
    app.press_key("mod+s")
        .expect("detect the disk conflict from Preview");
    for viewport in [WIDE, NARROW] {
        app.set_viewport(viewport)
            .expect("resize the visible window");
        app.wait_until("the stage reaches the requested viewport", WAIT, |screen| {
            node(screen, "workspace").is_some_and(|node| node.bounds.size.width == viewport.width)
        })
        .expect("resize workspace");
        let actions = [
            "conflict-compare",
            "conflict-keep-mine",
            "conflict-use-disk",
            "conflict-save-as",
        ];
        let recovery: &[&str] = if cfg!(windows) {
            &["recovery-restore-0", "recovery-discard-0"]
        } else {
            &[]
        };
        for id in actions.into_iter().chain(recovery.iter().copied()) {
            let selector = format!("id:{id}");
            app.wait_for(&selector)
                .expect("the action remains visible in full Preview");
            let screen = app.screen();
            // Count all laid-out nodes, including a clipped source clone
            // that a visible-only selector would miss.
            let layout = screen.layout_nodes();
            let mut mounted = layout
                .iter()
                .filter(|node| node.test_id.as_deref() == Some(id));
            let action = mounted.next().expect("the banner action is mounted");
            assert!(
                mounted.next().is_none(),
                "{id} has a duplicate mount at {} px",
                viewport.width.0
            );
            assert_eq!(
                screen
                    .press_target(&selector)
                    .expect("unscoped action selector"),
                Some(action.id),
                "{id} is pressable in Preview"
            );
        }
    }
    app.press("id:conflict-compare")
        .expect("compare from narrow Preview");
    app.wait_for("id:conflict-previous")
        .expect("the comparison opens");
}

#[test]
fn escape_from_context_focuses_the_live_full_preview() {
    let _fixture = Fixture::notes();
    let mut app = start_in(WIDE);
    step_view(&mut app);
    step_view(&mut app);
    app.press("id:title-file")
        .expect("open Context over Preview");
    app.wait_for("id:context-field")
        .expect("Context occupies the dock");
    app.clear_commands();
    app.press_key("escape").expect("close Context");
    app.wait_until("focus was sent to the live reading view", WAIT, |screen| {
        let Ok(reading) = screen.id("id:stage-dock >> id:preview-column") else {
            return false;
        };
        screen
            .commands()
            .iter()
            .any(|(id, command)| *id == reading && matches!(command, Command::Focus))
    })
    .expect("closing Context returns focus to the mounted Preview");
}

#[test]
fn split_web_keeps_the_same_webview_in_preview_and_on_the_way_out() {
    let _fixture = Fixture::notes();
    let (mut app, trace) = traced_document(false);
    step_view(&mut app);
    app.press("id:title-settings").expect("open the menu");
    app.press("id:title-menu-web")
        .expect("use the Web renderer in Split");
    app.wait_for("id:web-view")
        .expect("the WebView has mounted");
    let web = app.screen().id("id:web-view").unwrap();
    step_view(&mut app);
    assert_view(&app, PREVIEW);
    assert_eq!(
        app.screen().id("id:web-view").unwrap(),
        web,
        "Split to Preview keeps the same WebView"
    );
    cycle_without_advancing(&mut app, &trace);
    app.wait(Duration::from_millis(32))
        .expect("advance into the exit");
    assert!(divider_x(&app) < stage_width(&app));
    assert_eq!(
        app.screen().id("id:web-view").unwrap(),
        web,
        "the outgoing screen keeps the Web renderer"
    );
}

#[test]
fn leaving_narrow_through_settings_discards_the_invisible_sidebar_overlay() {
    let _fixture = Fixture::notes();
    let mut app = start_in(NARROW);
    app.press_key("mod+b").expect("open the narrow overlay");
    app.wait_for("id:sidebar-panel").expect("the overlay opens");
    app.press_key("mod+,").expect("open Settings over it");
    app.wait_for("id:settings-page")
        .expect("Settings replaces the workspace");
    app.set_viewport(WIDE).expect("widen the Settings window");
    app.wait_until("wide Settings window", WAIT, |screen| {
        node(screen, "shell").is_some_and(|node| node.bounds.size.width == WIDE.width)
    })
    .expect("the layout is no longer Narrow");
    app.press_key("escape")
        .expect("close Settings with one Escape");
    app.wait_for_gone("id:settings-page", WAIT)
        .expect("the invisible overlay must not eat Escape");
    app.set_viewport(NARROW).expect("narrow the window again");
    wait_for_sidebar_width(&mut app, COLLAPSED);
    assert!(
        node(app.screen(), "sidebar-panel").is_none(),
        "the old overlay must not reopen"
    );
}

#[test]
fn every_open_tab_is_one_click_away_with_the_sidebar_expanded() {
    let fixture = Fixture::new("external/outside.md", "# Outside\n");
    let inside = fixture.write("workspace/inside.md", "# Inside\n");
    let mut app = start_in(WIDE);
    wait_for_title(&mut app, "outside.md");
    wait_for_sidebar_width(&mut app, tokens::SIDEBAR_W);
    app.on_pick_paths(vec![inside.parent().expect("workspace root").to_path_buf()])
        .expect("choose the workspace in the picker");
    app.press("id:title-settings").expect("open the title menu");
    app.press("id:title-menu-open-folder")
        .expect("open the workspace");
    app.wait_for("id:sidebar-tree >> role:button[inside.md]")
        .expect("the chosen workspace is in the tree");
    app.press_key("mod+n").expect("open an untitled draft");
    wait_for_title(&mut app, "Untitled 1");

    app.wait_for("id:sidebar-outside >> text:Outside folder")
        .expect("the outside folder group");
    for (group, id) in [
        ("sidebar-outside", "sidebar-outside-row"),
        ("sidebar-strip", "rail-file-row"),
    ] {
        if group == "sidebar-strip" {
            app.press_key("mod+b").expect("collapse to the strip");
            wait_for_sidebar_width(&mut app, COLLAPSED);
        }
        for title in ["outside.md", "Untitled 1"] {
            let selector = format!("id:{group} >> role:button[{title}]");
            assert_row(&app, &selector, id);
            assert!(!title_is(app.screen(), title), "{title} is not active yet");
            app.press(&selector)
                .expect("one press switches to the open tab");
            wait_for_title(&mut app, title);
        }
    }
}
