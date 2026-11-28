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
//! The page obeys the Afterglow rule: nothing here emits light.
//!
//! Reactions: the page is rebuilt when the language or the layout changes
//! (`app.rs` re-keys the shell), so its labels are read once. Everything that
//! must follow the state itself — the choices, the validation line, the usage
//! counts, the credential status — reads its signal inside the binding that
//! draws it.

use mt_core::{
    companion_font,
    model::{EndpointIdentity, EndpointIdentityError, Provider},
    settings::{Language, SettingsData, ThemePreference},
};
use nsh_base::input;
use nsh_core::{Color, element::ScrollEvent, keymap::When, prelude::*};

use crate::{
    i18n::{self, Key},
    state::{AppState, CredentialStatus},
    ui::{
        fonts::{Companion, CompanionAction, CompanionFailure, CompanionStatus},
        notices, prims,
        tokens::{
            self, COLUMN_GAP, FONT_INPUT, FONT_UI, HAIRLINE, MEASURE_NARROW, MEASURE_WIDE, RAIL_W,
            SETTINGS_HEADER_H, SPACE_1, SPACE_2, SPACE_3, SPACE_4, SPACE_5, TYPE_PREVIEW,
            TYPE_PREVIEW_LINE, TYPE_UI, TYPE_UI_LINE, WEIGHT_MEDIUM,
        },
    },
};

/// Whether the settings page is up.
pub fn is_open(state: &AppState) -> bool {
    state.settings_open.get()
}

