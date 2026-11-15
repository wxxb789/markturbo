//! Owner lane: settings.
//!
//! The settings page: it takes the whole room under the title row (the shell
//! shows it instead of the column and the field), because configuring how the
//! app talks to a model is not something to do beside your document.
//!
//! Contract: [`is_open`] says the room is the settings'; [`page`] draws it
//! under the title row. Everything the old `settings_page` and `usage_settings`
//! wired lives here, with the same `test_id`s the tests use: `api-key`,
//! `usage-counters`, `usage-summary`, `usage-retention`, `usage-details`.
//!
//! The page obeys the Afterglow rule: nothing here emits light. A credential is
//! shown as words (`Key saved in Windows Credential Manager`, `No key saved`),
//! never as the key; a base URL is validated as it is typed, and the error is
//! the line under the field.
//!
//! Reactions: the page is rebuilt when the language or the layout changes
//! (`app.rs` re-keys the shell), so its labels are read once. Everything that
//! must follow the state itself — the choices, the validation line, the usage
//! counts, the credential status — reads its signal inside the binding that
//! draws it.

use mt_core::{
    model::{EndpointIdentity, EndpointIdentityError, Provider},
    settings::{Language, SettingsData, ThemePreference},
};
use nsh_base::{button, input};
use nsh_core::{Color, prelude::*};

use crate::{
    i18n::{self, Key},
    state::{AppState, CredentialStatus},
    ui::{
        motion, notices, prims,
        tokens::{
            self, COLUMN_GAP, FONT_UI, HAIRLINE, MEASURE_WIDE, RAIL_W, SPACE_2, SPACE_3, SPACE_4,
            SPACE_5, TYPE_PREVIEW, TYPE_PREVIEW_LINE, TYPE_UI, TYPE_UI_LINE, WEIGHT_MEDIUM,
        },
    },
};

/// The form's field width: the measure is for reading prose, and a form reads
/// better narrower.
const FIELD_W: f32 = 560.0;

/// Whether the settings page is up.
pub fn is_open(state: &AppState) -> bool {
    state.settings_open.get()
}

/// The settings page, under the title row: the header, the form, and the app's
/// own words at the foot.
pub fn page(state: AppState, language: Language) -> impl IntoElement {
    let close_state = state.clone();
    let save_state = state.clone();
    view()
        .test_id("settings-page")
        .flex_1()
        .min_h(px(0.0))
        .w_full()
        .flex_col()
        .pl(px(RAIL_W + COLUMN_GAP))
        .pr(px(SPACE_5))
        .pt(px(SPACE_4))
        .pb(px(SPACE_4))
        .child(
            view()
                .test_id("settings-header")
                .w_full()
                .max_w(px(MEASURE_WIDE))
                .flex_row()
                .items_center()
                .justify_between()
                .child(heading(i18n::t(language, Key::Settings)))
                .child(
                    view()
                        .flex_row()
                        .items_center()
                        .gap(px(SPACE_3))
                        .child(prims::primary_action(
                            "settings-save",
                            i18n::t(language, Key::SaveSettings),
                            move || save_state.save_settings(),
                        ))
                        .child(prims::quiet_action(
                            "settings-close",
                            i18n::t(language, Key::Close),
                            move || close_state.close_settings(),
                        )),
                ),
        )
        .child(view().pt(px(SPACE_3)).child(prims::hairline()))
        .child(
            scroll()
                .flex_1()
                .min_h(px(0.0))
                .w_full()
                .flex_col()
                .pt(px(SPACE_3))
                .child(content(state.clone(), language)),
        )
        .child(notice_line(state.clone()))
        .child(notices::status_line(state, language))
}

/// The page's one heading: 16px medium ink — 22px is the model's voice alone.
fn heading(content: &'static str) -> impl IntoElement {
    text(content)
        .font_family(FONT_UI)
        .font_weight(WEIGHT_MEDIUM)
        .text_size(px(TYPE_PREVIEW))
        .line_height(px(TYPE_PREVIEW_LINE))
        .text_color(move || tokens::palette().ink)
}

/// The line above the status line: the last credential, usage or store outcome.
/// It is absent until there is something to say, so an untouched page keeps its
/// calm.
fn notice_line(state: AppState) -> impl IntoElement {
    let condition_state = state.clone();
    view().test_id("settings-notice").child(when(
        move || !condition_state.settings_notice.get().is_empty(),
        move || {
            view()
                .pt(px(SPACE_3))
                .child(
                    text(move || state.settings_notice.get())
                        .font_family(FONT_UI)
                        .text_size(px(TYPE_UI))
                        .line_height(px(TYPE_UI_LINE))
                        .text_color(move || tokens::palette().ink_rest),
                )
                .into_element()
        },
    ))
}

/// The form: one group per decision, in the order they matter.
fn content(state: AppState, language: Language) -> impl IntoElement {
    view()
        .test_id("settings-content")
        .w_full()
        .max_w(px(MEASURE_WIDE))
        .flex_col()
        .gap(px(SPACE_4))
        .child(endpoint_section(state.clone(), language))
        .child(credential_section(state.clone(), language))
        .child(translation_section(state.clone(), language))
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
            |settings: &SettingsData| settings.language,
            state.clone(),
            |state, value| state.set_language(value),
        ))
        .child(theme_row(state.clone(), language))
        .child(usage_section(state, language))
}

