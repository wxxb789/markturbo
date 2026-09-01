#![cfg_attr(
    all(target_os = "windows", not(debug_assertions)),
    windows_subsystem = "windows"
)]

//! markturbo — a native workspace for Markdown as the interface between humans
//! and AI agents.

use std::fs::{File, OpenOptions};
use std::io;
use std::path::Path;
use std::path::PathBuf;
#[cfg(target_os = "linux")]
use std::sync::Arc;

use gpui::*;
use gpui_component::Root;
use mt_app::assets::Assets;
use mt_app::views::workspace::Workspace;

const APP_ID: &str = "io.github.wxxb789.markturbo";

#[cfg(target_os = "linux")]
fn linux_window_icon() -> Arc<image::RgbaImage> {
    Arc::new(
        image::load_from_memory_with_format(
            include_bytes!("../resources/icons/markturbo-256.png"),
            image::ImageFormat::Png,
        )
        .expect("the checked-in markturbo icon must be a valid PNG")
        .into_rgba8(),
    )
}

const USAGE: &str = "\
markturbo — a native workspace for Markdown as the human-agent interface

USAGE:
    markturbo [PATH]

ARGS:
    PATH    A directory to open as a workspace, or a file to open (its parent
            becomes the workspace). Omit for the welcome screen; use `.` to
            open the current directory.

OPTIONS:
    -h, --help       Print this help
    -V, --version    Print the version

ENVIRONMENT:
    ANTHROPIC_API_KEY           Anthropic key, if not set in Settings
    OPENAI_API_KEY              OpenAI key, if not set in Settings
    MARKTURBO_TRANSLATE_MODEL   Model id, if not set in Settings
    MT_MATH_FONT_DIR            Folder holding the KaTeX fonts, if they are
                                not beside the executable or installed
    MARKTURBO_DATA_DIR          Override the platform runtime-data directory
    RUST_LOG                    Log filter, e.g. RUST_LOG=debug
";

fn open_log_file(path: &Path) -> Option<File> {
    let parent = path.parent()?;
    std::fs::create_dir_all(parent).ok()?;
    OpenOptions::new().create(true).append(true).open(path).ok()
}

fn init_logging() {
    let log_path = mt_app::app_paths::log_path();
    let file = log_path.as_deref().and_then(open_log_file);
    let active_path = file.as_ref().and(log_path);
    let target = file
        .map(|file| env_logger::Target::Pipe(Box::new(file)))
        .unwrap_or_else(|| env_logger::Target::Pipe(Box::new(io::sink())));

    env_logger::builder()
        .filter_level(log::LevelFilter::Info)
        .parse_default_env()
        .write_style(env_logger::WriteStyle::Never)
        .target(target)
        .init();

    #[cfg(not(all(target_os = "windows", not(debug_assertions))))]
    let default_panic_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        log::error!("panic: {info}");
        #[cfg(not(all(target_os = "windows", not(debug_assertions))))]
        default_panic_hook(info);
    }));
    if let Some(path) = active_path {
        log::info!(
            "markturbo {} started; pid={}; log={}",
            env!("CARGO_PKG_VERSION"),
            std::process::id(),
            path.display()
        );
    }
}

/// Resolve the workspace to open from the command line.
///
/// Returns `Err` when the process should exit without opening a window (help,
/// version, or a bad path) — reported by the caller so `main` stays linear.
fn resolve_target(args: &[String]) -> Result<Option<PathBuf>, (String, i32)> {
    match args.first().map(String::as_str) {
        Some("-h" | "--help") => Err((USAGE.to_string(), 0)),
        Some("-V" | "--version") => Err((format!("markturbo {}", env!("CARGO_PKG_VERSION")), 0)),
        Some(flag) if flag.starts_with('-') => {
            Err((format!("unknown option `{flag}`\n\n{USAGE}"), 2))
        }
        Some(path) => {
            let path = PathBuf::from(path);
            if !path.exists() {
                return Err((format!("no such file or directory: {}", path.display()), 1));
            }
            // Canonicalize so the file watcher and the tree agree on one form,
            // and so a relative argument survives any later cwd change.
            Ok(Some(path.canonicalize().unwrap_or(path)))
        }
        // No target is an intentional first-use state. Terminal callers that
        // want cwd pass `.` explicitly, which keeps desktop and CLI behavior
        // deterministic without platform heuristics.
        None => Ok(None),
    }
}

