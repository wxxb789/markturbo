//! The settings page.
//!
//! A takeover of the document column rather than a dialog: settings here are
//! mostly toggles the user wants to see take effect on the document behind
//! them, and a modal covering that document would hide the feedback.
//!
//! Non-secret values go through [`AppSettings`], which persists on each edit.
//! Credential drafts stay local and move only into
//! [`crate::credentials::CredentialVault`] after an explicit action. What this
//! view cannot do is repaint the rest of the app or test a model transport;
//! those are the [`SettingsEvent`]s, named for what changed rather than for the
//! response the workspace chooses.

use gpui::*;
use gpui_component::{
    Icon, IconName,
    setting::{SettingField, SettingGroup, SettingItem, SettingPage, Settings},
};

use crate::i18n::{self, Key};
use crate::settings::{AppSettings, GroupBy, Language, ThemePreference};

use super::model_settings::ModelSettings;

/// What the user changed, for whoever has to repaint because of it.
///
/// Named for the change rather than the response: this view does not know that
/// a theme change also invalidates cached WebView HTML, and should not have to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SettingsEvent {
    /// The theme preference or one of the two presets changed. Already written
    /// to [`AppSettings`]; what is left is repainting whatever caches a palette.
    ThemeChanged,
    /// The interface language changed. Labels resolve during render, so this is
    /// a redraw of every view rather than a reload of anything.
    LanguageChanged,
    /// A setting that governs which skills are discovered changed, so the list
    /// on screen is now answering the previous question.
    SkillScopeChanged,
    /// Test the credential for the current provider and endpoint. The workspace
    /// owns transport and user-visible request status; Settings never sends a
    /// network request directly.
    TestModelCredential,
}

/// The settings page.
///
/// Non-secret preferences remain in the global [`AppSettings`]. Model-specific
/// draft and operation state is isolated in [`ModelSettings`].
pub struct SettingsView {
    focus_handle: FocusHandle,
    pub(super) model: ModelSettings,
}

