//! End-to-end Review -> Revision consent and application through a loopback
//! provider. The server returns a fixed Review and a streaming Revision so the
//! app's real transport, disclosure, approval and apply paths run.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::time::Duration;

use nsh_engine::{EngineConfig, Root};
use nsh_testing::Headless;

mod support;

#[test]
fn review_then_revision_applies_only_approved_change() {
    let source = "Original\n";
    let review_output = serde_json::json!({
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
    let revision_output = serde_json::json!({
        "schema_version": "revision-v1",
        "groups": [{
            "rationale": "Use the reviewed title.",
            "edits": [{
                "range": {"start": 0, "end": 8},
                "expected_source": "Original",
                "replacement": "Revised"
            }]
        }],
        "question_coverage": []
    });

    let listener = TcpListener::bind("127.0.0.1:0").expect("bind loopback");
    let base_url = format!("http://{}/v1/", listener.local_addr().unwrap());
    let review_text = review_output.to_string();
    let revision_text = revision_output.to_string();
    std::thread::spawn(move || {
        for _ in 0..4 {
            let Ok((mut stream, _)) = listener.accept() else {
                return;
            };
            let body = read_http_body(&mut stream);
            let request: serde_json::Value =
                serde_json::from_slice(&body).unwrap_or(serde_json::Value::Null);
            let streaming = request
                .get("stream")
                .and_then(serde_json::Value::as_bool)
                .unwrap_or(false);
            if streaming {
                write_sse(&mut stream, &revision_text);
            } else {
                write_review_json(&mut stream, &review_text);
            }
        }
    });

    let dir = tempfile::tempdir().expect("temp dir");
    let config = tempfile::tempdir().expect("config dir");
    let document = dir.path().join("plan.md");
    std::fs::write(&document, source).expect("write fixture");
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
    app.wait_for("label:Markdown source")
        .expect("source editor");

    app.press("text:Review").expect("start review");
    app.wait_for("text:Send").expect("review disclosure");
    app.press("text:Send").expect("send review");
    app.wait_for_within("text:Review ready", Duration::from_secs(20))
        .expect("review ready");

    app.press("text:Prepare revision")
        .expect("prepare revision");
    app.wait_for("text:Send revision")
        .expect("revision disclosure");
    app.press("text:Send revision").expect("send revision");
    app.wait_for("text:☐ Rejected").expect("revision hunk");
    app.press("text:☐ Rejected").expect("approve hunk");
    app.wait_for("text:☑ Approved").expect("hunk approved");
    app.press("text:Apply approved").expect("apply revision");
    app.wait_for("text:Approved changes applied to the buffer.")
        .expect("apply status");
    app.press("text:Save").expect("save applied revision");
    support::wait_for_file(
        &mut app,
        &document,
        |text| text == "Revised\n",
        "the applied revision is saved",
        Duration::from_secs(20),
    );
    let counts = mt_core::usage::UsageStore::open(true)
        .inspect(std::time::SystemTime::now())
        .unwrap();
    let day = counts.days().values().next().expect("daily usage counts");
    assert_eq!(day.accepted_hunks, 1);
    assert_eq!(day.rejected_hunks, 0);
    assert_eq!(
        day.events[&mt_core::usage::UsageEventKind::RevisionDecision],
        1
    );
    assert_eq!(
        day.events[&mt_core::usage::UsageEventKind::RevisionApplied],
        1
    );
    assert_eq!(
        day.events[&mt_core::usage::UsageEventKind::ArtifactSaved],
        1
    );
    assert_eq!(
        std::fs::read_to_string(&document).expect("read saved file"),
        "Revised\n"
    );
}

fn read_http_body(stream: &mut TcpStream) -> Vec<u8> {
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .expect("read timeout");
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
    write_response(stream, "application/json", response.as_bytes());
}

fn write_sse(stream: &mut TcpStream, output: &str) {
    let midpoint = output
        .char_indices()
        .nth(output.chars().count() / 2)
        .map_or(output.len(), |(index, _)| index);
    let (first, second) = output.split_at(midpoint);
    let mut events = String::new();
    for delta in [first, second] {
        events.push_str("data: ");
        events.push_str(
            &serde_json::json!({
                "id": "chatcmpl-revision",
                "object": "chat.completion.chunk",
                "model": "review-model",
                "choices": [{"index": 0, "delta": {"content": delta}, "finish_reason": null}]
            })
            .to_string(),
        );
        events.push_str("\n\n");
    }
    events.push_str("data: [DONE]\n\n");
    write_response(stream, "text/event-stream", events.as_bytes());
}

fn write_response(stream: &mut TcpStream, content_type: &str, body: &[u8]) {
    let _ = write!(
        stream,
        "HTTP/1.1 200 OK\r\ncontent-type: {content_type}\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
        body.len(),
    );
    let _ = stream.write_all(body);
    let _ = stream.flush();
}
