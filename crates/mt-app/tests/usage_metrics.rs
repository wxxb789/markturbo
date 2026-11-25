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
        std::env::set_var(
            "MARKTURBO_TEST_LOCK_NAMESPACE",
            std::process::id().to_string(),
        );
    }

    let mut app = start();
    open_settings(&mut app);
    app.wait_for("id:usage-counters").expect("usage section");
    app.wait_for("id:usage-summary").expect("readable summary");
    app.wait_for("id:usage-retention").expect("retention note");
    app.wait_for(
        "text:Last 180 days: 0 days active · 0 Reviews · 0 revision decisions · 0 total events",
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
    app.wait_for("text:Hide details")
        .expect("the toggle opened");
    app.press("id:usage-inspect").expect("hide daily counts");
    app.wait_for_gone("id:usage-details", Duration::from_secs(5))
        .expect("the toggle closes the details");
    app.wait_for("text:Show details")
        .expect("the toggle closed");
    app.press("id:usage-inspect")
        .expect("inspect daily counts again");
    app.wait_for("id:usage-details")
        .expect("daily details again");
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
    app.press("id:usage-on")
        .expect("enable recording for the clear tests");
    app.wait_for("text:Usage-counter preference saved.")
        .expect("recording enabled");
    app.press("id:usage-clear").expect("clear local aggregate");
    app.wait_for(
        "text:Local counts deleted. Recording preference and exported copies are unchanged.",
    )
    .expect("clear confirmation");
    app.wait_for(
        "text:Last 180 days: 0 days active · 0 Reviews · 0 revision decisions · 0 total events",
    )
    .expect("summary reflects Clear at once");
    assert!(
        store.exists(),
        "Clear committed before its undo hold elapsed"
    );
    app.press("id:status-undo").expect("undo the clear");
    app.wait_for(
        "text:Last 180 days: 1 days active · 0 Reviews · 0 revision decisions · 2 total events",
    )
    .expect("the summary is back");
    assert!(store.exists(), "Undo left the store deleted");
    app.press("id:usage-clear")
        .expect("clear the local aggregate again");
    app.wait_for(
        "text:Last 180 days: 0 days active · 0 Reviews · 0 revision decisions · 0 total events",
    )
    .expect("summary reflects the second Clear");
    app.set_clipboard(Some("HELD-PASTE-SENTINEL".to_owned()));
    app.press("id:title-settings")
        .expect("open the title menu over Settings");
    app.press("id:title-menu-paste")
        .expect("paste during the hold");
    app.wait_for(
        "text:Last 180 days: 1 days active · 0 Reviews · 0 revision decisions · 1 total events",
    )
    .expect("an event recorded during the hold belongs to the cleared state");
    app.press("id:usage-inspect")
        .expect("inspect the held counts");
    app.wait_for("id:usage-details").expect("daily details");
    let held_export = export_dir.path().join("held-summary.json");
    app.on_pick_save_path(Some(held_export.clone()))
        .expect("choose the export destination");
    app.press("id:usage-export")
        .expect("export the held counts");
    support::wait_for_path(
        &mut app,
        &held_export,
        "export the held state",
        Duration::from_secs(5),
    );
    let held: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&held_export).unwrap()).unwrap();
    let days = held["days"].as_object().expect("the exported days");
    assert_eq!(days.len(), 1, "the export carries one day");
    let day = days.values().next().unwrap();
    assert_eq!(
        day["events"].as_object().unwrap().len(),
        1,
        "the export matches the details: only the held event"
    );
    assert_eq!(day["events"]["artifact_pasted"], 1);
    assert!(
        !std::fs::read_to_string(&held_export)
            .unwrap()
            .contains("HELD-PASTE-SENTINEL")
    );
    app.press("id:usage-inspect")
        .expect("close the held details");
    app.wait(Duration::from_millis(
        mt_app::ui::motion::UNDO_HOLD_MS + 500,
    ))
    .expect("the undo hold elapses");
    let committed: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&store).unwrap()).unwrap();
    let days = committed["days"].as_object().expect("the committed days");
    assert_eq!(days.len(), 1, "the commit leaves one day");
    let day = days.values().next().unwrap();
    assert_eq!(
        day["events"].as_object().unwrap().len(),
        1,
        "the commit dropped the cleared counts"
    );
    assert_eq!(
        day["events"]["artifact_pasted"], 1,
        "the commit kept the event recorded during the hold"
    );
    app.press("id:usage-clear")
        .expect("clear the held counts again");
    app.wait_for(
        "text:Last 180 days: 0 days active · 0 Reviews · 0 revision decisions · 0 total events",
    )
    .expect("summary reflects the third Clear");
    app.set_clipboard(Some("HELD-AGAIN".to_owned()));
    app.press("id:title-settings")
        .expect("open the title menu over Settings");
    app.press("id:title-menu-paste")
        .expect("paste during the hold again");
    app.wait_for(
        "text:Last 180 days: 1 days active · 0 Reviews · 0 revision decisions · 1 total events",
    )
    .expect("the pasted event shows during the hold");
    app.press("id:status-undo")
        .expect("undo the cleared counts");
    app.wait_for(
        "text:Last 180 days: 1 days active · 0 Reviews · 0 revision decisions · 2 total events",
    )
    .expect("Undo restores the old counts plus the event recorded during the hold");
    app.press("id:usage-clear")
        .expect("clear the local aggregate for good");
    app.wait(Duration::from_millis(
        mt_app::ui::motion::UNDO_HOLD_MS + 500,
    ))
    .expect("the undo hold elapses");
    assert!(!store.exists(), "Clear never committed after the hold");
    assert!(export.exists(), "Clear deleted the user's exported copy");
    app.press("id:settings-language-chinese")
        .expect("switch settings language");
    app.wait_for("text:本地使用计数")
        .expect("localized section");
    app.wait_for("text:显示明细").expect("localized inspection");
    app.wait_for("text:导出计数…").expect("localized export");
    app.wait_for("text:清除计数").expect("localized Clear");
    app.wait_for("text:近 180 天：活跃 0 天 · 完成审查 0 次 · 修订决定 0 次 · 共 0 条记录")
        .expect("localized summary");
    app.wait_for("text:计数保留 180 天，过期自动清理。关闭即停止记录；“清除计数”仅删除本机记录，不影响已导出的文件。")
        .expect("localized retention policy");
}