/// One labelled group: its label in resting ink, its rows 16px beneath.
fn section(label: &'static str, rows: impl IntoElement) -> impl IntoElement {
    view()
        .flex_col()
        .gap(px(SPACE_3))
        .child(prims::rest(label))
        .child(rows)
}

/// One labelled field: its visible label, the field itself (an input over a
/// hairline) and the line under it — the validation error while invalid, the
/// hint otherwise.
fn field_row(
    label: &'static str,
    field: impl IntoElement,
    note: impl IntoElement,
) -> impl IntoElement {
    view()
        .flex_col()
        .gap(px(SPACE_2))
        .child(prims::rest(label))
        .child(view().max_w(px(FIELD_W)).child(field))
        .child(view().max_w(px(FIELD_W)).child(note))
}

/// One labelled field with nothing under it: an empty note would still cost
/// the gap above it.
fn field_row_bare(label: &'static str, field: impl IntoElement) -> impl IntoElement {
    view()
        .flex_col()
        .gap(px(SPACE_2))
        .child(prims::rest(label))
        .child(view().max_w(px(FIELD_W)).child(field))
}

/// The model endpoint: the wire format, the base URL with its validation line,
/// and the model id.
fn endpoint_section(state: AppState, language: Language) -> impl IntoElement {
    let base_change_state = state.clone();
    let model_change_state = state.clone();
    let snapshot = state.settings.get();
    section(
        s(S::Endpoint, language),
        view()
            .test_id("settings-endpoint")
            .flex_col()
            .gap(px(SPACE_3))
            .child(choice_row(
                "settings-provider",
                i18n::t(language, Key::Provider),
                &provider_choices(language),
                provider_choice,
                state.clone(),
                set_provider,
            ))
            .child(field_row(
                i18n::t(language, Key::BaseUrl),
                prims::text_field(
                    "settings-base-url",
                    i18n::t(language, Key::BaseUrl),
                    snapshot.model_base_url.clone(),
                    s(S::ProviderDefault, language),
                    move |value: &str| {
                        base_change_state
                            .settings
                            .update(|settings| settings.model_base_url = value.to_owned());
                    },
                ),
                base_url_note_view(state.clone(), language),
            ))
            .child(field_row_bare(
                i18n::t(language, Key::Model),
                prims::text_field(
                    "settings-model",
                    i18n::t(language, Key::Model),
                    snapshot.model_name.clone(),
                    s(S::ProviderDefault, language),
                    move |value: &str| {
                        model_change_state
                            .settings
                            .update(|settings| settings.model_name = value.to_owned());
                    },
                ),
            )),
    )
}

/// A wire format the settings name, as one of the page's choices.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ProviderChoice {
    /// No wire format: an operation picks the first provider that has a key.
    Best,
    /// One of the wire formats this build speaks.
    Wire(Provider),
    /// A wire format this build does not know (a hand-edited settings file):
    /// no choice is current, and picking one replaces it.
    Unsupported,
}

/// The choices of the wire-format row.
fn provider_choices(language: Language) -> [(&'static str, &'static str, ProviderChoice); 4] {
    [
        (
            "settings-provider-best",
            i18n::t(language, Key::ProviderBestAvailable),
            ProviderChoice::Best,
        ),
        (
            "settings-provider-anthropic",
            Provider::AnthropicMessages.label(),
            ProviderChoice::Wire(Provider::AnthropicMessages),
        ),
        (
            "settings-provider-openai-chat",
            Provider::OpenAiChat.label(),
            ProviderChoice::Wire(Provider::OpenAiChat),
        ),
        (
            "settings-provider-openai-responses",
            Provider::OpenAiResponses.label(),
            ProviderChoice::Wire(Provider::OpenAiResponses),
        ),
    ]
}

/// Which choice the settings currently name.
fn provider_choice(settings: &SettingsData) -> ProviderChoice {
    let key = settings.model_provider.trim();
    if key.is_empty() {
        ProviderChoice::Best
    } else {
        Provider::from_key(key).map_or(ProviderChoice::Unsupported, ProviderChoice::Wire)
    }
}

/// Write the chosen wire format into the settings, in memory; `Save` or
/// `Close` persists it, as the old page did.
fn set_provider(state: &AppState, choice: ProviderChoice) {
    let key = match choice {
        ProviderChoice::Best => "",
        ProviderChoice::Wire(provider) => provider.key(),
        ProviderChoice::Unsupported => return,
    };
    state
        .settings
        .update(|settings| settings.model_provider = key.to_owned());
}

/// The line under the base URL: the localized parse error while the value is
/// invalid, the short hint otherwise. `true` marks the error.
fn base_url_note(language: Language, provider: ProviderChoice, raw: &str) -> (SharedString, bool) {
    let raw = raw.trim();
    if raw.is_empty() {
        return (s(S::BaseUrlHint, language).into(), false);
    }
    // With no wire format chosen, validate the way the core's own
    // persist-safety check does: against OpenAI Chat Completions.
    let provider = match provider {
        ProviderChoice::Wire(provider) => provider,
        ProviderChoice::Best | ProviderChoice::Unsupported => Provider::OpenAiChat,
    };
    match EndpointIdentity::parse(provider, Some(raw)) {
        Ok(_) => (s(S::BaseUrlHint, language).into(), false),
        Err(error) => (endpoint_error_text(language, &error).into(), true),
    }
}

