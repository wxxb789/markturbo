//! Owner lane: chrome.
//!
//! The Shoji sidebar pushes the stage above the narrow breakpoint and
//! overlays it below. Its folded edge strip gives every open tab one tick;
//! its unfolded panel shows the workspace tree and tabs outside the folder.
//!
//! Ctrl+P and Ctrl+Shift+F still open the file and content palettes.

use std::{
    cell::Cell,
    collections::{HashMap, HashSet},
    path::PathBuf,
};

use mt_core::settings::Language;
use nsh_base::input;
use nsh_core::{AccessibilityState, Color, element::KeyOptions, keymap::When, prelude::*};

use crate::{
    i18n::{self, Key},
    state::{AppState, Entry, Tab, TrackCurve, TrackTween},
    ui::{
        consent, motion, prims, shell,
        tokens::{
            self, FONT_INPUT, FONT_UI, HAIRLINE, Layout, RAIL_DOT, RAIL_W, SIDEBAR_CAPTION,
            SIDEBAR_MARK_W, SIDEBAR_ROW_H, SIDEBAR_TICK_H, SIDEBAR_TICK_INSET,
            SIDEBAR_TICK_LABEL_SHIFT, SIDEBAR_TICK_LABEL_X, SIDEBAR_TICK_W, SIDEBAR_W, SPACE_1,
            SPACE_2, SPACE_3, SPACE_4, TYPE_UI, TYPE_UI_LINE,
        },
    },
};

/// How many rows the palette lists at once.
const MAX_ROWS: usize = 200;

pub(super) fn rail(state: AppState, language: Language, layout: Layout) -> impl IntoElement {
    let narrow = layout.field_overlays();
    if !narrow {
        state.sidebar_overlay.set(false);
    }
    let open = if narrow {
        state.sidebar_overlay
    } else {
        state.sidebar_open
    };
    let initial = if open.get_untracked() { 1.0 } else { 0.0 };
    let travel = signal(initial);
    let fade = signal(initial);
    let travel_tween = TrackTween::default();
    let fade_tween = TrackTween::default();
    let owner = Owner::current();
    let before = Cell::new((open.get_untracked(), untracked(motion::reduced_motion)));
    effect(move || {
        let target = open.get();
        let reduced = motion::reduced_motion();
        let (was_open, was_reduced) = before.replace((target, reduced));
        let to = if target { 1.0 } else { 0.0 };
        if reduced {
            if !was_reduced {
                fade_tween.set_now(fade, travel.get_untracked());
            }
            travel_tween.set_now(travel, to);
            if was_open != target || !was_reduced {
                fade_tween.to(owner, fade, to, TrackCurve::Fade);
            }
        } else if was_reduced || was_open != target {
            if was_reduced {
                travel_tween.set_now(travel, fade.get_untracked());
            }
            travel_tween.to(owner, travel, to, TrackCurve::Rail);
        }
    });
    let width = computed(move || {
        if narrow {
            RAIL_W
        } else {
            RAIL_W + (SIDEBAR_W - RAIL_W) * travel.get()
        }
    });
    let drawn_width = shell::sidebar_width();
    effect(move || drawn_width.set(shell::SidebarWidth(width.get())));
    let visibility = computed(move || {
        if motion::reduced_motion() {
            fade.get()
        } else {
            travel.get()
        }
    });
    let palette = Palette {
        open: state.palette_open,
        mode: signal(PaletteMode::Files),
        filter: signal(String::new()),
        highlighted: signal(None),
        submitted: signal(String::new()),
    };
    bind_keys(&state, palette, open, narrow);
    let strip_state = state.clone();
    let panel_state = state.clone();
    let collapsed = signal(HashSet::<PathBuf>::new());
    let clip = view()
        .absolute()
        .top(px(0.0))
        .bottom(px(0.0))
        .left(px(if narrow { RAIL_W } else { 0.0 }))
        .w(move || {
            px(if motion::reduced_motion() {
                SIDEBAR_W
            } else if narrow {
                SIDEBAR_W * travel.get()
            } else {
                width.get()
            })
        })
        .test_id("sidebar-panel-clip")
        .overflow_hidden()
        .pointer_events(PointerEvents::BoxNone)
        .child(when(
            move || open.get() || visibility.get() > 0.0,
            move || {
                panel(
                    panel_state.clone(),
                    language,
                    palette,
                    open,
                    travel,
                    fade,
                    narrow,
                    collapsed,
                )
            },
        ));
    view()
        .test_id("sidebar")
        .relative()
        .z_index(4)
        .flex_shrink_0()
        .w(move || px(width.get()))
        .h(relative(1.0))
        .bg(move || tokens::palette().room)
        .child(clip)
        .child(when(
            move || !open.get() || visibility.get() < 1.0,
            move || strip(strip_state.clone(), palette, open, visibility),
        ))
        .child(when(move || palette.open.get(), {
            move || palette_line(state.clone(), language, palette).into_element()
        }))
}

