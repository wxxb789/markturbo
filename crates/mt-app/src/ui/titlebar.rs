//! Contract: [`title_row`] fills the row and owns its height and its padding.
//! The window runs with the system title bar hidden (`shell` sets it), so the
//! row reserves the native controls' area — `prims::caption_insets()` — and
//! marks itself draggable, or the window cannot be moved.
//!
//! `Review` stays in the row, dimmed while unavailable.

use std::{cell::Cell, time::Duration};

use mt_core::settings::Language;
use nsh_core::{
    AccessibilityCurrent, AccessibilityState, Color,
    element::{AnimatedValue, KeyOptions},
    keymap::When,
    prelude::*,
};

use crate::{
    app_paths,
    i18n::{self, Key},
    state::{AppState, Pane, flow_copy},
    ui::{
        consent, motion, notices, prims, revision, shell,
        tokens::{
            self, COLUMN_GAP, HAIRLINE, RAIL_DOT, RAIL_W, SPACE_1, SPACE_2, SPACE_3, TITLE_ROW_H,
            TYPE_UI, TYPE_UI_LINE,
        },
        welcome,
    },
};

/// The menu's width: every row and its shortcut stay on one line.
const MENU_W: f32 = 240.0;

#[derive(Clone, Copy)]
struct MenuMotion {
    open: Signal<bool>,
    visible: Signal<bool>,
    opacity: AnimatedValue,
    scale: AnimatedValue,
}

impl MenuMotion {
    fn new(open: Signal<bool>) -> Self {
        let visible = signal(false);
        let opacity = animated_value(0.0);
        let reduced = motion::reduced_motion();
        let scale = animated_value(if reduced { 1.0 } else { motion::MENU_SCALE });
        let hide_task = Cell::new(None::<Spawned<()>>);
        let timer_owner = Owner::current();
        let previous = Cell::new(false);
        let previous_reduced = Cell::new(reduced);

        effect(move || {
            let cancel_hide = || {
                if let Some(hide) = hide_task.take() {
                    hide.cancel();
                }
            };
            let schedule_hide = |duration: f32| {
                cancel_hide();
                hide_task.set(Some(timer_owner.run(|| {
                    set_timeout(Duration::from_millis(duration as u64), move || {
                        visible.set(false);
                    })
                })));
            };
            let is_open = open.get();
            let reduced = motion::reduced_motion();
            let open_changed = previous.replace(is_open) != is_open;
            let motion_changed = previous_reduced.replace(reduced) != reduced;
            if !open_changed {
                if motion_changed && reduced {
                    opacity.timing(
                        if is_open { 1.0 } else { 0.0 },
                        motion::REDUCED_CROSSFADE_MS,
                        motion::EASE_OUT,
                    );
                    scale.stop();
                    scale.set(1.0);
                    if !is_open && visible.get_untracked() {
                        schedule_hide(motion::REDUCED_CROSSFADE_MS);
                    }
                }
                return;
            }

            if is_open {
                cancel_hide();
                let was_visible = visible.get_untracked();
                visible.set(true);
                opacity.timing(
                    1.0,
                    if reduced {
                        motion::REDUCED_CROSSFADE_MS
                    } else {
                        motion::MENU_ENTER_MS
                    },
                    motion::EASE_OUT,
                );
                if reduced {
                    scale.set(1.0);
                } else {
                    if !was_visible {
                        scale.set(motion::MENU_SCALE);
                    }
                    scale.timing(1.0, motion::MENU_ENTER_MS, motion::EASE_OUT);
                }
            } else {
                let duration = if reduced {
                    motion::REDUCED_CROSSFADE_MS
                } else {
                    motion::MENU_EXIT_MS
                };
                opacity.timing(0.0, duration, motion::EASE_OUT);
                scale.stop();
                if reduced {
                    scale.set(1.0);
                }
                schedule_hide(duration);
            }
        });

        Self {
            open,
            visible,
            opacity,
            scale,
        }
    }

    fn is_open(self) -> bool {
        self.open.get()
    }

    fn is_visible(self) -> bool {
        self.visible.get()
    }

    fn toggle(self) {
        self.open.update(|open| *open = !*open);
    }

    fn close(self) {
        self.open.set(false);
    }
}

pub fn title_row(state: AppState, language: Language) -> impl IntoElement {
    // `Review` (and the rest of `Ctrl+Enter`) from wherever the focus is.
    let review_state = state.clone();
    effect(move || bind_mod_enter(&review_state));

    let menu = MenuMotion::new(state.title_menu_open);

    // Escape stands a waiting Translation down, as it does a waiting Review:
    // the late answer is discarded by generation and nothing is applied. The
    // menu is the topmost layer: while it is open, Escape closes it alone.
    let translation_state = state.clone();
    effect(move || {
        if !translation_state.translation_busy.get() || consent::is_open(&translation_state) {
            return;
        }
        let keys_state = translation_state.clone();
        on_key_with(
            "escape",
            KeyOptions {
                when: When::Always,
                ..KeyOptions::default()
            },
            move |_| {
                if keys_state.escape_top_layer() {
                    return;
                }
                keys_state.cancel_translation_in_flight();
            },
        );
    });

    // The menu's document commands, reachable without it. While a consent
    // card is open it is modal: these shortcuts do nothing (the keymap still
    // consumes the key, so the card's own keys are the only ones that act).
    let open_state = state.clone();
    on_key("mod+o", move |_: &str| {
        if !consent::is_open(&open_state) {
            welcome::choose_folder(open_state.clone());
        }
    });
    let new_state = state.clone();
    on_key("mod+n", move |_: &str| {
        if !consent::is_open(&new_state) {
            new_state.new_document();
        }
    });
    // Save; an untitled document goes to Save As. The source editor binds no
    // `mod+s`, so the default `when` fires it from the editor too.
    let save_state = state.clone();
    on_key("mod+s", move |_: &str| {
        if !consent::is_open(&save_state) {
            save_state.save_active();
        }
    });
    let save_as_state = state.clone();
    on_key("shift+mod+s", move |_: &str| {
        if save_as_state.active.get().is_some() && !consent::is_open(&save_as_state) {
            save_as_state.save_as_active();
        }
    });
    let settings_state = state.clone();
    on_key("mod+,", move |_: &str| {
        if !consent::is_open(&settings_state) {
            settings_state.toggle_settings();
        }
    });
    // Every Escape binding runs the same fixed-order decision, so the
    // newest one cannot pick a layer the contract puts lower.
    let escape_state = state.clone();
    on_key_with(
        "escape",
        KeyOptions {
            when: When::Always,
            pass_through: true,
            ..KeyOptions::default()
        },
        move |_: &str| {
            escape_state.escape_top_layer();
        },
    );

    view()
        .test_id("title-row")
        .relative()
        // Above the workspace and the field, so the menu's panel is not
        // painted over. The shell's overlay layer (the consent card) belongs
        // to the window's root view and stays above the whole room.
        .z_index(5)
        // The native controls' area arrives after the first frame and changes
        // when the window is maximized: the row's height and insets are bound
        // to it, so they follow without the row being rebuilt.
        .h(move || Length::from(px(TITLE_ROW_H.max(prims::caption_insets().top.0))))
        .w_full()
        .flex_row()
        .items_center()
        .gap(px(SPACE_3))
        .pl(move || {
            Length::from(px(
                (RAIL_W + COLUMN_GAP).max(prims::caption_insets().left.0 + SPACE_3)
            ))
        })
        .pr(move || Length::from(px(prims::caption_insets().right.0 + SPACE_3)))
        .bg(move || tokens::palette().room)
        .window_drag(true)
        .child(when(move || menu.is_open(), {
            let state = state.clone();
            move || menu_scrim(state.clone(), menu)
        }))
        .child(when(
            {
                let state = state.clone();
                move || state.active.get().is_some()
            },
            {
                let state = state.clone();
                move || file_name(state.clone(), language).into_element()
            },
        ))
        .child(view().flex_1())
        .child(review_action(state.clone(), language))
        .child(menu_anchor(state.clone(), language, menu))
}

