//! Save As over an existing file, after the native save dialog's own
//! overwrite confirmation, writes the file (atomically, as Save does),
//! moves the tab to it and clears its dirty mark. A destination open in
//! another tab is refused and left untouched, so two tabs never edit one file
//! and unsaved text in the other tab is never orphaned.

mod support;

use std::time::Duration;

use nsh_engine::{EngineConfig, Points, Root};
use nsh_testing::Headless;

const TIMEOUT: Duration = Duration::from_secs(20);

#[test]
fn save_as_replaces_a_confirmed_existing_destination() {
    let dir = tempfile::tempdir().expect("temp dir");
    let config = tempfile::tempdir().expect("config dir");
    let first = dir.path().join("first.md");
    let second = dir.path().join("second.md");
    let existing = dir.path().join("existing.md");
    std::fs::write(&first, "# First heading\n").expect("write first");
    std::fs::write(&second, "# Second heading\n").expect("write second");
    std::fs::write(&existing, "old bytes\n").expect("write existing");

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
    app.wait_for("label:Markdown source")
        .expect("first source editor");
    app.type_text("label:Markdown source", "Unsaved first. ")
        .expect("edit first");
    app.wait_for("id:title-dirty").expect("first is dirty");

    // The edited first tab keeps its place; second.md opens beside it.
    app.press_key("mod+p").expect("open the palette");
    app.wait_for("id:palette-input").expect("the palette");
    app.type_text("id:palette-input", "second")
        .expect("filter to second.md");
    app.submit("id:palette-input").expect("open second.md");
    app.wait_for("text:Second heading")
        .expect("second document");
    app.type_text("label:Markdown source", "Edited second. ")
        .expect("edit second");
    app.wait_for("id:title-dirty").expect("second is dirty");

    // The destination is open in another tab with unsaved text: refused.
    app.on_pick_save_path(Some(first.clone()))
        .expect("pick first.md");
    app.press("id:title-settings").expect("open the menu");
    app.press("id:title-menu-save-as").expect("Save As");
    app.wait_for("text:/is already open/")
        .expect("Save As onto an open document is refused");
    assert_eq!(
        std::fs::read_to_string(&first).expect("read first"),
        "# First heading\n",
        "the open destination is not touched"
    );

    // An existing file the dialog confirmed: replaced with the buffer, and
    // the tab now names it, clean.
    app.on_pick_save_path(Some(existing.clone()))
        .expect("pick existing.md");
    app.press("id:title-settings").expect("open the menu again");
    app.press("id:title-menu-save-as").expect("Save As again");
    support::wait_for_file(
        &mut app,
        &existing,
        |text| text == "Edited second. # Second heading\n",
        "Save As never replaced the confirmed destination",
        TIMEOUT,
    );
    app.wait_for("text:existing.md")
        .expect("the tab names the new file");
    app.wait_for_gone("id:title-dirty", Duration::from_secs(5))
        .expect("the saved tab is clean");
    assert_eq!(
        std::fs::read_to_string(&second).expect("read second"),
        "# Second heading\n",
        "Save As leaves the old file alone"
    );
    assert_eq!(
        std::fs::read_to_string(&first).expect("read first"),
        "# First heading\n"
    );
}
