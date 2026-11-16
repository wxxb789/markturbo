//! Review of the source selection: the consent discloses `Selection` and
//! exactly the selected bytes, the request carries only the selected text, the
//! scope switch rebinds the card, and the findings land on the document's own
//! lines.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::mpsc;
use std::time::Duration;

use nsh_engine::{EngineConfig, Root};
use nsh_testing::Headless;

const DOCUMENT: &str = "# Plan\n\nIntro OUTSIDE-BEFORE-SENTINEL.\n\nSELECTED-SENTINEL keep the target words.\n\nAfter OUTSIDE-AFTER-SENTINEL.\n";
const SELECTED: &str = "SELECTED-SENTINEL keep the target words.";

#[test]
fn a_selection_review_sends_only_the_selection_and_anchors_its_findings() {
    let start = DOCUMENT.find(SELECTED).unwrap();
    let end = start + SELECTED.len();
    let output = serde_json::json!({
        "schema_version": "review-v1",
        "scope": {
            "kind": "selection",
            "range": {"start": start, "end": end},
            "missing_context": true
        },
        "understood_intent": {
            "stated_goal": "keep the selected intent",
            "relevant_context": [],
            "constraints": [],
            "non_goals": [],
            "expected_deliverable": "a reviewed selection",
            "success_evidence": [],
            "inferred_assumptions": [],
            "unresolved_decisions": []
        },
        "findings": [{
            "kind": "source",
            "text": "the target words",
            "anchor": {"kind": "document_quote", "quote": "the target words"}
        }],
        "clarification_questions": []
    });
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind loopback");
    let base_url = format!("http://{}/v1/", listener.local_addr().unwrap());
    let review_text = output.to_string();
    let (bodies, received) = mpsc::channel();
    std::thread::spawn(move || {
        // Every request that reaches the endpoint is reported; only a review
        // is answered.
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else {
                return;
            };
            let body = read_http_body(&mut stream);
            let _ = bodies.send(String::from_utf8_lossy(&body).into_owned());
            write_review_json(&mut stream, &review_text);
        }
    });

    let dir = tempfile::tempdir().expect("temp dir");
    let config = tempfile::tempdir().expect("config dir");
    let document = dir.path().join("plan.md");
    std::fs::write(&document, DOCUMENT).expect("write fixture");
    let endpoint = mt_core::model::EndpointIdentity::parse(
        mt_core::model::Provider::OpenAiChat,
        Some(&base_url),
    )
    .expect("endpoint");
    let identity = endpoint.credential_target().to_string();
    std::fs::write(
        config.path().join("settings.toml"),
        format!(
            "model-provider = \"openai-chat\"\nmodel-name = \"review-model\"\nmodel-base-url = \"{base_url}\"\nmodel-environment-key-identity = \"{identity}\"\n"
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

    // With nothing selected, the card offers no selection to switch to.
    app.press("id:title-review").expect("open Review");
    app.wait_for("id:consent-card")
        .expect("document disclosure");
    assert!(
        app.wait_for_gone("id:consent-scope-selection", Duration::from_secs(1))
            .is_ok(),
        "an empty selection is not offered"
    );
    assert!(
        app.shows(&format!("{} B", DOCUMENT.len())),
        "the document's bytes"
    );
    app.press("id:consent-cancel-button").expect("cancel");
    app.wait_for_gone("id:consent-card", Duration::from_secs(2))
        .expect("the card closed");

    // Select the fifth line in the source editor.
    app.type_text("label:Markdown source", "")
        .expect("focus the editor");
    app.press_key("ctrl+home").expect("to the start");
    for _ in 0..4 {
        app.press_key("down").expect("down a line");
    }
    app.press_key("shift+end").expect("select the line");

    app.press("id:title-settings").expect("open the menu");
    app.press("id:title-menu-review-selection")
        .expect("Review selection");
    app.wait_for("id:consent-card")
        .expect("selection disclosure");
    app.wait_for("id:consent-scope-selection")
        .expect("the scope says Selection");
    assert!(app.shows("Selection"), "the scope word");
    let selected_bytes = format!("{} B", SELECTED.len());
    assert!(app.shows(&selected_bytes), "exactly the selected bytes");
    assert!(
        app.shows("selected text only"),
        "the row says only the selection leaves"
    );
    assert!(
        !app.shows(&format!("{} B", DOCUMENT.len())),
        "not the document's bytes"
    );

    // The switch rebinds the card to the document and back.
    app.press("label:Document").expect("switch to the document");
    app.wait_for(&format!("text:{} B", DOCUMENT.len()))
        .expect("the document's bytes after the switch");
    app.press("label:Selection")
        .expect("switch back to the selection");
    app.wait_for(&format!("text:{selected_bytes}"))
        .expect("the selection's bytes again");

    app.long_press("id:consent-send").expect("send Review");
    let body = received
        .recv_timeout(Duration::from_secs(20))
        .expect("the request reached the endpoint");
    assert!(body.contains("SELECTED-SENTINEL"), "the selection is sent");
    for outside in [
        "OUTSIDE-BEFORE-SENTINEL",
        "OUTSIDE-AFTER-SENTINEL",
        "# Plan",
    ] {
        assert!(!body.contains(outside), "{outside} stayed on the device");
    }

    app.wait_for_within("text:keep the selected intent", Duration::from_secs(20))
        .expect("the model's stated goal");
    // The finding is anchored inside the selection, on the document's line 5
    // (the selection's own first line would be line 1).
    app.press("id:review-findings-step")
        .expect("step to the finding");
    app.wait_for_within("text:/line 5/", Duration::from_secs(5))
        .expect("the finding names the document's line");
    assert!(
        received.recv_timeout(Duration::from_millis(200)).is_err(),
        "one consent sent one request"
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
