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
    let config = tempfile::tempdir().expect("config dir");
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