fn bind_keys(state: &AppState, palette: Palette, open: Signal<bool>, narrow: bool) {
    // Every Escape binding runs the same fixed-order decision, so the
    // palette cannot be closed out of turn while a guard or menu is up.
    effect({
        let escape_state = state.clone();
        move || {
            if palette.open.get() || (narrow && open.get()) {
                let press_state = escape_state.clone();
                on_key_with(
                    "escape",
                    KeyOptions {
                        when: When::Always,
                        ..KeyOptions::default()
                    },
                    move |_| {
                        press_state.escape_top_layer();
                    },
                );
            }
        }
    });

    // Ctrl+P and Ctrl+Shift+F: the same line, opened on the files and on the
    // search. They are not keys a text input uses, so the default `when`
    // fires them from the editor too. While a consent card is open it is
    // modal: the shortcuts do nothing (the keymap still consumes the key).
    let palette_state = state.clone();
    on_key("mod+p", move |_: &str| {
        if !consent::is_open(&palette_state) {
            palette.show(PaletteMode::Files, String::new());
        }
    });
    let filter_state = state.clone();
    on_key("shift+mod+f", move |_: &str| {
        if !consent::is_open(&filter_state) {
            palette.show(PaletteMode::Search, filter_state.search_query.get());
        }
    });
    // The tab's own close, and the active tab's from the keyboard.
    let close_state = state.clone();
    on_key("mod+w", move |_: &str| {
        if consent::is_open(&close_state) {
            return;
        }
        if let Some(tab) = close_state.active_tab() {
            close_state.request_close_tab(tab.id);
        }
    });
    // The palette's rows, stepped from the keyboard while its input has the
    // focus (an input keeps the arrows for its caret, hence `Always` + the
    // key passed on: the caret still moves, and nothing is taken from a
    // binding around it).
    let down_state = state.clone();
    on_key_with(
        "down",
        KeyOptions {
            when: When::Always,
            pass_through: true,
            ..KeyOptions::default()
        },
        move |_: &str| {
            if palette.open.get() {
                palette.step(&down_state, 1);
            }
        },
    );
    let up_state = state.clone();
    on_key_with(
        "up",
        KeyOptions {
            when: When::Always,
            pass_through: true,
            ..KeyOptions::default()
        },
        move |_: &str| {
            if palette.open.get() {
                palette.step(&up_state, -1);
            }
        },
    );

    let toggle_state = state.clone();
    on_key("mod+b", move |_: &str| {
        if !consent::is_open(&toggle_state) {
            toggle_state.toggle_sidebar(Layout::of(shell::client_width_now()));
            if narrow && !open.get_untracked() {
                toggle_state.focus_editor();
            }
        }
    });
}

