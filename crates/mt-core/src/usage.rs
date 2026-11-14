//! Optional local, content-free daily usage counters.
//!
//! Recording accepts only a closed vocabulary of categories and counts, never
//! free-form strings, paths, credentials, source text or model output. UTC days
//! are integer days since the Unix epoch, not precise event timestamps. The
//! only outbound boundary is an explicit caller-requested export to a file.

use std::{
    collections::BTreeMap,
    fmt,
    io::{self, Write},
    path::{Path, PathBuf},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use serde::{Deserialize, Serialize};

use crate::review::{ArtifactLens, ReviewRequest, ReviewScope};

pub const USAGE_SCHEMA_VERSION: u32 = 1;
pub const RETENTION_DAYS: u64 = 180;
const SECONDS_PER_DAY: u64 = 86_400;
const STAGING_PREFIX: &str = ".markturbo-usage-";
const STAGING_SUFFIX: &str = ".tmp";

/// UTC day index. Times before the Unix epoch are placed in day zero.
pub fn day_bucket(now: SystemTime) -> u64 {
    now.duration_since(UNIX_EPOCH).unwrap_or_default().as_secs() / SECONDS_PER_DAY
}

/// Whole-artifact Review (including a Skill package) or selection Review.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UsageReviewScope {
    Document,
    Selection,
}

/// Request-to-result elapsed time, reduced before entering the store.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReviewDurationBucket {
    UnderOneMinute,
    OneToFiveMinutes,
    FiveToFifteenMinutes,
    FifteenMinutesOrMore,
}

impl ReviewDurationBucket {
    pub fn from_duration(duration: Duration) -> Self {
        match duration.as_secs() {
            0..60 => Self::UnderOneMinute,
            60..300 => Self::OneToFiveMinutes,
            300..900 => Self::FiveToFifteenMinutes,
            _ => Self::FifteenMinutesOrMore,
        }
    }
}

/// The complete recording boundary. No variant can carry user content.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UsageEvent {
    FirstLaunch,
    ArtifactOpened,
    ArtifactPasted,
    ReviewStarted {
        scope: UsageReviewScope,
        lens: ArtifactLens,
    },
    ReviewCompleted(ReviewDurationBucket),
    ClarificationAnswered,
    ClarificationDeferred,
    ClarificationRejected,
    RevisionDecision {
        accepted_hunks: u64,
        rejected_hunks: u64,
    },
    RevisionApplied,
    ArtifactSaved,
    ArtifactCopied,
    ProviderFailure,
    CredentialFailure,
    StaleResultBlocked,
    RecoveryRestored,
}

impl UsageEvent {
    /// Reduce an existing classification and request scope locally. Source,
    /// path, selection bounds and Effective Agent Context are not retained.
    pub fn review_started(request: &ReviewRequest) -> Self {
        Self::ReviewStarted {
            scope: match request.scope {
                ReviewScope::Selection { .. } => UsageReviewScope::Selection,
                ReviewScope::Document | ReviewScope::AgentSkillPackage => {
                    UsageReviewScope::Document
                }
            },
            lens: request.lens,
        }
    }

    fn kind(self) -> UsageEventKind {
        match self {
            Self::FirstLaunch => UsageEventKind::FirstLaunch,
            Self::ArtifactOpened => UsageEventKind::ArtifactOpened,
            Self::ArtifactPasted => UsageEventKind::ArtifactPasted,
            Self::ReviewStarted { .. } => UsageEventKind::ReviewStarted,
            Self::ReviewCompleted(_) => UsageEventKind::ReviewCompleted,
            Self::ClarificationAnswered => UsageEventKind::ClarificationAnswered,
            Self::ClarificationDeferred => UsageEventKind::ClarificationDeferred,
            Self::ClarificationRejected => UsageEventKind::ClarificationRejected,
            Self::RevisionDecision { .. } => UsageEventKind::RevisionDecision,
            Self::RevisionApplied => UsageEventKind::RevisionApplied,
            Self::ArtifactSaved => UsageEventKind::ArtifactSaved,
            Self::ArtifactCopied => UsageEventKind::ArtifactCopied,
            Self::ProviderFailure => UsageEventKind::ProviderFailure,
            Self::CredentialFailure => UsageEventKind::CredentialFailure,
            Self::StaleResultBlocked => UsageEventKind::StaleResultBlocked,
            Self::RecoveryRestored => UsageEventKind::RecoveryRestored,
        }
    }
}

/// Closed JSON keys, not caller-provided event names.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UsageEventKind {
    FirstLaunch,
    ArtifactOpened,
    ArtifactPasted,
    ReviewStarted,
    ReviewCompleted,
    ClarificationAnswered,
    ClarificationDeferred,
    ClarificationRejected,
    RevisionDecision,
    RevisionApplied,
    ArtifactSaved,
    ArtifactCopied,
    ProviderFailure,
    CredentialFailure,
    StaleResultBlocked,
    RecoveryRestored,
}

