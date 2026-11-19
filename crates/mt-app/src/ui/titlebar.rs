//! Owner lane: chrome.
//!
//! The 32px opaque title row: the file's name at the left (aligned with the
//! text column), `Review` and `Settings` at the right, and the menu
//! `Settings` opens.
//!
//! Contract: [`title_row`] fills the row and owns its height and its padding.
//! The window runs with the system title bar hidden (`shell` sets it), so the
//! row reserves the native controls' area — `prims::caption_insets()` — and
//! marks itself draggable, or the window cannot be moved.
//!
//! `Review` is the row's one action: it is the document's one next step and it
//! sits at full ink. Every other command — the pane modes, Context, Save and
//! Save As among them — lives in the `Settings` menu, which also carries
//! Quit, names the shortcuts its commands have and marks the pane on screen;
//! pressing the file's name opens Context (the spec's "'Context' expands the
//! title row's file path"). The row holds nothing else, at any width.

use mt_core::settings::Language;
use nsh_core::{
    AccessibilityCurrent, AccessibilityState, Color, element::KeyOptions, keymap::When, prelude::*,
};

use crate::{
    i18n::{self, Key},
    state::{AppState, Pane},
    ui::{
        consent,
        motion::HOVER_MS,
        notices, prims, revision, shell,
        tokens::{
            self, COLUMN_GAP, HAIRLINE, RAIL_DOT, RAIL_W, SPACE_1, SPACE_2, SPACE_3, TITLE_ROW_H,
            TYPE_UI, TYPE_UI_LINE,
        },
        welcome,
    },
};

/// The menu's width: every row and its shortcut stay on one line.
const MENU_W: f32 = 240.0;

/// The title row. `state` is the shell's handle on the app; `language` is the
/// language the shell was re-keyed with.
pub fn title_row(state: AppState, language: Language) -> impl IntoElement {
    // `Review` (and the rest of `Ctrl+Enter`) from wherever the focus is.
    let review_state = state.clone();
    effect(move || bind_mod_enter(&review_state));

    let menu = signal(false);

    // The menu's document commands, reachable without it.
    let open_state = state.clone();
    on_key("mod+o", move |_: &str| {
        welcome::choose_folder(open_state.clone())
    });
    let new_state = state.clone();
    on_key("mod+n", move |_: &str| new_state.new_document());
    // Save; an untitled document goes to Save As. The source editor binds no
    // `mod+s`, so the default `when` fires it from the editor too.
    let save_state = state.clone();
    on_key("mod+s", move |_: &str| save_state.save_active());
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
        // No document, no name (the welcome): the row holds `Settings` alone.
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

/// Binds `Ctrl+Enter` (`mod+enter`; on an Apple keyboard `ctrl+enter` too, the
/// key the hints name) to [`mod_enter`], the app's one handler for it. Call it
/// inside an effect: the binding is dropped with the effect's run, and it
/// reads whether a consent card is up, whose hold takes the key's
/// auto-repeats (anything else fires once per press).
///
/// The keymap fires the newest binding of a keystroke, and the source editor
/// binds its own `mod+enter` (insert a line below) every time it takes the
/// focus. So besides the title row, the source editor binds this again on
/// each of its `Focus` events (`document`), and the surfaces that
/// claim the key (the consent card, the Revision field's `Done`) bind it as
/// they come — every one of them the same binding.
pub fn bind_mod_enter(state: &AppState) {
    let held = consent::is_open(state);
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
                repeat: held,
                ..KeyOptions::default()
            },
            move |_: &str| mod_enter(&state),
        );
    }
}

/// `Ctrl+Enter`: the consent card's hold while one is up, `Done` while a
/// fresh proposal is docked, else `Review`.
fn mod_enter(state: &AppState) {
    if consent::is_open(state) {
        consent::key_hold();
        return;
    }
    // A card this press opens must not read the same key's repeats as a
    // hold: it arms only once the key was let go.
    consent::note_key_press();
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
    {
        state.start_review();
    }
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
        // The name gives way first: `Review` and `Settings` keep their room at
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
        row_action("title-review", review, true, move || start.start_review()).into_element()
    })
}

/// `Settings`, the word that opens the row's menu: quieter than `Review`,
/// and it tells screen readers whether its menu is open.
fn menu_anchor(state: AppState, language: Language, menu: Signal<bool>) -> impl IntoElement {
    let panel_state = state;
    view()
        .test_id("title-menu-anchor")
        .relative()
        .child(
            row_action(
                "title-settings",
                i18n::t(language, Key::Settings),
                false,
                move || {
                    menu.update(|open| *open = !*open);
                },
            )
            .accessibility_state(move || AccessibilityState {
                expanded: Some(menu.get()),
                has_popup: true,
                ..AccessibilityState::default()
            }),
        )
        .child(when(move || menu.get(), {
            move || menu_panel(panel_state.clone(), language, menu).into_element()
        }))
}