impl SettingsView {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        Self {
            focus_handle: cx.focus_handle(),
            model: ModelSettings::new(window, cx),
        }
    }

    /// Appearance: theme mode, the two presets, and the interface language.
    fn appearance(&self, this: &Entity<Self>, cx: &Context<Self>) -> SettingPage {
        let theme_options: Vec<(SharedString, SharedString)> = ThemePreference::ALL
            .iter()
            .map(|p| (p.key().into(), p.label().into()))
            .collect();
        let light_presets: Vec<(SharedString, SharedString)> = crate::theme::for_mode(false)
            .map(|p| (p.id.into(), p.name.into()))
            .collect();
        let dark_presets: Vec<(SharedString, SharedString)> = crate::theme::for_mode(true)
            .map(|p| (p.id.into(), p.name.into()))
            .collect();
        let language_options: Vec<(SharedString, SharedString)> = Language::ALL
            .iter()
            .map(|l| (l.key().into(), l.label().into()))
            .collect();

        SettingPage::new(i18n::t(Key::Appearance, cx))
            .icon(Icon::new(IconName::Palette))
            .default_open(true)
            .group(
                SettingGroup::new()
                    .title(i18n::t(Key::Theme, cx))
                    .items(vec![
                        SettingItem::new(
                            i18n::t(Key::Mode, cx),
                            SettingField::dropdown(
                                theme_options,
                                |cx: &App| AppSettings::global(cx).theme.key().into(),
                                emit(this, SettingsEvent::ThemeChanged, |value, settings| {
                                    settings.theme = ThemePreference::from_key(&value)
                                }),
                            )
                            .default_value(ThemePreference::System.key().to_string()),
                        )
                        .description(i18n::t(Key::ModeHelp, cx)),
                        // Two presets rather than one: the mode above can be
                        // System, so a machine that flips at sunset has to know
                        // which preset to land on either side.
                        SettingItem::new(
                            i18n::t(Key::LightTheme, cx),
                            SettingField::dropdown(
                                light_presets,
                                |cx: &App| AppSettings::global(cx).theme_light.clone().into(),
                                emit(this, SettingsEvent::ThemeChanged, |value, settings| {
                                    settings.theme_light = value.to_string()
                                }),
                            )
                            .default_value(crate::theme::DEFAULT_LIGHT.to_string()),
                        )
                        .description(i18n::t(Key::LightThemeHelp, cx)),
                        SettingItem::new(
                            i18n::t(Key::DarkTheme, cx),
                            SettingField::dropdown(
                                dark_presets,
                                |cx: &App| AppSettings::global(cx).theme_dark.clone().into(),
                                emit(this, SettingsEvent::ThemeChanged, |value, settings| {
                                    settings.theme_dark = value.to_string()
                                }),
                            )
                            .default_value(crate::theme::DEFAULT_DARK.to_string()),
                        )
                        .description(i18n::t(Key::DarkThemeHelp, cx)),
                    ]),
            )
            .group(
                SettingGroup::new().title(i18n::t(Key::Language_, cx)).item(
                    SettingItem::new(
                        i18n::t(Key::Language_, cx),
                        SettingField::dropdown(
                            language_options,
                            |cx: &App| AppSettings::global(cx).language.key().into(),
                            emit(this, SettingsEvent::LanguageChanged, |value, settings| {
                                settings.language = Language::from_key(&value)
                            }),
                        )
                        .default_value(Language::default().key().to_string()),
                    )
                    .description(i18n::t(Key::LanguageHelp, cx)),
                ),
            )
    }

    /// Editor: how the split behaves and whether the watcher reloads.
    fn editor(&self, cx: &Context<Self>) -> SettingPage {
        SettingPage::new(i18n::t(Key::Editor, cx))
            .icon(Icon::new(IconName::LayoutDashboard))
            .group(
                SettingGroup::new().title(i18n::t(Key::SplitView, cx)).item(
                    SettingItem::new(
                        i18n::t(Key::SyncScrolling, cx),
                        SettingField::switch(
                            |cx: &App| AppSettings::global(cx).split_sync_scroll,
                            toggle(|value, settings| settings.split_sync_scroll = value),
                        )
                        .default_value(false),
                    )
                    .description(i18n::t(Key::SyncScrollingHelp, cx)),
                ),
            )
            .group(
                SettingGroup::new().title(i18n::t(Key::Watching, cx)).item(
                    SettingItem::new(
                        i18n::t(Key::AutoRefresh, cx),
                        SettingField::switch(
                            |cx: &App| AppSettings::global(cx).watch_auto_reload,
                            toggle(|value, settings| settings.watch_auto_reload = value),
                        )
                        .default_value(false),
                    )
                    .description(i18n::t(Key::AutoRefreshHelp, cx)),
                ),
            )
    }

    /// Skills: what discovery covers, and how the result is grouped.
    fn skills(&self, this: &Entity<Self>, cx: &Context<Self>) -> SettingPage {
        let group_options: Vec<(SharedString, SharedString)> = GroupBy::ALL
            .iter()
            .map(|g| (g.key().into(), g.label().into()))
            .collect();

        SettingPage::new(i18n::t(Key::Skills, cx))
            .icon(Icon::new(IconName::Bot))
            .group(
                SettingGroup::new()
                    .title(i18n::t(Key::Discovery, cx))
                    .items(vec![
                        SettingItem::new(
                            i18n::t(Key::IncludeGlobalSkills, cx),
                            SettingField::switch(
                                |cx: &App| AppSettings::global(cx).skills_include_global,
                                emit_bool(
                                    this,
                                    SettingsEvent::SkillScopeChanged,
                                    |value, settings| settings.skills_include_global = value,
                                ),
                            )
                            .default_value(true),
                        )
                        .description(i18n::t(Key::IncludeGlobalSkillsHelp, cx)),
                        SettingItem::new(
                            i18n::t(Key::ShowInternalSkills, cx),
                            SettingField::switch(
                                |cx: &App| AppSettings::global(cx).skills_include_internal,
                                emit_bool(
                                    this,
                                    SettingsEvent::SkillScopeChanged,
                                    |value, settings| settings.skills_include_internal = value,
                                ),
                            )
                            .default_value(false),
                        )
                        .description(i18n::t(Key::ShowInternalSkillsHelp, cx)),
                        // Regrouping only reorders what is already loaded, so
                        // no event: the harness panel reads the setting during
                        // its own render.
                        SettingItem::new(
                            i18n::t(Key::GroupBy, cx),
                            SettingField::dropdown(
                                group_options,
                                |cx: &App| AppSettings::global(cx).skills_group_by.key().into(),
                                write(|value, settings| {
                                    settings.skills_group_by = GroupBy::from_key(&value)
                                }),
                            )
                            .default_value(GroupBy::Origin.key().to_string()),
                        )
                        .description(i18n::t(Key::GroupByHelp, cx)),
                    ]),
            )
    }
}