fn strip(
    state: AppState,
    palette: Palette,
    open: Signal<bool>,
    visibility: Memo<f32>,
) -> impl IntoElement {
    let tabs = state.tabs;
    let active = state.active;
    view()
        .test_id("sidebar-strip")
        .role(AccessibilityRole::Group)
        .absolute()
        .top(px(0.0))
        .left(px(0.0))
        .w(px(RAIL_W))
        .pt(px(SPACE_4))
        .flex_col()
        .opacity(move || 1.0 - visibility.get())
        .pointer_events(move || {
            if open.get() {
                PointerEvents::None
            } else {
                PointerEvents::BoxNone
            }
        })
        .child(each(
            move || tabs.get(),
            |tab| tab.id,
            move |row: ReadSignal<Tab>, _| {
                let id = row.get_untracked().id;
                let press_state = state.clone();
                let hover = signal(false);
                let opacity = animated_value(0.0);
                let shift = animated_value(-SIDEBAR_TICK_LABEL_SHIFT);
                effect(move || {
                    let hovered = hover.get();
                    let reduced = motion::reduced_motion();
                    opacity.timing(
                        if hovered { 1.0 } else { 0.0 },
                        if reduced {
                            motion::REDUCED_CROSSFADE_MS
                        } else {
                            motion::HOVER_MS
                        },
                        motion::EASE_OUT,
                    );
                    if reduced {
                        shift.set(0.0);
                    } else {
                        shift.timing(
                            if hovered {
                                0.0
                            } else {
                                -SIDEBAR_TICK_LABEL_SHIFT
                            },
                            motion::RAIL_WIDEN_MS,
                            motion::EASE_OUT,
                        );
                    }
                });
                let strong = move || active.get() == Some(id) || hover.get();
                press_row(
                    "rail-file-row",
                    move || row.get().title,
                    move || !open.get(),
                )
                .w(px(RAIL_W))
                .h(px(SPACE_4))
                .pl(px(SIDEBAR_TICK_INSET))
                .relative()
                .on_mouse_enter(move |_: &MouseEvent| hover.set(true))
                .on_mouse_leave(move |_: &MouseEvent| hover.set(false))
                .on_press(move || activate(&press_state, palette, id, false))
                .child(
                    view()
                        .w(px(SIDEBAR_TICK_W))
                        .h(move || {
                            px(if strong() {
                                SIDEBAR_MARK_W
                            } else {
                                SIDEBAR_TICK_H
                            })
                        })
                        .bg(move || {
                            if strong() {
                                tokens::palette().ink
                            } else {
                                tokens::palette().ink_at(0.45)
                            }
                        }),
                )
                .child(
                    view()
                        .test_id("sidebar-tick-label")
                        .absolute()
                        .left(px(SIDEBAR_TICK_LABEL_X))
                        .top(px(SPACE_1))
                        .pointer_events(PointerEvents::None)
                        .bg(move || tokens::palette().room)
                        .bind_opacity(&opacity)
                        .bind_translate_x(&shift)
                        .child(
                            view()
                                .px(px(SPACE_2))
                                .bg(move || tokens::palette().afterimage_plate)
                                .child(prims::ink(move || row.get().title).number_of_lines(1)),
                        ),
                )
            },
        ))
}

fn panel(
    state: AppState,
    language: Language,
    palette: Palette,
    open: Signal<bool>,
    travel: Signal<f32>,
    fade: Signal<f32>,
    narrow: bool,
    collapsed: Signal<HashSet<PathBuf>>,
) -> impl IntoElement {
    let groups = tab_groups(state.clone());
    view()
        .test_id("sidebar-panel")
        .role(AccessibilityRole::Group)
        .w(px(SIDEBAR_W))
        .absolute()
        .top(px(0.0))
        .bottom(px(0.0))
        .left(px(0.0))
        .translate_x(move || {
            px(if motion::reduced_motion() {
                0.0
            } else {
                -(SIDEBAR_W - RAIL_W) * (1.0 - travel.get())
            })
        })
        .opacity(move || {
            if motion::reduced_motion() {
                fade.get()
            } else {
                1.0
            }
        })
        .pointer_events(move || {
            if open.get() {
                PointerEvents::Auto
            } else {
                PointerEvents::None
            }
        })
        .bg(move || tokens::palette().room)
        .child(
            scroll()
                .w_full()
                .h(relative(1.0))
                .flex_col()
                .bg(move || tokens::palette().afterimage_plate)
                .child(
                    view()
                        .w_full()
                        .flex_col()
                        .pt(px(SPACE_4))
                        .px(px(SPACE_3))
                        .pb(px(SPACE_3))
                        .gap(px(SPACE_3))
                        .child(tab_group(
                            state.clone(),
                            language,
                            palette,
                            open,
                            narrow,
                            groups,
                            TabGroup::Outside,
                        ))
                        .child(tab_group(
                            state.clone(),
                            language,
                            palette,
                            open,
                            narrow,
                            groups,
                            TabGroup::Unlisted,
                        ))
                        .child(tree(state, language, palette, open, narrow, collapsed)),
                ),
        )
}

#[derive(Clone, Copy)]
enum TabGroup {
    Outside,
    Unlisted,
}