/// The row's menu, under `Settings`: the document's actions with their
/// shortcuts, the panes with the one on screen marked, Context, the settings
/// page, and Quit.
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
        .w(px(MENU_W))
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
                Some(shortcut(false, "O")),
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
            menu_row(
                "title-menu-new-file",
                s(S::NewFile, language),
                Some(shortcut(false, "N")),
                move || {
                    menu.set(false);
                    new_state.new_document();
                },
            )
        })
        .child({
            let paste_state = state.clone();
            menu_row(
                "title-menu-paste",
                s(S::PasteAsNew, language),
                None,
                move || {
                    menu.set(false);
                    paste_state.new_from_clipboard();
                },
            )
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
                let row = |test_id: &'static str,
                           label: &'static str,
                           hint: Option<SharedString>,
                           run: fn(&AppState)| {
                    let state = state.clone();
                    menu_row(test_id, label, hint, move || {
                        menu.set(false);
                        run(&state);
                    })
                    .into_element()
                };
                // A pane's row: the pane on screen carries the mark.
                let pane = |test_id: &'static str, label: &'static str, pane: Pane| {
                    pane_row(&state, menu, test_id, label, pane)
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
                    .child(row(
                        "title-menu-review-selection",
                        s(S::ReviewSelection, language),
                        None,
                        |state| state.start_review_selection(),
                    ))
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
    menu_row(test_id, label, None, move || {
        menu.set(false);
        run(&state);
    })
}

/// A pane's row: it shows the active document in `pane`. The pane on screen
/// carries the mark — the rail's dot, where a shortcut would be — and is the
/// current item for screen readers.
fn pane_row(
    state: &AppState,
    menu: Signal<bool>,
    test_id: &'static str,
    label: &'static str,
    pane: Pane,
) -> AnyElement {
    let on_screen = {
        let state = state.clone();
        move || state.active_tab().is_some_and(|tab| tab.pane.get() == pane)
    };
    let current = on_screen.clone();
    let state = state.clone();
    menu_row(test_id, label, None, move || {
        menu.set(false);
        if let Some(tab) = state.active_tab() {
            state.set_pane(tab.id, pane);
        }
    })
    .accessibility_state(move || AccessibilityState {
        current: current().then_some(AccessibilityCurrent::True),
        ..AccessibilityState::default()
    })
    .child(when(on_screen, || {
        view()
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
fn shortcut(shift: bool, key: &str) -> SharedString {
    SharedString::from(match (apple_keys(), shift) {
        (true, false) => format!("\u{2318}{key}"),
        (true, true) => format!("\u{21E7}\u{2318}{key}"),
        (false, false) => format!("Ctrl+{key}"),
        (false, true) => format!("Ctrl+Shift+{key}"),
    })
}

/// A caption's ink: quieter than a row's label and still text contrast —
/// body ink's hue at .54 (dark) / .66 (light).
fn caption_ink() -> Color {
    let palette = tokens::palette();
    palette.ink_at(if palette.is_dark { 0.54 } else { 0.66 })
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

/// One row of the menu, on one line: its label at quiet ink (full ink and
/// underlined on hover or keyboard focus), then the shortcut it has, if any,
/// right-aligned in caption ink. The label's text draws the hover itself: a
/// style on the row would not reach it.
fn menu_row(
    test_id: &'static str,
    label: &'static str,
    hint: Option<SharedString>,
    on_press: impl Fn() + 'static,
) -> View {
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
        .on_press(on_press)
        .child(
            text(label)
                .font_family(tokens::FONT_UI)
                .text_size(px(TYPE_UI))
                .line_height(px(TYPE_UI_LINE))
                .text_color(move || tokens::palette().ink_rest)
                .flex_1()
                .min_w(px(0.0))
                .truncate()
                .transition_colors()
                .duration(HOVER_MS)
                .group_hover(|s| s.text_color(tokens::palette().ink).underline())
                .group_focus_visible(|s| s.text_color(tokens::palette().ink).underline()),
        )
        .children(hint.map(|hint| {
            text(hint)
                .font_family(tokens::FONT_UI)
                .text_size(px(TYPE_UI))
                .line_height(px(TYPE_UI_LINE))
                .text_color(caption_ink)
                .flex_shrink_0()
                .into_element()
        }))
}

/// A title-row action, named as it reads. `strong` is `Review`, the app's one
/// plain emphasis: the medium face at body ink over a 34% underline (the
/// prototype's `.act`), the underline stepping to full ink on hover or
/// keyboard focus. Otherwise (`Settings`) resting ink that steps to full ink,
/// underlined, on hover or focus. No heat: it is local work, lit by ambient
/// ink. The underline is the text's own: a decoration on the view around it
/// would not reach the glyphs.
fn row_action(
    test_id: &'static str,
    label: &'static str,
    strong: bool,
    on_press: impl Fn() + 'static,
) -> View {
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
        .duration(HOVER_MS)
        .group_hover(|s| {
            s.text_color(tokens::palette().ink)
                .underline()
                .text_decoration_color(tokens::palette().ink)
        })
        .group_focus_visible(|s| {
            s.text_color(tokens::palette().ink)
                .underline()
                .text_decoration_color(tokens::palette().ink)
        });
    if strong {
        label_text = label_text
            .underline()
            .text_decoration_color(move || tokens::palette().ink_at(0.34));
    }
    // A plain press target, not `nsh_base`'s button: the control itself
    // carries the test id, so a test presses `id:` and lands on the view that
    // hears the press.
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
        .on_press(on_press)
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
