//! The keyboard and the title row, as a user meets them on Windows:
//!
//! * the `Ctrl+Enter` that opens the consent card never sends it, however
//!   long it is held; a fresh hold after a release does;
//! * `Ctrl+S` saves the active document from the focused editor;
//! * the title row's `Review` is underlined, `Settings` is a word that says
//!   whether its menu is open, the menu names its shortcuts, and the view
//!   button names its current and next views.

mod support;

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::Mutex;
use std::time::Duration;

use nsh_core::{
    EventKind, EventPayload, KeyEvent,
    element::{Idiom, Os, SharedString},
    keymap::Modifiers,
};
use nsh_engine::{EngineConfig, Points, Root};
use nsh_testing::Headless;

static TEST_LOCK: Mutex<()> = Mutex::new(());

const SOURCE: &str = "Original\n";
const EDITOR: &str = "label:Markdown source";

/// The app on `plan.md`, on `os`. With `provider` the settings point Review
/// at a loopback provider that answers with no findings, and the app runs on
/// a manual clock under reduced motion, so the hold's charge runs only as far
/// as the test moves it. The directories live as long as the returned guards.
fn start(provider: bool, os: Os) -> (Headless, tempfile::TempDir, tempfile::TempDir) {
    let directory = tempfile::tempdir().expect("document dir");
    let config = tempfile::tempdir().expect("config dir");
    let document = directory.path().join("plan.md");
    std::fs::write(&document, SOURCE).expect("write document");
    if provider {
        let base_url = loopback_provider();
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
    }
    // SAFETY: the tests in this binary serialize app lifetimes and
    // environment changes behind `TEST_LOCK`.
    unsafe {
        std::env::set_var("MARKTURBO_OPEN", &document);
        std::env::set_var("MARKTURBO_CONFIG_DIR", config.path());
        std::env::set_var("MARKTURBO_DATA_DIR", config.path().join("data"));
        std::env::set_var(
            "MARKTURBO_TEST_LOCK_NAMESPACE",
            std::process::id().to_string(),
        );
        std::env::set_var("CODEX_HOME", config.path().join("codex"));
        std::env::set_var("OPENAI_API_KEY", "loopback-test-key");
    }
    let mut engine = EngineConfig::new(Root::of::<mt_app::App>());
    engine.environment.os = os;
    engine.environment.idiom = Idiom::Desktop;
    if provider {
        engine.clock = nsh_core::time::Clock::manual(std::time::SystemTime::now());
        engine.environment.reduce_motion = true;
    }
    let mut app = Headless::start(engine).expect("start the app");
    app.set_viewport(nsh_core::Size::new(Points(1440.0), Points(900.0)))
        .expect("viewport");
    app.wait_for(EDITOR).expect("the source editor");
    app.settle();
    (app, directory, config)
}

fn sent(app: &Headless) -> bool {
    app.screen()
        .matches("text:/^Sent · /")
        .expect("the selector")
}

/// One raw `KeyDown`/`KeyUp` on the focused card, as the platform's
/// focused-view delivery sends it (the headless backend delivers raw keys on
/// `Os::Linux`, as the Windows host does for a focused custom view).
fn card_key(app: &mut Headless, key: &str, down: bool, modifiers: Modifiers, repeat: bool) {
    let id = app.screen().id("id:consent-card").expect("the card");
    let mut event = KeyEvent::default();
    event.key = SharedString::from(key.to_owned());
    event.modifiers = modifiers;
    event.repeat = repeat;
    let engine = app.engine().control();
    engine.dispatch_event(
        id,
        if down {
            EventKind::KeyDown
        } else {
            EventKind::KeyUp
        },
        EventPayload::Key(event),
    );
    let (sent, received) = std::sync::mpsc::channel();
    engine.flush(move |_| {
        let _ = sent.send(());
    });
    received
        .recv_timeout(Duration::from_secs(5))
        .expect("the key flushed");
    app.catch_up();
}

