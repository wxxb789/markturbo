//! Cancelling an untitled draft's Save As does not drop the close guard: the
//! card stays up with the localized notice, and Cancel returns the keyboard
//! to the editor (decision 5, and the guard part of the Escape contract).

use std::time::Duration;

use nsh_engine::{EngineConfig, Points, Root};
use nsh_testing::Headless;

const TIMEOUT: Duration = Duration::from_secs(20);

#[test]
fn cancelling_an_untitled_save_as_keeps_the_close_guard_open() {
    let config = tempfile::tempdir().expect("config dir");

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

    let mut app =
        Headless::start(EngineConfig::new(Root::of::<mt_app::App>())).expect("start the app");
    app.set_viewport(nsh_core::Size::new(Points(1280.0), Points(800.0)))
        .expect("wide viewport");
    app.wait_for("id:welcome").expect("welcome");
    app.press("label:New file").expect("new draft");
    app.wait_for("label:Markdown source")
        .expect("the draft editor");
    app.type_text("label:Markdown source", "Draft")
        .expect("edit the draft");
    app.wait_for("id:title-dirty").expect("the dirty mark");

    // Ctrl+W raises the close guard; its Save goes through the picker, which
    // the test cancels.
    app.type_text("label:Markdown source", "")
        .expect("focus the editor for Ctrl+W");
    app.press_key("mod+w").expect("press Ctrl+W");
    app.wait_for("id:guard-save").expect("the close guard");

    app.on_pick_save_path(None).expect("cancel the picker");
    app.press("id:guard-card >> label:Save")
        .expect("save through the picker");
    app.wait_for_within("text:Save cancelled — the document stays open.", TIMEOUT)
        .expect("the cancelled Save As keeps the guard, with its notice");
    assert!(
        app.screen().matches("id:guard-save").expect("selector"),
        "the guard stays open after the cancelled picker"
    );
    assert!(
        app.screen()
            .matches("id:source-editor >> text:/Draft/")
            .expect("selector"),
        "the draft is untouched: {:?}",
        app.screen().texts()
    );

    // Escape cancels the guard and returns the keyboard to the editor: the
    // next plain key types into the document.
    app.press_key("escape").expect("cancel the guard");
    app.wait_for_gone("id:guard-card", Duration::from_secs(5))
        .expect("the guard goes");
    app.press_key("x").expect("type without clicking");
    app.wait_for("id:source-editor >> text:/Draftx/")
        .expect("the keystroke landed in the editor");
}
