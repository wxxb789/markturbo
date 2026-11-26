//! Escape backs out of the Agent Context field: with the editor holding the
//! focus and from inside the field, and only after the layers above it.

use std::time::Duration;

use nsh_core::{
    Size,
    element::{Idiom, Os},
    platform::ColorScheme,
};
use nsh_engine::{EngineConfig, Points, Root};
use nsh_testing::Headless;

const EDITOR: &str = "label:Markdown source";

fn start() -> (Headless, tempfile::TempDir, tempfile::TempDir) {
    let work = tempfile::tempdir().expect("workspace");
    std::fs::write(work.path().join("AGENTS.md"), "# Rules\n").expect("write AGENTS.md");
    std::fs::write(work.path().join("draft.md"), "# Draft\n").expect("write draft");
    let config = tempfile::tempdir().expect("config dir");
    let codex_home = tempfile::tempdir().expect("codex home");
    // SAFETY: nextest runs each test in its own process.
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
    let mut engine = EngineConfig::new(Root::of::<mt_app::App>());
    engine.environment.os = Os::Windows;
    engine.environment.idiom = Idiom::Desktop;
    engine.environment.color_scheme = ColorScheme::Light;
    let mut app = Headless::start(engine).expect("start app");
    app.set_viewport(Size::new(Points(1400.0), Points(900.0)))
        .expect("viewport");
    app.wait_for("id:welcome").expect("welcome");
    app.on_pick_paths(vec![work.path().to_path_buf()])
        .expect("answer the folder picker");
    app.press("id:welcome-open-folder")
        .expect("open the folder");
    app.wait_for("id:workspace-home").expect("the workspace");
    app.press_key("mod+p").expect("open the palette");
    app.wait_for("id:palette-input").expect("the palette");
    app.type_text("id:palette-input", "draft").expect("filter");
    app.submit("id:palette-input").expect("open draft.md");
    app.wait_for(EDITOR).expect("the editor");
    (app, work, config)
}

fn open_context(app: &mut Headless) {
    app.press("id:title-settings").expect("open the title menu");
    app.press("id:title-menu-context").expect("open Context");
    app.wait_for("id:context-field").expect("the context field");
}

#[test]
fn escape_closes_context_from_the_editor_and_from_the_field() {
    let (mut app, _work, _config) = start();

    // Context is the topmost open layer; Escape in the editor closes it and
    // returns the keyboard there.
    open_context(&mut app);
    app.press_key("escape").expect("escape");
    app.wait_for_gone("id:context-field", Duration::from_secs(5))
        .expect("Escape closes the Context field");
    app.press_key("z").expect("type after the close");
    assert!(
        app.screen()
            .input_text(EDITOR)
            .expect("read the editor")
            .is_some_and(|text| text.contains('z')),
        "Escape left the keyboard off the editor: {:?}",
        app.screen().input_text(EDITOR)
    );

    // Focus inside the field: a field control takes it, and Escape still
    // closes the field.
    open_context(&mut app);
    app.wait_for("id:context-profile-details")
        .expect("a field control");
    app.press("id:context-profile-details")
        .expect("focus a control inside the field");
    app.press_key("escape").expect("escape");
    app.wait_for_gone("id:context-field", Duration::from_secs(5))
        .expect("Escape closes the Context field from its own focus");

    // The title menu stays the topmost layer: Escape closes it before the
    // field.
    open_context(&mut app);
    app.press("id:title-settings").expect("open the title menu");
    app.wait_for("id:title-menu").expect("the menu");
    app.press_key("escape").expect("first Escape");
    app.wait_for_gone("id:title-menu", Duration::from_secs(5))
        .expect("the menu closes first");
    assert!(
        app.screen()
            .matches("id:context-field")
            .expect("valid selector"),
        "the first Escape closed the Context field under the menu"
    );
    app.press_key("escape").expect("second Escape");
    app.wait_for_gone("id:context-field", Duration::from_secs(5))
        .expect("the second Escape closes the Context field");
}