/// A setter that only persists.
///
/// The plain case: every control writes through `AppSettings::update`, which is
/// what makes the change survive a restart.
pub(super) fn write(
    edit: impl Fn(SharedString, &mut AppSettings) + 'static,
) -> impl Fn(SharedString, &mut App) + 'static {
    move |value, cx| AppSettings::update(cx, |settings| edit(value, settings))
}

/// A setter that persists, then announces what changed.
///
/// The entity is held weakly on purpose: these closures outlive the render that
/// built them, and a strong handle here would be a cycle through the element
/// tree that keeps the page alive after the workspace drops it.
fn emit(
    this: &Entity<SettingsView>,
    event: SettingsEvent,
    edit: impl Fn(SharedString, &mut AppSettings) + 'static,
) -> impl Fn(SharedString, &mut App) + 'static {
    let this = this.downgrade();
    move |value, cx| {
        AppSettings::update(cx, |settings| edit(value, settings));
        // Written before the event, so a subscriber reading `AppSettings` sees
        // the new value rather than the one it is being told about.
        let _ = this.update(cx, |_, cx| cx.emit(event));
    }
}

/// [`emit`] for a switch.
fn emit_bool(
    this: &Entity<SettingsView>,
    event: SettingsEvent,
    edit: impl Fn(bool, &mut AppSettings) + 'static,
) -> impl Fn(bool, &mut App) + 'static {
    let this = this.downgrade();
    move |value, cx| {
        AppSettings::update(cx, |settings| edit(value, settings));
        let _ = this.update(cx, |_, cx| cx.emit(event));
    }
}

/// [`write`] for a switch.
fn toggle(edit: impl Fn(bool, &mut AppSettings) + 'static) -> impl Fn(bool, &mut App) + 'static {
    move |value, cx| AppSettings::update(cx, |settings| edit(value, settings))
}

impl EventEmitter<SettingsEvent> for SettingsView {}

impl Focusable for SettingsView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for SettingsView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // Cloned once and handed to each page: `cx.entity()` cannot be called
        // again while the page builders borrow `cx`.
        let this = cx.entity();

        Settings::new("settings")
            .page(self.appearance(&this, cx))
            .page(self.model.translation(&this, cx))
            .page(self.editor(cx))
            .page(self.skills(&this, cx))
    }
}

#[cfg(test)]
mod tests {
    // Import selectively: the `gpui::*` glob above re-exports a `test`
    // attribute macro that shadows the built-in one and blows the recursion
    // limit.
    use std::sync::atomic::{AtomicUsize, Ordering};

    use crate::credentials::{CredentialError, CredentialVault, Secret, SecureCredentialStore};
    use crate::i18n::{self, Key, text};
    use crate::model::Provider;
    #[cfg(target_os = "windows")]
    use crate::model::{EndpointIdentity, ModelOperation, ModelRequestDisclosure, OutboundScope};
    use crate::settings::{AppSettings, Language};
    use gpui::AppContext as _;
    #[cfg(target_os = "windows")]
    use mt_doc::{
        DocType, Document,
        translate::{Scope, TranslationRequest},
    };

    use super::super::{
        model_settings::{
            endpoint_draft_changed, environment_credential_authorized,
            set_environment_credential_authorization,
        },
        production_source,
    };
    use super::{SettingsEvent, SettingsView};

    #[derive(Default)]
    struct RecordingCredentialStore {
        writes: AtomicUsize,
    }

    impl SecureCredentialStore for RecordingCredentialStore {
        fn is_supported(&self) -> bool {
            true
        }

        fn read(&self, _: &str) -> Result<Option<Secret>, CredentialError> {
            Ok(None)
        }

        fn write(&self, _: &str, _: &Secret) -> Result<(), CredentialError> {
            self.writes.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }

        fn delete(&self, _: &str) -> Result<(), CredentialError> {
            Ok(())
        }
    }

    fn settings_production_source() -> String {
        [
            production_source(include_str!("settings_page.rs")),
            production_source(include_str!("model_settings.rs")),
        ]
        .concat()
    }