fn main() {
    // GPUI's DirectComposition swap chain covers ordinary child HWNDs. The
    // WebView worker therefore uses the compatibility compositor, matching
    // gpui-component's WebView example, so its WS_CHILD host can remain inside
    // the one application window.
    //
    // SAFETY: this is single-threaded startup, before the application or any
    // worker exists.
    #[cfg(target_os = "windows")]
    unsafe {
        std::env::set_var("GPUI_DISABLE_DIRECT_COMPOSITION", "true")
    };

    // Renderers run on background tasks and are wrapped in `catch_unwind`; a
    // panic there becomes an inline diagnostic. The logger and the process panic
    // hook both write under the per-user application data directory, never to a
    // release console window.
    init_logging();

    let args: Vec<String> = std::env::args().skip(1).collect();
    let target = match resolve_target(&args) {
        Ok(target) => target,
        Err((message, code)) => {
            if code == 0 {
                println!("{message}");
            } else {
                log::error!("{message}");
                eprintln!("{message}");
            }
            std::process::exit(code);
        }
    };

    let app = gpui_platform::application().with_assets(Assets);

    app.run(move |cx| {
        // Set the process identity before opening a window. Windows uses this
        // as its AppUserModelID; Linux matches it to the staged desktop file.
        cx.set_app_identity(APP_ID, "markturbo");
        // Must come before any component is constructed.
        gpui_component::init(cx);
        // Before the first window: `Workspace::new` reads the saved theme to
        // apply it ahead of the first frame.
        mt_app::settings::AppSettings::init(cx);
        mt_app::views::workspace::init(cx);

        let mut window_size = size(px(1400.0), px(900.0));
        if let Some(display) = cx.primary_display() {
            let bounds = display.bounds().size;
            window_size.width = window_size.width.min(bounds.width * 0.9);
            window_size.height = window_size.height.min(bounds.height * 0.9);
        }
        let window_bounds = Bounds::centered(None, window_size, cx);

        cx.spawn(async move |cx| {
            let options = WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(window_bounds)),
                app_id: Some(APP_ID.to_owned()),
                window_min_size: Some(gpui::Size {
                    width: px(720.),
                    height: px(480.),
                }),
                #[cfg(target_os = "linux")]
                window_background: gpui::WindowBackgroundAppearance::Transparent,
                #[cfg(target_os = "linux")]
                window_decorations: Some(gpui::WindowDecorations::Client),
                #[cfg(target_os = "linux")]
                icon: Some(linux_window_icon()),
                kind: WindowKind::Normal,
                ..gpui_component::TitleBar::window_options()
            };

            // Held so the window can focus it below: keybindings only dispatch
            // along the focused element's path, and nothing focuses the
            // workspace on its own — so without this, Ctrl+O and friends do
            // nothing until something has been clicked.
            let mut workspace = None;
            let window = cx
                .open_window(options, |window, cx| {
                    let view = cx.new(|cx| Workspace::new(target, window, cx));
                    workspace = Some(view.clone());
                    cx.new(|cx| Root::new(view, window, cx))
                })
                .expect("failed to open window");

            window
                .update(cx, |_, window, cx| {
                    window.activate_window();
                    window.set_window_title("markturbo");
                    if let Some(workspace) = workspace {
                        let handle = workspace.read(cx).focus_handle(cx);
                        window.focus(&handle, cx);
                    }
                    cx.on_release(|_, cx| cx.quit()).detach();
                })
                .expect("failed to configure window");
        })
        .detach();
    });
}

#[cfg(test)]
mod tests {
    // Import selectively: the `gpui::*` glob in the parent re-exports a `test`
    // attribute macro that shadows the built-in one and blows the recursion
    // limit.
    use super::{open_log_file, resolve_target};
    use std::io::Write as _;

