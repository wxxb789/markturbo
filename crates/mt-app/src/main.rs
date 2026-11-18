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
    // `cargo test` builds this binary with the crate's test features, so the
    // copy it leaves in target/<profile>/ keeps keys in memory only. Such a
    // build must never pass for the shipped app or accept a person's key.
    if cfg!(feature = "test-vault") {
        eprintln!(
            "This markturbo build was made by `cargo test` and has no secure credential store; \
             rebuild with `cargo build -p mt-app` to run the app."
        );
        std::process::exit(2);
    }
    mt_app::logging::init();
    let tokens = mt_app::ui::tokens::theme();
    // The faces travel with the root, so the first frame is already measured
    // with them (the app registers them again at startup for hosts that build
    // the root themselves; nsh ignores a font it has seen).
    let mut root = platform::Root::of::<mt_app::App>().tokens(tokens);
    for face in mt_app::ui::fonts::faces() {
        root = root.font(face);
    }
    // Startup tokens travel with the root; reactive set_tokens requires the
    // engine runtime, which run_app has not started yet.
    platform::run_app(
        platform::AppSource::Rust(root),
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
