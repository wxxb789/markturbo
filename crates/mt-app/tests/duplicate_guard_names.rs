//! Two open files with the same name are told apart in the quit guard by
//! their parent folders.

use std::time::Duration;

use nsh_engine::{EngineConfig, Points, Root};
use nsh_testing::Headless;

#[test]
fn shared_open_filenames_carry_their_parent_folders_in_the_guard() {
    let workspace = tempfile::tempdir().expect("workspace");
    let config = tempfile::tempdir().expect("config dir");
    let drafts = workspace.path().join("drafts");
    let archive = workspace.path().join("archive");
    std::fs::create_dir(&drafts).expect("drafts dir");
    std::fs::create_dir(&archive).expect("archive dir");
    std::fs::write(drafts.join("plan.md"), "# Drafts plan\n").expect("write drafts");
    std::fs::write(archive.join("plan.md"), "# Archive plan\n").expect("write archive");

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
    app.wait_for("id:welcome-open-folder")
        .expect("the welcome's next action");
    app.on_pick_paths(vec![workspace.path().to_path_buf()])
        .expect("answer the folder picker");
    app.press("id:welcome-open-folder").expect("open a folder");
    app.wait_for("id:workspace-home")
        .expect("the workspace in the column");

    // Open and dirty both same-named documents, one from each folder.
    for (needle, heading) in [("drafts", "# Drafts plan"), ("archive", "# Archive plan")] {
        app.press_key("mod+p").expect("open the palette");
        app.wait_for("id:palette-input").expect("the palette");
        app.type_text("id:palette-input", needle).expect("filter");
        app.submit("id:palette-input").expect("open the file");
        app.wait_for(&format!("id:source-editor >> text:{heading}"))
            .expect("the document's editor");
        app.type_text("label:Markdown source", " edited")
            .expect("dirty the document");
    }

    app.press("id:title-settings").expect("open the title menu");
    app.press("id:title-menu-quit").expect("request quit");
    app.wait_for("id:guard-card").expect("the quit guard");
    app.wait_for_within(
        "text:drafts/plan.md, archive/plan.md",
        Duration::from_secs(20),
    )
    .expect("the guard names both files by their folders");
}
