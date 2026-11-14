//! End-to-end Review -> Revision consent and application through a loopback
//! provider. The server returns a fixed Review and a streaming Revision so the
//! app's real transport, disclosure, approval and apply paths run.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::Mutex;
use std::time::Duration;

use nsh_core::prelude::*;
use nsh_engine::{EngineConfig, Root};
use nsh_testing::Headless;

mod support;

static TEST_LOCK: Mutex<()> = Mutex::new(());

#[test]
fn partial_accept_records_the_final_split_once() {
    let _guard = TEST_LOCK.lock().unwrap();
    let source = "Original\nUnchanged\n";
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
        }, {
            "rationale": "Change the second line.",
            "edits": [{
                "range": {"start": 9, "end": 18},
                "expected_source": "Unchanged",
                "replacement": "Changed"
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

    // SAFETY: the tests in this binary serialize app lifetimes and environment changes.
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
    app.press("text:Use the reviewed title.")
        .expect("approve the first hunk only");
    app.wait_for("text:☑ Approved").expect("hunk approved");
    app.press("text:Apply approved").expect("apply revision");
    app.wait_for("text:Approved changes applied to the buffer.")
        .expect("apply status");
    app.press("text:Save").expect("save applied revision");
    support::wait_for_file(
        &mut app,
        &document,
        |text| text == "Revised\nUnchanged\n",
        "the applied revision is saved",
        Duration::from_secs(20),
    );
    let counts = mt_core::usage::UsageStore::open(true)
        .inspect(std::time::SystemTime::now())
        .unwrap();
    let day = counts.days().values().next().expect("daily usage counts");
    assert_eq!(day.accepted_hunks, 1);
    assert_eq!(day.rejected_hunks, 1);
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
        "Revised\nUnchanged\n"
    );
}

/// A persistent Apply control also exercises a second invocation after the
/// production panel removes its completed proposal's controls.
#[component]
fn RevisionDecisionActions() -> impl IntoElement {
    let state = mt_app::state::AppState::new();
    state.open_file(
        std::env::var_os("MARKTURBO_OPEN")
            .expect("document path")
            .into(),
    );
    let review = state.clone();
    let send_review = state.clone();
    let revision = state.clone();
    let send_revision = state.clone();
    let approve = state.clone();
    let apply = state.clone();
    let review_view = state.review_view;
    let revision_status = state.revision_status;
    let hunks = state.revision_hunks;
    let tabs = state.tabs;
    view()
        .flex_col()
        .child(text("Review").on_press(move || review.start_review()))
        .child(text("Send Review").on_press(move || send_review.send_review()))
        .child(text("Prepare Revision").on_press(move || revision.start_revision()))
        .child(text("Send Revision").on_press(move || send_revision.send_revision()))
        .child(text("Approve first hunk").on_press(move || approve.toggle_revision_hunk(0)))
        .child(text("Apply").on_press(move || apply.apply_revision()))
        .child(text(move || {
            review_view
                .get()
                .map(|view| view.status)
                .unwrap_or_default()
        }))
        .child(text(move || revision_status.get().unwrap_or_default()))
        .child(text(move || format!("Hunks: {}", hunks.get().len())))
        .child(text(move || {
            tabs.get()
                .first()
                .map(|tab| tab.text.get())
                .unwrap_or_default()
        }))
}

#[test]
fn all_rejected_apply_records_one_decision_and_finalizes() {
    let _guard = TEST_LOCK.lock().unwrap();
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
    })
    .to_string();
    let revision_output = serde_json::json!({
        "schema_version": "revision-v1",
        "groups": [{
            "rationale": "Revise the first line.",
            "edits": [{
                "range": {"start": 0, "end": 5},
                "expected_source": "First",
                "replacement": "Revised first"
            }]
        }, {
            "rationale": "Revise the second line.",
            "edits": [{
                "range": {"start": 6, "end": 12},
                "expected_source": "Second",
                "replacement": "Revised second"
            }]
        }],
        "question_coverage": []
    })
    .to_string();
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind loopback");
    let base_url = format!("http://{}/v1/", listener.local_addr().unwrap());
    std::thread::spawn(move || {
        for _ in 0..3 {
            let (mut stream, _) = listener.accept().expect("provider request");
            let body = read_http_body(&mut stream);
            let request: serde_json::Value = serde_json::from_slice(&body).expect("request JSON");
            if request["stream"].as_bool().unwrap_or(false) {
                write_sse(&mut stream, &revision_output);
            } else {
                write_review_json(&mut stream, &review_output);
            }
        }
    });
    let directory = tempfile::tempdir().expect("document dir");
    let config = tempfile::tempdir().expect("config dir");
    let document = directory.path().join("plan.md");
    std::fs::write(&document, "First\nSecond\n").expect("write document");
    let endpoint = mt_core::model::EndpointIdentity::parse(
        mt_core::model::Provider::OpenAiChat,
        Some(&base_url),
    )
    .expect("endpoint");
    let identity = endpoint.credential_target();
    std::fs::write(config.path().join("settings.toml"), format!(
        "model-provider = \"openai-chat\"\nmodel-name = \"review-model\"\nmodel-base-url = \"{base_url}\"\nmodel-environment-key-identity = \"{identity}\"\nusage-metrics-enabled = true\n"
    )).expect("write settings");
    // SAFETY: the tests in this binary serialize app lifetimes and environment changes.
    unsafe {
        std::env::set_var("MARKTURBO_OPEN", &document);
        std::env::set_var("MARKTURBO_CONFIG_DIR", config.path());
        std::env::set_var("MARKTURBO_DATA_DIR", config.path().join("data"));
        std::env::set_var("OPENAI_API_KEY", "loopback-test-key");
    }
    let mut app = Headless::start(EngineConfig::new(Root::of::<RevisionDecisionActions>()))
        .expect("start revision actions");
    app.wait_for("text:First\nSecond\n")
        .expect("document loaded");
    app.press("text:Review").expect("prepare Review");
    app.press("text:Send Review").expect("send Review");
    app.wait_for_within("text:Review ready", Duration::from_secs(20))
        .expect("Review ready");
    app.press("text:Prepare Revision")
        .expect("prepare Revision");
    app.press("text:Send Revision").expect("send Revision");
    app.wait_for("text:Hunks: 2").expect("two rejected hunks");
    app.press("text:Apply").expect("finalize all rejected");
    app.wait_for("text:No changes approved.")
        .expect("rejection status");
    app.wait_for("text:Hunks: 0").expect("proposal finalized");
    assert_eq!(revision_usage_totals(), (1, 0, 2, 0));
    let store = config.path().join("data/usage.json");
    let finalized = std::fs::read(&store).expect("finalized usage aggregate");
    app.press("text:Apply")
        .expect("repeat Apply after finalization");
    app.wait_for("text:Run Review first to prepare a Revision.")
        .expect("no live proposal");
    assert_eq!(std::fs::read(&store).unwrap(), finalized);
    assert_eq!(revision_usage_totals(), (1, 0, 2, 0));
    assert_eq!(
        std::fs::read_to_string(&document).unwrap(),
        "First\nSecond\n"
    );

    app.press("text:Prepare Revision")
        .expect("prepare a fresh proposal");
    app.press("text:Send Revision")
        .expect("send the fresh Revision");
    app.wait_for("text:Hunks: 2").expect("fresh proposal hunks");
    app.press("text:Approve first hunk")
        .expect("approve one fresh hunk");
    app.press("text:Apply").expect("apply fresh final split");
    app.wait_for("text:Approved changes applied to the buffer.")
        .expect("fresh apply");
    app.wait_for("text:Revised first\nSecond\n")
        .expect("only fresh approval applied");
    assert_eq!(revision_usage_totals(), (2, 1, 3, 1));
    let finalized = std::fs::read(&store).unwrap();
    app.press("text:Apply")
        .expect("repeat Apply after successful finalization");
    app.wait_for("text:Run Review first to prepare a Revision.")
        .expect("fresh proposal finalized");
    assert_eq!(std::fs::read(&store).unwrap(), finalized);
}

fn revision_usage_totals() -> (u64, u64, u64, u64) {
    use mt_core::usage::UsageEventKind;
    mt_core::usage::UsageStore::open(true)
        .inspect(std::time::SystemTime::now())
        .unwrap()
        .days()
        .values()
        .fold((0, 0, 0, 0), |total, day| {
            (
                total.0
                    + day
                        .events
                        .get(&UsageEventKind::RevisionDecision)
                        .copied()
                        .unwrap_or_default(),
                total.1 + day.accepted_hunks,
                total.2 + day.rejected_hunks,
                total.3
                    + day
                        .events
                        .get(&UsageEventKind::RevisionApplied)
                        .copied()
                        .unwrap_or_default(),
            )
        })
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
