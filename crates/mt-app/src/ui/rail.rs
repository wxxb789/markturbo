//! Owner lane: chrome.
//!
//! The rail: a 44px strip at the far left with one 6px dot per open file
//! (plain ink; the dot turns ember only while the file holds live model
//! output), widening to 240px on hover to show names.
//!
//! Contract: [`rail`] is the strip's whole column — the shell places it beside
//! the text column and gives it no width of its own, so the width, the hover
//! widening (160ms `cubic-bezier(0.2, 0, 0, 1)`) and the names all belong
//! here.
//!
//! The rail also carries the app's file lists: `Ctrl+P` opens the workspace's
//! files and `Ctrl+Shift+F` opens a content search as one input line at the
//! top of the field. A single press opens in the preview slot — the tab the
//! next single press replaces — and a double press pins it.

use std::path::PathBuf;

use mt_core::settings::Language;
use nsh_base::input;
use nsh_core::{Color, prelude::*};

use crate::{
    i18n::{self, Key},
    state::{AppState, Tab},
    ui::{
        motion::{RAIL_WIDEN_MS, rail_easing, reduced_motion},
        prims, shell,
        tokens::{
            self, COLUMN_GAP, FONT_UI, RAIL_DOT, RAIL_HOVER_W, RAIL_W, SPACE_1, SPACE_2, SPACE_3,
            SPACE_5, TYPE_UI, TYPE_UI_LINE,
        },
    },
};

/// How many rows the palette lists at once.
const MAX_ROWS: usize = 200;

/// The rail: one dot per open file, the active file's at full ink.
pub fn rail(state: AppState, language: Language) -> impl IntoElement {
    let hovered = signal(false);
    let slide = animated_value(0.0);
    let palette = Palette {
        open: signal(false),
        mode: signal(PaletteMode::Files),
        filter: signal(String::new()),
        highlighted: signal(0),
        submitted: signal(String::new()),
    };

    // Ctrl+P and Ctrl+Shift+F: the same line, opened on the files and on the
    // search. They are not keys a text input uses, so the default `when`
    // fires them from the editor too.
    let key_state = state.clone();
    on_key("mod+p", move |_: &str| {
        palette.show(PaletteMode::Files, String::new())
    });
    on_key("shift+mod+f", move |_: &str| {
        palette.show(PaletteMode::Search, key_state.search_query.get())
    });

    view()
        .test_id("rail")
        .relative()
        // Above the text column and the field, so the widening and the
        // palette line are not painted over. The shell's overlay layer stays
        // above the whole room.
        .z_index(4)
        .w(px(RAIL_W))
        .h(relative(1.0))
        .bg(move || tokens::palette().room)
        .child(strip(state.clone(), hovered, slide, palette))
        .child(when(move || palette.open.get(), {
            let state = state.clone();
            move || palette_line(state.clone(), language, palette).into_element()
        }))
}

/// The 44px strip itself: the ground that widens, the dots, and the names
/// that appear with it.
fn strip(
    state: AppState,
    hovered: Signal<bool>,
    slide: AnimatedValue,
    palette: Palette,
) -> impl IntoElement {
    let offset = slide
        .node()
        .mul(RAIL_HOVER_W - RAIL_W)
        .sub(RAIL_HOVER_W - RAIL_W);
    view()
        .test_id("rail-hover")
        .absolute()
        .top(px(0.0))
        .bottom(px(0.0))
        .left(px(0.0))
        // While the strip is open its whole 240px listens, so the pointer on
        // a name keeps it open — as the widened strip would.
        .w(move || px(if hovered.get() { RAIL_HOVER_W } else { RAIL_W }))
        .on_mouse_enter(move |_: &MouseEvent| {
            hovered.set(true);
            if reduced_motion() {
                slide.set(1.0);
            } else {
                slide.timing(1.0, RAIL_WIDEN_MS, rail_easing());
            }
        })
        .on_mouse_leave(move |_: &MouseEvent| {
            hovered.set(false);
            if reduced_motion() {
                slide.set(0.0);
            } else {
                slide.timing(0.0, RAIL_WIDEN_MS, rail_easing());
            }
        })
        // The widened ground slides out from under the dots; the names it
        // uncovers fade in with it. Nothing moves: the dots stay where they
        // are, and the column does not move at all.
        .child(
            view()
                .test_id("rail-ground")
                .absolute()
                .top(px(0.0))
                .bottom(px(0.0))
                .left(px(0.0))
                .w(px(RAIL_HOVER_W))
                .bg(move || tokens::palette().room)
                .bind_translate_x(&offset),
        )
        .child(dots(state.clone(), palette))
        .child(names(state, slide, palette))
}

