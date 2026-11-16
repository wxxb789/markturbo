//! Owner lane: chrome.
//!
//! The 32px opaque title row: the file's name at the left (aligned with the
//! text column), `Review` and the settings glyph at the right, and the menu
//! the glyph opens.
//!
//! Contract: [`title_row`] fills the row and owns its height and its padding.
//! The window runs with the system title bar hidden (`shell` sets it), so the
//! row reserves the native controls' area — `prims::caption_insets()` — and
//! marks itself draggable, or the window cannot be moved.
//!
//! `Review` is the row's one action: it is the document's one next step and it
//! sits at full ink. Every other command — the pane modes, Context, Save and
//! Save As among them — lives in the settings glyph's menu, which also carries
//! Quit, and pressing the file's name opens Context (the spec's "'Context'
//! expands the title row's file path"). The row holds nothing else, at any
//! width.

use mt_core::settings::Language;
use nsh_core::{Color, element::KeyOptions, keymap::When, prelude::*};

use crate::{
    i18n::{self, Key},
    state::{AppState, Pane},
    ui::{
        consent,
        motion::HOVER_MS,
        notices, prims, shell,
        tokens::{
            self, COLUMN_GAP, HAIRLINE, RAIL_W, SPACE_1, SPACE_2, SPACE_3, TITLE_ROW_H, TYPE_UI,
            TYPE_UI_LINE,
        },
        welcome,
    },
};

