//! Escape backs out of the Agent Context field: with the editor holding the
//! focus and from inside the field, and only after the layers above it.

use std::{
    io::{BufRead, BufReader, Read, Write},
    net::{TcpListener, TcpStream},
    sync::{Mutex, mpsc},
    time::Duration,
};

use nsh_core::{
    Size,
    element::{Idiom, Os},
    platform::ColorScheme,
};
use nsh_engine::{EngineConfig, Points, Root};
use nsh_testing::Headless;

mod support;

const EDITOR: &str = "label:Markdown source";

static TEST_LOCK: Mutex<()> = Mutex::new(());

fn start() -> (Headless, tempfile::TempDir, tempfile::TempDir) {
    let work = tempfile::tempdir().expect("workspace");
    std::fs::write(work.path().join("AGENTS.md"), "# Rules\n").expect("write AGENTS.md");
    std::fs::write(work.path().join("draft.md"), "# Draft\n").expect("write draft");
    let config = tempfile::tempdir().expect("config dir");
    let codex_home = tempfile::tempdir().expect("codex home");
    // SAFETY: TEST_LOCK serializes app lifetimes and environment changes.
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

fn start_waiting_translation() -> (
    Headless,
    tempfile::TempDir,
    tempfile::TempDir,
    mpsc::Sender<()>,
) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind loopback");
    let base_url = format!("http://{}/v1/", listener.local_addr().unwrap());
    let (release_response, response_waiting) = mpsc::channel();
    std::thread::spawn(move || {
        let Ok((mut stream, _)) = listener.accept() else {
            return;
        };
        let _ = read_http_body(&mut stream);
        response_waiting
            .recv_timeout(Duration::from_secs(20))
            .expect("release the translation");
        write_json(&mut stream, "[\"你好\"]");
    });

    let work = tempfile::tempdir().expect("workspace");
    std::fs::write(work.path().join("draft.md"), "# Draft\n").expect("write draft");
    let config = tempfile::tempdir().expect("config dir");
    let codex_home = config.path().join("codex-home");
    std::fs::create_dir_all(&codex_home).expect("codex home");
    let endpoint = mt_core::model::EndpointIdentity::parse(
        mt_core::model::Provider::OpenAiChat,
        Some(&base_url),
    )
    .expect("endpoint");
    let identity = endpoint.credential_target().to_string();
    std::fs::write(
        config.path().join("settings.toml"),
        format!(
            "model-provider = \"openai-chat\"\nmodel-name = \"translation-model\"\nmodel-base-url = \"{base_url}\"\nmodel-environment-key-identity = \"{identity}\"\ntranslate-to = \"zh\"\n"
        ),
    )
    .expect("write settings");
    // SAFETY: TEST_LOCK serializes app lifetimes and environment changes.
    unsafe {
        std::env::remove_var("MARKTURBO_OPEN");
        std::env::set_var("MARKTURBO_CONFIG_DIR", config.path());
        std::env::set_var("MARKTURBO_DATA_DIR", config.path().join("data"));
        std::env::set_var(
            "MARKTURBO_TEST_LOCK_NAMESPACE",
            std::process::id().to_string(),
        );
        std::env::set_var("CODEX_HOME", &codex_home);
        std::env::set_var("OPENAI_API_KEY", "loopback-test-key");
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

    app.press("id:title-settings").expect("open the title menu");
    app.press("id:title-menu-translate")
        .expect("start the translation");
    app.wait_for("id:consent-card").expect("the disclosure");
    support::hold_send(&mut app);
    app.wait_for("text:Waiting for model request approval…")
        .expect("the translation is in flight");

    (app, work, config, release_response)
}

#[test]
fn escape_closes_context_from_the_editor_and_from_the_field() {
    let _guard = TEST_LOCK.lock().unwrap_or_else(|error| error.into_inner());
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

#[test]
fn escape_cancels_a_translation_in_flight_before_closing_context() {
    let _guard = TEST_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    let (mut app, _work, _config, release_response) = start_waiting_translation();

    open_context(&mut app);
    app.press_key("escape").expect("Escape");
    app.wait_for("text:Cancelled — nothing will be applied.")
        .expect("the translation is cancelled");
    assert!(
        app.screen().matches("id:context-field").expect("selector"),
        "the Escape closed Context under the in-flight translation: {:?}",
        app.screen().texts()
    );

    release_response.send(()).expect("release the late answer");
    app.wait(Duration::from_millis(600))
        .expect("the late answer lands");
    assert!(
        !app.shows("你好"),
        "the late translation was applied: {:?}",
        app.screen().texts()
    );
    assert!(app.shows("# Draft"), "the document was changed");

    app.press_key("escape").expect("second Escape");
    app.wait_for_gone("id:context-field", Duration::from_secs(5))
        .expect("the second Escape closes the Context field");
}

#[test]
fn an_editor_selection_collapses_before_escape_cancels_a_translation() {
    let _guard = TEST_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    let (mut app, _work, _config, release_response) = start_waiting_translation();

    app.type_text(EDITOR, "").expect("focus the editor");
    app.press_key("mod+a").expect("select all");
    app.press_key("escape").expect("the editor's Escape");
    assert!(
        app.shows("Waiting for model request approval…"),
        "the selection's Escape cancelled the translation: {:?}",
        app.screen().texts()
    );

    app.press_key("escape").expect("the page's Escape");
    app.wait_for("text:Cancelled — nothing will be applied.")
        .expect("the translation is cancelled");

    release_response.send(()).expect("release the late answer");
    app.wait(Duration::from_millis(600))
        .expect("the late answer lands");
    assert!(
        !app.shows("你好"),
        "the late translation was applied: {:?}",
        app.screen().texts()
    );
    assert!(app.shows("# Draft"), "the document was changed");
}

#[test]
fn escape_closes_the_disk_compare_before_closing_context() {
    let _guard = TEST_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    let (mut app, work, _config) = start();
    app.type_text(EDITOR, "\nLocal edit.")
        .expect("edit the buffer");
    std::fs::write(work.path().join("draft.md"), "# Draft changed on disk\n")
        .expect("replace the file");
    app.press("id:title-settings").expect("open the title menu");
    app.press("id:title-menu-save").expect("attempt the save");
    app.wait_for("id:conflict-banner")
        .expect("the conflict banner");
    app.press("label:Compare").expect("open the comparison");
    app.wait_for("id:conflict-previous")
        .expect("the comparison is open");

    open_context(&mut app);
    app.press_key("escape").expect("Escape");
    app.wait_for_gone("id:conflict-previous", Duration::from_secs(5))
        .expect("the comparison closes first");
    assert!(
        app.screen().matches("id:context-field").expect("selector"),
        "the Escape closed the Context field under the comparison: {:?}",
        app.screen().texts()
    );

    app.press_key("escape").expect("second Escape");
    app.wait_for_gone("id:context-field", Duration::from_secs(5))
        .expect("the second Escape closes the Context field");
}

fn read_http_body(stream: &mut TcpStream) -> Vec<u8> {
    let mut reader = BufReader::new(stream.try_clone().expect("clone stream"));
    let mut content_length = 0;
    loop {
        let mut line = String::new();
        reader.read_line(&mut line).expect("read header");
        if line == "\r\n" || line.is_empty() {
            break;
        }
        if let Some((name, value)) = line.split_once(':')
            && name.eq_ignore_ascii_case("content-length")
        {
            content_length = value.trim().parse().expect("content length");
        }
    }
    let mut body = vec![0; content_length];
    reader.read_exact(&mut body).expect("read body");
    body
}

fn write_json(stream: &mut TcpStream, content: &str) {
    let response = serde_json::json!({
        "id": "chatcmpl-translation",
        "object": "chat.completion",
        "model": "translation-model",
        "choices": [{
            "index": 0,
            "message": {"role": "assistant", "content": content},
            "finish_reason": "stop"
        }]
    })
    .to_string();
    let _ = write!(
        stream,
        "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
        response.len(),
    );
    let _ = stream.write_all(response.as_bytes());
    let _ = stream.flush();
}
