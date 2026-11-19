//! A document opens in Source; the pane the user picks is kept for the
//! documents opened after it.

use std::time::Duration;

use nsh_engine::{EngineConfig, Points, Root};
use nsh_testing::Headless;

#[test]
fn documents_open_in_source_then_in_the_pane_last_chosen() {
    let dir = tempfile::tempdir().expect("temp dir");
    let config = tempfile::tempdir().expect("config dir");
    let first = dir.path().join("first.md");
    let second = dir.path().join("second.md");
    std::fs::write(&first, "# First heading\n").expect("write first");
    std::fs::write(&second, "# Second heading\n").expect("write second");

    // SAFETY: this test binary has one test, so no other test reads the
    // environment concurrently.
    unsafe {
        std::env::set_var("MARKTURBO_OPEN", &first);
        std::env::set_var("MARKTURBO_CONFIG_DIR", config.path());
        std::env::set_var("MARKTURBO_DATA_DIR", config.path().join("data"));
    }

    let mut app =
        Headless::start(EngineConfig::new(Root::of::<mt_app::App>())).expect("start the app");
    app.set_viewport(nsh_core::Size::new(Points(1440.0), Points(900.0)))
        .expect("wide viewport");

    // Source: the editor alone, no preview in the field.
    app.wait_for("label:Markdown source")
        .expect("the source editor");
    app.settle();
    assert!(
        !app.screen().matches("id:preview-field").expect("selector"),
        "the document opened with a preview beside it"
    );

    // The user picks Split.
    app.press("id:title-settings")
        .expect("open the glyph's menu");
    app.press("id:title-menu-split").expect("choose Split");
    app.wait_for("id:preview-field")
        .expect("Split: the preview in the field");

    // The next document opens in Split too.
    app.press_key("mod+p").expect("open the palette");
    app.wait_for("id:palette-input").expect("the palette");
    app.type_text("id:palette-input", "second")
        .expect("filter to second.md");
    app.submit("id:palette-input").expect("open second.md");
    app.wait_for_gone(
        "id:preview-field >> text:First heading",
        Duration::from_secs(5),
    )
    .expect("the first document left the field");
    app.wait_for("id:preview-field >> text:Second heading")
        .expect("the second document opens in the pane last chosen");
    app.wait_for("label:Markdown source")
        .expect("Split keeps the editor");
}