    #[cfg(target_os = "windows")]
    struct PrivacyScreenshotSurface {
        settings: gpui::Entity<SettingsView>,
        _request: TranslationRequest,
    }

    #[cfg(target_os = "windows")]
    impl gpui::Render for PrivacyScreenshotSurface {
        fn render(
            &mut self,
            _: &mut gpui::Window,
            cx: &mut gpui::Context<Self>,
        ) -> impl gpui::IntoElement {
            gpui_component::setting::Settings::new("privacy-settings")
                .page(self.settings.read(cx).model.translation(&self.settings, cx))
        }
    }

    /// No string on this page may be authored inline.
    ///
    /// Source-level: rendering a `Settings` needs a window, and the failure is
    /// invisible in English anyway — which is exactly why it lasted. Sixteen
    /// keys sat translated and unreferenced while the page hard-coded the same
    /// words, so switching the interface to Chinese changed the panels and left
    /// Settings in English.
    #[test]
    fn every_visible_string_on_the_settings_page_comes_from_the_string_table() {
        let code = settings_production_source();

        for builder in [
            "SettingPage::new(",
            "SettingGroup::new().title(",
            ".title(",
            "SettingItem::new(",
            ".description(",
        ] {
            for (at, _) in code.match_indices(builder) {
                let rest = &code[at + builder.len()..];
                let argument = rest.trim_start();
                assert!(
                    !argument.starts_with('"'),
                    "`{builder}` is given a literal, which cannot translate:\n{}",
                    &argument[..argument.len().min(80)]
                );
            }
        }
    }

    /// The page's own strings must exist in both languages.
    ///
    /// The general coverage tests live in `i18n`; this one names the keys this
    /// file depends on, so deleting one from the table fails here rather than
    /// showing a blank label.
    #[test]
    fn the_settings_keys_read_differently_in_each_language() {
        for key in [
            Key::Appearance,
            Key::Theme,
            Key::Mode,
            Key::ModeHelp,
            Key::ModelConfiguration,
            Key::EndpointStatus,
            Key::Credential,
            Key::CredentialHelp,
            Key::StoreCredentialSecurely,
            Key::UseCredentialForSession,
            Key::TestCredential,
            Key::DeleteCredential,
            Key::EnvironmentCredential,
            Key::EnvironmentCredentialAuthorizationSaveFailed,
            Key::LegacyCredential,
            Key::MigrateLegacyCredential,
            Key::Editor,
            Key::SplitView,
            Key::GroupByHelp,
        ] {
            for language in Language::ALL {
                assert!(!text(key, language).is_empty(), "{key:?}");
            }
        }
        // And the Chinese table is not silently falling back for the block this
        // page exists to make live.
        assert_ne!(
            text(Key::Appearance, Language::Chinese),
            text(Key::Appearance, Language::English)
        );
        assert_ne!(
            text(Key::ModeHelp, Language::Chinese),
            text(Key::ModeHelp, Language::English)
        );
    }

    /// The API-key description must be one flowed sentence.
    ///
    /// It was three runs of thirty-four spaces baked into the string literal —
    /// a `cargo fmt` artifact of a `\`-continued literal whose continuation
    /// lines were re-indented — so the rendered text read `Leave` then a gap
    /// then `empty`. Invisible in the source, obvious in the window.
    #[test]
    fn no_description_carries_a_run_of_spaces_from_source_indentation() {
        for language in Language::ALL {
            for key in [
                Key::ModeHelp,
                Key::LanguageHelp,
                Key::ProviderHelp,
                Key::ApiKeyHelp,
                Key::CredentialHelp,
                Key::LegacyCredentialHelp,
                Key::BaseUrlHelp,
                Key::SyncScrollingHelp,
                Key::AutoRefreshHelp,
                Key::IncludeGlobalSkillsHelp,
                Key::ShowInternalSkillsHelp,
            ] {
                let value = text(key, language);
                assert!(
                    !value.contains("  "),
                    "{key:?} in {} has a double space: {value}",
                    language.label()
                );
            }
        }
    }

