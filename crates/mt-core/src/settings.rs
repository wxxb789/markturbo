//! Headless user settings: serializable schema and best-effort TOML persistence.
//!
//! Presentation stays in `mt-app`; this module owns only data, defaults, recent
//! target invariants, configuration path policy, and persistence/security rules.
//! A malformed settings file is left untouched and falls back to defaults so it
//! cannot prevent the application from opening.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// Stable preset id used for the default light appearance.
pub const DEFAULT_LIGHT_THEME_ID: &str = "light";
/// Stable preset id used for the default dark appearance.
pub const DEFAULT_DARK_THEME_ID: &str = "dark";

#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(transparent)]
struct LegacyCredential(String);

impl std::fmt::Debug for LegacyCredential {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("<redacted>")
    }
}

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
            .find(|preference| preference.key() == key)
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
            .find(|language| language.key().eq_ignore_ascii_case(key))
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
            .find(|group| group.key() == key)
            .unwrap_or_default()
    }
}

/// A target the user can reopen from the welcome state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum RecentTargetKind {
    #[default]
    File,
    Workspace,
}

/// Presentation data for an entry in the recently opened target list.
///
/// `path` is the source of truth; the kind and display name are persisted only
/// so the welcome screen can describe the target without opening it first.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "kebab-case")]
pub struct RecentTarget {
    pub path: PathBuf,
    pub kind: RecentTargetKind,
    pub display_name: String,
}

impl RecentTarget {
    pub fn new(
        path: impl Into<PathBuf>,
        kind: RecentTargetKind,
        display_name: impl Into<String>,
    ) -> Self {
        Self {
            path: path.into(),
            kind,
            display_name: display_name.into(),
        }
    }
}

impl Default for RecentTarget {
    fn default() -> Self {
        Self {
            path: PathBuf::new(),
            kind: RecentTargetKind::default(),
            display_name: String::new(),
        }
    }
}

/// The welcome screen keeps its history short enough to scan.
pub const MAX_RECENT_TARGETS: usize = 10;

/// Serializable settings shared with the presentation layer.
///
/// `#[serde(default)]` on every field lets settings written by older builds
/// load after fields are added. Scalar fields precede `recent_targets`: TOML
/// requires scalars to appear before an array table.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "kebab-case")]
pub struct SettingsData {
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
    /// Provider wire-format id shared by every model-backed operation.
    #[serde(alias = "translate-provider")]
    pub model_provider: String,
    /// Model id shared by every model-backed operation.
    #[serde(alias = "translate-model")]
    pub model_name: String,
    /// Base URL shared by every model-backed operation.
    ///
    /// Empty means the schema's own default. Setting it is what points the app
    /// at a self-hosted or proxied server — the wire format is the same, so an
    /// OpenAI-compatible endpoint needs nothing else beyond including the
    /// version segment: `http://127.0.0.1:8000/v1`, not `http://127.0.0.1:8000`.
    /// Only the leaf path is appended, so a base URL missing `/v1` reaches an
    /// endpoint that is not there.
    #[serde(alias = "translate-base-url")]
    pub model_base_url: String,
    /// Exact custom endpoint identity authorized to receive the provider's
    /// ambient environment credential. Empty means no custom authorization.
    pub model_environment_key_identity: String,
    /// A key written by an older build. It remains in the original settings
    /// file until the user approves migration and secure storage verifies the
    /// write; current UI never creates or displays this field.
    #[serde(
        rename = "translate-api-key",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    legacy_model_api_key: Option<LegacyCredential>,
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
    /// Whether a no-argument launch begins in the welcome state.
    pub show_welcome_on_startup: bool,
    /// Most-recently used paths shown by the welcome state.
    ///
    /// This must remain last so TOML writes all scalar fields before its array
    /// tables and can read its own output back.
    pub recent_targets: Vec<RecentTarget>,
}

