//! Model endpoint, credential, and Translation settings.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, Icon, IconName, Sizable as _,
    button::{Button, ButtonVariants as _},
    h_flex,
    input::{Input, InputContentType, InputState},
    setting::{SettingField, SettingGroup, SettingItem, SettingPage},
    v_flex,
};
use gpui_kit::*;

use crate::credentials::{
    CredentialError, CredentialErrorKind, CredentialVault, LegacyCredentialMigrationError,
    remove_migrated_legacy_credential, secure_legacy_credential,
};
use crate::i18n::{self, Key};
use crate::model::{
    EndpointIdentity, EndpointIdentityError, Provider, endpoint_input_is_safe_to_persist,
};
use crate::settings::{self, AppSettings};

use super::settings_page::{SettingsEvent, SettingsView, write};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CredentialOperation {
    Store,
    Delete,
    Migrate,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CredentialPresence {
    ProviderRequired,
    InvalidEndpoint,
    SecureStoreUnavailable,
    Present,
    Absent,
    Error(Key),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct CredentialNotice {
    key: Key,
    error: bool,
}

enum ConfiguredEndpoint {
    ProviderRequired,
    UnsupportedProvider,
    Invalid(EndpointIdentityError),
    Ready(EndpointIdentity),
}

/// Model-specific state kept out of the general settings page.
pub(super) struct ModelSettings {
    model_base_url_draft: Entity<InputState>,
    _model_base_url_subscription: Subscription,
    credential_draft: Entity<InputState>,
    _credential_subscription: Subscription,
    credential_operation: Option<CredentialOperation>,
    credential_presence: CredentialPresence,
    credential_notice: Option<CredentialNotice>,
}

impl ModelSettings {
    pub(super) fn new(window: &mut Window, cx: &mut Context<SettingsView>) -> Self {
        let model_base_url = AppSettings::global(cx).model_base_url.clone();
        let model_base_url = if endpoint_input_is_safe_to_persist(&model_base_url) {
            model_base_url
        } else {
            String::new()
        };
        let model_base_url_draft =
            cx.new(|cx| InputState::new(window, cx).default_value(model_base_url));
        let model_base_url_subscription = cx.observe(&model_base_url_draft, |_, _, cx| cx.notify());
        let credential_draft = cx.new(|cx| InputState::new(window, cx).masked(true));
        let credential_subscription = cx.observe(&credential_draft, |_, _, cx| cx.notify());
        let mut this = Self {
            model_base_url_draft,
            _model_base_url_subscription: model_base_url_subscription,
            credential_draft,
            _credential_subscription: credential_subscription,
            credential_operation: None,
            credential_presence: CredentialPresence::ProviderRequired,
            credential_notice: None,
        };
        this.refresh_credential_presence(cx);
        this
    }

    /// Shared model configuration and Translation-specific preferences.
    pub(super) fn translation(&self, this: &Entity<SettingsView>, cx: &App) -> SettingPage {
        // Every schema is listed, not only the ones with a key present: the
        // point of choosing one is often to configure it, and a dropdown that
        // hides the option until its environment variable exists gives the user
        // nowhere to start. The description names the variable each needs.
        let mut provider_options: Vec<(SharedString, SharedString)> =
            vec![("".into(), i18n::t(Key::ProviderBestAvailable, cx).into())];
        provider_options.extend(
            Provider::ALL
                .into_iter()
                .map(|p| (p.key().into(), p.label().into())),
        );

        let controls_disabled = self.credential_operation.is_some();
        let has_unsaved_endpoint = endpoint_draft_changed(
            AppSettings::global(cx),
            self.model_base_url_draft.read(cx).value().as_ref(),
        );
        let mut model_items = vec![
            SettingItem::new(
                i18n::t(Key::Provider, cx),
                SettingField::dropdown(
                    provider_options,
                    |cx: &App| AppSettings::global(cx).model_provider.clone().into(),
                    write_model(this, |value, settings| {
                        settings.model_provider = value.to_string()
                    }),
                ),
            )
            .description(i18n::t(Key::ProviderHelp, cx))
            .disabled(controls_disabled),
            SettingItem::new(i18n::t(Key::BaseUrl, cx), self.model_base_url_control(this))
                .description(i18n::t(Key::BaseUrlHelp, cx))
                .disabled(controls_disabled),
            SettingItem::new(
                i18n::t(Key::Model, cx),
                SettingField::input(
                    |cx: &App| AppSettings::global(cx).model_name.clone().into(),
                    write(|value, settings| settings.model_name = value.to_string()),
                ),
            )
            .description(i18n::t(Key::ModelHelp, cx))
            .disabled(controls_disabled),
            SettingItem::new(
                i18n::t(Key::EndpointStatus, cx),
                SettingField::render(|_, _, cx| {
                    let (message, error) = endpoint_status(cx);
                    div()
                        .w_full()
                        .text_sm()
                        .text_color(if error {
                            cx.theme().danger
                        } else {
                            cx.theme().muted_foreground
                        })
                        .child(message)
                }),
            ),
        ];

        if matches!(
            configured_endpoint(AppSettings::global(cx)),
            ConfiguredEndpoint::Ready(ref endpoint) if !endpoint.is_vendor_default()
        ) {
            let settings_view = this.downgrade();
            model_items.push(
                SettingItem::new(
                    i18n::t(Key::EnvironmentCredential, cx),
                    SettingField::switch(
                        |cx: &App| environment_credential_authorized(AppSettings::global(cx)),
                        move |value, cx| {
                            let failed = AppSettings::try_update(cx, |settings| {
                                set_environment_credential_authorization(settings, value)
                            })
                            .is_err();
                            let _ = settings_view.update(cx, |view, cx| {
                                view.model.credential_notice = failed.then_some(CredentialNotice {
                                    key: Key::EnvironmentCredentialAuthorizationSaveFailed,
                                    error: true,
                                });
                                cx.notify();
                            });
                        },
                    ),
                )
                .description(environment_credential_help(cx))
                .disabled(controls_disabled || has_unsaved_endpoint),
            );
        }

        let mut credential_items = vec![
            SettingItem::new(i18n::t(Key::Credential, cx), self.credential_controls(this))
                .description(i18n::t(Key::CredentialHelp, cx)),
        ];

        if AppSettings::global(cx).legacy_model_api_key().is_some() {
            let this = this.downgrade();
            credential_items.push(
                SettingItem::new(
                    i18n::t(Key::LegacyCredential, cx),
                    SettingField::render(move |options, _, cx| {
                        let pending = this.upgrade().is_some_and(|this| {
                            this.read(cx).model.credential_operation
                                == Some(CredentialOperation::Migrate)
                        });
                        let disabled = this.upgrade().is_none_or(|this| {
                            let this = this.read(cx);
                            this.model.credential_operation.is_some()
                                || endpoint_draft_changed(
                                    AppSettings::global(cx),
                                    this.model.model_base_url_draft.read(cx).value().as_ref(),
                                )
                                || !CredentialVault::global(cx).secure_store_supported()
                                || !matches!(
                                    configured_endpoint(AppSettings::global(cx)),
                                    ConfiguredEndpoint::Ready(_)
                                )
                        });
                        Button::new("migrate-legacy-model-credential")
                            .label(i18n::t(Key::MigrateLegacyCredential, cx))
                            .with_size(options.size())
                            .outline()
                            .loading(pending)
                            .disabled(disabled)
                            .on_click({
                                let this = this.clone();
                                move |_, window, cx| {
                                    let _ = this.update(cx, |this, cx| {
                                        this.model.confirm_legacy_migration(window, cx)
                                    });
                                }
                            })
                    }),
                )
                .description(i18n::t(Key::LegacyCredentialHelp, cx)),
            );
        }

        SettingPage::new(i18n::t(Key::Translation, cx))
            .icon(Icon::new(IconName::Globe))
            .groups(vec![
                SettingGroup::new()
                    .title(i18n::t(Key::ModelConfiguration, cx))
                    .items(model_items),
                SettingGroup::new()
                    .title(i18n::t(Key::Credentials, cx))
                    .items(credential_items),
                SettingGroup::new()
                    .title(i18n::t(Key::Translation, cx))
                    .item(
                        SettingItem::new(
                            i18n::t(Key::TargetLanguage, cx),
                            SettingField::input(
                                |cx: &App| AppSettings::global(cx).translate_to.clone().into(),
                                write(|value, settings| settings.translate_to = value.to_string()),
                            )
                            .default_value("zh"),
                        )
                        .description(i18n::t(Key::TargetLanguageHelp, cx)),
                    ),
            ])
    }

    fn credential_controls(&self, this: &Entity<SettingsView>) -> SettingField<SharedString> {
        let draft = self.credential_draft.clone();
        let base_url_draft = self.model_base_url_draft.clone();
        let this = this.downgrade();
        SettingField::render(move |options, _, cx| {
            let (operation, presence, notice) = this
                .upgrade()
                .map(|this| {
                    let this = this.read(cx);
                    (
                        this.model.credential_operation,
                        this.model.credential_presence,
                        this.model.credential_notice,
                    )
                })
                .unwrap_or((None, CredentialPresence::ProviderRequired, None));
            let endpoint_draft_changed = endpoint_draft_changed(
                AppSettings::global(cx),
                base_url_draft.read(cx).value().as_ref(),
            );
            let endpoint_ready = matches!(
                configured_endpoint(AppSettings::global(cx)),
                ConfiguredEndpoint::Ready(_)
            ) && !endpoint_draft_changed;
            let has_draft = !draft.read(cx).value().trim().is_empty();
            let busy = operation.is_some();
            let secure_store_supported = CredentialVault::global(cx).secure_store_supported();
            let (status_key, status_error) = if endpoint_draft_changed {
                (Key::SaveEndpointBeforeCredential, false)
            } else {
                credential_status(operation, presence, notice)
            };

            v_flex()
                .w_full()
                .gap_2()
                .child(
                    Input::new(&draft)
                        .content_type(InputContentType::Password)
                        .accessibility_id("model-credential-draft")
                        .aria_label(i18n::t(Key::Credential, cx))
                        .with_size(options.size())
                        .disabled(busy),
                )
                .child(
                    h_flex().w_full().flex_wrap().gap_2().children([
                        Button::new("store-model-credential")
                            .label(i18n::t(Key::StoreCredentialSecurely, cx))
                            .with_size(options.size())
                            .primary()
                            .loading(operation == Some(CredentialOperation::Store))
                            .disabled(
                                busy || !endpoint_ready || !secure_store_supported || !has_draft,
                            )
                            .on_click({
                                let this = this.clone();
                                move |_, window, cx| {
                                    let _ = this.update(cx, |this, cx| {
                                        this.model.store_credential(window, cx)
                                    });
                                }
                            }),
                        Button::new("use-model-credential-for-session")
                            .label(i18n::t(Key::UseCredentialForSession, cx))
                            .with_size(options.size())
                            .outline()
                            .disabled(busy || !endpoint_ready || !has_draft)
                            .on_click({
                                let this = this.clone();
                                move |_, window, cx| {
                                    let _ = this.update(cx, |this, cx| {
                                        this.model.use_credential_for_session(window, cx)
                                    });
                                }
                            }),
                        Button::new("test-model-credential")
                            .label(i18n::t(Key::TestCredential, cx))
                            .with_size(options.size())
                            .outline()
                            .disabled(busy || !endpoint_ready)
                            .on_click({
                                let this = this.clone();
                                move |_, _, cx| {
                                    let _ = this.update(cx, |this, cx| {
                                        this.model.request_credential_test(cx)
                                    });
                                }
                            }),
                        Button::new("delete-model-credential")
                            .label(i18n::t(Key::DeleteCredential, cx))
                            .with_size(options.size())
                            .danger()
                            .outline()
                            .loading(operation == Some(CredentialOperation::Delete))
                            .disabled(busy || !endpoint_ready)
                            .on_click({
                                let this = this.clone();
                                move |_, window, cx| {
                                    let _ = this.update(cx, |this, cx| {
                                        this.model.confirm_delete_credential(window, cx)
                                    });
                                }
                            }),
                    ]),
                )
                .child(
                    div()
                        .w_full()
                        .text_sm()
                        .text_color(if status_error {
                            cx.theme().danger
                        } else {
                            cx.theme().muted_foreground
                        })
                        .child(i18n::t(status_key, cx)),
                )
        })
    }

    fn model_base_url_control(&self, this: &Entity<SettingsView>) -> SettingField<SharedString> {
        let draft = self.model_base_url_draft.clone();
        let this = this.downgrade();
        SettingField::render(move |options, _, cx| {
            let busy = this
                .upgrade()
                .is_some_and(|this| this.read(cx).model.credential_operation.is_some());
            let changed =
                draft.read(cx).value().as_ref() != AppSettings::global(cx).model_base_url.as_str();
            h_flex()
                .w_full()
                .gap_2()
                .child(
                    Input::new(&draft)
                        .w_full()
                        .with_size(options.size())
                        .disabled(busy),
                )
                .child(
                    Button::new("save-model-base-url")
                        .label(i18n::t(Key::Save, cx))
                        .with_size(options.size())
                        .outline()
                        .disabled(busy || !changed)
                        .on_click({
                            let this = this.clone();
                            move |_, window, cx| {
                                let _ = this.update(cx, |this, cx| {
                                    this.model.apply_model_base_url(window, cx)
                                });
                            }
                        }),
                )
        })
    }

    fn apply_model_base_url(&mut self, window: &mut Window, cx: &mut Context<SettingsView>) {
        let value = self.model_base_url_draft.read(cx).value().to_string();
        let normalized = if value.trim().is_empty() {
            String::new()
        } else {
            let provider = Provider::from_key(AppSettings::global(cx).model_provider.trim())
                .unwrap_or(Provider::OpenAiChat);
            let endpoint = match EndpointIdentity::parse(provider, Some(&value)) {
                Ok(endpoint) => endpoint,
                Err(_) => {
                    let persisted = AppSettings::global(cx).model_base_url.clone();
                    let persisted = if endpoint_input_is_safe_to_persist(&persisted) {
                        persisted
                    } else {
                        String::new()
                    };
                    self.model_base_url_draft
                        .update(cx, |draft, cx| draft.set_value(persisted, window, cx));
                    self.set_credential_notice(Key::CredentialEndpointInvalid, true, cx);
                    return;
                }
            };
            endpoint.base_url().to_owned()
        };

        AppSettings::update(cx, |settings| settings.model_base_url = normalized.clone());
        self.model_base_url_draft
            .update(cx, |draft, cx| draft.set_value(normalized, window, cx));
        self.credential_notice = None;
        self.refresh_credential_presence(cx);
        cx.notify();
    }

    fn store_credential(&mut self, window: &mut Window, cx: &mut Context<SettingsView>) {
        let Some(target) = current_credential_target(cx) else {
            self.set_credential_notice(Key::ChooseProviderForCredential, true, cx);
            return;
        };
        let value = self.credential_draft.read(cx).value().to_string();
        if value.trim().is_empty() {
            self.set_credential_notice(Key::CredentialRequired, true, cx);
            return;
        }
        if !CredentialVault::global(cx).secure_store_supported() {
            self.set_credential_notice(Key::SecureStoreUnavailable, true, cx);
            return;
        }

        let vault = CredentialVault::global(cx).clone();
        self.credential_operation = Some(CredentialOperation::Store);
        self.credential_notice = None;
        cx.notify();
        cx.spawn_in(window, async move |this, cx| {
            let result = cx
                .background_spawn(async move { vault.replace_persistent(&target, value) })
                .await;
            loop {
                let result = result.clone();
                if crate::views::try_update_in(&this, cx, move |this, window, cx| {
                    this.model.credential_operation = None;
                    match result {
                        Ok(()) => {
                            this.model.clear_credential_draft(window, cx);
                            this.model.credential_notice = Some(CredentialNotice {
                                key: Key::CredentialStoredSecurely,
                                error: false,
                            });
                        }
                        Err(error) => {
                            this.model.credential_notice = Some(CredentialNotice {
                                key: credential_error_key(&error),
                                error: true,
                            });
                        }
                    }
                    this.model.refresh_credential_presence(cx);
                    cx.notify();
                })
                .is_some()
                {
                    break;
                }
                if this.upgrade().is_none() {
                    break;
                }
                cx.background_executor()
                    .timer(Duration::from_millis(1))
                    .await;
            }
        })
        .detach();
    }

    fn use_credential_for_session(&mut self, window: &mut Window, cx: &mut Context<SettingsView>) {
        let Some(target) = current_credential_target(cx) else {
            self.set_credential_notice(Key::ChooseProviderForCredential, true, cx);
            return;
        };
        let value = self.credential_draft.read(cx).value().to_string();
        match CredentialVault::global(cx).replace_session(target, value) {
            Ok(()) => {
                self.clear_credential_draft(window, cx);
                self.set_credential_notice(Key::CredentialSessionActive, false, cx);
            }
            Err(error) => self.set_credential_notice(credential_error_key(&error), true, cx),
        }
    }

    fn request_credential_test(&mut self, cx: &mut Context<SettingsView>) {
        if current_credential_target(cx).is_none() {
            self.set_credential_notice(Key::ChooseProviderForCredential, true, cx);
            return;
        }
        self.set_credential_notice(Key::CredentialTestRequested, false, cx);
        cx.emit(SettingsEvent::TestModelCredential);
    }

    fn confirm_delete_credential(&mut self, window: &mut Window, cx: &mut Context<SettingsView>) {
        let ConfiguredEndpoint::Ready(endpoint) = configured_endpoint(AppSettings::global(cx))
        else {
            self.set_credential_notice(Key::ChooseProviderForCredential, true, cx);
            return;
        };
        let target = endpoint.credential_target().as_str().to_owned();
        let title = i18n::t(Key::DeleteCredentialTitle, cx);
        let description = i18n::delete_model_credential_description(&endpoint, cx);
        let answer = window.prompt(
            PromptLevel::Warning,
            title,
            Some(&description),
            &[
                PromptButton::new(i18n::t(Key::DeleteCredential, cx)),
                PromptButton::cancel(i18n::t(Key::Cancel, cx)),
            ],
            cx,
        );
        let vault = CredentialVault::global(cx).clone();
        self.credential_operation = Some(CredentialOperation::Delete);
        cx.notify();
        cx.spawn_in(window, async move |this, cx| {
            if answer.await.unwrap_or(1) != 0 {
                loop {
                    if crate::views::try_update_in(&this, cx, |this, _, cx| {
                        this.model.credential_operation = None;
                        cx.notify();
                    })
                    .is_some()
                    {
                        break;
                    }
                    if this.upgrade().is_none() {
                        break;
                    }
                    cx.background_executor()
                        .timer(Duration::from_millis(1))
                        .await;
                }
                return;
            }
            let result = cx
                .background_spawn(async move { vault.delete(&target) })
                .await;
            loop {
                let result = result.clone();
                if crate::views::try_update_in(&this, cx, move |this, _, cx| {
                    this.model.credential_operation = None;
                    this.model.credential_notice = Some(match result {
                        Ok(()) => CredentialNotice {
                            key: Key::CredentialDeleted,
                            error: false,
                        },
                        Err(error) => CredentialNotice {
                            key: credential_error_key(&error),
                            error: true,
                        },
                    });
                    this.model.refresh_credential_presence(cx);
                    cx.notify();
                })
                .is_some()
                {
                    break;
                }
                if this.upgrade().is_none() {
                    break;
                }
                cx.background_executor()
                    .timer(Duration::from_millis(1))
                    .await;
            }
        })
        .detach();
    }

    pub(super) fn confirm_legacy_migration(
        &mut self,
        window: &mut Window,
        cx: &mut Context<SettingsView>,
    ) {
        let ConfiguredEndpoint::Ready(endpoint) = configured_endpoint(AppSettings::global(cx))
        else {
            self.set_credential_notice(Key::ChooseProviderForCredential, true, cx);
            return;
        };
        let Some(path) = settings::settings_path() else {
            self.set_credential_notice(Key::SettingsPathUnavailable, true, cx);
            return;
        };
        let target = endpoint.credential_target().as_str().to_owned();
        let title = i18n::t(Key::MigrateLegacyCredentialTitle, cx);
        let description = i18n::migrate_legacy_credential_description(&endpoint, cx);
        let answer = window.prompt(
            PromptLevel::Warning,
            title,
            Some(&description),
            &[
                PromptButton::ok(i18n::t(Key::MigrateLegacyCredential, cx)),
                PromptButton::cancel(i18n::t(Key::Cancel, cx)),
            ],
            cx,
        );
        let vault = CredentialVault::global(cx).clone();
        let legacy_settings = AppSettings::global(cx).clone();
        self.credential_operation = Some(CredentialOperation::Migrate);
        cx.notify();
        cx.spawn_in(window, async move |this, cx| {
            if answer.await.unwrap_or(1) != 0 {
                loop {
                    if crate::views::try_update_in(&this, cx, |this, _, cx| {
                        this.model.credential_operation = None;
                        cx.notify();
                    })
                    .is_some()
                    {
                        break;
                    }
                    if this.upgrade().is_none() {
                        break;
                    }
                    cx.background_executor()
                        .timer(Duration::from_millis(1))
                        .await;
                }
                return;
            }
            let result = cx
                .background_spawn(async move {
                    secure_legacy_credential(&legacy_settings, &vault, &target)
                })
                .await;
            let result = Arc::new(Mutex::new(Some(result)));
            loop {
                let result = result.clone();
                let path = path.clone();
                if crate::views::try_update_in(&this, cx, move |this, _, cx| {
                    let result = result
                        .lock()
                        .unwrap_or_else(|poisoned| poisoned.into_inner())
                        .take();
                    let Some(result) = result else { return };
                    this.model.credential_operation = None;
                    match result {
                        Ok(_) => {
                            let mut latest = AppSettings::global(cx).clone();
                            match remove_migrated_legacy_credential(&mut latest, &path) {
                                Ok(()) => {
                                    *AppSettings::global_mut(cx) = latest;
                                    this.model.credential_notice = Some(CredentialNotice {
                                        key: Key::LegacyCredentialMigrated,
                                        error: false,
                                    });
                                }
                                Err(error) => {
                                    this.model.credential_notice = Some(CredentialNotice {
                                        key: migration_error_key(&error),
                                        error: true,
                                    });
                                }
                            }
                        }
                        Err(error) => {
                            this.model.credential_notice = Some(CredentialNotice {
                                key: migration_error_key(&error),
                                error: true,
                            });
                        }
                    }
                    this.model.refresh_credential_presence(cx);
                    cx.notify();
                })
                .is_some()
                {
                    break;
                }
                if this.upgrade().is_none() {
                    break;
                }
                cx.background_executor()
                    .timer(Duration::from_millis(1))
                    .await;
            }
        })
        .detach();
    }

    fn clear_credential_draft(&self, window: &mut Window, cx: &mut Context<SettingsView>) {
        self.credential_draft
            .update(cx, |draft, cx| draft.set_value("", window, cx));
    }

    fn set_credential_notice(&mut self, key: Key, error: bool, cx: &mut Context<SettingsView>) {
        self.credential_notice = Some(CredentialNotice { key, error });
        cx.notify();
    }

    fn refresh_credential_presence(&mut self, cx: &App) {
        self.credential_presence = match configured_endpoint(AppSettings::global(cx)) {
            ConfiguredEndpoint::ProviderRequired | ConfiguredEndpoint::UnsupportedProvider => {
                CredentialPresence::ProviderRequired
            }
            ConfiguredEndpoint::Invalid(_) => CredentialPresence::InvalidEndpoint,
            ConfiguredEndpoint::Ready(endpoint) => {
                let vault = CredentialVault::global(cx);
                if !vault.secure_store_supported() {
                    CredentialPresence::SecureStoreUnavailable
                } else {
                    match vault.has_persistent(endpoint.credential_target().as_str()) {
                        Ok(true) => CredentialPresence::Present,
                        Ok(false) => CredentialPresence::Absent,
                        Err(error) => CredentialPresence::Error(credential_error_key(&error)),
                    }
                }
            }
        };
    }
}

fn configured_endpoint(settings: &AppSettings) -> ConfiguredEndpoint {
    let provider = settings.model_provider.trim();
    if provider.is_empty() {
        return ConfiguredEndpoint::ProviderRequired;
    }
    let Some(provider) = Provider::from_key(provider) else {
        return ConfiguredEndpoint::UnsupportedProvider;
    };
    match EndpointIdentity::parse(provider, Some(&settings.model_base_url)) {
        Ok(endpoint) => ConfiguredEndpoint::Ready(endpoint),
        Err(error) => ConfiguredEndpoint::Invalid(error),
    }
}

pub(super) fn endpoint_draft_changed(settings: &AppSettings, draft: &str) -> bool {
    draft != settings.model_base_url
}

fn current_credential_target(cx: &App) -> Option<String> {
    match configured_endpoint(AppSettings::global(cx)) {
        ConfiguredEndpoint::Ready(endpoint) => {
            Some(endpoint.credential_target().as_str().to_owned())
        }
        ConfiguredEndpoint::ProviderRequired
        | ConfiguredEndpoint::UnsupportedProvider
        | ConfiguredEndpoint::Invalid(_) => None,
    }
}

fn endpoint_status(cx: &App) -> (String, bool) {
    match configured_endpoint(AppSettings::global(cx)) {
        ConfiguredEndpoint::ProviderRequired => {
            (i18n::t(Key::ChooseProviderForCredential, cx).into(), true)
        }
        ConfiguredEndpoint::UnsupportedProvider => {
            (i18n::t(Key::UnsupportedProvider, cx).into(), true)
        }
        ConfiguredEndpoint::Invalid(error) => (i18n::model_endpoint_error(&error, cx), true),
        ConfiguredEndpoint::Ready(endpoint) => (i18n::model_endpoint_status(&endpoint, cx), false),
    }
}

pub(super) fn environment_credential_authorized(settings: &AppSettings) -> bool {
    let ConfiguredEndpoint::Ready(endpoint) = configured_endpoint(settings) else {
        return false;
    };
    !endpoint.is_vendor_default()
        && settings.model_environment_key_identity == endpoint.credential_target().as_str()
}

pub(super) fn set_environment_credential_authorization(
    settings: &mut AppSettings,
    authorized: bool,
) {
    let ConfiguredEndpoint::Ready(endpoint) = configured_endpoint(settings) else {
        return;
    };
    if endpoint.is_vendor_default() {
        return;
    }

    let target = endpoint.credential_target();
    if authorized {
        settings.model_environment_key_identity = target.as_str().to_owned();
    } else if settings.model_environment_key_identity == target.as_str() {
        settings.model_environment_key_identity.clear();
    }
}

fn environment_credential_help(cx: &App) -> String {
    let ConfiguredEndpoint::Ready(endpoint) = configured_endpoint(AppSettings::global(cx)) else {
        return i18n::t(Key::ChooseProviderForCredential, cx).into();
    };
    i18n::environment_credential_description(
        endpoint.provider().credential_environment_variable(),
        &endpoint,
        cx,
    )
}

fn credential_status(
    operation: Option<CredentialOperation>,
    presence: CredentialPresence,
    notice: Option<CredentialNotice>,
) -> (Key, bool) {
    if let Some(operation) = operation {
        return (
            match operation {
                CredentialOperation::Store => Key::CredentialStoring,
                CredentialOperation::Delete => Key::CredentialDeleting,
                CredentialOperation::Migrate => Key::CredentialMigrating,
            },
            false,
        );
    }
    if let Some(notice) = notice {
        return (notice.key, notice.error);
    }
    match presence {
        CredentialPresence::ProviderRequired => (Key::ChooseProviderForCredential, true),
        CredentialPresence::InvalidEndpoint => (Key::CredentialEndpointInvalid, true),
        CredentialPresence::SecureStoreUnavailable => (Key::SecureStoreUnavailable, false),
        CredentialPresence::Present => (Key::CredentialStoredPresent, false),
        CredentialPresence::Absent => (Key::CredentialStoredAbsent, false),
        CredentialPresence::Error(key) => (key, true),
    }
}

fn credential_error_key(error: &CredentialError) -> Key {
    match error.kind() {
        CredentialErrorKind::Unavailable => Key::SecureStoreUnavailable,
        CredentialErrorKind::Read => Key::CredentialReadFailed,
        CredentialErrorKind::Write => Key::CredentialWriteFailed,
        CredentialErrorKind::Verify => Key::CredentialVerifyFailed,
        CredentialErrorKind::Delete => Key::CredentialDeleteFailed,
        CredentialErrorKind::InvalidEncoding => Key::CredentialInvalidEncoding,
        CredentialErrorKind::Empty => Key::CredentialRequired,
    }
}

fn migration_error_key(error: &LegacyCredentialMigrationError) -> Key {
    match error {
        LegacyCredentialMigrationError::SecureStore(error) => credential_error_key(error),
        LegacyCredentialMigrationError::SettingsWrite(_) => Key::CredentialMigrationSaveFailed,
    }
}

/// A model setter also invalidates credential status derived from its endpoint.
fn write_model(
    this: &Entity<SettingsView>,
    edit: impl Fn(SharedString, &mut AppSettings) + 'static,
) -> impl Fn(SharedString, &mut App) + 'static {
    let this = this.downgrade();
    move |value, cx| {
        AppSettings::update(cx, |settings| edit(value, settings));
        let _ = this.update(cx, |this, cx| {
            this.model.credential_notice = None;
            this.model.refresh_credential_presence(cx);
            cx.notify();
        });
    }
}

#[cfg(test)]
impl ModelSettings {
    pub(super) fn credential_draft(&self) -> Entity<InputState> {
        self.credential_draft.clone()
    }

    pub(super) fn model_base_url_draft(&self) -> Entity<InputState> {
        self.model_base_url_draft.clone()
    }

    pub(super) fn operation_pending(&self) -> bool {
        self.credential_operation.is_some()
    }
}
