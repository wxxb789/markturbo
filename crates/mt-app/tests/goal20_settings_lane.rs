use std::{sync::Mutex, time::Duration};

use mt_core::model::{EndpointIdentity, Provider};
use nsh_core::{
    Size,
    element::{Idiom, Os},
    platform::ColorScheme,
};
use nsh_engine::{EngineConfig, Points, Root};
use nsh_testing::Headless;

const EDITOR: &str = "label:Markdown source";

static TEST_LOCK: Mutex<()> = Mutex::new(());

fn start() -> (Headless, tempfile::TempDir, tempfile::TempDir) {
    start_with(None)
}

/// [`start`] with the app's clock under the test's control, so an Undo hold
/// elapses only when the test runs the clock.
fn start_with_manual_clock() -> (Headless, tempfile::TempDir, tempfile::TempDir) {
    start_with(Some(nsh_core::time::Clock::manual(
        std::time::SystemTime::now(),
    )))
}

fn start_with(
    clock: Option<nsh_core::time::Clock>,
) -> (Headless, tempfile::TempDir, tempfile::TempDir) {
    start_with_config(tempfile::tempdir().expect("config dir"), clock)
}

fn start_with_config(
    config: tempfile::TempDir,
    clock: Option<nsh_core::time::Clock>,
) -> (Headless, tempfile::TempDir, tempfile::TempDir) {
    let codex_home = tempfile::tempdir().expect("Codex home");
    let data_dir = config.path().join("data");
    // SAFETY: TEST_LOCK serializes app lifetimes and environment changes.
    unsafe {
        std::env::remove_var("MARKTURBO_OPEN");
        std::env::remove_var("OPENAI_API_KEY");
        std::env::set_var("MARKTURBO_CONFIG_DIR", config.path());
        std::env::set_var("MARKTURBO_DATA_DIR", data_dir);
        std::env::set_var(
            "MARKTURBO_TEST_LOCK_NAMESPACE",
            std::process::id().to_string(),
        );
        std::env::set_var("CODEX_HOME", codex_home.path());
    }

    let mut engine = EngineConfig::new(Root::of::<mt_app::App>());
    engine.environment.os = Os::Windows;
    engine.environment.idiom = Idiom::Desktop;
    engine.environment.color_scheme = ColorScheme::Light;
    if let Some(clock) = clock {
        engine.clock = clock;
    }
    let mut app = Headless::start(engine).expect("start app");
    app.set_viewport(Size::new(Points(1280.0), Points(900.0)))
        .expect("900px viewport");
    app.wait_for("id:shell").expect("shell");
    (app, config, codex_home)
}

fn open_settings(app: &mut Headless) {
    app.press("id:title-settings").expect("open title menu");
    app.press("id:title-menu-settings").expect("open settings");
    app.wait_for("id:settings-page").expect("settings page");
}

fn persisted(config: &tempfile::TempDir) -> mt_core::settings::SettingsData {
    mt_core::settings::SettingsData::load_from(&config.path().join("settings.toml"))
}

fn openai_target(base_url: &str) -> String {
    EndpointIdentity::parse(Provider::OpenAiChat, Some(base_url))
        .expect("the endpoint parses")
        .credential_target()
        .to_string()
}

fn set_base_url(app: &mut Headless, url: &str) {
    app.clear("id:settings-base-url")
        .expect("clear the base URL");
    app.type_text("id:settings-base-url", url)
        .expect("type the base URL");
    app.type_text("id:settings-model", "")
        .expect("blur the base URL");
}

#[test]
fn escape_closes_settings_and_the_editor_takes_typing_again() {
    let _guard = TEST_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    let (mut app, _config, _codex_home) = start();
    app.set_viewport(Size::new(Points(2400.0), Points(1200.0)))
        .expect("a viewport tall enough for both status lines");
    app.press("id:welcome-new-file").expect("new document");
    app.wait_for(EDITOR).expect("the source editor");
    open_settings(&mut app);

    app.press_key("escape").expect("escape");
    app.wait_for_gone("id:settings-page", Duration::from_secs(5))
        .expect("Settings closes on Escape");
    app.wait_for(EDITOR).expect("the editor is back");
    app.wait(Duration::from_millis(500))
        .expect("the focus request lands");
    app.press_key("z")
        .expect("a character goes to the focused editor");
    app.wait_for("id:source-editor >> text:/^z$/")
        .expect("the character landed in the editor");
}

#[test]
fn escape_closes_the_title_menu_before_settings() {
    let _guard = TEST_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    let (mut app, _config, _codex_home) = start();
    open_settings(&mut app);
    app.press("id:title-settings").expect("open the title menu");
    app.wait_for("id:title-menu")
        .expect("the menu is disclosed");

    app.press_key("escape").expect("first Escape");
    app.wait_for_gone("id:title-menu", Duration::from_secs(5))
        .expect("the title menu closes first");
    assert!(
        app.screen()
            .matches("id:settings-page")
            .expect("valid settings-page selector"),
        "the first Escape closed Settings under the menu"
    );

    app.press_key("escape").expect("second Escape");
    app.wait_for_gone("id:settings-page", Duration::from_secs(5))
        .expect("the second Escape closes Settings");
}

