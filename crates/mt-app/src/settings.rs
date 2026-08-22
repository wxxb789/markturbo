//! User settings: one global, persisted as TOML.
//!
//! Kept in `mt-app` rather than `mt-doc` because these are application
//! preferences (theme, provider, list grouping) — the document engine stays
//! headless and has no opinion about them.
//!
//! TOML rather than JSON because a settings file is something a person opens.
//! It takes comments, which JSON cannot, and it does not fail on a trailing
//! comma. The format has one rule that constrains this file: every scalar must
//! be written before any table, so [`AppSettings`] must stay flat — a nested
//! struct or map would serialize to a document `toml` refuses to read back.
//! `the_settings_document_is_flat_enough_for_toml_to_read_back` holds that.
//!
//! Persistence is best-effort by design: a corrupt or unreadable settings file
//! must never stop the app opening. A bad file falls back to defaults, and the
//! next successful write replaces it.

use std::path::PathBuf;

use gpui::{App, Global};
use serde::{Deserialize, Serialize};

/// Which theme to use.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum ThemePreference {
    /// Follow the OS. The default: an app that ignores the system theme is the
    /// one bright window at night.
    #[default]
    System,
    Light,
    Dark,
}

impl ThemePreference {
    pub const ALL: [ThemePreference; 3] = [
        ThemePreference::System,
        ThemePreference::Light,
        ThemePreference::Dark,
    ];

    pub fn label(self) -> &'static str {
        match self {
            ThemePreference::System => "System",
            ThemePreference::Light => "Light",
            ThemePreference::Dark => "Dark",
        }
    }

    /// The serialized form, which is also the settings-dropdown key.
    pub fn key(self) -> &'static str {
        match self {
            ThemePreference::System => "system",
            ThemePreference::Light => "light",
            ThemePreference::Dark => "dark",
        }
    }

    pub fn from_key(key: &str) -> Self {
        Self::ALL
            .into_iter()
            .find(|p| p.key() == key)
            .unwrap_or_default()
    }
}

/// Which language the interface is written in.
///
/// Separate from `translate_to`, which is about documents: a user may well read
/// the UI in Chinese while translating documents into Japanese.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum Language {
    #[default]
    #[serde(rename = "en-us")]
    English,
    #[serde(rename = "zh-cn")]
    Chinese,
}

impl Language {
    pub const ALL: [Language; 2] = [Language::English, Language::Chinese];

    /// The language's own name, written in itself.
    ///
    /// A picker that lists "Chinese" in English is unreadable to exactly the
    /// person who needs it; every UI that gets this right uses endonyms.
    pub fn label(self) -> &'static str {
        match self {
            Language::English => "English",
            Language::Chinese => "简体中文",
        }
    }

    /// BCP-47 tag, which is also the settings key.
    pub fn key(self) -> &'static str {
        match self {
            Language::English => "en-us",
            Language::Chinese => "zh-cn",
        }
    }

    pub fn from_key(key: &str) -> Self {
        Self::ALL
            .into_iter()
            .find(|l| l.key().eq_ignore_ascii_case(key))
            .unwrap_or_default()
    }
}

/// How to group the Skills list.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum GroupBy {
    /// Workspace vs global.
    #[default]
    Origin,
    /// Which harness's directory it was found in (`claude-code`, `codex`, …).
    Harness,
    /// Valid vs invalid, so a conformance sweep is one click.
    Status,
    /// One flat alphabetical list.
    None,
}

impl GroupBy {
    pub const ALL: [GroupBy; 4] = [
        GroupBy::Origin,
        GroupBy::Harness,
        GroupBy::Status,
        GroupBy::None,
    ];

    pub fn label(self) -> &'static str {
        match self {
            GroupBy::Origin => "Origin",
            GroupBy::Harness => "Harness",
            GroupBy::Status => "Status",
            GroupBy::None => "None",
        }
    }

    pub fn key(self) -> &'static str {
        match self {
            GroupBy::Origin => "origin",
            GroupBy::Harness => "harness",
            GroupBy::Status => "status",
            GroupBy::None => "none",
        }
    }

    pub fn from_key(key: &str) -> Self {
        Self::ALL
            .into_iter()
            .find(|g| g.key() == key)
            .unwrap_or_default()
    }
}