fn ctrl_enter() -> Modifiers {
    Modifiers {
        ctrl: true,
        ..Modifiers::default()
    }
}

/// The app with the consent card opened by `Ctrl+Enter` from the editor.
fn open_card_with_ctrl_enter(os: Os) -> (Headless, tempfile::TempDir, tempfile::TempDir) {
    let (mut app, directory, config) = start(true, os);
    app.type_text(EDITOR, "").expect("focus the editor");
    app.press_key("ctrl+enter").expect("ctrl+enter");
    app.wait_for("id:consent-card").expect("the card opened");
    (app, directory, config)
}

#[test]
fn the_ctrl_enter_that_opens_the_card_never_sends_it() {
    let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let (mut app, _directory, _config) = open_card_with_ctrl_enter(Os::Linux);

    // The same key stays down for well past the charge a send needs. The
    // opening press reached the focus before the card existed; its
    // auto-repeats carry `repeat` and must not charge it. The headless source
    // reports no repeats, so the test plays the opening hold as the platform
    // does.
    for _ in 0..40 {
        card_key(&mut app, "Enter", true, ctrl_enter(), true);
        app.run_for(Duration::from_millis(33))
            .expect("the next repeat");
    }
    assert!(!sent(&app), "the key that opened the card sent it");
    assert!(app.shows("Send for review?"), "the card is still waiting");

    // Let go, then a deliberate hold at once: that one sends.
    card_key(&mut app, "Enter", false, ctrl_enter(), false);
    card_key(&mut app, "Enter", true, ctrl_enter(), false);
    app.run_for(Duration::from_millis(1_500))
        .expect("the fresh hold");
    assert!(
        sent(&app),
        "a fresh hold after the release sends: {:?}",
        app.screen().texts()
    );
}

#[test]
fn a_fresh_hold_soon_after_the_opening_tap_sends() {
    let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let (mut app, _directory, _config) = open_card_with_ctrl_enter(Os::Linux);

    // The opening press was a tap; a deliberate hold follows well inside the
    // platform's slowest repeat delay, and needs no pause before it.
    app.run_for(Duration::from_millis(300))
        .expect("a short release");
    card_key(&mut app, "Enter", true, ctrl_enter(), false);
    app.run_for(Duration::from_millis(1_500))
        .expect("the fresh hold");
    assert!(
        sent(&app),
        "a hold 300ms after the opening tap sends: {:?}",
        app.screen().texts()
    );
}

#[test]
fn the_consent_card_reads_question_destination_boundary_and_key() {
    let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let (mut app, _directory, _config) = start(true, Os::Windows);
    app.press("id:title-review").expect("Review");
    app.wait_for("id:consent-card").expect("the card opened");

    // The question is read first: the medium face, a step above the body.
    let headline = app
        .screen()
        .span_style("id:consent-card >> text:Send for review?")
        .expect("the headline");
    assert_eq!(headline.font_size.0, 16.0, "{headline:?}");
    assert_eq!(headline.line_height.0, 24.0, "{headline:?}");
    assert!(
        format!("{:?}", headline.font_family).contains("Martian Grotesk Std Md"),
        "{headline:?}"
    );

    // The destination is a list of terms, each value its own text in the
    // source face, in the order the line disclosed them.
    let destination = "id:consent-destination";
    for (term, value) in [
        ("To", "/^Local model · 127\\.0\\.0\\.1:\\d+$/"),
        ("Transport", "HTTP (not encrypted)"),
        ("API", "/^OpenAI .* \\/v1$/"),
        ("Proxy", "disabled"),
    ] {
        app.wait_for(&format!("{destination} >> text:{term}"))
            .unwrap_or_else(|why| panic!("the `{term}` term: {why}"));
        let value_selector = format!("{destination} >> text:{value}");
        let style = app
            .screen()
            .span_style(&value_selector)
            .unwrap_or_else(|why| panic!("`{term}`'s value {value}: {why}"));
        assert!(
            format!("{:?}", style.font_family).contains("Martian Mono"),
            "`{term}`'s value is in the source face: {style:?}"
        );
    }

    // One boundary sentence; the key as chips, not a sentence repeating the
    // button.
    assert!(app.shows("This content and the request framing leave this device."));
    assert!(!app.shows("Request framing crosses this boundary."));
    assert!(!app.shows("Hold Ctrl+Enter to send"));
    for key in ["Ctrl", "Enter"] {
        let chip = app
            .screen()
            .span_style(&format!("id:consent-key-hint >> text:{key}"))
            .unwrap_or_else(|why| panic!("the `{key}` chip: {why}"));
        assert!(
            format!("{:?}", chip.font_family).contains("Martian Mono"),
            "{chip:?}"
        );
    }
    // Cancel is still the card's cancel.
    app.press("id:consent-cancel-button").expect("Cancel");
    app.wait_for_gone("id:consent-card", Duration::from_secs(2))
        .expect("Cancel closed the card");
}