/// The parse error in the current language.
fn endpoint_error_text(language: Language, error: &EndpointIdentityError) -> &'static str {
    let key = match error {
        EndpointIdentityError::InvalidUrl(_) => Key::EndpointInvalidUrl,
        EndpointIdentityError::UnsupportedScheme => Key::EndpointUnsupportedScheme,
        EndpointIdentityError::MissingHost => Key::EndpointMissingHost,
        EndpointIdentityError::UserInfoNotAllowed => Key::EndpointUserInfoNotAllowed,
        EndpointIdentityError::QueryNotAllowed => Key::EndpointQueryNotAllowed,
        EndpointIdentityError::FragmentNotAllowed => Key::EndpointFragmentNotAllowed,
        EndpointIdentityError::InsecureRemoteTransport => Key::EndpointInsecureRemoteTransport,
    };
    i18n::t(language, key)
}

/// The reactive validation line under the base URL field.
fn base_url_note_view(state: AppState, language: Language) -> impl IntoElement {
    let text_state = state.clone();
    let colour_state = state;
    text(move || {
        let settings = text_state.settings.get();
        base_url_note(
            language,
            provider_choice(&settings),
            &settings.model_base_url,
        )
        .0
    })
    .test_id("settings-base-url-note")
    .font_family(FONT_UI)
    .text_size(px(TYPE_UI))
    .line_height(px(TYPE_UI_LINE))
    .text_color(move || {
        let settings = colour_state.settings.get();
        if base_url_note(
            language,
            provider_choice(&settings),
            &settings.model_base_url,
        )
        .1
        {
            tokens::palette().danger
        } else {
            tokens::palette().ink_faint
        }
    })
}

/// The target language the translation operations aim at.
fn translation_section(state: AppState, language: Language) -> impl IntoElement {
    let change_state = state.clone();
    let snapshot = state.settings.get();
    section(
        i18n::t(language, Key::Translation),
        view().test_id("settings-translation").child(field_row(
            i18n::t(language, Key::TargetLanguage),
            prims::text_field(
                "settings-target-language",
                i18n::t(language, Key::TargetLanguage),
                snapshot.translate_to.clone(),
                s(S::TargetLanguagePlaceholder, language),
                move |value: &str| {
                    change_state
                        .settings
                        .update(|settings| settings.translate_to = value.to_owned());
                },
            ),
            prims::faint(s(S::TargetLanguageHint, language)),
        )),
    )
}

/// The credential: where the key would come from, a replacement field, and the
/// three actions of the old page (store, use for this session, remove).
fn credential_section(state: AppState, language: Language) -> impl IntoElement {
    let value_state = state.clone();
    let change_state = state.clone();
    // Where the keyboard focus goes after Remove: the store control stays the
    // same element through every status change, so it is a safe landing place.
    let store_ref = element_ref();
    section(
        i18n::t(language, Key::Credential),
        view()
            .test_id("settings-credential")
            .flex_col()
            .gap(px(SPACE_3))
            .child(credential_status_line(state.clone(), language))
            .child(
                view()
                    .max_w(px(FIELD_W))
                    .flex_col()
                    .child(
                        input()
                            .test_id("api-key")
                            .label(i18n::t(language, Key::ApiKey))
                            .masked(true)
                            .value(move || value_state.settings_credential.get())
                            .placeholder(s(S::ReplacementKey, language))
                            .font_family(FONT_UI)
                            .text_size(px(TYPE_UI))
                            .bg(Color::TRANSPARENT)
                            .on_change(move |value: &str| {
                                change_state.settings_credential.set(value.to_owned());
                            })
                            .w_full(),
                    )
                    .child(prims::hairline()),
            )
            .child(credential_actions(
                state.clone(),
                state.clone(),
                state,
                language,
                store_ref,
            )),
    )
}

/// The credential as words: where the key comes from, never the key.
fn credential_status_line(state: AppState, language: Language) -> impl IntoElement {
    let colour_state = state.clone();
    text(move || credential_status_text(&state, language))
        .test_id("settings-credential-status")
        .font_family(FONT_UI)
        .text_size(px(TYPE_UI))
        .line_height(px(TYPE_UI_LINE))
        .text_color(move || {
            if current_credential_status(&colour_state) == CredentialStatus::Unavailable {
                tokens::palette().danger
            } else {
                tokens::palette().ink_rest
            }
        })
}

/// The vault's answer for the configured endpoint, memoised on the endpoint and
/// the credential revision (see [`AppState::credential_status_cached`]): the
/// memo signal makes a store or a removal reach the views that read it, and the
/// settings signal makes an endpoint change reach them. The status is never
/// derived from the key field.
fn current_credential_status(state: &AppState) -> CredentialStatus {
    state.credential_status_cached()
}

