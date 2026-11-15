//! Owner lane: chrome.
//!
//! The 32px opaque title row: the file's name at the left (aligned with the
//! text column), the document's actions at the right, and the menu the
//! settings glyph opens.
//!
//! Contract: [`title_row`] fills the row and owns its height and its padding.
//! The window runs with the system title bar hidden (`shell` sets it), so the
//! row reserves the native controls' area — `prims::caption_insets()` — and
//! marks itself draggable, or the window cannot be moved.
//!
//! Only `Review` is an emphatic action: it is the document's one next step and
//! it sits at full ink, the shortcuts' hint beside it. Everything else is a
//! quiet text action — local work is lit by ambient ink, never by heat — and
//! the actions a narrow window cannot hold move into the settings glyph's
//! menu, which also carries Quit.

use std::rc::Rc;

use mt_core::settings::Language;
use nsh_core::{Color, element::KeyOptions, keymap::When, prelude::*};

use crate::{
    i18n::{self, Key},
    state::{AppState, Pane},
    ui::{
        motion::HOVER_MS,
        prims, shell,
        tokens::{
            self, COLUMN_GAP, FONT_SOURCE, HAIRLINE, Layout, RAIL_W, SPACE_1, SPACE_2, SPACE_3,
            TITLE_ROW_H, TYPE_UI, TYPE_UI_LINE,
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
            // would drop the proposal being decided.
            if review_state.active.get().is_some() && review_state.revision_hunks.get().is_empty() {
                review_state.start_review();
            }
        },
    );

    let insets = prims::caption_insets();
    let height = TITLE_ROW_H.max(insets.top.0);
    let layout = Layout::of(shell::client_width_now());
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
        .h(px(height))
        .w_full()
        .flex_row()
        .items_center()
        .gap(px(SPACE_3))
        .pl(px((RAIL_W + COLUMN_GAP).max(insets.left.0 + SPACE_3)))
        .pr(px(insets.right.0 + SPACE_3))
        .bg(move || tokens::palette().room)
        .window_drag(true)
        .child(when(move || menu.get(), {
            move || menu_scrim(menu).into_element()
        }))
        .child(file_name(state.clone()))
        .child(view().flex_1())
        .child(actions(state.clone(), language, layout, menu))
        .child(menu_anchor(state.clone(), language, menu))
}