#[test]
fn the_shortcut_toggles_settings() {
    let _guard = TEST_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    let (mut app, _config, _codex_home) = start();
    app.press_key("mod+,")
        .expect("open Settings with the shortcut");
    app.wait_for("id:settings-page").expect("Settings opens");
    app.press_key("mod+,")
        .expect("close Settings with the shortcut");
    app.wait_for_gone("id:settings-page", Duration::from_secs(5))
        .expect("Settings closes with the shortcut");
}

#[test]
fn settings_auto_save_without_the_save_control() {
    let _guard = TEST_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    let (mut app, config, _codex_home) = start();
    open_settings(&mut app);
    assert!(
        !app.screen()
            .matches("id:settings-save")
            .expect("valid settings-save selector"),
        "auto-save leaves no Save control"
    );

    app.press("text:OpenAI Chat Completions")
        .expect("choose a wire format");
    app.wait_for("text:No credential stored")
        .expect("configured status");
    assert!(
        !app.screen().shows("Settings saved."),
        "the choice press persisted without a receipt"
    );
    assert_eq!(
        persisted(&config).model_provider,
        "openai-chat",
        "the choice press persisted the wire format"
    );

    app.clear("id:settings-base-url").expect("clear the field");
    app.type_text("id:settings-base-url", "http://127.0.0.1:8000/v1")
        .expect("type a loopback URL");
    app.type_text("id:settings-model", "")
        .expect("blur the base URL");
    assert_eq!(
        persisted(&config).model_base_url,
        "http://127.0.0.1:8000/v1",
        "the blur saved the valid base URL"
    );
}

#[test]
fn an_invalid_base_url_keeps_the_last_valid_value_saved() {
    let _guard = TEST_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    let (mut app, config, _codex_home) = start();
    open_settings(&mut app);
    app.press("text:OpenAI Chat Completions")
        .expect("choose a wire format");
    app.wait_for("text:No credential stored")
        .expect("configured status");

    app.clear("id:settings-base-url").expect("clear the field");
    app.type_text("id:settings-base-url", "http://127.0.0.1:8000/v1")
        .expect("type a loopback URL");
    app.type_text("id:settings-model", "")
        .expect("blur the base URL");

    app.clear("id:settings-base-url").expect("clear the field");
    app.type_text("id:settings-base-url", "http://example.com/v1")
        .expect("type an invalid remote HTTP URL");
    app.type_text("id:settings-model", "")
        .expect("blur the invalid base URL");
    app.wait_for(
        "text:HTTPS is required for remote hosts. For a local server on this machine, use a numeric loopback address such as http://127.0.0.1:11434/v1.",
    )
    .expect("the inline error");
    assert_eq!(
        app.screen()
            .input_text("id:settings-base-url")
            .expect("read the field")
            .unwrap_or_default(),
        "http://example.com/v1",
        "the field keeps the invalid edit for correction"
    );
    assert_eq!(
        persisted(&config).model_base_url,
        "http://127.0.0.1:8000/v1",
        "the invalid edit never replaced the last valid value on disk"
    );
}

#[test]
fn an_endpoint_switch_during_a_credential_hold_leaves_the_other_key_alone() {
    let _guard = TEST_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    let (mut app, _config, _codex_home) = start();
    open_settings(&mut app);
    app.press("text:OpenAI Chat Completions")
        .expect("choose a wire format");
    set_base_url(&mut app, "http://127.0.0.1:11441/v1");
    app.type_text("id:api-key", "synthetic-key-a")
        .expect("type a key");
    app.press("text:Store securely").expect("store key a");
    app.wait_for("text:Credential stored securely for this endpoint")
        .expect("a is stored");

    set_base_url(&mut app, "http://127.0.0.1:11442/v1");
    app.type_text("id:api-key", "synthetic-key-b")
        .expect("type another key");
    app.press("text:Store securely").expect("store key b");
    app.wait_for("text:Credential stored securely for this endpoint")
        .expect("b is stored");

    let (target_a, target_b) = (
        openai_target("http://127.0.0.1:11441/v1"),
        openai_target("http://127.0.0.1:11442/v1"),
    );
    assert_eq!(
        mt_app::test_vault::stored(&target_a).as_deref(),
        Some("synthetic-key-a")
    );
    assert_eq!(
        mt_app::test_vault::stored(&target_b).as_deref(),
        Some("synthetic-key-b")
    );

    app.press("text:Remove").expect("start b's credential hold");
    app.wait_for("text:No credential stored")
        .expect("b reads as removed during the hold");
    assert_eq!(
        mt_app::test_vault::stored(&target_b).as_deref(),
        Some("synthetic-key-b"),
        "the hold still keeps b's captured credential"
    );

    set_base_url(&mut app, "http://127.0.0.1:11441/v1");
    app.wait_for_within(
        "text:Credential stored securely for this endpoint",
        Duration::from_secs(1),
    )
    .expect("a still reads as stored while b's removal is pending");
    assert_eq!(
        mt_app::test_vault::stored(&target_b).as_deref(),
        Some("synthetic-key-b"),
        "b's hold was still open while a was read"
    );

    app.wait(Duration::from_millis(
        mt_app::ui::motion::UNDO_HOLD_MS + 500,
    ))
    .expect("b's hold elapses");
    assert_eq!(
        mt_app::test_vault::stored(&target_a).as_deref(),
        Some("synthetic-key-a"),
        "the commit touched the endpoint that was on screen now"
    );
    assert_eq!(
        mt_app::test_vault::stored(&target_b),
        None,
        "the captured endpoint's credential never committed"
    );
}