impl Default for SettingsData {
    fn default() -> Self {
        Self {
            theme: ThemePreference::default(),
            theme_light: DEFAULT_LIGHT_THEME_ID.into(),
            theme_dark: DEFAULT_DARK_THEME_ID.into(),
            language: Language::default(),
            translate_to: "zh".into(),
            // Empty means "whatever is configured and available", so a machine
            // that gains an API key starts using it without editing settings.
            model_provider: String::new(),
            model_name: String::new(),
            model_base_url: String::new(),
            model_environment_key_identity: String::new(),
            legacy_model_api_key: None,
            split_sync_scroll: false,
            watch_auto_reload: false,
            skills_include_global: true,
            skills_include_internal: false,
            skills_group_by: GroupBy::default(),
            show_welcome_on_startup: true,
            recent_targets: Vec::new(),
        }
    }
}

impl SettingsData {
    /// Place `target` first, replace an older entry for the same path, and cap
    /// the history at [`MAX_RECENT_TARGETS`].
    pub fn record_recent_target(&mut self, target: RecentTarget) {
        if target.path.as_os_str().is_empty() {
            return;
        }

        self.recent_targets
            .retain(|existing| existing.path != target.path);
        self.recent_targets.insert(0, target);
        self.recent_targets.truncate(MAX_RECENT_TARGETS);
    }

    /// Remove a stale or user-dismissed recent target by path.
    pub fn remove_recent_target(&mut self, path: &Path) -> bool {
        let len = self.recent_targets.len();
        self.recent_targets.retain(|target| target.path != path);
        self.recent_targets.len() != len
    }

    fn normalize_recent_targets(&mut self) {
        let mut normalized = Vec::with_capacity(self.recent_targets.len().min(MAX_RECENT_TARGETS));

        for target in std::mem::take(&mut self.recent_targets) {
            if target.path.as_os_str().is_empty()
                || normalized
                    .iter()
                    .any(|existing: &RecentTarget| existing.path == target.path)
            {
                continue;
            }

            normalized.push(target);
            if normalized.len() == MAX_RECENT_TARGETS {
                break;
            }
        }

        self.recent_targets = normalized;
    }

    /// Read the settings file, falling back to defaults.
    pub fn load() -> Self {
        settings_path()
            .map(|path| Self::load_from(&path))
            .unwrap_or_default()
    }

    /// Read from an explicit path. Anything unreadable or malformed yields
    /// defaults — a settings file must never be able to stop the app opening.
    pub fn load_from(path: &Path) -> Self {
        let Ok(text) = std::fs::read_to_string(path) else {
            return Self::default();
        };
        match toml::from_str::<Self>(&text) {
            Ok(mut settings) => {
                settings.normalize_recent_targets();
                settings
            }
            Err(error) => {
                // Do not delete or rewrite it: the user may have hand-edited it
                // and a diagnostic they can act on beats silent data loss. The
                // parser error can quote source text, including a legacy key.
                let _ = error;
                log::warn!("ignoring malformed {}", path.display());
                Self::default()
            }
        }
    }

    /// Write to disk. Failures are logged, never fatal.
    pub fn save(&self) {
        let Some(path) = settings_path() else { return };
        if let Err(error) = self.try_save_to_with_intent(&path, SettingsWriteIntent::General) {
            log::warn!("cannot write {}: {error}", path.display());
        }
    }

    /// Write a complete settings snapshot to an explicit path.
    /// Available only to tests and test-support builds.
    #[cfg(any(test, feature = "test-support"))]
    pub fn save_to(&self, path: &Path) {
        if let Err(error) = self.write_to(path) {
            log::warn!("cannot write {}: {error}", path.display());
        }
    }

    /// Persist a non-security settings edit, preserving the latest on-disk
    /// credential and endpoint authorization state.
    ///
    /// This is exposed for the `mt-app` global adapter; the write intent itself
    /// remains private to this module.
    pub fn try_save_general(&self) -> std::io::Result<Self> {
        self.try_save_with_intent(SettingsWriteIntent::General)
    }

