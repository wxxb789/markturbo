//! A tab the model's answer belongs to must keep it, whichever tab is on
//! screen when the next file opens.
//!
//! The answer travels with its tab, so switching to another file first (here
//! `b.md`, from the palette, which only activates the tab it names) must not
//! make the reviewed `a.md` look replaceable: opening `c.md` afterwards
//! leaves it in place, and its Review is still the one the field offers to
//! continue.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::time::Duration;

use nsh_engine::{EngineConfig, Root};
use nsh_testing::Headless;

mod support;

#[test]
fn the_slot_keeps_a_reviewed_tab_that_is_not_on_screen() {
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
    let startup = dir.path().join("b.md");
    let reviewed = dir.path().join("a.md");
    let later = dir.path().join("c.md");
    std::fs::write(&startup, "# B\n").expect("write startup");
    std::fs::write(&reviewed, "# A\n").expect("write reviewed");
    std::fs::write(&later, "# C\n").expect("write later");
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
        std::env::set_var("MARKTURBO_OPEN", &startup);
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
        nsh_engine::Points(1440.0),
        nsh_engine::Points(900.0),
    ))
    .expect("wide viewport");
    app.wait_for("text:# B").expect("the startup document");

    // `a.md` opens from the palette as a normal tab.
    open_from_palette(&mut app, "a.md");
    app.wait_for("text:# A").expect("the reviewed document");

    app.press("id:title-review").expect("open Review");
    app.wait_for("id:consent-card").expect("Review disclosure");
    support::hold_send(&mut app);
    app.wait_for_within("id:review-prepare", Duration::from_secs(20))
        .expect("Review result");
    app.wait_for("text:keep the document intent")
        .expect("the model's stated goal");

    // Another file comes to the screen; every tab stays open.
    open_from_palette(&mut app, "b.md");
    app.wait_for("text:# B")
        .expect("the startup document active again");
    assert_eq!(
        app.screen().count("id:rail-close").expect("selector"),
        2,
        "both files are open before another opens"
    );

    // A third file opens beside them: the reviewed tab is not on screen, but
    // it is the one holding the answer.
    open_from_palette(&mut app, "c.md");
    app.wait_for("text:# C").expect("the new file opens");
    assert_eq!(
        app.screen().count("id:rail-close").expect("selector"),
        3,
        "the reviewed tab stays open: {:?}",
        app.screen().texts()
    );

    // And its Review is still the one the field continues from.
    open_from_palette(&mut app, "a.md");
    app.wait_for("text:# A")
        .expect("the reviewed document active again");
    app.wait_for("id:review-prepare")
        .expect("the Review still belongs to its tab");
    app.wait_for("text:keep the document intent")
        .expect("the answer is still the one on screen");
}

/// `Ctrl+P`, the file's name, Return: the palette opens the file in a normal
/// tab (or activates the tab already holding it).
fn open_from_palette(app: &mut Headless, name: &str) {
    app.press_key("mod+p").expect("open the palette");
    app.wait_for("id:palette-input")
        .expect("the palette's line");
    app.type_text("id:palette-input", name)
        .expect("filter to the file");
    app.submit("id:palette-input").expect("open the file");
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