/// One 6px dot per open file, at the strip's left edge.
fn dots(state: AppState, palette: Palette) -> impl IntoElement {
    let list_state = state.clone();
    view()
        .test_id("rail-dots")
        .absolute()
        .top(px(0.0))
        .left(px(0.0))
        .w(px(RAIL_W))
        .pt(px(SPACE_5))
        .flex_col()
        .child(each(
            move || list_state.tabs.get(),
            |tab| tab.id,
            move |tab: ReadSignal<Tab>, _| {
                let tab = tab.get();
                let id = tab.id;
                let active_state = state.clone();
                let press_state = state.clone();
                let live_state = state.clone();
                let live_dot_state = state.clone();
                view()
                    .test_id("rail-file-row")
                    .h(px(24.0))
                    .w(px(RAIL_W))
                    .pl(px(15.0))
                    .flex_row()
                    .items_center()
                    .cursor_pointer()
                    .on_press(move |event: &PressEvent| {
                        press_state.set_active(id);
                        if event.clicks >= 2 {
                            press_state.pin_tab(id);
                        }
                        palette.close(&press_state);
                    })
                    .child(
                        view()
                            .test_id("rail-dot")
                            .w(px(14.0))
                            .h(px(14.0))
                            .rounded(px(7.0))
                            .flex_row()
                            .items_center()
                            .justify_center()
                            .bg(move || {
                                if holds_model_output(&live_state, id) {
                                    tokens::palette().ember_bloom
                                } else {
                                    Color::TRANSPARENT
                                }
                            })
                            .child(
                                view()
                                    .w(px(RAIL_DOT))
                                    .h(px(RAIL_DOT))
                                    .rounded(px(RAIL_DOT / 2.0))
                                    .bg(move || {
                                        if holds_model_output(&live_dot_state, id) {
                                            tokens::palette().ember
                                        } else if active_state.active.get() == Some(id) {
                                            tokens::palette().ink
                                        } else {
                                            // The prototype's `.dot`: the ink at
                                            // 45%, between resting and faint.
                                            tokens::palette().ink_at(0.45)
                                        }
                                    }),
                            ),
                    )
            },
        ))
}

/// The open files' names, at the widened strip's name column.
fn names(state: AppState, slide: AnimatedValue, palette: Palette) -> impl IntoElement {
    let list_state = state.clone();
    view()
        .test_id("rail-names")
        .absolute()
        .top(px(0.0))
        .bottom(px(0.0))
        .left(px(0.0))
        .w(px(RAIL_HOVER_W))
        .overflow_hidden()
        .pt(px(SPACE_5))
        .flex_col()
        .bind_opacity(&slide)
        .child(each(
            move || list_state.tabs.get(),
            |tab| tab.id,
            move |tab: ReadSignal<Tab>, _| {
                let tab = tab.get();
                let id = tab.id;
                let title = tab.title.clone();
                let active_state = state.clone();
                let press_state = state.clone();
                view()
                    .test_id("rail-name-row")
                    .h(px(24.0))
                    .w_full()
                    .pl(px(44.0))
                    .pr(px(SPACE_2))
                    .flex_row()
                    .items_center()
                    .cursor_pointer()
                    .hover(|s| s.text_color(tokens::palette().ink))
                    .on_press(move |event: &PressEvent| {
                        press_state.set_active(id);
                        if event.clicks >= 2 {
                            press_state.pin_tab(id);
                        }
                        palette.close(&press_state);
                    })
                    .child(
                        text(title)
                            .font_family(FONT_UI)
                            .text_size(px(TYPE_UI))
                            .line_height(px(TYPE_UI_LINE))
                            .text_color(move || {
                                if active_state.active.get() == Some(id) {
                                    tokens::palette().ink
                                } else {
                                    tokens::palette().ink_rest
                                }
                            })
                            .truncate(),
                    )
            },
        ))
}

/// Whether `id` holds the model's live output: the tab the review or the
/// revision on screen belongs to.
fn holds_model_output(state: &AppState, id: u64) -> bool {
    state.active.get() == Some(id)
        && (state.review_open.get() || !state.revision_hunks.get().is_empty())
}

/// The palette's own state: whether it is up, which list it shows, the text
/// being typed, the highlighted row and the query last sent to the search.
#[derive(Clone, Copy)]
struct Palette {
    open: Signal<bool>,
    mode: Signal<PaletteMode>,
    filter: Signal<String>,
    highlighted: Signal<usize>,
    submitted: Signal<String>,
}

/// Which list the palette shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PaletteMode {
    Files,
    Search,
}

impl Palette {
    /// Opens it on `mode`, with `text` in the input.
    fn show(&self, mode: PaletteMode, text: String) {
        self.mode.set(mode);
        self.filter.set(text);
        self.highlighted.set(0);
        self.submitted.set(String::new());
        self.open.set(true);
    }

