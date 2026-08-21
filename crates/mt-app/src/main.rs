//! markturbo — a native workspace for Markdown as the interface between humans
//! and AI agents.

use std::path::PathBuf;

use gpui::*;
use gpui_component::Root;
use mt_app::assets::Assets;
use mt_app::views::workspace::Workspace;

const USAGE: &str = "\
markturbo — a native workspace for Markdown as the human-agent interface

USAGE:
    markturbo [PATH]

ARGS:
    PATH    A directory to open as a workspace, or a file to open (its parent
            becomes the workspace). Defaults to the current directory.

OPTIONS:
    -h, --help       Print this help
    -V, --version    Print the version

ENVIRONMENT:
    ANTHROPIC_API_KEY           Enables the Anthropic translation provider
    MARKTURBO_TRANSLATE_TO      Target language (default: zh)
    MARKTURBO_TRANSLATE_MODEL   Model id (default: claude-sonnet-5)
    RUST_LOG                    Log filter, e.g. RUST_LOG=debug
";

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
        // No argument: open the current directory. Launching from a terminal in
        // a repo is the common case, and it makes the app immediately useful.
        None => Ok(std::env::current_dir().ok()),
    }
}

fn main() {
    // GPUI composites its swap chain through DirectComposition, which sits on
    // top of the WebView2 child HWND — the Web preview loads and paints, and
    // then is covered by the window's own surface. Disabling composition puts
    // the swap chain on the HWND directly and lets the child window through.
    // This is what gpui-component's own webview example does, with the comment
    // "Required this for Windows to render the WebView".
    //
    // The cost is transparent window backgrounds, which need a composition swap
    // chain — this app only asks for those on Linux, so on Windows there is
    // nothing to trade away.
    //
    // SAFETY: single-threaded, before any window or background executor exists.
    #[cfg(target_os = "windows")]
    unsafe {
        std::env::set_var("GPUI_DISABLE_DIRECT_COMPOSITION", "true")
    };

    // Renderers run on background tasks and are wrapped in `catch_unwind`; a
    // panic there becomes an inline diagnostic. Logging it is still useful.
    env_logger::builder()
        .filter_level(log::LevelFilter::Info)
        .parse_default_env()
        .init();

    let args: Vec<String> = std::env::args().skip(1).collect();
    let target = match resolve_target(&args) {
        Ok(target) => target,
        Err((message, code)) => {
            if code == 0 {
                println!("{message}");
            } else {
                eprintln!("{message}");
            }
            std::process::exit(code);
        }
    };

    // Before the window, so MathJax's ~640ms engine start-up overlaps with it
    // rather than landing on the first document that contains a formula.
    mt_app::renderer::warm_up();

    let app = gpui_platform::application().with_assets(Assets);

    app.run(move |cx| {
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
                window_min_size: Some(gpui::Size {
                    width: px(720.),
                    height: px(480.),
                }),
                #[cfg(target_os = "linux")]
                window_background: gpui::WindowBackgroundAppearance::Transparent,
                #[cfg(target_os = "linux")]
                window_decorations: Some(gpui::WindowDecorations::Client),
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
    use super::resolve_target;

    fn args(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn no_argument_opens_the_current_directory() {
        let target = resolve_target(&[]).expect("should not exit");
        assert_eq!(target, std::env::current_dir().ok());
    }

    #[test]
    fn help_and_version_exit_cleanly() {
        for flag in ["-h", "--help"] {
            let (message, code) = resolve_target(&args(&[flag])).unwrap_err();
            assert_eq!(code, 0, "help is not an error");
            assert!(message.contains("USAGE"));
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