    #[test]
    fn goal_05a_credential_ui_keeps_secrets_local_masked_and_unprefilled() {
        let code = settings_production_source();

        assert!(code.contains("credential_draft: Entity<InputState>"));
        assert!(code.contains("InputState::new(window, cx).masked(true)"));
        assert!(code.contains("InputContentType::Password"));
        assert!(!code.contains(".mask_toggle()"));
        assert!(!code.contains("translate_api_key"));
        assert!(!code.contains("legacy_model_api_key().unwrap"));
        assert!(!code.contains("legacy_model_api_key().expect"));
        assert!(!code.contains(".resolve("));
        assert!(!code.contains(".expose("));

        assert!(
            !code.contains(
                "credential_draft = cx.new(|cx| InputState::new(window, cx).default_value"
            )
        );
    }

    #[gpui::test]
    fn credential_ui_state_never_prefills_sensitive_settings(cx: &mut gpui::TestAppContext) {
        let settings: AppSettings = toml::from_str(
            "translate-api-key = \"legacy-ui-secret\"\n\
             model-base-url = \"https://user:endpoint-secret@example.com/v1/\"\n",
        )
        .unwrap();
        cx.update(|app| {
            gpui_component::init(app);
            app.set_global(settings);
            CredentialVault::init(app);
        });
        let captured = std::rc::Rc::new(std::cell::RefCell::new(None));
        let (_, cx) = cx.add_window_view({
            let captured = captured.clone();
            move |window, cx| {
                let view = cx.new(|cx| SettingsView::new(window, cx));
                *captured.borrow_mut() = Some(view.clone());
                gpui_component::Root::new(view, window, cx)
            }
        });
        let view = captured.borrow().clone().expect("the SettingsView entity");

        view.read_with(cx, |view, app| {
            let credential_draft = view.model.credential_draft();
            let credential = credential_draft.read(app);
            assert!(credential.value().is_empty());
            assert!(credential.presentation().is_masked());
            assert!(
                view.model
                    .model_base_url_draft()
                    .read(app)
                    .value()
                    .is_empty()
            );
        });
    }

    #[gpui::test]
    fn goal_05a_credential_lifecycle_cancel_preserves_the_legacy_copy(
        cx: &mut gpui::TestAppContext,
    ) {
        let settings: AppSettings = toml::from_str(
            "model-provider = \"openai-chat\"\n\
             translate-api-key = \"cancelled-migration-sentinel\"\n",
        )
        .unwrap();
        let store = std::sync::Arc::new(RecordingCredentialStore::default());
        cx.update(|app| {
            gpui_component::init(app);
            app.set_global(settings);
            app.set_global(CredentialVault::with_store(store.clone()));
        });
        let captured = std::rc::Rc::new(std::cell::RefCell::new(None));
        let (_, cx) = cx.add_window_view({
            let captured = captured.clone();
            move |window, cx| {
                let view = cx.new(|cx| SettingsView::new(window, cx));
                *captured.borrow_mut() = Some(view.clone());
                gpui_component::Root::new(view, window, cx)
            }
        });
        let view = captured.borrow().clone().expect("the SettingsView entity");

        cx.update(|window, app| {
            view.update(app, |view, cx| {
                view.model.confirm_legacy_migration(window, cx)
            });
        });
        assert!(cx.has_pending_prompt());
        cx.simulate_prompt_answer("Cancel");
        cx.run_until_parked();

        assert_eq!(store.writes.load(Ordering::SeqCst), 0);
        view.read_with(cx, |view, app| {
            assert!(!view.model.operation_pending());
            assert_eq!(
                AppSettings::global(app).legacy_model_api_key(),
                Some("cancelled-migration-sentinel")
            );
        });
    }

    #[test]
    fn goal_05a_model_settings_and_credential_actions_use_the_shared_contracts() {
        let code = settings_production_source();

        for required in [
            "model_provider",
            "model_name",
            "model_base_url",
            "model_environment_key_identity",
            "AppSettings::try_update",
            "model_base_url_control",
            "apply_model_base_url",
            "SettingsEvent::TestModelCredential",
            "credential_target()",
            "CredentialVault::global",
            "replace_persistent",
            "replace_session",
            "secure_legacy_credential",
            "remove_migrated_legacy_credential",
            ".delete(",
            "window.prompt(",
        ] {
            assert!(code.contains(required), "missing Goal 05A path: {required}");
        }

        assert!(!code.contains("settings.translate_provider"));
        assert!(!code.contains("settings.translate_model"));
        assert!(!code.contains("settings.translate_base_url"));
        assert!(!code.contains(".update_in("));
        assert!(
            code.matches("try_update_in(&this").count() >= 5,
            "credential completion paths must retry fallible window updates"
        );
        assert_eq!(
            SettingsEvent::TestModelCredential,
            SettingsEvent::TestModelCredential
        );
    }

