//! GPUI integration for persisted settings.
//!
//! The serializable schema, defaults, path policy, and persistence/security
//! rules live in `mt-core::settings`. This app-local newtype is the GPUI Global
//! boundary and preserves notifications when settings are updated.

use std::ops::{Deref, DerefMut};
use std::path::Path;

use gpui_kit::{App, Global};
use serde::{Deserialize, Serialize};

use mt_core::settings::{SettingsData, ThemePreference};

/// Settings installed as a GPUI global.
///
/// Dereferencing exposes the headless settings fields and methods without
/// making `mt-core` depend on GPUI. Serialization is transparent so settings
/// retain the exact `SettingsData` TOML wire format.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct AppSettings(SettingsData);

impl Deref for AppSettings {
    type Target = SettingsData;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl DerefMut for AppSettings {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

impl From<SettingsData> for AppSettings {
    fn from(settings: SettingsData) -> Self {
        Self(settings)
    }
}

impl From<AppSettings> for SettingsData {
    fn from(settings: AppSettings) -> Self {
        settings.0
    }
}

impl Global for AppSettings {}

impl AppSettings {
    pub fn global(cx: &App) -> &AppSettings {
        cx.global::<AppSettings>()
    }

    pub fn global_mut(cx: &mut App) -> &mut AppSettings {
        cx.global_mut::<AppSettings>()
    }

    /// Load from disk (or defaults) and install as the global.
    pub fn init(cx: &mut App) {
        #[cfg(test)]
        cx.set_global(Self::default());
        #[cfg(not(test))]
        cx.set_global(Self::load());
    }

    /// Apply `edit`, then persist. Every setter goes through this so no change
    /// can be made that is forgotten on restart.
    ///
    /// Observers registered with `cx.observe_global::<AppSettings>` are notified
    /// automatically: `global_mut` pushes a `NotifyGlobalObservers` effect, so
    /// there is nothing to call here.
    pub fn update(cx: &mut App, edit: impl FnOnce(&mut AppSettings)) {
        let mut candidate = Self::global(cx).clone();
        edit(&mut candidate);
        #[cfg(not(test))]
        {
            candidate = match candidate.0.try_save_general() {
                Ok(persisted) => Self::from(persisted),
                Err(error) => {
                    if let Some(path) = mt_core::settings::settings_path() {
                        log::warn!("cannot write {}: {error}", path.display());
                    }
                    candidate
                }
            };
        }
        *Self::global_mut(cx) = candidate;
    }

    /// Persist a security-sensitive edit before publishing it to the UI.
    pub fn try_update(cx: &mut App, edit: impl FnOnce(&mut AppSettings)) -> std::io::Result<()> {
        let next = Self::prepare_persisted_update(Self::global(cx), edit, |candidate| {
            #[cfg(not(test))]
            {
                candidate
                    .0
                    .try_save_environment_credential_authorization()
                    .map(Self::from)
            }
            #[cfg(test)]
            {
                Ok(candidate.clone())
            }
        })?;
        *Self::global_mut(cx) = next;
        Ok(())
    }

    fn prepare_persisted_update(
        current: &Self,
        edit: impl FnOnce(&mut Self),
        persist: impl FnOnce(&Self) -> std::io::Result<Self>,
    ) -> std::io::Result<Self> {
        let mut candidate = current.clone();
        edit(&mut candidate);
        persist(&candidate)
    }

    /// Read the settings file, falling back to defaults.
    pub fn load() -> Self {
        Self(SettingsData::load())
    }

    /// Read from an explicit path, falling back to defaults when it is
    /// unreadable or malformed.
    pub fn load_from(path: &Path) -> Self {
        Self(SettingsData::load_from(path))
    }

    /// Write to disk. Failures are logged, never fatal.
    pub fn save(&self) {
        self.0.save();
    }

    /// Write to an explicit path without merging an existing settings file.
    #[cfg(test)]
    pub fn save_to(&self, path: &Path) {
        self.0.save_to(path);
    }
}

/// Apply the theme preference.
///
/// Resolves the preference to a light/dark mode, then applies the preset the
/// user picked for that mode. `System` reads the OS appearance rather than
/// guessing; the other two are explicit. Pass the window where there is one —
/// on Linux the app-level appearance query errors, which is why gpui-component's
/// helper takes one at all — and `None` from a setting callback, which only has
/// an `App`.
///
/// This recolors GPUI. It does *not* rebuild the Web preview, which caches HTML
/// with the palette baked in; see `Workspace::reapply_theme`.
pub fn apply_theme(
    preference: ThemePreference,
    window: Option<&mut gpui_kit::Window>,
    cx: &mut App,
) {
    let dark = resolve_dark(preference, window.as_deref(), cx);
    let settings = AppSettings::global(cx);
    let id = if dark {
        settings.theme_dark.clone()
    } else {
        settings.theme_light.clone()
    };
    crate::theme::apply(crate::theme::by_id(&id, dark), window, cx);
}

/// Whether `preference` means dark right now.
///
/// For `System` this is the window's appearance where there is a window, and the
/// app-level one otherwise. gpui-component's own `sync_system_appearance` prefers
/// the window for the same reason: the app-level query errors on Linux.
fn resolve_dark(preference: ThemePreference, window: Option<&gpui_kit::Window>, cx: &App) -> bool {
    use gpui_kit::component::ThemeMode;

    match preference {
        ThemePreference::Light => false,
        ThemePreference::Dark => true,
        ThemePreference::System => {
            let appearance = match window {
                Some(window) => window.appearance(),
                None => cx.window_appearance(),
            };
            ThemeMode::from(appearance).is_dark()
        }
    }
}

/// The preset that is currently in effect.
pub fn active_preset(cx: &App) -> &'static crate::theme::Preset {
    let dark = is_dark(cx);
    let settings = AppSettings::global(cx);
    let id = if dark {
        &settings.theme_dark
    } else {
        &settings.theme_light
    };
    crate::theme::by_id(id, dark)
}

/// Whether the *effective* theme is dark, after the preference is resolved.
///
/// The Web preview needs this: it renders in its own browser context and has no
/// access to the GPUI theme.
pub fn is_dark(cx: &App) -> bool {
    gpui_kit::component::Theme::global(cx).mode.is_dark()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn failed_persisted_update_keeps_the_published_security_state() {
        let mut current = AppSettings::default();
        current.model_environment_key_identity = "approved-endpoint".into();

        let result = AppSettings::prepare_persisted_update(
            &current,
            |candidate| candidate.model_environment_key_identity.clear(),
            |_| Err(std::io::Error::other("synthetic write failure")),
        );

        assert!(result.is_err());
        assert_eq!(current.model_environment_key_identity, "approved-endpoint");
    }

    #[test]
    fn persisted_update_publishes_the_reconciled_snapshot() {
        let current: AppSettings =
            toml::from_str("translate-api-key = \"stale-memory-sentinel\"").unwrap();

        let persisted = AppSettings::prepare_persisted_update(
            &current,
            |_| {},
            |candidate| {
                let mut written = candidate.clone();
                written.clear_legacy_model_api_key();
                Ok(written)
            },
        )
        .unwrap();

        assert!(persisted.legacy_model_api_key().is_none());
    }

    #[test]
    fn app_settings_serialization_is_transparent_to_the_core_schema() {
        let mut settings = AppSettings::default();
        settings.theme = ThemePreference::Dark;

        let text = toml::to_string_pretty(&settings).unwrap();
        let back: AppSettings = toml::from_str(&text).unwrap();

        assert!(text.contains(r#"theme = "dark""#));
        assert_eq!(back, settings);
    }

    #[test]
    fn default_preset_ids_name_real_presets() {
        let settings = AppSettings::default();
        assert_eq!(
            crate::theme::by_id(&settings.theme_light, false).id,
            settings.theme_light
        );
        assert_eq!(
            crate::theme::by_id(&settings.theme_dark, true).id,
            settings.theme_dark
        );
    }
}