/// The status line's copy.
fn credential_status_text(state: &AppState, language: Language) -> SharedString {
    let settings = state.settings.get();
    match current_credential_status(state) {
        CredentialStatus::Unconfigured => {
            // No wire format, one this build does not know, or a base URL that
            // does not parse: say which.
            let key = match provider_choice(&settings) {
                ProviderChoice::Best => Key::ChooseProviderForCredential,
                ProviderChoice::Unsupported => Key::UnsupportedProvider,
                ProviderChoice::Wire(_) => Key::CredentialEndpointInvalid,
            };
            SharedString::from(i18n::t(language, key))
        }
        CredentialStatus::Missing => s(S::NoKeySaved, language).into(),
        CredentialStatus::Session => s(S::KeySavedSession, language).into(),
        CredentialStatus::Stored => s(S::KeySavedStored, language).into(),
        CredentialStatus::Environment => s(S::KeyFromEnvironment, language).into(),
        CredentialStatus::Unavailable => {
            SharedString::from(i18n::t(language, Key::CredentialReadFailed))
        }
    }
}

/// The credential's actions: store (or replace) the typed key, use it for this
/// session only, and forget the endpoint's key.
fn credential_actions(
    store_state: AppState,
    session_state: AppState,
    remove_state: AppState,
    language: Language,
    store_ref: ElementRef,
) -> impl IntoElement {
    let label_state = store_state.clone();
    let remove_condition = remove_state.clone();
    view()
        .flex_row()
        .items_center()
        .gap(px(SPACE_3))
        .child(
            reactive_action(
                "settings-credential-store",
                move || {
                    if current_credential_status(&label_state) == CredentialStatus::Stored {
                        s(S::Replace, language).into()
                    } else {
                        SharedString::from(i18n::t(language, Key::StoreCredentialSecurely))
                    }
                },
                {
                    let state = store_state.clone();
                    move || store_key(&state, language, false)
                },
            )
            .element_ref(&store_ref),
        )
        .child(prims::quiet_action(
            "settings-credential-session",
            i18n::t(language, Key::UseCredentialForSession),
            move || store_key(&session_state, language, true),
        ))
        .child(when(
            move || {
                matches!(
                    current_credential_status(&remove_condition),
                    CredentialStatus::Stored | CredentialStatus::Session
                )
            },
            move || {
                prims::quiet_action("settings-credential-remove", s(S::Remove, language), {
                    let state = remove_state.clone();
                    move || remove_key(&state, language, store_ref)
                })
                .into_element()
            },
        ))
}

/// Store the key in the field: `session` keeps it for this process only, else
/// it goes to the platform's secure store. The field is cleared either way, and
/// the status line follows through the revision signal.
fn store_key(state: &AppState, language: Language, session: bool) {
    let value = state.settings_credential.get();
    if value.trim().is_empty() {
        state.settings_notice.set(SharedString::from(i18n::t(
            language,
            Key::CredentialRequired,
        )));
        return;
    }
    let result = if session {
        state.store_session_credential(&value)
    } else {
        state.store_persistent_credential(&value)
    };
    match result {
        Ok(()) => {
            state.settings_credential.set(String::new());
            state.settings_credential_revision.update(|n| *n += 1);
            let key = if session {
                Key::KeyStoredForSession
            } else {
                Key::KeyStoredSecurely
            };
            state
                .settings_notice
                .set(SharedString::from(i18n::t(language, key)));
        }
        Err(error) => state.settings_notice.set(SharedString::from(error)),
    }
}

/// Forget the endpoint's key, in the secure store and in this process.
fn remove_key(state: &AppState, language: Language, focus_after: ElementRef) {
    match state.remove_credential() {
        Ok(()) => {
            state.settings_credential_revision.update(|n| *n += 1);
            state.settings_notice.set(SharedString::from(i18n::t(
                language,
                Key::CredentialDeleted,
            )));
            // This button goes away with the key it forgot: the keyboard user
            // lands on the store control instead of nowhere.
            focus_after.focus();
        }
        Err(error) => state.settings_notice.set(SharedString::from(error)),
    }
}

/// Theme: one signature swatch and one word per appearance.
fn theme_row(state: AppState, language: Language) -> impl IntoElement {
    let mut row = view()
        .test_id("settings-theme")
        .flex_row()
        .items_center()
        .gap(px(SPACE_3))
        .child(prims::rest(i18n::t(language, Key::Theme)));
    for (id, name, choice) in [
        (
            "settings-theme-system",
            i18n::t(language, Key::System),
            ThemePreference::System,
        ),
        (
            "settings-theme-light",
            i18n::t(language, Key::Light),
            ThemePreference::Light,
        ),
        (
            "settings-theme-dark",
            i18n::t(language, Key::Dark),
            ThemePreference::Dark,
        ),
    ] {
        let read_state = state.clone();
        let press_state = state.clone();
        row = row.child(
            view()
                .flex_row()
                .items_center()
                .gap(px(SPACE_2))
                .child(swatch(choice))
                .child(choice_button(
                    id,
                    name,
                    move || read_state.settings.get().theme == choice,
                    move || {
                        if press_state.settings.get().theme != choice {
                            press_state.set_theme(choice);
                        }
                    },
                )),
        );
    }
    row
}

/// One theme choice's signature: a small plate of that theme's ground with its
/// three ink steps. The theme is how the room lights, so the swatch shows its
/// light, never a glow.
fn swatch(theme: ThemePreference) -> impl IntoElement {
    let mut row = view().accessible(false).flex_row();
    match theme {
        ThemePreference::System => row = row.child(swatch_plate(false)).child(swatch_plate(true)),
        ThemePreference::Light => row = row.child(swatch_plate(false)),
        ThemePreference::Dark => row = row.child(swatch_plate(true)),
    }
    row
}