    /// Save a general edit at an explicit path, retaining newer credential
    /// and endpoint authorization fields from the on-disk snapshot.
    #[cfg(test)]
    pub(crate) fn try_save_general_to(&self, path: &Path) -> std::io::Result<Self> {
        self.try_save_to_with_intent(path, SettingsWriteIntent::General)
    }

    /// Persist an explicit environment-credential authorization change.
    ///
    /// This is exposed only so the `mt-app` adapter can persist the authorization
    /// before publishing it to GPUI observers.
    pub fn try_save_environment_credential_authorization(&self) -> std::io::Result<Self> {
        self.try_save_with_intent(SettingsWriteIntent::EnvironmentCredentialAuthorization)
    }

    /// Clear a legacy credential only if the current file still contains the
    /// verified snapshot's exact value and still identifies the verified
    /// endpoint. The write preserves every other field from the latest file.
    pub(crate) fn try_save_after_legacy_migration(
        &self,
        target: &str,
        path: &Path,
    ) -> Result<Self, LegacyCredentialMigrationSaveError> {
        let _guard = lock_settings_writes().map_err(LegacyCredentialMigrationSaveError::Io)?;
        let latest = match std::fs::read_to_string(path) {
            Ok(contents) => toml::from_str::<Self>(&contents).map_err(|_| {
                LegacyCredentialMigrationSaveError::Io(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "existing settings are malformed",
                ))
            })?,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Self::default(),
            Err(error) => return Err(LegacyCredentialMigrationSaveError::Io(error)),
        };

        if self.legacy_model_api_key.is_none()
            || self.legacy_model_api_key != latest.legacy_model_api_key
            || !self.matches_model_credential_target(target)
            || !latest.matches_model_credential_target(target)
        {
            return Err(LegacyCredentialMigrationSaveError::Conflict);
        }

        let mut migrated = latest;
        migrated.legacy_model_api_key = None;
        migrated
            .write_to(path)
            .map_err(LegacyCredentialMigrationSaveError::Io)?;
        Ok(migrated)
    }

    /// The only value-bearing access to the legacy credential is a trimmed,
    /// borrowed value for the secure-storage migration workflow.
    pub fn legacy_model_api_key(&self) -> Option<&str> {
        self.legacy_model_api_key
            .as_ref()
            .map(|credential| credential.0.trim())
            .filter(|credential| !credential.is_empty())
    }

    fn matches_model_credential_target(&self, target: &str) -> bool {
        let Some(provider) = crate::model::Provider::from_key(self.model_provider.trim()) else {
            return false;
        };
        crate::model::EndpointIdentity::parse(provider, Some(&self.model_base_url))
            .is_ok_and(|endpoint| endpoint.credential_target().as_str() == target)
    }

    /// Change only an in-memory copy of the legacy credential field.
    #[cfg(any(test, feature = "test-support"))]
    pub fn clear_legacy_model_api_key(&mut self) {
        self.legacy_model_api_key = None;
    }

    fn try_save_with_intent(&self, intent: SettingsWriteIntent) -> std::io::Result<Self> {
        let path = settings_path().ok_or_else(|| {
            std::io::Error::new(std::io::ErrorKind::NotFound, "settings path unavailable")
        })?;
        self.try_save_to_with_intent(&path, intent)
    }

    fn try_save_to_with_intent(
        &self,
        path: &Path,
        intent: SettingsWriteIntent,
    ) -> std::io::Result<Self> {
        let _guard = lock_settings_writes()?;
        let current = match std::fs::read_to_string(path) {
            Ok(current) => toml::from_str::<Self>(&current).map_err(|_| {
                std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "existing settings are malformed",
                )
            })?,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Self::default(),
            Err(error) => return Err(error),
        };
        let mut candidate = self.clone();
        candidate.legacy_model_api_key = current.legacy_model_api_key;
        if intent != SettingsWriteIntent::EnvironmentCredentialAuthorization {
            candidate.model_environment_key_identity = current.model_environment_key_identity;
        }
        candidate.write_to(path)?;
        Ok(candidate)
    }

    fn write_to(&self, path: &Path) -> std::io::Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let text = toml::to_string_pretty(self).map_err(|error| {
            std::io::Error::other(format!("cannot serialize settings: {error}"))
        })?;
        std::fs::write(path, text)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SettingsWriteIntent {
    General,
    EnvironmentCredentialAuthorization,
}

