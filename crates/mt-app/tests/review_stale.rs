//! A Review result stops presenting itself as current after its source changes.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::time::Duration;

use nsh_engine::{EngineConfig, Root};
use nsh_testing::Headless;

#[test]
fn editing_the_reviewed_document_marks_the_result_stale() {
    let output = serde_json::json!({
        "schema_version": "review-v1",
        "scope": {"kind": "document"},
        "understood_intent": {
            "stated_goal": "keep the document intent",
            "relevant_context": [],
            "constraints": [],
            "non_goals": [],
            "expected_deliverable": "a reviewed document",
            "success_evidence": [],
            "inferred_assumptions": [],
            "unresolved_decisions": []
        },
        "findings": [],
        "clarification_questions": []
    });
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind loopback");
    let base_url = format!("http://{}/v1/", listener.local_addr().unwrap());
    let review_text = output.to_string();
    std::thread::spawn(move || {
        let Ok((mut stream, _)) = listener.accept() else {
            return;
        };
        let _ = read_http_body(&mut stream);
        write_review_json(&mut stream, &review_text);
    });

    let dir = tempfile::tempdir().expect("temp dir");
    let config = tempfile::tempdir().expect("config dir");
    let document = dir.path().join("plan.md");
    std::fs::write(&document, "# Plan\nPRIVATE-REVIEW-DOCUMENT-SENTINEL\n").expect("write fixture");
    let endpoint = mt_core::model::EndpointIdentity::parse(
        mt_core::model::Provider::OpenAiChat,
        Some(&base_url),
    )
    .expect("endpoint");
    let identity = endpoint.credential_target().to_string();
    std::fs::write(
        config.path().join("settings.toml"),
        format!(
            "model-provider = \"openai-chat\"\nmodel-name = \"review-model\"\nmodel-base-url = \"{base_url}\"\nmodel-environment-key-identity = \"{identity}\"\nusage-metrics-enabled = true\n"
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
    .expect("wide viewport");
    app.wait_for("label:Markdown source")
        .expect("source editor");
    app.press("text:Review").expect("open Review");
    app.wait_for("text:Send").expect("Review disclosure");
    app.press("text:Send").expect("send Review");
    app.wait_for_within("text:Review ready", Duration::from_secs(20))
        .expect("Review result");

    app.type_text("label:Markdown source", "\n\nLocal edit.")
        .expect("edit the reviewed document");
    app.wait_for_within(
        "text:This review describes an older source revision and is stale.",
        Duration::from_secs(5),
    )
    .expect("Review result marked stale");

    let store = config.path().join("data/usage.json");
    let stored = std::fs::read_to_string(&store).expect("usage aggregate");
    let json: serde_json::Value = serde_json::from_str(&stored).unwrap();
    let day = json["days"].as_object().unwrap().values().next().unwrap();
    assert_eq!(day["events"]["review_started"], 1);
    assert_eq!(day["events"]["review_completed"], 1);
    assert_eq!(day["review_started"]["document"]["plan"], 1);
    let usage = mt_core::usage::UsageStore::open(true);
    let exported =
        String::from_utf8(usage.export_json(std::time::SystemTime::now()).unwrap()).unwrap();
    for text in [&stored, &exported] {
        for sentinel in [
            "PRIVATE-REVIEW-DOCUMENT-SENTINEL",
            "plan.md",
            "loopback-test-key",
            &base_url,
            &document.to_string_lossy(),
        ] {
            assert!(
                !text.contains(sentinel),
                "prohibited content entered usage data"
            );
        }
    }
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

fn write_review_json(stream: &mut TcpStream, output: &str) {
    let response = serde_json::json!({
        "id": "chatcmpl-review",
        "object": "chat.completion",
        "model": "review-model",
        "choices": [{
            "index": 0,
            "message": {"role": "assistant", "content": output},
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
