//! The shell must rebuild its copy when the interface language changes.

use nsh_engine::{EngineConfig, Points, Root};
use nsh_testing::Headless;

#[test]
fn switching_language_relocalizes_the_live_shell() {
    let config = tempfile::tempdir().expect("config dir");

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
    app.wait_for("text:Open folder\u{2026}")
        .expect("English shell");

    // Settings live behind the glyph's menu: Open it, then the page.
    app.press("id:title-settings")
        .expect("open the glyph's menu");
    app.wait_for("id:title-menu-settings")
        .expect("the Settings row");
    app.press("id:title-menu-settings")
        .expect("open the settings page");
    app.wait_for("id:settings-page").expect("the settings page");
    app.wait_for("text:Settings").expect("English settings");
    app.wait_for("text:Language").expect("the language row");

    // The choice's test id sits on the wrapper; the press goes to the
    // button's own label inside it.
    app.press("label:简体中文").expect("switch language");
    app.wait_for("text:界面语言").expect("Chinese settings");
    app.wait_for("text:设置").expect("Chinese heading");
    assert!(!app.shows("Language"), "English settings copy remained");

    // The room behind the page is Chinese too, as soon as it is back.
    // The header's Close control: `关闭` is also the usage toggle's Off
    // choice, so the header scopes the control.
    app.press("id:settings-header >> label:关闭")
        .expect("close the settings page");
    app.wait_for("id:welcome").expect("the welcome surface");
    app.wait_for("text:打开文件夹\u{2026}")
        .expect("Chinese shell");
    app.wait_for("text:打开、编辑与搜索都在这台设备上完成。")
        .expect("Chinese welcome guidance");
    assert!(
        !app.shows("Open folder\u{2026}"),
        "English shell copy remained"
    );
    assert!(!app.shows("Settings"), "English shell copy remained");
}