#[test]
fn a_second_tier_two_action_commits_the_first_at_once() {
    let _guard = TEST_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    let (mut app, _config, _codex_home) = start();
    open_settings(&mut app);
    app.press("text:OpenAI Chat Completions")
        .expect("choose a wire format");
    set_base_url(&mut app, "http://127.0.0.1:11443/v1");
    app.type_text("id:api-key", "synthetic-key-two")
        .expect("type a key");
    app.press("text:Store securely").expect("store the key");
    app.wait_for("text:Credential stored securely for this endpoint")
        .expect("the key is stored");
    let target = openai_target("http://127.0.0.1:11443/v1");
    assert_eq!(
        mt_app::test_vault::stored(&target).as_deref(),
        Some("synthetic-key-two")
    );

    app.press("text:Remove").expect("start the first hold");
    app.wait_for("text:No credential stored")
        .expect("the removal is shown at once");
    assert_eq!(
        mt_app::test_vault::stored(&target).as_deref(),
        Some("synthetic-key-two"),
        "the first hold keeps its captured credential"
    );

    app.scroll("id:settings-scroll", Points(10_000.0))
        .expect("scroll to the usage counters");
    app.settle();
    app.press("id:usage-clear")
        .expect("start the second Tier 2 action");
    assert_eq!(
        mt_app::test_vault::stored(&target),
        None,
        "the newer Tier 2 action did not commit the older one at once"
    );

    app.press("id:status-undo").expect("undo the counts clear");
    assert_eq!(
        mt_app::test_vault::stored(&target),
        None,
        "the older action's Undo survived the newer one"
    );
}

#[test]
fn leaving_settings_keeps_the_credential_hold_running() {
    let _guard = TEST_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    let (mut app, _config, _codex_home) = start();
    open_settings(&mut app);
    app.press("text:OpenAI Chat Completions")
        .expect("choose a wire format");
    set_base_url(&mut app, "http://127.0.0.1:11444/v1");
    app.type_text("id:api-key", "synthetic-key-three")
        .expect("type a key");
    app.press("text:Store securely").expect("store the key");
    app.wait_for("text:Credential stored securely for this endpoint")
        .expect("the key is stored");
    let target = openai_target("http://127.0.0.1:11444/v1");

    app.press("text:Remove").expect("start the hold");
    app.wait_for("text:No credential stored")
        .expect("the removal is shown at once");

    app.press("id:settings-close").expect("leave settings");
    app.wait_for_gone("id:settings-page", Duration::from_secs(5))
        .expect("settings closed");
    assert_eq!(
        mt_app::test_vault::stored(&target).as_deref(),
        Some("synthetic-key-three"),
        "leaving settings committed the hold"
    );
    app.wait_for_within("id:status-undo", Duration::from_secs(1))
        .expect("the editor status line carries the pending Undo");

    app.wait(Duration::from_millis(
        mt_app::ui::motion::UNDO_HOLD_MS + 500,
    ))
    .expect("the hold elapses");
    assert_eq!(
        mt_app::test_vault::stored(&target),
        None,
        "the hold never committed at its deadline"
    );
}

#[test]
fn typing_alone_does_not_save_but_blur_does() {
    let _guard = TEST_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    let (mut app, config, _codex_home) = start();
    open_settings(&mut app);
    app.press("text:OpenAI Chat Completions")
        .expect("choose a wire format");
    let before = persisted(&config);

    app.clear("id:settings-base-url").expect("clear the field");
    app.type_text("id:settings-base-url", "http://127.0.0.1:11445/v1")
        .expect("type a URL");
    assert_eq!(
        persisted(&config).model_base_url,
        before.model_base_url,
        "typing alone saved the base URL"
    );

    app.type_text("id:settings-model", "")
        .expect("blur the base URL");
    assert_eq!(
        persisted(&config).model_base_url,
        "http://127.0.0.1:11445/v1",
        "the blur saved the base URL"
    );
}

#[test]
fn a_valid_model_saves_while_the_base_url_is_invalid() {
    let _guard = TEST_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    let (mut app, config, _codex_home) = start();
    open_settings(&mut app);
    app.press("text:OpenAI Chat Completions")
        .expect("choose a wire format");
    set_base_url(&mut app, "http://127.0.0.1:11446/v1");
    let valid = persisted(&config).model_base_url;
    assert_eq!(valid, "http://127.0.0.1:11446/v1");

    app.clear("id:settings-base-url").expect("clear the field");
    app.type_text("id:settings-base-url", "http://example.com/v1")
        .expect("type an invalid URL");
    app.clear("id:settings-model")
        .expect("clear the model field");
    app.type_text("id:settings-model", "valid-model")
        .expect("type a valid model");
    app.type_text("id:settings-target-language", "")
        .expect("blur the model");

    let saved = persisted(&config);
    assert_eq!(
        saved.model_name, "valid-model",
        "the valid model was blocked by the invalid base URL"
    );
    assert_eq!(
        saved.model_base_url, valid,
        "the invalid base URL replaced the last valid value on disk"
    );
}

