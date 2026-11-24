//! A Review result is kept work: closing the field hides it, the titlebar
//! Review reopens it, a new consent's cancel restores it (stale if the
//! document moved on), and results belong to their tab. Escape on a waiting
//! Review closes the wait, discards the late answer by generation, and says
//! nothing will be applied.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::mpsc;
use std::time::Duration;

use nsh_engine::{EngineConfig, Root};
use nsh_testing::Headless;

const RESULT: &str = "keep the document intent";

#[test]
fn a_preserved_review_reopens_and_escapes_the_wait() {
    let output = serde_json::json!({
        "schema_version": "review-v1",
        "scope": {"kind": "document"},
        "understood_intent": {
            "stated_goal": RESULT,
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
    })
    .to_string();
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind loopback");
    let base_url = format!("http://{}/v1/", listener.local_addr().unwrap());
    let (release_second, second_waiting) = mpsc::channel();
    std::thread::spawn(move || {
        for request in 0..2 {
            let Ok((mut stream, _)) = listener.accept() else {
                return;
            };
            let _ = read_http_body(&mut stream);
            if request == 1 {
                second_waiting
                    .recv_timeout(Duration::from_secs(20))
                    .expect("release the second review");
            }
            write_review_json(&mut stream, &output);
        }
    });

    let dir = tempfile::tempdir().expect("temp dir");
    let config = tempfile::tempdir().expect("config dir");
    let document = dir.path().join("plan.md");
    let other = dir.path().join("other.md");
    std::fs::write(&document, "# Plan\nA document under review.\n").expect("write fixture");
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
            "model-provider = \"openai-chat\"\nmodel-name = \"review-model\"\nmodel-base-url = \"{base_url}\"\nmodel-environment-key-identity = \"{identity}\"\n"
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

    // One Review, then its result is up.
    app.press("id:title-review").expect("open Review");
    app.wait_for("id:consent-card").expect("Review disclosure");
    app.long_press("id:consent-send").expect("send Review");
    app.wait_for_within("id:review-prepare", Duration::from_secs(20))
        .expect("Review result");
    app.wait_for(&format!("text:{RESULT}"))
        .expect("the result's intent");

    // Closing the field hides the result; the titlebar Review reopens it
    // instead of preparing a new one.
    app.press("id:review-close").expect("close the field");
    app.wait_for_gone("id:review-field", Duration::from_secs(2))
        .expect("the field closed");
    app.press("id:title-review").expect("reopen Review");
    app.wait_for("id:review-prepare")
        .expect("the preserved result came back");
    assert!(
        app.screen().matches("id:consent-card") == Ok(false),
        "reopening never starts a new consent: {:?}",
        app.screen().texts()
    );

    // Results belong to their tab: another document shows none, and coming
    // back shows the result again.
    app.press_key("mod+p").expect("open the file palette");
    app.wait_for("id:palette-input").expect("the palette input");
    app.type_text("id:palette-input", "other")
        .expect("filter to other.md");
    app.press("id:palette-row").expect("open other.md");
    app.wait_for("id:title-file >> text:other.md")
        .expect("the other tab");
    app.wait_for_gone("id:review-field", Duration::from_secs(2))
        .expect("no result for another tab");
    app.press_key("mod+p").expect("open the palette again");
    app.type_text("id:palette-input", "plan")
        .expect("filter back to plan.md");
    app.press("id:palette-row").expect("back to plan.md");
    app.wait_for(&format!("text:{RESULT}"))
        .expect("the result shows on its own tab again");

    // An edit while the new consent card waits leaves the restored result
    // stale.
    app.press("id:title-review")
        .expect("prepare another Review");
    app.wait_for("id:consent-card").expect("the new disclosure");
    app.type_text("label:Markdown source", "\nchanged while waiting")
        .expect("the document moves on behind the card");
    app.press("id:consent-cancel-button")
        .expect("cancel the consent");
    app.wait_for_within("id:review-stale", Duration::from_secs(5))
        .expect("the restored result is stale");
    app.wait_for("text:Text changed since this review")
        .expect("the stale banner names the state");

    // Escape on a waiting Review closes the wait. The late answer is
    // discarded by generation, and the notice says nothing was applied.
    app.press("id:review-again").expect("start a fresh Review");
    app.wait_for("id:consent-card").expect("the new disclosure");
    app.long_press("id:consent-send")
        .expect("send the second Review");
    app.wait_for("id:review-cancel")
        .expect("the waiting line is up");
    app.press_key("escape").expect("Escape cancels the wait");
    app.wait_for_gone("id:review-field", Duration::from_secs(2))
        .expect("the cancelled field closed");
    app.wait_for("text:Cancelled — nothing will be applied.")
        .expect("the notice says nothing will be applied");
    let notice = || {
        app.screen()
            .texts()
            .into_iter()
            .collect::<Vec<_>>()
            .join("\n")
    };
    assert!(
        !notice().contains("recall"),
        "the notice never claims a recall: {:?}",
        notice()
    );

    // The loopback answer arrives after the cancel and changes nothing.
    release_second.send(()).expect("release the late answer");
    app.wait(Duration::from_millis(600))
        .expect("the late answer lands");
    assert!(
        app.screen().matches("id:review-field") == Ok(false),
        "the late result never lands: {:?}",
        app.screen().texts()
    );
    assert!(
        !app.shows(RESULT) && !app.shows("keep the document intent"),
        "nothing of the late result is shown"
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
