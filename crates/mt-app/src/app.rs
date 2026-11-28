//! The application root: startup, the checkpoint ticker, and the shell.
//!
//! Components run once; every changing value is a signal. The shell is re-keyed
//! when the language changes or the window crosses a layout breakpoint, so a
//! surface that is expensive to update (the settings page, a tree, a preview)
//! is rebuilt instead of patched.

use std::{cell::Cell, path::PathBuf, rc::Rc, time::Duration};

use mt_core::settings::Language;
use nsh_core::{
    Points,
    desktop::{QuitMode, TitleBarOptions, TitleBarStyle, WindowAttributes},
    element::desktop::{set_quit_mode, window},
    prelude::*,
};

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
    // The app ends when its last window closes on every platform; the
    // platform default would leave it in the macOS Dock.
    set_quit_mode(QuitMode::LastWindowClosed);
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
    let tick_state = state.clone();
    let ticker_alive = Rc::new(Cell::new(true));
    let ticker_flag = ticker_alive.clone();
    let _ticker = stored(set_interval(Duration::from_secs(1), move || {
        if ticker_flag.get() {
            tick_state.checkpoint_tick();
            // The watcher's debounced batches are drained here too: the drain
            // is not a filesystem poll (nothing is stat'ed until an event
            // names a path), and it adds no timer of its own — a repeating
            // timer of its own perturbs nsh's deferred editor focus.
            tick_state.disk_tick();
        }
    }));
    let _ticker_guard = stored(TickerGuard(ticker_alive));
    let status = ui::notices::StatusPresence::new(state.clone());
    let close_state = state.clone();
    let content_state = state.clone();
    window()
        .id("main")
        .title("MarkTurbo")
        .default_size(size(px(1280.0), px(800.0)))
        .attributes(main_window_attributes())
        .on_close_press(move |_| close_state.request_quit())
        .content(move || {
            let state = content_state.clone();
            state
                .main_window
                .borrow_mut()
                .replace(nsh_core::element::desktop::current_window());
            if ui::cjk_optics::capture_requested() {
                return ui::cjk_optics::capture(state.companion_font.clone()).into_element();
            }
            shell(state, status).into_element()
        })
}

/// The shell as the main window's content.
fn shell(state: AppState, status: ui::notices::StatusPresence) -> impl IntoElement {
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
                    ui::shell::shell_with_status(state.clone(), language, layout, status)
                        .into_element()
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

/// The main window's attributes: the app draws its own title row
/// (`ui::shell`), and a close press reaches `AppState::request_quit` instead
/// of closing the window by itself.
fn main_window_attributes() -> WindowAttributes {
    // The option structs are `#[non_exhaustive]`: built from their defaults,
    // then set field by field.
    let mut title_bar = TitleBarOptions::default();
    title_bar.style = TitleBarStyle::Hidden;
    title_bar.height = Some(Points(ui::tokens::TITLE_ROW_H));
    let mut attributes = WindowAttributes::default();
    attributes.close_on_press = Some(false);
    attributes.title_bar = Some(title_bar);
    attributes.min_size = Some(Some(size(
        px(ui::tokens::WINDOW_MIN_W),
        px(ui::tokens::WINDOW_MIN_H),
    )));
    attributes
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

#[cfg(test)]
mod tests {
    use super::main_window_attributes;
    use nsh_core::prelude::*;

    #[test]
    fn the_main_window_declares_the_minimum_size_token() {
        assert_eq!(
            main_window_attributes().min_size,
            Some(Some(size(
                px(crate::ui::tokens::WINDOW_MIN_W),
                px(crate::ui::tokens::WINDOW_MIN_H),
            ))),
            "the main window's minimum is the layout token, not an ad-hoc size"
        );
    }
}
