use nsh_core::{
    Size,
    element::{Idiom, Os},
    platform::ColorScheme,
};
use nsh_engine::{EngineConfig, Points, Root};
use nsh_testing::{Headless, LayoutNode};
use std::sync::Mutex;

static TEST_LOCK: Mutex<()> = Mutex::new(());

fn node(app: &Headless, test_id: &str) -> LayoutNode {
    app.screen()
        .layout_nodes()
        .into_iter()
        .find(|node| node.test_id.as_deref() == Some(test_id))
        .unwrap_or_else(|| panic!("`{test_id}` is on screen"))
}

fn start_settings() -> (Headless, tempfile::TempDir, tempfile::TempDir) {
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
    // The viewport before the window opens: the declared window sizes
    // itself from it, so the page lays out at this size.
    engine.viewport = Size::new(Points(1280.0), Points(900.0));
    let mut app = Headless::start(engine).expect("start app");
    app.wait_for("id:shell").expect("shell");
    app.press("id:title-settings").expect("open title menu");
    app.press("id:title-menu-settings").expect("open settings");
    app.wait_for("id:settings-page").expect("settings page");
    app.settle();
    (app, config, codex_home)
}

fn make_settings_file_unwritable(config: &tempfile::TempDir) {
    std::fs::create_dir(config.path().join("settings.toml")).expect("block settings file");
}

#[test]
fn settings_actions_end_at_the_field_column() {
    let _guard = TEST_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    let (app, _config, _codex_home) = start_settings();
    let field = node(&app, "settings-model").bounds;
    let close = node(&app, "settings-close").bounds;
    let field_right = field.origin.x.0 + field.size.width.0;
    let action_right = close.origin.x.0 + close.size.width.0;
    assert!(
        (action_right - field_right).abs() <= 0.5,
        "the header action ends at field column x={field_right}: close={close:?}, field={field:?}"
    );
    assert!(
        !app.screen()
            .matches("id:settings-save")
            .expect("valid settings-save selector"),
        "auto-save leaves no Save control in the header"
    );
}

#[test]
fn settings_page_rows_land_on_the_four_point_grid() {
    let _guard = TEST_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    let (app, _config, _codex_home) = start_settings();
    for id in [
        "settings-header",
        "settings-scroll",
        "settings-content",
        "settings-endpoint",
        "settings-provider",
        "settings-model",
        "settings-credential",
        "api-key",
        "settings-translation",
        "settings-target-language",
        "settings-language",
        "settings-theme",
        "settings-cjk-font",
        "usage-counters",
    ] {
        let y = node(&app, id).bounds.origin.y.0;
        let snapped = (y / mt_app::ui::tokens::SPACE_1).round() * mt_app::ui::tokens::SPACE_1;
        assert!(
            (y - snapped).abs() <= 0.5,
            "`{id}` starts on the 4px grid: y={y}"
        );
    }
}

#[test]
fn selected_theme_uses_an_ink_two_pixel_underline() {
    let _guard = TEST_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    let (app, _config, _codex_home) = start_settings();
    let selected = app
        .screen()
        .accessibility_state("id:settings-theme-system")
        .expect("theme selection state");
    assert!(selected.selected, "System is the selected theme");
    let style = app
        .screen()
        .span_style("id:settings-theme-system >> text:/^System$/")
        .expect("selected theme text");
    assert_eq!(style.color, mt_app::ui::tokens::LIGHT.ink);
    assert!(style.decoration.underline, "selected theme is underlined");
    assert_eq!(style.decoration_thickness, Some(Points(2.0)));
    assert_eq!(style.decoration_color, Some(mt_app::ui::tokens::LIGHT.ink));
}

