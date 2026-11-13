//! Translation must remain reachable, disclose its outbound scope, and apply
//! only to the source revision it was prepared from, including during Quit.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::time::Duration;

use nsh_engine::{EngineConfig, Root};
use nsh_testing::{DialogAnswer, Headless};

mod support;

#[test]
fn translation_requires_consent_and_quit_saves_updates_received_during_its_dialog() {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind loopback");
    let base_url = format!("http://{}/v1/", listener.local_addr().unwrap());
    let (release_response, response_ready) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        for (request, content) in ["[\"你好\"]", "[\"你好，再见\"]"].into_iter().enumerate()
        {
            let Ok((mut stream, _)) = listener.accept() else {
                return;
            };
            let _ = read_http_body(&mut stream);
            if request == 1 {
                response_ready
                    .recv_timeout(Duration::from_secs(20))
                    .expect("release response during Quit");
            }
            write_json(&mut stream, content);
        }
    });

    let dir = tempfile::tempdir().expect("temp dir");
    let config = tempfile::tempdir().expect("config dir");
    let document = dir.path().join("plan.md");
    let other = dir.path().join("other.md");
    std::fs::write(&document, "# Hello\n").expect("write fixture");
    std::fs::write(&other, "# Other\n").expect("write other fixture");
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
        std::env::set_var("OPENAI_API_KEY", "loopback-test-key");
    }

    let mut app =
        Headless::start(EngineConfig::new(Root::of::<mt_app::App>())).expect("start the app");
    app.set_viewport(nsh_core::Size::new(
        nsh_engine::Points(1280.0),
        nsh_engine::Points(800.0),
    ))
    .expect("wide viewport for the toolbar");
    app.wait_for("label:Markdown source")
        .expect("source editor");

    app.on_dialog(|_| DialogAnswer::Press("Send".to_owned()));
    app.press("text:More layouts")
        .expect("open the secondary actions");
    app.press("text:Translate").expect("start translation");
    app.wait_for_within(
        "text:Translated 1 segment(s) into zh.",
        Duration::from_secs(20),
    )
    .expect("translation status");
    assert!(
        app.shows("你好"),
        "translation not shown: {:?}",
        app.screen().texts()
    );

    app.press("text:Save").expect("save translation");
    support::wait_for_file(
        &mut app,
        &document,
        |text| text.contains("你好"),
        "the saved document carries the translation",
        Duration::from_secs(20),
    );

    if app.shows("More layouts") {
        app.press("text:More layouts")
            .expect("open the secondary actions");
    }
    app.press("text:Translate")
        .expect("start another translation");
    app.wait_for("text:Waiting for model request approval…")
        .expect("second translation is pending");
    app.press("text:other.md").expect("open another tab");
    app.wait_for("text:Other").expect("other document");
    app.type_text("label:Markdown source", "\nUnsaved edit.")
        .expect("dirty the other tab");
    app.press("text:plan.md >> nth=0")
        .expect("return to the translation target");
    app.wait_for("label:Markdown source")
        .expect("translation target editor");
    app.answer_dialogs_myself();
    app.press("text:Quit").expect("request quit");
    support::wait_for_pending_dialog(
        &mut app,
        "Quit must await a decision",
        Duration::from_secs(20),
    );

    release_response.send(()).expect("release translation");
    app.wait_for_within("text:你好，再见", Duration::from_secs(20))
        .expect("second translation applies while Quit is pending");
    assert!(app.pending_dialog().is_some(), "Quit must still be pending");
    app.answer_dialog(DialogAnswer::Press("Save all".to_owned()))
        .expect("save all and quit");

    support::wait_for_file(
        &mut app,
        &document,
        |text| text.contains("你好，再见"),
        "Quit saves the translation it received",
        Duration::from_secs(20),
    );
    support::wait_for_file(
        &mut app,
        &other,
        |text| text.contains("Unsaved edit."),
        "Quit saves the other dirty tab",
        Duration::from_secs(20),
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