pub(crate) fn page(
    state: AppState,
    language: Language,
    presence: notices::StatusPresence,
) -> impl IntoElement {
    // The store can change under us — another instance storing or removing a
    // key — and nothing in this process reports that, so every (re)build of the
    // page reads the vault afresh. Within a page, the status is memoised.
    state.forget_credential_status();
    let escape_state = state.clone();
    on_key_with(
        "escape",
        KeyOptions {
            when: When::Always,
            ..KeyOptions::default()
        },
        move |_| {
            escape_state.escape_top_layer();
        },
    );
    let close_state = state.clone();
    let scroll_state = state.clone();
    let scroll_offset = state.settings_scroll_offset.get();
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
                .max_w(px(MEASURE_NARROW))
                .min_h(px(SETTINGS_HEADER_H))
                .flex_row()
                .items_center()
                .child(heading(i18n::t(language, Key::Settings)))
                .child(view().flex_1())
                .child(view().flex_row().items_center().gap(px(SPACE_3)).child(
                    prims::quiet_action(
                        "settings-close",
                        i18n::t(language, Key::Close),
                        move || close_state.close_settings(),
                    ),
                )),
        )
        .child(
            view()
                .w_full()
                .h(px(SPACE_3))
                .border_b(px(HAIRLINE))
                .border_b_color(move || tokens::palette().hairline),
        )
        .child(
            scroll()
                .test_id("settings-scroll")
                .flex_1()
                .min_h(px(0.0))
                .w_full()
                .flex_col()
                .pt(px(SPACE_3))
                .content_offset(point(Points(0.0), Points(scroll_offset)))
                .on_scroll(move |event: &ScrollEvent| {
                    scroll_state.settings_scroll_offset.set(event.offset.y.0);
                })
                .child(content(state.clone(), language)),
        )
        .child(notice_line(state.clone()))
        .child(notices::status_line_at(
            state,
            language,
            presence,
            crate::state::StatusSurface::Settings,
        ))
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
/// calm — but its space is always reserved, so a notice appearing below the
/// scroll cannot shrink the viewport under the content the user scrolled to.
fn notice_line(state: AppState) -> impl IntoElement {
    let condition_state = state.clone();
    let text_state = state.clone();
    let color_state = state;
    view()
        .test_id("settings-notice")
        .min_h(px(SPACE_3 + TYPE_UI_LINE))
        .child(when(
            move || !condition_state.settings_notice.get().is_empty(),
            move || {
                view()
                    .pt(px(SPACE_3))
                    .child(
                        text(move || text_state.settings_notice.get())
                            .font_family(FONT_UI)
                            .text_size(px(TYPE_UI))
                            .line_height(px(TYPE_UI_LINE))
                            .text_color(move || {
                                if color_state.settings_notice_error.get() {
                                    tokens::palette().danger
                                } else {
                                    tokens::palette().ink_rest
                                }
                            }),
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
        .child(cjk_font_section(state.companion_font.clone(), language))
        .child(usage_section(state, language))
}

/// One labelled group: its label in resting ink, its rows 16px beneath.
fn section(label: &'static str, rows: impl IntoElement) -> impl IntoElement {
    view()
        .flex_col()
        .gap(px(SPACE_3))
        .child(prims::rest(label).h(px(prims::UI_TEXT_BOX_H)))
        .child(rows)
}

/// One labelled field: its visible label, the field itself (an input over a
/// hairline) and the line under it — the validation error while invalid, the
/// hint otherwise. Its boxes are pinned so the row keeps the 4px rhythm
/// whatever the platform measures its text and controls at.
fn field_row(
    label: &'static str,
    field: impl IntoElement,
    note: impl IntoElement,
) -> impl IntoElement {
    view()
        .flex_col()
        .gap(px(SPACE_2))
        .child(prims::rest(label).h(px(prims::UI_TEXT_BOX_H)))
        .child(
            view()
                .max_w(px(MEASURE_NARROW))
                .h(px(FIELD_BOX_H))
                .child(field),
        )
        .child(
            view()
                .max_w(px(MEASURE_NARROW))
                .h(px(NOTE_BOX_H))
                .child(note),
        )
}

/// The field's box: its text line, its rule and the rule's clearance.
const FIELD_BOX_H: f32 = SPACE_5 / 2.0;
/// A note's box: two UI line boxes.
const NOTE_BOX_H: f32 = 2.0 * TYPE_UI_LINE;

fn endpoint_section(state: AppState, language: Language) -> impl IntoElement {
    let base_change_state = state.clone();
    let base_blur_state = state.clone();
    let model_change_state = state.clone();
    let model_blur_state = state.clone();
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
            .child(
                view()
                    .max_w(px(MEASURE_WIDE))
                    .min_h(px(2.0 * TYPE_UI_LINE))
                    .child(prims::faint(i18n::t(language, Key::ProviderHelp))),
            )
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
                    move || {
                        base_blur_state.save_settings_quietly();
                    },
                ),
                base_url_note_view(state.clone(), language),
            ))
            .child(field_row(
                i18n::t(language, Key::Model),
                view().pb(px(SPACE_1 - HAIRLINE)).child(prims::text_field(
                    "settings-model",
                    i18n::t(language, Key::Model),
                    snapshot.model_name.clone(),
                    s(S::ProviderDefault, language),
                    move |value: &str| {
                        model_change_state
                            .settings
                            .update(|settings| settings.model_name = value.to_owned());
                    },
                    move || {
                        model_blur_state.save_settings_quietly();
                    },
                )),
                view()
                    .min_h(px(2.0 * TYPE_UI_LINE))
                    .child(prims::faint(i18n::t(language, Key::ModelHelp))),
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

fn set_provider(state: &AppState, choice: ProviderChoice) {
    let key = match choice {
        ProviderChoice::Best => "",
        ProviderChoice::Wire(provider) => provider.key(),
        ProviderChoice::Unsupported => return,
    };
    state
        .settings
        .update(|settings| settings.model_provider = key.to_owned());
    state.save_settings_quietly();
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
            tokens::palette().ink_caption
        }
    })
}

/// The target language the translation operations aim at.
fn translation_section(state: AppState, language: Language) -> impl IntoElement {
    let change_state = state.clone();
    let blur_state = state.clone();
    let snapshot = state.settings.get();
    section(
        i18n::t(language, Key::Translation),
        view().test_id("settings-translation").child(field_row(
            i18n::t(language, Key::TargetLanguage),
            view().pb(px(SPACE_1 - HAIRLINE)).child(prims::text_field(
                "settings-target-language",
                i18n::t(language, Key::TargetLanguage),
                snapshot.translate_to.clone(),
                s(S::TargetLanguagePlaceholder, language),
                move |value: &str| {
                    change_state
                        .settings
                        .update(|settings| settings.translate_to = value.to_owned());
                },
                move || {
                    blur_state.save_settings_quietly();
                },
            )),
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
                    .max_w(px(MEASURE_NARROW))
                    .flex_col()
                    .h(px(FIELD_BOX_H))
                    .pb(px(SPACE_1 - HAIRLINE))
                    .child(
                        input()
                            .test_id("api-key")
                            .label(i18n::t(language, Key::ApiKey))
                            .masked(true)
                            .value(move || value_state.settings_credential.get())
                            .placeholder(s(S::ReplacementKey, language))
                            .font_family(FONT_INPUT)
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
    let height_state = state.clone();
    text(move || credential_status_text(&state, language))
        .test_id("settings-credential-status")
        .font_family(FONT_UI)
        .text_size(px(TYPE_UI))
        .line_height(px(TYPE_UI_LINE))
        .min_h(move || px(credential_status_height(&height_state)))
        .text_color(move || {
            if current_credential_status(&colour_state) == CredentialStatus::Unavailable {
                tokens::palette().danger
            } else {
                tokens::palette().ink_rest
            }
        })
}

fn credential_status_height(state: &AppState) -> f32 {
    match current_credential_status(state) {
        CredentialStatus::Unconfigured | CredentialStatus::Unavailable => NOTE_BOX_H,
        CredentialStatus::Missing if !state.secure_store_supported() => NOTE_BOX_H,
        _ => TYPE_UI_LINE,
    }
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
        CredentialStatus::Missing if !state.secure_store_supported() => {
            SharedString::from(i18n::t(language, Key::SecureStoreUnavailable))
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

fn credential_actions(
    store_state: AppState,
    session_state: AppState,
    remove_state: AppState,
    language: Language,
    store_ref: ElementRef,
) -> impl IntoElement {
    let label_state = store_state.clone();
    let store_enabled_state = store_state.clone();
    let session_enabled_state = session_state.clone();
    let store_supported = store_state.secure_store_supported();
    let remove_condition = remove_state.clone();
    view()
        .flex_row()
        .items_center()
        .gap(px(SPACE_3))
        .child(when(
            move || store_supported,
            move || {
                let label_state = label_state.clone();
                let store_enabled_state = store_enabled_state.clone();
                prims::action(
                    "settings-credential-store",
                    move || {
                        if current_credential_status(&label_state) == CredentialStatus::Stored {
                            SharedString::from(s(S::Replace, language))
                        } else {
                            SharedString::from(i18n::t(language, Key::StoreCredentialSecurely))
                        }
                    },
                    prims::ActionTone::Quiet,
                    prims::ActionOptions {
                        disabled: Some(
                            (move || {
                                current_credential_status(&store_enabled_state)
                                    == CredentialStatus::Unconfigured
                            })
                            .into_reactive(),
                        ),
                        reference: Some(store_ref),
                        ..Default::default()
                    },
                    {
                        let state = store_state.clone();
                        move || store_key(&state, language, false)
                    },
                )
                .into_element()
            },
        ))
        .child(
            prims::quiet_action(
                "settings-credential-session",
                i18n::t(language, Key::UseCredentialForSession),
                move || store_key(&session_state, language, true),
            )
            .disabled(move || {
                current_credential_status(&session_enabled_state) == CredentialStatus::Unconfigured
            })
            .disabled_style(|style| style.opacity(0.45)),
        )
        .child(when(
            move || {
                matches!(
                    current_credential_status(&remove_condition),
                    CredentialStatus::Stored | CredentialStatus::Session
                )
            },
            move || {
                prims::danger_action("settings-credential-remove", s(S::Remove, language), {
                    let state = remove_state.clone();
                    move || remove_key(&state, store_ref)
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
        state.set_settings_notice(i18n::t(language, Key::CredentialRequired), false);
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
            state.set_settings_notice(i18n::t(language, key), false);
        }
        Err(error) => state.set_settings_notice(error, true),
    }
}

fn remove_key(state: &AppState, focus_after: ElementRef) {
    match state.remove_credential() {
        Ok(()) => focus_after.focus(),
        Err(error) => state.set_settings_notice(error, true),
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
                .child(prims::action(
                    id,
                    name,
                    prims::ActionTone::Quiet,
                    prims::ActionOptions {
                        chosen: Some(
                            (move || read_state.settings.get().theme == choice).into_reactive(),
                        ),
                        ..Default::default()
                    },
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

/// The companion Chinese font: what it is and where it would come from, and
/// the actions available in each state. Nothing is fetched until Download or
/// Retry is pressed, after the disclosure names the host, size and licence.
pub(super) fn cjk_font_section(companion: Companion, language: Language) -> impl IntoElement {
    let status = companion.status;
    let total = companion.size();
    let offers = |action| {
        let companion = companion.clone();
        move || companion.offers(action)
    };
    let disclosure = companion.clone();
    let (download, retry, cancel, remove) = (
        companion.clone(),
        companion.clone(),
        companion.clone(),
        companion.clone(),
    );
    section(
        s(S::ChineseFont, language),
        view()
            .test_id("settings-cjk-font")
            .flex_col()
            .gap(px(SPACE_3))
            .child(
                view()
                    .test_id("settings-cjk-font-status")
                    .child(prims::ink(move || {
                        cjk_font_status_text(status.get(), total, language)
                    })),
            )
            .child(when(
                move || {
                    matches!(
                        disclosure.action(),
                        Some(CompanionAction::Download | CompanionAction::Retry)
                    )
                },
                move || {
                    view()
                        .test_id("settings-cjk-font-disclosure")
                        .max_w(px(MEASURE_WIDE))
                        .child(prims::faint(cjk_font_disclosure(language)))
                        .into_element()
                },
            ))
            .child(
                view()
                    .flex_row()
                    .items_center()
                    .gap(px(SPACE_3))
                    .child(when(offers(CompanionAction::Download), move || {
                        let companion = download.clone();
                        prims::primary_action(
                            "settings-cjk-font-download",
                            s(S::DownloadAndApply, language),
                            move || companion.download(),
                        )
                        .into_element()
                    }))
                    .child(when(offers(CompanionAction::Retry), move || {
                        let companion = retry.clone();
                        prims::primary_action(
                            "settings-cjk-font-retry",
                            s(S::Retry, language),
                            move || companion.download(),
                        )
                        .into_element()
                    }))
                    .child(when(offers(CompanionAction::Cancel), move || {
                        let companion = cancel.clone();
                        prims::quiet_action(
                            "settings-cjk-font-cancel",
                            i18n::t(language, Key::Cancel),
                            move || companion.cancel(),
                        )
                        .into_element()
                    }))
                    .child(when(offers(CompanionAction::Remove), move || {
                        let companion = remove.clone();
                        prims::quiet_action(
                            "settings-cjk-font-remove",
                            s(S::Remove, language),
                            move || companion.remove(),
                        )
                        .into_element()
                    })),
            ),
    )
}

fn cjk_font_status_text(status: CompanionStatus, total: u64, language: Language) -> SharedString {
    let key = match status {
        CompanionStatus::Downloading(received) => {
            let megabytes = |bytes: u64| bytes as f64 / 1_000_000.0;
            return format!(
                "{} {:.1} / {:.1} MB",
                s(S::FontDownloading, language),
                megabytes(received),
                megabytes(total)
            )
            .into();
        }
        CompanionStatus::Applying => S::FontApplying,
        CompanionStatus::Absent => S::FontAbsent,
        CompanionStatus::Installed => S::FontInstalled,
        CompanionStatus::Removing => S::FontRemoving,
        CompanionStatus::Removed => S::FontRemoved,
        CompanionStatus::Failed(CompanionFailure::Network) => S::FontFailedNetwork,
        CompanionStatus::Failed(CompanionFailure::Verification) => S::FontFailedVerification,
        CompanionStatus::Failed(CompanionFailure::Disk) => S::FontFailedDisk,
        CompanionStatus::Failed(CompanionFailure::Registration) => S::FontFailedRegistration,
        CompanionStatus::Failed(CompanionFailure::Removal) => S::FontFailedRemoval,
    };
    s(key, language).into()
}

/// What pressing Download does, said before it is pressed: the host, the
/// size, the licence, where the file goes, and that no document is sent.
fn cjk_font_disclosure(language: Language) -> SharedString {
    let megabytes = companion_font::SIZE as f64 / 1_000_000.0;
    let (host, license) = (companion_font::HOST, companion_font::LICENSE);
    match language {
        Language::English => format!(
            "Source Han Sans CN by Adobe pairs Chinese text with the app's own type. \
             Downloading it fetches about {megabytes:.1} MB from {host} ({license}) once, \
             and keeps it in MarkTurbo's own data folder; nothing is installed for the \
             rest of the system. No document content is sent."
        ),
        Language::Chinese => format!(
            "Adobe 思源黑体 CN 让中文与应用自带字体更协调。下载时会从 {host} 获取约 \
             {megabytes:.1} MB（{license}），仅保存在 MarkTurbo 自己的数据目录中，\
             不会安装到系统。不会发送任何文档内容。"
        ),
    }
    .into()
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
        .child(
            view()
                .max_w(px(MEASURE_WIDE))
                .child(prims::faint(i18n::t(language, Key::UsageDisclosure))),
        )
        .child(choice_row(
            "usage-toggle",
            i18n::t(language, Key::UsageCounters),
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
                .child(prims::action(
                    "usage-inspect",
                    move || {
                        let language = inspection_state.settings.get().language;
                        if inspection_state.usage_inspection_open.get() {
                            SharedString::from(i18n::t(language, Key::UsageHideDetails))
                        } else {
                            SharedString::from(i18n::t(language, Key::UsageInspect))
                        }
                    },
                    prims::ActionTone::Quiet,
                    prims::ActionOptions::default(),
                    move || inspect_state.inspect_usage(),
                ))
                .child(prims::quiet_action(
                    "usage-export",
                    i18n::t(language, Key::UsageExport),
                    move || export_state.export_usage(),
                ))
                .child(prims::danger_action(
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
        .role(AccessibilityRole::Radiogroup)
        .optional_label(Some(SharedString::from(label)))
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
        row = row.child(prims::action(
            id,
            name,
            prims::ActionTone::Quiet,
            prims::ActionOptions {
                chosen: Some(
                    (move || read(&current_state.settings.get()) == value).into_reactive(),
                ),
                ..Default::default()
            },
            move || {
                if read(&press_state.settings.get()) != value {
                    set(&press_state, value);
                }
            },
        ));
    }
    row
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
    ChineseFont,
    DownloadAndApply,
    Retry,
    FontAbsent,
    FontDownloading,
    FontApplying,
    FontInstalled,
    FontRemoving,
    FontRemoved,
    FontFailedNetwork,
    FontFailedVerification,
    FontFailedDisk,
    FontFailedRegistration,
    FontFailedRemoval,
}

fn s(key: S, language: Language) -> &'static str {
    match language {
        Language::English => match key {
            S::Endpoint => "Endpoint",
            S::BaseUrlHint => {
                "Leave empty for the default endpoint. Remote hosts require HTTPS; plain HTTP \
                 is allowed only for numeric loopback addresses (e.g. 127.0.0.1:11434/v1)."
            }
            S::ProviderDefault => "Wire format default",
            S::TargetLanguagePlaceholder => "zh",
            S::TargetLanguageHint => "A language name or code, e.g. zh, ja, German.",
            S::ReplacementKey => "Enter API key or token",
            S::NoKeySaved => "No credential stored",
            S::KeySavedStored => "Credential stored securely for this endpoint",
            S::KeySavedSession => "Key saved for this session only",
            S::KeyFromEnvironment => "Key taken from the environment",
            S::Replace => "Replace",
            S::Remove => "Remove",
            S::ChineseFont => "Chinese font",
            S::DownloadAndApply => "Download and apply",
            S::Retry => "Retry",
            S::FontAbsent => "Not installed. Chinese text uses the system's font.",
            S::FontDownloading => "Downloading",
            S::FontApplying => "Applying…",
            S::FontInstalled => "Installed and applied: Chinese text is set in Source Han Sans CN.",
            S::FontRemoving => "Removing…",
            S::FontRemoved => {
                "Removed. Chinese text keeps Source Han Sans CN until MarkTurbo restarts."
            }
            S::FontFailedNetwork => {
                "Download failed: could not reach the font server. Nothing was installed \
                 — check your connection and press Retry."
            }
            S::FontFailedVerification => {
                "Download failed: the file was not the expected font, so it was discarded."
            }
            S::FontFailedDisk => {
                "The font file could not be saved or read back in MarkTurbo's data folder."
            }
            S::FontFailedRegistration => "The font could not be registered and was not applied.",
            S::FontFailedRemoval => {
                "Removal failed: the font could not be deleted from MarkTurbo's data folder."
            }
        },
        Language::Chinese => match key {
            S::Endpoint => "端点",
            S::BaseUrlHint => {
                "留空则使用默认端点。远程主机必须使用 HTTPS；仅数字回环地址（如 127.0.0.1:11434/v1）可使用 HTTP。"
            }
            S::ProviderDefault => "协议格式默认值",
            S::TargetLanguagePlaceholder => "zh",
            S::TargetLanguageHint => "语言名称或代码，例如 zh、ja、German。",
            S::ReplacementKey => "输入 API 密钥或令牌",
            S::NoKeySaved => "未存储凭据",
            S::KeySavedStored => "此端点的凭据已安全存储",
            S::KeySavedSession => "密钥仅在本次会话中有效",
            S::KeyFromEnvironment => "密钥来自环境变量",
            S::Replace => "替换",
            S::Remove => "移除",
            S::ChineseFont => "中文字体",
            S::DownloadAndApply => "下载并应用",
            S::Retry => "重试",
            S::FontAbsent => "未安装。中文使用系统字体。",
            S::FontDownloading => "正在下载",
            S::FontApplying => "正在应用…",
            S::FontInstalled => "已安装并应用：中文使用思源黑体 CN。",
            S::FontRemoving => "正在移除…",
            S::FontRemoved => "已移除。重启 MarkTurbo 前，中文仍使用思源黑体 CN。",
            S::FontFailedNetwork => {
                "下载失败：无法连接字体服务器。未安装任何内容——请检查网络连接后点击“重试”。"
            }
            S::FontFailedVerification => "下载失败：文件与预期字体不符，已丢弃。",
            S::FontFailedDisk => "无法在 MarkTurbo 的数据目录中保存或读回字体文件。",
            S::FontFailedRegistration => "字体注册失败，未能应用。",
            S::FontFailedRemoval => "移除失败：无法从 MarkTurbo 的数据目录中删除字体。",
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
            atomic::{AtomicBool, AtomicUsize, Ordering},
        },
    };

    use crate::ui::motion;
    use mt_core::credentials::{CredentialError, Secret, SecureCredentialStore};

    /// One choice, current, and a line that counts the presses it hears.
    #[component]
    fn ChoiceHarness() -> impl IntoElement {
        let presses = signal(0u32);
        view()
            .flex_col()
            .child(prims::action(
                "harness-choice",
                "Light",
                prims::ActionTone::Quiet,
                prims::ActionOptions {
                    chosen: Some((|| true).into_reactive()),
                    ..Default::default()
                },
                move || presses.update(|n| *n += 1),
            ))
            .child(view().test_id("harness-presses").child(prims::ink(move || {
                SharedString::from(format!("{} presses", presses.get()))
            })))
    }

    #[component]
    fn MountedTransitionProbe() -> impl IntoElement {
        let shown = signal(false);
        view()
            .flex_col()
            .child(prims::primary_action("probe-show", "Show", move || {
                shown.set(true)
            }))
            .child(when(
                move || shown.get(),
                move || {
                    view()
                        .flex_col()
                        .child(prims::quiet_action("probe-quiet", "Quiet", || {}))
                        .child(prims::action(
                            "probe-choice",
                            "Choice",
                            prims::ActionTone::Quiet,
                            prims::ActionOptions {
                                chosen: Some((|| true).into_reactive()),
                                ..Default::default()
                            },
                            || {},
                        ))
                        .into_element()
                },
            ))
    }

    #[test]
    fn an_actions_label_colour_transition_uses_the_ease_out_curve() {
        use std::sync::{Arc, Mutex};

        use nsh_core::MountItem;

        let mut app = nsh_testing::Headless::start(nsh_engine::EngineConfig::new(
            nsh_testing::windowed_root::<MountedTransitionProbe>(),
        ))
        .expect("start the harness");
        app.wait_for("id:probe-show").expect("the harness");
        let batches = Arc::new(Mutex::new(Vec::<MountItem>::new()));
        app.observe({
            let batches = batches.clone();
            move |_, batch| {
                batches
                    .lock()
                    .expect("lock the batch record")
                    .extend(batch.items.iter().cloned());
            }
        });
        app.press("id:probe-show").expect("mount the labels");
        app.wait_for("id:probe-choice").expect("the choice");
        let batches = batches.lock().expect("lock the batch record");
        for text in ["Quiet", "Choice"] {
            let label = batches
                .iter()
                .find_map(|item| match item {
                    MountItem::Text {
                        id, text: content, ..
                    } if content.text == text => Some(*id),
                    _ => None,
                })
                .unwrap_or_else(|| panic!("the {text} label's text node"));
            let transition = batches
                .iter()
                .find_map(|item| match item {
                    MountItem::ViewStyle { id, style } if *id == label => style.transition,
                    _ => None,
                })
                .unwrap_or_else(|| panic!("the {text} label's colour transition"));
            assert_eq!(
                transition.easing,
                [0.23, 1.0, 0.32, 1.0],
                "{text} must ease out, not ride nsh's default curve"
            );
            assert_eq!(transition.duration, motion::HOVER_MS, "{text} hover timing");
        }
    }

    #[test]
    fn a_choices_test_id_is_on_the_button_itself() {
        let mut app = nsh_testing::Headless::start(nsh_engine::EngineConfig::new(
            nsh_testing::windowed_root::<ChoiceHarness>(),
        ))
        .expect("start the harness");
        app.wait_for("id:harness-presses").expect("the harness");
        let node = app
            .screen()
            .layout_nodes()
            .into_iter()
            .find(|node| node.test_id.as_deref() == Some("harness-choice"))
            .expect("the choice")
            .id;
        for selector in ["id:harness-choice", "role:button[Light]"] {
            assert_eq!(
                app.screen().press_target(selector).expect("selector"),
                Some(node),
                "`{selector}` is the control carrying the id"
            );
        }
        app.press("id:harness-choice").expect("press the choice");
        app.wait_for("text:1 presses").expect("the press heard");
    }

    /// The vault a test drives: credentials live in this map and never in the
    /// platform's secure store, and every read is counted, so a test can prove
    /// the page does not read the store per keystroke. `fail_read` makes the
    /// next reads fail the way a contended store does.
    #[derive(Default)]
    struct CountingStore {
        values: Mutex<HashMap<String, Secret>>,
        reads: AtomicUsize,
        fail_read: AtomicBool,
        fail_write: AtomicBool,
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

        /// Store `secret` for `target` behind the page's back, as another
        /// instance of the app would.
        fn store_externally(&self, target: &str, secret: &str) {
            self.values
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .insert(target.to_owned(), Secret::new(secret).expect("a secret"));
        }

        /// The one error a test can build with the public API: `Secret::new`
        /// refuses an empty value, and the status cares only that the store
        /// failed.
        fn an_error() -> CredentialError {
            Secret::new("").expect_err("an empty secret is refused")
        }
    }

    impl SecureCredentialStore for CountingStore {
        fn is_supported(&self) -> bool {
            true
        }

        fn read(&self, target: &str) -> Result<Option<Secret>, CredentialError> {
            self.reads.fetch_add(1, Ordering::Relaxed);
            if self.fail_read.load(Ordering::Relaxed) {
                return Err(Self::an_error());
            }
            Ok(self
                .values
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .get(target)
                .cloned())
        }

        fn write(&self, target: &str, secret: &Secret) -> Result<(), CredentialError> {
            if self.fail_write.load(Ordering::Relaxed) {
                return Err(Self::an_error());
            }
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

    struct NoSecureStore;

    impl SecureCredentialStore for NoSecureStore {
        fn is_supported(&self) -> bool {
            false
        }
        fn read(&self, _: &str) -> Result<Option<Secret>, CredentialError> {
            Ok(None)
        }
        fn write(&self, _: &str, _: &Secret) -> Result<(), CredentialError> {
            Ok(())
        }
        fn delete(&self, _: &str) -> Result<(), CredentialError> {
            Ok(())
        }
    }

    #[component]
    fn CredentialErrorProbe() -> impl IntoElement {
        let configure = |state: &AppState| {
            state.settings.update(|settings| {
                settings.model_provider = "openai-chat".to_owned();
                settings.model_base_url = "http://127.0.0.1:11434/v1".to_owned();
            });
        };
        let unsupported = AppState::with_vault(
            mt_core::credentials::CredentialVault::with_test_store(Arc::new(NoSecureStore)),
        );
        configure(&unsupported);
        let unsupported_status = credential_status_text(&unsupported, Language::English);
        let unsupported_status_chinese = credential_status_text(&unsupported, Language::Chinese);
        let unsupported_error = unsupported
            .store_persistent_credential("synthetic")
            .expect_err("an unsupported store refuses a secure write");
        unsupported
            .settings
            .update(|settings| settings.language = Language::Chinese);
        let unsupported_error_chinese = unsupported
            .store_persistent_credential("synthetic")
            .expect_err("an unsupported store refuses a secure write in Chinese");

        let failing = Arc::new(CountingStore::default());
        failing.fail_write.store(true, Ordering::Relaxed);
        let failing_state = AppState::with_vault(
            mt_core::credentials::CredentialVault::with_test_store(failing),
        );
        configure(&failing_state);
        let write_error = failing_state
            .store_persistent_credential("synthetic")
            .expect_err("a failing store reports the write refusal");
        failing_state
            .settings
            .update(|settings| settings.language = Language::Chinese);
        let write_error_chinese = failing_state
            .store_persistent_credential("synthetic")
            .expect_err("a failing store reports the write refusal in Chinese");

        let unreadable = Arc::new(CountingStore::default());
        unreadable.fail_read.store(true, Ordering::Relaxed);
        let unreadable_state = AppState::with_vault(
            mt_core::credentials::CredentialVault::with_test_store(unreadable),
        );
        configure(&unreadable_state);
        let read_error = credential_status_text(&unreadable_state, Language::English);
        let read_error_chinese = credential_status_text(&unreadable_state, Language::Chinese);

        view()
            .test_id("credential-errors")
            .child(prims::ink(SharedString::from(format!(
                "A:{unsupported_status}|B:{unsupported_error}|C:{write_error}\
                 |D:{unsupported_status_chinese}|E:{unsupported_error_chinese}|F:{write_error_chinese}\
                 |G:{read_error}|H:{read_error_chinese}"
            ))))
    }

    #[test]
    fn credential_failures_are_localized_with_a_next_step() {
        let mut app = nsh_testing::Headless::start(nsh_engine::EngineConfig::new(
            nsh_testing::windowed_root::<CredentialErrorProbe>(),
        ))
        .expect("start the probe");
        app.wait_for("id:credential-errors")
            .expect("the probe's line");
        let summary = app
            .screen()
            .texts()
            .into_iter()
            .find(|text| text.starts_with("A:"))
            .expect("the probe's line");
        assert_eq!(
            summary,
            "A:OS keychain storage is unavailable on this system. Use \"Use for session\" or set an environment variable.\
             |B:OS keychain storage is unavailable on this system. Use \"Use for session\" or set an environment variable.\
             |C:Could not save to secure storage. Use \"Use for session\" instead, or check your OS keychain permissions.\
             |D:当前系统不支持安全凭据存储。请点击“仅本次会话使用”或配置环境变量。\
             |E:当前系统不支持安全凭据存储。请点击“仅本次会话使用”或配置环境变量。\
             |F:无法写入系统安全存储。请改用“仅本次会话使用”，或检查系统凭据管理器权限。\
             |G:The secure credential could not be read. Retry, or use \"Use for session\" or an environment variable.\
             |H:无法读取安全存储的凭据。请重试，或改用“仅本次会话使用”或环境变量。"
        );
        assert!(
            !summary.contains("synthetic"),
            "a credential error echoed the key: {summary}"
        );
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

    /// The probe's root: the real shell around the page, so the probe opens
    /// Settings the way a person does — through the title row and its menu — and
    /// can close it again. Its vault is the probe's own in-memory store, never
    /// the platform's.
    #[component]
    fn SettingsProbe() -> impl IntoElement {
        crate::ui::fonts::register();
        let state = AppState::with_vault(mt_core::credentials::CredentialVault::with_test_store(
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
            .child(crate::ui::shell::shell(
                state,
                Language::English,
                tokens::Layout::of(1440.0),
            ))
    }

    /// The marker the process below passes to the one it spawns: every
    /// `MARKTURBO_*` path variable is process-global, and the lib test binary
    /// also runs consent's and context's app probes in parallel threads, so this
    /// probe only ever touches a process it owns.
    const PROBE_CHILD: &str = "MARKTURBO_SETTINGS_PROBE_CHILD";
    const SAVE_FAILURE_PROBE_CHILD: &str = "MARKTURBO_SETTINGS_SAVE_FAILURE_PROBE_CHILD";

    /// Runs the probe in a process of its own, with the temp paths in that
    /// process's environment. Nothing here calls `set_var`, so the shared test
    /// binary never has its own environment rewritten under another test.
    #[test]
    fn the_settings_probe_runs_in_its_own_process() {
        if std::env::var_os(PROBE_CHILD).is_some() {
            // The child is running the probe below right now.
            return;
        }
        let config = tempfile::tempdir().expect("config dir");
        let output = std::process::Command::new(std::env::current_exe().expect("the test binary"))
            .args([
                "--exact",
                "ui::settings::tests::the_page_follows_the_endpoint_and_manages_the_keys",
            ])
            .env_remove("MARKTURBO_OPEN")
            .env(PROBE_CHILD, "1")
            .env("MARKTURBO_CONFIG_DIR", config.path())
            .env("MARKTURBO_DATA_DIR", config.path().join("data"))
            .output()
            .expect("run the settings probe");
        assert!(
            output.status.success(),
            "the settings probe failed:\n{}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }

    #[test]
    fn closing_settings_keeps_the_page_open_when_saving_fails() {
        if std::env::var_os(SAVE_FAILURE_PROBE_CHILD).is_none() {
            let output = std::process::Command::new(
                std::env::current_exe().expect("the test binary"),
            )
            .args([
                "--exact",
                "ui::settings::tests::closing_settings_keeps_the_page_open_when_saving_fails",
            ])
            .env_remove("MARKTURBO_OPEN")
            .env(SAVE_FAILURE_PROBE_CHILD, "1")
            .output()
            .expect("run the settings save-failure probe");
            assert!(
                output.status.success(),
                "the save-failure probe failed:\n{}{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
            return;
        }

        let mut config = nsh_engine::EngineConfig::new(
            nsh_testing::windowed_root::<SettingsProbe>().tokens(tokens::theme()),
        );
        config.environment.color_scheme = nsh_core::platform::ColorScheme::Light;
        let mut app = nsh_testing::Headless::start(config).expect("start Settings");
        app.set_viewport(nsh_core::Size::new(px(1440.0), px(900.0)))
            .expect("the Settings window");
        app.press("id:title-settings").expect("open the title menu");
        app.wait_for("id:title-menu-settings")
            .expect("the Settings menu item");
        app.press("id:title-menu-settings").expect("open Settings");
        app.wait_for("id:settings-page").expect("the Settings page");
        let settings_path = mt_core::settings::settings_path().expect("the hermetic settings path");
        std::fs::create_dir(&settings_path).expect("block the settings file with a directory");
        app.press("id:settings-close").expect("close Settings");
        app.wait_for("id:settings-page >> text:Could not save settings. Check that the settings file is writable, then try again.")
            .expect("the failed save is shown in Settings");
        assert!(
            app.screen()
                .matches("id:settings-page")
                .expect("Settings page"),
            "Settings stays open after a failed save"
        );
        assert_eq!(
            app.screen()
                .span_style("text:Could not save settings. Check that the settings file is writable, then try again.")
                .expect("the Settings save error")
                .color,
            tokens::LIGHT.danger
        );
    }

    /// Drives the app in the headless harness: the status line follows the wire
    /// format, the line under the base URL follows what is typed, the secure
    /// store is read for an endpoint change and not for anything else, a failure
    /// to read it is retried rather than remembered, the stored and session keys
    /// are forgotten again, and a key another instance stores appears when the
    /// page is opened again. Kept because the real window cannot be driven on
    /// this host (a disconnected session takes no synthetic input).
    #[test]
    fn the_page_follows_the_endpoint_and_manages_the_keys() {
        if std::env::var_os(PROBE_CHILD).is_none() {
            // Only the process its sibling above spawns drives the app: run
            // directly, this has no temp paths of its own to lay the app down
            // in.
            return;
        }
        // The store fails until the probe has seen the failure: a contended
        // store must show up and must not be remembered.
        PROBE_STORE.fail_read.store(true, Ordering::Relaxed);
        let mut app = nsh_testing::Headless::start(nsh_engine::EngineConfig::new(
            nsh_testing::windowed_root::<SettingsProbe>(),
        ))
        .expect("start the app");
        app.set_viewport(nsh_core::Size::new(
            nsh_core::Points(1440.0),
            nsh_core::Points(900.0),
        ))
        .expect("viewport");
        // The gear opens the row's menu; its Settings row opens the page.
        app.press("id:title-settings").expect("open the menu");
        app.wait_for("id:title-menu-settings").expect("the menu");
        app.press("id:title-menu-settings").expect("open settings");
        app.wait_for("id:settings-page").expect("the page");
        app.wait_for(
            "text:The secure credential could not be read. Retry, or use \"Use for session\" or an environment variable.",
        )
        .expect("the failed read is shown");

        // The failure was a moment, not a state: an unrelated change re-reads
        // the vault and the status recovers without the endpoint moving.
        PROBE_STORE.fail_read.store(false, Ordering::Relaxed);
        app.clear("id:settings-model")
            .expect("clear the model field");
        app.type_text("id:settings-model", "probe-model")
            .expect("type a model");
        app.wait_for("text:No credential stored")
            .expect("the status recovered");

        // A remote HTTP endpoint is refused, and the line under the field says
        // why.
        app.clear("id:settings-base-url").expect("clear the field");
        app.type_text("id:settings-base-url", "http://example.com/v1")
            .expect("type a base URL");
        app.wait_for("text:HTTPS is required for remote hosts. For a local server on this machine, use a numeric loopback address such as http://127.0.0.1:11434/v1.")
            .expect("validation error");

        // A numeric loopback URL over HTTP is the one plain-HTTP case the core
        // takes: the error line goes back to the hint.
        app.clear("id:settings-base-url").expect("clear the field");
        app.type_text("id:settings-base-url", "http://127.0.0.1:8000/v1")
            .expect("type a loopback URL");
        app.wait_for_gone(
            "text:HTTPS is required for remote hosts. For a local server on this machine, use a numeric loopback address such as http://127.0.0.1:11434/v1.",
            std::time::Duration::from_secs(5),
        )
        .expect("validation error gone");

        // The wire format is a choice, and the status line follows it. A custom
        // endpoint never consults the provider's environment variable, so the
        // status is this process's own.
        app.press("text:Best available")
            .expect("choose best available");
        app.wait_for(
            "text:Choose a wire format above (not Best available) to configure its credential.",
        )
        .expect("unconfigured status");
        app.press("text:OpenAI Chat Completions")
            .expect("choose a wire format");
        app.wait_for("text:No credential stored")
            .expect("configured status");

        // A choice that is now the current one stays the same control: pressing
        // it again is still a press (a keyboard user keeps the focus) and does
        // nothing — not even re-saving the settings.
        app.press("text:OpenAI Chat Completions")
            .expect("the current wire format is still a control");
        app.press("text:English")
            .expect("the current language is still a control");
        app.wait_for("text:No credential stored")
            .expect("unchanged status");
        assert!(
            !app.screen().shows("Settings saved."),
            "pressing the current choice saved the settings again"
        );

        // The status is read from the vault for an endpoint change, and for
        // nothing else: typing in a field the status does not depend on, or
        // toggling the local counters, must not touch the secure store.
        PROBE_STORE.forget_reads();
        app.clear("id:settings-model")
            .expect("clear the model field");
        app.type_text("id:settings-model", "reads-check")
            .expect("type a model");
        app.wait_for("text:reads-check").expect("the model landed");
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
        app.wait_for("text:Credential stored securely for this endpoint")
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
        app.wait_for("text:Credential stored securely for this endpoint")
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
        app.wait_for("text:No credential stored")
            .expect("status after removal");
        assert_eq!(
            PROBE_STORE.stored(&target).as_deref(),
            Some("synthetic-replacement-key"),
            "Remove committed before its undo hold elapsed"
        );
        app.press("id:status-undo").expect("undo the removal");
        app.wait_for("text:Credential stored securely for this endpoint")
            .expect("the stored status is back");
        assert_eq!(
            PROBE_STORE.stored(&target).as_deref(),
            Some("synthetic-replacement-key"),
            "Undo lost the credential"
        );
        app.press("text:Remove")
            .expect("forget the stored key for good");
        app.wait_for("text:No credential stored")
            .expect("status after the second removal");
        app.wait(std::time::Duration::from_millis(
            crate::ui::motion::UNDO_HOLD_MS + 500,
        ))
        .expect("the hold elapses");
        assert_eq!(
            PROBE_STORE.stored(&target),
            None,
            "the delete never committed after the hold"
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
        app.wait_for("text:No credential stored")
            .expect("status after removal");

        // A key another instance of the app stores appears when the page is
        // opened again: the status is not remembered past the page.
        app.press("text:Close").expect("close settings");
        app.wait_for_gone("id:settings-page", std::time::Duration::from_secs(5))
            .expect("the page closed");
        PROBE_STORE.store_externally(&target, "external-key");
        app.press("id:title-settings").expect("open the menu again");
        app.wait_for("id:title-menu-settings").expect("the menu");
        app.press("id:title-menu-settings")
            .expect("open settings again");
        app.wait_for("text:Credential stored securely for this endpoint")
            .expect("the status read the store afresh");
    }
}
