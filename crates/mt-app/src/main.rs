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
    let result = with_webview_network_block(set_webview_environment, run_app);

    #[cfg(not(target_os = "windows"))]
    let result = run_app();

    result
}

#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
fn run_app() -> impl std::process::Termination {
    #[cfg(all(test, target_os = "windows"))]
    tests::observe_app_start();

    // The test harness injects the consent hold's platform source: a shipped
    // app must refuse it as firmly as the test vault.
    if cfg!(feature = "test-harness") {
        eprintln!(
            "This markturbo build enables the test harness; \
             rebuild with `cargo build -p mt-app` to run the app."
        );
        std::process::exit(2);
    }
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

#[cfg(target_os = "windows")]
fn set_webview_environment(name: &str, value: &str) {
    // SAFETY: std::env::set_var is documented as safe on Windows, even when
    // other threads access the process environment.
    unsafe { std::env::set_var(name, value) }
}

#[cfg(any(target_os = "windows", test))]
const WEBVIEW2_ARGUMENTS_ENV: &str = "WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS";

// WebView2 reads these browser arguments when its environment is created.
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
    let mut capabilities = Capabilities::all();
    capabilities.webview = Some(Vec::new());
    let mut open_url = nsh_core::access::OpenUrlGrant::default();
    open_url.schemes = ["http", "https", "mailto"].map(str::to_owned).to_vec();
    capabilities.open_url = Some(open_url);
    capabilities
}

#[cfg(test)]
mod tests {
    use super::{app_capabilities, with_webview_network_block};
    use nsh_core::access::Access;
    use std::cell::RefCell;

    #[cfg(target_os = "windows")]
    thread_local! {
        static APP_STARTUP: RefCell<Option<Box<dyn FnOnce()>>> = const { RefCell::new(None) };
    }

    #[cfg(target_os = "windows")]
    #[derive(Debug)]
    struct AppStartupReached;

    #[cfg(target_os = "windows")]
    pub(super) fn observe_app_start() {
        let callback = APP_STARTUP.with(|hook| hook.borrow_mut().take());
        if let Some(callback) = callback {
            callback();
        }
    }

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
        assert!(access.allows_open_url("https://example.com/"));
        assert!(access.allows_open_url("http://example.com/"));
        assert!(access.allows_open_url("mailto:owner@example.com"));
        assert!(!access.allows_open_url("tel:+15550100"));
        assert!(!access.allows_open_url("file:///page.html"));
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

    #[cfg(target_os = "windows")]
    #[test]
    fn main_applies_webview_network_block_before_starting_the_app() {
        use std::panic::{AssertUnwindSafe, catch_unwind};
        use std::process::Command;

        const CHILD_MODE: &str = "MT_MAIN_WEBVIEW_NETWORK_BLOCK_CHILD_TEST";
        const CHILD_SUCCESS: &str = "main-applied-webview-network-block-before-startup";

        if std::env::var_os(CHILD_MODE).is_none() {
            let output = Command::new(std::env::current_exe().expect("test executable path"))
                .args([
                    "--exact",
                    "tests::main_applies_webview_network_block_before_starting_the_app",
                    "--nocapture",
                ])
                .env(CHILD_MODE, "1")
                .env(
                    super::WEBVIEW2_ARGUMENTS_ENV,
                    "--inherited-browser-arguments",
                )
                .output()
                .expect("run app startup check in an isolated child process");
            assert!(
                output.status.success(),
                "child startup test failed\nstdout:\n{}\nstderr:\n{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr),
            );
            assert!(
                String::from_utf8_lossy(&output.stdout).contains(CHILD_SUCCESS),
                "child process exited successfully without running the startup check\nstdout:\n{}",
                String::from_utf8_lossy(&output.stdout),
            );
            return;
        }

        APP_STARTUP.with(|hook| {
            *hook.borrow_mut() = Some(Box::new(|| {
                let arguments = std::env::var(super::WEBVIEW2_ARGUMENTS_ENV).ok();
                assert_eq!(
                    arguments.as_deref(),
                    Some(super::WEBVIEW2_NETWORK_BLOCK_ARGUMENTS)
                );
                std::panic::panic_any(AppStartupReached);
            }))
        });

        let result = catch_unwind(AssertUnwindSafe(super::main));
        APP_STARTUP.with(|hook| *hook.borrow_mut() = None);

        let error = match result {
            Err(error) => error,
            Ok(_) => panic!("the test hook stops before opening a native window"),
        };
        assert!(
            error.is::<AppStartupReached>(),
            "the test reached app startup only after installing the network block"
        );
        println!("{CHILD_SUCCESS}");
    }
}

#[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
compile_error!(
    "Run this binary on macOS, Windows or Linux; use the TSX entry with nsh run for iOS and Android."
);