// While a consent card is open it is modal: the room's own shortcuts are
// inert, so no picker opens and no tab closes during a charge, and the
// charge itself is one unbroken press.
#[test]
fn a_waiting_consent_card_keeps_the_room_shortcuts_inert() {
    let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let (mut app, _directory, _config) = open_card_with_ctrl_enter(Os::Linux);

    // `Ctrl+,` is inert too: Settings never opens under the card.
    app.press_key("ctrl+,").expect("the settings shortcut");
    assert_eq!(
        app.screen().matches("id:settings-page"),
        Ok(false),
        "Settings opened under the consent card: {:?}",
        app.screen().texts()
    );

    // A deliberate hold is underway.
    card_key(&mut app, "Enter", true, ctrl_enter(), false);
    app.run_for(Duration::from_millis(300)).expect("held");

    // The platform has file dialogs: a request would be recorded.
    app.on_pick_paths(Vec::new()).expect("file dialogs");
    app.press_key("ctrl+o").expect("the folder shortcut");
    assert!(
        app.screen().file_requests().is_empty(),
        "the folder picker opened during the charge: {:?}",
        app.screen().file_requests()
    );
    app.press_key("ctrl+w").expect("the tab-close shortcut");
    assert_eq!(
        app.screen().matches("id:title-file"),
        Ok(true),
        "the card's tab was closed: {:?}",
        app.screen().texts()
    );
    assert_eq!(
        app.screen().matches("id:consent-card"),
        Ok(true),
        "the card stays while the hold runs"
    );

    // The hold was one unbroken press: the inert shortcuts changed nothing,
    // and the charge completed at its own deadline.
    app.run_for(Duration::from_millis(600))
        .expect("the rest of the charge");
    assert!(
        sent(&app),
        "the unbroken hold sent after the shortcuts did nothing: {:?}",
        app.screen().texts()
    );
}

#[test]
fn ctrl_s_saves_from_the_focused_editor() {
    let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let (mut app, directory, _config) = start(false, Os::Windows);
    let path = directory.path().join("plan.md");
    app.type_text(EDITOR, "Saved by key.")
        .expect("type into the editor");
    app.wait_for("id:title-dirty")
        .expect("the document is dirty");

    app.press_key("ctrl+s").expect("ctrl+s");
    support::wait_for_file(
        &mut app,
        &path,
        |text| text.contains("Saved by key."),
        "Ctrl+S wrote the file",
        Duration::from_secs(5),
    );
    app.wait_for_gone("id:title-dirty", Duration::from_secs(5))
        .expect("the dirty mark cleared");
    assert!(
        !app.shows("Saved by key.s"),
        "the key typed nothing into the text"
    );
}