/// The file's name, with its folder at non-text ink ahead of it so two files
/// of one name are told apart, and a mark while the document is not saved.
fn file_name(state: AppState) -> impl IntoElement {
    let folder_state = state.clone();
    let name_state = state.clone();
    let dirty_state = state;
    view()
        .test_id("title-file")
        .flex_row()
        .items_center()
        .gap(px(SPACE_2))
        .min_w(px(0.0))
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

/// The document's actions: Save, the three modes, Context and Review, what the
/// width holds of them.
fn actions(
    state: AppState,
    language: Language,
    layout: Layout,
    menu: Signal<bool>,
) -> impl IntoElement {
    let has_document: Rc<dyn Fn() -> bool> = {
        let state = state.clone();
        Rc::new(move || state.active.get().is_some())
    };
    let can_review: Rc<dyn Fn() -> bool> = {
        let state = state.clone();
        Rc::new(move || state.active.get().is_some() && state.revision_hunks.get().is_empty())
    };
    let roomy: Rc<dyn Fn() -> bool> = {
        let has_document = has_document.clone();
        Rc::new(move || has_document() && layout != Layout::Narrow)
    };
    let wide: Rc<dyn Fn() -> bool> = {
        let has_document = has_document.clone();
        Rc::new(move || has_document() && layout == Layout::Wide)
    };
    view()
        .test_id("title-actions")
        .flex_row()
        .items_center()
        .gap(px(SPACE_3))
        .child(when(
            {
                let wide = wide.clone();
                move || wide()
            },
            {
                let state = state.clone();
                move || {
                    let state = state.clone();
                    row_action(
                        "title-save",
                        i18n::t(language, Key::Save),
                        Rc::new(|| Look::Quiet),
                        move || state.save_active(),
                    )
                    .into_element()
                }
            },
        ))
        .child(mode_action(
            state.clone(),
            Pane::Source,
            i18n::t(language, Key::ModeSource),
            "title-edit",
            roomy.clone(),
            menu,
        ))
        .child(mode_action(
            state.clone(),
            Pane::Preview,
            i18n::t(language, Key::ModeNative),
            "title-preview",
            roomy.clone(),
            menu,
        ))
        .child(mode_action(
            state.clone(),
            Pane::Split,
            s(S::Split, language),
            "title-split",
            roomy.clone(),
            menu,
        ))
        .child(when(
            {
                let roomy = roomy.clone();
                move || roomy()
            },
            {
                let state = state.clone();
                move || {
                    let toggle = state.clone();
                    row_action(
                        "title-context",
                        s(S::Context, language),
                        Rc::new(|| Look::Quiet),
                        move || toggle.toggle_context(),
                    )
                    .into_element()
                }
            },
        ))
        .child(when(
            {
                let can_review = can_review.clone();
                move || can_review()
            },
            {
                let state = state.clone();
                move || {
                    let start = state.clone();
                    row_action(
                        "title-review",
                        i18n::t(language, Key::Review),
                        Rc::new(|| Look::Ink),
                        move || start.start_review(),
                    )
                    .into_element()
                }
            },
        ))
        .child(when(
            {
                let can_review = can_review.clone();
                move || can_review() && layout == Layout::Wide
            },
            move || {
                view()
                    .test_id("title-review-kbd")
                    .child(key_hint(&shortcut("mod+enter")))
                    .into_element()
            },
        ))
}

/// One of the three modes: the one in effect sits at full ink, the others
/// rest. A narrow window folds them into the menu.
fn mode_action(
    state: AppState,
    pane: Pane,
    label: &'static str,
    test_id: &'static str,
    show: Rc<dyn Fn() -> bool>,
    menu: Signal<bool>,
) -> AnyElement {
    let current: Rc<dyn Fn() -> bool> = {
        let state = state.clone();
        Rc::new(move || state.active_tab().map(|tab| tab.pane.get()) == Some(pane))
    };
    when(
        move || show(),
        move || {
            let state = state.clone();
            let current = current.clone();
            row_action(
                test_id,
                label,
                Rc::new(move || {
                    if current() {
                        Look::Current
                    } else {
                        Look::Quiet
                    }
                }),
                move || {
                    if let Some(tab) = state.active_tab() {
                        state.set_pane(tab.id, pane);
                    }
                    menu.set(false);
                },
            )
            .into_element()
        },
    )
    .into_element()
}

/// The glyph that opens the row's menu: the room's one icon.
fn menu_anchor(state: AppState, language: Language, menu: Signal<bool>) -> impl IntoElement {
    let panel_state = state;
    view()
        .test_id("title-menu-anchor")
        .relative()
        .child(row_action(
            "title-settings",
            "\u{2699}",
            Rc::new(|| Look::Ink),
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

/// How a title-row action reads at rest. [`Look::Ink`] is the app's one plain
/// emphasis — body ink over a 34% underline (the prototype's `.act`), for the
/// state's one next action; [`Look::Current`] is body ink without the
/// underline, for the mode in effect; [`Look::Quiet`] rests at resting ink.
/// No heat: it is local work, lit by ambient ink.
#[derive(Clone, Copy)]
enum Look {
    Ink,
    Current,
    Quiet,
}

/// A title-row action: ink or resting ink at rest, the drawn underline
/// stepping to full ink with the text on hover, and the designed ring when it
/// has the keyboard focus.
fn row_action(
    test_id: &'static str,
    label: impl Into<SharedString>,
    look: Rc<dyn Fn() -> Look>,
    on_press: impl Fn() + 'static,
) -> impl Interactive + ParentElement {
    let label: SharedString = label.into();
    let text_look = look.clone();
    let decoration_look = look.clone();
    // A plain press target, not `nsh_base`'s button: the control itself
    // carries the test id, so a test presses `id:` and lands on the view that
    // hears the press.
    view()
        .test_id(test_id)
        .role(AccessibilityRole::Button)
        .optional_label(Some(label.clone()))
        .window_drag(false)
        .cursor_pointer()
        .bg(Color::TRANSPARENT)
        .rounded(px(0.0))
        .px(px(SPACE_1))
        .py(px(SPACE_1))
        .underline()
        .underline_offset(px(4.0))
        .text_decoration_color(move || match decoration_look() {
            Look::Ink => tokens::palette().ink_at(0.34),
            Look::Current | Look::Quiet => tokens::with_alpha(tokens::palette().ink, 0.0),
        })
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
                .text_color(move || match text_look() {
                    Look::Ink => tokens::palette().ink,
                    Look::Current => tokens::palette().ink,
                    Look::Quiet => tokens::palette().ink_rest,
                }),
        )
}

/// A keystroke hint with a label this module owns (`prims::kbd` takes a
/// `&'static str`; a platform-dependent one is not static).
fn key_hint(label: &str) -> impl IntoElement {
    view()
        .px(px(SPACE_1))
        .border(px(HAIRLINE))
        .border_color(move || tokens::palette().ink_faint)
        .bg(Color::TRANSPARENT)
        .child(
            text(SharedString::from(label.to_owned()))
                .font_family(FONT_SOURCE)
                .text_size(px(11.0))
                .line_height(px(16.0))
                .text_color(move || tokens::palette().ink),
        )
}

/// A keystroke as a hint shows it: `Ctrl+Enter` here, `⌘↩` on Apple's
/// platforms.
fn shortcut(keys: &str) -> String {
    let apple = nsh_core::element::apple_keys();
    nsh_core::keymap::Keystroke::parse_with(keys, apple)
        .map(|keystroke| keystroke.label(nsh_core::element::os(), apple))
        .unwrap_or_else(|_| keys.to_owned())
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
        },
    }
}
