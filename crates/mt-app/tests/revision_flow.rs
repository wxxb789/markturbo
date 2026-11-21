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
    app.long_press("id:consent-send").expect("send review");
    app.wait_for_within("id:review-prepare", Duration::from_secs(20))
        .expect("review ready");

    // Prepare revision is the result's one next action.
    app.press("label:Prepare revision")
        .expect("prepare revision");
    app.wait_for("text:Ask for a revision?")
        .expect("revision disclosure names the revision");
    app.long_press("id:consent-send").expect("send revision");
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
    app.wait_for("text:Approved changes applied to the buffer.")
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
    let directory = tempfile::tempdir().expect("document dir");
    let config = tempfile::tempdir().expect("config dir");
    let document = directory.path().join("plan.md");
    std::fs::write(&document, source).expect("write document");
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
    app.wait_for("text:Approved changes applied to the buffer.")
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
    app.wait_for("text:No changes approved.")
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
    app.wait_for("text:Can't undo — this part was edited after it was accepted.")
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

/// What `Replace the document` types over the whole buffer.
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
    app.wait_for("text:Can't apply — this part changed")
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
            && app.shows("Can't apply — this part changed")
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
    app.wait_for("text:No changes approved.")
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

/// A Revision answer that arrives after its Review was reset (here by a
/// Review of another document) belongs to no live proposal: it shows no
/// hunks, and nothing of it can be accepted into either document.
#[test]
fn a_revision_answer_after_its_review_was_reset_writes_nothing() {
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
        for _ in 0..2 {
            let Ok((mut stream, _)) = listener.accept() else {
                return;
            };
            let body = read_http_body(&mut stream);
            let request: serde_json::Value =
                serde_json::from_slice(&body).unwrap_or(serde_json::Value::Null);
            if request["stream"].as_bool().unwrap_or(false) {
                // Held back until the test has reset the Review.
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

    // Switch to the other document and review it: the first Review and its
    // Revision are reset while the answer is still held back.
    app.press("text:Open other")
        .expect("open the other document");
    app.wait_for("text:Tabs: 2")
        .expect("the other document is open");
    app.press("text:Review").expect("review the other document");
    app.wait_for("text:Busy: false")
        .expect("the reset clears the loading state");

    release.send(()).expect("release the Revision answer");
    answer_sent
        .recv_timeout(Duration::from_secs(20))
        .expect("the answer is sent");
    app.wait(Duration::from_millis(1500))
        .expect("the late answer lands");
    assert!(
        app.shows("Hunks: 0") && app.shows("Busy: false"),
        "a late answer to a reset Review presents nothing: {:?}",
        app.screen().texts()
    );
    app.press("text:Approve first hunk")
        .expect("try to accept a late hunk");
    app.wait(Duration::from_millis(300)).expect("settle");
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