/// Binds `Ctrl+Enter` (`mod+enter`; on an Apple keyboard `ctrl+enter` too, the
/// key the hints name) to [`mod_enter`], the app's one handler for it. Call it
/// inside an effect: the binding is dropped with the effect's run, and it is
/// not made while a consent card is up, whose own focused handlers measure its
/// hold.
///
/// The keymap fires the newest binding of a keystroke, and the source editor
/// binds its own `mod+enter` (insert a line below) every time it takes the
/// focus. So besides the title row, the source editor binds this again on
/// each of its `Focus` events (`document`), and the surfaces that
/// claim the key (the Revision field's `Done`) bind it as they come — every
/// one of them the same binding.
pub fn bind_mod_enter(state: &AppState) {
    // The consent card has the keyboard focus while it is open: its own
    // `KeyDown`/`KeyUp` handlers measure the hold, and a binding here would
    // take `Ctrl+Enter` from them before they see it. Not binding also makes
    // the effects that call this re-run when a card opens or closes.
    if consent::is_open(state) {
        return;
    }
    let keys: &[&str] = if apple_keys() {
        &["mod+enter", "ctrl+enter"]
    } else {
        &["mod+enter"]
    };
    for keystroke in keys {
        let state = state.clone();
        on_key_with(
            keystroke,
            KeyOptions {
                // `enter` is a key a text input uses: the binding asks for
                // the focus wherever it is.
                when: When::Always,
                ..KeyOptions::default()
            },
            move |_: &str| mod_enter(&state),
        );
    }
}

/// `Ctrl+Enter`: `Done` while a fresh proposal is docked, else `Review`.
/// While a consent card is up the key belongs to the card's own handlers
/// ([`consent::card`]), so a binding that fires anyway does nothing.
fn mod_enter(state: &AppState) {
    if consent::is_open(state) {
        return;
    }
    if revision::done_applies(state) {
        revision::done(state);
        return;
    }
    // A revision on screen is a decision waiting: reviewing over it would
    // drop the proposal being decided. A review in flight or a guard decision
    // is not restarted either.
    if state.active.get().is_some()
        && state.revision_hunks.get().is_empty()
        && !notices::guard_is_open(state)
        && !state.review_busy.get()
        && !state.reopen_review()
    {
        state.start_review();
    }
}

/// The file's name, with its folder at non-text ink ahead of it so two files
/// of one name are told apart, and a mark while the document is not saved.
/// Pressing it opens (and closes) Context: the path the name stands for is
/// the order Codex applies its instructions in.
fn file_name(state: AppState, language: Language) -> impl IntoElement {
    let folder_state = state.clone();
    let name_state = state.clone();
    let label_state = state.clone();
    let dirty_state = state.clone();
    let press_state = state.clone();
    prims::press_feedback(
        view()
            .test_id("title-file")
            .role(AccessibilityRole::Button)
            .optional_label(move || {
                label_state
                    .active_tab()
                    .map(|tab| SharedString::from(show_context_label(language, &tab.title)))
            })
            .window_drag(false)
            .cursor_pointer()
            .bg(Color::TRANSPARENT)
            .on_press(move || {
                press_state.toggle_context();
                press_state.focus_editor();
            }),
    )
    .flex_row()
    .items_center()
    .gap(px(SPACE_2))
    .flex_shrink(1.0)
    .min_w(px(0.0))
    .hover(|s| s.text_color(|| tokens::palette().ink).underline())
    .focus_visible(|s| s.text_color(|| tokens::palette().ink).underline())
    .child(
        text(move || match folder_state.active_tab() {
            Some(tab) if !tab.untitled => tab
                .path
                .parent()
                .and_then(|parent| parent.file_name())
                .and_then(|name| name.to_str())
                .map(|name| SharedString::from(format!("{name} /")))
                .unwrap_or_default(),
            _ => SharedString::from(""),
        })
        .font_family(tokens::FONT_UI)
        .text_size(px(TYPE_UI))
        .line_height(px(TYPE_UI_LINE))
        .text_color(move || tokens::palette().ink_caption),
    )
    .child(
        text(move || match name_state.active_tab() {
            Some(tab) => tab.title.clone(),
            None => SharedString::from(""),
        })
        .test_id("title-file-name")
        .font_family(tokens::FONT_UI)
        .text_size(px(TYPE_UI))
        .line_height(px(TYPE_UI_LINE))
        .text_color(move || tokens::palette().ink_rest)
        .flex_shrink(1.0)
        .min_w(px(0.0))
        .truncate(),
    )
    .child(when(
        move || dirty_state.active_tab().is_some_and(|tab| tab.dirty.get()),
        move || {
            text("\u{25CF}")
                .test_id("title-dirty")
                .font_family(tokens::FONT_UI)
                .text_size(px(TYPE_UI))
                .line_height(px(TYPE_UI_LINE))
                .text_color(move || tokens::palette().ink)
                .into_element()
        },
    ))
}

