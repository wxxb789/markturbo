//! End-to-end Review -> Revision consent and application through a loopback
//! provider. The server returns a fixed Review and a streaming Revision so the
//! app's real transport, disclosure, approval and apply paths run.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::Mutex;
use std::time::Duration;

use mt_core::settings::Language;
use nsh_core::prelude::*;
use nsh_engine::{EngineConfig, Root};
use nsh_testing::Headless;

mod support;

static TEST_LOCK: Mutex<()> = Mutex::new(());

#[test]
fn cancelling_revision_stops_the_pending_request_and_restores_review() {
    let _guard = TEST_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let source = "Original\nUnchanged\n";
    let question = "Which audience should this revision prioritize?";
    let unanswered_question = "What should readers do after reading it?";
    let answer = "Readers new to the topic.";
    let (review, _) = two_change_outputs(source);
    let mut review: serde_json::Value = serde_json::from_str(&review).expect("Review output");
    review["findings"] = serde_json::json!([{
        "kind": "inference",
        "text": "The title needs a clearer promise.",
        "anchor": {"kind": "document_wide"}
    }]);
    review["clarification_questions"] = serde_json::json!([{
        "question": question,
        "priority": "high",
        "impact": null
    }, {
        "question": unanswered_question,
        "priority": "medium",
        "impact": null
    }]);
    let review = review.to_string();
    let listener = TcpListener::bind("127.0.0.1:0").expect("loopback");
    let base_url = format!("http://{}/v1/", listener.local_addr().unwrap());
    let (started, request_received) = std::sync::mpsc::sync_channel(1);
    let server = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().expect("Review request");
        read_http_body(&mut stream);
        write_review_json(&mut stream, &review);
        drop(stream);
        let (mut stream, _) = listener.accept().expect("Revision request");
        read_http_body(&mut stream);
        stream
            .set_read_timeout(Some(Duration::from_secs(5)))
            .expect("bound the disconnect wait");
        started.send(()).expect("Revision started");
        stream.read(&mut [0; 1])
    });
    let (document, _directory, _config) = revision_fixture(source, &base_url);
    let mut app =
        Headless::start(EngineConfig::new(Root::of::<mt_app::App>())).expect("start the app");
    app.set_viewport(nsh_core::Size::new(px(1440.0), px(900.0)))
        .expect("wide viewport");
    app.wait_for("label:Markdown source")
        .expect("source editor");
    app.type_text("label:Markdown source", "\nUnsaved edit.")
        .expect("edit the buffer without saving");
    app.wait_for("id:title-dirty").expect("the dirty buffer");
    let buffer = app
        .screen()
        .input_text("label:Markdown source")
        .expect("read the unsaved buffer")
        .map(str::to_owned)
        .expect("the editor contains text");
    assert!(buffer.contains("Unsaved edit.") && buffer.contains(source));
    assert_eq!(
        app.screen()
            .input_text("label:Markdown source")
            .expect("read the buffer"),
        Some(buffer.as_str())
    );
    app.press("id:title-review").expect("prepare Review");
    app.wait_for("id:consent-card").expect("Review disclosure");
    support::hold_send(&mut app);
    app.wait_for_within("id:review-prepare", Duration::from_secs(20))
        .expect("Review ready");
    app.wait_for(&format!("text:{question}"))
        .expect("the clarification question");
    app.type_text("id:review-answer-input", answer)
        .expect("answer the clarification question");
    assert_eq!(
        app.screen()
            .input_text("id:review-answer-input")
            .expect("read the answer before submission")
            .as_deref(),
        Some(answer)
    );
    app.press("id:review-answer-submit")
        .expect("submit the answer");
    app.wait_for("text:Answered")
        .expect("the question is answered");
    app.wait_for(&format!("text:{unanswered_question}"))
        .expect("the remaining clarification is unanswered");
    app.press("id:review-findings-step")
        .expect("expand the finding");
    app.wait_for("text:The title needs a clearer promise.")
        .expect("the finding is expanded");
    app.press("id:review-prepare").expect("prepare Revision");
    app.wait_for("text:Ask for a revision?")
        .expect("Revision disclosure");
    assert!(
        !app.screen()
            .matches("id:review-finding")
            .expect("the finding selector"),
        "preparing Revision hides the expanded finding"
    );
    support::hold_send(&mut app);
    request_received
        .recv_timeout(Duration::from_secs(5))
        .expect("request waiting for headers");
    app.press("id:revision-cancel").expect("cancel Revision");
    app.wait_for("id:review-prepare")
        .expect("original Review restored");
    app.wait_for("text:The title needs a clearer promise.")
        .expect("the expanded finding is restored");
    let restored_buffer = app
        .screen()
        .input_text("label:Markdown source")
        .expect("read the restored buffer")
        .map(str::to_owned);
    app.press(&format!("label:{question}"))
        .expect("reopen the answered question");
    let restored_answer = app
        .screen()
        .input_text("id:review-answer-input")
        .expect("read the restored clarification answer")
        .map(str::to_owned);
    assert_eq!(
        (restored_buffer.as_deref(), restored_answer.as_deref(),),
        (Some(buffer.as_str()), Some(answer))
    );
    let disconnect = server.join().expect("loopback server");
    assert!(
        matches!(disconnect, Ok(0))
            || matches!(&disconnect, Err(error) if error.kind() == std::io::ErrorKind::ConnectionReset),
        "the pending HTTP request did not disconnect: {disconnect:?}"
    );
    assert!(
        !app.screen()
            .matches("id:revision-field")
            .expect("revision selector"),
        "the cancelled request left a revision surface"
    );
    assert_eq!(std::fs::read_to_string(document).expect("document"), source);
}