#[test]
fn the_page_renders_the_wire_format_and_model_help() {
    let _guard = TEST_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    let (mut app, _config, _codex_home) = start();
    open_settings(&mut app);
    app.wait_for("text:Wire format")
        .expect("the provider row is renamed");
    app.wait_for("text:/Selects the API protocol/")
        .expect("the wire-format help");
    app.wait_for(
        "text:/Leave empty to use MARKTURBO_MODEL, then MARKTURBO_TRANSLATE_MODEL, then the wire format's default model/"
    )
    .expect("the model help");
}

#[test]
fn an_invalid_base_url_stays_out_of_settings_when_usage_is_toggled() {
    let _guard = TEST_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    let (mut app, config, _codex_home) = start();
    open_settings(&mut app);
    app.press("text:OpenAI Chat Completions")
        .expect("choose a wire format");
    set_base_url(&mut app, "http://127.0.0.1:11448/v1");
    assert_eq!(
        persisted(&config).model_base_url,
        "http://127.0.0.1:11448/v1"
    );

    app.clear("id:settings-base-url")
        .expect("clear the base URL");
    app.type_text("id:settings-base-url", "https://user:secret@example.com/v1")
        .expect("type a URL carrying userinfo");
    app.scroll("id:settings-scroll", Points(10_000.0))
        .expect("scroll to the usage counters");
    app.settle();
    app.press("id:usage-on").expect("toggle the usage counters");
    app.wait_for("text:Usage-counter preference saved.")
        .expect("the consent persisted");

    assert_eq!(
        persisted(&config).model_base_url,
        "http://127.0.0.1:11448/v1",
        "the usage toggle saved the invalid base URL"
    );
    let settings =
        std::fs::read_to_string(config.path().join("settings.toml")).expect("the settings file");
    assert!(
        !settings.contains("secret"),
        "the invalid URL's userinfo reached the settings file"
    );
    assert_eq!(
        app.screen()
            .input_text("id:settings-base-url")
            .expect("read the field")
            .unwrap_or_default(),
        "https://user:secret@example.com/v1",
        "the usage toggle replaced the edit on screen"
    );
    app.wait_for("text:The endpoint URL cannot contain user information.")
        .expect("the usage toggle cleared the validation error");
}

#[test]
fn a_rejected_base_url_cannot_reach_disk_after_settings_closes() {
    let _guard = TEST_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    let (mut app, config, _codex_home) = start();
    app.press("id:welcome-new-file").expect("new document");
    app.wait_for(EDITOR).expect("the source editor");
    open_settings(&mut app);
    app.press("text:OpenAI Chat Completions")
        .expect("choose a wire format");
    set_base_url(&mut app, "http://127.0.0.1:11451/v1");

    app.clear("id:settings-base-url").expect("clear the field");
    app.type_text(
        "id:settings-base-url",
        "https://user:secret@example.invalid:bad",
    )
    .expect("type a rejected URL");
    app.press("id:settings-close").expect("close settings");
    app.wait_for_gone("id:settings-page", Duration::from_secs(5))
        .expect("settings closed");

    app.press("id:title-view").expect("choose Split");
    app.press("id:title-view").expect("choose Preview");
    app.wait_for("id:preview-column")
        .expect("the preview pane is current");

    let settings =
        std::fs::read_to_string(config.path().join("settings.toml")).expect("the settings file");
    assert!(
        !settings.contains("user") && !settings.contains("secret"),
        "a rejected URL's userinfo reached the settings file: {settings}"
    );
    assert_eq!(
        persisted(&config).model_base_url,
        "http://127.0.0.1:11451/v1",
        "the pane switch replaced the last valid base URL"
    );
}

#[test]
fn a_pending_removal_blocks_the_review_from_using_the_credential() {
    let _guard = TEST_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    let (mut app, _config, _codex_home) = start();
    app.press("id:welcome-new-file").expect("new document");
    app.wait_for(EDITOR).expect("the source editor");
    open_settings(&mut app);
    app.press("text:OpenAI Chat Completions")
        .expect("choose a wire format");
    set_base_url(&mut app, "http://127.0.0.1:11449/v1");
    app.type_text("id:api-key", "synthetic-pending-key")
        .expect("type a key");
    app.press("text:Store securely").expect("store the key");
    app.wait_for("text:Credential stored securely for this endpoint")
        .expect("the key is stored");

    app.press("text:Remove").expect("start the removal hold");
    app.wait_for("text:No credential stored")
        .expect("the removal is shown at once");
    assert_eq!(
        mt_app::test_vault::stored(&openai_target("http://127.0.0.1:11449/v1")).as_deref(),
        Some("synthetic-pending-key"),
        "the hold keeps the vault's copy"
    );

    app.press("id:settings-close").expect("leave settings");
    app.wait_for_gone("id:settings-page", Duration::from_secs(5))
        .expect("settings closed");
    app.press("id:title-review").expect("start a review");
    app.wait_for_within("id:review-unavailable", Duration::from_secs(20))
        .expect("the review says why it cannot run");
    app.wait_for("text:No model set up")
        .expect("the pending credential reads as missing");
    assert!(
        !app.screen().shows("Send for review?"),
        "the pending removal must not arm a credential request"
    );
}