#[test]
fn the_title_row_reads_review_settings_and_a_menu_of_shortcuts() {
    let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let (mut app, _directory, _config) = start(false, Os::Windows);

    // `Review` draws its own underline, in the medium face.
    let review = app
        .screen()
        .span_style("id:title-review >> text:Review")
        .expect("Review's text");
    assert!(
        review.decoration.underline,
        "Review is underlined: {review:?}"
    );
    assert!(
        format!("{:?}", review.font_family).contains("Martian Grotesk Std Md"),
        "Review is set in the medium face: {review:?}"
    );

    // `Settings` is a word, named as it reads, that says whether its menu
    // is open.
    app.wait_for("id:title-settings >> text:Settings")
        .expect("the settings word");
    let closed = app
        .screen()
        .accessibility_state("id:title-settings")
        .expect("the control's state");
    assert!(closed.has_popup, "it opens a menu");
    assert_eq!(closed.expanded, Some(false), "the menu is closed");
    assert_eq!(
        app.screen()
            .count("id:title-row >> role:button[Settings]")
            .expect("selector"),
        1,
        "screen readers name it as it reads"
    );

    app.press("id:title-settings").expect("open the menu");
    app.press("id:title-menu-web")
        .expect("toggle compatibility");
    app.wait_for_gone("id:title-menu", Duration::from_secs(5))
        .expect("the panel dismounts after the exit");

    app.press("id:title-settings").expect("open the menu");
    assert_eq!(
        app.screen()
            .accessibility_state("id:title-settings")
            .expect("the control's state")
            .expanded,
        Some(true),
        "the menu is open"
    );
    for (row, keys) in [
        ("title-menu-open-folder", "Ctrl+O"),
        ("title-menu-new-file", "Ctrl+N"),
        ("title-menu-save", "Ctrl+S"),
        ("title-menu-save-as", "Ctrl+Shift+S"),
    ] {
        app.wait_for(&format!("id:{row} >> text:{keys}"))
            .unwrap_or_else(|why| panic!("`{row}` names {keys}: {why}"));
    }
    app.wait_for("id:title-view >> text:Edit")
        .expect("the toggle does not cycle Edit");
    assert_eq!(
        app.screen()
            .accessibility_state("id:title-menu-web")
            .expect("compatibility state")
            .checked,
        Some(nsh_core::Checked::True),
        "the menu exposes the compatibility preference"
    );

    // Every row is one line, in a menu at least 240 wide.
    let nodes = app.screen().layout_nodes();
    let bounds = |id: &str| {
        nodes
            .iter()
            .find(|node| node.test_id.as_deref() == Some(id))
            .map(|node| node.bounds)
            .unwrap_or_else(|| panic!("`{id}` on screen"))
    };
    assert!(
        bounds("title-menu").size.width.0 >= 240.0,
        "the menu is at least 240 wide: {:?}",
        bounds("title-menu")
    );
    let one_line = bounds("title-menu-new-file").size.height.0;
    for row in [
        "title-menu-open-folder",
        "title-menu-paste",
        "title-menu-save-as",
        "title-menu-review-selection",
    ] {
        assert_eq!(
            bounds(row).size.height.0,
            one_line,
            "`{row}` is one line high"
        );
    }

    app.press("id:title-settings").expect("close the menu");
    app.press("id:title-file")
        .expect("open Context and return focus to the editor");
    app.press_key("z").expect("type into the editor");
    assert!(
        app.screen().texts().iter().any(|text| {
            text.contains("Original") && (text.contains("zOriginal") || text.contains("Originalz"))
        }),
        "the editor received the character after the title-file press: {:?}",
        app.screen().texts()
    );
}

/// A provider on loopback that answers every Review with no findings; its
/// base URL.
fn loopback_provider() -> String {
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
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind loopback");
    let base_url = format!("http://{}/v1/", listener.local_addr().unwrap());
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else {
                return;
            };
            read_http_body(&mut stream);
            write_review_json(&mut stream, &review);
        }
    });
    base_url
}

fn read_http_body(stream: &mut TcpStream) {
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
}

fn write_review_json(stream: &mut TcpStream, output: &str) {
    let body = serde_json::json!({
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
        "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
        body.len(),
    );
    let _ = stream.flush();
}
