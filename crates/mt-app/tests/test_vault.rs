//! An integration test's app never reaches the platform's secure store: a key
//! stored through Settings lands in the test build's in-memory store.

use nsh_engine::{EngineConfig, Points, Root};
use nsh_testing::Headless;

#[test]
fn a_key_stored_through_settings_lands_in_the_test_store() {
    let config = tempfile::tempdir().expect("config dir");
    let base_url = "http://127.0.0.1:9/v1";
    // A custom endpoint with no environment identity: the status comes from
    // the store alone.
    std::fs::write(
        config.path().join("settings.toml"),
        format!("model-provider = \"openai-chat\"\nmodel-base-url = \"{base_url}\"\n"),
    )
    .expect("write settings");
    let target = mt_core::model::EndpointIdentity::parse(
        mt_core::model::Provider::OpenAiChat,
        Some(base_url),
    )
    .expect("endpoint")
    .credential_target()
    .to_string();
    assert_eq!(mt_app::test_vault::stored(&target), None);

    // SAFETY: this binary has one test.
    unsafe {
        std::env::remove_var("MARKTURBO_OPEN");
        std::env::set_var("MARKTURBO_CONFIG_DIR", config.path());
        std::env::set_var("MARKTURBO_DATA_DIR", config.path().join("data"));
        std::env::set_var(
            "MARKTURBO_TEST_LOCK_NAMESPACE",
            std::process::id().to_string(),
        );
    }
    let mut app =
        Headless::start(EngineConfig::new(Root::of::<mt_app::App>())).expect("start the app");
    app.set_viewport(nsh_core::Size::new(Points(1280.0), Points(800.0)))
        .expect("wide viewport");
    app.wait_for("id:shell").expect("shell");
    app.press("id:title-settings").expect("open the menu");
    app.press("id:title-menu-settings").expect("open settings");
    app.wait_for("id:api-key").expect("credential field");
    app.type_text("id:api-key", "synthetic-vault-key")
        .expect("type a key");
    app.press("text:Store securely").expect("store securely");
    app.wait_for("text:Key stored securely.")
        .expect("the store reports success");

    assert_eq!(
        mt_app::test_vault::stored(&target).as_deref(),
        Some("synthetic-vault-key"),
        "the key went somewhere other than the test store"
    );
}