    fn args(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    /// Windows must use GPUI's child-HWND compatibility compositor.
    #[test]
    fn direct_composition_is_disabled_before_the_window_exists() {
        let source = include_str!("main.rs");
        let test_module = source
            .find("\n#[cfg(test)]")
            .expect("the test module marker");
        let source = &source[..test_module];
        let disable_key = ["GPUI_DISABLE_DIRECT_", "COMPOSITION"].concat();
        let disable = source
            .find(&format!("set_var(\"{disable_key}\""))
            .expect("the compatibility switch");
        let application = source
            .find("gpui_platform::application()")
            .expect("GPUI application initialization");

        assert!(
            disable < application,
            "the compositor is selected before GPUI initializes"
        );
        assert!(
            !source.contains("cfg(any(target_os = \"windows\", target_os = \"linux\"))"),
            "Windows transparent backgrounds require DirectComposition and conflict with the compatibility path"
        );
        assert!(
            source.contains("cx.new(|cx| Root::new(view, window, cx))"),
            "the main window keeps one ordinary GPUI root"
        );
    }

    #[test]
    fn release_windows_uses_gui_subsystem_and_file_logging() {
        let source = include_str!("main.rs");
        let test_module = source
            .find("\n#[cfg(test)]")
            .expect("the test module marker");
        let source = &source[..test_module];

        assert!(source.contains("all(target_os = \"windows\", not(debug_assertions))"));
        assert!(source.contains("windows_subsystem = \"windows\""));
        assert!(source.contains("mt_app::app_paths::log_path()"));
        assert!(source.contains("env_logger::Target::Pipe"));
        assert!(source.contains("io::sink()"));
        assert!(!source.contains("env_logger::Target::Stderr"));
        assert!(source.contains("std::panic::set_hook"));
        assert!(source.contains("std::panic::take_hook"));

        let logging = source
            .find("\n    init_logging();")
            .expect("file logging initialization");
        let arguments = source
            .find("std::env::args()")
            .expect("command-line argument handling");
        assert!(
            logging < arguments,
            "logging starts before argument handling"
        );
    }

    #[test]
    fn log_files_append_without_truncating_previous_output() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("logs/markturbo-42.log");

        open_log_file(&path).unwrap().write_all(b"one\n").unwrap();
        open_log_file(&path).unwrap().write_all(b"two\n").unwrap();

        assert_eq!(std::fs::read_to_string(path).unwrap(), "one\ntwo\n");
    }

    #[test]
    fn no_argument_leaves_target_for_the_welcome_flow() {
        let target = resolve_target(&[]).expect("should not exit");
        assert_eq!(target, None);
    }

    #[test]
    fn a_dot_argument_keeps_the_explicit_current_directory_launch() {
        let target = resolve_target(&args(&["."]))
            .expect("should open")
            .expect("explicit target");
        assert!(target.is_dir());
    }

    #[test]
    fn app_identity_matches_the_desktop_integration() {
        assert_eq!(super::APP_ID, "io.github.wxxb789.markturbo");
        let icon = image::load_from_memory_with_format(
            include_bytes!("../resources/icons/markturbo-256.png"),
            image::ImageFormat::Png,
        )
        .expect("the runtime icon must decode")
        .into_rgba8();
        assert_eq!(
            (icon.width(), icon.height()),
            (256, 256),
            "the runtime icon must stay at the X11 integration size"
        );
    }

    #[test]
    fn help_and_version_exit_cleanly() {
        for flag in ["-h", "--help"] {
            let (message, code) = resolve_target(&args(&[flag])).unwrap_err();
            assert_eq!(code, 0, "help is not an error");
            assert!(message.contains("USAGE"));
            assert!(message.contains("welcome screen"));
            assert!(message.contains("use `.`"));
        }
        for flag in ["-V", "--version"] {
            let (message, code) = resolve_target(&args(&[flag])).unwrap_err();
            assert_eq!(code, 0);
            assert!(message.contains(env!("CARGO_PKG_VERSION")));
        }
    }

    #[test]
    fn an_unknown_flag_is_an_error_with_usage() {
        let (message, code) = resolve_target(&args(&["--nope"])).unwrap_err();
        assert_eq!(code, 2);
        assert!(message.contains("--nope"));
        assert!(message.contains("USAGE"), "must show how to use it");
    }

    #[test]
    fn a_missing_path_is_reported_rather_than_opening_an_empty_window() {
        let (message, code) = resolve_target(&args(&["Q:/definitely/not/here/xyz"])).unwrap_err();
        assert_eq!(code, 1);
        assert!(message.contains("no such file or directory"));
    }

    #[test]
    fn a_directory_argument_is_used_as_given() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().to_path_buf();
        let target = resolve_target(&args(&[path.to_str().unwrap()]))
            .expect("should open")
            .expect("some path");
        // Canonicalization may add a UNC prefix on Windows; compare resolved
        // forms rather than strings.
        assert_eq!(target.canonicalize().unwrap(), path.canonicalize().unwrap());
    }

    #[test]
    fn a_file_argument_is_accepted() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("README.md");
        std::fs::write(&file, "# Hi\n").unwrap();
        let target = resolve_target(&args(&[file.to_str().unwrap()]))
            .expect("should open")
            .expect("some path");
        assert!(target.is_file(), "the file itself is passed through");
    }
}
