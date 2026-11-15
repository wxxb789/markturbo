//! A clean profile shows the welcome surface, not an empty editor toolbar.

use std::time::Duration;

use nsh_engine::{EngineConfig, Points, Root};
use nsh_testing::Headless;

#[test]
fn welcome_has_one_next_action_and_no_document_toolbar() {
    let config = tempfile::tempdir().expect("config dir");

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

    // The state's one next action, and the quiet choices under it.
    app.wait_for("id:welcome-open-folder")
        .expect("the welcome's next action");
    app.wait_for("text:Open folder")
        .expect("the next action's copy");
    for choice in [
        "id:welcome-new-file",
        "id:welcome-paste",
        "id:welcome-open-sample",
    ] {
        assert_eq!(
            app.screen().count(choice).expect("parse choice selector"),
            1,
            "welcome choice `{choice}`"
        );
    }
    app.wait_for("text:Opening, editing and search stay on this device.")
        .expect("the local-only line");

    // No document is open, so no document surface or document action is on
    // the welcome: no editor, no Save, no pane controls. Review is the
    // document state's own action and appears with a document (below), not
    // before one; with nothing to review, the welcome states the local-only
    // promise instead.
    for chrome in [
        "id:source-editor",
        "id:title-save",
        "id:title-edit",
        "id:title-preview",
        "id:title-split",
        "id:title-context",
    ] {
        assert_eq!(
            app.screen().count(chrome).expect("parse chrome selector"),
            0,
            "`{chrome}` leaked into Welcome"
        );
    }
    assert!(!app.shows("Save"), "document toolbar leaked into Welcome");
    assert!(
        !app.shows("Web preview"),
        "advanced layouts leaked into Welcome"
    );

    // Review becomes available as soon as a document opens — here through
    // the welcome's own New file choice, since a headless run has no folder
    // picker — and the welcome yields to the document surface.
    app.press("label:New file")
        .expect("the welcome's New file choice");
    app.wait_for("id:source-editor")
        .expect("the new document's editor");
    app.wait_for("id:title-review")
        .expect("Review is available once a document is open");
    app.wait_for_gone("id:welcome", Duration::from_secs(5))
        .expect("the welcome yields to the document");
}
