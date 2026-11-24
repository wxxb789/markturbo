//! Escape on a waiting Translation closes the wait, discards the late answer
//! by generation, and says nothing will be applied.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::mpsc;
use std::time::Duration;

use nsh_engine::{EngineConfig, Root};
use nsh_testing::Headless;

#[test]
fn escape_discards_a_late_translation() {
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

    let dir = tempfile::tempdir().expect("temp dir");
    let config = tempfile::tempdir().expect("config dir");
    let document = dir.path().join("plan.md");
    std::fs::write(&document, "# Hello\n").expect("write fixture");
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
    app.set_viewport(nsh_core::Size::new(
        nsh_engine::Points(1280.0),
        nsh_engine::Points(800.0),
    ))
    .expect("wide viewport");
    app.wait_for("label:Markdown source")
        .expect("source editor");

    app.press("id:title-settings").expect("open the menu");
    app.press("id:title-menu-translate")
        .expect("start translation");
    app.wait_for("id:consent-card")
        .expect("translation disclosure");
    app.long_press("id:consent-send").expect("send translation");
    app.wait_for("text:Waiting for model request approval…")
        .expect("the translation is waiting");

    app.press_key("escape").expect("Escape cancels the wait");
    app.wait_for("text:Cancelled — nothing will be applied.")
        .expect("the notice says nothing will be applied");

    release_response.send(()).expect("release the late answer");
    app.wait(Duration::from_millis(600))
        .expect("the late answer lands");
    assert!(
        !app.shows("你好"),
        "the late translation was discarded: {:?}",
        app.screen().texts()
    );
    assert!(
        app.shows("# Hello"),
        "the document was not changed: {:?}",
        app.screen().texts()
    );
    assert!(
        !app.shows("Waiting for model request approval…"),
        "the wait ended"
    );
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