#[derive(Debug)]
pub(crate) enum LegacyCredentialMigrationSaveError {
    Conflict,
    Io(std::io::Error),
}

#[cfg(target_os = "windows")]
struct SettingsWriteGuard {
    handle: windows::Win32::Foundation::HANDLE,
}

#[cfg(target_os = "windows")]
impl Drop for SettingsWriteGuard {
    fn drop(&mut self) {
        use windows::Win32::{Foundation::CloseHandle, System::Threading::ReleaseMutex};

        let _ = unsafe { ReleaseMutex(self.handle) };
        let _ = unsafe { CloseHandle(self.handle) };
    }
}

#[cfg(target_os = "windows")]
fn lock_settings_writes() -> std::io::Result<SettingsWriteGuard> {
    lock_settings_writes_named("Global\\markturbo-settings-write-v1")
}

#[cfg(target_os = "windows")]
fn lock_settings_writes_named(name: &str) -> std::io::Result<SettingsWriteGuard> {
    use windows::Win32::{
        Foundation::{CloseHandle, GetLastError, WAIT_ABANDONED, WAIT_OBJECT_0, WAIT_TIMEOUT},
        System::Threading::{CreateMutexW, WaitForSingleObject},
    };
    use windows::core::PCWSTR;

    const WAIT_MILLISECONDS: u32 = 100;
    let name: Vec<u16> = name.encode_utf16().chain(std::iter::once(0)).collect();
    let handle = unsafe { CreateMutexW(None, false, PCWSTR(name.as_ptr())) }
        .map_err(std::io::Error::other)?;
    let wait = unsafe { WaitForSingleObject(handle, WAIT_MILLISECONDS) };
    if wait == WAIT_OBJECT_0 || wait == WAIT_ABANDONED {
        return Ok(SettingsWriteGuard { handle });
    }

    let error = if wait == WAIT_TIMEOUT {
        std::io::Error::new(
            std::io::ErrorKind::WouldBlock,
            "another markturbo process is writing settings",
        )
    } else {
        std::io::Error::from_raw_os_error(unsafe { GetLastError() }.0 as i32)
    };
    let _ = unsafe { CloseHandle(handle) };
    Err(error)
}

#[cfg(not(target_os = "windows"))]
struct SettingsWriteGuard {
    _guard: std::sync::MutexGuard<'static, ()>,
}

#[cfg(not(target_os = "windows"))]
fn lock_settings_writes() -> std::io::Result<SettingsWriteGuard> {
    static WRITES: std::sync::Mutex<()> = std::sync::Mutex::new(());
    Ok(SettingsWriteGuard {
        _guard: WRITES
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()),
    })
}

/// Where settings live.
///
/// `$MARKTURBO_CONFIG_DIR` first so a test or a portable install can redirect
/// it. Otherwise the platform's own answer, via `dirs`: `%APPDATA%\\markturbo`
/// on Windows, `~/Library/Application Support/markturbo` on macOS,
/// `$XDG_CONFIG_HOME/markturbo` (or `~/.config/markturbo`) elsewhere.
///
/// The macOS path deliberately uses its native application-support location;
/// macOS is not an XDG platform.
pub fn settings_path() -> Option<PathBuf> {
    Some(config_dir()?.join("settings.toml"))
}