/// A plate of one appearance's ground with its ink steps.
fn swatch_plate(dark: bool) -> impl IntoElement {
    let palette = if dark { tokens::DARK } else { tokens::LIGHT };
    view()
        .h(px(16.0))
        .w(px(20.0))
        .bg(palette.room)
        .border(px(HAIRLINE))
        .border_color(palette.hairline)
        .flex_row()
        .items_center()
        .justify_center()
        .gap(px(2.0))
        .child(ink_bar(palette.ink))
        .child(ink_bar(palette.ink_rest))
        .child(ink_bar(palette.ink_faint))
}

/// One ink step of a swatch.
fn ink_bar(color: Color) -> impl IntoElement {
    view().w(px(2.0)).h(px(8.0)).bg(color)
}

/// The local usage counters: on or off, what they hold, and the three local
/// actions the old page offered.
fn usage_section(state: AppState, language: Language) -> impl IntoElement {
    let counts = state.usage_counts;
    let inspection_state = state.clone();
    let inspect_state = state.clone();
    let export_state = state.clone();
    let clear_state = state.clone();
    view()
        .test_id("usage-counters")
        .flex_col()
        .gap(px(SPACE_3))
        .child(prims::rest(i18n::t(language, Key::UsageCounters)))
        .child(
            view()
                .max_w(px(MEASURE_WIDE))
                .child(prims::faint(i18n::t(language, Key::UsageDisclosure))),
        )
        .child(choice_row(
            "usage-toggle",
            "",
            &[
                ("usage-off", i18n::t(language, Key::UsageOff), false),
                ("usage-on", i18n::t(language, Key::UsageOn), true),
            ],
            |settings: &SettingsData| settings.usage_metrics_enabled,
            state.clone(),
            |state, enabled| state.set_usage_enabled(enabled),
        ))
        .child(view().test_id("usage-summary").child(prims::ink(move || {
            SharedString::from(i18n::usage_summary(language, &counts.get()))
        })))
        .child(
            view()
                .test_id("usage-retention")
                .child(prims::faint(i18n::t(language, Key::UsageRetention))),
        )
        .child(
            view()
                .flex_row()
                .items_center()
                .gap(px(SPACE_3))
                .child(prims::quiet_action(
                    "usage-inspect",
                    i18n::t(language, Key::UsageInspect),
                    move || inspect_state.inspect_usage(),
                ))
                .child(prims::quiet_action(
                    "usage-export",
                    i18n::t(language, Key::UsageExport),
                    move || export_state.export_usage(),
                ))
                .child(prims::quiet_action(
                    "usage-clear",
                    i18n::t(language, Key::UsageClear),
                    move || clear_state.clear_usage(),
                )),
        )
        .child(when(
            move || inspection_state.usage_inspection_open.get(),
            move || {
                view()
                    .test_id("usage-details")
                    .flex_col()
                    .child(each(
                        move || {
                            i18n::usage_details(language, &counts.get())
                                .lines()
                                .map(str::to_owned)
                                .collect::<Vec<String>>()
                        },
                        |line: &String| line.clone(),
                        |line: ReadSignal<String>, _| {
                            prims::rest(move || SharedString::from(line.get()))
                        },
                    ))
                    .into_element()
            },
        ))
}

/// A row of mutually exclusive choices: the current one reads as the value (full
/// ink, selected for screen readers) and the others rest, but every choice stays
/// the same pressable control, so choosing one with the keyboard keeps the
/// focus. `read` reads the settings signal inside the bindings, so a change
/// repaints the row.
fn choice_row<T: Copy + PartialEq + 'static>(
    test_id: &'static str,
    label: &'static str,
    choices: &[(&'static str, &'static str, T)],
    read: fn(&SettingsData) -> T,
    state: AppState,
    set: fn(&AppState, T),
) -> impl IntoElement {
    let mut row = view()
        .test_id(test_id)
        .flex_row()
        .items_center()
        .gap(px(SPACE_3));
    if !label.is_empty() {
        row = row.child(prims::rest(label));
    }
    for (id, name, value) in choices {
        let id = *id;
        let name = *name;
        let value = *value;
        let current_state = state.clone();
        let press_state = state.clone();
        row = row.child(choice_button(
            id,
            name,
            move || read(&current_state.settings.get()) == value,
            move || {
                if read(&press_state.settings.get()) != value {
                    set(&press_state, value);
                }
            },
        ));
    }
    row
}

/// One choice of a mutually exclusive row: the current one carries full ink and
/// the selected state for screen readers, the others rest, and every one of
/// them is the same pressable control — so a choice made with the keyboard
/// keeps the focus where it was.
fn choice_button(
    test_id: &'static str,
    label: &'static str,
    current: impl Fn() -> bool + Clone + 'static,
    on_press: impl Fn() + 'static,
) -> impl Interactive + ParentElement {
    let label: SharedString = label.into();
    let colour = {
        let current = current.clone();
        move || {
            if current() {
                tokens::palette().ink
            } else {
                tokens::palette().ink_rest
            }
        }
    };
    // The test id goes on the wrapper: `nsh_base`'s components are not views,
    // so `test_id` reaches them through a plain element around them.
    view()
        .test_id(test_id)
        .bg(Color::TRANSPARENT)
        .window_drag(false)
        .child(
            button()
                .accessibility_label(Some(label.clone()))
                .selected(current)
                .px(px(2.0))
                .py(px(4.0))
                .bg(Color::TRANSPARENT)
                .cursor_pointer()
                .rounded(px(0.0))
                .transition_colors()
                .duration(motion::HOVER_MS)
                .hover(|s| s.text_color(tokens::palette().ink).underline())
                .focus_visible(|s| s.text_color(tokens::palette().ink).underline())
                .on_press(on_press)
                .child(
                    text(label)
                        .font_family(FONT_UI)
                        .text_size(px(TYPE_UI))
                        .line_height(px(TYPE_UI_LINE))
                        .text_color(colour),
                ),
        )
}