#[test]
fn a_pending_undo_survives_closing_the_review_panel() {
    let _guard = TEST_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    let (mut app, _config, _codex_home) = start_with_manual_clock();
    app.press("id:welcome-new-file").expect("new document");
    app.wait_for(EDITOR).expect("the source editor");
    open_settings(&mut app);
    app.press("text:OpenAI Chat Completions")
        .expect("choose a wire format");
    set_base_url(&mut app, "http://127.0.0.1:11450/v1");
    app.type_text("id:api-key", "synthetic-key-review-undo")
        .expect("type a key");
    app.press("text:Store securely").expect("store the key");
    app.wait_for("text:Credential stored securely for this endpoint")
        .expect("the key is stored");
    let target = openai_target("http://127.0.0.1:11450/v1");

    app.press("text:Remove").expect("start the removal hold");
    app.wait_for("text:No credential stored")
        .expect("the removal is shown at once");
    app.press("id:settings-close").expect("leave settings");
    app.wait_for_gone("id:settings-page", Duration::from_secs(5))
        .expect("settings closed");

    app.press("id:title-review").expect("start a review");
    app.wait_for_within("id:review-unavailable", Duration::from_secs(20))
        .expect("the review says why it cannot run");
    app.wait_for("text:No model set up")
        .expect("the pending credential reads as missing");
    app.wait_for_within("id:status-undo", Duration::from_secs(1))
        .expect("the review status line carries the pending Undo");

    app.press("id:review-close").expect("close the review");
    app.wait_for_gone("id:review-unavailable", Duration::from_secs(5))
        .expect("the review field closes");
    app.wait_for_within("id:status-undo", Duration::from_secs(1))
        .expect("the editor status line carries the pending Undo");

    app.press("id:status-undo").expect("undo the removal");
    app.run_for(Duration::from_millis(
        mt_app::ui::motion::UNDO_HOLD_MS + 500,
    ))
    .expect("the hold would have elapsed");
    assert_eq!(
        mt_app::test_vault::stored(&target).as_deref(),
        Some("synthetic-key-review-undo"),
        "the undone removal committed after Review closed"
    );
}

#[test]
fn credential_actions_wait_for_a_configured_endpoint() {
    let _guard = TEST_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    let (mut app, _config, _codex_home) = start();
    open_settings(&mut app);
    app.wait_for(
        "text:Choose a wire format above (not Best available) to configure its credential.",
    )
    .expect("unconfigured status");
    app.wait_for("text:Use for session")
        .expect("the session control exists");
    app.wait_for("text:Store securely")
        .expect("the store control exists");
    assert!(
        app.press("text:Use for session").is_err(),
        "the session action is disabled while unconfigured"
    );
    assert!(
        app.press("text:Store securely").is_err(),
        "the store action is disabled while unconfigured"
    );

    app.press("text:OpenAI Chat Completions")
        .expect("configure a wire format");
    app.wait_for("text:No credential stored")
        .expect("configured status");
    app.press("text:Use for session")
        .expect("the session action is enabled");
    app.wait_for("text:Enter a credential first.")
        .expect("the enabled action ran");
}

#[test]
fn the_usage_row_carries_its_accessible_label() {
    let _guard = TEST_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    let (mut app, _config, _codex_home) = start();
    open_settings(&mut app);
    app.scroll("id:settings-scroll", Points(10_000.0))
        .expect("scroll to usage settings");
    app.settle();
    assert!(
        app.screen()
            .matches("role:radiogroup[Local usage counters]")
            .expect("valid radiogroup selector"),
        "the usage toggle keeps its accessible label"
    );
    app.wait_for("text:Show details")
        .expect("the closed toggle label");
}

#[test]
fn clearing_a_corrupt_usage_file_deletes_it_at_once() {
    let _guard = TEST_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    let (mut app, config, _codex_home) = start();
    let store = config.path().join("data").join("usage.json");
    std::fs::create_dir_all(store.parent().expect("the data directory"))
        .expect("the data directory");
    std::fs::write(&store, "PRIVATE-INPUT").expect("a corrupt store");
    open_settings(&mut app);
    app.scroll("id:settings-scroll", Points(10_000.0))
        .expect("scroll to the usage counters");
    app.settle();
    app.press("id:usage-clear")
        .expect("clear the corrupt store");
    app.wait_for(
        "text:Local counts deleted. Recording preference and exported copies are unchanged.",
    )
    .expect("clear confirmation");
    assert!(!store.exists(), "the corrupt store was not deleted");
    app.wait_for("id:status-undo").expect("the Undo");
    app.press("id:status-undo").expect("undo the empty clear");
    assert!(!store.exists(), "Undo resurrected the corrupt file");
}

