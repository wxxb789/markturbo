//! The Selection scope is the source editor's selection as it is on screen
//! when the Review is prepared: a selection a remounted editor no longer
//! shows is not reviewed, and a selection the text moved under is reviewed
//! where the editor shows it, never at its old line and column.

use std::time::Duration;

use nsh_engine::{EngineConfig, Points, Root};
use nsh_testing::Headless;

const DOCUMENT: &str = "# Plan\n\nIntro OUTSIDE-BEFORE-SENTINEL.\n\nSELECTED-SENTINEL keep the target words.\n\nAfter OUTSIDE-AFTER-SENTINEL.\n";
const SELECTED: &str = "SELECTED-SENTINEL keep the target words.";
/// The document as it is on disk after an external write: two lines above
/// the selection, so its old line and column now point at the intro line.
const RELOADED: &str = "# Plan\n\nINSERTED-LINE-SENTINEL.\n\nIntro OUTSIDE-BEFORE-SENTINEL.\n\nSELECTED-SENTINEL keep the target words.\n\nAfter OUTSIDE-AFTER-SENTINEL.\n";
const NO_SELECTION: &str = "Select text in the source to review a selection.";

/// Select the document's fifth line (from 0: line 4) in the source editor.
fn select_line_five(app: &mut Headless) {
    app.type_text("label:Markdown source", "")
        .expect("focus the editor");
    app.press_key("ctrl+home").expect("to the start");
    for _ in 0..4 {
        app.press_key("down").expect("down a line");
    }
    app.press_key("shift+end").expect("select the line");
}

fn menu(app: &mut Headless, row: &str) {
    app.press("id:title-settings").expect("open the menu");
    app.press(&format!("id:{row}")).expect("the menu row");
}

/// `Review selection` prepares nothing and says so; a plain Review offers no
/// Selection scope.
fn assert_no_selection_review(app: &mut Headless, after: &str) {
    menu(app, "title-menu-review-selection");
    app.wait_for(&format!("text:{NO_SELECTION}"))
        .unwrap_or_else(|_| panic!("after {after}, Review selection asks for a selection"));
    assert!(
        !app.screen()
            .matches("id:consent-card")
            .expect("the selector"),
        "after {after}, no consent card for a selection nobody sees"
    );
    app.press("id:title-review").expect("open Review");
    app.wait_for("id:consent-card")
        .expect("document disclosure");
    assert!(
        app.wait_for_gone("id:consent-scope-selection", Duration::from_secs(1))
            .is_ok(),
        "after {after}, the card offers no Selection scope"
    );
    assert!(
        !app.shows("selected text only"),
        "after {after}, nothing says only a selection leaves"
    );
    app.press("id:consent-cancel-button").expect("cancel");
    app.wait_for_gone("id:consent-card", Duration::from_secs(2))
        .expect("the card closed");
}

#[test]
fn a_selection_the_editor_no_longer_shows_is_not_reviewed() {
    let dir = tempfile::tempdir().expect("temp dir");
    let config = tempfile::tempdir().expect("config dir");
    let document = dir.path().join("plan.md");
    let other = dir.path().join("other.md");
    std::fs::write(&document, DOCUMENT).expect("write fixture");
    std::fs::write(&other, "# Other heading\n").expect("write the other file");
    // Nothing is sent in this test: the endpoint is only configured.
    let base_url = "http://127.0.0.1:9/v1/";
    let endpoint = mt_core::model::EndpointIdentity::parse(
        mt_core::model::Provider::OpenAiChat,
        Some(base_url),
    )
    .expect("endpoint");
    let identity = endpoint.credential_target().to_string();
    std::fs::write(
        config.path().join("settings.toml"),
        format!(
            "model-provider = \"openai-chat\"\nmodel-name = \"review-model\"\nmodel-base-url = \"{base_url}\"\nmodel-environment-key-identity = \"{identity}\"\n"
        ),
    )
    .expect("write settings");

    // SAFETY: this binary has one test.
    unsafe {
        std::env::set_var("MARKTURBO_OPEN", &document);
        std::env::set_var("MARKTURBO_CONFIG_DIR", config.path());
        std::env::set_var("MARKTURBO_DATA_DIR", config.path().join("data"));
        std::env::set_var(
            "MARKTURBO_TEST_LOCK_NAMESPACE",
            std::process::id().to_string(),
        );
        std::env::set_var("OPENAI_API_KEY", "loopback-test-key");
    }

    let mut app =
        Headless::start(EngineConfig::new(Root::of::<mt_app::App>())).expect("start the app");
    app.set_viewport(nsh_core::Size::new(Points(1280.0), Points(800.0)))
        .expect("wide viewport");
    app.wait_for("label:Markdown source")
        .expect("source editor");

    // Source -> Preview -> Source remounts the editor with a fresh caret.
    select_line_five(&mut app);
    menu(&mut app, "title-menu-preview");
    app.wait_for_gone("label:Markdown source", Duration::from_secs(5))
        .expect("the preview took the column");
    // In Preview there is no source editor, so there is no selection.
    assert_no_selection_review(&mut app, "the Preview pane");
    menu(&mut app, "title-menu-edit");
    app.wait_for("label:Markdown source")
        .expect("the source editor again");
    assert_no_selection_review(&mut app, "a pane round trip");

    // Another tab and back remounts the editor too.
    select_line_five(&mut app);
    app.press_key("mod+p").expect("open the palette");
    app.wait_for("id:palette-input").expect("the palette");
    app.type_text("id:palette-input", "other")
        .expect("filter to other.md");
    app.submit("id:palette-input").expect("open other.md");
    app.wait_for("text:/Other heading/").expect("the other tab");
    app.press_key("mod+p").expect("open the palette");
    app.wait_for("id:palette-input").expect("the palette");
    app.type_text("id:palette-input", "plan")
        .expect("filter to plan.md");
    app.submit("id:palette-input").expect("open plan.md");
    app.wait_for("text:/SELECTED-SENTINEL/")
        .expect("back on the plan");
    app.wait_for("label:Markdown source")
        .expect("the plan's source editor");
    assert_no_selection_review(&mut app, "a tab round trip");

    // The text changes under a live selection (Reload from disk): the
    // selection is reviewed where the editor shows it, not at its old line.
    select_line_five(&mut app);
    std::fs::write(&document, RELOADED).expect("the external write");
    menu(&mut app, "title-menu-save");
    app.wait_for("id:conflict-use-disk")
        .expect("the conflict's use-disk choice");
    app.press("label:Use disk version").expect("use the disk");
    app.wait_for("id:guard-reload").expect("the reload guard");
    app.press("id:guard-reload").expect("confirm the reload");
    app.wait_for("text:/INSERTED-LINE-SENTINEL/")
        .expect("the reloaded text");
    menu(&mut app, "title-menu-review-selection");
    app.wait_for("id:consent-scope-selection")
        .expect("the moved selection is still a selection");
    let intro = "Intro OUTSIDE-BEFORE-SENTINEL.";
    assert!(
        app.shows(&format!("{} B", SELECTED.len())),
        "exactly the selected line's bytes, where the editor moved it"
    );
    assert!(
        !app.shows(&format!("{} B", intro.len())),
        "not the line that now sits at the selection's old place"
    );
}
