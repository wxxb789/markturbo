//! Usage recording is explicit, inspectable, export-only and reversible.

use std::time::Duration;

use nsh_engine::{EngineConfig, Points, Root};
use nsh_testing::Headless;

mod support;

fn start() -> Headless {
    let mut app =
        Headless::start(EngineConfig::new(Root::of::<mt_app::App>())).expect("start the app");
    app.set_viewport(nsh_core::Size::new(Points(1280.0), Points(800.0)))
        .expect("wide viewport");
    app.wait_for("id:shell").expect("shell");
    app
}

/// Opens the settings page from the title row's menu: the gear is the room's
/// one icon, and the page lives behind it.
fn open_settings(app: &mut Headless) {
    app.press("id:title-settings").expect("open the menu");
    app.press("id:title-menu-settings").expect("open settings");
    app.wait_for("id:settings-page").expect("settings page");
}

#[test]
fn usage_metrics_consent_inspection_export_clear_and_localization() {
    let config = tempfile::tempdir().expect("config dir");
    let export_dir = tempfile::tempdir().expect("export dir");
    let store = config.path().join("data/usage.json");
    let settings = config.path().join("settings.toml");
    let export = export_dir.path().join("chosen-summary.json");
    // SAFETY: this binary has one test; each app is stopped before restarting.
    unsafe {
        std::env::remove_var("MARKTURBO_OPEN");
        std::env::set_var("MARKTURBO_CONFIG_DIR", config.path());
        std::env::set_var("MARKTURBO_DATA_DIR", config.path().join("data"));
    }

    let mut app = start();
    open_settings(&mut app);
    app.wait_for("id:usage-counters").expect("usage section");
    app.wait_for("id:usage-summary").expect("readable summary");
    app.wait_for("id:usage-retention").expect("retention note");
    app.wait_for(
        "text:0 retained UTC days · 0 Reviews completed · 0 revision decisions · 0 events",
    )
    .expect("empty summary");
    assert!(!mt_core::settings::SettingsData::load_from(&settings).usage_metrics_enabled);
    assert!(
        !store.exists(),
        "default-off startup and inspection wrote counts"
    );
    app.press("id:usage-on")
        .expect("explicitly enable recording");
    app.wait_for("text:Usage-counter preference saved.")
        .expect("consent persisted");
    assert!(mt_core::settings::SettingsData::load_from(&settings).usage_metrics_enabled);
    app.press("id:settings-close").expect("close settings");
    app.set_clipboard(Some("PRIVATE-PASTE-SENTINEL".to_owned()));
    // Paste is the welcome state's control: no document is open yet.
    app.press("id:welcome-paste").expect("paste an artifact");
    support::wait_for_path(&mut app, &store, "count the paste", Duration::from_secs(5));
    drop(app);

    let mut app = start();
    assert!(mt_core::settings::SettingsData::load_from(&settings).usage_metrics_enabled);
    open_settings(&mut app);
    app.press("id:usage-inspect").expect("inspect daily counts");
    app.wait_for("id:usage-details").expect("daily details");
    let json: serde_json::Value = serde_json::from_slice(&std::fs::read(&store).unwrap()).unwrap();
    let day = json["days"].as_object().unwrap().values().next().unwrap();
    assert_eq!(day["events"]["artifact_pasted"], 1);
    assert_eq!(day["events"]["first_launch"], 1);
    app.on_pick_save_path(Some(export.clone()))
        .expect("choose export destination");
    app.press("id:usage-export").expect("deliberate export");
    support::wait_for_path(
        &mut app,
        &export,
        "export aggregate",
        Duration::from_secs(5),
    );
    let bytes = std::fs::read(&export).unwrap();
    let exported: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(exported.as_object().unwrap().len(), 3);
    assert_eq!(exported["schema_version"], 1);
    assert_eq!(exported["app_version"], env!("CARGO_PKG_VERSION"));
    assert_eq!(exported["days"], json["days"]);
    for text in [
        String::from_utf8(bytes).unwrap(),
        std::fs::read_to_string(&store).unwrap(),
    ] {
        assert!(!text.contains("PRIVATE-PASTE-SENTINEL"));
        assert!(!text.contains("chosen-summary"));
        assert!(!text.contains(&config.path().to_string_lossy().to_string()));
    }
    app.press("id:usage-off").expect("disable recording");
    app.wait_for("text:Usage-counter preference saved.")
        .expect("disable saved");
    assert!(!mt_core::settings::SettingsData::load_from(&settings).usage_metrics_enabled);
    let disabled_bytes = std::fs::read(&store).unwrap();
    app.press("id:settings-close").expect("close settings");
    app.set_clipboard(Some("ANOTHER-PRIVATE-PASTE".to_owned()));
    app.press("id:welcome-paste")
        .expect("local paste while disabled");
    app.wait_for("text:ANOTHER-PRIVATE-PASTE")
        .expect("paste completed");
    assert_eq!(std::fs::read(&store).unwrap(), disabled_bytes);
    drop(app);

    let mut app = start();
    assert!(!mt_core::settings::SettingsData::load_from(&settings).usage_metrics_enabled);
    assert_eq!(std::fs::read(&store).unwrap(), disabled_bytes);
    open_settings(&mut app);
    app.press("id:usage-clear").expect("clear local aggregate");
    app.wait_for(
        "text:Local counts deleted. Recording preference and exported copies are unchanged.",
    )
    .expect("clear confirmation");
    assert!(!store.exists());
    assert!(export.exists(), "Clear deleted the user's exported copy");
    app.wait_for(
        "text:0 retained UTC days · 0 Reviews completed · 0 revision decisions · 0 events",
    )
    .expect("summary reflects Clear");
    app.press("id:settings-language-chinese")
        .expect("switch settings language");
    app.wait_for("text:本地使用计数")
        .expect("localized section");
    app.wait_for("text:查看计数").expect("localized inspection");
    app.wait_for("text:导出计数…").expect("localized export");
    app.wait_for("text:清除计数").expect("localized Clear");
    app.wait_for("text:保留 0 个 UTC 日 · 完成 0 次审查 · 0 次修订决定 · 0 个事件")
        .expect("localized summary");
    app.wait_for("text:计数保留 180 天，读取/保存时清理更早的日期。日期键为自 1970-01-01 起的 UTC 日数。关闭即停止记录；清除仅删除本地计数，不删除您导出的副本。")
        .expect("localized retention policy");
}