#[test]
fn a_failed_undo_keeps_the_counts_for_a_retry() {
    let _guard = TEST_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    let (mut app, config, _codex_home) = start_with_manual_clock();
    let store = config.path().join("data").join("usage.json");
    let day = mt_core::usage::day_bucket(std::time::SystemTime::now());
    std::fs::create_dir_all(store.parent().expect("the data directory"))
        .expect("the data directory");
    std::fs::write(
        &store,
        format!(
            r#"{{"schema_version":1,"days":{{"{day}":{{"events":{{"artifact_opened":1}},"review_started":{{}},"review_durations":{{}},"accepted_hunks":0,"rejected_hunks":0}}}}}}"#
        ),
    )
    .expect("a seeded store");

    open_settings(&mut app);
    app.scroll("id:settings-scroll", Points(10_000.0))
        .expect("scroll to the usage counters");
    app.settle();
    app.press("id:usage-clear").expect("clear the counts");
    app.wait_for(
        "text:Local counts deleted. Recording preference and exported copies are unchanged.",
    )
    .expect("clear confirmation");
    assert!(!store.exists(), "the clear left the store behind");

    // A directory at the store path makes every read of the store fail, so
    // the Undo's restore fails. It stands in for a held write lock.
    std::fs::create_dir(&store).expect("a decoy store");
    app.press("id:status-undo").expect("run the failing Undo");
    app.settle();
    assert!(
        !app.screen().shows("Undone."),
        "a failed restore still reported success"
    );
    app.wait_for(
        "text:Local usage counts could not be read or written. Editing and Review remain available.",
    )
    .expect("the failure notice");
    app.wait_for("id:status-undo")
        .expect("the failed Undo stayed live for a retry");

    std::fs::remove_dir(&store).expect("clear the decoy");
    app.press("id:status-undo").expect("retry the Undo");
    app.wait_for("text:Undone.").expect("the retry succeeded");
    assert!(
        !app.screen().shows(
            "Local usage counts could not be read or written. Editing and Review remain available."
        ),
        "the retry left the failure notice up after restoring the counts"
    );
    let json: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&store).expect("the restored store"))
            .expect("valid JSON");
    let day_key = day.to_string();
    assert_eq!(
        json["days"][&day_key]["events"]["artifact_opened"], 1,
        "the retry did not restore the captured counts"
    );
}

#[test]
fn a_language_switch_clears_the_english_settings_notice() {
    let _guard = TEST_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    let (mut app, _config, _codex_home) = start();
    open_settings(&mut app);
    app.press("id:usage-on").expect("enable the usage counters");
    app.wait_for("text:Usage-counter preference saved.")
        .expect("the English notice");
    app.press("id:settings-language-chinese")
        .expect("switch the language");
    app.wait_for("text:界面语言").expect("the Chinese page");
    assert!(
        !app.shows("Usage-counter preference saved."),
        "the stale English notice survived the switch: {:?}",
        app.screen().texts()
    );
}

#[test]
fn a_language_switch_renames_a_live_undo() {
    let _guard = TEST_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    let (mut app, _config, _codex_home) = start();
    app.set_viewport(Size::new(Points(1400.0), Points(900.0)))
        .expect("the walk's viewport");
    open_settings(&mut app);
    app.scroll("id:settings-scroll", Points(10_000.0))
        .expect("scroll to the usage counters");
    app.settle();
    app.press("id:usage-on").expect("enable the counters");
    app.press("id:usage-clear").expect("clear the counts");
    app.wait_for("id:settings-page >> id:status-undo")
        .expect("the Undo");
    app.scroll("id:settings-scroll", Points(-2_000.0))
        .expect("back to the language row");
    app.settle();

    app.press("id:settings-language-chinese")
        .expect("switch the language");
    app.wait_for("text:界面语言").expect("the Chinese page");
    app.wait_for("id:settings-page >> text:撤销清除计数")
        .expect("the Undo names its action in the new language");
    app.wait_for("text:已删除本地计数。记录偏好和已导出的副本不变。")
        .expect("the Undo's own result follows the language");
    assert!(
        !app.shows("Local counts deleted. Recording preference and exported copies are unchanged."),
        "the English result survived the switch: {:?}",
        app.screen().texts()
    );
    assert!(
        !app.shows("设置已保存。") && !app.shows("Settings saved."),
        "the save notice replaced the live Undo's message: {:?}",
        app.screen().texts()
    );
    assert_eq!(
        app.screen().count("id:status-action").expect("selector"),
        1,
        "the Undo stayed on exactly one status line"
    );
    app.press("id:status-action").expect("the Undo still works");
    app.wait_for_gone("id:status-action", Duration::from_secs(5))
        .expect("the Undo is spent");
}

#[test]
fn a_theme_switch_during_a_live_undo_shows_the_saves_own_text() {
    let _guard = TEST_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    let (mut app, _config, _codex_home) = start_with_manual_clock();
    app.set_viewport(Size::new(Points(1400.0), Points(900.0)))
        .expect("the walk's viewport");
    open_settings(&mut app);
    app.scroll("id:settings-scroll", Points(10_000.0))
        .expect("scroll to the usage counters");
    app.settle();
    app.press("id:usage-on").expect("enable the counters");
    app.press("id:usage-clear").expect("clear the counts");
    app.wait_for("id:settings-page >> id:status-undo")
        .expect("the Undo");
    app.scroll("id:settings-scroll", Points(-10_000.0))
        .expect("back to the theme and language rows");
    app.settle();

    app.press("id:settings-theme-dark")
        .expect("switch the theme while the Undo is live");
    app.wait_for("text:Settings saved.")
        .expect("the save shows its own text");
    app.wait_for("id:settings-page >> text:Undo clear counts")
        .expect("the Undo names its action");

    app.press("id:settings-language-chinese")
        .expect("switch the language");
    app.wait_for("text:界面语言").expect("the Chinese page");
    app.wait_for("id:settings-page >> text:撤销清除计数")
        .expect("the Undo label in the new language");
    app.wait_for("text:Settings saved.")
        .expect("the newest message keeps its own text");
    assert_eq!(
        app.screen().count("id:status-action").expect("selector"),
        1,
        "the Undo stayed on exactly one status line"
    );
}