fn review_action(state: AppState, language: Language) -> impl IntoElement {
    let enabled_state = state.clone();
    let blocked_state = state.clone();
    let enabled = move || {
        enabled_state.active.get().is_some()
            && enabled_state.revision_hunks.get().is_empty()
            && !enabled_state.review_start_waiting()
    };
    let press_enabled = enabled.clone();
    let waiting_state = state.clone();
    let hint_state = state.clone();
    let start = state;
    view()
        .flex_row()
        .items_center()
        .gap(px(SPACE_2))
        .child(
            row_action(
                "title-review",
                i18n::t(language, Key::Review),
                true,
                enabled,
                move || {
                    if press_enabled() && !start.reopen_review() {
                        // A newly opened consent card keeps focus; focus_editor
                        // only restores it when Review did not open a modal.
                        start.start_review();
                        start.focus_editor();
                    }
                },
            )
            .optional_accessibility_description(move || {
                if waiting_state.review_start_waiting() {
                    Some(SharedString::from(
                        flow_copy::S::ReviewWaitingElsewhere.text(language),
                    ))
                } else if !blocked_state.revision_hunks.get().is_empty() {
                    Some(SharedString::from(review_blocked_label(language)))
                } else {
                    None
                }
            }),
        )
        .child(when(
            move || hint_state.review_start_waiting(),
            move || {
                view()
                    .test_id("title-review-wait")
                    .flex_shrink(1.0)
                    .min_w(px(0.0))
                    .child(
                        prims::faint(move || flow_copy::S::ReviewWaitingElsewhere.text(language))
                            .truncate(),
                    )
            },
        ))
}

/// `Settings`, the word that opens the row's menu: quieter than `Review`,
/// and it tells screen readers whether its menu is open.
fn menu_anchor(state: AppState, language: Language, menu: MenuMotion) -> impl IntoElement {
    let panel_state = state;
    view()
        .test_id("title-menu-anchor")
        .relative()
        .child(
            row_action(
                "title-settings",
                i18n::t(language, Key::Settings),
                false,
                || true,
                move || {
                    menu.toggle();
                },
            )
            .accessibility_state(move || AccessibilityState {
                expanded: Some(menu.is_open()),
                has_popup: true,
                ..AccessibilityState::default()
            }),
        )
        .child(when(
            // Keep the panel's rows and focus handles alive through a reversal.
            move || menu.is_visible(),
            move || menu_panel(panel_state.clone(), language, menu).into_element(),
        ))
}

fn menu_panel(state: AppState, language: Language, menu: MenuMotion) -> impl IntoElement {
    let document_rows = signal(state.active.get_untracked().is_some());
    let pane_selection = signal(untracked(|| state.active_tab().map(|tab| tab.pane.get())));
    let document_state = state.clone();
    // Retain the menu's last snapshot while it fades out.
    effect(move || {
        if menu.is_open() {
            document_rows.set(document_state.active.get().is_some());
            pane_selection.set(document_state.active_tab().map(|tab| tab.pane.get()));
        }
    });
    let document = move || document_rows.get();
    let panel = menu_panel_frame(menu, "title-menu");
    panel
        .child({
            let open_state = state.clone();
            let first = element_ref();
            effect(move || {
                if menu.is_open() {
                    first.focus();
                }
            });
            menu_row(
                menu,
                "title-menu-open-folder",
                s(S::OpenFolder, language),
                Some(shortcut(false, "O")),
                || true,
                move || {
                    menu.close();
                    welcome::choose_folder(open_state.clone());
                },
            )
            .element_ref(&first)
        })
        // The welcome's two ways to start a document, wherever the menu is
        // opened from: the new document becomes the active tab.
        .child({
            let new_state = state.clone();
            menu_row(
                menu,
                "title-menu-new-file",
                s(S::NewFile, language),
                Some(shortcut(false, "N")),
                || true,
                move || {
                    menu.close();
                    new_state.new_document();
                },
            )
        })
        .child({
            let paste_state = state.clone();
            menu_row(
                menu,
                "title-menu-paste",
                s(S::PasteAsNew, language),
                None,
                || true,
                move || {
                    menu.close();
                    paste_state.new_from_clipboard();
                },
            )
        })
        .child(when(app_paths::bundled_sample_available, {
            let sample_state = state.clone();
            move || {
                let press_state = sample_state.clone();
                menu_row(
                    menu,
                    "title-menu-open-sample",
                    s(S::OpenSample, language),
                    None,
                    || true,
                    move || {
                        menu.close();
                        press_state.open_bundled_sample();
                    },
                )
                .into_element()
            }
        }))
        .child(
            view()
                .px(px(SPACE_3))
                .py(px(SPACE_2))
                .child(prims::hairline())
                .into_element(),
        )
        .child(when(document, {
            let state = state.clone();
            move || {
                let row = |test_id: &'static str,
                           label: &'static str,
                           hint: Option<SharedString>,
                           run: fn(&AppState)| {
                    let state = state.clone();
                    menu_row(
                        menu,
                        test_id,
                        label,
                        hint,
                        || true,
                        move || {
                            menu.close();
                            run(&state);
                        },
                    )
                    .into_element()
                };
                let pane = |test_id: &'static str, label: &'static str, pane: Pane| {
                    pane_row(&state, menu, pane_selection, test_id, label, pane)
                };
                view()
                    .child(row(
                        "title-menu-save",
                        i18n::t(language, Key::Save),
                        Some(shortcut(false, "S")),
                        |state| state.save_active(),
                    ))
                    .child(row(
                        "title-menu-save-as",
                        s(S::SaveAs, language),
                        Some(shortcut(true, "S")),
                        |state| state.save_as_active(),
                    ))
                    .child(pane(
                        "title-menu-edit",
                        i18n::t(language, Key::ModeSource),
                        Pane::Source,
                    ))
                    .child(pane(
                        "title-menu-preview",
                        i18n::t(language, Key::ModeNative),
                        Pane::Preview,
                    ))
                    .child(pane("title-menu-split", s(S::Split, language), Pane::Split))
                    .child(pane(
                        "title-menu-web",
                        s(S::WebPreview, language),
                        Pane::Web,
                    ))
                    .child(pane(
                        "title-menu-split-web",
                        s(S::SplitWeb, language),
                        Pane::SplitWeb,
                    ))
                    .child(row(
                        "title-menu-translate",
                        i18n::t(language, Key::Translate),
                        None,
                        |state| state.translate_active(),
                    ))
                    .child({
                        let enabled_state = state.clone();
                        let waiting_state = state.clone();
                        let review_state = state.clone();
                        menu_row(
                            menu,
                            "title-menu-review-selection",
                            s(S::ReviewSelection, language),
                            None,
                            move || !enabled_state.review_busy_elsewhere.get(),
                            move || {
                                menu.close();
                                review_state.start_review_selection();
                            },
                        )
                        .optional_accessibility_description(move || {
                            waiting_state.review_busy_elsewhere.get().then(|| {
                                SharedString::from(
                                    flow_copy::S::ReviewWaitingElsewhere.text(language),
                                )
                            })
                        })
                        .into_element()
                    })
                    .child({
                        let hint_state = state.clone();
                        when(
                            move || hint_state.review_busy_elsewhere.get(),
                            move || {
                                view()
                                    .test_id("title-menu-review-wait")
                                    .w_full()
                                    .px(px(SPACE_3))
                                    .pb(px(SPACE_2))
                                    .child(prims::faint(move || {
                                        flow_copy::S::ReviewWaitingElsewhere.text(language)
                                    }))
                            },
                        )
                    })
                    .into_element()
            }
        }))
        .child(when(
            {
                let gate_state = state.clone();
                move || gate_state.root.get().is_some() || document_rows.get()
            },
            {
                let context_state = state.clone();
                move || {
                    menu_item(
                        &context_state,
                        menu,
                        "title-menu-context",
                        s(S::Context, language),
                        |state| state.toggle_context(),
                    )
                    .into_element()
                }
            },
        ))
        .child(
            view()
                .px(px(SPACE_3))
                .py(px(SPACE_2))
                .child(prims::hairline())
                .into_element(),
        )
        .child(menu_item(
            &state,
            menu,
            "title-menu-settings",
            i18n::t(language, Key::Settings),
            |state| state.open_settings(),
        ))
        .child(menu_item(
            &state,
            menu,
            "title-menu-quit",
            i18n::t(language, Key::Quit),
            |state| state.request_quit(),
        ))
}

