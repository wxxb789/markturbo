//! Runs the Rust application directly on the machine's native backend.

#![cfg_attr(target_os = "windows", windows_subsystem = "windows")]

use nsh_core::access::Capabilities;
#[cfg(target_os = "linux")]
use nsh_platform_linux as platform;
#[cfg(target_os = "macos")]
use nsh_platform_macos as platform;
#[cfg(target_os = "windows")]
use nsh_platform_windows as platform;

#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
fn main() -> impl std::process::Termination {
    #[cfg(target_os = "windows")]
    let result = with_webview_network_block(
        |name, value| {
            // SAFETY: std::env::set_var is documented as safe on Windows, even when
            // other threads access the process environment.
            unsafe { std::env::set_var(name, value) }
        },
        run_app,
    );

    #[cfg(not(target_os = "windows"))]
    let result = run_app();

    result
}

#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
fn run_app() -> impl std::process::Termination {
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
    let mut root = platform::Root::of::<mt_app::App>()
        .tokens(tokens)
        .capabilities(app_capabilities());
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

#[cfg(any(target_os = "windows", test))]
const WEBVIEW2_ARGUMENTS_ENV: &str = "WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS";

// Fail name lookups, including the `.invalid` proxy; route HTTP(S) through that
// unreachable host; remove Chromium's implicit loopback bypass; and prevent
// WebRTC from using non-proxied UDP. The reserved host avoids assuming a local
// port is closed.
#[cfg(any(target_os = "windows", test))]
const WEBVIEW2_NETWORK_BLOCK_ARGUMENTS: &str = concat!(
    "--host-resolver-rules=\"MAP * ^NOTFOUND\" ",
    "--proxy-server=http://proxy.invalid:9 ",
    "--proxy-bypass-list=<-loopback> ",
    "--webrtc-ip-handling-policy=disable_non_proxied_udp",
);

#[cfg(any(target_os = "windows", test))]
fn with_webview_network_block<T>(
    set_environment: impl FnOnce(&str, &str),
    start_app: impl FnOnce() -> T,
) -> T {
    set_environment(WEBVIEW2_ARGUMENTS_ENV, WEBVIEW2_NETWORK_BLOCK_ARGUMENTS);
    start_app()
}

fn app_capabilities() -> Capabilities {
    // Keep the standalone app's existing capabilities while declaring no
    // external hosts for engine-mediated WebView loads.
    let mut capabilities = Capabilities::all();
    capabilities.webview = Some(Vec::new());
    capabilities
}

#[cfg(test)]
mod tests {
    use super::{app_capabilities, with_webview_network_block};
    use nsh_core::access::Access;
    use std::cell::RefCell;

    #[test]
    fn engine_webview_capability_allows_inline_documents_but_no_external_hosts() {
        let access = Access {
            declared: app_capabilities(),
            granted: None,
        };
        assert!(access.allows_webviews());
        assert!(access.allows_webview("about:blank"));
        assert!(!access.allows_webview("https://example.com/"));
        assert!(!access.allows_webview("file:///page.html"));
    }

    #[test]
    fn webview_network_block_replaces_inherited_arguments_before_startup() {
        const EXPECTED_ARGUMENTS: &str = "--host-resolver-rules=\"MAP * ^NOTFOUND\" --proxy-server=http://proxy.invalid:9 --proxy-bypass-list=<-loopback> --webrtc-ip-handling-policy=disable_non_proxied_udp";

        let inherited = RefCell::new(Some("--user-supplied-browser-arguments".to_owned()));
        let observations = RefCell::new(Vec::new());

        with_webview_network_block(
            |name, arguments| {
                *inherited.borrow_mut() = Some(arguments.to_owned());
                observations.borrow_mut().push((
                    "set".to_owned(),
                    name.to_owned(),
                    arguments.to_owned(),
                ));
            },
            || {
                observations.borrow_mut().push((
                    "first-webview".to_owned(),
                    String::new(),
                    inherited.borrow().as_deref().unwrap_or_default().to_owned(),
                ));
            },
        );

        assert_eq!(
            observations.into_inner(),
            [
                (
                    "set".to_owned(),
                    "WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS".to_owned(),
                    EXPECTED_ARGUMENTS.to_owned(),
                ),
                (
                    "first-webview".to_owned(),
                    String::new(),
                    EXPECTED_ARGUMENTS.to_owned(),
                ),
            ],
        );
    }
}

#[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
compile_error!(
    "Run this binary on macOS, Windows or Linux; use the TSX entry with nsh run for iOS and Android."
);
