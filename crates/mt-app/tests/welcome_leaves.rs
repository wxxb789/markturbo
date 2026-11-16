//! Opening a workspace replaces the welcome: the folder picker's choice, Open
//! sample, and a directory given on the command line (`MARKTURBO_OPEN`) each
//! leave the column showing the workspace, whose files open from it.

use std::{path::Path, time::Duration};

use nsh_engine::{EngineConfig, Points, Root};
use nsh_testing::Headless;

const GONE: Duration = Duration::from_secs(5);

fn start(config: &Path, open: Option<&Path>) -> Headless {
    // SAFETY: this binary has one test, which starts its apps one at a time.
    unsafe {
        match open {
            Some(path) => std::env::set_var("MARKTURBO_OPEN", path),
            None => std::env::remove_var("MARKTURBO_OPEN"),
        }
        std::env::set_var("MARKTURBO_CONFIG_DIR", config);
        std::env::set_var("MARKTURBO_DATA_DIR", config.join("data"));
    }
    let mut app =
        Headless::start(EngineConfig::new(Root::of::<mt_app::App>())).expect("start the app");
    app.set_viewport(nsh_core::Size::new(Points(1280.0), Points(800.0)))
        .expect("wide viewport");
    app
}

#[test]
fn opening_a_workspace_replaces_the_welcome() {
    let workspace = tempfile::tempdir().expect("workspace");
    std::fs::write(workspace.path().join("readme.md"), "# Readme\n").expect("write a document");

    // The folder picker's choice.
    let config = tempfile::tempdir().expect("config dir");
    let mut app = start(config.path(), None);
    app.wait_for("id:welcome-open-folder")
        .expect("the welcome's next action");
    app.on_pick_paths(vec![workspace.path().to_path_buf()])
        .expect("answer the folder picker");
    app.press("id:welcome-open-folder").expect("open a folder");
    app.wait_for_gone("id:welcome", GONE)
        .expect("the picked folder replaces the welcome");
    app.wait_for("id:workspace-home")
        .expect("the workspace in the column");
    app.wait_for("role:button[readme.md]")
        .expect("the workspace's document, ready to open");
    app.press("role:button[readme.md]")
        .expect("open the document");
    app.wait_for("label:Markdown source")
        .expect("the document's editor");
    drop(app);

    // Open sample.
    let config = tempfile::tempdir().expect("config dir");
    let mut app = start(config.path(), None);
    app.wait_for("id:welcome-open-sample")
        .expect("the welcome's Open sample");
    app.press("id:welcome-open-sample")
        .expect("open the sample");
    app.wait_for_gone("id:welcome", GONE)
        .expect("the sample replaces the welcome");
    app.wait_for("id:workspace-home")
        .expect("the sample workspace in the column");
    drop(app);

    // A directory on the command line never shows the welcome.
    let config = tempfile::tempdir().expect("config dir");
    let mut app = start(config.path(), Some(workspace.path()));
    app.wait_for("id:workspace-home")
        .expect("the directory's workspace in the column");
    assert_eq!(
        app.screen().count("id:welcome").expect("selector"),
        0,
        "no welcome under a directory argument: {:?}",
        app.screen().texts()
    );
    app.wait_for("id:workspace-home-file")
        .expect("the directory's document listed");
    assert_eq!(
        app.screen()
            .count("id:workspace-home-empty")
            .expect("selector"),
        0,
        "a folder with a document is not called empty"
    );
    drop(app);

    // An empty folder says so, and lists nothing.
    let empty = tempfile::tempdir().expect("empty workspace");
    let config = tempfile::tempdir().expect("config dir");
    let mut app = start(config.path(), Some(empty.path()));
    app.wait_for("id:workspace-home-empty")
        .expect("the empty folder's line");
    assert_eq!(
        app.screen()
            .count("id:workspace-home-file")
            .expect("selector"),
        0,
        "no file rows in an empty folder"
    );
}
