//! One UI timer never cancels another's step (A1): a fresh Review's seconds
//! hint and the glow clock of the result on screen keep their own timers, so
//! the light still steps while the field waits and after a Review completes.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::mpsc;
use std::time::{Duration, SystemTime};

use nsh_core::time::Clock;
use nsh_engine::{EngineConfig, Root};
use nsh_testing::Headless;

mod support;

const RESULT: &str = "keep the document intent";

#[test]
fn the_glow_clock_keeps_stepping_while_the_field_waits_and_after_a_review() {
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
    let review_text = output;
    // The second Review's answer waits until the test releases it, so the
    // field is observably busy while the glow clock should still step.
    let (release, waiting) = mpsc::channel::<()>();
    std::thread::spawn(move || {
        for request in 0..2 {
            let Ok((mut stream, _)) = listener.accept() else {
                return;
            };
            let _ = read_http_body(&mut stream);
            if request == 1 {
                let _ = waiting.recv_timeout(Duration::from_secs(30));
            }
            write_review_json(&mut stream, &review_text);
        }
    });

    let dir = tempfile::tempdir().expect("temp dir");
    let config = tempfile::tempdir().expect("config dir");
    let document = dir.path().join("plan.md");
    std::fs::write(&document, "# Plan\nA document under review.\n").expect("write fixture");
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

    let mut engine = EngineConfig::new(Root::of::<mt_app::App>());
    engine.clock = Clock::manual(SystemTime::now());
    // A platform that delivers raw key events to the focused card, as the
    // Windows host does: the headless backend does so on `Os::Linux`.
    engine.environment.os = nsh_core::element::Os::Linux;
    engine.environment.idiom = nsh_core::element::Idiom::Desktop;
    let mut app = Headless::start(engine).expect("start the app");
    app.set_viewport(nsh_core::Size::new(
        nsh_engine::Points(1280.0),
        nsh_engine::Points(800.0),
    ))
    .expect("wide viewport");
    app.wait_for("label:Markdown source")
        .expect("source editor");

    // One Review, then its result is up and its glow clock is stepping.
    app.press("id:title-review").expect("open Review");
    app.wait_for("id:consent-card").expect("Review disclosure");
    support::hold_send(&mut app);
    app.wait_for_within("id:review-prepare", Duration::from_secs(20))
        .expect("Review result");
    app.wait_for(&format!("text:{RESULT}"))
        .expect("the result's intent");
    app.run_for(Duration::from_millis(500))
        .expect("the glow clock steps");
    let stepped = mt_app::test_vault::glow_ticks();
    app.run_for(Duration::from_millis(500))
        .expect("the glow clock steps again");
    assert!(
        mt_app::test_vault::glow_ticks() > stepped,
        "the glow clock keeps stepping while a result is live"
    );

    // A fresh Review runs while the stale result's light still shows: the
    // card's own focus step and the wait's hint keep slots of their own.
    app.type_text("label:Markdown source", "\n\nLocal edit.")
        .expect("edit the reviewed document");
    app.wait_for("id:review-stale")
        .expect("the result turns stale");
    app.press("id:review-again").expect("start a fresh Review");
    app.wait_for("id:consent-card").expect("the new disclosure");
    // The stale result's light still shows under the card, and its clock
    // still steps; the card's own focus step takes nothing from it.
    let card_ticks = mt_app::test_vault::glow_ticks();
    app.run_for(Duration::from_millis(400))
        .expect("the glow clock steps under the card");
    assert!(
        mt_app::test_vault::glow_ticks() > card_ticks,
        "the card's focus step does not cancel the glow clock"
    );
    // The card takes the keyboard while the glow clock runs: only the card's
    // own handlers measure the tap.
    app.press_key("ctrl+enter").expect("a tap on the card");
    app.wait_for("text:Nothing was sent.")
        .expect("the card heard the tap while the glow clock ran");
    support::hold_send(&mut app);
    app.wait_for("id:review-cancel")
        .expect("the waiting line is up");
    // The seconds hint is its own timer, kept alive to its deadline.
    app.wait_for("text:Usually 10–30 seconds. Esc cancels.")
        .expect("the seconds hint shows while the field waits");

    // The answer lands: the clock still steps under the new result.
    release.send(()).expect("release the Review answer");
    app.wait_for_within("id:review-prepare", Duration::from_secs(20))
        .expect("the second result");
    let finished_ticks = mt_app::test_vault::glow_ticks();
    app.run_for(Duration::from_millis(500))
        .expect("the glow clock steps after the Review completes");
    assert!(
        mt_app::test_vault::glow_ticks() > finished_ticks,
        "a Review completing does not freeze the glow clock"
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
