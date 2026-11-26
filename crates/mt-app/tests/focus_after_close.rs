//! A close or a tab switch mounts the next tab's editor a commit later; the
//! keyboard focus must land on that editor, proven by where the next key
//! types, never on the document container in between.

use std::{sync::Mutex, time::Duration};

use mt_app::state::AppState;
use nsh_core::Command;
use nsh_engine::{EngineConfig, Points, Root};
use nsh_testing::Headless;

const TIMEOUT: Duration = Duration::from_secs(20);

static TEST_LOCK: Mutex<()> = Mutex::new(());

fn editor_text(app: &Headless) -> String {
    app.screen()
        .input_text("label:Markdown source")
        .expect("read editor text")
        .unwrap_or_default()
        .to_owned()
}

/// Whether the first focus the app gave after the command history was
/// cleared went to the mounted source editor: a focus that reached the
/// document container or anything else first would swallow the next key.
fn editor_focused_first(app: &Headless) -> bool {
    let Ok(editor) = app.screen().id("label:Markdown source") else {
        return false;
    };
    app.screen()
        .commands()
        .iter()
        .find(|(_, command)| matches!(command, Command::Focus))
        .is_some_and(|(id, _)| *id == editor)
}

/// Waits until the app's focus command history shows the mounted source
/// editor taking the keyboard before any other view, within the app's own
/// focus-retry window of the first screen the editor is on: a close or a
/// rail switch mounts the next editor a commit later and retries the focus
/// for [`AppState::FOCUS_RETRY_FRAMES`] frames, so a focus after that window
/// is no retry at all.
fn wait_for_editor_focus(app: &mut Headless) {
    app.wait_for("label:Markdown source")
        .expect("the mounted source editor");
    for _ in 0..AppState::FOCUS_RETRY_FRAMES {
        if editor_focused_first(app) {
            return;
        }
        app.next_change(Duration::from_millis(25))
            .expect("wait for the editor focus");
    }
    assert!(
        editor_focused_first(app),
        "the mounted editor took longer than {} frames to take the focus: {:?}",
        AppState::FOCUS_RETRY_FRAMES,
        app.screen().texts()
    );
}

#[test]
fn focus_lands_on_the_editor_the_next_tab_mounts() {
    let _guard = TEST_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    let dir = tempfile::tempdir().expect("temp dir");
    let config = tempfile::tempdir().expect("config dir");
    let first = dir.path().join("first.md");
    let second = dir.path().join("second.md");
    let third = dir.path().join("third.md");
    std::fs::write(&first, "# First heading\n").expect("write first");
    std::fs::write(&second, "# Second heading\n").expect("write second");
    std::fs::write(&third, "# Third heading\n").expect("write third");

    // SAFETY: TEST_LOCK serializes app lifetimes and environment changes.
    unsafe {
        std::env::set_var("MARKTURBO_OPEN", &first);
        std::env::set_var("MARKTURBO_CONFIG_DIR", config.path());
        std::env::set_var("MARKTURBO_DATA_DIR", config.path().join("data"));
        std::env::set_var(
            "MARKTURBO_TEST_LOCK_NAMESPACE",
            std::process::id().to_string(),
        );
    }

    let mut app =
        Headless::start(EngineConfig::new(Root::of::<mt_app::App>())).expect("start the app");
    app.set_viewport(nsh_core::Size::new(Points(1440.0), Points(900.0)))
        .expect("wide viewport");
    app.wait_for("text:# First heading")
        .expect("first document");
    app.wait_for("label:Markdown source")
        .expect("first source editor");

    let open = |app: &mut Headless, name: &str, heading: &str| {
        app.press_key("mod+p").expect("open the palette");
        app.wait_for("id:palette-input").expect("the palette");
        app.type_text("id:palette-input", name)
            .expect("filter the palette");
        app.submit("id:palette-input").expect("open the file");
        app.wait_for(heading).expect("the opened document");
    };
    open(&mut app, "second", "text:# Second heading");

    // Dirty the second tab, close it through the guard's Don't save chosen
    // with the keyboard (Left off Cancel, Enter), and type once the fallback
    // tab's editor has the focus: the key can only land there, not on the
    // document container it mounts inside. The guard closes on the Enter
    // binding, whose owner dies with the card; the focus retry must outlive
    // it.
    app.type_text("label:Markdown source", "\nDirty line")
        .expect("dirty the second tab");
    app.press_key("mod+w").expect("close the dirty tab");
    app.wait_for("id:guard-dont-save").expect("the close guard");
    app.press_key("left").expect("select Don't save");
    app.clear_commands();
    app.press_key("enter").expect("confirm with the keyboard");
    app.wait_for_gone("id:guard-card", TIMEOUT)
        .expect("the guard closes");
    app.wait_for("text:# First heading")
        .expect("the fallback document");
    wait_for_editor_focus(&mut app);
    app.press_key("z").expect("type after the close");
    assert!(
        editor_text(&app).contains('z'),
        "the close left the keyboard off the fallback editor: {:?}",
        editor_text(&app)
    );

    // The same path through a rail switch: first.md's dot rebuilds its
    // editor, and the next key must land there.
    open(&mut app, "third", "text:# Third heading");
    app.clear_commands();
    app.press("label:first.md").expect("switch to first.md");
    app.wait_for("id:title-file >> text:first.md")
        .expect("first.md is active again");
    wait_for_editor_focus(&mut app);
    app.press_key("y").expect("type after the switch");
    assert!(
        editor_text(&app).contains('y'),
        "the rail switch left the keyboard off the new editor: {:?}",
        editor_text(&app)
    );
}

#[test]
fn ctrl_n_after_escape_focuses_the_new_editor() {
    let _guard = TEST_LOCK.lock().unwrap_or_else(|error| error.into_inner());
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

    let mut app =
        Headless::start(EngineConfig::new(Root::of::<mt_app::App>())).expect("start the app");
    app.set_viewport(nsh_core::Size::new(Points(1280.0), Points(800.0)))
        .expect("wide viewport");
    app.wait_for("id:welcome").expect("the welcome page");
    app.press("id:welcome-new-file").expect("new document");
    app.wait_for("label:Markdown source").expect("the editor");
    app.type_text("label:Markdown source", "seed")
        .expect("type into the draft");

    // Escape blurs the editor; the next new tab must still take the
    // keyboard when it mounts.
    app.press_key("escape").expect("escape");
    app.press_key("mod+n").expect("Ctrl+N");
    app.wait_until("a second tab", TIMEOUT, |screen| {
        screen.count("id:rail-file-row").unwrap_or(0) == 2
    })
    .expect("Ctrl+N starts another document");
    app.press_key("q").expect("type into the new document");
    assert!(
        editor_text(&app).contains('q'),
        "Ctrl+N left the keyboard off the new editor: {:?}",
        editor_text(&app)
    );
}
