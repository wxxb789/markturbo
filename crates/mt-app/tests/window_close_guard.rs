//! The window's own close (its close button, Alt+F4) goes through the same
//! 'Close without saving?' guard as Quit.
//!
//! A clean window closes at once; a dirty one raises the guard card, whose
//! Cancel keeps the window and the text, Don't save closes without writing,
//! and Save writes before closing. A save that fails or whose picker is
//! cancelled keeps the window open.
//!
//! The scenarios run one after another in this binary's only test: each sets
//! the process environment the app reads at startup.

mod support;

use std::path::Path;
use std::time::Duration;

use nsh_core::platform::{Idiom, Os};
use nsh_engine::{EngineConfig, Points, Root};
use nsh_testing::Headless;

const TIMEOUT: Duration = Duration::from_secs(20);

/// Starts the app as the Windows desktop runs it (so the main window exists),
/// opening `open` when given.
fn start(open: Option<&Path>, config: &Path) -> Headless {
    // SAFETY: this binary has one test, and it starts one app at a time.
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
    engine.environment.os = Os::Windows;
    engine.environment.idiom = Idiom::Desktop;
    let mut app = Headless::start(engine).expect("start the app");
    app.set_viewport(nsh_core::Size::new(Points(1280.0), Points(800.0)))
        .expect("wide viewport");
    app
}

fn visible(app: &mut Headless) -> bool {
    app.window_state("main")
        .expect("the desktop has a main window")
        .is_some_and(|state| state.visible)
}

fn editor_text(app: &mut Headless) -> String {
    app.screen()
        .input_text("label:Markdown source")
        .expect("read editor text")
        .unwrap_or_default()
        .to_owned()
}

fn dirty_file(dir: &Path, config: &Path, name: &str) -> (Headless, std::path::PathBuf) {
    let path = dir.join(name);
    std::fs::write(&path, "# Notes\n").expect("write fixture");
    let mut app = start(Some(&path), config);
    app.wait_for("label:Markdown source")
        .expect("source editor");
    app.type_text("label:Markdown source", "\nUnsaved edit.")
        .expect("edit the buffer");
    app.wait_for("id:title-dirty").expect("dirty mark");
    (app, path)
}