fn menu_panel_frame(menu: MenuMotion, test_id: &str) -> View {
    view()
        .test_id(test_id)
        .absolute()
        .top(px(TITLE_ROW_H))
        .right(px(0.0))
        .w(px(MENU_W))
        .flex_col()
        .py(px(SPACE_2))
        .bg(move || tokens::palette().room)
        .border(px(HAIRLINE))
        .border_color(move || tokens::palette().hairline)
        .origin_top_right()
        .bind_opacity(&menu.opacity)
        .bind_scale(&menu.scale)
        .pointer_events(move || {
            if menu.is_open() {
                PointerEvents::Auto
            } else {
                PointerEvents::None
            }
        })
        .focusable(false)
}

fn menu_item(
    state: &AppState,
    menu: MenuMotion,
    test_id: &'static str,
    label: &'static str,
    run: fn(&AppState),
) -> impl IntoElement {
    let state = state.clone();
    menu_row(
        menu,
        test_id,
        label,
        None,
        || true,
        move || {
            menu.close();
            run(&state);
        },
    )
}

fn pane_row(
    state: &AppState,
    menu: MenuMotion,
    pane_selection: Signal<Option<Pane>>,
    test_id: &'static str,
    label: &'static str,
    pane: Pane,
) -> AnyElement {
    let on_screen = move || pane_selection.get() == Some(pane);
    let current = on_screen;
    let state = state.clone();
    menu_row(
        menu,
        test_id,
        label,
        None,
        || true,
        move || {
            menu.close();
            if let Some(tab) = state.active_tab() {
                state.set_pane(tab.id, pane);
            }
        },
    )
    .accessibility_state(move || AccessibilityState {
        current: current().then_some(AccessibilityCurrent::True),
        ..AccessibilityState::default()
    })
    .child(when(on_screen, || {
        view()
            .test_id("title-menu-pane-dot")
            .w(px(RAIL_DOT))
            .h(px(RAIL_DOT))
            .rounded(px(RAIL_DOT / 2.0))
            .flex_shrink_0()
            .bg(move || tokens::palette().ink)
            .into_element()
    }))
    .into_element()
}

/// A shortcut as the menu names it: `Ctrl+S`, `Ctrl+Shift+S`; `⌘S`, `⇧⌘S`
/// where `mod` is Command.
pub(crate) fn shortcut(shift: bool, key: &str) -> SharedString {
    SharedString::from(match (apple_keys(), shift) {
        (true, false) => format!("\u{2318}{key}"),
        (true, true) => format!("\u{21E7}\u{2318}{key}"),
        (false, false) => format!("Ctrl+{key}"),
        (false, true) => format!("Ctrl+Shift+{key}"),
    })
}

/// The click-catcher under the menu: a press anywhere else closes it through
/// [`AppState::close_title_menu`], so the keyboard returns to the document.
/// It is the row's first child, so what the row holds stays pressable above
/// it.
fn menu_scrim(state: AppState, menu: MenuMotion) -> impl IntoElement {
    view()
        .test_id("title-menu-scrim")
        // A backdrop closes a popover for the pointer; it is not a stop of
        // the keyboard focus.
        .focusable(false)
        .absolute()
        .top(px(0.0))
        .left(px(0.0))
        .w(move || px(shell::client_width_now()))
        .h(move || px(window().height.0))
        .pointer_events(move || {
            if menu.is_open() {
                PointerEvents::Auto
            } else {
                PointerEvents::None
            }
        })
        .on_press(move || state.close_title_menu())
}