    /// Closes it and drops what the search found: the results are the line's,
    /// and nothing else shows them.
    fn close(&self, state: &AppState) {
        self.open.set(false);
        self.filter.set(String::new());
        self.highlighted.set(0);
        self.submitted.set(String::new());
        state.clear_search();
    }

    /// What the input holds: the filter while listing files, the query while
    /// searching.
    fn text(&self, state: &AppState) -> String {
        match self.mode.get() {
            PaletteMode::Files => self.filter.get(),
            PaletteMode::Search => state.search_query.get(),
        }
    }

    fn set_text(&self, state: &AppState, text: &str) {
        self.highlighted.set(0);
        match self.mode.get() {
            PaletteMode::Files => self.filter.set(text.to_owned()),
            PaletteMode::Search => state.search_query.set(text.to_owned()),
        }
    }

    /// The rows it lists now.
    fn rows(&self, state: &AppState) -> Vec<Row> {
        let text = self.text(state);
        match self.mode.get() {
            PaletteMode::Files => file_rows(state, &text),
            PaletteMode::Search => search_rows(state),
        }
    }
}

/// One row of the palette: what it opens, what it shows, and its key.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Row {
    path: PathBuf,
    label: SharedString,
    detail: SharedString,
    key: String,
}

/// The workspace's files, filtered by `text`: the name or the path matching is
/// enough.
fn file_rows(state: &AppState, text: &str) -> Vec<Row> {
    let needle = text.trim().to_lowercase();
    let root = state.root.get();
    state
        .entries
        .get()
        .into_iter()
        .filter(|entry| !entry.is_dir)
        .filter(|entry| {
            if needle.is_empty() {
                return true;
            }
            entry.name.to_lowercase().contains(&needle)
                || entry
                    .path
                    .to_string_lossy()
                    .to_lowercase()
                    .contains(&needle)
        })
        .take(MAX_ROWS)
        .map(|entry| {
            let detail = root
                .as_ref()
                .and_then(|root| entry.path.strip_prefix(root).ok())
                .and_then(|relative| relative.parent())
                .map(|folder| folder.display().to_string())
                .unwrap_or_default();
            Row {
                label: SharedString::from(entry.name),
                detail: SharedString::from(detail),
                key: entry.path.display().to_string(),
                path: entry.path,
            }
        })
        .collect()
}

/// What the last search found, in the order it found it.
fn search_rows(state: &AppState) -> Vec<Row> {
    state
        .search_results
        .get()
        .into_iter()
        .map(|hit| Row {
            key: hit.label.clone(),
            label: SharedString::from(hit.label),
            detail: SharedString::from(""),
            path: hit.path,
        })
        .collect()
}

/// The line the palette draws: an input at the top of the field, the rows it
/// lists under it, and what the search is doing.
fn palette_line(state: AppState, language: Language, palette: Palette) -> impl IntoElement {
    let scrim_state = state.clone();
    let count_state = state.clone();
    view()
        .test_id("palette")
        .child(
            view()
                .test_id("palette-scrim")
                .absolute()
                .top(px(0.0))
                .bottom(px(0.0))
                .left(px(RAIL_W))
                .w(move || px((shell::client_width_now() - RAIL_W).max(0.0)))
                .on_press(move || palette.close(&scrim_state)),
        )
        .child(
            view()
                .test_id("palette-line")
                .absolute()
                .top(px(SPACE_3))
                .left(px(RAIL_W + COLUMN_GAP))
                .w(move || {
                    px((shell::client_width_now()
                        - RAIL_W
                        - COLUMN_GAP
                        - tokens::FIELD_RIGHT_INSET)
                        .max(tokens::FIELD_READING_MAX_W))
                })
                .flex_col()
                .bg(move || tokens::palette().room)
                .child(palette_input(state.clone(), language, palette))
                .child(palette_rows(state.clone(), language, palette))
                .child({
                    let condition_state = count_state.clone();
                    when(
                        move || notice(&condition_state, &palette, language).is_some(),
                        move || {
                            let text = notice(&count_state, &palette, language).unwrap_or_default();
                            view()
                                .test_id("palette-notice")
                                .px(px(SPACE_2))
                                .pb(px(SPACE_2))
                                .child(prims::faint(text))
                                .into_element()
                        },
                    )
                })
                .child(when(
                    {
                        let truncated_state = state.clone();
                        move || truncated_state.search_truncated.get()
                    },
                    move || {
                        view()
                            .test_id("palette-truncated")
                            .px(px(SPACE_2))
                            .pb(px(SPACE_2))
                            .child(prims::faint(i18n::t(language, Key::ShowingFirstResults)))
                            .into_element()
                    },
                )),
        )
}