/// One day's aggregate; there are no individual event records.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UsageDayCounts {
    pub events: BTreeMap<UsageEventKind, u64>,
    pub review_started: BTreeMap<UsageReviewScope, BTreeMap<ArtifactLens, u64>>,
    pub review_durations: BTreeMap<ReviewDurationBucket, u64>,
    pub accepted_hunks: u64,
    pub rejected_hunks: u64,
}

impl UsageDayCounts {
    fn record(&mut self, event: UsageEvent) {
        increment(self.events.entry(event.kind()).or_default(), 1);
        match event {
            UsageEvent::ReviewStarted { scope, lens } => increment(
                self.review_started
                    .entry(scope)
                    .or_default()
                    .entry(lens)
                    .or_default(),
                1,
            ),
            UsageEvent::ReviewCompleted(duration) => {
                increment(self.review_durations.entry(duration).or_default(), 1);
            }
            UsageEvent::RevisionDecision {
                accepted_hunks,
                rejected_hunks,
            } => {
                increment(&mut self.accepted_hunks, accepted_hunks);
                increment(&mut self.rejected_hunks, rejected_hunks);
            }
            _ => {}
        }
    }
}

fn increment(count: &mut u64, amount: u64) {
    *count = count.saturating_add(amount);
}

/// The sole on-disk JSON document. Day keys are UTC Unix day indices.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UsageAggregate {
    schema_version: u32,
    days: BTreeMap<u64, UsageDayCounts>,
}

impl Default for UsageAggregate {
    fn default() -> Self {
        Self {
            schema_version: USAGE_SCHEMA_VERSION,
            days: BTreeMap::new(),
        }
    }
}

impl UsageAggregate {
    pub fn days(&self) -> &BTreeMap<u64, UsageDayCounts> {
        &self.days
    }

    fn prune(&mut self, today: u64) -> bool {
        let previous = self.days.len();
        let oldest = today.saturating_sub(RETENTION_DAYS);
        // Future day keys are not plausible local observations and would
        // otherwise defeat the bound after a clock correction or edited file.
        self.days.retain(|day, _| (oldest..=today).contains(day));
        self.days.len() != previous
    }

    fn export_json(&self) -> Result<Vec<u8>, UsageError> {
        #[derive(Serialize)]
        struct Export<'a> {
            schema_version: u32,
            app_version: &'static str,
            days: &'a BTreeMap<u64, UsageDayCounts>,
        }
        serde_json::to_vec_pretty(&Export {
            schema_version: USAGE_SCHEMA_VERSION,
            app_version: env!("CARGO_PKG_VERSION"),
            days: &self.days,
        })
        .map_err(|_| UsageError::InvalidStore)
    }
}

/// Content-free errors: formatting never exposes a path or decoded input.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UsageError {
    DataDirectoryUnavailable,
    InvalidStore,
    UnsupportedSchema,
    Io(io::ErrorKind),
}

impl fmt::Display for UsageError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::DataDirectoryUnavailable => {
                formatter.write_str("usage data directory unavailable")
            }
            Self::InvalidStore => formatter.write_str("usage data is invalid"),
            Self::UnsupportedSchema => formatter.write_str("usage data schema is unsupported"),
            Self::Io(_) => formatter.write_str("usage data could not be read or written"),
        }
    }
}

impl std::error::Error for UsageError {}

impl From<io::Error> for UsageError {
    fn from(error: io::Error) -> Self {
        Self::Io(error.kind())
    }
}

/// A small store handle, without an in-memory copy that could resurrect cleared
/// counts. Construction and toggling perform no I/O. Disabled recording exits
/// before acquiring a lock, resolving a path or creating a directory.
pub struct UsageStore {
    path: Option<PathBuf>,
    consent_path: Option<PathBuf>,
    enabled: bool,
}

impl UsageStore {
    pub fn open(enabled: bool) -> Self {
        Self {
            path: crate::runtime_paths::usage_path(),
            consent_path: crate::settings::settings_path(),
            enabled,
        }
    }

    #[cfg(any(test, feature = "test-support"))]
    pub fn open_in(data_root: &Path, consent_path: &Path, enabled: bool) -> Self {
        Self {
            path: Some(data_root.join("usage.json")),
            consent_path: Some(consent_path.to_path_buf()),
            enabled,
        }
    }

    pub fn enabled(&self) -> bool {
        self.enabled
    }

    /// The app persists explicit consent before enabling this handle.
    pub fn set_enabled(&mut self, enabled: bool) {
        self.enabled = enabled;
    }