/// Hover and focus styles belong on the label text so they reach its glyphs.
fn menu_row(
    menu: MenuMotion,
    test_id: &'static str,
    label: &'static str,
    hint: Option<SharedString>,
    enabled: impl Fn() -> bool + Clone + 'static,
    on_press: impl Fn() + 'static,
) -> View {
    let duration = prims::press_color_duration();
    let label_opacity = enabled.clone();
    prims::press_feedback_with_duration(
        view()
            .test_id(test_id)
            .w_full()
            .role(AccessibilityRole::Button)
            .optional_label(Some(SharedString::from(label)))
            .window_drag(false)
            .cursor_pointer()
            .bg(Color::TRANSPARENT)
            .flex_row()
            .items_center()
            .gap(px(SPACE_3))
            .px(px(SPACE_3))
            .py(px(SPACE_2))
            .group()
            .disabled(move || !menu.is_open() || !enabled())
            .focusable(move || menu.is_open())
            .on_press(on_press),
        duration,
    )
    .child(
        text(label)
            .font_family(tokens::FONT_UI)
            .text_size(px(TYPE_UI))
            .line_height(px(TYPE_UI_LINE))
            .text_color(move || {
                if label_opacity() {
                    tokens::palette().ink_rest
                } else {
                    tokens::with_alpha(tokens::palette().ink_rest, 0.45)
                }
            })
            .flex_1()
            .min_w(px(0.0))
            .truncate()
            .transition_colors()
            .duration(move || duration.get())
            .group_hover(|s| s.text_color(|| tokens::palette().ink).underline())
            .group_focus_visible(|s| s.text_color(|| tokens::palette().ink).underline()),
    )
    .children(hint.map(|hint| {
        text(hint)
            .font_family(tokens::FONT_UI)
            .text_size(px(TYPE_UI))
            .line_height(px(TYPE_UI_LINE))
            .text_color(|| tokens::palette().ink_caption)
            .flex_shrink_0()
            .into_element()
    }))
}

fn row_action(
    test_id: &'static str,
    label: &'static str,
    strong: bool,
    enabled: impl Fn() -> bool + Clone + 'static,
    on_press: impl Fn() + 'static,
) -> View {
    let duration = prims::press_color_duration();
    let mut label_text = text(label)
        .font_family(if strong {
            tokens::FONT_MEDIUM
        } else {
            tokens::FONT_UI
        })
        .font_weight(if strong {
            tokens::WEIGHT_MEDIUM
        } else {
            tokens::WEIGHT_UI
        })
        .text_size(px(TYPE_UI))
        .line_height(px(TYPE_UI_LINE))
        .text_color(move || {
            if strong {
                tokens::palette().ink
            } else {
                tokens::palette().ink_rest
            }
        })
        .underline_offset(px(4.0))
        .transition_colors()
        .duration(move || duration.get())
        .group_hover(|s| {
            s.text_color(|| tokens::palette().ink)
                .underline()
                .text_decoration_color(|| tokens::palette().ink)
        })
        .group_focus_visible(|s| {
            s.text_color(|| tokens::palette().ink)
                .underline()
                .text_decoration_color(|| tokens::palette().ink)
        });
    if strong {
        label_text = label_text
            .underline()
            .text_decoration_color(move || tokens::palette().ink_at(0.34));
    }
    let press_enabled = enabled.clone();
    prims::press_feedback_with_duration(
        view()
            .test_id(test_id)
            .role(AccessibilityRole::Button)
            .optional_label(Some(SharedString::from(label)))
            .window_drag(false)
            .cursor_pointer()
            .bg(Color::TRANSPARENT)
            .rounded(px(0.0))
            .px(px(SPACE_1))
            .py(px(SPACE_1))
            .group()
            .disabled(move || !press_enabled())
            .disabled_style(|s| s.opacity(0.45))
            .on_press(on_press),
        duration,
    )
    .child(label_text)
}

/// The title row's copy. New strings live here rather than in `i18n.rs` (a
/// merge hotspot): the match is exhaustive, so a missing translation is a
/// compile error.
#[derive(Clone, Copy)]
enum S {
    OpenFolder,
    NewFile,
    PasteAsNew,
    OpenSample,
    SaveAs,
    Split,
    WebPreview,
    SplitWeb,
    Context,
    ReviewSelection,
}

fn s(key: S, language: Language) -> &'static str {
    match language {
        Language::English => match key {
            S::OpenFolder => "Open folder\u{2026}",
            S::NewFile => "New file",
            S::PasteAsNew => "Paste as new document",
            S::OpenSample => "Open sample",
            S::SaveAs => "Save As\u{2026}",
            S::Split => "Split",
            S::WebPreview => "Compatibility preview",
            S::SplitWeb => "Split \u{00B7} Compatibility",
            S::Context => "Context",
            S::ReviewSelection => "Review selection",
        },
        Language::Chinese => match key {
            S::OpenFolder => "打开文件夹\u{2026}",
            S::NewFile => "新建文件",
            S::PasteAsNew => "粘贴为新文档",
            S::OpenSample => "打开示例",
            S::SaveAs => "另存为\u{2026}",
            S::Split => "分栏",
            S::WebPreview => "兼容预览",
            S::SplitWeb => "分栏 \u{00B7} 兼容预览",
            S::Context => "上下文",
            S::ReviewSelection => "审查选区",
        },
    }
}

fn show_context_label(language: Language, name: &str) -> String {
    match language {
        Language::English => format!("Show agent context for {name}"),
        Language::Chinese => format!("显示 {name} 的智能体上下文"),
    }
}