/// A quiet local action whose label follows the state. The credential's one
/// store control must stay the same element while its label changes from
/// 'Store securely' to 'Replace', or the focus a keyboard user put on it is
/// thrown away.
fn reactive_action(
    test_id: &'static str,
    label: impl Fn() -> SharedString + Clone + 'static,
    on_press: impl Fn() + 'static,
) -> impl Interactive + ParentElement {
    view()
        .test_id(test_id)
        .bg(Color::TRANSPARENT)
        .window_drag(false)
        .child(
            button()
                .accessibility_label({
                    let label = label.clone();
                    move || Some(label())
                })
                .px(px(2.0))
                .py(px(4.0))
                .bg(Color::TRANSPARENT)
                .cursor_pointer()
                .rounded(px(0.0))
                .transition_colors()
                .duration(motion::HOVER_MS)
                .hover(|s| s.text_color(tokens::palette().ink).underline())
                .focus_visible(|s| s.text_color(tokens::palette().ink).underline())
                .on_press(on_press)
                .child(
                    text(label)
                        .font_family(FONT_UI)
                        .text_size(px(TYPE_UI))
                        .line_height(px(TYPE_UI_LINE))
                        .text_color(move || tokens::palette().ink_rest),
                ),
        )
}

/// The settings page's own copy. New strings live here rather than in
/// `i18n.rs` (a merge hotspot): the match is exhaustive, so a missing
/// translation is a compile error.
#[derive(Clone, Copy)]
enum S {
    Endpoint,
    BaseUrlHint,
    ProviderDefault,
    TargetLanguagePlaceholder,
    TargetLanguageHint,
    ReplacementKey,
    NoKeySaved,
    KeySavedStored,
    KeySavedSession,
    KeyFromEnvironment,
    Replace,
    Remove,
}