#[test]
fn partial_accept_records_the_final_split_once() {
    let _guard = TEST_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
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

    app.press("id:title-review").expect("start review");
    app.wait_for("id:consent-card").expect("review disclosure");
    support::hold_send(&mut app);
    app.wait_for_within("id:review-prepare", Duration::from_secs(20))
        .expect("review ready");

    // Prepare revision is the result's one next action.
    app.press("label:Prepare revision")
        .expect("prepare revision");
    app.wait_for("text:Ask for a revision?")
        .expect("revision disclosure names the revision");
    support::hold_send(&mut app);
    // The hunks arrive with the reading position on the first group.
    app.wait_for("id:revision-accept").expect("revision hunk");
    app.wait_for(
        "text:Revision ready. Each accepted change enters the buffer at once; nothing is saved.",
    )
    .expect("revision ready");
    app.wait_for("text:Use the reviewed title.")
        .expect("the first hunk is the one docked");
    // Both hunks start undecided, and an undecided hunk is rejected: `Done`
    // applies accepted changes only. The docked one waits for its decision,
    // the other waits in the list with no decision on its own row, and
    // neither counts as decided or as an applied change.
    app.wait_for("id:revision-reject")
        .expect("the docked hunk waits for a decision");
    app.wait_for("text:0 of 2 decided")
        .expect("both hunks start undecided");
    assert_eq!(
        app.screen()
            .span_style("text:0 of 2 decided")
            .expect("the revision decision count")
            .font_family
            .expect("the decision count font")
            .name()
            .as_ref(),
        mt_app::ui::tokens::FONT_SOURCE
    );
    app.wait_for("text:2 not applied")
        .expect("both hunks start rejected");
    assert_eq!(
        app.screen()
            .count("text:/ · (Accepted|Rejected)$/")
            .expect("decision-suffix selector"),
        0,
        "no hunk starts decided"
    );
    app.wait_for("text:Change the second line.")
        .expect("the second hunk waits in the list");
    // Approve the docked hunk only: the second hunk stays rejected.
    app.press("label:Accept")
        .expect("approve the first hunk only");
    app.wait_for("text:1 of 2 decided")
        .expect("exactly one hunk is decided");
    // The decision is per hunk and reads on the accepted hunk's own row: the
    // dock extinguishes, the next undecided hunk takes it, and the accepted
    // one keeps its `Accepted` word.
    app.wait_for("text:Use the reviewed title. · Accepted")
        .expect("the accepted hunk shows its decision");
    app.wait_for("text:1 not applied")
        .expect("the second hunk stays rejected");
    app.press("label:Done")
        .expect("apply the approved revision");
    app.wait_for("text:/^Approved changes applied to the document/")
        .expect("apply status");
    // Save through the title row's menu: the applied revision lands on disk.
    app.press("id:title-settings").expect("open the menu");
    app.press("id:title-menu-save")
        .expect("save applied revision");
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

/// The revision field over a document the loopback provider reviews, with
/// the buffer on screen: the field's own keys decide, and the text node shows
/// what each decision wrote.
#[component]
fn RevisionFieldOverBuffer() -> impl IntoElement {
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
    let edit = state.clone();
    let replace = state.clone();
    let review_view = state.review_view;
    let tabs = state.tabs;
    let open_state = state.clone();
    let field_state = state.clone();
    // The controls beside the field, which takes the window's height.
    let controls = view()
        .flex_col()
        .w(px(320.0))
        .child(text("Review").on_press(move || review.start_review()))
        .child(text("Send Review").on_press(move || send_review.send_review()))
        .child(text("Prepare Revision").on_press(move || revision.start_revision()))
        .child(text("Send Revision").on_press(move || send_revision.send_revision()))
        // An edit inside the accepted first word, made as the source editor
        // makes one.
        .child(text("Edit the first line").on_press(move || {
            if let Some(tab) = edit.active_tab() {
                let edited = tab.text.get().replacen("Revised", "Re-vised", 1);
                tab.text.set(SharedString::from(edited));
                tab.dirty.set(true);
                tab.revision.update(|revision| *revision += 1);
                edit.note_edit(tab.id);
            }
        }))
        // The whole buffer typed over, as select-all and typing make it.
        .child(text("Replace the document").on_press(move || {
            if let Some(tab) = replace.active_tab() {
                tab.text.set(SharedString::from(REPLACEMENT));
                tab.dirty.set(true);
                tab.revision.update(|revision| *revision += 1);
                replace.note_edit(tab.id);
            }
        }))
        .child(text(move || {
            review_view
                .get()
                .map(|view| view.status)
                .unwrap_or_default()
        }))
        .child(text(move || {
            format!(
                "Buffer: {}",
                tabs.get()
                    .first()
                    .map(|tab| tab.text.get())
                    .unwrap_or_default()
            )
        }))
        // The buffer escaped, so a selector compares every byte (a text
        // selector ignores the spaces around a text).
        .child(text(move || {
            format!(
                "Bytes: {:?}",
                tabs.get()
                    .first()
                    .map(|tab| tab.text.get().to_string())
                    .unwrap_or_default()
            )
        }))
        .child(text(move || {
            format!(
                "Dirty: {}",
                tabs.get().first().is_some_and(|tab| tab.dirty.get())
            )
        }));
    view()
        .flex_row()
        .child(controls)
        .child(view().flex_1().child(when(
            move || mt_app::ui::revision::is_open(&open_state),
            move || mt_app::ui::revision::field(field_state.clone(), Language::English),
        )))
}

/// A Review with no findings and a Revision of two changes to `source` that
/// both change length: `Original` (8 bytes) becomes `Revised` (7),
/// `Unchanged` (9) becomes `Changed` (7). Each range is where the word first
/// appears in `source`.
fn two_change_outputs(source: &str) -> (String, String) {
    let range = |word: &str| {
        let start = source.find(word).expect("the word is in the source");
        serde_json::json!({"start": start, "end": start + word.len()})
    };
    let review = serde_json::json!({
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
    let revision = serde_json::json!({
        "schema_version": "revision-v1",
        "groups": [{
            "rationale": "Use the reviewed title.",
            "edits": [{
                "range": range("Original"),
                "expected_source": "Original",
                "replacement": "Revised"
            }]
        }, {
            "rationale": "Change the second line.",
            "edits": [{
                "range": range("Unchanged"),
                "expected_source": "Unchanged",
                "replacement": "Changed"
            }]
        }],
        "question_coverage": []
    });
    (review.to_string(), revision.to_string())
}

/// Serve `review` and `revision` on a loopback provider, write `source` and
/// the settings that point at it, and start `RevisionFieldOverBuffer` up to
/// the moment the hunks arrive. The temporary directories live as long as
/// the returned guards. The buffer holds `source` with `\n` line ends, and
/// the Revision's ranges are in the buffer's text.
fn start_field_over_buffer(
    source: &str,
) -> (
    Headless,
    std::path::PathBuf,
    tempfile::TempDir,
    tempfile::TempDir,
) {
    let buffer = source.replace("\r\n", "\n");
    let (review_output, revision_output) = two_change_outputs(&buffer);
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind loopback");
    let base_url = format!("http://{}/v1/", listener.local_addr().unwrap());
    std::thread::spawn(move || {
        for _ in 0..4 {
            let Ok((mut stream, _)) = listener.accept() else {
                return;
            };
            let body = read_http_body(&mut stream);
            let request: serde_json::Value =
                serde_json::from_slice(&body).unwrap_or(serde_json::Value::Null);
            if request["stream"].as_bool().unwrap_or(false) {
                write_sse(&mut stream, &revision_output);
            } else {
                write_review_json(&mut stream, &review_output);
            }
        }
    });
    let (document, directory, config) = revision_fixture(source, &base_url);
    let mut app = Headless::start(EngineConfig::new(Root::of::<RevisionFieldOverBuffer>()))
        .expect("start the field over the buffer");
    app.wait_for(&format!("text:Buffer: {buffer}"))
        .expect("document loaded");
    app.press("text:Review").expect("prepare Review");
    app.press("text:Send Review").expect("send Review");
    app.wait_for_within("text:Review ready", Duration::from_secs(20))
        .expect("Review ready");
    app.press("text:Prepare Revision")
        .expect("prepare Revision");
    app.press("text:Send Revision").expect("send Revision");
    app.wait_for_within("id:revision-accept", Duration::from_secs(20))
        .expect("the hunks arrive");
    app.wait_for("text:0 of 2 decided")
        .expect("nothing is decided yet");
    (app, document, directory, config)
}

fn revision_fixture(
    source: &str,
    base_url: &str,
) -> (std::path::PathBuf, tempfile::TempDir, tempfile::TempDir) {
    let directory = tempfile::tempdir().expect("document dir");
    let config = tempfile::tempdir().expect("config dir");
    let document = directory.path().join("plan.md");
    std::fs::write(&document, source).expect("write document");
    let endpoint = mt_core::model::EndpointIdentity::parse(
        mt_core::model::Provider::OpenAiChat,
        Some(base_url),
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
        std::env::set_var(
            "MARKTURBO_TEST_LOCK_NAMESPACE",
            std::process::id().to_string(),
        );
        std::env::set_var("OPENAI_API_KEY", "loopback-test-key");
    }
    (document, directory, config)
}

/// Accept writes its hunk into the buffer at once, and only that hunk; a
/// later hunk lands on its own bytes after an earlier one changed length;
/// Reject writes nothing; `Ctrl+Z` takes each decision back with exactly
/// its text; `Done` keeps what was accepted, discards the undecided hunk
/// unwritten, and nothing reaches the disk.
#[test]
fn decisions_write_at_once_and_undo_restores_the_exact_text() {
    let _guard = TEST_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let source = "Original\nUnchanged\n";
    let (mut app, document, _directory, _config) = start_field_over_buffer(source);

    // Accept the docked first hunk: its replacement is in the buffer now,
    // before any `Done`, and the second line is untouched.
    app.press_key("a").expect("press A");
    app.wait_for("text:Buffer: Revised\nUnchanged\n")
        .expect("the accepted hunk is written at once, and only it");
    app.wait_for("text:1 of 2 decided").expect("one decided");
    // The reading position moves on once the dock extinguishes.
    app.wait_for("text:Use the reviewed title. · Accepted")
        .expect("the second hunk is docked");

    // The second hunk sits one byte earlier now; it still replaces exactly
    // its own word.
    app.press_key("a").expect("press A on the second hunk");
    app.wait_for("text:Buffer: Revised\nChanged\n")
        .expect("the later hunk follows the earlier length change");
    app.wait_for("text:2 of 2 decided").expect("both decided");

    // Ctrl+Z takes the last decision back together with its text.
    app.press_key("ctrl+z").expect("undo the second accept");
    app.wait_for("text:1 of 2 decided")
        .expect("the decision is back");
    app.wait_for("text:Buffer: Revised\nUnchanged\n")
        .expect("exactly the second hunk's reviewed bytes are back");

    // Reject writes nothing.
    app.press_key("r").expect("reject the second hunk");
    app.wait_for("text:2 of 2 decided").expect("rejected");
    assert!(
        buffer_shows(&app, "Revised\nUnchanged\n"),
        "a rejection leaves the text: {:?}",
        app.screen().texts()
    );
    app.press_key("ctrl+z").expect("undo the rejection");
    app.wait_for("text:1 of 2 decided")
        .expect("the rejection is taken back");
    assert!(
        buffer_shows(&app, "Revised\nUnchanged\n"),
        "undoing a rejection writes nothing: {:?}",
        app.screen().texts()
    );

    // Repeated Ctrl+Z walks further back, to the reviewed bytes exactly.
    app.press_key("ctrl+z").expect("undo the first accept");
    app.wait_for("text:0 of 2 decided")
        .expect("no decision is left");
    app.wait_for(&format!("text:Buffer: {source}"))
        .expect("the reviewed text, byte for byte");
    assert_eq!(
        std::fs::read_to_string(&document).unwrap(),
        source,
        "deciding saves nothing"
    );

    // Accept the first hunk again and finish with the second undecided.
    app.press_key("a").expect("accept the first hunk again");
    app.wait_for("text:Buffer: Revised\nUnchanged\n")
        .expect("written again");
    app.wait_for("text:1 of 2 decided").expect("one decided");
    app.press_key("ctrl+enter").expect("Done");
    app.wait_for("text:/^Approved changes applied to the document/")
        .expect("Done keeps the accepted change");
    app.wait_for("text:1 not applied")
        .expect("Done reports the undecided hunk as not applied");
    assert!(
        buffer_shows(&app, "Revised\nUnchanged\n"),
        "Done writes nothing for the undecided hunk: {:?}",
        app.screen().texts()
    );
    assert_eq!(
        std::fs::read_to_string(&document).unwrap(),
        source,
        "Done saves nothing"
    );
    assert_eq!(
        revision_usage_totals(),
        (1, 1, 1, 1),
        "one decision record: one accepted, one not, one applied"
    );
}

/// `R` on every hunk in the field and then `Done` leaves the buffer and the
/// disk byte for byte as reviewed — CRLF line ends on disk, CJK and emoji
/// included — with the tab clean, and records one decision of two
/// rejections. A second `Ctrl+Enter` records nothing more.
#[test]
fn rejecting_every_hunk_then_done_keeps_the_exact_bytes() {
    let _guard = TEST_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let source = "Original 标题 🎉\r\nUnchanged 第二行 😀\r\n";
    let (mut app, document, _directory, config) = start_field_over_buffer(source);
    // The editor breaks lines at `\r\n` and holds `\n`; saving restores the
    // file's line ends, which nothing here does.
    let bytes = format!("Bytes: {:?}", source.replace("\r\n", "\n"));
    assert!(
        app.shows(&bytes),
        "the buffer before any decision: {:?}",
        app.screen().texts()
    );

    app.press_key("r").expect("reject the first hunk");
    app.wait_for("text:1 of 2 decided").expect("one decided");
    app.wait_for("text:Use the reviewed title. · Rejected")
        .expect("the second hunk is docked");
    app.press_key("r").expect("reject the second hunk");
    app.wait_for("text:2 of 2 decided").expect("both decided");
    assert!(
        app.shows(&bytes),
        "a rejection writes nothing: {:?}",
        app.screen().texts()
    );

    app.press_key("ctrl+enter").expect("Done");
    app.wait_for("text:/^No changes were approved/")
        .expect("Done with nothing approved");
    assert!(
        app.shows(&bytes) && app.shows("Dirty: false"),
        "the buffer is the reviewed source, clean: {:?}",
        app.screen().texts()
    );
    assert_eq!(
        std::fs::read(&document).unwrap(),
        source.as_bytes(),
        "the disk is the reviewed source"
    );
    assert_eq!(
        revision_usage_totals(),
        (1, 0, 2, 0),
        "one decision record: two rejected, nothing applied"
    );

    let store = config.path().join("data/usage.json");
    let finalized = std::fs::read(&store).expect("finalized usage aggregate");
    app.press_key("ctrl+enter").expect("Ctrl+Enter again");
    app.wait(Duration::from_millis(300)).expect("settle");
    assert_eq!(std::fs::read(&store).unwrap(), finalized);
    assert!(app.shows(&bytes) && app.shows("Dirty: false"));
    assert_eq!(std::fs::read(&document).unwrap(), source.as_bytes());
}

/// The user edits between decisions: an edit inside an accepted change
/// refuses its undo with a message (the decision and the edit both stand),
/// while the untouched hunk still lands on its own, moved bytes.
#[test]
fn an_edit_between_decisions_refuses_the_undo_it_would_corrupt() {
    let _guard = TEST_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let source = "Original\nUnchanged\n";
    let (mut app, document, _directory, _config) = start_field_over_buffer(source);

    app.press_key("a").expect("accept the first hunk");
    app.wait_for("text:Buffer: Revised\nUnchanged\n")
        .expect("written at once");
    app.wait_for("text:Use the reviewed title. · Accepted")
        .expect("the second hunk is docked");

    // The user types inside the accepted replacement.
    app.press("text:Edit the first line")
        .expect("edit the accepted text");
    app.wait_for("text:Buffer: Re-vised\nUnchanged\n")
        .expect("the edit is in the buffer");

    // The second hunk was not touched: it still accepts, one byte later.
    app.press_key("a").expect("accept the second hunk");
    app.wait_for("text:Buffer: Re-vised\nChanged\n")
        .expect("the untouched hunk follows the user's edit");
    app.wait_for("text:2 of 2 decided").expect("both decided");

    // Undo the second accept: its part is intact, so its bytes come back.
    app.press_key("ctrl+z").expect("undo the second accept");
    app.wait_for("text:Buffer: Re-vised\nUnchanged\n")
        .expect("the second hunk's reviewed bytes are back");
    app.wait_for("text:1 of 2 decided").expect("one decided");

    // Undo the first: the user edited it since, so the undo is refused with
    // a message and nothing is written over the edit.
    app.press_key("ctrl+z").expect("undo the edited accept");
    app.wait_for("text:Can't undo — this section was edited after the change was accepted.")
        .expect("the refusal says why");
    assert!(
        buffer_shows(&app, "Re-vised\nUnchanged\n") && app.shows("1 of 2 decided"),
        "the text and the decision both stand: {:?}",
        app.screen().texts()
    );
    assert_eq!(std::fs::read_to_string(&document).unwrap(), source);
}

/// `Copy` puts exactly the approved result on the clipboard — the buffer
/// with the accepted change in it and nothing of the undecided one — and
/// leaves the editor, the dirty flag, the decisions and the file as they
/// were.
#[test]
fn copy_exports_only_the_approved_result_and_saves_nothing() {
    let _guard = TEST_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let source = "Original\nUnchanged\n";
    let (mut app, document, _directory, _config) = start_field_over_buffer(source);
    // The footer's actions lead with `Done`: a phone-width window would clip
    // the trailing ones, so the whole row is judged at a desktop size.
    app.set_viewport(nsh_core::Size::new(px(1440.0), px(900.0)))
        .expect("wide viewport");

    // One button, found by its id and named for a screen reader.
    assert_eq!(
        app.screen()
            .count("role:button[Copy]")
            .expect("the role selector"),
        1,
        "Copy is one named button: {:?}",
        app.screen().texts()
    );

    // Nothing accepted yet: the approved result is the reviewed text.
    app.set_clipboard(None);
    app.press("id:revision-copy").expect("press Copy");
    assert_eq!(
        app.screen().clipboard(),
        Some(source),
        "nothing approved yet: the reviewed text"
    );
    assert!(
        app.shows("Dirty: false"),
        "Copy dirties nothing: {:?}",
        app.screen().texts()
    );

    // Accept the first hunk, leave the second undecided.
    app.press_key("a").expect("accept the first hunk");
    app.wait_for("text:Buffer: Revised\nUnchanged\n")
        .expect("the accepted hunk is written");
    app.wait_for("text:Dirty: true").expect("the tab is dirty");
    app.set_clipboard(None);
    app.press("id:revision-copy").expect("press Copy");
    assert_eq!(
        app.screen().clipboard(),
        Some("Revised\nUnchanged\n"),
        "the approved result exactly: the accepted change, not the undecided one"
    );
    assert!(
        buffer_shows(&app, "Revised\nUnchanged\n") && app.shows("1 of 2 decided"),
        "Copy changes neither the editor nor the decisions: {:?}",
        app.screen().texts()
    );
    assert!(
        app.shows("Dirty: true"),
        "Copy does not mark the document saved: {:?}",
        app.screen().texts()
    );
    assert_eq!(
        std::fs::read_to_string(&document).unwrap(),
        source,
        "Copy writes nothing to the file"
    );
}

const REPLACEMENT: &str = "Typed over everything\n";

#[test]
fn an_edit_over_every_hunk_keeps_accept_disabled_and_done_writes_nothing() {
    let _guard = TEST_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let source = "Original\nUnchanged\n";
    let (mut app, document, _directory, _config) = start_field_over_buffer(source);

    app.press("text:Replace the document")
        .expect("type over the document");
    app.wait_for(&format!("text:Buffer: {REPLACEMENT}"))
        .expect("the user's text is in the buffer");
    app.wait_for("text:Can't apply — this section was edited since the review.")
        .expect("the docked hunk says it can't apply");
    assert!(
        app.screen()
            .matches("id:revision-accept")
            .expect("the selector"),
        "Accept stays in the row: {:?}",
        app.screen().texts()
    );
    assert!(
        app.press("id:revision-accept").is_err(),
        "the unavailable Accept cannot be pressed"
    );
    app.press_key("a").expect("press A");
    app.press_key("j").expect("dock the second hunk");
    app.settle();
    assert!(
        app.shows("0 of 2 decided")
            && app.shows("Can't apply — this section was edited since the review.")
            && app
                .screen()
                .matches("id:revision-accept")
                .expect("the selector"),
        "the unavailable Accept remains visible: {:?}",
        app.screen().texts()
    );
    assert!(
        app.press("id:revision-accept").is_err(),
        "the second unavailable Accept cannot be pressed"
    );

    app.press_key("ctrl+enter").expect("Done");
    app.wait_for("text:/^No changes were approved/")
        .expect("Done with nothing approved");
    assert!(
        buffer_shows(&app, REPLACEMENT),
        "Done writes nothing over the user's text: {:?}",
        app.screen().texts()
    );
    assert_eq!(
        std::fs::read(&document).unwrap(),
        source.as_bytes(),
        "the disk keeps the reviewed bytes"
    );
}

/// Two documents and the Revision controls: the Revision is prepared for
/// the first, then a Review of the second resets it while the Revision is
/// still out.
#[component]
fn RevisionAcrossDocuments() -> impl IntoElement {
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
    let other = state.clone();
    let first = state.clone();
    let approve = state.clone();
    let review_view = state.review_view;
    let hunks = state.revision_hunks;
    let busy = state.revision_busy;
    let tabs = state.tabs;
    view()
        .flex_col()
        .child(text("Review").on_press(move || review.start_review()))
        .child(text("Send Review").on_press(move || send_review.send_review()))
        .child(text("Prepare Revision").on_press(move || revision.start_revision()))
        .child(text("Send Revision").on_press(move || send_revision.send_revision()))
        .child(text("Open other").on_press(move || {
            other.open_file(
                std::env::var_os("MARKTURBO_OTHER")
                    .expect("other document path")
                    .into(),
            )
        }))
        .child(text("Open first").on_press(move || {
            first.open_file(
                std::env::var_os("MARKTURBO_OPEN")
                    .expect("document path")
                    .into(),
            )
        }))
        .child(text("Approve first hunk").on_press(move || approve.toggle_revision_hunk(0)))
        .child(text(move || {
            review_view
                .get()
                .map(|view| view.status)
                .unwrap_or_default()
        }))
        .child(text(move || format!("Hunks: {}", hunks.get().len())))
        .child(text(move || format!("Busy: {}", busy.get())))
        .child(text(move || format!("Tabs: {}", tabs.get().len())))
        .child(text(move || {
            tabs.get()
                .iter()
                .map(|tab| format!("[{}]", tab.text.get()))
                .collect::<String>()
        }))
}

/// Preparing a new Review does not cancel an in-flight Revision; only
/// sending it does. The late answer presents on its own tab and cannot be
/// applied from another one, and the send clears it without writing either
/// document.
#[test]
fn a_prepared_review_keeps_an_in_flight_revision_until_it_is_sent() {
    let _guard = TEST_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let source = "Original\nUnchanged\n";
    let (review_output, revision_output) = two_change_outputs(source);
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind loopback");
    let base_url = format!("http://{}/v1/", listener.local_addr().unwrap());
    let (release, released) = std::sync::mpsc::channel::<()>();
    let (answered, answer_sent) = std::sync::mpsc::channel::<()>();
    std::thread::spawn(move || {
        for _ in 0..3 {
            let Ok((mut stream, _)) = listener.accept() else {
                return;
            };
            let body = read_http_body(&mut stream);
            let request: serde_json::Value =
                serde_json::from_slice(&body).unwrap_or(serde_json::Value::Null);
            if request["stream"].as_bool().unwrap_or(false) {
                let _ = released.recv();
                write_sse(&mut stream, &revision_output);
                let _ = answered.send(());
            } else {
                write_review_json(&mut stream, &review_output);
            }
        }
    });
    let directory = tempfile::tempdir().expect("document dir");
    let config = tempfile::tempdir().expect("config dir");
    let document = directory.path().join("plan.md");
    let other = directory.path().join("other.md");
    // The other document starts with the reviewed one's text: a placement
    // that diffs the reviewed text against it would find the first hunk's
    // bytes there.
    let other_source = "Original\nUnchanged\nMore\n";
    std::fs::write(&document, source).expect("write document");
    std::fs::write(&other, other_source).expect("write other document");
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
        std::env::set_var("MARKTURBO_OTHER", &other);
        std::env::set_var("MARKTURBO_CONFIG_DIR", config.path());
        std::env::set_var("MARKTURBO_DATA_DIR", config.path().join("data"));
        std::env::set_var(
            "MARKTURBO_TEST_LOCK_NAMESPACE",
            std::process::id().to_string(),
        );
        std::env::set_var("OPENAI_API_KEY", "loopback-test-key");
    }
    let mut app = Headless::start(EngineConfig::new(Root::of::<RevisionAcrossDocuments>()))
        .expect("start the two-document controls");
    app.wait_for("text:Tabs: 1").expect("document loaded");
    app.press("text:Review").expect("prepare Review");
    app.press("text:Send Review").expect("send Review");
    app.wait_for_within("text:Review ready", Duration::from_secs(20))
        .expect("Review ready");
    app.press("text:Prepare Revision")
        .expect("prepare Revision");
    app.press("text:Send Revision").expect("send Revision");
    app.wait_for("text:Busy: true")
        .expect("the Revision is out");

    // Switch to the other document and prepare its Review: the in-flight
    // Revision is not cancelled by the preparation alone. The shown tab has
    // no busy row of it; the switch back proves it still owns the request.
    app.press("text:Open other")
        .expect("open the other document");
    app.wait_for("text:Tabs: 2")
        .expect("the other document is open");
    app.press("text:Review").expect("review the other document");
    app.wait(Duration::from_millis(300))
        .expect("the preparation settles");
    assert!(
        app.shows("Busy: false") && app.shows("Hunks: 0"),
        "the other tab shows nothing of the in-flight Revision: {:?}",
        app.screen().texts()
    );
    app.press("text:Open first")
        .expect("back to the reviewed tab");
    app.wait_for("text:Busy: true")
        .expect("preparing a Review must not cancel the Revision");

    // The late answer lands in its own tab's session: the shown tab has no
    // hunks of it, and nothing of it can be applied from there.
    app.press("text:Open other").expect("back to the other tab");
    release.send(()).expect("release the Revision answer");
    answer_sent
        .recv_timeout(Duration::from_secs(20))
        .expect("the answer is sent");
    app.wait(Duration::from_millis(1500))
        .expect("the late answer lands");
    assert!(
        app.shows("Hunks: 0") && app.shows("Busy: false"),
        "a background tab's answer never replaces the shown tab: {:?}",
        app.screen().texts()
    );
    app.press("text:Approve first hunk")
        .expect("try to accept the other tab's hunk");
    app.wait(Duration::from_millis(300)).expect("settle");
    let both = format!("[{source}][{other_source}]");
    assert!(
        app.screen()
            .matches(&format!("text:{both}"))
            .expect("the documents selector"),
        "neither document changes from another tab: {:?}",
        app.screen().texts()
    );

    // Back on the reviewed tab its own proposal is live, and applying it
    // writes there.
    app.press("text:Open first")
        .expect("back to the reviewed document");
    app.wait_for("text:Hunks: 2")
        .expect("the late answer's hunks are on their own tab");
    app.press("text:Approve first hunk")
        .expect("accept the first hunk on its own tab");
    let revised = "[Revised\nUnchanged\n][Original\nUnchanged\nMore\n]";
    app.wait_for(&format!("text:{revised}"))
        .expect("the accepted hunk entered the reviewed buffer");

    // Sending a new Review of the reviewed tab is what resets its Revision,
    // and it writes neither document.
    app.press("text:Review")
        .expect("prepare the reviewed tab's next Review");
    app.press("text:Send Review")
        .expect("send the reviewed tab's Review");
    app.wait_for("text:Hunks: 0")
        .expect("the sent Review cleared the tab's Revision");
    app.wait(Duration::from_millis(1500))
        .expect("the new Review's answer lands");
    assert_eq!(std::fs::read_to_string(&document).unwrap(), source);
    assert_eq!(std::fs::read_to_string(&other).unwrap(), other_source);
}

// A Revision prepared on A and sent from B goes out under A's id and its
// answer lands in A's session, never B's: switching tabs between prepare and
// send cannot redirect it.
#[test]
fn a_prepared_revision_lands_on_the_tab_that_prepared_it() {
    let _guard = TEST_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let source = "Original\nUnchanged\n";
    let other_source = "Original\nUnchanged\nMore\n";
    let (review_output, revision_output) = two_change_outputs(source);
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind loopback");
    let base_url = format!("http://{}/v1/", listener.local_addr().unwrap());
    let (release, released) = std::sync::mpsc::channel::<()>();
    std::thread::spawn(move || {
        for _ in 0..3 {
            let Ok((mut stream, _)) = listener.accept() else {
                return;
            };
            let body = read_http_body(&mut stream);
            let request: serde_json::Value =
                serde_json::from_slice(&body).unwrap_or(serde_json::Value::Null);
            if request["stream"].as_bool().unwrap_or(false) {
                let _ = released.recv();
                write_sse(&mut stream, &revision_output);
            } else {
                write_review_json(&mut stream, &review_output);
            }
        }
    });
    let directory = tempfile::tempdir().expect("document dir");
    let config = tempfile::tempdir().expect("config dir");
    let document = directory.path().join("plan.md");
    let other = directory.path().join("other.md");
    std::fs::write(&document, source).expect("write document");
    std::fs::write(&other, other_source).expect("write other document");
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
        std::env::set_var("MARKTURBO_OTHER", &other);
        std::env::set_var("MARKTURBO_CONFIG_DIR", config.path());
        std::env::set_var("MARKTURBO_DATA_DIR", config.path().join("data"));
        std::env::set_var(
            "MARKTURBO_TEST_LOCK_NAMESPACE",
            std::process::id().to_string(),
        );
        std::env::set_var("OPENAI_API_KEY", "loopback-test-key");
    }
    let mut app = Headless::start(EngineConfig::new(Root::of::<RevisionAcrossDocuments>()))
        .expect("start the two-document controls");
    app.wait_for("text:Tabs: 1").expect("document loaded");

    // A reviews; B reviews too, so B has a Revision context of its own.
    app.press("text:Review").expect("prepare A's Review");
    app.press("text:Send Review").expect("send A's Review");
    app.wait_for_within("text:Review ready", Duration::from_secs(20))
        .expect("A ready");
    app.press("text:Open other").expect("open B");
    app.wait_for("text:Tabs: 2").expect("B open");
    app.press("text:Review").expect("prepare B's Review");
    app.press("text:Send Review").expect("send B's Review");
    app.wait_for_within("text:Review ready", Duration::from_secs(20))
        .expect("B ready");

    // Back on A, prepare its Revision; the consent waits. Then switch to B
    // and send it from there: B has a Revision context, A's is the one that
    // must receive the answer.
    app.press("text:Open first").expect("back to A");
    app.press("text:Prepare Revision")
        .expect("prepare A's Revision");
    app.press("text:Open other").expect("switch to B");
    app.wait_for("text:Tabs: 2").expect("B shown");
    app.press("text:Send Revision")
        .expect("send the prepared Revision");
    assert!(
        app.shows("Busy: false") && app.shows("Hunks: 0"),
        "the sent request belongs to A, not B: {:?}",
        app.screen().texts()
    );
    app.press("text:Open first").expect("back to A");
    app.wait_for("text:Busy: true")
        .expect("A owns the sent request");
    release.send(()).expect("release the Revision answer");
    app.wait_for_within("text:Busy: false", Duration::from_secs(20))
        .expect("the Revision finished");
    app.wait_for_within("text:Hunks: 2", Duration::from_secs(20))
        .expect("A's own Revision is live");
    app.wait(Duration::from_millis(300))
        .expect("the answer settles");

    // B, the other tab, has nothing of the answer.
    app.press("text:Open other").expect("back to B");
    assert!(
        app.shows("Hunks: 0"),
        "B took A's answer: {:?}",
        app.screen().texts()
    );
    // Neither document changed: nothing was decided.
    let both = format!("[{source}][{other_source}]");
    assert!(
        app.screen()
            .matches(&format!("text:{both}"))
            .expect("the documents selector"),
        "neither document changes: {:?}",
        app.screen().texts()
    );
    assert_eq!(std::fs::read_to_string(&document).unwrap(), source);
    assert_eq!(std::fs::read_to_string(&other).unwrap(), other_source);
}

/// Serve one Review and one Revision on loopback: the review with no
/// findings and the revision of two changes, both over `source`. Its base
/// URL.
fn serve_review_and_revision(source: &str) -> String {
    let (review_output, revision_output) = two_change_outputs(source);
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind loopback");
    let base_url = format!("http://{}/v1/", listener.local_addr().unwrap());
    std::thread::spawn(move || {
        for _ in 0..2 {
            let Ok((mut stream, _)) = listener.accept() else {
                return;
            };
            let body = read_http_body(&mut stream);
            let request: serde_json::Value =
                serde_json::from_slice(&body).unwrap_or(serde_json::Value::Null);
            if request["stream"].as_bool().unwrap_or(false) {
                write_sse(&mut stream, &revision_output);
            } else {
                write_review_json(&mut stream, &review_output);
            }
        }
    });
    base_url
}

// While any consent card is open it is modal in full: the Revision keys are
// inert under the Translation card, and nothing of it reaches the proposal.
#[test]
fn revision_keys_are_inert_under_a_translation_card() {
    let _guard = TEST_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let source = "Original\nUnchanged\n";
    let base_url = serve_review_and_revision(source);
    let (_document, _directory, _config) = revision_fixture(source, &base_url);
    let mut app =
        Headless::start(EngineConfig::new(Root::of::<mt_app::App>())).expect("start the app");
    app.set_viewport(nsh_core::Size::new(
        nsh_engine::Points(1280.0),
        nsh_engine::Points(800.0),
    ))
    .expect("wide viewport");
    app.wait_for("label:Markdown source")
        .expect("source editor");

    app.press("id:title-review").expect("prepare Review");
    app.wait_for("id:consent-card").expect("Review disclosure");
    support::hold_send(&mut app);
    app.wait_for_within("id:review-prepare", Duration::from_secs(20))
        .expect("Review ready");
    app.press("label:Prepare revision")
        .expect("prepare Revision");
    app.wait_for("text:Ask for a revision?")
        .expect("Revision disclosure");
    support::hold_send(&mut app);
    app.wait_for_within("id:revision-accept", Duration::from_secs(20))
        .expect("the hunks arrive");
    app.wait_for("text:0 of 2 decided")
        .expect("nothing is decided yet");

    // The Translation consent card opens over the proposal.
    app.press("id:title-settings").expect("open the menu");
    app.press("id:title-menu-translate")
        .expect("start Translation");
    app.wait_for("id:consent-card")
        .expect("Translation disclosure");
    app.wait_for("text:Send for translation?")
        .expect("the Translation card");

    // `A` is the card's to ignore: no decision, no buffer write.
    app.press_key("a").expect("press A under the card");
    app.wait(Duration::from_millis(200))
        .expect("the decision would land by now");
    assert!(
        app.shows("0 of 2 decided"),
        "A decided under the Translation card: {:?}",
        app.screen().texts()
    );
    assert_eq!(
        app.screen()
            .input_text("label:Markdown source")
            .expect("read the buffer"),
        Some(source),
        "A wrote the hunk under the Translation card"
    );

    // The card closes with its Cancel; nothing of it reaches the proposal.
    app.press("id:consent-cancel-button")
        .expect("cancel the Translation");
    app.wait_for_gone("id:consent-card", Duration::from_secs(2))
        .expect("the card closed");
    app.wait(Duration::from_millis(500))
        .expect("a late decision would land by now");
    assert!(
        app.shows("0 of 2 decided"),
        "a decision landed after the card closed: {:?}",
        app.screen().texts()
    );
    assert_eq!(
        app.screen()
            .input_text("label:Markdown source")
            .expect("read the buffer"),
        Some(source),
        "nothing wrote the hunk"
    );
}

// The per-tab store must never hand back a session older than the screen: a
// decision made before a later Review was prepared survives its cancel, and
// the panel's Undo still takes its text back. Closing the card also returns
// the keyboard to the source editor: a typed character lands in the buffer
// and decides nothing.
#[test]
fn a_decision_survives_the_cancel_of_a_later_review() {
    let _guard = TEST_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let source = "Original\nUnchanged\n";
    let base_url = serve_review_and_revision(source);
    let (_document, _directory, _config) = revision_fixture(source, &base_url);
    let mut app =
        Headless::start(EngineConfig::new(Root::of::<mt_app::App>())).expect("start the app");
    app.set_viewport(nsh_core::Size::new(
        nsh_engine::Points(1280.0),
        nsh_engine::Points(800.0),
    ))
    .expect("wide viewport");
    app.wait_for("label:Markdown source")
        .expect("source editor");

    app.press("id:title-review").expect("prepare Review");
    app.wait_for("id:consent-card").expect("Review disclosure");
    support::hold_send(&mut app);
    app.wait_for_within("id:review-prepare", Duration::from_secs(20))
        .expect("Review ready");
    app.press("label:Prepare revision")
        .expect("prepare Revision");
    app.wait_for("text:Ask for a revision?")
        .expect("Revision disclosure");
    support::hold_send(&mut app);
    app.wait_for_within("id:revision-accept", Duration::from_secs(20))
        .expect("the hunks arrive");
    app.wait_for("text:0 of 2 decided")
        .expect("nothing is decided yet");

    // Accept the docked hunk: the text and the decision are the screen's.
    app.press("label:Accept").expect("accept the docked hunk");
    app.wait_for("text:1 of 2 decided").expect("one decided");
    assert_eq!(
        app.screen()
            .input_text("label:Markdown source")
            .expect("read the buffer"),
        Some("Revised\nUnchanged\n"),
        "the accepted hunk entered the buffer"
    );

    // Select a line, start `Review selection` from the menu, then cancel the
    // disclosure: the parked Review comes back and the Revision must too.
    app.type_text("label:Markdown source", "")
        .expect("focus the editor");
    app.press_key("ctrl+home").expect("to the start");
    app.press_key("down").expect("down one line");
    app.press_key("shift+end").expect("select the line");
    app.press("id:title-settings").expect("open the menu");
    app.press("id:title-menu-review-selection")
        .expect("Review selection");
    app.wait_for("id:consent-card")
        .expect("the selection disclosure");
    app.press_key("escape").expect("cancel the new Review");
    app.wait_for_gone("id:consent-card", Duration::from_secs(2))
        .expect("the disclosure closed");

    // The decision is still the screen's, and the card's close returned the
    // keyboard to the source editor: `a` is a character in the buffer, not a
    // decision for the docked hunk below it.
    app.wait_for("text:1 of 2 decided")
        .expect("the decision survived the cancel");
    app.press_key("a").expect("type in the editor");
    app.wait(Duration::from_millis(200))
        .expect("a decision would land by now");
    assert!(
        app.shows("1 of 2 decided"),
        "`a` decided a hunk: {:?}",
        app.screen().texts()
    );
    let typed = app
        .screen()
        .input_text("label:Markdown source")
        .expect("read the buffer")
        .map(str::to_owned)
        .expect("the editor contains text");
    assert!(
        typed.contains('a') && typed != "Revised\nUnchanged\n",
        "the character never reached the source editor: {typed:?}"
    );

    // The decision history is independently the panel's: its Undo button
    // takes the pre-cancel accept back, not a decision `a` never made.
    app.press("id:revision-undo")
        .expect("press the panel's Undo");
    app.wait_for("text:0 of 2 decided")
        .expect("Undo took the decision back");
    assert_eq!(
        app.screen()
            .input_text("label:Markdown source")
            .expect("read the buffer"),
        Some(typed.replacen("Revised", "Original", 1).as_str()),
        "Undo restored the pre-cancel text and kept the typed character"
    );
}

// A Review answer for one tab cancels only that tab's Revision work: another
// tab's waiting Revision consent card survives the answer, and sending from
// that card still lands its hunks.
#[test]
fn a_background_reviews_answer_leaves_another_tabs_revision_card() {
    let _guard = TEST_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let source = "Original\nUnchanged\n";
    let (review_output, revision_output) = two_change_outputs(source);
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind loopback");
    let base_url = format!("http://{}/v1/", listener.local_addr().unwrap());
    let (release_answer, answer_waits) = std::sync::mpsc::channel::<()>();
    let server = std::thread::spawn(move || {
        let mut reviews = 0;
        for _ in 0..4 {
            let Ok((mut stream, _)) = listener.accept() else {
                return;
            };
            let body = read_http_body(&mut stream);
            let request: serde_json::Value =
                serde_json::from_slice(&body).unwrap_or(serde_json::Value::Null);
            if request["stream"].as_bool().unwrap_or(false) {
                write_sse(&mut stream, &revision_output);
            } else {
                reviews += 1;
                if reviews == 3 {
                    let _ = answer_waits.recv();
                }
                write_review_json(&mut stream, &review_output);
            }
        }
    });
    let directory = tempfile::tempdir().expect("document dir");
    let config = tempfile::tempdir().expect("config dir");
    let document = directory.path().join("plan.md");
    let other = directory.path().join("other.md");
    std::fs::write(&document, source).expect("write document");
    std::fs::write(&other, source).expect("write other document");
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

    // Both tabs get a Review result to build a Revision from.
    app.press("id:title-review").expect("prepare A's Review");
    app.wait_for("id:consent-card").expect("A's disclosure");
    support::hold_send(&mut app);
    app.wait_for_within("id:review-prepare", Duration::from_secs(20))
        .expect("A's result");
    app.press_key("mod+p").expect("open the palette");
    app.type_text("id:palette-input", "other")
        .expect("filter to other.md");
    app.press("id:palette-row").expect("open other.md");
    app.wait_for("id:title-file >> text:other.md")
        .expect("B's tab");
    app.press("id:title-review").expect("prepare B's Review");
    app.wait_for("id:consent-card").expect("B's disclosure");
    support::hold_send(&mut app);
    app.wait_for_within("id:review-prepare", Duration::from_secs(20))
        .expect("B's result");

    // A's next Review is waiting: switch to B and open B's Revision consent
    // card while the answer is still out.
    app.press_key("mod+p").expect("open the palette");
    app.type_text("id:palette-input", "plan")
        .expect("filter back to plan.md");
    app.press("id:palette-row").expect("back to A");
    app.wait_for("id:title-file >> text:plan.md")
        .expect("A's tab");
    app.press("id:title-review")
        .expect("prepare A's next Review");
    app.wait_for("id:consent-card").expect("the new disclosure");
    support::hold_send(&mut app);
    app.wait_for_gone("id:consent-card", Duration::from_secs(2))
        .expect("A's request is out");
    app.press_key("mod+p").expect("open the palette");
    app.type_text("id:palette-input", "other")
        .expect("filter to other.md");
    app.press("id:palette-row").expect("open other.md");
    app.wait_for("id:title-file >> text:other.md")
        .expect("B's tab");
    app.press("label:Prepare revision")
        .expect("prepare B's Revision");
    app.wait_for("text:Ask for a revision?")
        .expect("B's Revision card");

    // A's answer arrives while B's card waits: the card belongs to B and
    // stays open. The generation-check hook reports the answer's arrival.
    let (checked_tx, checked_rx) = std::sync::mpsc::channel::<()>();
    mt_app::test_vault::on_review_generation_checked(move || {
        let _ = checked_tx.send(());
    });
    release_answer.send(()).expect("release A's answer");
    checked_rx
        .recv_timeout(Duration::from_secs(20))
        .expect("A's answer reached the app");
    app.wait(Duration::from_millis(200))
        .expect("the answer lands");
    app.catch_up();
    assert!(
        app.shows("Ask for a revision?"),
        "A's answer closed B's Revision card: {:?}",
        app.screen().texts()
    );

    // Sending from B's surviving card still lands B's Revision.
    support::hold_send(&mut app);
    app.wait_for_within("id:revision-accept", Duration::from_secs(20))
        .expect("B's Revision answer still lands");
    app.wait_for("text:0 of 2 decided")
        .expect("B's hunks arrived");
    let _ = server.join();
    assert_eq!(
        std::fs::read_to_string(&document).unwrap(),
        source,
        "A's document is untouched"
    );
    assert_eq!(
        std::fs::read_to_string(&other).unwrap(),
        source,
        "B's document is untouched too"
    );
}

// An answer for one tab never cancels another tab's sent Revision: the
// transport stays live while the answer lands, and its hunks still arrive.
#[test]
fn a_background_reviews_answer_leaves_a_sent_revisions_result_alone() {
    let _guard = TEST_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let source = "Original\nUnchanged\n";
    let (review_output, revision_output) = two_change_outputs(source);
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind loopback");
    let base_url = format!("http://{}/v1/", listener.local_addr().unwrap());
    let (release_answer, answer_waits) = std::sync::mpsc::channel::<()>();
    let (release_revision, revision_waits) = std::sync::mpsc::channel::<()>();
    let server = std::thread::spawn(move || {
        let mut reviews = 0;
        for _ in 0..4 {
            let Ok((mut stream, _)) = listener.accept() else {
                return;
            };
            let body = read_http_body(&mut stream);
            let request: serde_json::Value =
                serde_json::from_slice(&body).unwrap_or(serde_json::Value::Null);
            if request["stream"].as_bool().unwrap_or(false) {
                let _ = revision_waits.recv();
                write_sse(&mut stream, &revision_output);
            } else {
                reviews += 1;
                if reviews == 3 {
                    let _ = answer_waits.recv();
                }
                write_review_json(&mut stream, &review_output);
            }
        }
    });
    let directory = tempfile::tempdir().expect("document dir");
    let config = tempfile::tempdir().expect("config dir");
    let document = directory.path().join("plan.md");
    let other = directory.path().join("other.md");
    std::fs::write(&document, source).expect("write document");
    std::fs::write(&other, source).expect("write other document");
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

    // Both tabs get a Review result to build a Revision from.
    app.press("id:title-review").expect("prepare A's Review");
    app.wait_for("id:consent-card").expect("A's disclosure");
    support::hold_send(&mut app);
    app.wait_for_within("id:review-prepare", Duration::from_secs(20))
        .expect("A's result");
    app.press_key("mod+p").expect("open the palette");
    app.type_text("id:palette-input", "other")
        .expect("filter to other.md");
    app.press("id:palette-row").expect("open other.md");
    app.wait_for("id:title-file >> text:other.md")
        .expect("B's tab");
    app.press("id:title-review").expect("prepare B's Review");
    app.wait_for("id:consent-card").expect("B's disclosure");
    support::hold_send(&mut app);
    app.wait_for_within("id:review-prepare", Duration::from_secs(20))
        .expect("B's result");

    // A's next Review waits; B's Revision is prepared and sent while it is
    // still out, so A's answer lands mid-transport.
    app.press_key("mod+p").expect("open the palette");
    app.type_text("id:palette-input", "plan")
        .expect("filter back to plan.md");
    app.press("id:palette-row").expect("back to A");
    app.wait_for("id:title-file >> text:plan.md")
        .expect("A's tab");
    app.press("id:title-review")
        .expect("prepare A's next Review");
    app.wait_for("id:consent-card").expect("the new disclosure");
    support::hold_send(&mut app);
    app.wait_for_gone("id:consent-card", Duration::from_secs(2))
        .expect("A's request is out");
    app.press_key("mod+p").expect("open the palette");
    app.type_text("id:palette-input", "other")
        .expect("filter to other.md");
    app.press("id:palette-row").expect("open other.md");
    app.wait_for("id:title-file >> text:other.md")
        .expect("B's tab");
    app.press("label:Prepare revision")
        .expect("prepare B's Revision");
    app.wait_for("text:Ask for a revision?")
        .expect("B's Revision card");
    support::hold_send(&mut app);
    app.wait_for("id:revision-busy")
        .expect("B's Revision is in flight");

    // A's answer arrives: the sent Revision must not be cancelled with it.
    let (checked_tx, checked_rx) = std::sync::mpsc::channel::<()>();
    mt_app::test_vault::on_review_generation_checked(move || {
        let _ = checked_tx.send(());
    });
    release_answer.send(()).expect("release A's answer");
    checked_rx
        .recv_timeout(Duration::from_secs(20))
        .expect("A's answer reached the app");
    app.wait(Duration::from_millis(200))
        .expect("the answer lands");
    app.catch_up();
    assert!(
        app.screen()
            .matches("id:revision-busy")
            .expect("the busy state"),
        "A's answer cancelled B's sent Revision: {:?}",
        app.screen().texts()
    );

    // The transport still delivers: releasing the answer lands the hunks.
    release_revision
        .send(())
        .expect("release the Revision answer");
    app.wait_for_within("id:revision-accept", Duration::from_secs(20))
        .expect("B's Revision answer still lands");
    app.wait_for("text:0 of 2 decided")
        .expect("B's hunks arrived");
    let _ = server.join();
}

#[test]
fn sending_another_tabs_review_leaves_a_sent_revision_alone() {
    let _guard = TEST_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let source = "Original\nUnchanged\n";
    let (review_output, revision_output) = two_change_outputs(source);
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind loopback");
    let base_url = format!("http://{}/v1/", listener.local_addr().unwrap());
    let (revision_started_tx, revision_started_rx) = std::sync::mpsc::sync_channel::<()>(1);
    let (release_revision, revision_waits) = std::sync::mpsc::channel::<()>();
    let (b_review_started_tx, b_review_started_rx) = std::sync::mpsc::sync_channel::<()>(1);
    let (release_b_review, b_review_waits) = std::sync::mpsc::channel::<()>();
    let (server_done_tx, server_done_rx) = std::sync::mpsc::channel::<()>();
    let server = std::thread::spawn(move || {
        let Some(mut stream) = accept_within(&listener, Duration::from_secs(20)) else {
            return;
        };
        read_http_body(&mut stream);
        write_review_json(&mut stream, &review_output);
        let Some(mut stream) = accept_within(&listener, Duration::from_secs(20)) else {
            return;
        };
        read_http_body(&mut stream);
        let _ = revision_started_tx.send(());
        let revision = std::thread::spawn(move || {
            let _ = revision_waits.recv_timeout(Duration::from_secs(30));
            write_sse(&mut stream, &revision_output);
        });
        let Some(mut stream) = accept_within(&listener, Duration::from_secs(20)) else {
            return;
        };
        read_http_body(&mut stream);
        let _ = b_review_started_tx.send(());
        let _ = b_review_waits.recv_timeout(Duration::from_secs(30));
        write_review_json(&mut stream, &review_output);
        let _ = revision.join();
        let _ = server_done_tx.send(());
    });
    let directory = tempfile::tempdir().expect("document dir");
    let config = tempfile::tempdir().expect("config dir");
    let document = directory.path().join("plan.md");
    let other = directory.path().join("other.md");
    std::fs::write(&document, source).expect("write document");
    std::fs::write(&other, source).expect("write other document");
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

    // A's Revision goes out first; the server holds its stream open.
    app.press("id:title-review").expect("prepare A's Review");
    app.wait_for("id:consent-card").expect("A's disclosure");
    support::hold_send(&mut app);
    app.wait_for_within("id:review-prepare", Duration::from_secs(20))
        .expect("A's result");
    app.press("label:Prepare revision")
        .expect("prepare A's Revision");
    app.wait_for("text:Ask for a revision?")
        .expect("A's Revision card");
    support::hold_send(&mut app);
    app.wait_for("id:revision-busy")
        .expect("A's Revision is in flight");
    revision_started_rx
        .recv_timeout(Duration::from_secs(20))
        .expect("A's Revision reached the server");

    // B is idle: A's Revision row does not follow the tab, and coming back
    // to A shows it again.
    app.press_key("mod+p").expect("open the palette");
    app.type_text("id:palette-input", "other")
        .expect("filter to other.md");
    app.press("id:palette-row").expect("open other.md");
    app.wait_for("id:title-file >> text:other.md")
        .expect("B's tab");
    app.wait_for_gone("id:revision-busy", Duration::from_secs(2))
        .expect("B never shows A's Revision row");
    app.press_key("mod+p").expect("open the palette");
    app.type_text("id:palette-input", "plan")
        .expect("filter back to plan.md");
    app.press("id:palette-row").expect("back to A");
    app.wait_for("id:title-file >> text:plan.md")
        .expect("A's tab");
    app.wait_for("id:revision-busy")
        .expect("A still owns its Revision");

    // B sends its Review while A's Revision still waits.
    app.press_key("mod+p").expect("open the palette");
    app.type_text("id:palette-input", "other")
        .expect("filter to other.md");
    app.press("id:palette-row").expect("open other.md");
    app.wait_for("id:title-file >> text:other.md")
        .expect("B's tab");
    app.press("id:title-review").expect("prepare B's Review");
    app.wait_for("id:consent-card").expect("B's disclosure");
    support::hold_send(&mut app);
    app.wait_for_gone("id:consent-card", Duration::from_secs(2))
        .expect("B's Review is out");
    b_review_started_rx
        .recv_timeout(Duration::from_secs(20))
        .expect("B's Review reached the server");

    // B shows its own waiting line, never A's Revision row; Escape cancels
    // B's Review and discards its late answer.
    app.wait_for_within("id:review-cancel", Duration::from_secs(5))
        .expect("B's own Review waiting line");
    app.wait_for_gone("id:revision-busy", Duration::from_secs(2))
        .expect("A's Revision row is not B's");
    app.press_key("escape").expect("cancel B's Review");
    app.wait_for_gone("id:review-field", Duration::from_secs(2))
        .expect("B's cancelled wait closed");
    let (b_checked_tx, b_checked_rx) = std::sync::mpsc::channel::<()>();
    mt_app::test_vault::on_review_generation_checked(move || {
        let _ = b_checked_tx.send(());
    });
    release_b_review.send(()).expect("release B's answer");
    b_checked_rx
        .recv_timeout(Duration::from_secs(20))
        .expect("B's late answer reached its generation check");
    app.catch_up();
    let ids: Vec<String> = app
        .screen()
        .layout_nodes()
        .iter()
        .filter_map(|node| node.test_id.as_deref().map(str::to_owned))
        .collect();
    assert!(
        !ids.iter().any(|id| id == "review-prepare"),
        "the cancelled Review must never land: {ids:?}"
    );
    // Nor is it stored: `Review` starts a new request instead of reopening a
    // result the cancel discarded.
    app.press("id:title-review")
        .expect("Review after B's late answer");
    app.wait_for_within("id:consent-card", Duration::from_secs(5))
        .expect("the discarded result cannot be reopened; a new Review starts instead");
    assert!(
        !app.shows("keep the document intent"),
        "nothing of the late result is shown"
    );
    app.press("id:consent-cancel-button")
        .expect("cancel the new consent");
    app.wait_for_gone("id:consent-card", Duration::from_secs(2))
        .expect("the consent closed");

    // A's release must still land on A, under its own name.
    release_revision
        .send(())
        .expect("release A's Revision answer");
    app.press_key("mod+p").expect("open the palette");
    app.type_text("id:palette-input", "plan")
        .expect("filter back to plan.md");
    app.press("id:palette-row").expect("back to A");
    app.wait_for("id:title-file >> text:plan.md")
        .expect("A's tab");
    app.wait_for_within("id:revision-accept", Duration::from_secs(20))
        .expect("A's Revision still lands");
    app.wait_for("text:0 of 2 decided")
        .expect("A's hunks arrived");
    server_done_rx
        .recv_timeout(Duration::from_secs(20))
        .expect("the server finished");
    let _ = server.join();
}

// A Revision on one document blocks `Prepare revision` on another, with the
// reason visible; closing the first tab cancels its request and frees the
// slot for the next one.
#[test]
fn a_revision_on_another_document_waits_and_closing_its_tab_frees_it() {
    let _guard = TEST_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let source = "Original\nUnchanged\n";
    let (review_output, revision_output) = two_change_outputs(source);
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind loopback");
    let base_url = format!("http://{}/v1/", listener.local_addr().unwrap());
    let (revision_started_tx, revision_started_rx) = std::sync::mpsc::sync_channel::<()>(1);
    let (release_revision, revision_waits) = std::sync::mpsc::channel::<()>();
    let server = std::thread::spawn(move || {
        // A's Review answers at once.
        let Some(mut stream) = accept_within(&listener, Duration::from_secs(20)) else {
            return;
        };
        read_http_body(&mut stream);
        write_review_json(&mut stream, &review_output);
        // A's Revision is held open.
        let Some(mut stream) = accept_within(&listener, Duration::from_secs(20)) else {
            return;
        };
        read_http_body(&mut stream);
        let _ = revision_started_tx.send(());
        let revision = std::thread::spawn(move || {
            let _ = revision_waits.recv_timeout(Duration::from_secs(60));
            write_sse(&mut stream, &revision_output);
        });
        // B's Review answers while the Revision still waits.
        let Some(mut stream) = accept_within(&listener, Duration::from_secs(20)) else {
            return;
        };
        read_http_body(&mut stream);
        write_review_json(&mut stream, &review_output);
        let _ = revision.join();
    });
    let directory = tempfile::tempdir().expect("document dir");
    let config = tempfile::tempdir().expect("config dir");
    let document = directory.path().join("plan.md");
    let other = directory.path().join("other.md");
    std::fs::write(&document, source).expect("write document");
    std::fs::write(&other, source).expect("write other document");
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

    // A's Review, then A's Revision: the server holds the Revision open.
    app.press("id:title-review").expect("prepare A's Review");
    app.wait_for("id:consent-card").expect("A's disclosure");
    support::hold_send(&mut app);
    app.wait_for_within("id:review-prepare", Duration::from_secs(20))
        .expect("A's result");
    app.press("label:Prepare revision")
        .expect("prepare A's Revision");
    app.wait_for("text:Ask for a revision?")
        .expect("A's Revision card");
    support::hold_send(&mut app);
    app.wait_for("id:revision-busy")
        .expect("A's Revision is in flight");
    revision_started_rx
        .recv_timeout(Duration::from_secs(20))
        .expect("A's Revision reached the server");

    // B gets its own Review result; `Prepare revision` waits, with the reason.
    app.press_key("mod+p").expect("open the palette");
    app.type_text("id:palette-input", "other")
        .expect("filter to other.md");
    app.press("id:palette-row").expect("open other.md");
    app.wait_for("id:title-file >> text:other.md")
        .expect("B's tab");
    app.press("id:title-review").expect("prepare B's Review");
    app.wait_for("id:consent-card").expect("B's disclosure");
    support::hold_send(&mut app);
    app.wait_for_within("id:review-prepare", Duration::from_secs(20))
        .expect("B's result");
    app.wait_for_within(
        "text:Waiting for Revision on another document.",
        Duration::from_secs(5),
    )
    .expect("the reason is shown");
    assert!(
        app.screen()
            .accessibility_state("id:review-prepare")
            .expect("Prepare revision's state")
            .disabled,
        "Prepare revision is disabled while another document's Revision is in flight"
    );

    // Closing A cancels its Revision and frees the slot for B.
    app.press_key("mod+p").expect("open the palette");
    app.type_text("id:palette-input", "plan")
        .expect("filter back to plan.md");
    app.press("id:palette-row").expect("back to A");
    app.wait_for("id:title-file >> text:plan.md")
        .expect("A's tab");
    app.press_key("mod+w").expect("close A");
    app.wait_for("id:title-file >> text:other.md")
        .expect("B is active again");
    app.wait_for_gone(
        "text:Waiting for Revision on another document.",
        Duration::from_secs(2),
    )
    .expect("the reason is gone");
    assert!(
        !app.screen()
            .accessibility_state("id:review-prepare")
            .expect("Prepare revision's state")
            .disabled,
        "the closed tab freed the slot"
    );
    app.press("label:Prepare revision")
        .expect("prepare B's Revision");
    app.wait_for_within("text:Ask for a revision?", Duration::from_secs(5))
        .expect("the slot is free");
    app.press("id:consent-cancel-button")
        .expect("cancel the Revision consent");
    app.wait_for_gone("id:consent-card", Duration::from_secs(2))
        .expect("the consent closed");
    release_revision
        .send(())
        .expect("release A's Revision answer");
    let _ = server.join();
}

/// Whether the buffer line reads `text` exactly (`shows` trims the final
/// newline the buffer keeps).
fn buffer_shows(app: &Headless, text: &str) -> bool {
    app.screen()
        .matches(&format!("text:Buffer: {text}"))
        .expect("the buffer selector")
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

/// Accept one loopback connection, or `None` after `timeout`: a server that
/// waits forever turns a missing request into a hung test.
fn accept_within(listener: &TcpListener, timeout: Duration) -> Option<TcpStream> {
    listener
        .set_nonblocking(true)
        .expect("nonblocking listener");
    let deadline = std::time::Instant::now() + timeout;
    loop {
        match listener.accept() {
            Ok((stream, _)) => {
                stream.set_nonblocking(false).expect("blocking stream");
                return Some(stream);
            }
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                if std::time::Instant::now() >= deadline {
                    return None;
                }
                std::thread::sleep(Duration::from_millis(10));
            }
            Err(_) => return None,
        }
    }
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