#[test]
fn a_slow_folder_shows_beside_the_named_undo() {
    let _guard = TEST_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    let (mut app, config, _codex_home) = start_with_manual_clock();
    let slow = tempfile::tempdir().expect("slow workspace");
    let release = config.path().join("release-slow-scan");
    std::fs::write(slow.path().join("slow.md"), "# Slow\n").expect("slow file");
    let day = mt_core::usage::day_bucket(std::time::SystemTime::now());
    let store = config.path().join("data").join("usage.json");
    std::fs::create_dir_all(store.parent().expect("the data directory"))
        .expect("the data directory");
    std::fs::write(
        &store,
        format!(
            r#"{{"schema_version":1,"days":{{"{day}":{{"events":{{"artifact_opened":1}},"review_started":{{}},"review_durations":{{}},"accepted_hunks":0,"rejected_hunks":0}}}}}}"#
        ),
    )
    .expect("a seeded store");

    app.set_viewport(Size::new(Points(1400.0), Points(900.0)))
        .expect("the walk's viewport");
    open_settings(&mut app);
    app.scroll("id:settings-scroll", Points(10_000.0))
        .expect("scroll to the usage counters");
    app.settle();
    app.wait_for(
        "text:Last 180 days: 1 day active · 0 Reviews · 0 revision decisions · 1 total event",
    )
    .expect("the seeded counts");
    app.press("id:usage-clear").expect("clear the counts");
    app.wait_for("id:settings-page >> id:status-action")
        .expect("the Undo");

    // The scan parks until the test releases it, so the open's notice owns
    // the line while the Undo is live.
    mt_app::test_vault::gate_read(slow.path(), &release);
    app.on_pick_paths(vec![slow.path().to_path_buf()])
        .expect("answer the folder picker");
    app.press("id:title-settings").expect("open the title menu");
    app.press("id:title-menu-open-folder")
        .expect("open the slow folder");
    app.wait_for("text:Opening folder…")
        .expect("the slow open announces itself");

    app.scroll("id:settings-scroll", Points(-10_000.0))
        .expect("back to the language row");
    app.settle();
    app.press("id:settings-language-chinese")
        .expect("switch the language");
    app.wait_for("text:界面语言").expect("the Chinese page");
    app.wait_for("id:settings-page >> text:撤销清除计数")
        .expect("the Undo names its action in the new language");
    assert!(
        app.screen()
            .matches("role:button[撤销清除计数]")
            .expect("the role selector"),
        "the Undo's accessible label follows the language"
    );
    app.wait_for("text:Opening folder…")
        .expect("the folder progress keeps its own text");
    assert_eq!(
        app.screen().count("id:status-action").expect("selector"),
        1,
        "the Undo stayed on exactly one status line"
    );

    std::fs::write(&release, "go").expect("release the slow scan");
    app.press("id:status-action").expect("undo the clear");
    app.scroll("id:settings-scroll", Points(10_000.0))
        .expect("back to the usage counters");
    app.settle();
    app.wait_for("text:近 180 天：活跃 1 天 · 完成审查 0 次 · 修订决定 0 次 · 共 1 条记录")
        .expect("the Undo restores the captured counts");
}

fn node_bounds(app: &Headless, id: &str) -> nsh_core::geometry::Bounds<Points> {
    app.screen()
        .layout_nodes()
        .into_iter()
        .find(|node| node.test_id.as_deref() == Some(id))
        .unwrap_or_else(|| panic!("`{id}` on screen"))
        .bounds
}

fn text_bounds(app: &Headless, content: &str) -> nsh_core::geometry::Bounds<Points> {
    app.screen()
        .layout_nodes()
        .into_iter()
        .find(|node| {
            node.text
                .as_ref()
                .is_some_and(|text| text.text.text == content)
        })
        .unwrap_or_else(|| panic!("`{content}` on screen"))
        .bounds
}

fn y_of(app: &Headless, id: &str) -> f32 {
    node_bounds(app, id).origin.y.0
}

#[test]
fn a_language_switch_keeps_the_settings_scroll_offset() {
    let _guard = TEST_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    let (mut app, _config, _codex_home) = start();
    app.set_viewport(Size::new(Points(1400.0), Points(900.0)))
        .expect("the walk's viewport");
    open_settings(&mut app);
    app.settle();
    let before = y_of(&app, "usage-counters");
    app.scroll("id:settings-scroll", Points(200.0))
        .expect("scroll down the page");
    app.settle();
    let scrolled = y_of(&app, "usage-counters");
    assert!(
        before - scrolled > 150.0,
        "the scroll did not move the page: {before} -> {scrolled}"
    );

    app.press("id:settings-language-chinese")
        .expect("switch the language");
    app.wait_for("text:界面语言").expect("the Chinese page");
    app.settle();
    let after = y_of(&app, "usage-counters");
    assert!(
        (after - scrolled).abs() < 80.0,
        "the language switch reset the scroll offset: {scrolled} -> {after}"
    );
}