fn s(key: S, language: Language) -> &'static str {
    match language {
        Language::English => match key {
            S::Endpoint => "Endpoint",
            S::BaseUrlHint => {
                "Leave empty for the provider default. Remote hosts need HTTPS; include the \
                 API path, e.g. /v1."
            }
            S::ProviderDefault => "Provider default",
            S::TargetLanguagePlaceholder => "zh",
            S::TargetLanguageHint => "A language name or code, e.g. zh, ja, German.",
            S::ReplacementKey => "Enter a replacement key",
            S::NoKeySaved => "No key saved",
            S::KeySavedStored => "Key saved in Windows Credential Manager",
            S::KeySavedSession => "Key saved for this session only",
            S::KeyFromEnvironment => "Key taken from the environment",
            S::Replace => "Replace",
            S::Remove => "Remove",
        },
        Language::Chinese => match key {
            S::Endpoint => "端点",
            S::BaseUrlHint => {
                "留空则使用服务商默认值。远程主机必须使用 HTTPS；请包含 API 路径，例如 /v1。"
            }
            S::ProviderDefault => "服务商默认值",
            S::TargetLanguagePlaceholder => "zh",
            S::TargetLanguageHint => "语言名称或代码，例如 zh、ja、German。",
            S::ReplacementKey => "输入替换密钥",
            S::NoKeySaved => "未保存密钥",
            S::KeySavedStored => "密钥已保存在 Windows 凭据管理器中",
            S::KeySavedSession => "密钥仅在本次会话中有效",
            S::KeyFromEnvironment => "密钥来自环境变量",
            S::Replace => "替换",
            S::Remove => "移除",
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        collections::HashMap,
        sync::{
            Arc, LazyLock, Mutex,
            atomic::{AtomicUsize, Ordering},
        },
    };

    use mt_core::credentials::{CredentialError, Secret, SecureCredentialStore};

    /// The vault a test drives: credentials live in this map and never in the
    /// platform's secure store, and every read is counted, so a test can prove
    /// the page does not read the store per keystroke.
    #[derive(Default)]
    struct CountingStore {
        values: Mutex<HashMap<String, Secret>>,
        reads: AtomicUsize,
    }

    impl CountingStore {
        fn reads(&self) -> usize {
            self.reads.load(Ordering::Relaxed)
        }

        fn forget_reads(&self) {
            self.reads.store(0, Ordering::Relaxed);
        }

        /// The secret stored for `target`, for the test's own assertions.
        fn stored(&self, target: &str) -> Option<String> {
            self.values
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .get(target)
                .map(|secret| secret.expose().to_owned())
        }
    }

    impl SecureCredentialStore for CountingStore {
        fn is_supported(&self) -> bool {
            true
        }

        fn read(&self, target: &str) -> Result<Option<Secret>, CredentialError> {
            self.reads.fetch_add(1, Ordering::Relaxed);
            Ok(self
                .values
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .get(target)
                .cloned())
        }

        fn write(&self, target: &str, secret: &Secret) -> Result<(), CredentialError> {
            self.values
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .insert(target.to_owned(), secret.clone());
            Ok(())
        }

        fn delete(&self, target: &str) -> Result<(), CredentialError> {
            self.values
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .remove(target);
            Ok(())
        }
    }

    /// The probe's vault: one per test binary, read by the test that drives the
    /// page so it can count the store reads.
    static PROBE_STORE: LazyLock<Arc<CountingStore>> =
        LazyLock::new(|| Arc::new(CountingStore::default()));

    #[test]
    fn an_empty_base_url_uses_the_provider_default() {
        let (note, error) = base_url_note(Language::English, ProviderChoice::Best, "  ");
        assert_eq!(
            note,
            SharedString::from(s(S::BaseUrlHint, Language::English))
        );
        assert!(!error);
    }

    #[test]
    fn a_usable_base_url_shows_the_hint() {
        for url in [
            "https://api.openai.com/v1",
            "http://127.0.0.1:11434/v1",
            "https://localhost:8443/v1/",
        ] {
            let (_, error) = base_url_note(Language::English, ProviderChoice::Best, url);
            assert!(!error, "{url} was rejected");
        }
    }

    #[test]
    fn plain_http_to_a_named_host_is_rejected() {
        // The core treats only numeric loopback addresses as local, so
        // `localhost` over HTTP must fail the same way a remote host does.
        let (_, error) = base_url_note(
            Language::English,
            ProviderChoice::Best,
            "http://localhost:8000/v1",
        );
        assert!(error);
    }

    #[test]
    fn an_unusable_base_url_shows_the_parse_error() {
        // A remote HTTP endpoint cannot be reached safely: this is the error the
        // field must show, not a silent pass.
        let (note, error) = base_url_note(
            Language::English,
            ProviderChoice::Wire(Provider::OpenAiChat),
            "http://example.com/v1",
        );
        assert!(error);
        assert_eq!(
            note,
            SharedString::from(i18n::t(
                Language::English,
                Key::EndpointInsecureRemoteTransport
            ))
        );

        let (note, error) = base_url_note(Language::Chinese, ProviderChoice::Best, "not a url");
        assert!(error);
        assert_eq!(
            note,
            SharedString::from(i18n::t(Language::Chinese, Key::EndpointInvalidUrl))
        );
    }

    #[test]
    fn the_wire_format_choice_follows_the_settings() {
        let mut settings = SettingsData::default();
        assert_eq!(provider_choice(&settings), ProviderChoice::Best);
        settings.model_provider = "openai-responses".into();
        assert_eq!(
            provider_choice(&settings),
            ProviderChoice::Wire(Provider::OpenAiResponses)
        );
        settings.model_provider = "not-a-wire-format".into();
        assert_eq!(provider_choice(&settings), ProviderChoice::Unsupported);
    }

    /// The page on its own, without the shell: the probe test's root. The shell
    /// is not the subject here — while the overlay layer takes pointer input
    /// nothing above it is pressable in the harness either. Its vault is the
    /// probe's own in-memory store, never the platform's.
    #[component]
    fn SettingsProbe() -> impl IntoElement {
        crate::ui::fonts::register();
        let state = AppState::with_vault(mt_core::credentials::CredentialVault::with_store(
            PROBE_STORE.clone(),
        ));
        // A custom endpoint, so the environment is never consulted: the status
        // this probe reads is the one the vault holds.
        state.settings.update(|settings| {
            settings.model_provider = "openai-chat".to_owned();
            settings.model_base_url = "http://127.0.0.1:11434/v1".to_owned();
        });
        view()
            .flex_1()
            .h(relative(1.0))
            .w_full()
            .flex_col()
            .bg(move || tokens::palette().room)
            .text_color(move || tokens::palette().ink)
            .font_family(FONT_UI)
            .text_size(px(TYPE_UI))
            .line_height(px(TYPE_UI_LINE))
            .child(page(state, Language::English))
    }

    /// Drives the page in the headless harness: the credential status follows
    /// the wire format, the line under the base URL follows what is typed, the
    /// secure store is read for an endpoint change and not for anything else,
    /// and the stored and session keys are forgotten again. Kept because the
    /// real window cannot be driven on this host (a disconnected session takes
    /// no synthetic input).
    #[test]
    fn the_page_follows_the_endpoint_and_manages_the_keys() {
        let config = tempfile::tempdir().expect("config dir");
        // SAFETY: no other test in this binary starts the app or reads these
        // variables.
        unsafe {
            std::env::remove_var("MARKTURBO_OPEN");
            std::env::set_var("MARKTURBO_CONFIG_DIR", config.path());
            std::env::set_var("MARKTURBO_DATA_DIR", config.path().join("data"));
        }
        let mut app =
            nsh_testing::Headless::start(nsh_engine::EngineConfig::new(nsh_engine::Root::of::<
                SettingsProbe,
            >()))
            .expect("start the page");
        app.set_viewport(nsh_core::Size::new(
            nsh_core::Points(1440.0),
            nsh_core::Points(900.0),
        ))
        .expect("viewport");
        app.wait_for("id:settings-page").expect("the page");
        app.wait_for("text:No key saved")
            .expect("credential status");

        // A remote HTTP endpoint is refused, and the line under the field says
        // why.
        app.clear("id:settings-base-url").expect("clear the field");
        app.type_text("id:settings-base-url", "http://example.com/v1")
            .expect("type a base URL");
        app.wait_for("text:Remote endpoints must use HTTPS.")
            .expect("validation error");

        // A numeric loopback URL over HTTP is the one plain-HTTP case the core
        // takes: the error line goes back to the hint.
        app.clear("id:settings-base-url").expect("clear the field");
        app.type_text("id:settings-base-url", "http://127.0.0.1:8000/v1")
            .expect("type a loopback URL");
        app.wait_for_gone(
            "text:Remote endpoints must use HTTPS.",
            std::time::Duration::from_secs(5),
        )
        .expect("validation error gone");

        // The wire format is a choice, and the status line follows it. A custom
        // endpoint never consults the provider's environment variable, so the
        // status is this process's own.
        app.press("text:Best available")
            .expect("choose best available");
        app.wait_for("text:Choose a provider before configuring credentials.")
            .expect("unconfigured status");
        app.press("text:OpenAI Chat Completions")
            .expect("choose a wire format");
        app.wait_for("text:No key saved")
            .expect("configured status");

        // A choice that is now the current one stays the same control: pressing
        // it again is still a press (a keyboard user keeps the focus) and does
        // nothing — not even re-saving the settings.
        app.press("text:OpenAI Chat Completions")
            .expect("the current wire format is still a control");
        app.press("text:English")
            .expect("the current language is still a control");
        app.wait_for("text:No key saved").expect("unchanged status");
        assert!(
            !app.screen().shows("Settings saved."),
            "pressing the current choice saved the settings again"
        );

        // The status is read from the vault for an endpoint change, and for
        // nothing else: typing in a field the status does not depend on, or
        // toggling the local counters, must not touch the secure store.
        PROBE_STORE.forget_reads();
        app.type_text("id:settings-model", "probe-model")
            .expect("type a model");
        app.wait_for("text:probe-model").expect("the model landed");
        assert_eq!(
            PROBE_STORE.reads(),
            0,
            "the status read the vault for a model edit"
        );
        app.clear("id:settings-target-language")
            .expect("clear the target language");
        app.type_text("id:settings-target-language", "ja")
            .expect("type a target language");
        app.wait_for("text:ja").expect("the target language landed");
        app.press("text:On").expect("enable the local counters");
        app.wait_for("text:Usage-counter preference saved.")
            .expect("the counters were saved");
        assert_eq!(
            PROBE_STORE.reads(),
            0,
            "the status read the vault for a preference change"
        );

        // The key stores in the vault the page was built with, the control
        // keeps its identity while its label changes to Replace, and Remove
        // forgets it again.
        let target =
            EndpointIdentity::parse(Provider::OpenAiChat, Some("http://127.0.0.1:8000/v1"))
                .expect("the probe's endpoint")
                .credential_target()
                .as_str()
                .to_owned();
        app.type_text("id:api-key", "synthetic-probe-key")
            .expect("type a key");
        app.press("text:Store securely").expect("store securely");
        app.wait_for("text:Key saved in Windows Credential Manager")
            .expect("stored status");
        app.wait_for("text:Key stored securely.")
            .expect("store notice");
        assert!(
            PROBE_STORE.reads() > 0,
            "the stored status never read the vault"
        );
        assert_eq!(
            PROBE_STORE.stored(&target).as_deref(),
            Some("synthetic-probe-key"),
            "the key did not reach the vault"
        );
        assert_eq!(
            app.screen()
                .input_text("id:api-key")
                .expect("read the key field")
                .unwrap_or_default(),
            "",
            "the field keeps the key after storing it"
        );
        app.type_text("id:api-key", "synthetic-replacement-key")
            .expect("type a replacement");
        app.press("text:Replace")
            .expect("the same control, replaced");
        app.wait_for("text:Key saved in Windows Credential Manager")
            .expect("still stored");
        assert_eq!(
            PROBE_STORE.stored(&target).as_deref(),
            Some("synthetic-replacement-key"),
            "Replace did not write the new key"
        );
        assert_eq!(
            app.screen()
                .input_text("id:api-key")
                .expect("read the key field")
                .unwrap_or_default(),
            "",
            "the field keeps the key after replacing it"
        );
        app.press("text:Remove").expect("forget the stored key");
        app.wait_for("text:No key saved")
            .expect("status after removal");
        assert_eq!(
            PROBE_STORE.stored(&target),
            None,
            "Remove left the key in the vault"
        );

        // The session fallback stores nothing persistent and is forgotten the
        // same way.
        app.type_text("id:api-key", "synthetic-probe-key")
            .expect("type a key");
        app.press("text:Use for session")
            .expect("store for the session");
        app.wait_for("text:Key saved for this session only")
            .expect("session status");
        app.wait_for("text:Key stored for this session.")
            .expect("store notice");
        assert_eq!(
            PROBE_STORE.stored(&target),
            None,
            "the session key was written to the secure store"
        );
        app.press("text:Remove").expect("forget the key");
        app.wait_for("text:No key saved")
            .expect("status after removal");
    }
}