    pub fn record(&self, event: UsageEvent, now: SystemTime) -> Result<(), UsageError> {
        if !self.enabled {
            return Ok(());
        }
        let _guard = crate::settings::lock_settings_writes()?;
        if !self.persisted_consent() {
            return Ok(());
        }
        let path = self.path()?;
        let mut aggregate = Self::load(path)?;
        let today = day_bucket(now);
        aggregate.prune(today);
        aggregate.days.entry(today).or_default().record(event);
        Self::save(path, &aggregate)
    }

    /// Inspect retained counts even when recording is disabled. A disabled
    /// handle never rewrites the store, including for retention maintenance.
    pub fn inspect(&self, now: SystemTime) -> Result<UsageAggregate, UsageError> {
        let _guard = crate::settings::lock_settings_writes()?;
        let path = self.path()?;
        let mut aggregate = Self::load(path)?;
        if aggregate.prune(day_bucket(now)) && self.enabled && self.persisted_consent() {
            Self::save(path, &aggregate)?;
        }
        Ok(aggregate)
    }

    /// Serialize only the approved aggregate fields and build-time app version.
    /// This does not send anything or create an export file.
    pub fn export_json(&self, now: SystemTime) -> Result<Vec<u8>, UsageError> {
        self.inspect(now)?.export_json()
    }

    /// Deliberate user export. As with Save As, an existing destination is not
    /// overwritten. No destination path is retained in the aggregate or errors.
    pub fn export_to(&self, destination: &Path, now: SystemTime) -> Result<(), UsageError> {
        let bytes = self.export_json(now)?;
        let parent = destination
            .parent()
            .filter(|path| !path.as_os_str().is_empty())
            .unwrap_or_else(|| Path::new("."));
        let mut temporary = tempfile::NamedTempFile::new_in(parent)?;
        temporary.write_all(&bytes)?;
        temporary.as_file().sync_all()?;
        temporary
            .persist_noclobber(destination)
            .map_err(|error| UsageError::from(error.error))?;
        Ok(())
    }

    /// Explicit Clear deletes the aggregate, including while recording is off.
    /// It does not change consent or delete copies previously exported by users.
    pub fn clear(&self) -> Result<(), UsageError> {
        let _guard = crate::settings::lock_settings_writes()?;
        let path = self.path()?;
        match std::fs::remove_file(path) {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
        Self::clear_staging(path.parent().ok_or(UsageError::DataDirectoryUnavailable)?)
    }

    fn persisted_consent(&self) -> bool {
        self.consent_path
            .as_deref()
            .and_then(|path| std::fs::read_to_string(path).ok())
            .and_then(|text| toml::from_str::<crate::settings::SettingsData>(&text).ok())
            .is_some_and(|settings| settings.usage_metrics_enabled)
    }

    fn clear_staging(parent: &Path) -> Result<(), UsageError> {
        let entries = match std::fs::read_dir(parent) {
            Ok(entries) => entries,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
            Err(error) => return Err(error.into()),
        };
        for entry in entries {
            let entry = entry?;
            let name = entry.file_name();
            let owned = name
                .to_str()
                .and_then(|name| name.strip_prefix(STAGING_PREFIX))
                .and_then(|name| name.strip_suffix(STAGING_SUFFIX))
                .is_some_and(|name| {
                    !name.is_empty() && name.bytes().all(|byte| byte.is_ascii_alphanumeric())
                });
            if owned && entry.file_type()?.is_file() {
                std::fs::remove_file(entry.path())?;
            }
        }
        Ok(())
    }

    fn path(&self) -> Result<&Path, UsageError> {
        self.path
            .as_deref()
            .ok_or(UsageError::DataDirectoryUnavailable)
    }

    fn load(path: &Path) -> Result<UsageAggregate, UsageError> {
        let bytes = match std::fs::read(path) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                return Ok(UsageAggregate::default());
            }
            Err(error) => return Err(error.into()),
        };
        let aggregate: UsageAggregate =
            serde_json::from_slice(&bytes).map_err(|_| UsageError::InvalidStore)?;
        if aggregate.schema_version != USAGE_SCHEMA_VERSION {
            return Err(UsageError::UnsupportedSchema);
        }
        Ok(aggregate)
    }

    fn save(path: &Path, aggregate: &UsageAggregate) -> Result<(), UsageError> {
        let parent = path.parent().ok_or(UsageError::DataDirectoryUnavailable)?;
        std::fs::create_dir_all(parent)?;
        Self::clear_staging(parent)?;
        let mut temporary = tempfile::Builder::new()
            .prefix(STAGING_PREFIX)
            .suffix(STAGING_SUFFIX)
            .tempfile_in(parent)?;
        serde_json::to_writer_pretty(&mut temporary, aggregate)
            .map_err(|_| UsageError::InvalidStore)?;
        temporary.as_file().sync_all()?;
        temporary
            .persist(path)
            .map_err(|error| UsageError::from(error.error))?;
        Ok(())
    }
}

#[cfg(test)]
mod tests;

#[cfg(test)]
pub(crate) use tests::USAGE_TEST_LOCK;