/// Everything the user can configure.
///
/// `#[serde(default)]` on every field is what makes a settings file written by
/// an older build still load after a field is added.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "kebab-case")]
pub struct AppSettings {
    pub theme: ThemePreference,
    /// Preset id used when the effective mode is light.
    ///
    /// Two ids rather than one: the theme preference can be `System`, and a
    /// machine that flips at sunset should land on the user's chosen dark preset
    /// rather than a generic one.
    pub theme_light: String,
    /// Preset id used when the effective mode is dark.
    pub theme_dark: String,
    /// Which language the interface itself is written in.
    pub language: Language,
    /// Target language for translation, e.g. `zh`.
    pub translate_to: String,
    /// Provider id, matching [`crate::translate::Provider::key`].
    pub translate_provider: String,
    /// Model id for providers that take one.
    pub translate_model: String,
    /// API key for the translation endpoint.
    ///
    /// Takes priority over the provider's environment variable: a key typed
    /// into Settings is an explicit choice, and there is otherwise no way to
    /// override what the shell exported from inside the app. Empty falls back
    /// to the environment, which remains the option for anyone who would rather
    /// a key never touched disk.
    pub translate_api_key: String,
    /// Base URL of the translation endpoint.
    ///
    /// Empty means the schema's own default. Setting it is what points the app
    /// at a self-hosted or proxied server — the wire format is the same, so an
    /// OpenAI-compatible endpoint needs nothing else.
    pub translate_base_url: String,
    /// Scroll the preview to follow the editor in Split mode.
    ///
    /// Off by default: the mapping is proportional, so on a document with one
    /// tall diagram the preview moves further than the eye expects. A user who
    /// wants the panes locked together says so.
    pub split_sync_scroll: bool,
    /// Reload an open document when the file changes on disk.
    ///
    /// Off by default: a reload the user did not ask for replaces what is on
    /// screen mid-read, so the safe version of this feature is the one they opt
    /// into. Even then the reload skips documents with unsaved edits — those
    /// keep the conflict banner, because automatic refresh must never discard
    /// typed text.
    pub watch_auto_reload: bool,
    /// Search the harness global directories as well as the workspace.
    pub skills_include_global: bool,
    /// Show skills marked `metadata.internal: true`.
    pub skills_include_internal: bool,
    pub skills_group_by: GroupBy,
}

impl Default for AppSettings {
    fn default() -> Self {
        Self {
            theme: ThemePreference::default(),
            theme_light: crate::theme::DEFAULT_LIGHT.into(),
            theme_dark: crate::theme::DEFAULT_DARK.into(),
            language: Language::default(),
            translate_to: "zh".into(),
            // Empty means "whatever is configured and available", so a machine
            // that gains an API key starts using it without editing settings.
            translate_provider: String::new(),
            translate_model: String::new(),
            translate_api_key: String::new(),
            translate_base_url: String::new(),
            split_sync_scroll: false,
            watch_auto_reload: false,
            skills_include_global: true,
            skills_include_internal: false,
            skills_group_by: GroupBy::default(),
        }
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
        cx.set_global(Self::load());
    }

    /// Apply `edit`, then persist. Every setter goes through this so no change
    /// can be made that is forgotten on restart.
    ///
    /// Observers registered with `cx.observe_global::<AppSettings>` are notified
    /// automatically: `global_mut` pushes a `NotifyGlobalObservers` effect, so
    /// there is nothing to call here. That is what replaced every setter
    /// hand-appending its own `relabel`/`rescan` follow-up — and forgetting to
    /// for eight of the fourteen settings.
    pub fn update(cx: &mut App, edit: impl FnOnce(&mut AppSettings)) {
        edit(Self::global_mut(cx));
        let settings = Self::global(cx).clone();
        settings.save();
    }

    /// Read the settings file, falling back to defaults.
    pub fn load() -> Self {
        settings_path()
            .map(|path| Self::load_from(&path))
            .unwrap_or_default()
    }

    /// Read from an explicit path. Anything unreadable or malformed yields
    /// defaults — a settings file must never be able to stop the app opening.
    pub fn load_from(path: &std::path::Path) -> Self {
        let Ok(text) = std::fs::read_to_string(path) else {
            return Self::default();
        };
        match toml::from_str(&text) {
            Ok(settings) => settings,
            Err(err) => {
                // Do not delete or rewrite it: the user may have hand-edited it
                // and a diagnostic they can act on beats silent data loss.
                log::warn!("ignoring malformed {}: {err}", path.display());
                Self::default()
            }
        }
    }

    /// Write to disk. Failures are logged, never fatal.
    pub fn save(&self) {
        let Some(path) = settings_path() else { return };
        self.save_to(&path);
    }

