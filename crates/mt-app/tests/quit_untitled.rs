//! Quit Save all must route untitled drafts through Save As.

use nsh_engine::{EngineConfig, Points, Root};
use nsh_testing::{DialogAnswer, Headless};

#[test]
fn quit_save_all_saves_an_untitled_draft() {
    let dir = tempfile::tempdir().expect("temp dir");
    let config = tempfile::tempdir().expect("config dir");
    let destination = dir.path().join("quit-draft.md");

    // SAFETY: this binary has one test.
    unsafe {
        std::env::remove_var("MARKTURBO_OPEN");
        std::env::set_var("MARKTURBO_CONFIG_DIR", config.path());
        std::env::set_var("MARKTURBO_DATA_DIR", config.path().join("data"));
    }

    let mut app =
        Headless::start(EngineConfig::new(Root::of::<mt_app::App>())).expect("start the app");
    app.set_viewport(nsh_core::Size::new(Points(1280.0), Points(800.0)))
        .expect("wide viewport");
    app.wait_for("text:Start a Markdown document")
        .expect("welcome");
    app.press("text:New").expect("new draft");
    app.wait_for("label:Markdown source").expect("draft editor");
    app.type_text("label:Markdown source", "Quit draft")
        .expect("edit draft");

    app.on_pick_save_path(Some(destination.clone()))
        .expect("register save path");
    app.on_dialog(|_| DialogAnswer::Press("Save all".to_owned()));
    app.press("text:Quit").expect("quit");

    // Quit shows its decision from a task and writes the draft from another:
    // pump the app's frames and answer the dialog instead of sleeping in this
    // thread, which never advances the app.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
    while !destination.exists() {
        if app.pending_dialog().is_some() {
            app.answer_dialog(DialogAnswer::Press("Save all".to_owned()))
                .expect("answer the quit dialog");
            continue;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "quit did not save the untitled draft: {:?}",
            app.screen().texts()
        );
        app.next_change(std::time::Duration::from_millis(25))
            .expect("settle while quitting");
    }
    assert_eq!(
        std::fs::read_to_string(&destination).expect("read saved draft"),
        "Quit draft"
    );
}