impl TabGroup {
    fn rows(self, groups: &(Vec<Tab>, Vec<Tab>)) -> &Vec<Tab> {
        match self {
            Self::Outside => &groups.0,
            Self::Unlisted => &groups.1,
        }
    }
}

fn tab_groups(state: AppState) -> Memo<(Vec<Tab>, Vec<Tab>)> {
    computed(move || {
        state.entries.with(|entries| {
            let files: HashSet<_> = entries
                .iter()
                .filter(|entry| !entry.is_dir)
                .map(|entry| &entry.path)
                .collect();
            state.tabs.with(|tabs| {
                let mut outside = Vec::new();
                let mut unlisted = Vec::new();
                for tab in tabs {
                    if state.outside_folder(tab) {
                        outside.push(tab.clone());
                    } else if !files.contains(&tab.path) {
                        unlisted.push(tab.clone());
                    }
                }
                (outside, unlisted)
            })
        })
    })
}

fn tab_group(
    state: AppState,
    language: Language,
    palette: Palette,
    open: Signal<bool>,
    narrow: bool,
    groups: Memo<(Vec<Tab>, Vec<Tab>)>,
    group: TabGroup,
) -> impl IntoElement {
    let active = state.active;
    let (id, heading) = match group {
        TabGroup::Outside => ("sidebar-outside", S::OutsideFolder),
        TabGroup::Unlisted => ("sidebar-unlisted", S::UnlistedFiles),
    };
    when(
        move || groups.with(|groups| !group.rows(groups).is_empty()),
        move || {
            let state = state.clone();
            view()
                .test_id(id)
                .w_full()
                .flex_col()
                .child(
                    prims::faint(s(heading, language))
                        .px(px(SPACE_2))
                        .pb(px(SPACE_1)),
                )
                .child(each(
                    move || groups.with(|groups| group.rows(groups).clone()),
                    |tab| tab.id,
                    move |row: ReadSignal<Tab>, _| {
                        let tab = row.get_untracked();
                        let id = tab.id;
                        let title = move || row.get().title;
                        let folder = (!tab.untitled)
                            .then(|| tab.path.parent())
                            .flatten()
                            .filter(|path| !path.as_os_str().is_empty())
                            .map(|path| SharedString::from(path.to_string_lossy().into_owned()));
                        let press_state = state.clone();
                        file_line(
                            "sidebar-outside-row",
                            0,
                            move || Some(id),
                            active,
                            title,
                            folder,
                            state.clone(),
                            language,
                            open,
                            move || activate(&press_state, palette, id, narrow),
                        )
                    },
                ))
        },
    )
}

fn tree(
    state: AppState,
    language: Language,
    palette: Palette,
    open: Signal<bool>,
    narrow: bool,
    collapsed: Signal<HashSet<PathBuf>>,
) -> impl IntoElement {
    let entries = state.entries;
    let active = state.active;
    let tabs = state.tabs;
    let open_files = computed(move || {
        tabs.with(|tabs| {
            let mut open = HashMap::new();
            for tab in tabs.iter().filter(|tab| !tab.untitled) {
                // The first tab of a path is the one activation targets, so
                // the row's dot, selected state and close share its id.
                open.entry(tab.path.clone()).or_insert(tab.id);
            }
            open
        })
    });
    view()
        .test_id("sidebar-tree")
        .w_full()
        .flex_col()
        .gap(px(HAIRLINE))
        .child(each(
            move || {
                collapsed.with(|collapsed| {
                    entries.with(|entries| {
                        if collapsed.is_empty() {
                            return entries.clone();
                        }
                        open_files.with(|open_files| {
                            let mut hidden_below = None;
                            entries
                                .iter()
                                .filter(|entry| {
                                    if hidden_below.is_some_and(|depth| entry.depth > depth) {
                                        // Even a folded folder keeps its open tabs one click away.
                                        return !entry.is_dir
                                            && open_files.contains_key(&entry.path);
                                    }
                                    hidden_below = (entry.is_dir
                                        && collapsed.contains(&entry.path))
                                    .then_some(entry.depth);
                                    true
                                })
                                .cloned()
                                .collect()
                        })
                    })
                })
            },
            |entry| entry.path.clone(),
            move |row: ReadSignal<Entry>, _| {
                let entry = row.get_untracked();
                let title = SharedString::from(entry.name);
                if entry.is_dir {
                    let path = entry.path.clone();
                    let expanded =
                        computed(move || collapsed.with(|folders| !folders.contains(&path)));
                    press_row("sidebar-folder", title.clone(), move || open.get())
                        .w_full()
                        .h(px(SIDEBAR_ROW_H))
                        .pl(px(indentation(entry.depth)))
                        .gap(px(SPACE_2))
                        .accessibility_state(move || AccessibilityState {
                            expanded: Some(expanded.get()),
                            ..Default::default()
                        })
                        .on_press(move || {
                            collapsed.update(|folders| {
                                if !folders.remove(&entry.path) {
                                    folders.insert(entry.path.clone());
                                }
                            })
                        })
                        .child(
                            mark_slot().child(prims::faint(move || {
                                if expanded.get() { "▾" } else { "▸" }
                            })),
                        )
                        .child(row_name(title, || false))
                        .into_element()
                } else {
                    let path = entry.path.clone();
                    let tab = computed(move || open_files.with(|open| open.get(&path).copied()));
                    let press_state = state.clone();
                    let press_tab = tab;
                    file_line(
                        "sidebar-file",
                        entry.depth,
                        move || tab.get(),
                        active,
                        move || title.clone(),
                        None,
                        state.clone(),
                        language,
                        open,
                        move || {
                            if let Some(id) = press_tab.get() {
                                activate(&press_state, palette, id, narrow);
                                return;
                            }
                            if narrow {
                                press_state.sidebar_overlay.set(false);
                            }
                            if palette.open.get() {
                                palette.close(&press_state);
                            }
                            press_state.open_file(entry.path.clone());
                        },
                    )
                    .into_element()
                }
            },
        ))
}

