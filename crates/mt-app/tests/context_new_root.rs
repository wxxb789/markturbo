//! An open Context field follows a new workspace root: the folder open
//! re-resolves the chain instead of leaving the old root's (or an empty)
//! result on screen until a manual Refresh.

use nsh_engine::{EngineConfig, Points, Root};
use nsh_testing::Headless;

#[test]
fn opening_another_folder_re_resolves_the_open_context() {
    let first = tempfile::tempdir().expect("first workspace");
    let second = tempfile::tempdir().expect("second workspace");
    let config = tempfile::tempdir().expect("config dir");
    let codex_home = tempfile::tempdir().expect("codex home");
    std::fs::write(first.path().join("draft.md"), "# Draft\n").expect("write first file");
    std::fs::write(second.path().join("AGENTS.md"), "# Rules\n").expect("write second file");

    // SAFETY: this binary has one test.
    unsafe {
        std::env::remove_var("MARKTURBO_OPEN");
        std::env::set_var("MARKTURBO_CONFIG_DIR", config.path());
        std::env::set_var("MARKTURBO_DATA_DIR", config.path().join("data"));
        std::env::set_var(
            "MARKTURBO_TEST_LOCK_NAMESPACE",
            std::process::id().to_string(),
        );
        std::env::set_var("CODEX_HOME", codex_home.path());
    }

    let mut app =
        Headless::start(EngineConfig::new(Root::of::<mt_app::App>())).expect("start the app");
    app.set_viewport(nsh_core::Size::new(Points(1440.0), Points(900.0)))
        .expect("wide viewport");
    app.wait_for("id:welcome").expect("welcome");

    // First workspace: no AGENTS.md, so the context chain has no source.
    app.on_pick_paths(vec![first.path().to_path_buf()])
        .expect("answer the first picker");
    app.press("id:welcome-open-folder")
        .expect("open the first folder");
    app.wait_for("id:workspace-home").expect("the workspace");
    app.press("id:title-settings")
        .expect("open the glyph's menu");
    app.press("id:title-menu-context")
        .expect("open the context");
    app.wait_for("id:context-field").expect("the context field");
    assert_eq!(
        app.screen().count("id:context-source-0").expect("selector"),
        0,
        "the first workspace has no instruction source"
    );

    // Second workspace: its AGENTS.md must resolve under the context still
    // on screen, without a manual Refresh.
    app.on_pick_paths(vec![second.path().to_path_buf()])
        .expect("answer the second picker");
    app.press("id:title-settings")
        .expect("open the glyph's menu");
    app.press("id:title-menu-open-folder")
        .expect("open the second folder");
    app.wait_for("id:context-source-0 >> text:AGENTS.md")
        .expect("the open context follows the new root");
    assert_eq!(
        app.screen().count("id:context-source-0").expect("selector"),
        1,
        "the second workspace's chain resolves exactly one source"
    );
    assert!(
        app.screen().matches("id:context-field").expect("selector"),
        "the context field stays open across the folder open"
    );
}