    #[test]
    fn destructive_credential_actions_confirm_before_touching_the_exact_target() {
        let code = settings_production_source();

        assert!(code.matches("window.prompt(").count() >= 2);
        assert!(code.matches("answer.await.unwrap_or(1) != 0").count() >= 2);
        assert!(code.contains("vault.delete(&target)"));
        assert!(code.contains("secure_legacy_credential("));
        assert!(code.contains("remove_migrated_legacy_credential("));
        assert!(code.contains("endpoint.credential_target().as_str().to_owned()"));
    }

    #[test]
    fn custom_environment_authorization_is_bound_to_one_exact_endpoint() {
        let mut settings = AppSettings::default();
        settings.model_provider = Provider::OpenAiChat.key().into();
        settings.model_base_url = "https://first.example/v1".into();

        set_environment_credential_authorization(&mut settings, true);
        let first_authorization = settings.model_environment_key_identity.clone();
        assert!(!first_authorization.is_empty());
        assert!(environment_credential_authorized(&settings));

        settings.model_environment_key_identity = format!(" {first_authorization}");
        assert!(!environment_credential_authorized(&settings));
        settings.model_environment_key_identity = first_authorization.clone();

        settings.model_base_url = "https://second.example/v1".into();
        assert!(!environment_credential_authorized(&settings));

        set_environment_credential_authorization(&mut settings, false);
        assert_eq!(settings.model_environment_key_identity, first_authorization);

        set_environment_credential_authorization(&mut settings, true);
        assert!(environment_credential_authorized(&settings));
        assert_ne!(settings.model_environment_key_identity, first_authorization);
    }

    #[test]
    fn credential_actions_require_the_visible_endpoint_draft_to_be_saved() {
        let mut settings = AppSettings::default();
        settings.model_base_url = "https://first.example/v1/".into();

        assert!(!endpoint_draft_changed(
            &settings,
            "https://first.example/v1/"
        ));
        assert!(endpoint_draft_changed(
            &settings,
            "https://second.example/v1/"
        ));
    }

    #[test]
    fn vendor_default_endpoint_never_needs_a_custom_environment_authorization() {
        let mut settings = AppSettings::default();
        settings.model_provider = Provider::AnthropicMessages.key().into();

        set_environment_credential_authorization(&mut settings, true);

        assert!(settings.model_environment_key_identity.is_empty());
        assert!(!environment_credential_authorized(&settings));
    }