fn review_blocked_label(language: Language) -> &'static str {
    match language {
        Language::English => "Finish or close the current revision before running Review.",
        Language::Chinese => "请先完成或关闭当前修订，再运行审查。",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc::{self, Receiver, Sender};

    use nsh_engine::{EngineConfig, Points, Root};
    use nsh_testing::Headless;

    #[component]
    fn ReviewProbe() -> impl IntoElement {
        title_row(AppState::new(), Language::English)
    }

    #[component]
    fn WelcomeMenuProbe() -> impl IntoElement {
        shell::shell(AppState::new(), Language::English, tokens::Layout::Wide)
    }

    #[component]
    fn PaneMenuProbe() -> impl IntoElement {
        let state = AppState::new();
        state.new_document();
        let tab = state.active_tab().expect("the untitled tab");
        state.set_pane(tab.id, Pane::Source);
        title_row(state, Language::English)
    }

    #[component]
    fn MenuUnderlayProbe() -> impl IntoElement {
        let pressed = signal(false);
        view()
            .relative()
            .w_full()
            .h_full()
            .child(
                view()
                    .absolute()
                    .top(px(TITLE_ROW_H * 2.0))
                    .right(px(MENU_W / 4.0))
                    .child(prims::quiet_action("underlay", "Press", move || {
                        pressed.set(true)
                    })),
            )
            .child(when(
                move || pressed.get(),
                || prims::ink("Underlay pressed"),
            ))
            .child(pane_menu_probe())
    }

    #[derive(Clone, PartialEq, Props)]
    struct MenuMotionProbeProps {
        content: Option<Renderer<bool>>,
    }

    #[component]
    fn MenuMotionProbe(props: MenuMotionProbeProps) -> impl IntoElement {
        props.content.unwrap().render("menu-motion", true)
    }

    fn menu_motion_contents(samples: Sender<(f32, f32)>) -> impl IntoElement {
        let menu = MenuMotion::new(signal(false));
        let read_task = Cell::new(None::<Spawned<()>>);

        view()
            .test_id("menu-motion-probe")
            .relative()
            .w(px(600.0))
            .h(px(400.0))
            .child(prims::quiet_action(
                "menu-motion-toggle",
                "Toggle",
                move || menu.toggle(),
            ))
            .child(when(
                move || menu.is_visible(),
                move || {
                    menu_panel_frame(menu, "menu-motion-panel")
                        .child(prims::ink("Menu"))
                        .into_element()
                },
            ))
            .child(prims::quiet_action("menu-motion-read", "Read", move || {
                let samples = samples.clone();
                read_task.set(Some(spawn(async move {
                    let opacity = menu.opacity.value().await;
                    let scale = menu.scale.value().await;
                    let _ = samples.send((opacity, scale));
                })));
            }))
    }

    fn manual_app(root: Root) -> Headless {
        let mut engine = EngineConfig::new(root);
        engine.clock = nsh_core::time::Clock::manual(std::time::SystemTime::now());
        let mut app = Headless::start(engine).expect("start the headless app");
        app.set_viewport(nsh_core::Size::new(Points(600.0), Points(400.0)))
            .expect("set the viewport");
        app
    }

    // Headless::press settles animations. Dispatch its press event directly so
    // this test can choose exactly when to advance the manual clock.
    fn press_without_settling(app: &Headless, selector: &str) {
        let id = app.screen().press_target(selector).unwrap().unwrap();
        let engine = app.engine().control();
        engine.dispatch_event(
            id,
            nsh_core::EventKind::Press,
            nsh_core::EventPayload::Press(Default::default()),
        );
        let (sent, received) = mpsc::channel();
        engine.flush(move |_| {
            let _ = sent.send(());
        });
        received.recv_timeout(Duration::from_secs(5)).unwrap();
    }

    fn menu_values(app: &mut Headless, samples: &Receiver<(f32, f32)>) -> (f32, f32) {
        press_without_settling(app, "id:menu-motion-read");
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        loop {
            // Deliver the animator's read reply without driving a frame.
            app.catch_up();
            match samples.recv_timeout(Duration::from_millis(10)) {
                Ok(value) => return value,
                Err(mpsc::RecvTimeoutError::Timeout) if std::time::Instant::now() < deadline => {}
                Err(error) => panic!("read menu motion: {error}"),
            }
        }
    }

    fn menu_item_count(app: &Headless) -> usize {
        [
            "title-menu-open-folder",
            "title-menu-new-file",
            "title-menu-paste",
            "title-menu-open-sample",
            "title-menu-save",
            "title-menu-save-as",
            "title-menu-edit",
            "title-menu-preview",
            "title-menu-split",
            "title-menu-web",
            "title-menu-split-web",
            "title-menu-translate",
            "title-menu-review-selection",
            "title-menu-context",
            "title-menu-settings",
            "title-menu-quit",
        ]
        .iter()
        .filter(|id| app.screen().matches(&format!("id:{id}")).unwrap())
        .count()
    }

    fn pane_dot_is_in_row(app: &Headless, row_id: &str) -> bool {
        let nodes = app.screen().layout_nodes();
        let dot = nodes
            .iter()
            .find(|node| node.test_id.as_deref() == Some("title-menu-pane-dot"))
            .expect("the active pane's menu dot");
        let row = nodes
            .iter()
            .find(|node| node.test_id.as_deref() == Some(row_id))
            .expect("the pane's menu row");
        row.bounds.contains(&dot.bounds.center())
    }

    #[test]
    fn review_stays_visible_and_disabled_without_a_document() {
        let mut app = manual_app(nsh_testing::windowed_root::<ReviewProbe>());
        app.wait_for("id:title-review")
            .expect("Review stays in the row");

        assert!(
            app.screen()
                .accessibility_state("id:title-review")
                .expect("Review's state")
                .disabled,
            "Review is announced as disabled"
        );
        assert_eq!(
            app.screen()
                .press_target("id:title-review")
                .expect("Review press target"),
            None,
            "a disabled Review cannot be pressed"
        );
    }

    #[test]
    fn hovered_titlebar_text_tracks_the_current_theme() {
        use nsh_core::{platform::ColorScheme, tailwind::state::HOVERED};

        let mut config =
            EngineConfig::new(nsh_testing::windowed_root::<ReviewProbe>().tokens(tokens::theme()));
        config.environment.color_scheme = ColorScheme::Light;
        config.environment.reduce_motion = true;
        let mut app = Headless::start(config).expect("start the titlebar");
        app.set_viewport(nsh_core::Size::new(Points(600.0), Points(400.0)))
            .expect("set the viewport");
        app.wait_for("id:title-settings")
            .expect("the title Settings action");
        let settings = app
            .screen()
            .layout_nodes()
            .into_iter()
            .find(|node| node.test_id.as_deref() == Some("title-settings"))
            .expect("the title Settings action")
            .id;
        app.engine().control().set_states(settings, HOVERED);
        app.settle();
        press_without_settling(&app, "id:title-settings");
        app.wait_for("id:title-menu-open-folder")
            .expect("the first title menu row");
        let open_folder = app
            .screen()
            .layout_nodes()
            .into_iter()
            .find(|node| node.test_id.as_deref() == Some("title-menu-open-folder"))
            .expect("the first title menu row")
            .id;
        app.engine().control().set_states(open_folder, HOVERED);
        app.settle();
        let light_colors = (
            app.screen()
                .span_style("id:title-settings >> text:Settings")
                .expect("the hovered title Settings text")
                .color,
            app.screen()
                .span_style("id:title-menu-open-folder >> text:Open folder\u{2026}")
                .expect("the hovered menu row text")
                .color,
        );
        assert_eq!(light_colors, (tokens::LIGHT.ink, tokens::LIGHT.ink));

        app.set_environment("system appearance becomes dark", |env| {
            env.color_scheme = ColorScheme::Dark;
        })
        .expect("report OS appearance");
        app.settle();
        let dark_colors = (
            app.screen()
                .span_style("id:title-settings >> text:Settings")
                .expect("the hovered title Settings text")
                .color,
            app.screen()
                .span_style("id:title-menu-open-folder >> text:Open folder\u{2026}")
                .expect("the hovered menu row text")
                .color,
        );
        assert_eq!(dark_colors, (tokens::DARK.ink, tokens::DARK.ink));
    }

    #[test]
    fn closing_the_welcome_menu_keeps_its_items_frozen_during_exit() {
        let mut app = manual_app(nsh_testing::windowed_root::<WelcomeMenuProbe>());
        app.wait_for("id:welcome")
            .expect("the welcome page before opening a document");
        press_without_settling(&app, "id:title-settings");
        app.wait_for("id:title-menu-new-file")
            .expect("the title menu's New file row");

        let item_count = menu_item_count(&app);
        assert_eq!(item_count, 6, "welcome menu items before New file");

        press_without_settling(&app, "id:title-menu-new-file");
        for _ in 0..2 {
            app.next_frame(Duration::from_millis(16)).unwrap();
        }

        assert!(
            app.screen().matches("id:title-file-name").unwrap(),
            "New file activates a document"
        );
        assert!(
            app.screen().matches("id:title-menu").unwrap(),
            "the menu remains mounted during its exit"
        );
        assert_eq!(
            menu_item_count(&app),
            item_count,
            "the fading menu must retain its welcome item set"
        );
    }

    #[test]
    fn pane_dot_stays_with_the_selection_when_the_menu_fades() {
        let mut app = manual_app(nsh_testing::windowed_root::<PaneMenuProbe>());
        app.wait_for("id:title-settings")
            .expect("the title settings action");
        press_without_settling(&app, "id:title-settings");
        app.wait_for("id:title-menu-preview")
            .expect("the Preview pane row");
        assert!(pane_dot_is_in_row(&app, "title-menu-edit"));

        press_without_settling(&app, "id:title-menu-preview");
        for _ in 0..2 {
            app.next_frame(Duration::from_millis(16))
                .expect("advance during the menu exit");
        }

        assert!(app.screen().matches("id:title-menu").unwrap());
        assert_eq!(
            app.screen()
                .press_target("id:title-menu-edit")
                .expect("the Source row selector"),
            None,
            "closed menu rows cannot receive presses during the exit"
        );
        assert!(pane_dot_is_in_row(&app, "title-menu-edit"));
        assert!(!pane_dot_is_in_row(&app, "title-menu-preview"));
    }

    #[test]
    fn a_fading_menu_leaves_the_underlay_pressable() {
        let mut app = manual_app(nsh_testing::windowed_root::<MenuUnderlayProbe>());
        let settings = "id:title-settings";
        let row = "id:title-menu-split";
        app.wait_for(settings).expect("the settings action");
        app.press(settings).expect("open the menu");
        let row_id = app
            .screen()
            .press_target(row)
            .unwrap()
            .expect("the open row");

        press_without_settling(&app, settings);
        for _ in 0..2 {
            app.next_frame(Duration::from_millis(16))
                .expect("advance into the menu exit");
        }
        let nodes = app.screen().layout_nodes();
        let bounds = |id| {
            nodes
                .iter()
                .find(|node| node.test_id.as_deref() == Some(id))
                .unwrap_or_else(|| panic!("{id} is mounted"))
                .bounds
        };
        assert!(bounds("title-menu").contains(&bounds("underlay").center()));
        assert!(nodes.iter().any(|node| node.id == row_id));
        assert!(!app.screen().matches("id:title-menu-scrim").unwrap());
        app.press(row)
            .expect_err("the fading row cannot take a press");
        app.press("id:underlay")
            .expect("press behind the fading menu");
        assert!(app.screen().matches("text:Underlay pressed").unwrap());
    }

    #[test]
    fn closing_and_reopening_the_menu_retargets_its_live_opacity_and_scale() {
        let (sent, samples) = mpsc::channel();
        let mut app = manual_app(nsh_testing::windowed_root_with::<MenuMotionProbe>(
            move || {
                let sent = sent.clone();
                MenuMotionProbeProps {
                    content: Some(Renderer::new(move |_| menu_motion_contents(sent.clone()))),
                }
            },
        ));
        app.wait_for("id:menu-motion-toggle")
            .expect("the menu control");

        let toggle = "id:menu-motion-toggle";
        press_without_settling(&app, toggle);
        for _ in 0..3 {
            app.next_frame(Duration::from_millis(16)).unwrap();
        }
        let entering = menu_values(&mut app, &samples);
        assert!(
            entering.0 > 0.05 && entering.0 < 0.99,
            "mid-enter opacity: {}",
            entering.0
        );
        assert!(
            entering.1 > motion::MENU_SCALE + 0.001 && entering.1 < 0.999,
            "mid-enter scale must be between its floor and 1: {}",
            entering.1
        );

        press_without_settling(&app, toggle);
        let leaving = menu_values(&mut app, &samples);
        assert!(
            (leaving.0 - entering.0).abs() <= 0.01,
            "close starts at opacity {}, got {}",
            entering.0,
            leaving.0
        );
        assert!(
            (leaving.1 - entering.1).abs() <= 0.001,
            "close starts at scale {}, got {}",
            entering.1,
            leaving.1
        );
        for _ in 0..2 {
            app.next_frame(Duration::from_millis(16)).unwrap();
        }
        let closing = menu_values(&mut app, &samples);
        assert!(
            closing.0 > 0.05 && closing.0 < leaving.0,
            "mid-exit opacity: {}",
            closing.0
        );
        assert!(
            (closing.1 - entering.1).abs() <= 0.001,
            "mid-exit scale must preserve its in-flight value: {} to {}",
            entering.1,
            closing.1
        );

        press_without_settling(&app, toggle);
        let reopening = menu_values(&mut app, &samples);
        assert!(
            (reopening.0 - closing.0).abs() <= 0.01,
            "reopening at the same clock time must preserve opacity: {} to {}",
            closing.0,
            reopening.0
        );
        assert!(
            (reopening.1 - closing.1).abs() <= 0.001,
            "reopening at the same clock time must preserve scale: {} to {}",
            closing.1,
            reopening.1
        );

        for _ in 0..2 {
            app.next_frame(Duration::from_millis(16)).unwrap();
        }
        let advancing = menu_values(&mut app, &samples);
        assert!(
            advancing.0 > reopening.0,
            "reopened opacity advances toward 1: {} to {}",
            reopening.0,
            advancing.0
        );
        assert!(
            advancing.1 > reopening.1,
            "reopened scale advances toward 1: {} to {}",
            reopening.1,
            advancing.1
        );
    }

    #[test]
    fn disabling_reduced_motion_during_menu_exit_still_unmounts_the_panel() {
        let (sent, _samples) = mpsc::channel();
        let mut app = manual_app(nsh_testing::windowed_root_with::<MenuMotionProbe>(
            move || {
                let sent = sent.clone();
                MenuMotionProbeProps {
                    content: Some(Renderer::new(move |_| menu_motion_contents(sent.clone()))),
                }
            },
        ));
        app.wait_for("id:menu-motion-toggle")
            .expect("the menu control");
        app.set_environment("reduced motion", |env| env.reduce_motion = true)
            .expect("enable reduced motion");

        press_without_settling(&app, "id:menu-motion-toggle");
        app.wait_for("id:menu-motion-panel")
            .expect("the menu panel opens");
        app.wait(Duration::from_millis(motion::REDUCED_CROSSFADE_MS as u64))
            .expect("finish the menu entrance");

        press_without_settling(&app, "id:menu-motion-toggle");
        app.wait(Duration::from_millis(
            motion::REDUCED_CROSSFADE_MS as u64 / 2,
        ))
        .expect("advance partway through the reduced-motion exit");
        let mounted_mid_exit = app.screen().matches("id:menu-motion-panel").unwrap();

        app.set_environment("motion enabled", |env| env.reduce_motion = false)
            .expect("disable reduced motion");
        app.wait(Duration::from_millis(
            motion::REDUCED_CROSSFADE_MS as u64 / 2 + 20,
        ))
        .expect("advance past the original menu exit deadline");

        assert!(
            mounted_mid_exit && !app.screen().matches("id:menu-motion-panel").unwrap(),
            "the menu panel must stay mounted during its exit and unmount after it"
        );
    }

    /// The row over a document view standing in for the source editor: the
    /// menu is open and the focus is on another control, so the Escape
    /// binding must close the menu and put the focus back on the document.
    #[component]
    fn MenuEscapeFocusProbe() -> impl IntoElement {
        let state = AppState::new();
        state.new_document();
        let tab = state.active_tab().expect("the tab");
        let target = element_ref();
        state.attach_document_view(tab.id, target);
        let menu_state = state.clone();
        let other = element_ref();
        let target_focused = signal(false);
        let other_focused = signal(false);
        view()
            .flex_col()
            .w(px(600.0))
            .h(px(500.0))
            .child(
                view()
                    .test_id("menu-focus-target")
                    .element_ref(&target)
                    .focusable(true)
                    .on_focus(move || target_focused.set(true))
                    .child(prims::ink("document")),
            )
            .child(
                view()
                    .test_id("menu-focus-other")
                    .element_ref(&other)
                    .focusable(true)
                    .on_focus(move || other_focused.set(true))
                    .child(prims::ink("other")),
            )
            .child(
                view()
                    .test_id("menu-open")
                    .focusable(false)
                    .on_press(move |_: &PressEvent| {
                        other.focus();
                        menu_state.title_menu_open.set(true);
                    })
                    .child(prims::ink("open the menu")),
            )
            .child(
                text(move || {
                    format!(
                        "target: {} other: {}",
                        target_focused.get(),
                        other_focused.get()
                    )
                })
                .test_id("menu-focus-state"),
            )
            .child(title_row(state, Language::English))
    }

    #[test]
    fn escape_closes_the_menu_and_returns_focus_to_the_document() {
        let mut config = EngineConfig::new(nsh_testing::windowed_root::<MenuEscapeFocusProbe>());
        // A platform that gives ordinary views the keyboard focus, as the
        // Windows host does.
        config.environment.os = nsh_core::platform::Os::Linux;
        let mut app = Headless::start(config).expect("start the probe");
        app.set_viewport(nsh_core::Size::new(Points(600.0), Points(500.0)))
            .expect("viewport");
        app.wait_for("id:menu-open").expect("the probe renders");
        app.press("id:menu-open").expect("open the menu");
        app.wait_for("id:title-menu-open-folder")
            .expect("the menu is open");
        app.wait_for("text:target: false other: true")
            .expect("the other control holds the focus");
        app.press_key("escape").expect("close the menu");
        app.wait_for_gone("id:title-menu-open-folder", Duration::from_secs(2))
            .expect("the menu closed");
        app.wait_for("text:target: true other: true")
            .expect("the document view took the focus back");
    }

    /// The row over two documents with a Review in flight on the first: the
    /// Review entry points wait and say why.
    #[component]
    fn ReviewWaitProbe() -> impl IntoElement {
        let state = AppState::new();
        state.new_document();
        let first = state.active.get().expect("the first document");
        state.arm_review_in_flight(first);
        state.new_document();
        title_row(state, Language::English)
    }

    #[test]
    fn review_entry_points_wait_for_another_documents_review() {
        let mut app = Headless::start(EngineConfig::new(nsh_testing::windowed_root::<
            ReviewWaitProbe,
        >()))
        .expect("start the probe");
        app.set_viewport(nsh_core::Size::new(Points(600.0), Points(900.0)))
            .expect("viewport");
        app.wait_for("id:title-review").expect("the row");
        assert!(
            app.screen()
                .accessibility_state("id:title-review")
                .expect("Review's state")
                .disabled,
            "the Review action waits"
        );
        assert_eq!(
            app.screen()
                .accessibility_description("id:title-review")
                .expect("Review's description")
                .as_deref(),
            Some("Waiting for Review on another document."),
            "the Review action says why"
        );
        app.wait_for("id:title-review-wait >> text:Waiting for Review on another document.")
            .expect("the Review action shows the reason visibly");
        app.press("id:title-settings").expect("open the menu");
        app.wait_for("id:title-menu-review-selection")
            .expect("the menu's Review row");
        assert!(
            app.screen()
                .accessibility_state("id:title-menu-review-selection")
                .expect("Review selection's state")
                .disabled,
            "the menu's Review waits"
        );
        assert_eq!(
            app.screen()
                .accessibility_description("id:title-menu-review-selection")
                .expect("Review selection's description")
                .as_deref(),
            Some("Waiting for Review on another document."),
            "the menu's Review says why"
        );
        app.wait_for("id:title-menu-review-wait >> text:Waiting for Review on another document.")
            .expect("the menu's Review shows the reason visibly");
    }
}