#[test]
fn the_settings_status_line_clears_the_usage_actions() {
    let _guard = TEST_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    let (mut app, _config, _codex_home) = start();
    app.set_viewport(Size::new(Points(1400.0), Points(900.0)))
        .expect("the walk's viewport");
    open_settings(&mut app);
    app.scroll("id:settings-scroll", Points(10_000.0))
        .expect("scroll the usage actions into view");
    app.settle();

    fn node(app: &Headless, id: &str) -> nsh_core::geometry::Bounds<Points> {
        app.screen()
            .layout_nodes()
            .into_iter()
            .find(|node| node.test_id.as_deref() == Some(id))
            .unwrap_or_else(|| panic!("`{id}` on screen"))
            .bounds
    }
    fn actions_bottom(app: &Headless) -> f32 {
        ["usage-inspect", "usage-export", "usage-clear"]
            .iter()
            .map(|id| {
                let bounds = node(app, id);
                bounds.origin.y.0 + bounds.size.height.0
            })
            .fold(0.0_f32, f32::max)
    }
    let status = node(&app, "status");
    assert!(
        actions_bottom(&app) <= status.origin.y.0 + 0.5,
        "the status line overlaps the usage actions: actions end at {}, status starts at {}",
        actions_bottom(&app),
        status.origin.y.0
    );

    // Toggling the counters from the walk's bottom scroll posts the notice
    // without a scroll: the actions stay clear of it and of the status
    // line, and the first press lands.
    app.press("id:usage-on").expect("enable the counters");
    app.wait_for("text:Usage-counter preference saved.")
        .expect("the notice");
    app.settle();
    let bottom = actions_bottom(&app);
    let notice = node(&app, "settings-notice");
    assert!(
        bottom <= notice.origin.y.0 + 0.5,
        "the notice overlaps the usage actions: actions end at {bottom}, notice starts at {}",
        notice.origin.y.0
    );
    let status = node(&app, "status");
    assert!(
        bottom <= status.origin.y.0 + 0.5,
        "the status line overlaps the usage actions after the notice: actions end at {bottom}, status starts at {}",
        status.origin.y.0
    );

    app.press_at(
        "id:usage-inspect",
        nsh_core::geometry::Point {
            x: Points(8.0),
            y: Points(6.0),
        },
    )
    .expect("the first press reaches the action");
    app.wait_for("text:Hide details")
        .expect("the first Show details press registered");
}

#[test]
fn a_wrapped_settings_notice_grows_the_notice_line_and_keeps_the_undo_pressable() {
    let _guard = TEST_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    let config = tempfile::tempdir().expect("config dir");
    std::fs::create_dir(config.path().join("settings.toml"))
        .expect("a settings path that refuses the write");
    let (mut app, _config, _codex_home) = start_with_config(config, None);
    app.set_viewport(Size::new(
        Points(mt_app::ui::tokens::WINDOW_MIN_W),
        Points(mt_app::ui::tokens::WINDOW_MIN_H),
    ))
    .expect("the minimum viewport");
    open_settings(&mut app);
    app.scroll("id:settings-scroll", Points(10_000.0))
        .expect("scroll to the usage counters");
    app.settle();
    app.press("id:usage-clear").expect("clear the counts");
    app.wait_for("id:settings-page >> id:status-undo")
        .expect("the Undo");
    app.press("id:usage-on")
        .expect("save a preference that cannot persist");
    let notice = "Could not save the preference. Recording is off for this session; try again before restarting.";
    app.wait_for(&format!("text:{notice}"))
        .expect("the failed-save notice");
    app.settle();

    let text = text_bounds(&app, notice);
    assert!(
        text.size.height.0 >= 2.0 * mt_app::ui::tokens::TYPE_UI_LINE - 0.5,
        "the notice did not wrap at the minimum width: {text:?}"
    );
    let line = node_bounds(&app, "settings-notice");
    assert!(
        line.size.height.0 >= mt_app::ui::tokens::SPACE_3 + text.size.height.0 - 0.5,
        "the notice line did not grow for the wrapped notice: line {line:?}, text {text:?}"
    );
    let status = node_bounds(&app, "status");
    assert!(
        text.origin.y.0 + text.size.height.0 <= status.origin.y.0 + 0.5,
        "the wrapped notice overflows into the status row: notice ends at {}, status starts at {}",
        text.origin.y.0 + text.size.height.0,
        status.origin.y.0
    );

    let undo = node_bounds(&app, "status-undo");
    assert!(
        undo.origin.y.0 >= -0.5
            && undo.origin.y.0 + undo.size.height.0 <= mt_app::ui::tokens::WINDOW_MIN_H + 0.5,
        "the Undo left the minimum window under the wrapped notice: {undo:?}"
    );
    let undo_control = app.screen().id("id:status-undo").expect("the Undo control");
    assert_eq!(
        app.screen()
            .press_target("id:status-action")
            .expect("a valid selector"),
        Some(undo_control),
        "the Undo's own control does not hear the press under the wrapped notice"
    );
    app.press("id:status-undo").expect("press the Undo");
    app.wait_for("text:Undone.").expect("the Undo ran");
}