    #[cfg(target_os = "windows")]
    #[test]
    #[ignore = "Goal 05A DirectX screenshot privacy acceptance; run explicitly"]
    fn rendered_privacy_surfaces_are_secret_invariant() {
        use std::sync::mpsc;
        use std::time::Duration;

        const CREDENTIAL_A: &str = "credential-sentinel-alpha";
        const CREDENTIAL_B: &str = "credential-sentinel-bravo";
        const REQUEST_A: &str = "private-request-alpha";
        const REQUEST_B: &str = "private-request-bravo";

        fn request(value: &str) -> (TranslationRequest, ModelRequestDisclosure) {
            let document = Document::with_type(DocType::Markdown, value.to_owned());
            let request = TranslationRequest::prepare(&document, &Scope::Document);
            let endpoint = EndpointIdentity::parse(Provider::OpenAiChat, None).unwrap();
            let disclosure = ModelRequestDisclosure::new(
                ModelOperation::Translation,
                endpoint,
                OutboundScope::document(request.input_bytes() as u64),
            );
            (request, disclosure)
        }

        fn open_surface(
            cx: &mut gpui::App,
            credential: &str,
            request: TranslationRequest,
            disclosure: ModelRequestDisclosure,
        ) -> gpui::WindowHandle<gpui_component::Root> {
            let credential = credential.to_owned();
            cx.open_window(
                gpui::WindowOptions {
                    window_bounds: Some(gpui::WindowBounds::Windowed(gpui::Bounds {
                        origin: gpui::point(gpui::px(0.0), gpui::px(0.0)),
                        size: gpui::size(gpui::px(1000.0), gpui::px(700.0)),
                    })),
                    show: false,
                    ..gpui_component::TitleBar::window_options()
                },
                move |window, cx| {
                    let settings = cx.new(|cx| SettingsView::new(window, cx));
                    let credential_draft = settings.read(cx).model.credential_draft();
                    credential_draft.update(cx, |draft, cx| {
                        draft.set_value(credential, window, cx);
                    });
                    let prompt_description = i18n::model_request_disclosure(&disclosure, cx);
                    let surface = cx.new(|_| PrivacyScreenshotSurface {
                        settings,
                        _request: request,
                    });
                    let root = cx.new(|cx| gpui_component::Root::new(surface, window, cx));
                    std::mem::drop(window.prompt(
                        gpui::PromptLevel::Warning,
                        i18n::t(Key::ModelRequestConsentTitle, cx),
                        Some(&prompt_description),
                        &[
                            gpui::PromptButton::ok(i18n::t(Key::SendToModel, cx)),
                            gpui::PromptButton::cancel(i18n::t(Key::Cancel, cx)),
                        ],
                        cx,
                    ));
                    root
                },
            )
            .unwrap()
        }

        assert_eq!(CREDENTIAL_A.len(), CREDENTIAL_B.len());
        assert_eq!(REQUEST_A.len(), REQUEST_B.len());
        let (request_a, disclosure_a) = request(REQUEST_A);
        let (request_b, disclosure_b) = request(REQUEST_B);
        assert_ne!(request_a.inputs(), request_b.inputs());

        let (sender, receiver) = mpsc::sync_channel(1);
        unsafe { std::env::set_var("GPUI_DISABLE_DIRECT_COMPOSITION", "true") };
        gpui_platform::application()
            .with_assets(crate::assets::Assets)
            .run(move |cx| {
                gpui_component::init(cx);
                cx.set_prompt_builder(gpui::fallback_prompt_renderer);
                let settings: AppSettings =
                    toml::from_str("model-provider = \"openai-chat\"\n").unwrap();
                cx.set_global(settings);
                cx.set_global(CredentialVault::with_store(std::sync::Arc::new(
                    RecordingCredentialStore::default(),
                )));

                let first = open_surface(cx, CREDENTIAL_A, request_a, disclosure_a);
                let second = open_surface(cx, CREDENTIAL_B, request_b, disclosure_b);
                cx.spawn(async move |cx| {
                    cx.background_executor()
                        .timer(Duration::from_millis(100))
                        .await;
                    let first = first
                        .update(cx, |_, window, _| window.render_to_image())
                        .map_err(|error| error.to_string())
                        .and_then(|image| image.map_err(|error| error.to_string()));
                    let second = second
                        .update(cx, |_, window, _| window.render_to_image())
                        .map_err(|error| error.to_string())
                        .and_then(|image| image.map_err(|error| error.to_string()));
                    let result = first.and_then(|first| second.map(|second| (first, second)));
                    let _ = sender.send(result);
                    cx.update(|cx| cx.quit());
                })
                .detach();
            });

        let (first, second) = receiver
            .recv_timeout(Duration::from_secs(30))
            .expect("the GPUI screenshot task completed")
            .expect("DirectX rendered the privacy surfaces");
        assert_eq!(first.dimensions(), second.dimensions());
        assert_eq!(first.as_raw(), second.as_raw());
        assert!(first.width() > 0 && first.height() > 0);
        assert!(
            first
                .pixels()
                .any(|pixel| pixel.0 != first.get_pixel(0, 0).0),
            "the captured privacy surface must not be blank"
        );

        let output = std::env::var_os("MARKTURBO_PRIVACY_SCREENSHOT")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| std::env::temp_dir().join("markturbo-goal-05a-privacy.png"));
        if let Some(parent) = output.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        first.save(&output).unwrap();
        let encoded = std::fs::read(output).unwrap();
        for sentinel in [CREDENTIAL_A, CREDENTIAL_B, REQUEST_A, REQUEST_B] {
            assert!(
                !encoded
                    .windows(sentinel.len())
                    .any(|bytes| bytes == sentinel.as_bytes()),
                "screenshot encoded a privacy sentinel"
            );
        }
    }
}
