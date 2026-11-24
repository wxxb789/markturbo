//! `Ctrl+Enter` from the focused source editor runs the app's command for the
//! state on screen — `Review`, the Revision field's `Done`, the consent
//! card's hold — and never edits the text. The editor binds `mod+enter`
//! (insert a line below) every time it takes the focus, so these run on
//! Windows, where the keymap decides the key before the focused field, and
//! move the focus away and back.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::Mutex;
use std::time::Duration;

use nsh_core::element::{Idiom, Os};
use nsh_engine::{EngineConfig, Root};
use nsh_testing::Headless;

static TEST_LOCK: Mutex<()> = Mutex::new(());

const SOURCE: &str = "Original\nUnchanged\n";
const EDITOR: &str = "label:Markdown source";

/// The app on `SOURCE`, on Windows, with a loopback provider that answers a
/// Review with no findings and a Revision of two changes; on the system's
/// clock, or (`held_keys`) on a manual one the test moves, under reduced
/// motion. The temporary directories live as long as the returned guards.
fn start(held_keys: bool) -> (Headless, tempfile::TempDir, tempfile::TempDir) {
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
    })
    .to_string();
    let revision = serde_json::json!({
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
    })
    .to_string();
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind loopback");
    let base_url = format!("http://{}/v1/", listener.local_addr().unwrap());
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else {
                return;
            };
            let body = read_http_body(&mut stream);
            let request: serde_json::Value =
                serde_json::from_slice(&body).unwrap_or(serde_json::Value::Null);
            if request["stream"].as_bool().unwrap_or(false) {
                write_sse(&mut stream, &revision);
            } else {
                write_review_json(&mut stream, &review);
            }
        }
    });
    let directory = tempfile::tempdir().expect("document dir");
    let config = tempfile::tempdir().expect("config dir");
    let document = directory.path().join("plan.md");
    std::fs::write(&document, SOURCE).expect("write document");
    let endpoint = mt_core::model::EndpointIdentity::parse(
        mt_core::model::Provider::OpenAiChat,
        Some(&base_url),
    )
    .expect("endpoint");
    let identity = endpoint.credential_target();
    std::fs::write(
        config.path().join("settings.toml"),
        format!(
            "model-provider = \"openai-chat\"\nmodel-name = \"review-model\"\nmodel-base-url = \"{base_url}\"\nmodel-environment-key-identity = \"{identity}\"\n"
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
    // Synthetic keys never reach the platform's key state or thread hook: the
    // harness drives the hold's keyboard source as physically held.
    mt_app::ui::consent::test_harness::held_keys();
    let mut engine = EngineConfig::new(Root::of::<mt_app::App>());
    engine.environment.os = Os::Windows;
    engine.environment.idiom = Idiom::Desktop;
    if held_keys {
        engine.clock = nsh_core::time::Clock::manual(std::time::SystemTime::now());
        engine.environment.reduce_motion = true;
    }
    let mut app = Headless::start(engine).expect("start the app");
    app.set_viewport(nsh_core::Size::new(
        nsh_engine::Points(1440.0),
        nsh_engine::Points(900.0),
    ))
    .expect("viewport");
    app.wait_for(EDITOR).expect("the source editor");
    app.settle();
    (app, directory, config)
}

/// The keyboard focus into the source editor, as a click into it gives it:
/// nothing typed.
fn focus_editor(app: &mut Headless) {
    app.type_text(EDITOR, "").expect("focus the editor");
    app.settle();
}

/// The focus out of the editor (into the palette's line) and back.
fn blur_and_refocus_editor(app: &mut Headless) {
    app.press_key("mod+p").expect("open the palette");
    app.type_text("id:palette-input", "")
        .expect("the focus moves to the palette's line");
    app.press_key("escape").expect("close the palette");
    app.wait_for_gone("id:palette", Duration::from_secs(2))
        .expect("the palette closed");
    focus_editor(app);
}

/// The editor's lines as drawn (the gutter's line numbers left out).
fn editor_lines(app: &Headless) -> Vec<String> {
    let all = app
        .screen()
        .count("id:source-editor >> text:/.*/")
        .expect("line selector");
    let numbers = app
        .screen()
        .count(r"id:source-editor >> text:/^\d+$/")
        .expect("gutter selector");
    // Each line the fixtures can hold, in the fixtures' order, then the
    // empty lines; every drawn line must be one of them.
    let mut lines = Vec::new();
    for line in SOURCE_LINES {
        if app
            .screen()
            .matches(&format!("id:source-editor >> text:/^{line}$/"))
            .expect("line selector")
        {
            lines.push(line.to_string());
        }
    }
    let empty = app
        .screen()
        .count("id:source-editor >> text:/^$/")
        .expect("empty-line selector");
    lines.extend(std::iter::repeat_n(String::new(), empty));
    assert_eq!(
        lines.len(),
        all - numbers,
        "every drawn line is one the fixtures know: {:?}",
        app.screen().texts()
    );
    lines
}

/// The texts a line of the fixtures can hold.
const SOURCE_LINES: [&str; 4] = ["Original", "Revised", "Unchanged", "Changed"];

/// `text`'s lines as the editor draws them: the last line is the empty one
/// after the final line break.
fn lines(text: &str) -> Vec<String> {
    text.split('\n').map(str::to_owned).collect()
}

fn cancel_card(app: &mut Headless) {
    app.press("id:consent-cancel-button").expect("press Cancel");
    app.wait_for_gone("id:consent-card", Duration::from_secs(2))
        .expect("the card closed");
}

fn close_card_with_key(app: &mut Headless, key: &str) {
    app.press_key(key).expect("a key");
    assert!(
        app.wait_for_gone("id:consent-card", Duration::from_secs(2))
            .is_ok(),
        "{key} cancels the card opened from the editor: {:?}",
        app.screen().texts()
    );
}

fn is_dirty(app: &Headless) -> bool {
    app.screen()
        .matches("id:title-dirty")
        .expect("dirty selector")
}

/// `Ctrl+Enter` with the editor focused, then `Review` must be on screen
/// and the text exactly as it was.
fn ctrl_enter_reviews(app: &mut Headless, what: &str) {
    let lines = editor_lines(app);
    let dirty = is_dirty(app);
    app.press_key("ctrl+enter").expect("ctrl+enter");
    assert!(
        app.wait_for_within("id:consent-card", Duration::from_secs(2))
            .is_ok(),
        "{what}: Ctrl+Enter from the focused editor starts Review: {:?}",
        app.screen().texts()
    );
    assert_eq!(editor_lines(app), lines, "{what}: the text is untouched");
    assert_eq!(is_dirty(app), dirty, "{what}: the dirty flag is untouched");
}

#[test]
fn ctrl_enter_from_the_focused_editor_starts_review_after_every_refocus() {
    let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let (mut app, _directory, _config) = start(false);
    focus_editor(&mut app);
    assert_eq!(editor_lines(&app), lines(SOURCE), "the document is loaded");
    assert!(!is_dirty(&app), "a focus edits nothing");
    ctrl_enter_reviews(&mut app, "first focus");
    assert_eq!(editor_lines(&app), lines(SOURCE));
    assert!(!is_dirty(&app));

    // The card opened from the editor answers its own keys: Escape and
    // Enter cancel it (the editor's Enter would insert a line), as Cancel
    // does. The editor's keys are bound again on every focus, and the
    // shortcut must stay the app's after each.
    close_card_with_key(&mut app, "escape");
    for (cycle, key) in ["enter", ""].into_iter().enumerate() {
        blur_and_refocus_editor(&mut app);
        ctrl_enter_reviews(&mut app, &format!("refocus {cycle}"));
        if key.is_empty() {
            cancel_card(&mut app);
        } else {
            close_card_with_key(&mut app, key);
        }
    }
    assert_eq!(
        editor_lines(&app),
        lines(SOURCE),
        "no line was ever inserted"
    );
    assert!(!is_dirty(&app), "and the document is still clean");
}

#[test]
fn ctrl_enter_from_the_focused_editor_is_done_while_hunks_are_docked() {
    let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let (mut app, _directory, _config) = start(false);
    app.press("id:title-review").expect("start review");
    app.wait_for("id:consent-card").expect("review disclosure");
    app.long_press("id:consent-send").expect("send review");
    app.wait_for_within("id:review-prepare", Duration::from_secs(20))
        .expect("review ready");
    app.press("label:Prepare revision")
        .expect("prepare revision");
    app.wait_for("text:Ask for a revision?")
        .expect("revision disclosure");
    app.long_press("id:consent-send").expect("send revision");
    app.wait_for_within("id:revision-accept", Duration::from_secs(20))
        .expect("the hunks arrive");
    app.press("label:Accept").expect("accept the first hunk");
    app.wait_for("text:1 of 2 decided").expect("one decided");
    let decided = lines("Revised\nUnchanged\n");
    assert_eq!(editor_lines(&app), decided, "the accepted hunk is written");

    focus_editor(&mut app);
    blur_and_refocus_editor(&mut app);
    app.press_key("ctrl+enter").expect("ctrl+enter");
    assert!(
        app.wait_for("text:/^Approved changes applied to the document/")
            .is_ok(),
        "Ctrl+Enter from the focused editor is Done: {:?}",
        app.screen().texts()
    );
    assert_eq!(
        editor_lines(&app),
        decided,
        "only the decided hunk changed the text: {:?}",
        app.screen().texts()
    );
}

#[test]
fn a_held_ctrl_enter_from_the_refocused_editor_charges_the_consent_hold() {
    let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    // A manual clock: the hold's watchdogs run only as far as the test moves
    // it, so presses can arrive inside them as a held key's repeats do (the
    // harness reports no auto-repeat). Reduced motion keeps the charge's
    // drawing off the frame clock (the same hold, watchdogs and send: only
    // its drawing steps), so a key press settles at once.
    let (mut app, _directory, _config) = start(true);
    app.press("id:title-review").expect("start review");
    app.wait_for("id:consent-card").expect("review disclosure");
    // The editor takes the focus while the card is up (it binds its keys
    // again then).
    focus_editor(&mut app);

    // A tap charges and cools: nothing is sent, the card stays.
    app.press_key("ctrl+enter").expect("tap ctrl+enter");
    app.run_for(Duration::from_millis(1_200))
        .expect("past the release watchdog");
    assert!(
        app.shows("Nothing was sent."),
        "a tap charges the hold and its early release says so: {:?}",
        app.screen().texts()
    );
    assert!(app.shows("Send for review?"), "the card stayed up");
    assert!(
        !app.screen()
            .matches("text:/^Sent · /")
            .expect("the selector"),
        "a tap authorizes nothing"
    );
    assert_eq!(editor_lines(&app), lines(SOURCE), "the tap edited nothing");
    app.run_for(Duration::from_millis(2_000))
        .expect("the notice ends");

    // A held key: presses inside the release watchdog (as its auto-repeats
    // come) carry the charge on until it completes and the Review is sent.
    for _ in 0..15 {
        app.press_key("ctrl+enter").expect("held ctrl+enter");
        app.run_for(Duration::from_millis(100))
            .expect("the next repeat");
    }
    assert!(
        app.screen()
            .matches("text:/^Sent · /")
            .expect("the selector"),
        "the held key sent the Review: {:?}",
        app.screen().texts()
    );
    assert_eq!(editor_lines(&app), lines(SOURCE), "the hold edited nothing");
    assert!(!is_dirty(&app), "and the document is still clean");
}

#[test]
fn a_refocus_in_the_middle_of_a_ctrl_enter_hold_keeps_one_tap_from_sending() {
    let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    // The same manual clock and reduced motion as the held-key test above.
    let (mut app, _directory, _config) = start(true);
    app.press("id:title-review").expect("start review");
    app.wait_for("id:consent-card").expect("review disclosure");
    focus_editor(&mut app);

    // A press from the editor, then the focus leaves the editor and comes
    // back while the key would still count as held: the binding that took
    // the press is bound again.
    app.press_key("ctrl+enter").expect("press ctrl+enter");
    app.run_for(Duration::from_millis(100))
        .expect("inside the release watchdog");
    blur_and_refocus_editor(&mut app);

    // The press was let go: the hold says so and cools, and the card stays.
    app.run_for(Duration::from_millis(1_200))
        .expect("past the release watchdog");
    assert!(
        app.shows("Nothing was sent."),
        "the interrupted press is released as a tap: {:?}",
        app.screen().texts()
    );
    app.run_for(Duration::from_millis(2_000))
        .expect("the notice ends");

    // One tap after that authorizes nothing.
    app.press_key("ctrl+enter").expect("tap ctrl+enter");
    app.run_for(Duration::from_millis(50)).expect("the tap");
    assert!(
        !app.screen()
            .matches("text:/^Sent · /")
            .expect("the selector"),
        "one tap sends nothing: {:?}",
        app.screen().texts()
    );
    assert!(app.shows("Send for review?"), "the card stayed up");
    assert_eq!(editor_lines(&app), lines(SOURCE), "nothing edited the text");
}

#[test]
fn a_ctrl_release_and_repress_mid_hold_sends_nothing_from_the_editor() {
    let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    // The same manual clock and reduced motion as the held-key test above.
    let (mut app, _directory, _config) = start(true);
    app.press("id:title-review").expect("start review");
    app.wait_for("id:consent-card").expect("review disclosure");
    focus_editor(&mut app);
    let keys = mt_app::ui::consent::test_harness::held_keys();

    // A held Ctrl+Enter: the control key comes up at 548 ms and back down at
    // 556 ms while Enter stays down, with the editor's text input focused.
    app.press_key("ctrl+enter").expect("press ctrl+enter");
    app.run_for(Duration::from_millis(548)).expect("to 548 ms");
    keys.transition();
    app.run_for(Duration::from_millis(8)).expect("to 556 ms");
    app.press_key("ctrl+enter")
        .expect("the repeat after the re-press");
    app.run_for(Duration::from_millis(400))
        .expect("past the first hold's 900 ms");
    assert!(
        !app.screen()
            .matches("text:/^Sent · /")
            .expect("the selector"),
        "a broken hold cannot authorize: {:?}",
        app.screen().texts()
    );
    assert!(app.shows("Send for review?"), "the card stayed up");
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
    let mut events = String::from("data: ");
    events.push_str(
        &serde_json::json!({
            "id": "chatcmpl-revision",
            "object": "chat.completion.chunk",
            "model": "review-model",
            "choices": [{"index": 0, "delta": {"content": output}, "finish_reason": null}]
        })
        .to_string(),
    );
    events.push_str("\n\ndata: [DONE]\n\n");
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