fn indentation(depth: usize) -> f32 {
    SPACE_2 + depth as f32 * SIDEBAR_MARK_W
}

fn activate(state: &AppState, palette: Palette, id: u64, narrow_panel: bool) {
    state.set_active(id);
    if narrow_panel {
        state.sidebar_overlay.set(false);
    }
    if palette.open.get() {
        palette.close(state);
    } else {
        state.focus_editor();
    }
}

fn press_row(
    id: &str,
    label: impl IntoReactive<SharedString>,
    enabled: impl Fn() -> bool + Clone + 'static,
) -> View {
    prims::press_feedback(view())
        .test_id(id)
        .role(AccessibilityRole::Button)
        .label(label)
        .flex_row()
        .items_center()
        .group()
        .cursor_pointer()
        .focusable(enabled.clone())
        .disabled(move || !enabled())
        .border(px(HAIRLINE))
        .border_color(Color::TRANSPARENT)
        .focus_visible(|s| s.border_color(|| tokens::palette().ink))
}

fn mark_slot() -> View {
    view()
        .w(px(SIDEBAR_MARK_W))
        .h(px(SIDEBAR_MARK_W))
        .flex_shrink_0()
        .flex_row()
        .items_center()
        .justify_center()
}

fn row_name(
    title: impl IntoReactive<SharedString>,
    selected: impl Fn() -> bool + 'static,
) -> impl IntoElement {
    text(title)
        .font_family(FONT_UI)
        .text_size(px(TYPE_UI))
        .line_height(px(TYPE_UI_LINE))
        .text_color(move || {
            if selected() {
                tokens::palette().ink
            } else {
                tokens::palette().ink_rest
            }
        })
        .group_hover(|s| s.text_color(|| tokens::palette().ink))
        .truncate()
}

