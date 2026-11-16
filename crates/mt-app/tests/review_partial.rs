//! An Agent Skill package that omits a supporting file its entrypoint refers
//! to is disclosed with the omission and its Review result is labelled
//! partial; once the file is there, the result is not.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::mpsc;
use std::time::Duration;

use nsh_engine::{EngineConfig, Root};
use nsh_testing::Headless;

const SKILL: &str = "---\nname: tool\ndescription: Runs the tool.\n---\n# Tool\n\nFollow [the guide](references/guide.md).\n";

#[test]
fn a_package_missing_a_referenced_file_is_reviewed_as_partial() {
    let output = serde_json::json!({
        "schema_version": "review-v1",
        "scope": {"kind": "agent_skill_package"},
        "understood_intent": {
            "stated_goal": "run the tool",
            "relevant_context": [],
            "constraints": [],
            "non_goals": [],
            "expected_deliverable": "a reviewed skill",
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
    let (bodies, received) = mpsc::channel();
    std::thread::spawn(move || {
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
    let skill_root = dir.path().join("tool");
    std::fs::create_dir_all(skill_root.join("references")).expect("skill directory");
    let skill = skill_root.join("SKILL.md");
    std::fs::write(&skill, SKILL).expect("write the skill");
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
        std::env::set_var("MARKTURBO_OPEN", &skill);
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

    // An Agent Skill is reviewed as one package: Review selection says so
    // and prepares nothing.
    app.type_text("label:Markdown source", "")
        .expect("focus the editor");
    app.press_key("ctrl+a").expect("select all");
    app.press("id:title-settings").expect("open the menu");
    app.press("id:title-menu-review-selection")
        .expect("Review selection");
    app.wait_for("text:An Agent Skill is reviewed as one package: use Review.")
        .expect("the package is reviewed whole");
    assert!(!app.shows("id:consent-card"), "nothing was prepared");
    assert!(
        !app.shows("id:review-unavailable"),
        "not a failure to prepare"
    );

    // The consent names the omission among what stays.
    app.press("id:title-review").expect("open Review");
    app.wait_for("id:consent-card").expect("package disclosure");
    app.wait_for("id:consent-row-stays")
        .expect("the omission is disclosed");
    assert!(app.shows("references/guide.md"), "the omitted path");
    assert!(app.shows("referenced file not found"), "why it is not sent");
    app.long_press("id:consent-send").expect("send Review");
    let body = received
        .recv_timeout(Duration::from_secs(20))
        .expect("the request reached the endpoint");
    assert!(
        body.contains("Follow [the guide]"),
        "the entrypoint is sent"
    );
    app.wait_for_within("text:run the tool", Duration::from_secs(20))
        .expect("the result");
    app.wait_for("id:review-partial")
        .expect("the result is labelled partial");
    assert!(app.shows("Partial review"), "the label's copy");

    // With the referenced file present, the package is whole and so is the
    // result.
    std::fs::write(skill_root.join("references").join("guide.md"), "Guide.\n")
        .expect("add the guide");
    app.press("id:title-review").expect("Review again");
    app.wait_for("id:consent-card").expect("package disclosure");
    assert!(
        app.wait_for_gone("id:consent-row-stays", Duration::from_secs(2))
            .is_ok(),
        "nothing stays behind"
    );
    app.long_press("id:consent-send").expect("send Review");
    let body = received
        .recv_timeout(Duration::from_secs(20))
        .expect("the second request reached the endpoint");
    assert!(body.contains("Guide."), "the supporting file is sent");
    app.wait_for_within("text:run the tool", Duration::from_secs(20))
        .expect("the second result");
    assert!(
        app.wait_for_gone("id:review-partial", Duration::from_secs(2))
            .is_ok(),
        "a complete package is not labelled partial"
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
