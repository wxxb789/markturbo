//! Runs the Rust application directly on the machine's native backend.

#![cfg_attr(target_os = "windows", windows_subsystem = "windows")]

#[cfg(target_os = "linux")]
use nsh_platform_linux as platform;
#[cfg(target_os = "macos")]
use nsh_platform_macos as platform;
#[cfg(target_os = "windows")]
use nsh_platform_windows as platform;

#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
fn main() -> impl std::process::Termination {
    env_logger::init();
    if let Some(path) = std::env::args_os().nth(1) {
        // SAFETY: this runs before the app starts and no other thread reads the
        // environment concurrently.
        unsafe { std::env::set_var("MARKTURBO_OPEN", path) };
    }
    let tokens = nsh_base::tokens_of(Some(include_str!("../theme.json")))
        .expect("theme.json is a valid nsh theme");
    nsh_core::prelude::set_tokens(tokens);
    platform::run_app(
        platform::AppSource::Rust(platform::Root::of::<mt_app::App>()),
        platform::AppOptions {
            title: "MarkTurbo".to_owned(),
            #[cfg(any(target_os = "linux", target_os = "windows"))]
            app_id: "com.markturbo.app".to_owned(),
            size: (1280, 800),
            ..Default::default()
        },
    )
}

#[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
compile_error!(
    "Run this binary on macOS, Windows or Linux; use the TSX entry with nsh run for iOS and Android."
);
