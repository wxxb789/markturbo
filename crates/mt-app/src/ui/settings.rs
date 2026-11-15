//! Owner lane: settings.
//!
//! The settings page: it takes the whole room (the shell shows it instead of
//! the column and the field), because configuring how the app talks to a model
//! is not something to do beside your document.
//!
//! Contract: [`is_open`] says the room is the settings'; [`page`] draws it
//! under the title row. Everything the old `settings_page` and `usage_settings`
//! wired belongs here, with the same `test_id`s the tests use: `api-key`,
//! `usage-counters`, `usage-summary`, `usage-retention`, `usage-details`.
//!
//! What is here now: the language and theme choices (both wired to the state
//! and complete), the endpoint and credential state as words (never the key),
//! and 'Close'. The settings lane adds the endpoint and base-url fields with
//! their validation, the credential controls, `Save`, and the usage counters.

use mt_core::settings::{Language, ThemePreference};
use nsh_core::prelude::*;

use crate::{
    i18n::{self, Key},
    state::AppState,
    ui::{
        prims,
        tokens::{SPACE_2, SPACE_3, SPACE_4, SPACE_5, SPACE_6},
    },
};

/// Whether the settings page is up.
pub fn is_open(state: &AppState) -> bool {
    state.settings_open.get()
}

/// The settings page, under the title row.
pub fn page(state: AppState, language: Language) -> impl IntoElement {
    let close_state = state.clone();
    let current = state.settings.get();
    view()
        .test_id("settings-page")
        .flex_1()
        .w_full()
        .flex_col()
        .pt(px(SPACE_5))
        .child(prims::ink(i18n::t(language, Key::Settings)))
        .child(
            view()
                .flex_col()
                .gap(px(SPACE_4))
                .pt(px(SPACE_5))
                .child(choice_row(
                    "settings-language",
                    i18n::t(language, Key::Language_),
                    &[
                        ("settings-language-english", "English", Language::English),
                        (
                            "settings-language-chinese",
                            "\u{7b80}\u{4f53}\u{4e2d}\u{6587}",
                            Language::Chinese,
                        ),
                    ],
                    current.language,
                    state.clone(),
                    |state, value| state.set_language(value),
                ))
                .child(choice_row(
                    "settings-theme",
                    i18n::t(language, Key::Theme),
                    &[
                        ("settings-theme-system", "System", ThemePreference::System),
                        ("settings-theme-light", "Light", ThemePreference::Light),
                        ("settings-theme-dark", "Dark", ThemePreference::Dark),
                    ],
                    current.theme,
                    state.clone(),
                    |state, value| state.set_theme(value),
                ))
                .child(
                    view()
                        .flex_col()
                        .gap(px(SPACE_2))
                        .child(prims::faint(move || {
                            SharedString::from(if current.model_base_url.is_empty() {
                                "No endpoint set"
                            } else {
                                "An endpoint is set"
                            })
                        }))
                        .child(prims::faint(
                            "Credentials are kept in Windows Credential Manager; the key is never shown.",
                        )),
                ),
        )
        .child(view().pt(px(SPACE_6)).child(prims::quiet_action(
            "settings-close",
            i18n::t(language, Key::Close),
            move || close_state.close_settings(),
        )))
}

/// A row of mutually exclusive choices: the current one is plain full-ink text
/// (it is the value, not an action), the others are quiet actions that set it.
fn choice_row<T: Copy + PartialEq + 'static>(
    test_id: &'static str,
    label: &'static str,
    choices: &[(&'static str, &'static str, T)],
    current: T,
    state: AppState,
    set: fn(&AppState, T),
) -> impl IntoElement {
    let mut row = view()
        .test_id(test_id)
        .flex_row()
        .items_center()
        .gap(px(SPACE_3))
        .child(prims::rest(label));
    for (id, name, value) in choices {
        row = row.child(if *value == current {
            prims::ink(*name).into_element()
        } else {
            let value = *value;
            let state = state.clone();
            prims::quiet_action(id, *name, move || set(&state, value)).into_element()
        });
    }
    view().flex_col().child(row)
}
