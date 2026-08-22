//! The settings page.
//!
//! A takeover of the document column rather than a dialog: settings here are
//! mostly toggles the user wants to see take effect on the document behind
//! them, and a modal covering that document would hide the feedback.
//!
//! Every value read and written goes through [`AppSettings`], which persists on
//! each edit. What this view cannot do is repaint the rest of the app — the Web
//! preview bakes the palette into cached HTML, the panels resolve their labels
//! at render time, and the skill list is scoped by two of these switches. Those
//! are the [`SettingsEvent`]s, named for what changed rather than for the
//! repaint the workspace chooses to do about it.

use gpui::*;
use gpui_component::{
    Icon, IconName,
    setting::{SettingField, SettingGroup, SettingItem, SettingPage, Settings},
};

use crate::i18n::{self, Key};
use crate::settings::{AppSettings, GroupBy, Language, ThemePreference};
use crate::translate::Provider;

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
}

/// The settings page.
///
/// No state of its own: every control reads its value from the global
/// [`AppSettings`] through a closure that runs at render time, and writes
/// through `AppSettings::update`. A cached copy here would be a second source
/// of truth for values the status bar and the harness panel also write.
pub struct SettingsView {
    focus_handle: FocusHandle,
}

impl SettingsView {
    pub fn new(cx: &mut Context<Self>) -> Self {
        Self {
            focus_handle: cx.focus_handle(),
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

    /// Translation: which endpoint to speak to, and what to ask it for.
    ///
    /// Nothing here needs the workspace: the provider is resolved fresh from
    /// [`AppSettings`] on every translation request.
    fn translation(&self, cx: &Context<Self>) -> SettingPage {
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

        SettingPage::new(i18n::t(Key::Translation, cx))
            .icon(Icon::new(IconName::Globe))
            .groups(vec![
                SettingGroup::new()
                    .title(i18n::t(Key::Provider, cx))
                    .items(vec![
                        SettingItem::new(
                            i18n::t(Key::Provider, cx),
                            SettingField::dropdown(
                                provider_options,
                                |cx: &App| {
                                    AppSettings::global(cx).translate_provider.clone().into()
                                },
                                write(|value, settings| {
                                    settings.translate_provider = value.to_string()
                                }),
                            ),
                        )
                        .description(i18n::t(Key::ProviderHelp, cx)),
                        SettingItem::new(
                            i18n::t(Key::ApiKey, cx),
                            SettingField::input(
                                |cx: &App| AppSettings::global(cx).translate_api_key.clone().into(),
                                write(|value, settings| {
                                    settings.translate_api_key = value.to_string()
                                }),
                            ),
                        )
                        .description(i18n::t(Key::ApiKeyHelp, cx)),
                        SettingItem::new(
                            i18n::t(Key::BaseUrl, cx),
                            SettingField::input(
                                |cx: &App| {
                                    AppSettings::global(cx).translate_base_url.clone().into()
                                },
                                write(|value, settings| {
                                    settings.translate_base_url = value.to_string()
                                }),
                            ),
                        )
                        .description(i18n::t(Key::BaseUrlHelp, cx)),
                        SettingItem::new(
                            i18n::t(Key::Model, cx),
                            SettingField::input(
                                |cx: &App| AppSettings::global(cx).translate_model.clone().into(),
                                write(|value, settings| {
                                    settings.translate_model = value.to_string()
                                }),
                            ),
                        )
                        .description(i18n::t(Key::ModelHelp, cx)),
                        SettingItem::new(
                            i18n::t(Key::TargetLanguage, cx),
                            SettingField::input(
                                |cx: &App| AppSettings::global(cx).translate_to.clone().into(),
                                write(|value, settings| settings.translate_to = value.to_string()),
                            )
                            .default_value("zh"),
                        )
                        .description(i18n::t(Key::TargetLanguageHelp, cx)),
                    ]),
            ])
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
fn write(
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
            .page(self.translation(cx))
            .page(self.editor(cx))
            .page(self.skills(&this, cx))
    }
}

#[cfg(test)]
mod tests {
    // Import selectively: the `gpui::*` glob above re-exports a `test`
    // attribute macro that shadows the built-in one and blows the recursion
    // limit.
    use crate::i18n::{Key, text};
    use crate::settings::Language;

    /// No string on this page may be authored inline.
    ///
    /// Source-level: rendering a `Settings` needs a window, and the failure is
    /// invisible in English anyway — which is exactly why it lasted. Sixteen
    /// keys sat translated and unreferenced while the page hard-coded the same
    /// words, so switching the interface to Chinese changed the panels and left
    /// Settings in English.
    #[test]
    fn every_visible_string_on_the_settings_page_comes_from_the_string_table() {
        let source = include_str!("settings_page.rs");
        let code = &source[..source.find("\n#[cfg(test)]").unwrap_or(source.len())];

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
            Key::ApiKey,
            Key::ApiKeyHelp,
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
}
