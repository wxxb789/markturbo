//! The application root: startup, the checkpoint ticker, and the shell.
//!
//! Components run once; every changing value is a signal. The shell is re-keyed
//! when the language changes or the window crosses a layout breakpoint, so a
//! surface that is expensive to update (the settings page, a tree, a preview)
//! is rebuilt instead of patched.

use std::{cell::Cell, path::PathBuf, rc::Rc, time::Duration};

use mt_core::settings::Language;
use nsh_core::{desktop::WindowAttributes, element::desktop::main_window, prelude::*};

use crate::{
    state::AppState,
    ui::{self, tokens::Layout},
};

/// The application's root component.
#[component]
pub fn App() -> impl IntoElement {
    crate::logging::init();
    // The fonts travel with the root (`main.rs` registers them before the first
    // render); this covers the hosts that build the root themselves, and nsh
    // ignores a font it has already seen.
    ui::fonts::register();
    let state = AppState::new();
    // A downloaded companion CJK face is read, verified and registered on a
    // blocking thread, after the first frame is on its way.
    state.companion_font.restore();
    if let Some(path) = startup_path() {
        log::debug!("startup: opening {}", path.display());
        if path.is_dir() {
            state.open_folder(path);
        } else {
            if let Some(parent) = path.parent() {
                state.open_folder(parent.to_path_buf());
            }
            state.open_file(path);
        }
    }
    guard_window_close(&state);
    let tick_state = state.clone();
    let ticker_alive = Rc::new(Cell::new(true));
    let ticker_flag = ticker_alive.clone();
    let _ticker = stored(set_interval(Duration::from_secs(1), move || {
        if ticker_flag.get() {
            tick_state.checkpoint_tick();
        }
    }));
    let _ticker_guard = stored(TickerGuard(ticker_alive));
    let shell_state = state.clone();
    view()
        .flex_1()
        .h(relative(1.0))
        .w_full()
        .bg(move || ui::tokens::palette().room)
        .text_color(move || ui::tokens::palette().ink)
        .child(each(
            move || {
                vec![(
                    shell_state.settings.get().language,
                    Layout::of(ui::shell::client_width_now()),
                )]
            },
            |key: &(Language, Layout)| (language_code(key.0), layout_code(key.1)),
            {
                let state = state.clone();
                move |key: ReadSignal<(Language, Layout)>, _| {
                    let (language, layout) = key.get();
                    log::debug!(
                        "layout: {:?} at {} wide",
                        layout,
                        ui::shell::client_width_now()
                    );
                    ui::shell(state.clone(), language, layout).into_element()
                }
            },
        ))
}

/// The file or folder to open at startup: the first command-line argument,
/// else `MARKTURBO_OPEN`. Read here rather than in `main.rs` because nsh's
/// own hosts (`nsh run`, `nsh package`, `nsh.json` `"root"`) mount `App`
/// without running `main.rs`. Test binaries take only the variable: their
/// arguments are test filters.
fn startup_path() -> Option<PathBuf> {
    let argument = if cfg!(any(test, feature = "test-vault")) {
        None
    } else {
        std::env::args_os().nth(1)
    };
    argument
        .or_else(|| std::env::var_os("MARKTURBO_OPEN"))
        .map(PathBuf::from)
}

/// Routes every user close of the main window (its close button, Alt+F4, the
/// taskbar, the system menu) through the same guard as Quit: the window no
/// longer closes by itself, and [`AppState::request_quit`] closes it at once
/// when nothing is dirty, else raises the 'Close without saving?' guard.
fn guard_window_close(state: &AppState) {
    let window = main_window();
    // The option structs are `#[non_exhaustive]`: built from their defaults,
    // then set field by field.
    let mut attributes = WindowAttributes::default();
    attributes.close_on_press = Some(false);
    window.set(attributes);
    let state = state.clone();
    window.on_close_press(move |_| state.request_quit());
}

fn language_code(language: Language) -> u8 {
    match language {
        Language::English => 0,
        Language::Chinese => 1,
    }
}

fn layout_code(layout: Layout) -> u8 {
    match layout {
        Layout::Wide => 0,
        Layout::Medium => 1,
        Layout::Narrow => 2,
    }
}

/// Marks the checkpoint ticker inactive when the window's owner is disposed,
/// so a racing interval cannot read signals that are already gone.
struct TickerGuard(Rc<Cell<bool>>);

impl Drop for TickerGuard {
    fn drop(&mut self) {
        self.0.set(false);
    }
}
