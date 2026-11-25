//! The palette with no folder names the next step instead of pretending a
//! search ran, and Escape returns the keyboard to the editor.

use std::time::Duration;

use nsh_engine::{EngineConfig, Points, Root};
use nsh_testing::Headless;

const HINT: &str = "Open a folder first to search its files.";

#[test]
fn the_palette_without_a_folder_says_so_and_escape_returns_focus() {
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

    // The file list says there is no folder to search.
    app.press_key("mod+p").expect("open the palette");
    app.wait_for("id:palette-input").expect("the palette");
    app.wait_for(&format!("text:{HINT}"))
        .expect("the palette names the missing folder");
    app.press_key("escape").expect("close the palette");
    app.wait_for_gone("id:palette", Duration::from_secs(5))
        .expect("the palette closes");

    // A submitted search with no folder never marks itself as run: the line
    // still names the missing folder rather than `No matches.`.
    app.press_key("shift+mod+f").expect("open the search");
    app.wait_for("id:palette-input").expect("the search line");
    app.type_text("id:palette-input", "needle")
        .expect("type the query");
    app.submit("id:palette-input").expect("run the search");
    app.wait_for(&format!("text:{HINT}"))
        .expect("the search line still names the missing folder");
    assert!(
        !app.shows("No matches."),
        "an unrun search must not read as an empty result set: {:?}",
        app.screen().texts()
    );
    app.press_key("escape").expect("close the search");
    app.wait_for_gone("id:palette", Duration::from_secs(5))
        .expect("the search closes");

    // With a document open, closing the palette returns the keyboard to the
    // editor: the next plain key types into the document.
    app.press("label:New file").expect("new draft");
    app.wait_for("label:Markdown source")
        .expect("the draft editor");
    app.type_text("label:Markdown source", "body")
        .expect("edit the draft");
    app.press_key("mod+p").expect("open the palette again");
    app.wait_for("id:palette-input").expect("the palette");
    app.press_key("escape").expect("close the palette");
    app.wait_for_gone("id:palette", Duration::from_secs(5))
        .expect("the palette closes");
    app.press_key("x").expect("type without clicking");
    app.wait_until(
        "the keystroke to land in the editor",
        Duration::from_secs(5),
        |screen| {
            screen
                .texts()
                .iter()
                .any(|text| *text == "bodyx" || *text == "xbody")
        },
    )
    .expect("the keystroke landed in the editor");
}