/// The palette's input: its one line, with the hairline under it.
fn palette_input(state: AppState, language: Language, palette: Palette) -> impl IntoElement {
    let value_state = state.clone();
    let change_state = state.clone();
    let submit_state = state.clone();
    let key_state = state.clone();
    view()
        .flex_col()
        .child(
            input()
                .test_id("palette-input")
                .label(SharedString::from(i18n::t(language, Key::SearchFiles)))
                .value(move || palette.text(&value_state))
                .placeholder(move || match palette.mode.get() {
                    PaletteMode::Files => s(S::FindFile, language).to_owned(),
                    PaletteMode::Search => i18n::t(language, Key::SearchFiles).to_owned(),
                })
                .font_family(FONT_UI)
                .text_size(px(TYPE_UI))
                .bg(Color::TRANSPARENT)
                .w_full()
                .auto_focus(true)
                .on_change(move |text: &str| palette.set_text(&change_state, text))
                .on_submit(move |_: &str| submit(&submit_state, palette))
                .on_key_press(move |key: &str| {
                    if key == "Escape" {
                        palette.close(&key_state);
                    }
                }),
        )
        .child(prims::hairline())
}

/// The rows, under the input.
fn palette_rows(state: AppState, language: Language, palette: Palette) -> impl IntoElement {
    let _ = language;
    let list_state = state.clone();
    scroll().max_h(px(360.0)).child(
        view()
            .test_id("palette-rows")
            .flex_col()
            .py(px(SPACE_1))
            .child(each(
                move || palette.rows(&list_state),
                |row: &Row| row.key.clone(),
                move |row: ReadSignal<Row>, index| {
                    let row = row.get();
                    let open_state = state.clone();
                    let path = row.path.clone();
                    let highlight = palette.highlighted;
                    let index = index.get();
                    view()
                        .test_id("palette-row")
                        .w_full()
                        .flex_row()
                        .items_center()
                        .gap(px(SPACE_3))
                        .px(px(SPACE_2))
                        .py(px(SPACE_1))
                        .cursor_pointer()
                        .hover(|s| s.text_color(tokens::palette().ink))
                        .on_press(move |event: &PressEvent| {
                            if event.clicks >= 2 {
                                if let Some(tab) = open_state.active_tab() {
                                    open_state.pin_tab(tab.id);
                                }
                                return;
                            }
                            palette.close(&open_state);
                            open_state.open_preview(path.clone());
                        })
                        .child(
                            text(row.label.clone())
                                .font_family(FONT_UI)
                                .text_size(px(TYPE_UI))
                                .line_height(px(TYPE_UI_LINE))
                                .text_color(move || {
                                    if highlight.get() == index {
                                        tokens::palette().ink
                                    } else {
                                        tokens::palette().ink_rest
                                    }
                                })
                                .truncate(),
                        )
                        .child(
                            text(row.detail.clone())
                                .font_family(FONT_UI)
                                .text_size(px(TYPE_UI))
                                .line_height(px(TYPE_UI_LINE))
                                .text_color(move || tokens::palette().ink_faint)
                                .truncate(),
                        )
                },
            )),
    )
}

/// What the line says under its rows when it is not simply listing them: the
/// search at work, and a query that found nothing.
fn notice(state: &AppState, palette: &Palette, language: Language) -> Option<SharedString> {
    match palette.mode.get() {
        PaletteMode::Files => {
            let typed = palette.text(state);
            let empty = palette.rows(state).is_empty();
            (empty && !typed.trim().is_empty())
                .then(|| SharedString::from(i18n::t(language, Key::NoMatches)))
        }
        PaletteMode::Search => {
            if state.search_busy.get() {
                Some(SharedString::from(i18n::t(language, Key::Searching)))
            } else if state.search_ran.get() && state.search_results.get().is_empty() {
                Some(SharedString::from(i18n::t(language, Key::NoMatches)))
            } else {
                None
            }
        }
    }
}

/// Enter in the palette: open the highlighted row; in the search, the first
/// Enter sends the query and the next opens what it found.
fn submit(state: &AppState, palette: Palette) {
    let text = palette.text(state);
    if palette.mode.get() == PaletteMode::Search && palette.submitted.get() != text {
        palette.submitted.set(text.clone());
        state.run_search(&text);
        return;
    }
    let rows = palette.rows(state);
    let Some(row) = rows.get(palette.highlighted.get()) else {
        return;
    };
    let path = row.path.clone();
    palette.close(state);
    state.open_preview(path);
}

/// The rail's copy. New strings live here rather than in `i18n.rs` (a merge
/// hotspot): the match is exhaustive, so a missing translation is a compile
/// error.
#[derive(Clone, Copy)]
enum S {
    FindFile,
}

fn s(key: S, language: Language) -> &'static str {
    match language {
        Language::English => match key {
            S::FindFile => "Find a file",
        },
        Language::Chinese => match key {
            S::FindFile => "查找文件",
        },
    }
}