fn config_dir() -> Option<PathBuf> {
    if let Some(dir) = env_path("MARKTURBO_CONFIG_DIR") {
        return Some(dir);
    }
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
        assert_eq!(SettingsData::default().theme, ThemePreference::System);
    }

    #[test]
    fn keys_round_trip() {
        for preference in ThemePreference::ALL {
            assert_eq!(ThemePreference::from_key(preference.key()), preference);
        }
        for group in GroupBy::ALL {
            assert_eq!(GroupBy::from_key(group.key()), group);
        }
        assert_eq!(
            ThemePreference::from_key("nonsense"),
            ThemePreference::System
        );
        assert_eq!(GroupBy::from_key(""), GroupBy::Origin);
    }

    #[test]
    fn settings_round_trip_through_toml() {
        let settings = SettingsData {
            theme: ThemePreference::Dark,
            translate_to: "ja".into(),
            skills_group_by: GroupBy::Harness,
            show_welcome_on_startup: false,
            recent_targets: vec![RecentTarget::new(
                PathBuf::from("C:/work/notes.md"),
                RecentTargetKind::File,
                "notes.md",
            )],
            ..SettingsData::default()
        };

        let text = toml::to_string_pretty(&settings).unwrap();
        let back: SettingsData = toml::from_str(&text).unwrap();
        assert_eq!(back, settings);
    }

    #[test]
    fn current_settings_never_serialize_an_api_credential_field() {
        let text = toml::to_string_pretty(&SettingsData::default()).unwrap();
        assert!(
            !text.contains("api-key"),
            "credential field leaked:\n{text}"
        );
    }

    #[test]
    fn stale_general_save_cannot_restore_revoked_environment_authorization() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.toml");
        let mut authorized = SettingsData {
            model_environment_key_identity: "markturbo:model:approved".into(),
            ..SettingsData::default()
        };
        authorized.write_to(&path).unwrap();
        let mut stale = SettingsData::load_from(&path);

        authorized.model_environment_key_identity.clear();
        let persisted = authorized
            .try_save_to_with_intent(
                &path,
                SettingsWriteIntent::EnvironmentCredentialAuthorization,
            )
            .unwrap();
        assert!(persisted.model_environment_key_identity.is_empty());

        stale.theme = ThemePreference::Dark;
        let persisted = stale
            .try_save_to_with_intent(&path, SettingsWriteIntent::General)
            .unwrap();
        assert_eq!(persisted.theme, ThemePreference::Dark);
        assert!(persisted.model_environment_key_identity.is_empty());
        assert!(
            SettingsData::load_from(&path)
                .model_environment_key_identity
                .is_empty()
        );
    }

    #[test]
    fn stale_security_state_is_not_written_when_the_settings_file_is_missing() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("missing-settings.toml");
        let stale: SettingsData = toml::from_str(
            "model-environment-key-identity = \"stale-authorization\"\n\
             translate-api-key = \"stale-plaintext\"\n",
        )
        .unwrap();

        let persisted = stale
            .try_save_to_with_intent(&path, SettingsWriteIntent::General)
            .unwrap();

        assert!(persisted.model_environment_key_identity.is_empty());
        assert!(persisted.legacy_model_api_key().is_none());
        let text = std::fs::read_to_string(path).unwrap();
        assert!(!text.contains("stale-authorization"));
        assert!(!text.contains("stale-plaintext"));
    }

    #[test]
    fn malformed_current_settings_block_a_stale_security_write() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.toml");
        let malformed = "translate-api-key = \"current-plaintext\"\n[";
        std::fs::write(&path, malformed).unwrap();
        let stale: SettingsData = toml::from_str(
            "model-environment-key-identity = \"stale-authorization\"\n\
             translate-api-key = \"stale-plaintext\"\n",
        )
        .unwrap();

        let error = stale
            .try_save_to_with_intent(&path, SettingsWriteIntent::General)
            .unwrap_err();

        assert_eq!(error.kind(), std::io::ErrorKind::InvalidData);
        assert_eq!(std::fs::read_to_string(path).unwrap(), malformed);
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn settings_writes_fail_closed_while_the_global_lock_is_busy() {
        let name = format!(
            "Local\\markturbo-settings-write-test-{}",
            std::process::id()
        );
        let _guard = lock_settings_writes_named(&name).unwrap();
        let error = std::thread::spawn(move || {
            lock_settings_writes_named(&name)
                .err()
                .expect("the lock is busy")
        })
        .join()
        .unwrap();

        assert_eq!(error.kind(), std::io::ErrorKind::WouldBlock);
    }

    /// TOML cannot represent a scalar after an array table.
    #[test]
    fn the_settings_document_serializes_recent_targets_last_and_reads_back() {
        let settings = SettingsData {
            recent_targets: vec![RecentTarget::new(
                PathBuf::from("C:/work"),
                RecentTargetKind::Workspace,
                "work",
            )],
            ..SettingsData::default()
        };
        let text = toml::to_string_pretty(&settings).unwrap();
        let recent_start = text
            .find("[[recent-targets]]")
            .expect("recent targets must be TOML array tables");
        assert!(
            !text[..recent_start].contains('['),
            "a table appeared before the recent list; TOML needs every scalar above:\n{text}"
        );
        assert_eq!(toml::from_str::<SettingsData>(&text).unwrap(), settings);
    }

    #[test]
    fn enums_and_field_names_survive_as_the_user_would_type_them() {
        let settings = SettingsData {
            theme: ThemePreference::Dark,
            language: Language::Chinese,
            skills_group_by: GroupBy::Harness,
            ..SettingsData::default()
        };
        let text = toml::to_string_pretty(&settings).unwrap();

        assert!(text.contains(r#"theme = "dark""#), "{text}");
        assert!(text.contains(r#"language = "zh-cn""#), "{text}");
        assert!(text.contains(r#"skills-group-by = "harness""#), "{text}");
        assert!(text.contains("theme-light = "), "{text}");
        assert!(
            !text.contains("theme_light"),
            "field names are kebab-case, not snake:\n{text}"
        );
    }

    #[test]
    fn a_partial_file_keeps_the_defaults_for_missing_fields() {
        let back: SettingsData = toml::from_str(r#"theme = "dark""#).unwrap();
        assert_eq!(back.theme, ThemePreference::Dark);
        assert_eq!(back.translate_to, SettingsData::default().translate_to);
        assert_eq!(back.skills_group_by, GroupBy::Origin);
        assert_eq!(back.theme_light, DEFAULT_LIGHT_THEME_ID);
        assert_eq!(back.theme_dark, DEFAULT_DARK_THEME_ID);
        assert!(back.show_welcome_on_startup);
        assert!(back.recent_targets.is_empty());
    }

    #[test]
    fn auto_reload_is_off_until_the_user_asks_for_it() {
        assert!(!SettingsData::default().watch_auto_reload);
        let back: SettingsData = toml::from_str(r#"theme = "dark""#).unwrap();
        assert!(!back.watch_auto_reload);

        let settings = SettingsData {
            watch_auto_reload: true,
            ..SettingsData::default()
        };
        let text = toml::to_string_pretty(&settings).unwrap();
        let back: SettingsData = toml::from_str(&text).unwrap();
        assert!(back.watch_auto_reload);
    }

    #[test]
    fn a_malformed_file_falls_back_rather_than_failing_to_start() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.toml");
        std::fs::write(&path, "this = is not [ valid toml").unwrap();

        assert_eq!(SettingsData::load_from(&path), SettingsData::default());
        assert!(path.exists(), "must not delete what it could not read");
    }

    #[test]
    fn a_missing_file_is_not_an_error() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("never-written.toml");
        assert_eq!(SettingsData::load_from(&path), SettingsData::default());
    }

    #[test]
    fn save_then_load_preserves_every_field() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nested").join("settings.toml");

        let settings = SettingsData {
            theme: ThemePreference::Light,
            theme_light: "sepia".into(),
            theme_dark: "nord".into(),
            language: Language::Chinese,
            split_sync_scroll: true,
            watch_auto_reload: true,
            model_provider: "anthropic".into(),
            model_base_url: "https://gw.invalid/openai".into(),
            model_name: "claude-sonnet-5".into(),
            model_environment_key_identity: "markturbo:model:test".into(),
            legacy_model_api_key: None,
            skills_include_internal: true,
            skills_include_global: false,
            skills_group_by: GroupBy::Status,
            translate_to: "de".into(),
            show_welcome_on_startup: false,
            recent_targets: vec![RecentTarget::new(
                PathBuf::from("C:/work/notes.md"),
                RecentTargetKind::File,
                "notes.md",
            )],
        };
        settings.save_to(&path);

        assert_eq!(SettingsData::load_from(&path), settings);
    }

    #[test]
    fn legacy_model_fields_load_without_becoming_the_current_serialized_schema() {
        let legacy = r#"
translate-provider = "openai-responses"
translate-model = "gpt-test"
translate-base-url = "https://gateway.invalid/v1"
translate-api-key = "legacy-secret-sentinel"
"#;

        let mut settings: SettingsData = toml::from_str(legacy).unwrap();
        assert_eq!(settings.model_provider, "openai-responses");
        assert_eq!(settings.model_name, "gpt-test");
        assert_eq!(settings.model_base_url, "https://gateway.invalid/v1");
        assert_eq!(
            settings.legacy_model_api_key(),
            Some("legacy-secret-sentinel")
        );

        let before_migration = toml::to_string_pretty(&settings).unwrap();
        assert!(before_migration.contains("legacy-secret-sentinel"));
        assert!(before_migration.contains("model-provider"));
        assert!(!before_migration.contains("translate-provider"));

        settings.clear_legacy_model_api_key();
        let after_migration = toml::to_string_pretty(&settings).unwrap();
        assert!(!after_migration.contains("legacy-secret-sentinel"));
        assert!(!after_migration.contains("translate-api-key"));
    }

    #[test]
    fn legacy_credentials_are_redacted_from_debug_output() {
        let settings: SettingsData =
            toml::from_str("translate-api-key = \"debug-secret-sentinel\"").unwrap();

        let debug = format!("{settings:?}");
        assert!(!debug.contains("debug-secret-sentinel"));
        assert!(debug.contains("<redacted>"));
    }

    #[test]
    fn unsafe_legacy_endpoint_is_preserved_until_the_user_explicitly_replaces_it() {
        for base_url in [
            "https://user:settings-secret@example.com/v1/",
            "https://example.com/v1/?api_key=settings-secret",
            "https://example.com/v1/#settings-secret",
        ] {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("settings.toml");
            let settings = SettingsData {
                model_base_url: base_url.into(),
                ..SettingsData::default()
            };
            let original = toml::to_string_pretty(&settings).unwrap();
            std::fs::write(&path, &original).unwrap();

            let loaded = SettingsData::load_from(&path);

            assert_eq!(loaded.model_base_url, base_url);
            assert_eq!(std::fs::read_to_string(&path).unwrap(), original);
        }
    }

    #[test]
    fn recent_targets_are_mru_deduplicated_and_capped() {
        let mut settings = SettingsData::default();
        for number in 0..=10 {
            settings.record_recent_target(RecentTarget::new(
                PathBuf::from(format!("workspace-{number}")),
                RecentTargetKind::Workspace,
                format!("Workspace {number}"),
            ));
        }

        assert_eq!(settings.recent_targets.len(), MAX_RECENT_TARGETS);
        assert_eq!(
            settings.recent_targets[0].path,
            PathBuf::from("workspace-10")
        );
        assert_eq!(
            settings.recent_targets.last().unwrap().path,
            PathBuf::from("workspace-1")
        );

        settings.record_recent_target(RecentTarget::new(
            PathBuf::from("workspace-5"),
            RecentTargetKind::File,
            "renamed.md",
        ));
        assert_eq!(settings.recent_targets.len(), MAX_RECENT_TARGETS);
        assert_eq!(
            settings.recent_targets[0].path,
            PathBuf::from("workspace-5")
        );
        assert_eq!(settings.recent_targets[0].kind, RecentTargetKind::File);
        assert_eq!(settings.recent_targets[0].display_name, "renamed.md");
        assert_eq!(
            settings
                .recent_targets
                .iter()
                .filter(|target| target.path.as_path() == Path::new("workspace-5"))
                .count(),
            1
        );
    }

    #[test]
    fn load_normalizes_hand_edited_recent_targets_in_mru_order() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.toml");
        std::fs::write(
            &path,
            r#"
[[recent-targets]]
path = "workspace-0"
display-name = "Workspace 0"

[[recent-targets]]
path = ""
display-name = "Empty"

[[recent-targets]]
path = "workspace-1"
display-name = "Workspace 1"

[[recent-targets]]
path = "workspace-2"
display-name = "Workspace 2"

[[recent-targets]]
path = "workspace-1"
display-name = "Duplicate"

[[recent-targets]]
path = "workspace-3"
display-name = "Workspace 3"

[[recent-targets]]
path = "workspace-4"
display-name = "Workspace 4"

[[recent-targets]]
path = "workspace-5"
display-name = "Workspace 5"

[[recent-targets]]
path = "workspace-6"
display-name = "Workspace 6"

[[recent-targets]]
path = "workspace-7"
display-name = "Workspace 7"

[[recent-targets]]
path = "workspace-8"
display-name = "Workspace 8"

[[recent-targets]]
path = "workspace-9"
display-name = "Workspace 9"

[[recent-targets]]
path = "workspace-10"
display-name = "Workspace 10"

[[recent-targets]]
path = "workspace-11"
display-name = "Workspace 11"
"#,
        )
        .unwrap();

        let settings = SettingsData::load_from(&path);
        assert_eq!(settings.recent_targets.len(), MAX_RECENT_TARGETS);
        let paths = settings
            .recent_targets
            .iter()
            .map(|target| target.path.clone())
            .collect::<Vec<_>>();
        assert_eq!(
            paths,
            (0..MAX_RECENT_TARGETS)
                .map(|number| PathBuf::from(format!("workspace-{number}")))
                .collect::<Vec<_>>(),
        );
        assert_eq!(settings.recent_targets[1].display_name, "Workspace 1");
    }

    #[test]
    fn recent_targets_can_be_removed_and_partial_entries_still_load() {
        let mut settings = SettingsData::default();
        let path = PathBuf::from("workspace");
        settings.record_recent_target(RecentTarget::new(
            path.clone(),
            RecentTargetKind::Workspace,
            "workspace",
        ));

        assert!(settings.remove_recent_target(&path));
        assert!(settings.recent_targets.is_empty());
        assert!(!settings.remove_recent_target(&path));

        let back: SettingsData = toml::from_str(
            r#"
[[recent-targets]]
path = "old-file.md"
"#,
        )
        .unwrap();
        assert_eq!(back.recent_targets.len(), 1);
        assert_eq!(back.recent_targets[0].path, PathBuf::from("old-file.md"));
        assert_eq!(back.recent_targets[0].kind, RecentTargetKind::File);
        assert!(back.recent_targets[0].display_name.is_empty());
    }

    #[test]
    fn the_settings_path_is_under_a_markturbo_directory() {
        let path = settings_path().expect("a platform default is always available");
        assert_eq!(
            path.file_name().and_then(|name| name.to_str()),
            Some("settings.toml")
        );
        assert!(
            path.parent()
                .is_some_and(|parent| parent.ends_with("markturbo")),
            "got {}",
            path.display()
        );
    }

    #[test]
    fn a_blank_env_path_is_treated_as_unset() {
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