#[test]
fn the_last_settings_section_is_reachable_by_scrolling() {
    let _guard = TEST_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    let (mut app, _config, _codex_home) = start_settings();
    let scroll = node(&app, "settings-scroll");
    let content_height = scroll.scroll.unwrap().content_size.unwrap().height.0;
    assert!(
        content_height > scroll.bounds.size.height.0,
        "settings content {content_height}px exceeds the {}px viewport",
        scroll.bounds.size.height.0
    );
    let usage_before = node(&app, "usage-counters").bounds;
    let scroll_bottom = scroll.bounds.origin.y.0 + scroll.bounds.size.height.0;
    let usage_before_bottom = usage_before.origin.y.0 + usage_before.size.height.0;
    assert!(
        usage_before_bottom > scroll_bottom + 0.5,
        "the last section initially extends below the viewport: usage={usage_before:?}, scroll={:?}",
        scroll.bounds
    );
    app.scroll("id:settings-scroll", Points(10_000.0))
        .expect("scroll settings to the last section");
    app.settle();
    let scroll = node(&app, "settings-scroll").bounds;
    let usage = node(&app, "usage-counters").bounds;
    let usage_bottom = usage.origin.y.0 + usage.size.height.0;
    let scroll_bottom = scroll.origin.y.0 + scroll.size.height.0;
    assert!(
        usage.origin.y.0 < usage_before.origin.y.0,
        "scrolling moves the last section into view: before={usage_before:?}, after={usage:?}"
    );
    assert!(
        usage.origin.y.0 >= scroll.origin.y.0 - 0.5 && usage_bottom <= scroll_bottom + 0.5,
        "last section is fully reachable inside the scroll viewport: usage={usage:?}, scroll={scroll:?}"
    );
}

#[test]
fn settings_notice_info_uses_neutral_ink() {
    let _guard = TEST_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    let (mut app, _config, _codex_home) = start_settings();
    app.press("text:OpenAI Chat Completions")
        .expect("configure a wire format");
    app.wait_for("text:No credential stored")
        .expect("configured status");
    app.press("id:settings-credential-session")
        .expect("empty credential reports an info notice");
    let info = "text:Enter a credential first.";
    app.wait_for(info).expect("info notice");
    assert_eq!(
        app.screen().span_style(info).expect("info notice").color,
        mt_app::ui::tokens::LIGHT.ink_rest
    );
}

#[test]
fn settings_notice_errors_use_danger_ink() {
    let _guard = TEST_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    let (mut app, config, _codex_home) = start_settings();
    make_settings_file_unwritable(&config);
    app.scroll("id:settings-scroll", Points(10_000.0))
        .expect("scroll to usage settings");
    app.settle();
    app.press("id:usage-on")
        .expect("enable consent against the unwritable settings file");
    let error = "text:Could not save the preference. Recording is off for this session; try again before restarting.";
    app.wait_for(error)
        .expect("settings notice reports the persistence failure");
    assert_eq!(
        app.screen().span_style(error).expect("error notice").color,
        mt_app::ui::tokens::LIGHT.danger
    );
}

#[test]
fn settings_save_failure_uses_danger_status_on_the_settings_surface() {
    let _guard = TEST_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    let (mut app, config, _codex_home) = start_settings();
    make_settings_file_unwritable(&config);
    app.press("id:settings-theme-light")
        .expect("auto-save a theme choice against the unwritable settings file");
    let error = "id:status >> text:Could not save settings. Check that the settings file is writable, then try again.";
    app.wait_for(error)
        .expect("settings save failure on its surface");
    assert_eq!(
        app.screen()
            .span_style(error)
            .expect("settings status")
            .color,
        mt_app::ui::tokens::LIGHT.danger
    );
    let page = node(&app, "settings-page").bounds;
    let status = node(&app, "status").bounds;
    assert!(
        status.origin.y.0 >= page.origin.y.0 - 0.5
            && status.origin.y.0 + status.size.height.0
                <= page.origin.y.0 + page.size.height.0 + 0.5,
        "status remains beside Settings: page={page:?}, status={status:?}"
    );
}

#[test]
fn closing_settings_shows_the_save_receipt_in_the_editor() {
    let _guard = TEST_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    let (mut app, _config, _codex_home) = start_settings();
    app.press("id:settings-close").expect("save and close");
    app.wait_for("id:column >> id:status >> text:Settings saved.")
        .expect("Close shows the receipt in the returning editor column");
}