#[allow(clippy::too_many_arguments)]
fn file_line(
    id: &str,
    depth: usize,
    tab: impl Fn() -> Option<u64> + Copy + 'static,
    active: Signal<Option<u64>>,
    title: impl Fn() -> SharedString + Clone + 'static,
    folder: Option<SharedString>,
    state: AppState,
    language: Language,
    open: Signal<bool>,
    on_press: impl Fn() + 'static,
) -> View {
    let selected = move || tab().is_some_and(|id| active.get() == Some(id));
    let min_height = if folder.is_some() {
        SIDEBAR_ROW_H + TYPE_UI_LINE
    } else {
        SIDEBAR_ROW_H
    };
    let close_title = title.clone();
    view()
        .w_full()
        .min_h(px(min_height))
        .flex_row()
        .items_center()
        .pr(px(SPACE_2))
        .group()
        .child(
            press_row(id, title.clone(), move || open.get())
                .accessibility_state(move || AccessibilityState {
                    selected: selected(),
                    ..Default::default()
                })
                .min_h(px(min_height))
                .flex_1()
                .min_w(px(0.0))
                .pl(px(indentation(depth)))
                .gap(px(SPACE_2))
                .on_press(on_press)
                .child(mark_slot().child(when(
                    move || tab().is_some(),
                    move || {
                        view()
                            .w(px(RAIL_DOT))
                            .h(px(RAIL_DOT))
                            .rounded(px(RAIL_DOT / 2.0))
                            .bg(move || {
                                if selected() {
                                    tokens::palette().ink
                                } else {
                                    tokens::palette().ink_at(0.45)
                                }
                            })
                    },
                )))
                .child(
                    view()
                        .min_w(px(0.0))
                        .flex_1()
                        .flex_col()
                        .child(row_name(title, selected))
                        .children(folder.map(|folder| {
                            text(folder)
                                .font_family(FONT_UI)
                                .text_size(px(SIDEBAR_CAPTION))
                                .text_color(move || tokens::palette().ink_caption)
                                .truncate()
                                .into_element()
                        })),
                ),
        )
        .child(when(
            move || tab().is_some(),
            move || {
                let state = state.clone();
                let title = close_title.clone();
                press_row(
                    "rail-close",
                    move || SharedString::from(format!("{} {}", s(S::Close, language), title())),
                    move || open.get(),
                )
                .w(px(SPACE_3))
                .h(px(SPACE_3))
                .flex_shrink_0()
                .justify_center()
                .opacity(0.0)
                .group_hover(|s| s.opacity(1.0))
                .focus_visible(|s| s.opacity(1.0).border_color(|| tokens::palette().ink))
                .transition_opacity()
                .duration(motion::HOVER_MS)
                .on_press(move || {
                    if let Some(id) = tab() {
                        state.request_close_tab(id);
                        state.focus_editor();
                    }
                })
                .child(prims::faint("×").group_hover(|s| s.text_color(|| tokens::palette().ink)))
            },
        ))
}