/// The title row. `state` is the shell's handle on the app; `language` is the
/// language the shell was re-keyed with.
pub fn title_row(state: AppState, language: Language) -> impl IntoElement {
    // `Review` is reachable from the editor itself: `enter` is a key a text
    // input uses, so the binding asks for the focus wherever it is.
    let review_state = state.clone();
    on_key_with(
        "mod+enter",
        KeyOptions {
            when: When::Always,
            ..KeyOptions::default()
        },
        move |_: &str| {
            // A revision on screen is a decision waiting: reviewing over it
            // would drop the proposal being decided. While a consent card is
            // up, Ctrl+Enter is its hold-to-send key; re-preparing the review
            // there would rebuild the card under the user's hold. A review in
            // flight or a guard decision is not restarted either.
            if review_state.active.get().is_some()
                && review_state.revision_hunks.get().is_empty()
                && !consent::is_open(&review_state)
                && !notices::guard_is_open(&review_state)
                && !review_state.review_busy.get()
            {
                review_state.start_review();
            }
        },
    );

    let menu = signal(false);

    // The menu's document commands, reachable without it.
    let open_state = state.clone();
    on_key("mod+o", move |_: &str| {
        welcome::choose_folder(open_state.clone())
    });
    let new_state = state.clone();
    on_key("mod+n", move |_: &str| new_state.new_document());
    let save_as_state = state.clone();
    on_key("shift+mod+s", move |_: &str| {
        if save_as_state.active.get().is_some() {
            save_as_state.save_as_active();
        }
    });
    // Escape closes the menu from anywhere. The binding passes the key on, so
    // it takes nothing from the surfaces that bind Escape themselves.
    on_key_with(
        "escape",
        KeyOptions {
            when: When::Always,
            pass_through: true,
            ..KeyOptions::default()
        },
        move |_: &str| {
            if menu.get() {
                menu.set(false);
            }
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
        .child(when(move || menu.get(), {
            move || menu_scrim(menu).into_element()
        }))
        // No document, no name (the welcome): the row holds the glyph alone.
        .child(when(
            {
                let state = state.clone();
                move || state.active.get().is_some()
            },
            {
                let state = state.clone();
                move || file_name(state.clone()).into_element()
            },
        ))
        .child(view().flex_1())
        .child(review_action(state.clone(), language))
        .child(menu_anchor(state.clone(), language, menu))
}

/// The file's name, with its folder at non-text ink ahead of it so two files
/// of one name are told apart, and a mark while the document is not saved.
/// Pressing it opens (and closes) Context: the path the name stands for is
/// the order Codex applies its instructions in.
fn file_name(state: AppState) -> impl IntoElement {
    let folder_state = state.clone();
    let name_state = state.clone();
    let label_state = state.clone();
    let dirty_state = state.clone();
    view()
        .test_id("title-file")
        .role(AccessibilityRole::Button)
        .optional_label(move || label_state.active_tab().map(|tab| tab.title.clone()))
        .window_drag(false)
        .cursor_pointer()
        .bg(Color::TRANSPARENT)
        .flex_row()
        .items_center()
        .gap(px(SPACE_2))
        // The name gives way first: `Review` and the glyph keep their room at
        // any width, and a long name truncates instead of running under them.
        .flex_shrink(1.0)
        .min_w(px(0.0))
        .transition_colors()
        .duration(HOVER_MS)
        .hover(|s| s.text_color(tokens::palette().ink).underline())
        .focus_visible(|s| s.text_color(tokens::palette().ink).underline())
        .on_press(move || state.toggle_context())
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
            .text_color(move || tokens::palette().ink_faint),
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

/// `Review`, the document's one next step, while there is a document and no
/// revision waiting on a decision.
fn review_action(state: AppState, language: Language) -> impl IntoElement {
    let can_review = {
        let state = state.clone();
        move || state.active.get().is_some() && state.revision_hunks.get().is_empty()
    };
    when(can_review, move || {
        let start = state.clone();
        let review = i18n::t(language, Key::Review);
        row_action("title-review", review, review, move || start.start_review()).into_element()
    })
}

/// The glyph that opens the row's menu: the room's one icon.
fn menu_anchor(state: AppState, language: Language, menu: Signal<bool>) -> impl IntoElement {
    let panel_state = state;
    view()
        .test_id("title-menu-anchor")
        .relative()
        // The room's one icon; screen readers (and tests) name it 'Settings'.
        .child(row_action(
            "title-settings",
            "\u{2699}",
            i18n::t(language, Key::Settings),
            move || {
                menu.update(|open| *open = !*open);
            },
        ))
        .child(when(move || menu.get(), {
            move || menu_panel(panel_state.clone(), language, menu).into_element()
        }))
}

/// The row's menu, under the glyph: the document's actions, Context, the
/// settings page, and Quit.
fn menu_panel(state: AppState, language: Language, menu: Signal<bool>) -> impl IntoElement {
    let document = {
        let state = state.clone();
        move || state.active.get().is_some()
    };
    view()
        .test_id("title-menu")
        .absolute()
        .top(px(TITLE_ROW_H))
        .right(px(0.0))
        .w(px(200.0))
        .flex_col()
        .py(px(SPACE_2))
        .bg(move || tokens::palette().room)
        .border(px(HAIRLINE))
        .border_color(move || tokens::palette().hairline)
        // A press anywhere opens the workspace it names, with or without a
        // document on screen.
        .child({
            let open_state = state.clone();
            // The keyboard comes to the first row as the menu appears. A
            // platform that focuses only inputs leaves it where it was.
            let first = element_ref();
            effect(move || first.focus());
            menu_row(
                "title-menu-open-folder",
                s(S::OpenFolder, language),
                move || {
                    menu.set(false);
                    welcome::choose_folder(open_state.clone());
                },
            )
            .element_ref(&first)
        })
        // The welcome's two ways to start a document, wherever the menu is
        // opened from: the new document becomes the active tab.
        .child({
            let new_state = state.clone();
            menu_row("title-menu-new-file", s(S::NewFile, language), move || {
                menu.set(false);
                new_state.new_document();
            })
        })
        .child({
            let paste_state = state.clone();
            menu_row("title-menu-paste", s(S::PasteAsNew, language), move || {
                menu.set(false);
                paste_state.new_from_clipboard();
            })
        })
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
                // Every row closes the menu first, then does its one thing.
                let row = |test_id: &'static str, label: &'static str, run: fn(&AppState)| {
                    let state = state.clone();
                    menu_row(test_id, label, move || {
                        menu.set(false);
                        run(&state);
                    })
                };
                view()
                    .child(
                        row("title-menu-save", i18n::t(language, Key::Save), |state| {
                            state.save_active();
                        })
                        .into_element(),
                    )
                    .child(
                        row("title-menu-save-as", s(S::SaveAs, language), |state| {
                            state.save_as_active()
                        })
                        .into_element(),
                    )
                    .child(
                        row(
                            "title-menu-edit",
                            i18n::t(language, Key::ModeSource),
                            |state| {
                                if let Some(tab) = state.active_tab() {
                                    state.set_pane(tab.id, Pane::Source);
                                }
                            },
                        )
                        .into_element(),
                    )
                    .child(
                        row(
                            "title-menu-preview",
                            i18n::t(language, Key::ModeNative),
                            |state| {
                                if let Some(tab) = state.active_tab() {
                                    state.set_pane(tab.id, Pane::Preview);
                                }
                            },
                        )
                        .into_element(),
                    )
                    .child(
                        row("title-menu-split", s(S::Split, language), |state| {
                            if let Some(tab) = state.active_tab() {
                                state.set_pane(tab.id, Pane::Split);
                            }
                        })
                        .into_element(),
                    )
                    .child(
                        row("title-menu-web", s(S::WebPreview, language), |state| {
                            if let Some(tab) = state.active_tab() {
                                state.set_pane(tab.id, Pane::Web);
                            }
                        })
                        .into_element(),
                    )
                    .child(
                        row("title-menu-split-web", s(S::SplitWeb, language), |state| {
                            if let Some(tab) = state.active_tab() {
                                state.set_pane(tab.id, Pane::SplitWeb);
                            }
                        })
                        .into_element(),
                    )
                    .child(
                        row(
                            "title-menu-translate",
                            i18n::t(language, Key::Translate),
                            |state| state.translate_active(),
                        )
                        .into_element(),
                    )
                    .child(
                        row(
                            "title-menu-review-selection",
                            s(S::ReviewSelection, language),
                            |state| state.start_review_selection(),
                        )
                        .into_element(),
                    )
                    .into_element()
            }
        }))
        .child(menu_item(
            &state,
            menu,
            "title-menu-context",
            s(S::Context, language),
            |state| state.toggle_context(),
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

/// One menu row, wired to `run`: it closes the menu first.
fn menu_item(
    state: &AppState,
    menu: Signal<bool>,
    test_id: &'static str,
    label: &'static str,
    run: fn(&AppState),
) -> impl IntoElement {
    let state = state.clone();
    menu_row(test_id, label, move || {
        menu.set(false);
        run(&state);
    })
}

/// The click-catcher under the menu: a press anywhere else closes it. It is
/// the row's first child, so what the row holds stays pressable above it.
fn menu_scrim(menu: Signal<bool>) -> impl IntoElement {
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
        .on_press(move || menu.set(false))
}

/// One row of the menu: quiet ink, full ink on hover.
fn menu_row(
    test_id: &'static str,
    label: &'static str,
    on_press: impl Fn() + 'static,
) -> impl Interactive + ParentElement {
    view()
        .test_id(test_id)
        .w_full()
        .role(AccessibilityRole::Button)
        .optional_label(Some(SharedString::from(label)))
        .window_drag(false)
        .cursor_pointer()
        .bg(Color::TRANSPARENT)
        .px(px(SPACE_3))
        .py(px(SPACE_2))
        .transition_colors()
        .duration(HOVER_MS)
        .hover(|s| s.text_color(tokens::palette().ink).underline())
        .focus_visible(|s| s.text_color(tokens::palette().ink).underline())
        .on_press(on_press)
        .child(
            text(label)
                .font_family(tokens::FONT_UI)
                .text_size(px(TYPE_UI))
                .line_height(px(TYPE_UI_LINE))
                .text_color(move || tokens::palette().ink_rest),
        )
}

/// A title-row action at the app's one plain emphasis — body ink over a 34%
/// underline (the prototype's `.act`) — the underline stepping to full ink on
/// hover, and the designed ring when it has the keyboard focus. No heat: it
/// is local work, lit by ambient ink.
fn row_action(
    test_id: &'static str,
    label: impl Into<SharedString>,
    accessible_label: &'static str,
    on_press: impl Fn() + 'static,
) -> impl Interactive + ParentElement {
    let label: SharedString = label.into();
    // A plain press target, not `nsh_base`'s button: the control itself
    // carries the test id, so a test presses `id:` and lands on the view that
    // hears the press.
    view()
        .test_id(test_id)
        .role(AccessibilityRole::Button)
        .optional_label(Some(SharedString::from(accessible_label)))
        .window_drag(false)
        .cursor_pointer()
        .bg(Color::TRANSPARENT)
        .rounded(px(0.0))
        .px(px(SPACE_1))
        .py(px(SPACE_1))
        .underline()
        .underline_offset(px(4.0))
        .text_decoration_color(move || tokens::palette().ink_at(0.34))
        .transition_colors()
        .duration(HOVER_MS)
        .hover(|s| {
            s.text_color(tokens::palette().ink)
                .text_decoration_color(tokens::palette().ink)
        })
        .focus_visible(|s| {
            s.text_color(tokens::palette().ink)
                .text_decoration_color(tokens::palette().ink)
        })
        .on_press(on_press)
        .child(
            text(label)
                .font_family(tokens::FONT_UI)
                .text_size(px(TYPE_UI))
                .line_height(px(TYPE_UI_LINE))
                .text_color(move || tokens::palette().ink),
        )
}

/// The title row's copy. New strings live here rather than in `i18n.rs` (a
/// merge hotspot): the match is exhaustive, so a missing translation is a
/// compile error.
#[derive(Clone, Copy)]
enum S {
    OpenFolder,
    NewFile,
    PasteAsNew,
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
            S::SaveAs => "Save As\u{2026}",
            S::Split => "Split",
            S::WebPreview => "Web preview",
            S::SplitWeb => "Split \u{00B7} Web",
            S::Context => "Context",
            S::ReviewSelection => "Review selection",
        },
        Language::Chinese => match key {
            S::OpenFolder => "打开文件夹\u{2026}",
            S::NewFile => "新建文件",
            S::PasteAsNew => "粘贴为新文档",
            S::SaveAs => "另存为\u{2026}",
            S::Split => "分栏",
            S::WebPreview => "Web 预览",
            S::SplitWeb => "分栏 \u{00B7} Web",
            S::Context => "上下文",
            S::ReviewSelection => "审查选区",
        },
    }
}