    pub fn save_to(&self, path: &std::path::Path) {
        if let Some(parent) = path.parent()
            && let Err(err) = std::fs::create_dir_all(parent)
        {
            log::warn!("cannot create {}: {err}", parent.display());
            return;
        }
        match toml::to_string_pretty(self) {
            Ok(text) => {
                if let Err(err) = std::fs::write(path, text) {
                    log::warn!("cannot write {}: {err}", path.display());
                }
            }
            Err(err) => log::warn!("cannot serialize settings: {err}"),
        }
    }
}

/// Where settings live.
///
/// `$MARKTURBO_CONFIG_DIR` first so a test or a portable install can redirect
/// it — the tests rely on that and would otherwise write to the developer's own
/// configuration. Otherwise the platform's own answer, via `dirs`:
/// `%APPDATA%\markturbo` on Windows, `~/Library/Application Support/markturbo`
/// on macOS, `$XDG_CONFIG_HOME/markturbo` (or `~/.config/markturbo`) elsewhere.
///
/// The macOS path is a deliberate change from the `~/.config` this used to
/// hand-roll. `~/.config` is the XDG answer and macOS is not an XDG platform;
/// a file there is invisible to every macOS convention for finding, backing up,
/// or migrating application data.
pub fn settings_path() -> Option<PathBuf> {
    Some(config_dir()?.join("settings.toml"))
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
pub fn apply_theme(preference: ThemePreference, window: Option<&mut gpui::Window>, cx: &mut App) {
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
fn resolve_dark(preference: ThemePreference, window: Option<&gpui::Window>, cx: &App) -> bool {
    use gpui_component::ThemeMode;

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
    gpui_component::Theme::global(cx).mode.is_dark()
}

fn config_dir() -> Option<PathBuf> {
    if let Some(dir) = env_path("MARKTURBO_CONFIG_DIR") {
        return Some(dir);
    }
    // `dirs` rather than four hand-rolled branches: it is the same answer this
    // file used to compute on Windows and Linux, and the *correct* one on macOS,
    // which the hand-rolled version got wrong by treating it as an XDG platform.
    Some(dirs::config_dir()?.join("markturbo"))
}

/// An environment variable as a path, treating blank as unset — a variable set
/// to whitespace would otherwise produce a path at the filesystem root.
fn env_path(var: &str) -> Option<PathBuf> {
    let value = std::env::var(var).ok()?;
    let trimmed = value.trim();
    (!trimmed.is_empty()).then(|| PathBuf::from(trimmed))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_follow_the_system_theme() {
        assert_eq!(AppSettings::default().theme, ThemePreference::System);
    }

    #[test]
    fn keys_round_trip() {
        for pref in ThemePreference::ALL {
            assert_eq!(ThemePreference::from_key(pref.key()), pref);
        }
        for group in GroupBy::ALL {
            assert_eq!(GroupBy::from_key(group.key()), group);
        }
        // An unknown key must not panic; it falls back to the default.
        assert_eq!(
            ThemePreference::from_key("nonsense"),
            ThemePreference::System
        );
        assert_eq!(GroupBy::from_key(""), GroupBy::Origin);
    }

    #[test]
    fn settings_round_trip_through_toml() {
        let settings = AppSettings {
            theme: ThemePreference::Dark,
            translate_to: "ja".into(),
            skills_group_by: GroupBy::Harness,
            ..AppSettings::default()
        };

        let text = toml::to_string_pretty(&settings).unwrap();
        let back: AppSettings = toml::from_str(&text).unwrap();
        assert_eq!(back, settings);
    }

    /// TOML cannot represent this struct if it ever gains a nested one.
    ///
    /// The rule that bites: every scalar must be emitted before any table, so a
    /// struct mixing scalars with a nested struct or map serializes to a
    /// document `toml` itself refuses to re-parse — or errors outright on the
    /// way out. Today every field is a scalar, and this fails the moment one is
    /// not, which is the point.
    #[test]
    fn the_settings_document_is_flat_enough_for_toml_to_read_back() {
        let text = toml::to_string_pretty(&AppSettings::default()).unwrap();
        assert!(
            !text.contains('['),
            "a table appeared; TOML needs every scalar above it:
{text}"
        );
        toml::from_str::<AppSettings>(&text).expect("its own output must parse");
    }

    /// The unit enums must land as plain strings, under kebab-case names.
    ///
    /// `#[serde(rename_all = "kebab-case")]` on the struct renames the *fields*,
    /// so it is `theme-light`, not `theme_light` — a test asserting the latter
    /// would pass against JSON and lie about TOML.
    #[test]
    fn enums_and_field_names_survive_as_the_user_would_type_them() {
        let settings = AppSettings {
            theme: ThemePreference::Dark,
            language: Language::Chinese,
            skills_group_by: GroupBy::Harness,
            ..AppSettings::default()
        };
        let text = toml::to_string_pretty(&settings).unwrap();

        assert!(text.contains(r#"theme = "dark""#), "{text}");
        assert!(text.contains(r#"language = "zh-cn""#), "{text}");
        assert!(text.contains(r#"skills-group-by = "harness""#), "{text}");
        assert!(text.contains("theme-light = "), "{text}");
        assert!(
            !text.contains("theme_light"),
            "field names are kebab-case, not snake:
{text}"
        );
    }

    #[test]
    fn a_partial_file_keeps_the_defaults_for_missing_fields() {
        // What a settings file written by an older build looks like.
        let back: AppSettings = toml::from_str(r#"theme = "dark""#).unwrap();
        assert_eq!(back.theme, ThemePreference::Dark);
        assert_eq!(back.translate_to, AppSettings::default().translate_to);
        assert_eq!(back.skills_group_by, GroupBy::Origin);
        // Added after the first release: a file that predates presets must
        // still resolve to a real one rather than an empty id.
        assert_eq!(back.theme_light, crate::theme::DEFAULT_LIGHT);
        assert_eq!(back.theme_dark, crate::theme::DEFAULT_DARK);
    }

    #[test]
    fn the_default_preset_ids_name_real_presets() {
        // A default that does not resolve would silently fall back, hiding a
        // typo behind correct-looking behavior.
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

    #[test]
    fn auto_reload_is_off_until_the_user_asks_for_it() {
        // A reload nobody asked for swaps the document out from under the
        // reader, so this one must stay opt-in — and a settings file written
        // before the field existed must not turn it on.
        assert!(!AppSettings::default().watch_auto_reload);
        let back: AppSettings = toml::from_str(r#"theme = "dark""#).unwrap();
        assert!(!back.watch_auto_reload);

        let settings = AppSettings {
            watch_auto_reload: true,
            ..AppSettings::default()
        };
        let text = toml::to_string_pretty(&settings).unwrap();
        let back: AppSettings = toml::from_str(&text).unwrap();
        assert!(back.watch_auto_reload);
    }

    #[test]
    fn a_malformed_file_falls_back_rather_than_failing_to_start() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.toml");
        std::fs::write(&path, "this = is not [ valid toml").unwrap();

        assert_eq!(AppSettings::load_from(&path), AppSettings::default());
        // The bad file is left alone: the user may have hand-edited it.
        assert!(path.exists(), "must not delete what it could not read");
    }

    #[test]
    fn a_missing_file_is_not_an_error() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("never-written.toml");
        assert_eq!(AppSettings::load_from(&path), AppSettings::default());
    }

    #[test]
    fn save_then_load_preserves_every_field() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nested").join("settings.toml");

        let settings = AppSettings {
            theme: ThemePreference::Light,
            theme_light: "sepia".into(),
            theme_dark: "nord".into(),
            language: Language::Chinese,
            split_sync_scroll: true,
            watch_auto_reload: true,
            translate_provider: "anthropic".into(),
            translate_api_key: "sk-test".into(),
            translate_base_url: "https://gw.invalid/openai".into(),
            translate_model: "claude-sonnet-5".into(),
            skills_include_internal: true,
            skills_include_global: false,
            skills_group_by: GroupBy::Status,
            translate_to: "de".into(),
        };
        // The parent does not exist yet: saving must create it.
        settings.save_to(&path);

        assert_eq!(AppSettings::load_from(&path), settings);
    }

    #[test]
    fn the_settings_path_is_under_a_markturbo_directory() {
        // Whichever platform branch is taken, the file must not land loose in
        // the user's config root next to everyone else's.
        let path = settings_path().expect("a platform default is always available");
        assert_eq!(
            path.file_name().and_then(|n| n.to_str()),
            Some("settings.toml")
        );
        assert!(
            path.parent().is_some_and(|p| p.ends_with("markturbo")),
            "got {}",
            path.display()
        );
    }

    #[test]
    fn a_blank_env_path_is_treated_as_unset() {
        // A variable set to whitespace would otherwise resolve to the
        // filesystem root.
        // SAFETY: single-threaded, and the variable is unique to this test.
        unsafe { std::env::set_var("MT_TEST_BLANK_PATH", "   ") };
        assert_eq!(env_path("MT_TEST_BLANK_PATH"), None);
        unsafe { std::env::set_var("MT_TEST_BLANK_PATH", " /tmp/x ") };
        assert_eq!(
            env_path("MT_TEST_BLANK_PATH"),
            Some(PathBuf::from("/tmp/x"))
        );
        unsafe { std::env::remove_var("MT_TEST_BLANK_PATH") };
    }
}