/// The palette's own state: whether it is up, which list it shows, the text
/// being typed, the highlighted row (by its key, which filtering cannot move)
/// and the query last sent to the search.
#[derive(Clone, Copy)]
struct Palette {
    open: Signal<bool>,
    mode: Signal<PaletteMode>,
    filter: Signal<String>,
    highlighted: Signal<Option<String>>,
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
        self.highlighted.set(None);
        self.submitted.set(String::new());
        self.open.set(true);
    }

    fn close(&self, state: &AppState) {
        state.close_palette();
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
        self.highlighted.set(None);
        match self.mode.get() {
            PaletteMode::Files => self.filter.set(text.to_owned()),
            PaletteMode::Search => state.search_query.set(text.to_owned()),
        }
    }

    /// The row the keyboard is on: the highlighted one while the list still
    /// holds it, else the first.
    fn current_row(&self, state: &AppState) -> Option<Row> {
        let rows = self.rows(state);
        let highlighted = self.highlighted.get();
        rows.iter()
            .find(|row| Some(row.key.as_str()) == highlighted.as_deref())
            .or_else(|| rows.first())
            .cloned()
    }

    /// Moves the highlight by `step` rows through the list it shows now.
    fn step(&self, state: &AppState, step: isize) {
        let rows = self.rows(state);
        if rows.is_empty() {
            return;
        }
        let at = self
            .highlighted
            .get()
            .and_then(|key| rows.iter().position(|row| row.key == key))
            .map_or(0, |at| at as isize);
        let last = rows.len() as isize - 1;
        let next = (at + step).clamp(0, last) as usize;
        self.highlighted.set(Some(rows[next].key.clone()));
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
    // The line belongs to the field: its left edge follows the field's own
    // (the overlay's, below 1040), and it is no wider than the field's
    // reading measure.
    let left = move || {
        let client = shell::client_width_now();
        Layout::of(client).field_left_at(client, shell::sidebar_width_now())
    };
    view()
        .test_id("palette")
        .child(
            view()
                .test_id("palette-scrim")
                // A backdrop closes a popover for the pointer; it is not a
                // stop of the keyboard focus.
                .focusable(false)
                .absolute()
                .top(px(0.0))
                .bottom(px(0.0))
                .left(move || px(shell::sidebar_width_now()))
                .w(move || px(shell::stage_width()))
                .on_press(move || palette.close(&scrim_state)),
        )
        .child(
            view()
                .test_id("palette-line")
                .absolute()
                .top(px(SPACE_3))
                .left(move || px(left()))
                .w(move || {
                    px(
                        (shell::client_width_now() - left() - tokens::FIELD_RIGHT_INSET)
                            .clamp(0.0, tokens::FIELD_READING_MAX_W),
                    )
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
    // The input is the palette's focus: a close of one above hands the
    // keyboard back to it.
    let reference = element_ref();
    state.attach_palette_view(reference);
    view()
        .flex_col()
        .child(
            input()
                .test_id("palette-input")
                .element_ref(&reference)
                .label(SharedString::from(s(S::FindFile, language)))
                .value(move || palette.text(&value_state))
                .placeholder(move || s(S::FindFile, language).to_owned())
                .font_family(FONT_INPUT)
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
                move |row: ReadSignal<Row>, _| {
                    let row = row.get();
                    let open_state = state.clone();
                    let path = row.path.clone();
                    let highlight = palette.highlighted;
                    // The key, not the index: filtering reuses a row's
                    // element with its index read once, so an index would
                    // point at whatever moved into that place.
                    let key = row.key.clone();
                    prims::press_feedback(view())
                        .test_id("palette-row")
                        .w_full()
                        .flex_row()
                        .items_center()
                        .gap(px(SPACE_3))
                        .px(px(SPACE_2))
                        .py(px(SPACE_1))
                        .cursor_pointer()
                        .hover(|s| s.text_color(|| tokens::palette().ink))
                        .on_press(move |_event: &PressEvent| {
                            palette.close(&open_state);
                            open_state.open_file(path.clone());
                        })
                        .child(
                            text(row.label.clone())
                                .font_family(FONT_UI)
                                .text_size(px(TYPE_UI))
                                .line_height(px(TYPE_UI_LINE))
                                .text_color(move || {
                                    if highlight.get().as_deref() == Some(key.as_str()) {
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
                                .text_color(move || tokens::palette().ink_caption)
                                .truncate(),
                        )
                },
            )),
    )
}

/// What the line says under its rows when it is not simply listing them: the
/// search at work, and a query that found nothing.
fn notice(state: &AppState, palette: &Palette, language: Language) -> Option<SharedString> {
    // With no folder and nothing open, every list is empty for a reason the
    // line can name, instead of reading as a search that found nothing.
    if state.root.get().is_none() && state.tabs.get().is_empty() {
        return Some(SharedString::from(s(S::NoFolder, language)));
    }
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

/// Enter in the palette: open the row the keyboard is on; in the search, the
/// first Enter sends the query and the next opens what it found.
fn submit(state: &AppState, palette: Palette) {
    let text = palette.text(state);
    if palette.mode.get() == PaletteMode::Search && palette.submitted.get() != text {
        palette.submitted.set(text.clone());
        state.run_search(&text);
        return;
    }
    let Some(row) = palette.current_row(state) else {
        return;
    };
    let path = row.path.clone();
    palette.close(state);
    state.open_file(path);
}

/// The rail's copy. New strings live here rather than in `i18n.rs` (a merge
/// hotspot): the match is exhaustive, so a missing translation is a compile
/// error.
#[derive(Clone, Copy)]
enum S {
    FindFile,
    Close,
    NoFolder,
    OutsideFolder,
    UnlistedFiles,
}

fn s(key: S, language: Language) -> &'static str {
    match language {
        Language::English => match key {
            S::FindFile => "Find a file",
            S::Close => "Close",
            S::NoFolder => "Open a folder first to search its files.",
            S::OutsideFolder => "Outside folder",
            S::UnlistedFiles => "Open files not in the tree",
        },
        Language::Chinese => match key {
            S::FindFile => "查找文件",
            S::Close => "关闭",
            S::NoFolder => "请先打开文件夹以搜索其中的文件。",
            S::OutsideFolder => "文件夹外",
            S::UnlistedFiles => "未列在树中的打开文件",
        },
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use nsh_core::{
        MountItem, Points,
        platform::{ColorScheme, Idiom, Os},
    };
    use nsh_engine::EngineConfig;
    use nsh_testing::Headless;

    use super::*;

    #[component]
    fn PaletteProbe() -> impl IntoElement {
        let state = AppState::new();
        state.new_document();
        state
            .tabs
            .update(|tabs| tabs[0].title = SharedString::from("Inactive"));
        state.new_document();
        view()
            .w(px(640.0))
            .h(px(400.0))
            .flex_row()
            .child(rail(state, Language::English, Layout::Narrow))
            .child(prims::primary_action("palette-primary", "Primary", || {}))
    }

    fn node_with(app: &Headless, test_id: &str) -> nsh_core::NodeId {
        app.screen()
            .layout_nodes()
            .into_iter()
            .find(|node| node.test_id.as_deref() == Some(test_id))
            .unwrap_or_else(|| panic!("the `{test_id}` view is mounted"))
            .id
    }

    fn rail_name_color(app: &Headless) -> Color {
        app.screen()
            .span_style("id:sidebar-panel >> text:Inactive")
            .expect("the hovered inactive rail name style")
            .color
    }

    #[test]
    fn hovered_sidebar_and_focused_actions_track_the_current_theme() {
        let mut config =
            EngineConfig::new(nsh_testing::windowed_root::<PaletteProbe>().tokens(tokens::theme()));
        config.environment.color_scheme = ColorScheme::Light;
        config.environment.idiom = Idiom::Desktop;
        config.environment.os = Os::Windows;
        config.environment.reduce_motion = true;
        let mut app = Headless::start(config).expect("start the palette probe");
        app.set_viewport(nsh_core::Size::new(Points(640.0), Points(400.0)))
            .expect("set the viewport");
        app.wait_for("id:sidebar-strip")
            .expect("the collapsed sidebar");
        app.press_key("mod+b").expect("open the sidebar panel");
        app.settle();
        app.wait_for("id:sidebar-panel >> text:Inactive")
            .expect("the inactive rail name");
        app.hover("id:sidebar-panel >> text:Inactive")
            .expect("hover the inactive rail name");
        app.settle();

        let rail_close = node_with(&app, "rail-close");
        let primary = node_with(&app, "palette-primary");

        let border_colors = Arc::new(Mutex::new(Vec::new()));
        app.observe({
            let border_colors = border_colors.clone();
            move |_, batch| {
                for item in &batch.items {
                    if let MountItem::ViewStyle { id, style } = item
                        && (*id == rail_close || *id == primary)
                    {
                        border_colors
                            .lock()
                            .expect("lock the border record")
                            .push((*id, style.borders.colors[0]));
                    }
                }
            }
        });
        let border_color = |node| {
            border_colors
                .lock()
                .expect("lock the border record")
                .iter()
                .rev()
                .find(|(id, _)| *id == node)
                .map(|(_, color)| *color)
        };

        for _ in 0..8 {
            if border_color(rail_close) == Some(tokens::LIGHT.ink) {
                break;
            }
            app.press_key("tab").expect("focus the rail close control");
        }
        let light_name_before_switch = rail_name_color(&app);
        let light_close_before_switch = border_color(rail_close);

        app.set_environment("system appearance becomes dark", |env| {
            env.color_scheme = ColorScheme::Dark;
        })
        .expect("report OS appearance");
        app.settle();

        let dark_name_color = rail_name_color(&app);
        let dark_close_color = border_color(rail_close);

        for _ in 0..8 {
            if [tokens::LIGHT.ink, tokens::DARK.ink]
                .contains(&border_color(primary).unwrap_or(Color::TRANSPARENT))
            {
                break;
            }
            app.press_key("tab").expect("focus the primary action");
        }
        let dark_primary_color = border_color(primary);

        app.set_environment("system appearance becomes light", |env| {
            env.color_scheme = ColorScheme::Light;
        })
        .expect("report OS appearance");
        app.settle();

        let light_name_after_switch = rail_name_color(&app);
        let light_primary_color = border_color(primary);
        assert_eq!(
            (
                light_name_before_switch,
                light_close_before_switch,
                dark_name_color,
                dark_close_color,
                dark_primary_color,
                light_name_after_switch,
                light_primary_color
            ),
            (
                tokens::LIGHT.ink,
                Some(tokens::LIGHT.ink),
                tokens::DARK.ink,
                Some(tokens::DARK.ink),
                Some(tokens::DARK.ink),
                tokens::LIGHT.ink,
                Some(tokens::LIGHT.ink)
            )
        );
    }
}