#[test]
fn the_window_close_button_is_guarded_like_quit() {
    let dir = tempfile::tempdir().expect("temp dir");
    let config = tempfile::tempdir().expect("config dir");

    // A clean window closes at once, with no guard.
    {
        let path = dir.path().join("clean.md");
        std::fs::write(&path, "# Clean\n").expect("write fixture");
        let mut app = start(Some(&path), config.path());
        app.wait_for("label:Markdown source")
            .expect("source editor");
        assert!(visible(&mut app), "the window starts visible");
        app.press_window_close("main").expect("press close");
        app.settle();
        assert!(!visible(&mut app), "a clean window closes at once");
    }

    // Dirty: the guard comes up and the window stays; Cancel keeps the text;
    // Don't save closes without writing.
    {
        let (mut app, path) = dirty_file(dir.path(), config.path(), "cancel.md");
        app.press_window_close("main").expect("press close");
        app.wait_for("id:guard-cancel")
            .expect("the close button raises the guard");
        assert!(visible(&mut app), "the guard keeps the window open");
        app.press("id:guard-card >> label:Cancel")
            .expect("cancel the close");
        app.wait_for_gone("id:guard-cancel", Duration::from_secs(5))
            .expect("the guard goes");
        app.settle();
        assert!(visible(&mut app), "Cancel keeps the window open");
        assert!(editor_text(&mut app).contains("Unsaved edit."));
        app.wait_for("id:title-dirty")
            .expect("the buffer is still dirty");

        app.press_window_close("main").expect("press close again");
        app.wait_for("id:guard-dont-save").expect("the guard again");
        app.press("id:guard-card >> label:Don't save")
            .expect("discard");
        app.settle();
        assert!(!visible(&mut app), "Don't save closes the window");
        assert_eq!(
            std::fs::read_to_string(&path).expect("read file"),
            "# Notes\n",
            "Don't save writes nothing"
        );
    }

    // Dirty: Save writes the file, then the window closes.
    {
        let (mut app, path) = dirty_file(dir.path(), config.path(), "save.md");
        app.press_window_close("main").expect("press close");
        app.wait_for("id:guard-save").expect("the guard");
        app.press("id:guard-card >> label:Save")
            .expect("save and close");
        support::wait_for_file(
            &mut app,
            &path,
            |text| text.contains("Unsaved edit."),
            "Save never wrote the buffer",
            TIMEOUT,
        );
        let deadline = std::time::Instant::now() + TIMEOUT;
        while visible(&mut app) {
            assert!(
                std::time::Instant::now() < deadline,
                "Save wrote the file but the window stayed open"
            );
            app.next_change(Duration::from_millis(25)).expect("settle");
        }
    }

    // Dirty, and the file changed on disk: the save is refused as a conflict,
    // so the window stays open with the text.
    {
        let (mut app, path) = dirty_file(dir.path(), config.path(), "conflict.md");
        std::fs::write(&path, "# Changed elsewhere\n").expect("external edit");
        app.press_window_close("main").expect("press close");
        app.wait_for("id:guard-save").expect("the guard");
        app.press("id:guard-card >> label:Save")
            .expect("try to save");
        app.wait_for("text:Could not save this document, so it stays open. Check file permissions or try Save As….")
            .expect("the guard says the save failed");
        app.settle();
        assert!(visible(&mut app), "a failed save keeps the window open");
        assert!(editor_text(&mut app).contains("Unsaved edit."));
        assert_eq!(
            std::fs::read_to_string(&path).expect("read file"),
            "# Changed elsewhere\n",
            "the other writer's text is not overwritten"
        );
    }

    // Untitled and dirty: a cancelled Save As picker keeps the window open;
    // a chosen path saves the draft, then the window closes.
    {
        let destination = dir.path().join("draft.md");
        let mut app = start(None, config.path());
        app.wait_for("id:welcome").expect("welcome");
        app.press("label:New file").expect("new draft");
        app.wait_for("label:Markdown source").expect("draft editor");
        app.type_text("label:Markdown source", "Close draft")
            .expect("edit draft");
        app.wait_for("id:title-dirty").expect("dirty mark");

        app.on_pick_save_path(None).expect("cancel the picker");
        app.press_window_close("main").expect("press close");
        app.wait_for("id:guard-save").expect("the guard");
        app.press("id:guard-card >> label:Save")
            .expect("save through the picker");
        app.wait_for("text:Save cancelled — the document stays open.")
            .expect("a cancelled picker leaves the guard up");
        assert!(visible(&mut app), "a cancelled picker keeps the window");
        assert!(!destination.exists());
        app.press("id:guard-card >> label:Cancel")
            .expect("cancel the close");
        app.wait_for_gone("id:guard-cancel", Duration::from_secs(5))
            .expect("the guard goes");
        assert_eq!(editor_text(&mut app), "Close draft");

        app.on_pick_save_path(Some(destination.clone()))
            .expect("register save path");
        app.press_window_close("main").expect("press close again");
        app.wait_for("id:guard-save").expect("the guard again");
        app.press("id:guard-card >> label:Save")
            .expect("save through the picker");
        support::wait_for_file(
            &mut app,
            &destination,
            |text| text == "Close draft",
            "the close guard never saved the draft",
            TIMEOUT,
        );
        let deadline = std::time::Instant::now() + TIMEOUT;
        while visible(&mut app) {
            assert!(
                std::time::Instant::now() < deadline,
                "the draft was saved but the window stayed open"
            );
            app.next_change(Duration::from_millis(25)).expect("settle");
        }
    }
}
