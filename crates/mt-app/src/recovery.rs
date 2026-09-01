//! Local, encrypted recovery checkpoints for dirty editor buffers.
//!
//! This module never writes a workspace file. Its only durable output is an
//! encrypted record under the per-user application-data directory, so a failed
//! checkpoint cannot damage the source document it is protecting.

use std::{
    collections::{HashMap, HashSet},
    fmt, fs,
    io::{self, Write},
    path::{Path, PathBuf},
    process,
    sync::{
        Arc, Mutex, MutexGuard,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

#[cfg(test)]
use std::sync::atomic::AtomicUsize;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::fs::{FileObjectId, FileStamp, LoadedFile, Newline, SourceIdentity};
#[cfg(windows)]
use crate::{app_paths, fs::file_object_id};

/// The maximum loss window acknowledged by the recovery contract.
pub const MAX_LOSS_WINDOW: Duration = Duration::from_secs(10);
/// The maximum time a dispatched recovery checkpoint may take to become durable.
pub const CHECKPOINT_COMMIT_BUDGET: Duration = Duration::from_secs(8);
const IDLE_CHECKPOINT_DELAY: Duration = Duration::from_secs(2);
const CHECKPOINT_RETRY_DELAY: Duration = Duration::from_secs(1);
const RECORD_EXTENSION: &str = "mtrecovery";
const RECORD_VERSION: u8 = 1;
const TRANSACTION_VERSION: u8 = 1;
const TRANSACTION_JOURNAL_NAME: &str = ".markturbo-recovery-transaction.json";
const TRANSACTION_COMMIT_NAME: &str = ".markturbo-recovery-transaction.commit";
const ARTIFACT_PREFIX: &str = ".markturbo-recovery-";
const RETIREMENT_MARKER_PREFIX: &str = ".markturbo-recovery-retiring-";
const RETIREMENT_MARKER_VERSION: u8 = 1;
const MAX_PARALLEL_RECOVERY_WORKERS: usize = 4;
const CHECKPOINT_WAVE_METADATA_OVERHEAD_BYTES: u64 = 4 * 1024;
static NEXT_MEMORY_RECOVERY_KEY: AtomicU64 = AtomicU64::new(0);
#[cfg(windows)]
const LOCAL_RECOVERY_ROOT_REQUIRED: &str = "recovery root must be on a local volume";
#[cfg(windows)]
const VOLUME_PATH_BUFFER_LEN: usize = 32_768;

/// Production retention limits for encrypted recovery records.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RecoveryLimits {
    pub max_records: usize,
    pub max_record_bytes: u64,
    pub max_total_bytes: u64,
    pub max_age: Duration,
}

impl Default for RecoveryLimits {
    fn default() -> Self {
        Self {
            max_records: 50,
            max_record_bytes: 32 * 1024 * 1024,
            max_total_bytes: 128 * 1024 * 1024,
            max_age: Duration::from_secs(7 * 24 * 60 * 60),
        }
    }
}

/// Stable opaque identifier for one recovery record.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct RecoveryKey(String);

impl RecoveryKey {
    /// Derive an opaque key from the path without exposing it in the filename.
    pub fn for_path(path: &Path) -> Self {
        let mut hasher = Sha256::new();
        #[cfg(windows)]
        {
            use std::os::windows::ffi::OsStrExt as _;

            hasher.update(b"markturbo-recovery-path-v2\0windows-osstr-wide-le\0");
            for unit in path.as_os_str().encode_wide() {
                hasher.update(unit.to_le_bytes());
            }
        }
        #[cfg(not(windows))]
        {
            hasher.update(b"markturbo-recovery-path-v2\0native-osstr-encoded\0");
            hasher.update(path.as_os_str().as_encoded_bytes());
        }
        Self(format!("{:x}", hasher.finalize()))
    }

    /// Use a caller-owned stable ID for a dirty buffer with no source path.
    pub fn for_document_id(document_id: &str) -> Self {
        let mut hasher = Sha256::new();
        hasher.update(b"markturbo-recovery-document-v1\0");
        hasher.update(document_id.as_bytes());
        Self(format!("{:x}", hasher.finalize()))
    }

    /// Generate a process-crossing opaque key for a buffer with no source
    /// path. The sequence makes calls unique inside one process; process ID and
    /// wall-clock nanoseconds distinguish separately started application
    /// processes without introducing a random-number dependency.
    pub fn new_memory() -> Self {
        let sequence = NEXT_MEMORY_RECOVERY_KEY.fetch_add(1, Ordering::Relaxed);
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let mut hasher = Sha256::new();
        hasher.update(b"markturbo-recovery-memory-v1\0");
        hasher.update(process::id().to_le_bytes());
        hasher.update(timestamp.to_le_bytes());
        hasher.update(sequence.to_le_bytes());
        Self(format!("{:x}", hasher.finalize()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    fn is_valid(&self) -> bool {
        self.0.len() == 64
            && self
                .0
                .bytes()
                .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
    }
}

/// A generation-scoped capability for committing a recovery checkpoint.
///
/// Callers capture this when scheduling background work. Intentional Save or
/// Discard invalidates the key before removing its record, so a worker holding
/// an older token cannot recreate recovery data after that decision.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecoveryToken {
    key: RecoveryKey,
    generation: u64,
}

/// A one-shot capability to finish retiring a recovery record.
///
/// `begin_retirement` invalidates checkpoint tokens synchronously, then the
/// caller may complete the filesystem work without holding the UI thread.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecoveryRetirement {
    batch: RecoveryRetirementBatch,
}

/// A one-shot capability to finish an atomic group of recovery retirements.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecoveryRetirementBatch {
    keys: Vec<RecoveryKey>,
    id: u64,
}

/// The durable result of finishing a recovery retirement.
#[derive(Debug)]
pub enum RetirementCompletion {
    /// The canonical record was absent or has been removed.
    Retired { removed: bool },
    /// The canonical record was already renamed out of recovery visibility,
    /// but deleting that retired artifact needs later maintenance.
    CleanupPending { error: RecoveryError },
}

/// The file metadata required to make a recovered buffer safe to save.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecoveryMetadata {
    pub source_path: Option<PathBuf>,
    pub encoding_name: String,
    pub had_bom: bool,
    pub newline: Newline,
    pub original_stamp: FileStamp,
    pub source_identity: SourceIdentity,
    pub decode_had_errors: bool,
}

impl RecoveryMetadata {
    pub fn from_loaded_file(file: &LoadedFile) -> Self {
        Self {
            source_path: Some(file.path.clone()),
            encoding_name: file.encoding.name().to_owned(),
            had_bom: file.had_bom,
            newline: file.newline,
            original_stamp: file.stamp.clone(),
            source_identity: file.source_identity.clone(),
            decode_had_errors: file.decode_had_errors,
        }
    }
}

/// Exact text and metadata to persist for one dirty buffer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecoveryCheckpoint {
    pub key: RecoveryKey,
    pub text: String,
    pub metadata: RecoveryMetadata,
}

/// A decoded recovery record.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecoveryRecord {
    pub key: RecoveryKey,
    pub text: String,
    pub metadata: RecoveryMetadata,
    pub checkpointed_at: SystemTime,
}

/// A record that can be offered for recovery, with its source safety state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecoveredRecord {
    pub record: RecoveryRecord,
    /// True when the file changed, disappeared, or cannot be read since the
    /// checkpoint. The caller must surface this as a conflict before Save.
    pub source_conflicted: bool,
}

/// A recoverable record was skipped. These values contain no document text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RecoveryIssue {
    Malformed { path: PathBuf },
    Oversized { path: PathBuf, bytes: u64 },
    Expired { path: PathBuf },
    Unreadable { path: PathBuf },
    CleanupPending { path: PathBuf },
}

/// Results of a non-mutating recovery scan.
#[derive(Debug, Default)]
pub struct RecoveryScan {
    pub records: Vec<RecoveredRecord>,
    pub issues: Vec<RecoveryIssue>,
}

/// Result of a successful checkpoint write.
#[derive(Debug, Default)]
pub struct CheckpointReceipt {
    pub maintenance: RecoveryMaintenance,
}

/// Result of a token-guarded checkpoint attempt.
#[derive(Debug)]
pub enum CheckpointOutcome {
    Written(CheckpointReceipt),
    /// A newer lifecycle decision invalidated this worker's captured token.
    Superseded,
}

/// One scheduled checkpoint in an ordered recovery batch.
///
/// The caller retains document identity and scheduling metadata alongside this
/// borrowed input, then maps the ordered outcomes back to its own work items.
pub(crate) struct RecoveryCheckpointAttempt<'a> {
    pub checkpoint: &'a RecoveryCheckpoint,
    pub token: &'a RecoveryToken,
}

/// One scheduled checkpoint with cancellation scoped to this attempt.
///
/// A cancelled attempt never suppresses other current attempts in its batch.
pub(crate) struct CancellableRecoveryCheckpointAttempt<'a> {
    pub checkpoint: &'a RecoveryCheckpoint,
    pub token: &'a RecoveryToken,
    pub cancelled: &'a AtomicBool,
}

struct CurrentRecoveryCheckpointAttempt<'a> {
    index: usize,
    checkpoint: &'a RecoveryCheckpoint,
    token: &'a RecoveryToken,
    cancelled: &'a AtomicBool,
}

enum CheckpointCapability {
    Current { active_keys: HashSet<RecoveryKey> },
    Superseded,
    Deferred,
}

/// Per-attempt outcome from [`RecoveryStore::checkpoint_batch_if_current`].
#[derive(Debug)]
pub(crate) enum CheckpointBatchOutcome {
    Written,
    Superseded,
    /// This attempt failed before a later batch item could safely reuse the
    /// retention scan. The scheduler should surface and retry it.
    Failed(RecoveryError),
    /// A preceding write or transaction failure made the cached scan unsafe to
    /// reuse. The scheduler should retry this attempt in a later batch.
    Deferred,
}

/// Ordered checkpoint outcomes plus maintenance performed once for the batch.
#[derive(Debug, Default)]
pub(crate) struct CheckpointBatchReceipt {
    pub outcomes: Vec<CheckpointBatchOutcome>,
    pub maintenance: RecoveryMaintenance,
}

/// Result of startup or post-checkpoint maintenance.
#[derive(Debug, Default)]
pub struct RecoveryMaintenance {
    pub removed_expired: usize,
    pub issues: Vec<RecoveryIssue>,
}

/// Recovery errors do not contain source text or encrypted payloads.
#[derive(Debug)]
pub enum RecoveryError {
    Unavailable(&'static str),
    Protection,
    Unprotection,
    OversizedCheckpoint {
        bytes: u64,
        limit: u64,
    },
    QuotaExceeded {
        required_records: usize,
        max_records: usize,
        required_bytes: u64,
        max_total_bytes: u64,
    },
    InvalidTimestamp,
    Io(io::Error),
    Serialization(serde_json::Error),
}

impl fmt::Display for RecoveryError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unavailable(reason) => write!(f, "recovery is unavailable: {reason}"),
            Self::Protection => f.write_str("recovery encryption failed"),
            Self::Unprotection => f.write_str("recovery decryption failed"),
            Self::OversizedCheckpoint { bytes, limit } => {
                write!(
                    f,
                    "recovery checkpoint is {bytes} bytes; limit is {limit} bytes"
                )
            }
            Self::QuotaExceeded {
                required_records,
                max_records,
                required_bytes,
                max_total_bytes,
            } => {
                write!(
                    f,
                    "recovery retention quota would require {required_records} records / {required_bytes} bytes; limits are {max_records} records / {max_total_bytes} bytes"
                )
            }
            Self::InvalidTimestamp => f.write_str("recovery record has an invalid timestamp"),
            Self::Io(error) => write!(f, "recovery storage failed: {error}"),
            Self::Serialization(error) => {
                write!(f, "recovery record serialization failed: {error}")
            }
        }
    }
}

impl std::error::Error for RecoveryError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(error) => Some(error),
            Self::Serialization(error) => Some(error),
            _ => None,
        }
    }
}

impl From<io::Error> for RecoveryError {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}

impl From<serde_json::Error> for RecoveryError {
    fn from(error: serde_json::Error) -> Self {
        Self::Serialization(error)
    }
}

/// Platform encryption boundary. Implementations must never return plaintext.
pub trait RecoveryProtector: Send + Sync {
    fn protect(&self, plaintext: &[u8]) -> Result<Vec<u8>, RecoveryError>;
    fn unprotect(&self, ciphertext: &[u8]) -> Result<Vec<u8>, RecoveryError>;
}

/// Current-user Windows DPAPI protection for production recovery data.
#[cfg(windows)]
#[derive(Debug, Default)]
pub struct DpapiProtector;

#[cfg(windows)]
impl RecoveryProtector for DpapiProtector {
    fn protect(&self, plaintext: &[u8]) -> Result<Vec<u8>, RecoveryError> {
        dpapi(plaintext, true)
    }

    fn unprotect(&self, ciphertext: &[u8]) -> Result<Vec<u8>, RecoveryError> {
        dpapi(ciphertext, false)
    }
}

#[cfg(windows)]
fn dpapi(input: &[u8], protect: bool) -> Result<Vec<u8>, RecoveryError> {
    use windows::Win32::{
        Foundation::{HLOCAL, LocalFree},
        Security::Cryptography::{
            CRYPT_INTEGER_BLOB, CRYPTPROTECT_UI_FORBIDDEN, CryptProtectData, CryptUnprotectData,
        },
    };

    let length = u32::try_from(input.len()).map_err(|_| RecoveryError::Protection)?;
    let input = CRYPT_INTEGER_BLOB {
        cbData: length,
        pbData: input.as_ptr().cast_mut(),
    };
    let mut output = CRYPT_INTEGER_BLOB::default();
    let result = unsafe {
        if protect {
            CryptProtectData(
                &input,
                None,
                None,
                None,
                None,
                CRYPTPROTECT_UI_FORBIDDEN,
                &mut output,
            )
        } else {
            CryptUnprotectData(
                &input,
                None,
                None,
                None,
                None,
                CRYPTPROTECT_UI_FORBIDDEN,
                &mut output,
            )
        }
    };
    result.map_err(|_| {
        if protect {
            RecoveryError::Protection
        } else {
            RecoveryError::Unprotection
        }
    })?;

    // DPAPI allocates this buffer with LocalAlloc. Copy before LocalFree so
    // the protected bytes never outlive the operating-system allocation.
    let output_bytes = unsafe {
        let bytes = std::slice::from_raw_parts(output.pbData, output.cbData as usize).to_vec();
        LocalFree(Some(HLOCAL(output.pbData.cast())));
        bytes
    };
    Ok(output_bytes)
}

/// Thread-safe recovery storage. It serializes recovery-root mutations so
/// concurrent checkpoints cannot race retention, while lifecycle capabilities
/// remain available to the UI without waiting for filesystem I/O.
#[derive(Clone)]
pub struct RecoveryStore {
    root: PathBuf,
    protector: Arc<dyn RecoveryProtector>,
    limits: RecoveryLimits,
    capability_lock: Arc<Mutex<CapabilityState>>,
    mutation_lock: Arc<Mutex<MutationState>>,
    retirement_marker_lock: Arc<Mutex<()>>,
    #[cfg(windows)]
    root_guard: Option<Arc<ProductionRootGuard>>,
    #[cfg(test)]
    failures: Arc<TestFailurePoints>,
}

/// Process-lifetime ownership of the production recovery root.
///
/// The directory handle is opened without following the final reparse point
/// and denies rename/delete sharing. The independent lease file rejects a
/// second process even when it opens the same directory through another path.
#[cfg(windows)]
struct ProductionRootGuard {
    canonical_root: PathBuf,
    object_id: FileObjectId,
    _directory: fs::File,
    _lease: fs::File,
}

#[cfg(test)]
#[derive(Default)]
struct TestFailurePoints {
    persist: AtomicBool,
    retirement_marker_sync: AtomicBool,
    rename: AtomicBool,
    delete: AtomicBool,
    presence: Mutex<Option<PathBuf>>,
    retention_scans: AtomicUsize,
    checkpoint_batches: AtomicUsize,
    after_checkpoint_prepare: Mutex<Option<TestPause>>,
    after_checkpoint_final_check: Mutex<Option<TestPause>>,
    after_retirement_marker: Mutex<Option<TestPause>>,
    before_transaction_journal: Mutex<Option<TestPause>>,
    after_transaction_journal: Mutex<Option<TestPause>>,
    after_eviction_reservation: Mutex<Option<TestPause>>,
}

#[cfg(test)]
struct TestPause {
    started: std::sync::mpsc::Sender<()>,
    release: Mutex<std::sync::mpsc::Receiver<()>>,
}

#[cfg(all(test, windows))]
thread_local! {
    static ROOT_ACQUISITION_HOOK: std::cell::RefCell<Option<Box<dyn FnOnce()>>> = const {
        std::cell::RefCell::new(None)
    };
}

#[cfg(all(test, windows))]
fn set_root_acquisition_hook(hook: impl FnOnce() + 'static) {
    ROOT_ACQUISITION_HOOK.with(|slot| {
        assert!(
            slot.borrow().is_none(),
            "a recovery root acquisition hook is already installed"
        );
        *slot.borrow_mut() = Some(Box::new(hook));
    });
}

#[cfg(all(test, windows))]
fn run_root_acquisition_hook() {
    ROOT_ACQUISITION_HOOK.with(|slot| {
        if let Some(hook) = slot.borrow_mut().take() {
            hook();
        }
    });
}

#[derive(Default)]
struct MutationState {
    next_artifact: u64,
}

/// Fast in-memory state used to linearize editor lifecycle decisions.
///
/// Never hold this lock across recovery-root I/O. The I/O mutex serializes
/// transactions, scans, writes, and deletes separately.
#[derive(Default)]
struct CapabilityState {
    generations: HashMap<RecoveryKey, u64>,
    active_keys: HashSet<RecoveryKey>,
    evicting_keys: HashSet<RecoveryKey>,
    retiring: HashMap<RecoveryKey, RetirementState>,
    next_retirement: u64,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum RetirementState {
    Persisting(u64),
    Pending(u64),
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct RetirementMarker {
    version: u8,
    keys: Vec<String>,
}

struct ReconciledRetirements {
    issues: Vec<RecoveryIssue>,
    marked_keys: HashSet<RecoveryKey>,
}

#[derive(Default)]
struct PendingCleanup {
    issues: Vec<RecoveryIssue>,
    quota_artifacts: Vec<PendingCleanupArtifact>,
    blocks_mutation: bool,
}

struct PendingCleanupArtifact {
    path: PathBuf,
    bytes: Option<u64>,
}

impl PendingCleanup {
    fn extend(&mut self, other: Self) {
        self.issues.extend(other.issues);
        self.quota_artifacts.extend(other.quota_artifacts);
        self.blocks_mutation |= other.blocks_mutation;
    }

    fn record(&mut self, path: &Path, reserves_quota: bool) {
        self.issues.push(RecoveryIssue::CleanupPending {
            path: path.to_path_buf(),
        });
        if reserves_quota {
            self.reserve_quota(path);
        }
    }

    fn reserve_quota(&mut self, path: &Path) {
        let bytes = match fs::symlink_metadata(path) {
            Ok(metadata) if metadata.is_file() => Some(metadata.len()),
            Ok(_) => None,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return,
            Err(_) => None,
        };
        self.quota_artifacts.push(PendingCleanupArtifact {
            path: path.to_path_buf(),
            bytes,
        });
    }
}

#[cfg(windows)]
impl ProductionRootGuard {
    fn acquire(root: &Path) -> Result<Self, RecoveryError> {
        use std::os::windows::fs::MetadataExt as _;
        use windows::Win32::Storage::FileSystem::FILE_ATTRIBUTE_REPARSE_POINT;

        let directory = open_root_guard_handle(root)?;
        #[cfg(all(test, windows))]
        run_root_acquisition_hook();
        let metadata = directory.metadata()?;
        if metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT.0 != 0 {
            return Err(RecoveryError::Unavailable(
                "recovery root must not be a reparse point",
            ));
        }
        let object_id = file_object_id(&directory)?.ok_or(RecoveryError::Unavailable(
            "recovery root does not provide a stable object identity",
        ))?;
        let canonical_root = fs::canonicalize(root)?;
        ensure_local_canonical_production_root(&canonical_root)?;
        let canonical_directory = open_root_guard_handle(&canonical_root)?;
        let canonical_metadata = canonical_directory.metadata()?;
        if canonical_metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT.0 != 0 {
            return Err(RecoveryError::Unavailable(
                "recovery root must not be a reparse point",
            ));
        }
        let canonical_object_id = file_object_id(&canonical_directory)?.ok_or(
            RecoveryError::Unavailable("recovery root does not provide a stable object identity"),
        )?;
        if canonical_object_id != object_id {
            return Err(RecoveryError::Unavailable(
                "recovery root changed while recovery was opening",
            ));
        }
        let lease = open_recovery_lease(&canonical_root)?;
        Ok(Self {
            canonical_root,
            object_id,
            _directory: directory,
            _lease: lease,
        })
    }

    fn verify(&self) -> Result<(), RecoveryError> {
        use std::os::windows::fs::MetadataExt as _;
        use windows::Win32::Storage::FileSystem::FILE_ATTRIBUTE_REPARSE_POINT;

        let current = open_root_guard_handle(&self.canonical_root)?;
        let metadata = current.metadata()?;
        if metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT.0 != 0 {
            return Err(RecoveryError::Unavailable(
                "recovery root changed while recovery was active",
            ));
        }
        let object_id = file_object_id(&current)?.ok_or(RecoveryError::Unavailable(
            "recovery root does not provide a stable object identity",
        ))?;
        if object_id != self.object_id {
            return Err(RecoveryError::Unavailable(
                "recovery root changed while recovery was active",
            ));
        }
        Ok(())
    }
}

#[cfg(windows)]
fn ensure_local_production_root(root: &Path) -> Result<(), RecoveryError> {
    ensure_local_drive_type(&production_volume_root(root)?)?;
    let ancestor = nearest_existing_production_ancestor(root, production_root_path_exists)?;
    ensure_local_canonical_production_root(&fs::canonicalize(ancestor)?)
}

#[cfg(windows)]
fn ensure_local_canonical_production_root(root: &Path) -> Result<(), RecoveryError> {
    use std::os::windows::ffi::OsStrExt as _;
    use windows::{Win32::Storage::FileSystem::GetVolumePathNameW, core::PCWSTR};

    let root: Vec<u16> = root.as_os_str().encode_wide().chain(Some(0)).collect();
    let mut volume_root = vec![0; VOLUME_PATH_BUFFER_LEN];
    unsafe { GetVolumePathNameW(PCWSTR(root.as_ptr()), &mut volume_root) }
        .map_err(|_| RecoveryError::Unavailable(LOCAL_RECOVERY_ROOT_REQUIRED))?;
    ensure_local_drive_type(&volume_root)
}

#[cfg(windows)]
fn production_volume_root(root: &Path) -> Result<Vec<u16>, RecoveryError> {
    use std::path::{Component, Prefix};

    let mut components = root.components();
    let Some(Component::Prefix(prefix)) = components.next() else {
        return Err(RecoveryError::Unavailable(LOCAL_RECOVERY_ROOT_REQUIRED));
    };
    if !matches!(components.next(), Some(Component::RootDir)) {
        return Err(RecoveryError::Unavailable(LOCAL_RECOVERY_ROOT_REQUIRED));
    }

    let volume = match prefix.kind() {
        Prefix::Disk(letter) => vec![letter as u16, b':' as u16, b'\\' as u16, 0],
        Prefix::VerbatimDisk(letter) => vec![
            b'\\' as u16,
            b'\\' as u16,
            b'?' as u16,
            b'\\' as u16,
            letter as u16,
            b':' as u16,
            b'\\' as u16,
            0,
        ],
        Prefix::Verbatim(name) => volume_guid_root(name)
            .ok_or(RecoveryError::Unavailable(LOCAL_RECOVERY_ROOT_REQUIRED))?,
        Prefix::UNC(..) | Prefix::VerbatimUNC(..) => {
            return Err(RecoveryError::Unavailable(LOCAL_RECOVERY_ROOT_REQUIRED));
        }
        _ => {
            return Err(RecoveryError::Unavailable(LOCAL_RECOVERY_ROOT_REQUIRED));
        }
    };
    Ok(volume)
}

#[cfg(windows)]
fn volume_guid_root(name: &std::ffi::OsStr) -> Option<Vec<u16>> {
    use std::os::windows::ffi::OsStrExt as _;
    use windows::core::GUID;

    const PREFIX: &[u8] = b"Volume{";
    const GUID_LEN: usize = 36;

    let name: Vec<u16> = name.encode_wide().collect();
    if name.len() != PREFIX.len() + GUID_LEN + 1
        || name.last() != Some(&(b'}' as u16))
        || !name[..PREFIX.len()]
            .iter()
            .zip(PREFIX)
            .all(|(&unit, &expected)| unit <= 0x7f && (unit as u8).eq_ignore_ascii_case(&expected))
    {
        return None;
    }

    let mut guid = [0; GUID_LEN];
    for (output, &unit) in guid
        .iter_mut()
        .zip(&name[PREFIX.len()..PREFIX.len() + GUID_LEN])
    {
        *output = u8::try_from(unit).ok()?;
    }
    GUID::try_from(std::str::from_utf8(&guid).ok()?).ok()?;

    let mut root = vec![b'\\' as u16, b'\\' as u16, b'?' as u16, b'\\' as u16];
    root.extend(name);
    root.extend([b'\\' as u16, 0]);
    Some(root)
}

#[cfg(windows)]
fn production_root_path_exists(path: &Path) -> io::Result<bool> {
    match fs::symlink_metadata(path) {
        Ok(_) => Ok(true),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error),
    }
}

#[cfg(windows)]
fn nearest_existing_production_ancestor(
    root: &Path,
    mut path_exists: impl FnMut(&Path) -> io::Result<bool>,
) -> io::Result<PathBuf> {
    let mut candidate = root;
    loop {
        if path_exists(candidate)? {
            return Ok(candidate.to_path_buf());
        }
        candidate = candidate.parent().ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::NotFound,
                "recovery root has no existing ancestor",
            )
        })?;
    }
}

#[cfg(windows)]
fn ensure_local_drive_type(root: &[u16]) -> Result<(), RecoveryError> {
    use windows::Win32::Storage::FileSystem::GetDriveTypeW;
    use windows::core::PCWSTR;

    if is_local_recovery_drive_type(unsafe { GetDriveTypeW(PCWSTR(root.as_ptr())) }) {
        Ok(())
    } else {
        Err(RecoveryError::Unavailable(LOCAL_RECOVERY_ROOT_REQUIRED))
    }
}

#[cfg(windows)]
fn is_local_recovery_drive_type(drive_type: u32) -> bool {
    use windows::Win32::System::WindowsProgramming::{DRIVE_FIXED, DRIVE_RAMDISK, DRIVE_REMOVABLE};

    matches!(drive_type, DRIVE_FIXED | DRIVE_REMOVABLE | DRIVE_RAMDISK)
}

#[cfg(windows)]
fn open_root_guard_handle(root: &Path) -> io::Result<fs::File> {
    use std::os::windows::fs::OpenOptionsExt as _;
    use windows::Win32::Storage::FileSystem::{
        FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT, FILE_SHARE_READ, FILE_SHARE_WRITE,
    };

    std::fs::OpenOptions::new()
        .read(true)
        .share_mode((FILE_SHARE_READ | FILE_SHARE_WRITE).0)
        .custom_flags((FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT).0)
        .open(root)
}

#[cfg(windows)]
fn open_recovery_lease(root: &Path) -> Result<fs::File, RecoveryError> {
    use std::os::windows::fs::{MetadataExt as _, OpenOptionsExt as _};
    use windows::Win32::{
        Foundation::ERROR_SHARING_VIOLATION,
        Storage::FileSystem::{FILE_ATTRIBUTE_REPARSE_POINT, FILE_FLAG_OPEN_REPARSE_POINT},
    };

    let lease = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .share_mode(0)
        .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT.0)
        .open(root.join(".markturbo-recovery.lock"))
        .map_err(|error| {
            if error.raw_os_error() == Some(ERROR_SHARING_VIOLATION.0 as i32) {
                RecoveryError::Unavailable(
                    "recovery is already active in another markturbo instance",
                )
            } else {
                RecoveryError::Io(error)
            }
        })?;
    if lease.metadata()?.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT.0 != 0 {
        return Err(RecoveryError::Unavailable(
            "recovery lease must not be a reparse point",
        ));
    }
    Ok(lease)
}

impl fmt::Debug for RecoveryStore {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RecoveryStore")
            .field("root", &self.root)
            .field("limits", &self.limits)
            .finish_non_exhaustive()
    }
}

impl RecoveryStore {
    /// Open the platform production store and prune expired records at startup.
    #[cfg(windows)]
    pub fn open() -> Result<(Self, RecoveryMaintenance), RecoveryError> {
        let root = app_paths::recovery_dir().ok_or(RecoveryError::Unavailable(
            "no per-user application-data directory is available",
        ))?;
        let store = Self::new_production_at(root, Arc::new(DpapiProtector))?;
        let maintenance = store.prune()?;
        Ok((store, maintenance))
    }

    /// Non-Windows builds deliberately provide no plaintext fallback.
    #[cfg(not(windows))]
    pub fn open() -> Result<(Self, RecoveryMaintenance), RecoveryError> {
        Err(RecoveryError::Unavailable(
            "current-user DPAPI is only available on Windows",
        ))
    }

    #[cfg(all(test, windows))]
    fn open_production_at_for_test(
        root: PathBuf,
        protector: Arc<dyn RecoveryProtector>,
    ) -> Result<Self, RecoveryError> {
        Self::new_production_at(root, protector)
    }

    /// Construct a store at an explicit application-data recovery directory.
    #[cfg(test)]
    pub fn new_at(
        root: PathBuf,
        protector: Arc<dyn RecoveryProtector>,
    ) -> Result<Self, RecoveryError> {
        Self::new_at_with_limits(root, protector, RecoveryLimits::default())
    }

    /// Test and diagnostic constructor with explicit retention limits.
    #[cfg(test)]
    pub fn new_at_with_limits(
        root: PathBuf,
        protector: Arc<dyn RecoveryProtector>,
        limits: RecoveryLimits,
    ) -> Result<Self, RecoveryError> {
        fs::create_dir_all(&root)?;
        Ok(Self::from_parts(root, protector, limits))
    }

    #[cfg(windows)]
    fn new_production_at(
        root: PathBuf,
        protector: Arc<dyn RecoveryProtector>,
    ) -> Result<Self, RecoveryError> {
        ensure_local_production_root(&root)?;
        fs::create_dir_all(&root)?;
        let root_guard = Arc::new(ProductionRootGuard::acquire(&root)?);
        let mut store = Self::from_parts(
            root_guard.canonical_root.clone(),
            protector,
            RecoveryLimits::default(),
        );
        store.root_guard = Some(root_guard);
        Ok(store)
    }

    fn from_parts(
        root: PathBuf,
        protector: Arc<dyn RecoveryProtector>,
        limits: RecoveryLimits,
    ) -> Self {
        Self {
            root,
            protector,
            limits,
            capability_lock: Arc::new(Mutex::new(CapabilityState::default())),
            mutation_lock: Arc::new(Mutex::new(MutationState::default())),
            retirement_marker_lock: Arc::new(Mutex::new(())),
            #[cfg(windows)]
            root_guard: None,
            #[cfg(test)]
            failures: Arc::new(TestFailurePoints::default()),
        }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// A plaintext payload above this bound cannot fit in one protected record.
    /// Payloads at or below it still require the final ciphertext-size check.
    pub(crate) fn plaintext_admission_ceiling(&self) -> u64 {
        self.limits.max_record_bytes
    }

    /// Persist a completed encrypted checkpoint, then evict oldest inactive
    /// records if retention requires it. `active_keys` must contain every
    /// currently open dirty buffer; active records are never evicted.
    pub fn checkpoint(
        &self,
        checkpoint: &RecoveryCheckpoint,
        active_keys: &HashSet<RecoveryKey>,
    ) -> Result<CheckpointReceipt, RecoveryError> {
        self.verify_production_root()?;
        self.checkpoint_at(checkpoint, active_keys, SystemTime::now())
    }

    /// Mark a dirty buffer as active and capture its checkpoint generation under
    /// one capability lock. The boolean is true when an already-reserved
    /// eviction means protection must wait for a later checkpoint.
    pub fn activate_and_current_token(&self, key: &RecoveryKey) -> (RecoveryToken, bool) {
        let mut state = self.lock_capabilities();
        state.active_keys.insert(key.clone());
        let protection_deferred = state.evicting_keys.contains(key);
        (
            RecoveryToken {
                key: key.clone(),
                generation: state.generations.get(key).copied().unwrap_or_default(),
            },
            protection_deferred,
        )
    }

    /// Capture the current generation before dispatching an already-active
    /// checkpoint. New dirty buffers must use [`Self::activate_and_current_token`].
    pub fn current_token(&self, key: &RecoveryKey) -> RecoveryToken {
        let state = self.lock_capabilities();
        RecoveryToken {
            key: key.clone(),
            generation: state.generations.get(key).copied().unwrap_or_default(),
        }
    }

    /// Persist a checkpoint only when its scheduling token is still current.
    pub fn checkpoint_if_current(
        &self,
        checkpoint: &RecoveryCheckpoint,
        active_keys: &HashSet<RecoveryKey>,
        token: RecoveryToken,
    ) -> Result<CheckpointOutcome, RecoveryError> {
        let mut batch = self.checkpoint_batch_if_current(
            [RecoveryCheckpointAttempt {
                checkpoint,
                token: &token,
            }],
            active_keys,
        );
        match batch
            .outcomes
            .pop()
            .expect("one recovery checkpoint attempt must produce one outcome")
        {
            CheckpointBatchOutcome::Written => Ok(CheckpointOutcome::Written(CheckpointReceipt {
                maintenance: batch.maintenance,
            })),
            CheckpointBatchOutcome::Superseded => Ok(CheckpointOutcome::Superseded),
            CheckpointBatchOutcome::Failed(error) => Err(error),
            CheckpointBatchOutcome::Deferred => Err(RecoveryError::Unavailable(
                "single recovery checkpoint was unexpectedly deferred",
            )),
        }
    }

    /// Persist current scheduled checkpoints under one root verification,
    /// mutation lock, and retention scan. Writes that evict a record retain
    /// their own transaction; an in-place same-key replacement is atomic.
    pub(crate) fn checkpoint_batch_if_current<'a>(
        &self,
        attempts: impl IntoIterator<Item = RecoveryCheckpointAttempt<'a>>,
        active_keys: &HashSet<RecoveryKey>,
    ) -> CheckpointBatchReceipt {
        let never_cancelled = AtomicBool::new(false);
        self.checkpoint_batch_if_current_cancellable(
            attempts
                .into_iter()
                .map(|attempt| CancellableRecoveryCheckpointAttempt {
                    checkpoint: attempt.checkpoint,
                    token: attempt.token,
                    cancelled: &never_cancelled,
                }),
            active_keys,
        )
    }

    /// Persist current scheduled checkpoints with cancellation scoped to each
    /// attempt. A cancelled item is superseded without interrupting current
    /// siblings. Retention work stops only after its bounded wave when every
    /// remaining current item has been cancelled.
    pub(crate) fn checkpoint_batch_if_current_cancellable<'a>(
        &self,
        attempts: impl IntoIterator<Item = CancellableRecoveryCheckpointAttempt<'a>>,
        active_keys: &HashSet<RecoveryKey>,
    ) -> CheckpointBatchReceipt {
        #[cfg(test)]
        self.failures
            .checkpoint_batches
            .fetch_add(1, Ordering::SeqCst);
        let attempts: Vec<_> = attempts.into_iter().collect();
        if attempts.is_empty() {
            return CheckpointBatchReceipt::default();
        }
        if attempts
            .iter()
            .all(|attempt| recovery_cancelled(attempt.cancelled))
        {
            return checkpoint_batch_superseded(attempts.len());
        }
        if let Err(error) = self.verify_production_root() {
            return checkpoint_batch_failure_for_cancellable_attempts(&attempts, error);
        }

        let mut state = self.lock_mutations();
        let capabilities = self.lock_capabilities();
        let mut outcomes: Vec<Option<CheckpointBatchOutcome>> =
            (0..attempts.len()).map(|_| None).collect();
        let mut current = Vec::with_capacity(attempts.len());
        for (index, attempt) in attempts.iter().enumerate() {
            if recovery_cancelled(attempt.cancelled) {
                outcomes[index] = Some(CheckpointBatchOutcome::Superseded);
                continue;
            }
            if !attempt.checkpoint.key.is_valid() {
                outcomes[index] = Some(CheckpointBatchOutcome::Failed(RecoveryError::Unavailable(
                    "recovery key is invalid",
                )));
                continue;
            }
            let generation = capabilities
                .generations
                .get(&attempt.checkpoint.key)
                .copied()
                .unwrap_or_default();
            if attempt.token.key != attempt.checkpoint.key || attempt.token.generation != generation
            {
                outcomes[index] = Some(CheckpointBatchOutcome::Superseded);
                continue;
            }
            if capabilities.retiring.contains_key(&attempt.checkpoint.key) {
                outcomes[index] = Some(CheckpointBatchOutcome::Deferred);
                continue;
            }
            current.push(CurrentRecoveryCheckpointAttempt {
                index,
                checkpoint: attempt.checkpoint,
                token: attempt.token,
                cancelled: attempt.cancelled,
            });
        }
        drop(capabilities);
        if current.is_empty() {
            return checkpoint_batch_receipt(outcomes, RecoveryMaintenance::default());
        }
        let current_keys: HashSet<_> = current
            .iter()
            .map(|attempt| attempt.checkpoint.key.clone())
            .collect();
        let mut batch_active_keys = active_keys.clone();
        let now = SystemTime::now();
        let input_bytes: Vec<_> = current
            .iter()
            .map(|attempt| checkpoint_wave_input_bytes(attempt.checkpoint))
            .collect();
        let ranges = recovery_wave_ranges(&input_bytes, self.limits.max_total_bytes);
        let (sender, receiver) = std::sync::mpsc::sync_channel(0);

        let mut receipt = std::thread::scope(|scope| {
            let current_for_producer = &current;
            let ranges_for_producer = &ranges;
            let producer = scope.spawn(move || {
                for range in ranges_for_producer {
                    if current_attempts_cancelled(current_for_producer) {
                        return;
                    }
                    let wave = &current_for_producer[range.clone()];
                    let prepared = run_recovery_wave(wave, |attempt| {
                        if recovery_cancelled(attempt.cancelled) {
                            return None;
                        }
                        let prepared = self.prepare_checkpoint(attempt.checkpoint, now);
                        (!recovery_cancelled(attempt.cancelled)).then_some(prepared)
                    });
                    if sender.send(prepared).is_err() {
                        return;
                    }
                }
            });
            let mut producer = Some(producer);

            let PrunedRetention {
                mut maintenance,
                mut scan,
                mut transaction_cleanup_pending,
            } = match self.prune_for_batch_locked_cancellable(now, &current_keys, &|| {
                current_attempts_cancelled(&current)
            }) {
                Ok(CancellablePrunedRetention::Completed(pruned)) => *pruned,
                Ok(CancellablePrunedRetention::Cancelled) => {
                    checkpoint_batch_supersede_pending(&mut outcomes);
                    drop(receiver);
                    join_recovery_worker(
                        producer
                            .take()
                            .expect("recovery preparation producer must be available"),
                    );
                    return checkpoint_batch_receipt(outcomes, RecoveryMaintenance::default());
                }
                Err(error) => {
                    drop(receiver);
                    join_recovery_worker(
                        producer
                            .take()
                            .expect("recovery preparation producer must be available"),
                    );
                    checkpoint_batch_fail_pending(&mut outcomes, &current, error);
                    return checkpoint_batch_receipt(outcomes, RecoveryMaintenance::default());
                }
            };
            if transaction_cleanup_pending {
                drop(receiver);
                join_recovery_worker(
                    producer
                        .take()
                        .expect("recovery preparation producer must be available"),
                );
                checkpoint_batch_fail_pending(
                    &mut outcomes,
                    &current,
                    RecoveryError::Unavailable("recovery transaction cleanup is pending"),
                );
                return checkpoint_batch_receipt(outcomes, maintenance);
            }
            batch_active_keys.extend(
                scan.skipped_replacements
                    .iter()
                    .map(|descriptor| descriptor.key.clone()),
            );

            for range in &ranges {
                if current_attempts_cancelled(&current) {
                    checkpoint_batch_supersede_pending(&mut outcomes);
                    drop(receiver);
                    join_recovery_worker(
                        producer
                            .take()
                            .expect("recovery preparation producer must be available"),
                    );
                    return checkpoint_batch_receipt(outcomes, maintenance);
                }
                let wave = &current[range.clone()];
                let prepared = match receiver.recv() {
                    Ok(prepared) => prepared,
                    Err(_) if current_attempts_cancelled(&current) => {
                        checkpoint_batch_supersede_pending(&mut outcomes);
                        drop(receiver);
                        join_recovery_worker(
                            producer
                                .take()
                                .expect("recovery preparation producer must be available"),
                        );
                        return checkpoint_batch_receipt(outcomes, maintenance);
                    }
                    Err(_) => {
                        join_recovery_worker(
                            producer
                                .take()
                                .expect("recovery preparation producer must be available"),
                        );
                        panic!("recovery preparation producer stopped before completing a wave");
                    }
                };
                for (attempt, prepared) in wave.iter().zip(prepared) {
                    if recovery_cancelled(attempt.cancelled) {
                        outcomes[attempt.index] = Some(CheckpointBatchOutcome::Superseded);
                        continue;
                    }
                    let outcome = match prepared {
                        Some(Ok(ciphertext)) => match self.checkpoint_into_scan_locked_cancellable(
                            attempt.checkpoint,
                            &ciphertext,
                            CheckpointWriteContext {
                                active_keys: &batch_active_keys,
                                now,
                                state: &mut state,
                                scan: &mut scan,
                                maintenance: &mut maintenance,
                                transaction_cleanup_pending: &mut transaction_cleanup_pending,
                                token: Some(attempt.token),
                                cancelled: attempt.cancelled,
                            },
                        ) {
                            Ok(CheckpointWriteResult::Written) => CheckpointBatchOutcome::Written,
                            Ok(CheckpointWriteResult::Superseded) => {
                                CheckpointBatchOutcome::Superseded
                            }
                            Ok(CheckpointWriteResult::Deferred) => CheckpointBatchOutcome::Deferred,
                            Err(CheckpointWriteError::Certain(error)) => {
                                CheckpointBatchOutcome::Failed(error)
                            }
                            Err(CheckpointWriteError::Uncertain(error)) => {
                                outcomes[attempt.index] =
                                    Some(CheckpointBatchOutcome::Failed(error));
                                checkpoint_batch_defer_pending(&mut outcomes, &current);
                                drop(receiver);
                                join_recovery_worker(
                                    producer
                                        .take()
                                        .expect("recovery preparation producer must be available"),
                                );
                                return checkpoint_batch_receipt(outcomes, maintenance);
                            }
                        },
                        Some(Err(error)) => CheckpointBatchOutcome::Failed(error),
                        None => CheckpointBatchOutcome::Superseded,
                    };
                    outcomes[attempt.index] = Some(outcome);
                    if transaction_cleanup_pending {
                        checkpoint_batch_defer_pending(&mut outcomes, &current);
                        drop(receiver);
                        join_recovery_worker(
                            producer
                                .take()
                                .expect("recovery preparation producer must be available"),
                        );
                        return checkpoint_batch_receipt(outcomes, maintenance);
                    }
                }
            }
            drop(receiver);
            join_recovery_worker(
                producer
                    .take()
                    .expect("recovery preparation producer must be available"),
            );
            checkpoint_batch_receipt(outcomes, maintenance)
        });

        if receipt.outcomes.iter().any(|outcome| {
            matches!(
                outcome,
                CheckpointBatchOutcome::Failed(_) | CheckpointBatchOutcome::Deferred
            )
        }) {
            match self.prune_locked(now) {
                Ok(fallback) => {
                    receipt.maintenance.removed_expired += fallback.maintenance.removed_expired;
                    receipt
                        .maintenance
                        .issues
                        .extend(fallback.maintenance.issues);
                }
                Err(error) => {
                    if let Some(outcome) = receipt
                        .outcomes
                        .iter_mut()
                        .find(|outcome| matches!(outcome, CheckpointBatchOutcome::Deferred))
                    {
                        *outcome = CheckpointBatchOutcome::Failed(error);
                    }
                }
            }
        }
        receipt
    }

    /// Durably invalidate captured checkpoint work for one key without waiting
    /// for the recovery-root mutation lock.
    pub fn begin_retirement(&self, key: &RecoveryKey) -> Result<RecoveryRetirement, RecoveryError> {
        self.begin_retirements([key.clone()])
            .map(|batch| RecoveryRetirement { batch })
    }

    /// Durably invalidate captured checkpoint work for an all-or-nothing key
    /// set. One atomic, synced marker makes every listed record non-restorable
    /// before this method returns.
    pub fn begin_retirements(
        &self,
        keys: impl IntoIterator<Item = RecoveryKey>,
    ) -> Result<RecoveryRetirementBatch, RecoveryError> {
        self.verify_production_root()?;
        let keys = normalized_retirement_keys(keys)?;
        let id = {
            let mut state = self.lock_capabilities();
            if keys.iter().any(|key| state.retiring.contains_key(key)) {
                return Err(RecoveryError::Unavailable(
                    "recovery retirement is already pending",
                ));
            }
            let id = state.next_retirement;
            state.next_retirement = state.next_retirement.wrapping_add(1);
            for key in &keys {
                state.active_keys.remove(key);
                let generation = state.generations.entry(key.clone()).or_default();
                *generation = generation.wrapping_add(1);
                state
                    .retiring
                    .insert(key.clone(), RetirementState::Persisting(id));
            }
            id
        };

        let batch = RecoveryRetirementBatch { keys, id };
        let marker = self.retirement_marker_path(&batch.keys);
        let result = {
            let _marker_guard = self.lock_retirement_markers();
            self.write_retirement_marker(&marker, &batch.keys)
        };
        if let Err(error) = result {
            self.clear_persisting_retirements(&batch);
            return Err(error);
        }

        #[cfg(test)]
        self.pause_once_for_test(&self.failures.after_retirement_marker);

        let mut state = self.lock_capabilities();
        if !batch
            .keys
            .iter()
            .all(|key| state.retiring.get(key) == Some(&RetirementState::Persisting(batch.id)))
        {
            return Err(RecoveryError::Unavailable(
                "recovery retirement state changed while its marker was written",
            ));
        }
        for key in &batch.keys {
            state
                .retiring
                .insert(key.clone(), RetirementState::Pending(batch.id));
        }
        Ok(batch)
    }

    /// Release the in-memory gate after a caller abandons background cleanup.
    ///
    /// This only clears the matching in-memory state. It deliberately keeps
    /// the incremented generation, so checkpoint work captured before
    /// retirement remains invalid. A new checkpoint still reconciles the
    /// durable marker before it can publish.
    pub fn abandon_retirement(&self, ticket: &RecoveryRetirement) -> bool {
        self.abandon_retirements(&ticket.batch)
    }

    /// Release the in-memory gate for a retirement batch without restoring any
    /// old checkpoint generation.
    pub fn abandon_retirements(&self, batch: &RecoveryRetirementBatch) -> bool {
        self.finish_retirements(batch)
    }

    /// Finish a retirement under the recovery-root I/O lock.
    ///
    /// The marker already makes the canonical record non-restorable. Any
    /// cleanup failure is returned as `CleanupPending` so callers retain the
    /// pending gate until it can be reconciled safely.
    pub fn complete_retirement(
        &self,
        ticket: RecoveryRetirement,
    ) -> Result<RetirementCompletion, RecoveryError> {
        self.complete_retirements(ticket.batch)
    }

    /// Finish a retirement batch under the recovery-root I/O lock.
    pub fn complete_retirements(
        &self,
        batch: RecoveryRetirementBatch,
    ) -> Result<RetirementCompletion, RecoveryError> {
        if let Err(error) = self.verify_production_root() {
            return Ok(RetirementCompletion::CleanupPending { error });
        }
        let mut state = self.lock_mutations();
        match self.reconcile_transaction_locked() {
            Ok(cleanup) if cleanup.blocks_mutation => {
                return Ok(RetirementCompletion::CleanupPending {
                    error: RecoveryError::Unavailable("recovery transaction cleanup is pending"),
                });
            }
            Ok(_) => {}
            Err(error) => return Ok(RetirementCompletion::CleanupPending { error }),
        }
        if !self.retirements_are_current(&batch) {
            return Ok(RetirementCompletion::CleanupPending {
                error: RecoveryError::Unavailable(
                    "recovery retirement ticket is no longer current",
                ),
            });
        }

        let marker = self.retirement_marker_path(&batch.keys);
        let _marker_guard = self.lock_retirement_markers();
        let mut removed = false;
        for key in &batch.keys {
            let path = self.record_path(key);
            let retired = match self.next_artifact_path("retired", &mut state) {
                Ok(path) => path,
                Err(error) => return Ok(RetirementCompletion::CleanupPending { error }),
            };
            match self.rename_path(&path, &retired) {
                Ok(()) => {
                    if let Err(error) = remove_if_exists(self, &retired) {
                        return Ok(RetirementCompletion::CleanupPending { error });
                    }
                    removed = true;
                }
                Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                Err(error) => {
                    return Ok(RetirementCompletion::CleanupPending {
                        error: error.into(),
                    });
                }
            }
        }
        if let Err(error) = remove_if_exists(self, &marker) {
            return Ok(RetirementCompletion::CleanupPending { error });
        }
        let _ = self.finish_retirements(&batch);
        Ok(RetirementCompletion::Retired { removed })
    }

    /// Invalidate and synchronously retire a record after an intentional Save
    /// or Discard. Callers that cannot wait for I/O should use the two phases.
    pub fn invalidate_and_delete(&self, key: &RecoveryKey) -> Result<bool, RecoveryError> {
        let ticket = self.begin_retirement(key)?;
        match self.complete_retirement(ticket)? {
            RetirementCompletion::Retired { removed } => Ok(removed),
            RetirementCompletion::CleanupPending { error } => Err(error),
        }
    }

    /// Decode every valid non-expired record. Bad recovery data is reported and
    /// skipped; it only reconciles recovery-store transaction artifacts.
    pub fn recover(&self) -> Result<RecoveryScan, RecoveryError> {
        self.verify_production_root()?;
        let _guard = self.lock_mutations();
        let transaction_cleanup = self.reconcile_transaction_locked()?;
        let retirements =
            self.reconcile_retirement_markers_locked(!transaction_cleanup.blocks_mutation)?;
        let mut scan = self
            .scan_recovery_locked_excluding_marked(SystemTime::now(), &retirements.marked_keys)?;
        let cleanup_issues = scan.apply_pending_cleanup(transaction_cleanup);
        scan.issues.extend(cleanup_issues);
        Ok(RecoveryScan {
            records: scan
                .records
                .into_iter()
                .map(|entry| {
                    entry
                        .recovered
                        .expect("recovery scans must retain decoded record payloads")
                })
                .collect(),
            issues: retirements.issues.into_iter().chain(scan.issues).collect(),
        })
    }

    /// Remove expired or definitely invalid records, while reporting records
    /// that cannot be decoded. Unreadable records remain so a transient
    /// protection failure cannot silently discard recoverable text.
    /// Call this at startup and after every completed checkpoint.
    pub fn prune(&self) -> Result<RecoveryMaintenance, RecoveryError> {
        self.verify_production_root()?;
        let _guard = self.lock_mutations();
        self.prune_locked(SystemTime::now())
            .map(|pruned| pruned.maintenance)
    }

    fn checkpoint_at(
        &self,
        checkpoint: &RecoveryCheckpoint,
        active_keys: &HashSet<RecoveryKey>,
        now: SystemTime,
    ) -> Result<CheckpointReceipt, RecoveryError> {
        if !checkpoint.key.is_valid() {
            return Err(RecoveryError::Unavailable("recovery key is invalid"));
        }
        let mut state = self.lock_mutations();
        self.checkpoint_at_locked(checkpoint, active_keys, now, &mut state)
    }

    fn checkpoint_at_locked(
        &self,
        checkpoint: &RecoveryCheckpoint,
        active_keys: &HashSet<RecoveryKey>,
        now: SystemTime,
        state: &mut MutationState,
    ) -> Result<CheckpointReceipt, RecoveryError> {
        let PrunedRetention {
            mut maintenance,
            mut scan,
            transaction_cleanup_pending,
        } = self.prune_locked(now)?;
        if transaction_cleanup_pending {
            return Err(RecoveryError::Unavailable(
                "recovery transaction cleanup is pending",
            ));
        }
        let mut transaction_cleanup_pending = false;
        let ciphertext = self.prepare_checkpoint(checkpoint, now)?;
        let never_cancelled = AtomicBool::new(false);
        match self
            .checkpoint_into_scan_locked_cancellable(
                checkpoint,
                &ciphertext,
                CheckpointWriteContext {
                    active_keys,
                    now,
                    state,
                    scan: &mut scan,
                    maintenance: &mut maintenance,
                    transaction_cleanup_pending: &mut transaction_cleanup_pending,
                    token: None,
                    cancelled: &never_cancelled,
                },
            )
            .map_err(CheckpointWriteError::into_error)?
        {
            CheckpointWriteResult::Written => {}
            CheckpointWriteResult::Superseded => {
                unreachable!("an uncancellable checkpoint cannot be superseded")
            }
            CheckpointWriteResult::Deferred => {
                unreachable!("an uncancellable checkpoint cannot be deferred")
            }
        }
        Ok(CheckpointReceipt { maintenance })
    }

    fn checkpoint_into_scan_locked_cancellable(
        &self,
        checkpoint: &RecoveryCheckpoint,
        ciphertext: &[u8],
        context: CheckpointWriteContext<'_>,
    ) -> Result<CheckpointWriteResult, CheckpointWriteError> {
        let CheckpointWriteContext {
            active_keys,
            now,
            state,
            scan,
            maintenance,
            transaction_cleanup_pending,
            token,
            cancelled,
        } = context;
        let capability = match self.checkpoint_capability(&checkpoint.key, token) {
            CheckpointCapability::Current { active_keys } => active_keys,
            CheckpointCapability::Superseded => return Ok(CheckpointWriteResult::Superseded),
            CheckpointCapability::Deferred if token.is_some() => {
                return Ok(CheckpointWriteResult::Deferred);
            }
            CheckpointCapability::Deferred => {
                return Err(CheckpointWriteError::Certain(RecoveryError::Unavailable(
                    "recovery retirement is pending",
                )));
            }
        };
        let new_size = ciphertext.len() as u64;
        if new_size > self.limits.max_record_bytes {
            return Err(CheckpointWriteError::Certain(
                RecoveryError::OversizedCheckpoint {
                    bytes: new_size,
                    limit: self.limits.max_record_bytes,
                },
            ));
        }
        if scan.has_metadata_unknown {
            return Err(CheckpointWriteError::Certain(RecoveryError::Unavailable(
                "a recovery record cannot be inspected for retention",
            )));
        }
        let target_path = self.record_path(&checkpoint.key);
        let replaced_bytes = scan
            .known_record_sizes
            .get(&target_path)
            .copied()
            .unwrap_or(0);
        let replacing = usize::from(scan.known_record_sizes.contains_key(&target_path));
        let mut retained: Vec<_> = scan
            .records
            .iter()
            .filter(|entry| entry.key != checkpoint.key && entry.path != target_path)
            .collect();
        let mut record_count = scan.known_record_count.saturating_sub(replacing);
        let mut total = scan.known_bytes.saturating_sub(replaced_bytes);
        let mut evictions = Vec::new();
        let mut eviction_keys = Vec::new();

        let active_keys: HashSet<_> = active_keys
            .iter()
            .chain(capability.iter())
            .cloned()
            .collect();
        while record_count + 1 > self.limits.max_records
            || total.saturating_add(new_size) > self.limits.max_total_bytes
        {
            let candidate = oldest_inactive_index(&retained, &active_keys).ok_or(
                RecoveryError::QuotaExceeded {
                    required_records: record_count.saturating_add(1),
                    max_records: self.limits.max_records,
                    required_bytes: total.saturating_add(new_size),
                    max_total_bytes: self.limits.max_total_bytes,
                },
            )?;
            let evicted = retained.remove(candidate);
            eviction_keys.push(evicted.key.clone());
            evictions.push(evicted.path.clone());
            record_count -= 1;
            total = total.saturating_sub(evicted.bytes);
        }

        let requires_transaction = !evictions.is_empty();
        let mut staged_paths = evictions;
        if replacing != 0 {
            staged_paths.push(target_path.clone());
        }
        staged_paths.sort();
        staged_paths.dedup();

        match self.checkpoint_capability(&checkpoint.key, token) {
            CheckpointCapability::Superseded => return Ok(CheckpointWriteResult::Superseded),
            CheckpointCapability::Deferred => return Ok(CheckpointWriteResult::Deferred),
            CheckpointCapability::Current { .. } => {}
        }
        if scan.marked_keys.contains(&checkpoint.key) {
            return match self.checkpoint_capability(&checkpoint.key, token) {
                CheckpointCapability::Superseded => Ok(CheckpointWriteResult::Superseded),
                CheckpointCapability::Deferred => Ok(CheckpointWriteResult::Deferred),
                CheckpointCapability::Current { .. } => Err(CheckpointWriteError::Certain(
                    RecoveryError::Unavailable("recovery retirement marker cleanup is pending"),
                )),
            };
        }
        if recovery_cancelled(cancelled) {
            return Ok(CheckpointWriteResult::Superseded);
        }
        if !requires_transaction {
            let prepared = self.prepare_atomic_write(ciphertext)?;
            #[cfg(test)]
            self.pause_checkpoint_write_for_test(&self.failures.after_checkpoint_prepare);
            if recovery_cancelled(cancelled) {
                return Ok(CheckpointWriteResult::Superseded);
            }
            #[cfg(test)]
            self.pause_once_for_test(&self.failures.after_checkpoint_final_check);
            self.persist_prepared_write(prepared, &target_path)
                .map_err(CheckpointWriteError::Uncertain)?;
        } else {
            match self.commit_checkpoint_transaction_cancellable(
                CheckpointTransactionPlan {
                    target_path: &target_path,
                    ciphertext,
                    paths_to_stage: &staged_paths,
                    eviction_keys: &eviction_keys,
                },
                state,
                cancelled,
                scan,
                maintenance,
                transaction_cleanup_pending,
            ) {
                Ok(CheckpointWriteResult::Written) => {}
                Ok(CheckpointWriteResult::Superseded) => {
                    return Ok(CheckpointWriteResult::Superseded);
                }
                Ok(CheckpointWriteResult::Deferred) => {
                    return Ok(CheckpointWriteResult::Deferred);
                }
                Err(error) => return Err(CheckpointWriteError::Uncertain(error)),
            }
        }
        scan.forget_paths_and_records(&staged_paths);
        scan.insert_checkpoint(StoredRecord {
            path: target_path,
            bytes: new_size,
            key: checkpoint.key.clone(),
            checkpointed_at: now,
            recovered: None,
        });
        Ok(CheckpointWriteResult::Written)
    }

    fn prepare_checkpoint(
        &self,
        checkpoint: &RecoveryCheckpoint,
        now: SystemTime,
    ) -> Result<Vec<u8>, RecoveryError> {
        let disk = DiskRecord::from_checkpoint(checkpoint, now)?;
        let plaintext = serde_json::to_vec(&disk)?;
        self.protector.protect(&plaintext)
    }

    fn prune_locked(&self, now: SystemTime) -> Result<PrunedRetention, RecoveryError> {
        let mut cleanup = self.reconcile_transaction_locked()?;
        let transaction_cleanup_pending = cleanup.blocks_mutation;
        let retirements = self.reconcile_retirement_markers_locked(!transaction_cleanup_pending)?;
        cleanup.extend(self.cleanup_retired_locked(!transaction_cleanup_pending)?);
        let mut scan =
            self.scan_retention_locked_excluding_marked(now, &retirements.marked_keys)?;
        let cleanup_issues = scan.apply_pending_cleanup(cleanup);
        scan.issues.extend(cleanup_issues);
        let mut pruned = self.prune_scan_locked(scan, !transaction_cleanup_pending)?;
        pruned.maintenance.issues.extend(retirements.issues);
        Ok(pruned)
    }

    fn prune_for_batch_locked_cancellable(
        &self,
        now: SystemTime,
        replacement_keys: &HashSet<RecoveryKey>,
        all_current_cancelled: &(dyn Fn() -> bool + Sync),
    ) -> Result<CancellablePrunedRetention, RecoveryError> {
        if all_current_cancelled() {
            return Ok(CancellablePrunedRetention::Cancelled);
        }
        let mut cleanup = self.reconcile_transaction_locked()?;
        let transaction_cleanup_pending = cleanup.blocks_mutation;
        let retirements = self.reconcile_retirement_markers_locked(!transaction_cleanup_pending)?;
        cleanup.extend(self.cleanup_retired_locked(!transaction_cleanup_pending)?);
        if all_current_cancelled() {
            return Ok(CancellablePrunedRetention::Cancelled);
        }
        let mut scan = match self.scan_retention_locked_excluding_cancellable(
            now,
            replacement_keys,
            &retirements.marked_keys,
            all_current_cancelled,
        )? {
            CancellableScan::Completed(scan) => *scan,
            CancellableScan::Cancelled => return Ok(CancellablePrunedRetention::Cancelled),
        };
        let cleanup_issues = scan.apply_pending_cleanup(cleanup);
        scan.issues.extend(cleanup_issues);
        if all_current_cancelled() {
            return Ok(CancellablePrunedRetention::Cancelled);
        }
        let mut pruned = self.prune_scan_locked(scan, !transaction_cleanup_pending)?;
        pruned.maintenance.issues.extend(retirements.issues);
        Ok(CancellablePrunedRetention::Completed(Box::new(pruned)))
    }

    fn prune_scan_locked(
        &self,
        mut scan: ScanState,
        cleanup_allowed: bool,
    ) -> Result<PrunedRetention, RecoveryError> {
        if !cleanup_allowed {
            let issues = std::mem::take(&mut scan.issues);
            return Ok(PrunedRetention {
                maintenance: RecoveryMaintenance {
                    removed_expired: 0,
                    issues,
                },
                scan,
                transaction_cleanup_pending: true,
            });
        }
        let mut removed_expired = 0;
        let expired_paths = std::mem::take(&mut scan.expired_paths);
        let mut removed_paths = Vec::new();
        for path in &expired_paths {
            match self.remove_path(path) {
                Ok(()) => {
                    removed_expired += 1;
                    removed_paths.push(path.clone());
                }
                Err(error) if error.kind() == io::ErrorKind::NotFound => {
                    removed_paths.push(path.clone());
                }
                Err(_) => scan
                    .issues
                    .push(RecoveryIssue::CleanupPending { path: path.clone() }),
            }
        }
        let invalid_paths = std::mem::take(&mut scan.invalid_paths);
        for path in &invalid_paths {
            match self.remove_path(path) {
                Ok(()) => removed_paths.push(path.clone()),
                Err(error) if error.kind() == io::ErrorKind::NotFound => {
                    removed_paths.push(path.clone());
                }
                Err(_) => scan
                    .issues
                    .push(RecoveryIssue::CleanupPending { path: path.clone() }),
            }
        }
        scan.forget_paths(&removed_paths);
        let issues = std::mem::take(&mut scan.issues);
        Ok(PrunedRetention {
            maintenance: RecoveryMaintenance {
                removed_expired,
                issues,
            },
            scan,
            transaction_cleanup_pending: false,
        })
    }

    fn commit_checkpoint_transaction_cancellable(
        &self,
        plan: CheckpointTransactionPlan<'_>,
        state: &mut MutationState,
        cancelled: &AtomicBool,
        scan: &mut ScanState,
        maintenance: &mut RecoveryMaintenance,
        transaction_cleanup_pending: &mut bool,
    ) -> Result<CheckpointWriteResult, RecoveryError> {
        let CheckpointTransactionPlan {
            target_path,
            ciphertext,
            paths_to_stage,
            eviction_keys,
        } = plan;
        let target_name = recovery_filename(target_path)?;
        let new_staged_path = self.next_artifact_path("new", state)?;
        let transaction = RecoveryTransaction {
            version: TRANSACTION_VERSION,
            target_name,
            new_staged_name: recovery_filename(&new_staged_path)?,
            staged: paths_to_stage
                .iter()
                .map(|path| {
                    Ok(RecoveryTransactionEntry {
                        original_name: recovery_filename(path)?,
                        staged_name: recovery_filename(&self.next_artifact_path("stage", state)?)?,
                    })
                })
                .collect::<Result<Vec<_>, RecoveryError>>()?,
        };
        transaction.validate()?;
        if !self.write_transaction_journal_cancellable(&transaction, cancelled)? {
            return Ok(CheckpointWriteResult::Superseded);
        }

        let reserved = {
            let mut capabilities = self.lock_capabilities();
            if eviction_keys.iter().any(|key| {
                capabilities.active_keys.contains(key) || capabilities.evicting_keys.contains(key)
            }) {
                false
            } else {
                capabilities
                    .evicting_keys
                    .extend(eviction_keys.iter().cloned());
                true
            }
        };
        if !reserved {
            let cleanup = self.rollback_transaction_locked(&transaction)?;
            *transaction_cleanup_pending |= cleanup.blocks_mutation;
            maintenance
                .issues
                .extend(scan.apply_pending_cleanup(cleanup));
            return Ok(CheckpointWriteResult::Deferred);
        }
        #[cfg(test)]
        self.pause_once_for_test(&self.failures.after_eviction_reservation);

        for entry in &transaction.staged {
            if let Err(error) = self.rename_path(
                &self.root.join(&entry.original_name),
                &self.root.join(&entry.staged_name),
            ) {
                if self.rollback_transaction_locked(&transaction).is_ok() {
                    self.clear_eviction_reservations(eviction_keys);
                }
                return Err(error.into());
            }
        }

        let prepared = match self.prepare_atomic_write(ciphertext) {
            Ok(prepared) => prepared,
            Err(error) => {
                if self.rollback_transaction_locked(&transaction).is_ok() {
                    self.clear_eviction_reservations(eviction_keys);
                }
                return Err(error);
            }
        };
        if let Err(error) =
            self.persist_prepared_write(prepared, &self.root.join(&transaction.new_staged_name))
        {
            if self.rollback_transaction_locked(&transaction).is_ok() {
                self.clear_eviction_reservations(eviction_keys);
            }
            return Err(error);
        }
        if let Err(error) =
            self.rename_path(&self.root.join(&transaction.new_staged_name), target_path)
        {
            if self.rollback_transaction_locked(&transaction).is_ok() {
                self.clear_eviction_reservations(eviction_keys);
            }
            return Err(error.into());
        }

        // The marker is the durable commit point. If writing it fails, leave the
        // staged records and journal intact so the next open can infer a fully
        // published transaction without exposing an over-quota record set.
        self.write_transaction_commit_marker()?;
        self.clear_eviction_reservations(eviction_keys);
        let cleanup = self.finish_committed_transaction_locked(&transaction);
        *transaction_cleanup_pending |= cleanup.blocks_mutation;
        maintenance
            .issues
            .extend(scan.apply_pending_cleanup(cleanup));
        Ok(CheckpointWriteResult::Written)
    }

    fn reconcile_transaction_locked(&self) -> Result<PendingCleanup, RecoveryError> {
        let journal_path = self.transaction_journal_path();
        if !self.path_is_present(&journal_path)? {
            self.validate_transaction_artifacts_locked(None)?;
            let mut cleanup = PendingCleanup::default();
            if !self.cleanup_protocol_path(&self.transaction_commit_path(), false, &mut cleanup) {
                cleanup.blocks_mutation = true;
            }
            self.clear_all_eviction_reservations();
            return Ok(cleanup);
        }
        let bytes = fs::read(&journal_path)?;
        let transaction: RecoveryTransaction = serde_json::from_slice(&bytes)
            .map_err(|_| RecoveryError::Unavailable("recovery transaction journal is invalid"))?;
        transaction.validate()?;
        self.validate_transaction_artifacts_locked(Some(&transaction))?;

        let cleanup = match self.transaction_disposition(&transaction)? {
            TransactionDisposition::Commit { inferred } => {
                if inferred {
                    self.write_transaction_commit_marker()?;
                }
                self.finish_committed_transaction_locked(&transaction)
            }
            TransactionDisposition::Rollback => self.rollback_transaction_locked(&transaction)?,
        };
        self.clear_all_eviction_reservations();
        Ok(cleanup)
    }

    fn validate_transaction_artifacts_locked(
        &self,
        transaction: Option<&RecoveryTransaction>,
    ) -> Result<(), RecoveryError> {
        let mut expected = HashSet::new();
        if let Some(transaction) = transaction {
            expected.insert(transaction.new_staged_name.as_str());
            expected.extend(
                transaction
                    .staged
                    .iter()
                    .map(|entry| entry.staged_name.as_str()),
            );
        }
        for entry in fs::read_dir(&self.root)? {
            let entry = entry?;
            let name = entry.file_name();
            let Some(name) = name.to_str() else {
                continue;
            };
            if (is_transaction_artifact_name(name, "new")
                || is_transaction_artifact_name(name, "stage"))
                && !expected.contains(name)
            {
                return Err(RecoveryError::Unavailable(
                    "recovery transaction artifacts do not match the journal",
                ));
            }
        }
        Ok(())
    }

    fn transaction_disposition(
        &self,
        transaction: &RecoveryTransaction,
    ) -> Result<TransactionDisposition, RecoveryError> {
        let target_present = self.path_is_present(&self.root.join(&transaction.target_name))?;
        let commit_present = self.path_is_present(&self.transaction_commit_path())?;
        let new_present = self.path_is_present(&self.root.join(&transaction.new_staged_name))?;
        let mut states = Vec::with_capacity(transaction.staged.len());
        for entry in &transaction.staged {
            states.push((
                entry.original_name == transaction.target_name,
                self.path_is_present(&self.root.join(&entry.original_name))?,
                self.path_is_present(&self.root.join(&entry.staged_name))?,
            ));
        }

        let originals_hidden = states
            .iter()
            .all(|(is_target, original, _)| *is_target || !*original);
        if commit_present {
            if target_present && !new_present && originals_hidden {
                return Ok(TransactionDisposition::Commit { inferred: false });
            }
            return Err(RecoveryError::Unavailable(
                "recovery transaction state is uncertain",
            ));
        }

        let fully_published = target_present
            && !new_present
            && originals_hidden
            && states.iter().all(|(_, _, staged)| *staged);
        if fully_published {
            return Ok(TransactionDisposition::Commit { inferred: true });
        }

        let target_is_original = states.iter().any(|(is_target, _, _)| *is_target);
        let target_is_unpublished = target_is_original || !target_present;
        let rollback_is_reversible = states
            .iter()
            .all(|(_, original, staged)| *original != *staged);
        if target_is_unpublished && rollback_is_reversible {
            Ok(TransactionDisposition::Rollback)
        } else {
            Err(RecoveryError::Unavailable(
                "recovery transaction state is uncertain",
            ))
        }
    }

    fn finish_committed_transaction_locked(
        &self,
        transaction: &RecoveryTransaction,
    ) -> PendingCleanup {
        let mut cleanup = PendingCleanup::default();
        let mut data_removed = true;
        for entry in &transaction.staged {
            data_removed &=
                self.cleanup_protocol_path(&self.root.join(&entry.staged_name), true, &mut cleanup);
        }
        data_removed &= self.cleanup_protocol_path(
            &self.root.join(&transaction.new_staged_name),
            true,
            &mut cleanup,
        );
        if !data_removed {
            cleanup.blocks_mutation = true;
            return cleanup;
        }
        if !self.cleanup_protocol_path(&self.transaction_journal_path(), false, &mut cleanup) {
            cleanup.blocks_mutation = true;
            return cleanup;
        }
        if !self.cleanup_protocol_path(&self.transaction_commit_path(), false, &mut cleanup) {
            cleanup.blocks_mutation = true;
        }
        cleanup
    }

    fn rollback_transaction_locked(
        &self,
        transaction: &RecoveryTransaction,
    ) -> Result<PendingCleanup, RecoveryError> {
        let mut cleanup = PendingCleanup::default();
        let new_removed = self.cleanup_protocol_path(
            &self.root.join(&transaction.new_staged_name),
            true,
            &mut cleanup,
        );
        for entry in &transaction.staged {
            let original = self.root.join(&entry.original_name);
            let staged = self.root.join(&entry.staged_name);
            match (
                self.path_is_present(&original)?,
                self.path_is_present(&staged)?,
            ) {
                (false, true) => self.rename_path(&staged, &original)?,
                (true, false) => {}
                _ => {
                    return Err(RecoveryError::Unavailable(
                        "recovery transaction rollback found conflicting records",
                    ));
                }
            }
        }
        if !new_removed {
            cleanup.blocks_mutation = true;
            return Ok(cleanup);
        }
        if !self.cleanup_protocol_path(&self.transaction_journal_path(), false, &mut cleanup) {
            cleanup.blocks_mutation = true;
            return Ok(cleanup);
        }
        if !self.cleanup_protocol_path(&self.transaction_commit_path(), false, &mut cleanup) {
            cleanup.blocks_mutation = true;
        }
        Ok(cleanup)
    }

    fn cleanup_protocol_path(
        &self,
        path: &Path,
        reserves_quota: bool,
        cleanup: &mut PendingCleanup,
    ) -> bool {
        match remove_if_exists(self, path) {
            Ok(()) => true,
            Err(_) => {
                cleanup.record(path, reserves_quota);
                false
            }
        }
    }

    #[cfg(test)]
    fn write_transaction_journal(
        &self,
        transaction: &RecoveryTransaction,
    ) -> Result<(), RecoveryError> {
        let never_cancelled = AtomicBool::new(false);
        let wrote = self.write_transaction_journal_cancellable(transaction, &never_cancelled)?;
        debug_assert!(
            wrote,
            "an uncancellable transaction journal must be written"
        );
        Ok(())
    }

    fn write_transaction_journal_cancellable(
        &self,
        transaction: &RecoveryTransaction,
        cancelled: &AtomicBool,
    ) -> Result<bool, RecoveryError> {
        let bytes = serde_json::to_vec(transaction)?;
        let temp = self.prepare_atomic_write(&bytes)?;
        #[cfg(test)]
        self.pause_checkpoint_write_for_test(&self.failures.before_transaction_journal);
        if recovery_cancelled(cancelled) {
            return Ok(false);
        }
        if self.path_is_present(&self.transaction_commit_path())? {
            return Err(RecoveryError::Unavailable(
                "recovery transaction commit marker cleanup is pending",
            ));
        }
        self.persist_prepared_write_noclobber(temp, &self.transaction_journal_path())?;
        #[cfg(test)]
        self.pause_checkpoint_write_for_test(&self.failures.after_transaction_journal);
        Ok(true)
    }

    fn write_transaction_commit_marker(&self) -> Result<(), RecoveryError> {
        let temp = self.prepare_atomic_write(b"committed")?;
        self.persist_prepared_write(temp, &self.transaction_commit_path())
    }

    fn cleanup_retired_locked(
        &self,
        cleanup_allowed: bool,
    ) -> Result<PendingCleanup, RecoveryError> {
        let mut cleanup = PendingCleanup::default();
        for entry in fs::read_dir(&self.root)? {
            let entry = entry?;
            let path = entry.path();
            if is_retired_path(&path) {
                if cleanup_allowed {
                    self.cleanup_protocol_path(&path, true, &mut cleanup);
                } else {
                    cleanup.reserve_quota(&path);
                }
            }
        }
        Ok(cleanup)
    }

    fn reconcile_retirement_markers_locked(
        &self,
        cleanup_allowed: bool,
    ) -> Result<ReconciledRetirements, RecoveryError> {
        let mut marker_paths = Vec::new();
        for entry in fs::read_dir(&self.root)? {
            let entry = entry?;
            let path = entry.path();
            let is_marker_prefix = entry
                .file_name()
                .to_str()
                .is_some_and(|name| name.starts_with(RETIREMENT_MARKER_PREFIX));
            if is_marker_prefix && !is_retirement_marker_path(&path) {
                return Err(RecoveryError::Unavailable(
                    "recovery retirement marker filename is invalid",
                ));
            }
            if is_marker_prefix {
                marker_paths.push(path);
            }
        }
        let markers = marker_paths
            .into_iter()
            .map(|path| {
                let mut file = fs::File::open(&path)?;
                let keys = decode_retirement_marker(&mut file)?;
                if path != self.retirement_marker_path(&keys) {
                    return Err(RecoveryError::Unavailable(
                        "recovery retirement marker keys do not match",
                    ));
                }
                Ok((path, keys))
            })
            .collect::<Result<Vec<_>, RecoveryError>>()?;
        let mut reconciled = ReconciledRetirements {
            issues: Vec::new(),
            marked_keys: HashSet::new(),
        };
        for (marker, keys) in markers {
            if !cleanup_allowed {
                reconciled.marked_keys.extend(keys);
                continue;
            }
            let issues = self.reconcile_retirement_marker_locked(&marker, &keys)?;
            if !issues.is_empty() {
                reconciled.marked_keys.extend(keys);
                reconciled.issues.extend(issues);
            }
        }
        Ok(reconciled)
    }

    fn reconcile_retirement_marker_locked(
        &self,
        marker: &Path,
        keys: &[RecoveryKey],
    ) -> Result<Vec<RecoveryIssue>, RecoveryError> {
        let _marker_guard = self.lock_retirement_markers();
        if !self.path_is_present(marker)? {
            return Ok(Vec::new());
        }
        let mut issues = Vec::new();
        for key in keys {
            let path = self.record_path(key);
            if remove_if_exists(self, &path).is_err() {
                issues.push(RecoveryIssue::CleanupPending { path });
            }
        }
        if issues.is_empty() && remove_if_exists(self, marker).is_err() {
            issues.push(RecoveryIssue::CleanupPending {
                path: marker.to_path_buf(),
            });
        }
        Ok(issues)
    }

    fn scan_recovery_locked_excluding_marked(
        &self,
        now: SystemTime,
        marked_keys: &HashSet<RecoveryKey>,
    ) -> Result<ScanState, RecoveryError> {
        self.scan_locked(now, ScanPurpose::Recovery, None, marked_keys)
    }

    #[cfg(test)]
    fn scan_retention_locked(&self, now: SystemTime) -> Result<ScanState, RecoveryError> {
        self.scan_retention_locked_excluding_marked(now, &HashSet::new())
    }

    fn scan_retention_locked_excluding_marked(
        &self,
        now: SystemTime,
        marked_keys: &HashSet<RecoveryKey>,
    ) -> Result<ScanState, RecoveryError> {
        self.scan_retention_locked_excluding_and_marked(now, &HashSet::new(), marked_keys)
    }

    #[cfg(test)]
    fn scan_retention_locked_excluding(
        &self,
        now: SystemTime,
        replacement_keys: &HashSet<RecoveryKey>,
    ) -> Result<ScanState, RecoveryError> {
        self.scan_retention_locked_excluding_and_marked(now, replacement_keys, &HashSet::new())
    }

    fn scan_retention_locked_excluding_and_marked(
        &self,
        now: SystemTime,
        replacement_keys: &HashSet<RecoveryKey>,
        marked_keys: &HashSet<RecoveryKey>,
    ) -> Result<ScanState, RecoveryError> {
        #[cfg(test)]
        self.failures.retention_scans.fetch_add(1, Ordering::SeqCst);
        self.scan_locked(
            now,
            ScanPurpose::Retention,
            Some(replacement_keys),
            marked_keys,
        )
    }

    fn scan_retention_locked_excluding_cancellable(
        &self,
        now: SystemTime,
        replacement_keys: &HashSet<RecoveryKey>,
        marked_keys: &HashSet<RecoveryKey>,
        cancelled: &(dyn Fn() -> bool + Sync),
    ) -> Result<CancellableScan, RecoveryError> {
        #[cfg(test)]
        self.failures.retention_scans.fetch_add(1, Ordering::SeqCst);
        self.scan_locked_cancellable(
            now,
            ScanPurpose::Retention,
            Some(replacement_keys),
            marked_keys,
            cancelled,
        )
    }

    fn scan_locked(
        &self,
        now: SystemTime,
        purpose: ScanPurpose,
        replacement_keys: Option<&HashSet<RecoveryKey>>,
        marked_keys: &HashSet<RecoveryKey>,
    ) -> Result<ScanState, RecoveryError> {
        let never_cancelled = || false;
        match self.scan_locked_cancellable(
            now,
            purpose,
            replacement_keys,
            marked_keys,
            &never_cancelled,
        )? {
            CancellableScan::Completed(scan) => Ok(*scan),
            CancellableScan::Cancelled => unreachable!("an uncancellable scan cannot be cancelled"),
        }
    }

    fn scan_locked_cancellable(
        &self,
        now: SystemTime,
        purpose: ScanPurpose,
        replacement_keys: Option<&HashSet<RecoveryKey>>,
        marked_keys: &HashSet<RecoveryKey>,
        cancelled: &(dyn Fn() -> bool + Sync),
    ) -> Result<CancellableScan, RecoveryError> {
        if cancelled() {
            return Ok(CancellableScan::Cancelled);
        }
        let mut scan = ScanState {
            marked_keys: marked_keys.clone(),
            ..ScanState::default()
        };
        let mut candidates = Vec::new();
        for entry in fs::read_dir(&self.root)? {
            if cancelled() {
                return Ok(CancellableScan::Cancelled);
            }
            let entry = entry?;
            let path = entry.path();
            let Some(filename_key) = record_key_from_path(&path) else {
                continue;
            };
            let metadata = match entry.metadata() {
                Ok(metadata) if metadata.is_file() => metadata,
                Ok(_) => continue,
                Err(_) => {
                    scan.has_metadata_unknown = true;
                    scan.issues.push(RecoveryIssue::Unreadable { path });
                    continue;
                }
            };
            let bytes = metadata.len();
            scan.known_record_count += 1;
            scan.known_bytes = scan.known_bytes.saturating_add(bytes);
            scan.known_record_sizes.insert(path.clone(), bytes);
            if marked_keys.contains(&filename_key) {
                continue;
            }
            if bytes > self.limits.max_record_bytes {
                scan.issues.push(RecoveryIssue::Oversized {
                    path: path.clone(),
                    bytes,
                });
                scan.invalid_paths.push(path);
                continue;
            }
            if matches!(purpose, ScanPurpose::Retention)
                && replacement_keys.is_some_and(|keys| keys.contains(&filename_key))
            {
                scan.skipped_replacements.push(SkippedRetentionDescriptor {
                    path,
                    bytes,
                    key: filename_key,
                });
                continue;
            }
            candidates.push(RecordCandidate {
                path,
                bytes,
                filename_key,
            });
        }
        let input_bytes: Vec<_> = candidates.iter().map(|candidate| candidate.bytes).collect();
        for range in recovery_wave_ranges(&input_bytes, self.limits.max_total_bytes) {
            if cancelled() {
                return Ok(CancellableScan::Cancelled);
            }
            let decoded = run_recovery_wave(&candidates[range], |candidate| {
                self.decode_record_candidate(candidate, now, purpose)
            });
            if cancelled() {
                return Ok(CancellableScan::Cancelled);
            }
            for decoded in decoded {
                match decoded {
                    DecodedRecord::Unreadable { path } => {
                        scan.issues.push(RecoveryIssue::Unreadable { path });
                    }
                    DecodedRecord::Malformed { path } => {
                        scan.issues
                            .push(RecoveryIssue::Malformed { path: path.clone() });
                        scan.invalid_paths.push(path);
                    }
                    DecodedRecord::Expired { path } => {
                        scan.issues
                            .push(RecoveryIssue::Expired { path: path.clone() });
                        scan.expired_paths.push(path);
                    }
                    DecodedRecord::Stored(record) => scan.records.push(*record),
                }
            }
        }
        Ok(CancellableScan::Completed(Box::new(scan)))
    }

    fn decode_record_candidate(
        &self,
        candidate: &RecordCandidate,
        now: SystemTime,
        purpose: ScanPurpose,
    ) -> DecodedRecord {
        let ciphertext = match fs::read(&candidate.path) {
            Ok(ciphertext) => ciphertext,
            Err(_) => {
                return DecodedRecord::Unreadable {
                    path: candidate.path.clone(),
                };
            }
        };
        let plaintext = match self.protector.unprotect(&ciphertext) {
            Ok(plaintext) => plaintext,
            Err(_) => {
                return DecodedRecord::Unreadable {
                    path: candidate.path.clone(),
                };
            }
        };
        let disk: DiskRecord = match serde_json::from_slice(&plaintext) {
            Ok(record) => record,
            Err(_) => {
                return DecodedRecord::Malformed {
                    path: candidate.path.clone(),
                };
            }
        };
        let record = match disk.into_record() {
            Ok(record) if record.key == candidate.filename_key => record,
            _ => {
                return DecodedRecord::Malformed {
                    path: candidate.path.clone(),
                };
            }
        };
        if is_expired(record.checkpointed_at, now, self.limits.max_age) {
            return DecodedRecord::Expired {
                path: candidate.path.clone(),
            };
        }
        let key = record.key.clone();
        let checkpointed_at = record.checkpointed_at;
        let recovered = matches!(purpose, ScanPurpose::Recovery).then(|| RecoveredRecord {
            source_conflicted: record.metadata.source_path.as_ref().is_some_and(|path| {
                crate::fs::recovery_source_matches(
                    path,
                    &record.metadata.original_stamp,
                    &record.metadata.source_identity,
                )
                .map_or(true, |matches| !matches)
            }),
            record,
        });
        DecodedRecord::Stored(Box::new(StoredRecord {
            path: candidate.path.clone(),
            bytes: candidate.bytes,
            key,
            checkpointed_at,
            recovered,
        }))
    }

    fn prepare_atomic_write(
        &self,
        ciphertext: &[u8],
    ) -> Result<tempfile::NamedTempFile, RecoveryError> {
        let mut temp = tempfile::Builder::new()
            .prefix(".markturbo-recovery-")
            .tempfile_in(&self.root)?;
        temp.write_all(ciphertext)?;
        temp.as_file_mut().sync_all()?;
        Ok(temp)
    }

    fn write_retirement_marker(
        &self,
        marker: &Path,
        keys: &[RecoveryKey],
    ) -> Result<(), RecoveryError> {
        let contents = RetirementMarker {
            version: RETIREMENT_MARKER_VERSION,
            keys: keys.iter().map(|key| key.0.clone()).collect(),
        };
        let bytes = serde_json::to_vec(&contents)?;
        let temp = self.prepare_atomic_write(&bytes)?;
        match self.persist_prepared_write_noclobber(temp, marker) {
            Ok(()) => Ok(()),
            Err(RecoveryError::Io(error)) if error.kind() == io::ErrorKind::AlreadyExists => {
                let mut file = fs::OpenOptions::new().read(true).write(true).open(marker)?;
                let existing_keys = decode_retirement_marker(&mut file)?;
                if existing_keys.as_slice() != keys {
                    return Err(RecoveryError::Unavailable(
                        "recovery retirement marker keys do not match",
                    ));
                }
                file.sync_all()?;
                Ok(())
            }
            Err(error) => Err(error),
        }
    }

    fn persist_prepared_write(
        &self,
        temp: tempfile::NamedTempFile,
        path: &Path,
    ) -> Result<(), RecoveryError> {
        #[cfg(test)]
        if self.failures.persist.swap(false, Ordering::SeqCst) {
            return Err(io::Error::other("injected recovery checkpoint persist failure").into());
        }
        let file = temp
            .persist(path)
            .map_err(|error| RecoveryError::Io(error.error))?;
        file.sync_all()?;
        Ok(())
    }

    fn persist_prepared_write_noclobber(
        &self,
        temp: tempfile::NamedTempFile,
        path: &Path,
    ) -> Result<(), RecoveryError> {
        #[cfg(test)]
        if self.failures.persist.swap(false, Ordering::SeqCst) {
            return Err(io::Error::other("injected recovery checkpoint persist failure").into());
        }
        let file = temp
            .persist_noclobber(path)
            .map_err(|error| RecoveryError::Io(error.error))?;
        #[cfg(test)]
        if self
            .failures
            .retirement_marker_sync
            .swap(false, Ordering::SeqCst)
        {
            return Err(
                io::Error::other("injected recovery retirement marker sync failure").into(),
            );
        }
        file.sync_all()?;
        Ok(())
    }

    fn rename_path(&self, from: &Path, to: &Path) -> io::Result<()> {
        #[cfg(test)]
        if self.failures.rename.swap(false, Ordering::SeqCst) {
            return Err(io::Error::other("injected recovery rename failure"));
        }
        fs::rename(from, to)
    }

    fn remove_path(&self, path: &Path) -> io::Result<()> {
        #[cfg(test)]
        if self.failures.delete.load(Ordering::SeqCst)
            && self.path_is_present(path)?
            && self.failures.delete.swap(false, Ordering::SeqCst)
        {
            return Err(io::Error::other("injected recovery delete failure"));
        }
        fs::remove_file(path)
    }

    fn path_is_present(&self, path: &Path) -> io::Result<bool> {
        #[cfg(test)]
        {
            let mut injected = self.failures.presence.lock().unwrap();
            if injected.as_ref().is_some_and(|injected| injected == path) {
                injected.take();
                return Err(io::Error::new(
                    io::ErrorKind::PermissionDenied,
                    "injected recovery presence failure",
                ));
            }
        }
        match fs::symlink_metadata(path) {
            Ok(_) => Ok(true),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
            Err(error) => Err(error),
        }
    }

    #[cfg(test)]
    fn atomic_write(&self, path: &Path, ciphertext: &[u8]) -> Result<(), RecoveryError> {
        let temp = self.prepare_atomic_write(ciphertext)?;
        self.persist_prepared_write(temp, path)
    }

    #[cfg(test)]
    pub(crate) fn fail_next_persist_for_test(&self) {
        self.failures.persist.store(true, Ordering::SeqCst);
    }

    #[cfg(test)]
    fn fail_next_retirement_marker_sync_for_test(&self) {
        self.failures
            .retirement_marker_sync
            .store(true, Ordering::SeqCst);
    }

    #[cfg(test)]
    pub(crate) fn fail_next_rename_for_test(&self) {
        self.failures.rename.store(true, Ordering::SeqCst);
    }

    #[cfg(test)]
    pub(crate) fn fail_next_delete_for_test(&self) {
        self.failures.delete.store(true, Ordering::SeqCst);
    }

    #[cfg(test)]
    fn fail_next_presence_for_test(&self, path: PathBuf) {
        *self.failures.presence.lock().unwrap() = Some(path);
    }

    #[cfg(test)]
    fn pause_after_checkpoint_prepare_for_test(
        &self,
    ) -> (std::sync::mpsc::Receiver<()>, std::sync::mpsc::Sender<()>) {
        self.install_test_pause(&self.failures.after_checkpoint_prepare)
    }

    #[cfg(test)]
    pub(crate) fn pause_after_checkpoint_final_check_for_test(
        &self,
    ) -> (std::sync::mpsc::Receiver<()>, std::sync::mpsc::Sender<()>) {
        self.install_test_pause(&self.failures.after_checkpoint_final_check)
    }

    #[cfg(test)]
    fn pause_after_retirement_marker_for_test(
        &self,
    ) -> (std::sync::mpsc::Receiver<()>, std::sync::mpsc::Sender<()>) {
        self.install_test_pause(&self.failures.after_retirement_marker)
    }

    #[cfg(test)]
    pub(crate) fn pause_before_transaction_journal_for_test(
        &self,
    ) -> (std::sync::mpsc::Receiver<()>, std::sync::mpsc::Sender<()>) {
        self.install_test_pause(&self.failures.before_transaction_journal)
    }

    #[cfg(test)]
    fn pause_after_transaction_journal_for_test(
        &self,
    ) -> (std::sync::mpsc::Receiver<()>, std::sync::mpsc::Sender<()>) {
        self.install_test_pause(&self.failures.after_transaction_journal)
    }

    #[cfg(test)]
    pub(crate) fn pause_after_eviction_reservation_for_test(
        &self,
    ) -> (std::sync::mpsc::Receiver<()>, std::sync::mpsc::Sender<()>) {
        self.install_test_pause(&self.failures.after_eviction_reservation)
    }

    #[cfg(test)]
    fn install_test_pause(
        &self,
        slot: &Mutex<Option<TestPause>>,
    ) -> (std::sync::mpsc::Receiver<()>, std::sync::mpsc::Sender<()>) {
        let (started_sender, started_receiver) = std::sync::mpsc::channel();
        let (release_sender, release_receiver) = std::sync::mpsc::channel();
        let mut slot = slot.lock().unwrap();
        assert!(slot.is_none(), "a recovery test pause is already installed");
        *slot = Some(TestPause {
            started: started_sender,
            release: Mutex::new(release_receiver),
        });
        (started_receiver, release_sender)
    }

    #[cfg(test)]
    fn pause_checkpoint_write_for_test(&self, slot: &Mutex<Option<TestPause>>) {
        let slot = slot.lock().unwrap();
        let Some(pause) = slot.as_ref() else {
            return;
        };
        pause.started.send(()).unwrap();
        pause.release.lock().unwrap().recv().unwrap();
    }

    #[cfg(test)]
    fn pause_once_for_test(&self, slot: &Mutex<Option<TestPause>>) {
        let Some(pause) = slot.lock().unwrap().take() else {
            return;
        };
        pause.started.send(()).unwrap();
        pause.release.lock().unwrap().recv().unwrap();
    }

    #[cfg(test)]
    pub(crate) fn retention_scan_count_for_test(&self) -> usize {
        self.failures.retention_scans.load(Ordering::SeqCst)
    }

    #[cfg(test)]
    pub(crate) fn checkpoint_batch_count_for_test(&self) -> usize {
        self.failures.checkpoint_batches.load(Ordering::SeqCst)
    }

    fn record_path(&self, key: &RecoveryKey) -> PathBuf {
        self.root
            .join(format!("{}.{}", key.as_str(), RECORD_EXTENSION))
    }

    fn retirement_marker_path(&self, keys: &[RecoveryKey]) -> PathBuf {
        let mut hasher = Sha256::new();
        hasher.update(b"markturbo-recovery-retirement-marker-v1\0");
        for key in keys {
            hasher.update(key.as_str().as_bytes());
            hasher.update([0]);
        }
        self.root
            .join(format!("{RETIREMENT_MARKER_PREFIX}{:x}", hasher.finalize()))
    }

    #[cfg(test)]
    fn retirement_marker_path_for_key(&self, key: &RecoveryKey) -> PathBuf {
        self.retirement_marker_path(std::slice::from_ref(key))
    }

    fn transaction_journal_path(&self) -> PathBuf {
        self.root.join(TRANSACTION_JOURNAL_NAME)
    }

    fn transaction_commit_path(&self) -> PathBuf {
        self.root.join(TRANSACTION_COMMIT_NAME)
    }

    fn next_artifact_path(
        &self,
        kind: &str,
        state: &mut MutationState,
    ) -> Result<PathBuf, RecoveryError> {
        loop {
            let id = state.next_artifact;
            state.next_artifact = state.next_artifact.wrapping_add(1);
            let path = self.root.join(format!("{ARTIFACT_PREFIX}{kind}-{id:016x}"));
            if !self.path_is_present(&path)? {
                return Ok(path);
            }
        }
    }

    fn verify_production_root(&self) -> Result<(), RecoveryError> {
        #[cfg(windows)]
        if let Some(root_guard) = &self.root_guard {
            root_guard.verify()?;
        }
        Ok(())
    }

    fn checkpoint_capability(
        &self,
        key: &RecoveryKey,
        token: Option<&RecoveryToken>,
    ) -> CheckpointCapability {
        let state = self.lock_capabilities();
        if token.is_some_and(|token| {
            token.key != *key
                || token.generation != state.generations.get(key).copied().unwrap_or_default()
        }) {
            return CheckpointCapability::Superseded;
        }
        if state.retiring.contains_key(key) {
            return CheckpointCapability::Deferred;
        }
        CheckpointCapability::Current {
            active_keys: state.active_keys.clone(),
        }
    }

    fn clear_eviction_reservations(&self, keys: &[RecoveryKey]) {
        let mut state = self.lock_capabilities();
        for key in keys {
            state.evicting_keys.remove(key);
        }
    }

    fn clear_all_eviction_reservations(&self) {
        self.lock_capabilities().evicting_keys.clear();
    }

    fn retirements_are_current(&self, batch: &RecoveryRetirementBatch) -> bool {
        let state = self.lock_capabilities();
        batch
            .keys
            .iter()
            .all(|key| state.retiring.get(key) == Some(&RetirementState::Pending(batch.id)))
    }

    fn clear_persisting_retirements(&self, batch: &RecoveryRetirementBatch) {
        let mut state = self.lock_capabilities();
        for key in &batch.keys {
            if state.retiring.get(key) == Some(&RetirementState::Persisting(batch.id)) {
                state.retiring.remove(key);
            }
        }
    }

    fn finish_retirements(&self, batch: &RecoveryRetirementBatch) -> bool {
        let mut state = self.lock_capabilities();
        if !batch
            .keys
            .iter()
            .all(|key| state.retiring.get(key) == Some(&RetirementState::Pending(batch.id)))
        {
            return false;
        }
        for key in &batch.keys {
            state.retiring.remove(key);
        }
        true
    }

    fn lock_retirement_markers(&self) -> MutexGuard<'_, ()> {
        self.retirement_marker_lock
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn lock_capabilities(&self) -> MutexGuard<'_, CapabilityState> {
        self.capability_lock
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn lock_mutations(&self) -> MutexGuard<'_, MutationState> {
        // A failed worker must not turn future checkpoints into an editor-side
        // panic. There is no in-progress write while a poisoned guard is held.
        self.mutation_lock
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

fn remove_if_exists(store: &RecoveryStore, path: &Path) -> Result<(), RecoveryError> {
    match store.remove_path(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.into()),
    }
}

fn recovery_filename(path: &Path) -> Result<String, RecoveryError> {
    path.file_name()
        .and_then(|name| name.to_str())
        .filter(|name| !name.is_empty() && !name.contains(['/', '\\']))
        .map(str::to_owned)
        .ok_or(RecoveryError::Unavailable(
            "recovery transaction filename is invalid",
        ))
}

fn is_transaction_artifact_name(name: &str, role: &str) -> bool {
    let Some(suffix) = name
        .strip_prefix(ARTIFACT_PREFIX)
        .and_then(|name| name.strip_prefix(role))
        .and_then(|name| name.strip_prefix('-'))
    else {
        return false;
    };
    suffix.len() == 16
        && suffix
            .bytes()
            .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
}

fn is_record_filename(name: &str) -> bool {
    record_key_from_path(Path::new(name)).is_some()
}

fn is_retired_path(path: &Path) -> bool {
    path.file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| is_transaction_artifact_name(name, "retired"))
}

fn is_retirement_marker_path(path: &Path) -> bool {
    path.file_name()
        .and_then(|name| name.to_str())
        .and_then(|name| name.strip_prefix(RETIREMENT_MARKER_PREFIX))
        .is_some_and(|suffix| {
            suffix.len() == 64
                && suffix
                    .bytes()
                    .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
        })
}

fn decode_retirement_marker(reader: impl io::Read) -> Result<Vec<RecoveryKey>, RecoveryError> {
    let marker: RetirementMarker = serde_json::from_reader(reader)?;
    if marker.version != RETIREMENT_MARKER_VERSION {
        return Err(RecoveryError::Unavailable(
            "recovery retirement marker is unsupported",
        ));
    }
    normalized_retirement_keys(marker.keys.into_iter().map(RecoveryKey))
}

fn normalized_retirement_keys(
    keys: impl IntoIterator<Item = RecoveryKey>,
) -> Result<Vec<RecoveryKey>, RecoveryError> {
    let mut keys: Vec<_> = keys.into_iter().collect();
    keys.sort();
    keys.dedup();
    if keys.is_empty() || keys.iter().any(|key| !key.is_valid()) {
        return Err(RecoveryError::Unavailable(
            "recovery retirement keys are invalid",
        ));
    }
    Ok(keys)
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct RecoveryTransaction {
    version: u8,
    target_name: String,
    new_staged_name: String,
    staged: Vec<RecoveryTransactionEntry>,
}

impl RecoveryTransaction {
    fn validate(&self) -> Result<(), RecoveryError> {
        if self.version != TRANSACTION_VERSION
            || !is_record_filename(&self.target_name)
            || !is_transaction_artifact_name(&self.new_staged_name, "new")
            || self.staged.is_empty()
        {
            return Err(RecoveryError::Unavailable(
                "recovery transaction journal is unsupported",
            ));
        }
        let mut original_names = HashSet::new();
        let mut staged_names = HashSet::new();
        for entry in &self.staged {
            if !is_record_filename(&entry.original_name)
                || !is_transaction_artifact_name(&entry.staged_name, "stage")
                || !original_names.insert(&entry.original_name)
                || !staged_names.insert(&entry.staged_name)
            {
                return Err(RecoveryError::Unavailable(
                    "recovery transaction journal is unsupported",
                ));
            }
        }
        if staged_names.contains(&self.new_staged_name) {
            return Err(RecoveryError::Unavailable(
                "recovery transaction journal is unsupported",
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct RecoveryTransactionEntry {
    original_name: String,
    staged_name: String,
}

enum TransactionDisposition {
    Commit { inferred: bool },
    Rollback,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ScanPurpose {
    Recovery,
    Retention,
}

#[derive(Default)]
struct ScanState {
    records: Vec<StoredRecord>,
    marked_keys: HashSet<RecoveryKey>,
    // A current batch can replace these canonical records atomically, so it
    // accounts for their quota without decrypting stale ciphertext first.
    // They are deliberately non-evictable until the batch either replaces
    // them or falls back to a full validation scan.
    skipped_replacements: Vec<SkippedRetentionDescriptor>,
    expired_paths: Vec<PathBuf>,
    invalid_paths: Vec<PathBuf>,
    // Every regular record and undeleted data artifact with readable metadata
    // reserves quota. This prevents failed cleanup or unreadable records from
    // making capacity appear free.
    known_record_count: usize,
    known_bytes: u64,
    known_record_sizes: HashMap<PathBuf, u64>,
    // An entry whose metadata cannot be read has an unknown size, so proceeding
    // would risk crossing the on-disk quota.
    has_metadata_unknown: bool,
    issues: Vec<RecoveryIssue>,
}

struct RecordCandidate {
    path: PathBuf,
    bytes: u64,
    filename_key: RecoveryKey,
}

struct SkippedRetentionDescriptor {
    path: PathBuf,
    bytes: u64,
    key: RecoveryKey,
}

enum DecodedRecord {
    Unreadable { path: PathBuf },
    Malformed { path: PathBuf },
    Expired { path: PathBuf },
    Stored(Box<StoredRecord>),
}

enum CancellableScan {
    Completed(Box<ScanState>),
    Cancelled,
}

impl ScanState {
    fn apply_pending_cleanup(&mut self, cleanup: PendingCleanup) -> Vec<RecoveryIssue> {
        for artifact in cleanup.quota_artifacts {
            let Some(bytes) = artifact.bytes else {
                self.has_metadata_unknown = true;
                continue;
            };
            if self
                .known_record_sizes
                .insert(artifact.path, bytes)
                .is_none()
            {
                self.known_record_count += 1;
                self.known_bytes = self.known_bytes.saturating_add(bytes);
            }
        }
        cleanup.issues
    }

    fn forget_paths(&mut self, paths: &[PathBuf]) {
        for path in paths {
            if let Some(bytes) = self.known_record_sizes.remove(path) {
                self.known_record_count = self.known_record_count.saturating_sub(1);
                self.known_bytes = self.known_bytes.saturating_sub(bytes);
            }
        }
    }

    fn forget_paths_and_records(&mut self, paths: &[PathBuf]) {
        self.forget_paths(paths);
        self.records
            .retain(|record| !paths.iter().any(|path| path == &record.path));
        self.skipped_replacements
            .retain(|record| !paths.iter().any(|path| path == &record.path));
        self.expired_paths
            .retain(|record_path| !paths.iter().any(|path| path == record_path));
        self.invalid_paths
            .retain(|record_path| !paths.iter().any(|path| path == record_path));
    }

    fn insert_checkpoint(&mut self, record: StoredRecord) {
        let path = record.path.clone();
        self.forget_paths_and_records(std::slice::from_ref(&path));
        self.known_record_count += 1;
        self.known_bytes = self.known_bytes.saturating_add(record.bytes);
        self.known_record_sizes.insert(path, record.bytes);
        self.records.push(record);
        debug_assert!(self.skipped_replacements.iter().all(|record| {
            record.key.is_valid()
                && self.known_record_sizes.get(&record.path).copied() == Some(record.bytes)
        }));
    }
}

struct PrunedRetention {
    maintenance: RecoveryMaintenance,
    scan: ScanState,
    transaction_cleanup_pending: bool,
}

enum CancellablePrunedRetention {
    Completed(Box<PrunedRetention>),
    Cancelled,
}

enum CheckpointWriteResult {
    Written,
    Superseded,
    Deferred,
}

struct CheckpointWriteContext<'a> {
    active_keys: &'a HashSet<RecoveryKey>,
    now: SystemTime,
    state: &'a mut MutationState,
    scan: &'a mut ScanState,
    maintenance: &'a mut RecoveryMaintenance,
    transaction_cleanup_pending: &'a mut bool,
    token: Option<&'a RecoveryToken>,
    cancelled: &'a AtomicBool,
}

struct CheckpointTransactionPlan<'a> {
    target_path: &'a Path,
    ciphertext: &'a [u8],
    paths_to_stage: &'a [PathBuf],
    eviction_keys: &'a [RecoveryKey],
}

enum CheckpointWriteError {
    Certain(RecoveryError),
    Uncertain(RecoveryError),
}

impl CheckpointWriteError {
    fn into_error(self) -> RecoveryError {
        match self {
            Self::Certain(error) | Self::Uncertain(error) => error,
        }
    }
}

impl From<RecoveryError> for CheckpointWriteError {
    fn from(error: RecoveryError) -> Self {
        Self::Certain(error)
    }
}

struct StoredRecord {
    path: PathBuf,
    bytes: u64,
    key: RecoveryKey,
    checkpointed_at: SystemTime,
    // Retention only needs the descriptor. Recovery scans retain the payload
    // after validating it so callers can restore exact user text and metadata.
    recovered: Option<RecoveredRecord>,
}

fn checkpoint_batch_failure_for_cancellable_attempts(
    attempts: &[CancellableRecoveryCheckpointAttempt<'_>],
    error: RecoveryError,
) -> CheckpointBatchReceipt {
    let mut error = Some(error);
    let outcomes = attempts
        .iter()
        .map(|attempt| {
            if recovery_cancelled(attempt.cancelled) {
                CheckpointBatchOutcome::Superseded
            } else if let Some(error) = error.take() {
                CheckpointBatchOutcome::Failed(error)
            } else {
                CheckpointBatchOutcome::Deferred
            }
        })
        .collect();
    CheckpointBatchReceipt {
        outcomes,
        maintenance: RecoveryMaintenance::default(),
    }
}

fn checkpoint_batch_superseded(attempt_count: usize) -> CheckpointBatchReceipt {
    CheckpointBatchReceipt {
        outcomes: (0..attempt_count)
            .map(|_| CheckpointBatchOutcome::Superseded)
            .collect(),
        maintenance: RecoveryMaintenance::default(),
    }
}

fn checkpoint_batch_supersede_pending(outcomes: &mut [Option<CheckpointBatchOutcome>]) {
    for outcome in outcomes {
        if outcome.is_none() {
            *outcome = Some(CheckpointBatchOutcome::Superseded);
        }
    }
}

fn checkpoint_batch_defer_pending(
    outcomes: &mut [Option<CheckpointBatchOutcome>],
    current: &[CurrentRecoveryCheckpointAttempt<'_>],
) {
    for attempt in current {
        if outcomes[attempt.index].is_none() {
            outcomes[attempt.index] = Some(if recovery_cancelled(attempt.cancelled) {
                CheckpointBatchOutcome::Superseded
            } else {
                CheckpointBatchOutcome::Deferred
            });
        }
    }
}

fn checkpoint_batch_fail_pending(
    outcomes: &mut [Option<CheckpointBatchOutcome>],
    current: &[CurrentRecoveryCheckpointAttempt<'_>],
    error: RecoveryError,
) {
    let mut error = Some(error);
    for attempt in current {
        if outcomes[attempt.index].is_none() {
            outcomes[attempt.index] = Some(if recovery_cancelled(attempt.cancelled) {
                CheckpointBatchOutcome::Superseded
            } else if let Some(error) = error.take() {
                CheckpointBatchOutcome::Failed(error)
            } else {
                CheckpointBatchOutcome::Deferred
            });
        }
    }
}

fn checkpoint_batch_receipt(
    outcomes: Vec<Option<CheckpointBatchOutcome>>,
    maintenance: RecoveryMaintenance,
) -> CheckpointBatchReceipt {
    CheckpointBatchReceipt {
        outcomes: outcomes
            .into_iter()
            .map(|outcome| outcome.expect("every batch attempt must have an outcome"))
            .collect(),
        maintenance,
    }
}

fn recovery_cancelled(cancelled: &AtomicBool) -> bool {
    cancelled.load(Ordering::Acquire)
}

fn current_attempts_cancelled(current: &[CurrentRecoveryCheckpointAttempt<'_>]) -> bool {
    current
        .iter()
        .all(|attempt| recovery_cancelled(attempt.cancelled))
}

fn join_recovery_worker<R>(worker: std::thread::ScopedJoinHandle<'_, R>) -> R {
    match worker.join() {
        Ok(result) => result,
        Err(payload) => std::panic::resume_unwind(payload),
    }
}

fn recovery_worker_limit() -> usize {
    std::thread::available_parallelism()
        .map(|available| recovery_worker_limit_for(available.get()))
        .unwrap_or(1)
}

fn recovery_worker_limit_for(available_parallelism: usize) -> usize {
    available_parallelism.clamp(1, MAX_PARALLEL_RECOVERY_WORKERS)
}

fn recovery_wave_ranges(input_bytes: &[u64], max_total_bytes: u64) -> Vec<std::ops::Range<usize>> {
    recovery_wave_ranges_with_worker_limit(input_bytes, max_total_bytes, recovery_worker_limit())
}

fn recovery_wave_ranges_with_worker_limit(
    input_bytes: &[u64],
    max_total_bytes: u64,
    worker_limit: usize,
) -> Vec<std::ops::Range<usize>> {
    let worker_limit = worker_limit.max(1);
    let mut ranges = Vec::new();
    let mut start = 0;
    let mut total_bytes: u64 = 0;

    for (index, bytes) in input_bytes.iter().copied().enumerate() {
        let worker_limit_reached = index - start == worker_limit;
        let byte_limit_reached = total_bytes.saturating_add(bytes) > max_total_bytes;
        if index > start && (worker_limit_reached || byte_limit_reached) {
            ranges.push(start..index);
            start = index;
            total_bytes = 0;
        }
        total_bytes = total_bytes.saturating_add(bytes);
    }
    if start < input_bytes.len() {
        ranges.push(start..input_bytes.len());
    }
    ranges
}

fn checkpoint_wave_input_bytes(checkpoint: &RecoveryCheckpoint) -> u64 {
    let mut bytes = checkpoint.text.len().try_into().unwrap_or(u64::MAX);
    bytes = bytes.saturating_add(checkpoint.key.0.len().try_into().unwrap_or(u64::MAX));
    bytes = bytes.saturating_add(
        checkpoint
            .metadata
            .encoding_name
            .len()
            .try_into()
            .unwrap_or(u64::MAX),
    );
    if let Some(path) = &checkpoint.metadata.source_path {
        bytes = bytes.saturating_add(path_wave_input_bytes(path));
    }
    if let SourceIdentity::SymbolicLink {
        link_target,
        resolved_target,
    } = &checkpoint.metadata.source_identity
    {
        bytes = bytes.saturating_add(path_wave_input_bytes(link_target));
        bytes = bytes.saturating_add(path_wave_input_bytes(resolved_target));
    }
    bytes.saturating_add(CHECKPOINT_WAVE_METADATA_OVERHEAD_BYTES)
}

fn path_wave_input_bytes(path: &Path) -> u64 {
    path.to_string_lossy().len().try_into().unwrap_or(u64::MAX)
}

fn run_recovery_wave<T, R>(items: &[T], work: impl Fn(&T) -> R + Sync) -> Vec<R>
where
    T: Sync,
    R: Send,
{
    std::thread::scope(|scope| {
        let work = &work;
        let workers: Vec<_> = items
            .iter()
            .map(|item| scope.spawn(move || work(item)))
            .collect();
        workers.into_iter().map(join_recovery_worker).collect()
    })
}

fn oldest_inactive_index(
    records: &[&StoredRecord],
    active_keys: &HashSet<RecoveryKey>,
) -> Option<usize> {
    records
        .iter()
        .enumerate()
        .filter(|(_, record)| !active_keys.contains(&record.key))
        .min_by_key(|(_, record)| (record.checkpointed_at, &record.key))
        .map(|(index, _)| index)
}

fn record_key_from_path(path: &Path) -> Option<RecoveryKey> {
    let name = path.file_name()?.to_str()?;
    let (key, extension) = name.rsplit_once('.')?;
    (extension == RECORD_EXTENSION)
        .then_some(RecoveryKey(key.to_owned()))
        .filter(RecoveryKey::is_valid)
}

fn is_expired(checkpointed_at: SystemTime, now: SystemTime, max_age: Duration) -> bool {
    now.duration_since(checkpointed_at)
        .is_ok_and(|age| age >= max_age)
}

#[derive(Debug, Serialize, Deserialize)]
struct DiskRecord {
    version: u8,
    key: String,
    checkpointed_at: DiskTimestamp,
    text: String,
    metadata: DiskMetadata,
}

impl DiskRecord {
    fn from_checkpoint(
        checkpoint: &RecoveryCheckpoint,
        checkpointed_at: SystemTime,
    ) -> Result<Self, RecoveryError> {
        Ok(Self {
            version: RECORD_VERSION,
            key: checkpoint.key.0.clone(),
            checkpointed_at: DiskTimestamp::from_system_time(checkpointed_at)?,
            text: checkpoint.text.clone(),
            metadata: DiskMetadata::from_metadata(&checkpoint.metadata)?,
        })
    }

    fn into_record(self) -> Result<RecoveryRecord, RecoveryError> {
        let key = RecoveryKey(self.key);
        if self.version != RECORD_VERSION || !key.is_valid() {
            return Err(RecoveryError::Unavailable("unsupported recovery record"));
        }
        Ok(RecoveryRecord {
            key,
            text: self.text,
            metadata: self.metadata.into_metadata()?,
            checkpointed_at: self.checkpointed_at.into_system_time()?,
        })
    }
}

#[derive(Debug, Serialize, Deserialize)]
struct DiskMetadata {
    source_path: Option<PathBuf>,
    encoding_name: String,
    had_bom: bool,
    newline: DiskNewline,
    original_stamp: DiskFileStamp,
    source_identity: DiskSourceIdentity,
    decode_had_errors: bool,
}

impl DiskMetadata {
    fn from_metadata(metadata: &RecoveryMetadata) -> Result<Self, RecoveryError> {
        Ok(Self {
            source_path: metadata.source_path.clone(),
            encoding_name: metadata.encoding_name.clone(),
            had_bom: metadata.had_bom,
            newline: metadata.newline.into(),
            original_stamp: DiskFileStamp::from_stamp(&metadata.original_stamp)?,
            source_identity: metadata.source_identity.clone().into(),
            decode_had_errors: metadata.decode_had_errors,
        })
    }

    fn into_metadata(self) -> Result<RecoveryMetadata, RecoveryError> {
        Ok(RecoveryMetadata {
            source_path: self.source_path,
            encoding_name: self.encoding_name,
            had_bom: self.had_bom,
            newline: self.newline.into(),
            original_stamp: self.original_stamp.into_stamp()?,
            source_identity: self.source_identity.into(),
            decode_had_errors: self.decode_had_errors,
        })
    }
}

#[derive(Debug, Serialize, Deserialize)]
enum DiskNewline {
    Lf,
    Crlf,
}

impl From<Newline> for DiskNewline {
    fn from(value: Newline) -> Self {
        match value {
            Newline::Lf => Self::Lf,
            Newline::Crlf => Self::Crlf,
        }
    }
}

impl From<DiskNewline> for Newline {
    fn from(value: DiskNewline) -> Self {
        match value {
            DiskNewline::Lf => Self::Lf,
            DiskNewline::Crlf => Self::Crlf,
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
struct DiskFileStamp {
    modified: Option<DiskTimestamp>,
    len: u64,
    digest: [u8; 32],
    #[serde(default, skip_serializing_if = "Option::is_none")]
    object_id: Option<FileObjectId>,
}

impl DiskFileStamp {
    fn from_stamp(stamp: &FileStamp) -> Result<Self, RecoveryError> {
        Ok(Self {
            modified: stamp
                .modified
                .map(DiskTimestamp::from_system_time)
                .transpose()?,
            len: stamp.len,
            digest: stamp.digest,
            object_id: stamp.object_id,
        })
    }

    fn into_stamp(self) -> Result<FileStamp, RecoveryError> {
        Ok(FileStamp {
            modified: self
                .modified
                .map(DiskTimestamp::into_system_time)
                .transpose()?,
            len: self.len,
            digest: self.digest,
            object_id: self.object_id,
        })
    }
}

#[derive(Debug, Serialize, Deserialize)]
enum DiskSourceIdentity {
    Regular,
    SymbolicLink {
        link_target: PathBuf,
        resolved_target: PathBuf,
    },
}

impl From<SourceIdentity> for DiskSourceIdentity {
    fn from(value: SourceIdentity) -> Self {
        match value {
            SourceIdentity::Regular => Self::Regular,
            SourceIdentity::SymbolicLink {
                link_target,
                resolved_target,
            } => Self::SymbolicLink {
                link_target,
                resolved_target,
            },
        }
    }
}

impl From<DiskSourceIdentity> for SourceIdentity {
    fn from(value: DiskSourceIdentity) -> Self {
        match value {
            DiskSourceIdentity::Regular => Self::Regular,
            DiskSourceIdentity::SymbolicLink {
                link_target,
                resolved_target,
            } => Self::SymbolicLink {
                link_target,
                resolved_target,
            },
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
struct DiskTimestamp {
    seconds: i64,
    nanos: u32,
}

impl DiskTimestamp {
    fn from_system_time(time: SystemTime) -> Result<Self, RecoveryError> {
        match time.duration_since(UNIX_EPOCH) {
            Ok(duration) => Ok(Self {
                seconds: i64::try_from(duration.as_secs())
                    .map_err(|_| RecoveryError::InvalidTimestamp)?,
                nanos: duration.subsec_nanos(),
            }),
            Err(error) => {
                let duration = error.duration();
                let seconds = i64::try_from(duration.as_secs())
                    .map_err(|_| RecoveryError::InvalidTimestamp)?;
                if duration.subsec_nanos() == 0 {
                    Ok(Self {
                        seconds: -seconds,
                        nanos: 0,
                    })
                } else {
                    Ok(Self {
                        seconds: -seconds
                            .checked_add(1)
                            .ok_or(RecoveryError::InvalidTimestamp)?,
                        nanos: 1_000_000_000 - duration.subsec_nanos(),
                    })
                }
            }
        }
    }

    fn into_system_time(self) -> Result<SystemTime, RecoveryError> {
        if self.nanos >= 1_000_000_000 {
            return Err(RecoveryError::InvalidTimestamp);
        }
        if self.seconds >= 0 {
            return UNIX_EPOCH
                .checked_add(Duration::new(self.seconds as u64, self.nanos))
                .ok_or(RecoveryError::InvalidTimestamp);
        }
        let seconds = self
            .seconds
            .checked_neg()
            .ok_or(RecoveryError::InvalidTimestamp)? as u64;
        let duration = if self.nanos == 0 {
            Duration::new(seconds, 0)
        } else {
            Duration::new(
                seconds
                    .checked_sub(1)
                    .ok_or(RecoveryError::InvalidTimestamp)?,
                1_000_000_000 - self.nanos,
            )
        };
        UNIX_EPOCH
            .checked_sub(duration)
            .ok_or(RecoveryError::InvalidTimestamp)
    }
}

/// Immutable timing contract for one dispatched recovery checkpoint.
///
/// A caller keeps this with its in-flight snapshot, then reports the outcome
/// with one of the corresponding [`CheckpointSchedule`] methods. The durable
/// deadline is intentionally based on dispatch rather than completion: a slow
/// write must not postpone the next recovery cadence.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CheckpointAttemptTiming {
    pub snapshot_at: Instant,
    pub oldest_covered_edit: Option<Instant>,
    pub durable_complete_by: Instant,
    previous_dispatch: Option<Instant>,
}

/// Pure timing state for recovery checkpoints.
#[derive(Debug, Default)]
pub struct CheckpointSchedule {
    dirty: bool,
    oldest_uncovered_edit: Option<Instant>,
    last_dispatch: Option<Instant>,
    failure_retry_not_before: Option<Instant>,
    fresh_commit_budget_on_next_dispatch: bool,
}

impl CheckpointSchedule {
    pub fn mark_dirty(&mut self, now: Instant) {
        self.dirty = true;
        self.oldest_uncovered_edit.get_or_insert(now);
    }

    pub fn mark_clean(&mut self) {
        *self = Self::default();
    }

    /// Start periodic refresh timing from a checkpoint that is already durable.
    pub fn mark_durable_baseline(&mut self, now: Instant) {
        self.dirty = true;
        self.oldest_uncovered_edit = None;
        self.last_dispatch = Some(now);
        self.failure_retry_not_before = None;
        self.fresh_commit_budget_on_next_dispatch = false;
    }

    /// Capture the coverage boundary immediately before the caller snapshots
    /// text for background persistence.
    pub fn checkpoint_dispatched(&mut self, now: Instant) -> Option<CheckpointAttemptTiming> {
        if !self.dirty {
            return None;
        }
        let oldest_covered_edit = self.oldest_uncovered_edit.take();
        let durable_complete_by = if self.fresh_commit_budget_on_next_dispatch {
            now + CHECKPOINT_COMMIT_BUDGET
        } else {
            oldest_covered_edit.map_or(now + CHECKPOINT_COMMIT_BUDGET, |edit| {
                (edit + MAX_LOSS_WINDOW).min(now + CHECKPOINT_COMMIT_BUDGET)
            })
        };
        let timing = CheckpointAttemptTiming {
            snapshot_at: now,
            oldest_covered_edit,
            durable_complete_by,
            previous_dispatch: self.last_dispatch,
        };
        self.last_dispatch = Some(now);
        self.failure_retry_not_before = None;
        self.fresh_commit_budget_on_next_dispatch = false;
        Some(timing)
    }

    /// Record a durable write for exactly the work covered by `timing`.
    pub fn checkpoint_written(&mut self, _timing: CheckpointAttemptTiming) {
        self.failure_retry_not_before = None;
    }

    /// Restore covered edits after a stale result. This is due immediately if
    /// its original deadline has already passed.
    pub fn checkpoint_superseded(&mut self, timing: CheckpointAttemptTiming) {
        self.restore_coverage(timing);
        self.last_dispatch = timing.previous_dispatch;
        self.failure_retry_not_before = None;
    }

    /// Restore covered edits after a failed write, but delay the retry enough
    /// to prevent a timer-driven busy loop.
    pub fn checkpoint_failed(&mut self, timing: CheckpointAttemptTiming, now: Instant) {
        self.restore_coverage(timing);
        self.last_dispatch = timing.previous_dispatch;
        self.failure_retry_not_before = Some(now + CHECKPOINT_RETRY_DELAY);
    }

    /// Restore covered edits after cancellation without acknowledging a
    /// durable checkpoint. Cancellation shares the failure retry throttle so
    /// a timer cannot immediately dispatch another cancelled attempt.
    pub fn checkpoint_cancelled(&mut self, timing: CheckpointAttemptTiming, now: Instant) {
        self.checkpoint_failed(timing, now);
    }

    /// Retry after the product loss-window deadline has already been missed.
    ///
    /// The uncovered edits remain pending, but their original deadline cannot
    /// be reused: it is already in the past and would cancel every retry before
    /// it could commit. The next dispatch therefore receives one fresh
    /// operational commit budget while the caller keeps the visible warning.
    pub fn checkpoint_deadline_missed(&mut self, timing: CheckpointAttemptTiming, now: Instant) {
        self.last_dispatch = timing.previous_dispatch;
        if self.oldest_uncovered_edit.is_some() {
            // A later edit still owns a valid loss-window deadline. The next
            // snapshot includes the entire buffer, so restoring the already
            // missed edit timestamp would only make that retry impossible.
            self.failure_retry_not_before = None;
            self.fresh_commit_budget_on_next_dispatch = false;
        } else {
            self.restore_coverage(timing);
            self.failure_retry_not_before = Some(now + CHECKPOINT_RETRY_DELAY);
            self.fresh_commit_budget_on_next_dispatch = true;
        }
    }

    pub fn is_due(&self, now: Instant) -> bool {
        self.next_deadline().is_some_and(|deadline| now >= deadline)
    }

    pub fn next_deadline(&self) -> Option<Instant> {
        if !self.dirty {
            return None;
        }
        let periodic = self
            .last_dispatch
            .map(|dispatch| dispatch + MAX_LOSS_WINDOW);
        let uncovered = self
            .oldest_uncovered_edit
            .map(|oldest| oldest + IDLE_CHECKPOINT_DELAY);
        let deadline = match (uncovered, periodic) {
            (Some(uncovered), Some(periodic)) => uncovered.min(periodic),
            (Some(uncovered), None) => uncovered,
            (None, Some(periodic)) => periodic,
            (None, None) => return None,
        };
        Some(
            self.failure_retry_not_before
                .map_or(deadline, |retry| deadline.max(retry)),
        )
    }

    fn restore_coverage(&mut self, timing: CheckpointAttemptTiming) {
        if !self.dirty {
            return;
        }
        let Some(covered) = timing.oldest_covered_edit else {
            return;
        };
        self.oldest_uncovered_edit = Some(
            self.oldest_uncovered_edit
                .map_or(covered, |existing| existing.min(covered)),
        );
    }
}

#[cfg(test)]
mod tests {
    use std::{
        sync::Arc,
        time::{Duration, Instant, SystemTime},
    };

    use super::*;

    #[derive(Default)]
    struct TestProtector;

    impl RecoveryProtector for TestProtector {
        fn protect(&self, plaintext: &[u8]) -> Result<Vec<u8>, RecoveryError> {
            Ok(plaintext.iter().rev().copied().collect())
        }

        fn unprotect(&self, ciphertext: &[u8]) -> Result<Vec<u8>, RecoveryError> {
            Ok(ciphertext.iter().rev().copied().collect())
        }
    }

    #[test]
    fn test_store_constructor_has_no_platform_specific_arguments() {
        let source = include_str!("recovery.rs");
        let constructor = source
            .split("pub fn new_at_with_limits(")
            .nth(1)
            .and_then(|source| source.split("#[cfg(windows)]").next())
            .unwrap();

        assert!(constructor.contains("Ok(Self::from_parts(root, protector, limits))"));
    }

    struct UnreadableProtector;

    impl RecoveryProtector for UnreadableProtector {
        fn protect(&self, _plaintext: &[u8]) -> Result<Vec<u8>, RecoveryError> {
            Err(RecoveryError::Protection)
        }

        fn unprotect(&self, _ciphertext: &[u8]) -> Result<Vec<u8>, RecoveryError> {
            Err(RecoveryError::Unprotection)
        }
    }

    struct WriteOnlyProtector;

    impl RecoveryProtector for WriteOnlyProtector {
        fn protect(&self, plaintext: &[u8]) -> Result<Vec<u8>, RecoveryError> {
            Ok(plaintext.iter().rev().copied().collect())
        }

        fn unprotect(&self, _ciphertext: &[u8]) -> Result<Vec<u8>, RecoveryError> {
            Err(RecoveryError::Unprotection)
        }
    }

    #[derive(Default)]
    struct TrackingProtector {
        protect_active: AtomicUsize,
        protect_peak: AtomicUsize,
        protect_calls: AtomicUsize,
        unprotect_active: AtomicUsize,
        unprotect_peak: AtomicUsize,
        unprotect_calls: AtomicUsize,
        combined_peak: AtomicUsize,
        stages_overlapped: AtomicBool,
    }

    impl TrackingProtector {
        fn reset(&self) {
            self.protect_active.store(0, Ordering::SeqCst);
            self.protect_peak.store(0, Ordering::SeqCst);
            self.protect_calls.store(0, Ordering::SeqCst);
            self.unprotect_active.store(0, Ordering::SeqCst);
            self.unprotect_peak.store(0, Ordering::SeqCst);
            self.unprotect_calls.store(0, Ordering::SeqCst);
            self.combined_peak.store(0, Ordering::SeqCst);
            self.stages_overlapped.store(false, Ordering::SeqCst);
        }

        fn track(
            active: &AtomicUsize,
            other_active: &AtomicUsize,
            peak: &AtomicUsize,
            calls: &AtomicUsize,
            combined_peak: &AtomicUsize,
            stages_overlapped: &AtomicBool,
        ) {
            calls.fetch_add(1, Ordering::SeqCst);
            let current = active.fetch_add(1, Ordering::SeqCst) + 1;
            peak.fetch_max(current, Ordering::SeqCst);
            combined_peak.fetch_max(
                current + other_active.load(Ordering::SeqCst),
                Ordering::SeqCst,
            );
            stages_overlapped.fetch_or(other_active.load(Ordering::SeqCst) != 0, Ordering::SeqCst);
            std::thread::sleep(Duration::from_millis(25));
            combined_peak.fetch_max(
                active.load(Ordering::SeqCst) + other_active.load(Ordering::SeqCst),
                Ordering::SeqCst,
            );
            stages_overlapped.fetch_or(other_active.load(Ordering::SeqCst) != 0, Ordering::SeqCst);
            active.fetch_sub(1, Ordering::SeqCst);
        }
    }

    impl RecoveryProtector for TrackingProtector {
        fn protect(&self, plaintext: &[u8]) -> Result<Vec<u8>, RecoveryError> {
            Self::track(
                &self.protect_active,
                &self.unprotect_active,
                &self.protect_peak,
                &self.protect_calls,
                &self.combined_peak,
                &self.stages_overlapped,
            );
            Ok(plaintext.iter().rev().copied().collect())
        }

        fn unprotect(&self, ciphertext: &[u8]) -> Result<Vec<u8>, RecoveryError> {
            Self::track(
                &self.unprotect_active,
                &self.protect_active,
                &self.unprotect_peak,
                &self.unprotect_calls,
                &self.combined_peak,
                &self.stages_overlapped,
            );
            Ok(ciphertext.iter().rev().copied().collect())
        }
    }

    #[derive(Default)]
    struct BlockingWaveGate {
        enabled: AtomicBool,
        entered: AtomicBool,
        calls: AtomicUsize,
        started: Mutex<Option<std::sync::mpsc::Sender<()>>>,
        release: Mutex<Option<std::sync::mpsc::Receiver<()>>>,
    }

    impl BlockingWaveGate {
        fn block_next_wave(&self) -> (std::sync::mpsc::Receiver<()>, std::sync::mpsc::Sender<()>) {
            let (started_sender, started_receiver) = std::sync::mpsc::channel();
            let (release_sender, release_receiver) = std::sync::mpsc::channel();
            self.entered.store(false, Ordering::Release);
            self.enabled.store(true, Ordering::Release);
            *self.started.lock().unwrap() = Some(started_sender);
            *self.release.lock().unwrap() = Some(release_receiver);
            (started_receiver, release_sender)
        }

        fn observe(&self) {
            self.calls.fetch_add(1, Ordering::Relaxed);
            if self.enabled.load(Ordering::Acquire) && !self.entered.swap(true, Ordering::AcqRel) {
                self.started
                    .lock()
                    .unwrap()
                    .as_ref()
                    .expect("blocking wave gate must have a start sender")
                    .send(())
                    .unwrap();
                self.release
                    .lock()
                    .unwrap()
                    .as_ref()
                    .expect("blocking wave gate must have a release receiver")
                    .recv()
                    .unwrap();
            }
        }

        fn reset_calls(&self) {
            self.calls.store(0, Ordering::Release);
        }
    }

    #[derive(Default)]
    struct BlockingProtector {
        protect_gate: BlockingWaveGate,
        unprotect_gate: BlockingWaveGate,
    }

    impl RecoveryProtector for BlockingProtector {
        fn protect(&self, plaintext: &[u8]) -> Result<Vec<u8>, RecoveryError> {
            self.protect_gate.observe();
            Ok(plaintext.iter().rev().copied().collect())
        }

        fn unprotect(&self, ciphertext: &[u8]) -> Result<Vec<u8>, RecoveryError> {
            self.unprotect_gate.observe();
            Ok(ciphertext.iter().rev().copied().collect())
        }
    }

    #[derive(Default)]
    struct OutOfOrderProtector {
        fast_finished: AtomicBool,
        slow_observed_fast: AtomicBool,
    }

    impl RecoveryProtector for OutOfOrderProtector {
        fn protect(&self, plaintext: &[u8]) -> Result<Vec<u8>, RecoveryError> {
            if plaintext
                .windows(b"slow-first".len())
                .any(|window| window == b"slow-first")
            {
                std::thread::sleep(Duration::from_millis(40));
                self.slow_observed_fast
                    .store(self.fast_finished.load(Ordering::SeqCst), Ordering::SeqCst);
            } else if plaintext
                .windows(b"fast-last".len())
                .any(|window| window == b"fast-last")
            {
                self.fast_finished.store(true, Ordering::SeqCst);
            }
            Ok(plaintext.iter().rev().copied().collect())
        }

        fn unprotect(&self, ciphertext: &[u8]) -> Result<Vec<u8>, RecoveryError> {
            Ok(ciphertext.iter().rev().copied().collect())
        }
    }

    struct PayloadFailingProtector;

    impl RecoveryProtector for PayloadFailingProtector {
        fn protect(&self, plaintext: &[u8]) -> Result<Vec<u8>, RecoveryError> {
            if plaintext
                .windows(b"would-fail".len())
                .any(|window| window == b"would-fail")
            {
                return Err(RecoveryError::Protection);
            }
            Ok(plaintext.iter().rev().copied().collect())
        }

        fn unprotect(&self, ciphertext: &[u8]) -> Result<Vec<u8>, RecoveryError> {
            Ok(ciphertext.iter().rev().copied().collect())
        }
    }

    fn test_store() -> (tempfile::TempDir, RecoveryStore) {
        let dir = tempfile::tempdir().unwrap();
        let store =
            RecoveryStore::new_at(dir.path().join("recovery"), Arc::new(TestProtector)).unwrap();
        (dir, store)
    }

    fn test_store_with_limits(limits: RecoveryLimits) -> (tempfile::TempDir, RecoveryStore) {
        let dir = tempfile::tempdir().unwrap();
        let store = RecoveryStore::new_at_with_limits(
            dir.path().join("recovery"),
            Arc::new(TestProtector),
            limits,
        )
        .unwrap();
        (dir, store)
    }

    fn metadata() -> RecoveryMetadata {
        RecoveryMetadata {
            source_path: None,
            encoding_name: "GBK".to_owned(),
            had_bom: true,
            newline: Newline::Crlf,
            original_stamp: FileStamp {
                modified: Some(UNIX_EPOCH + Duration::from_secs(42)),
                len: 9,
                digest: [7; 32],
                object_id: None,
            },
            source_identity: SourceIdentity::SymbolicLink {
                link_target: PathBuf::from("CLAUDE.md"),
                resolved_target: PathBuf::from("AGENTS.md"),
            },
            decode_had_errors: true,
        }
    }

    fn checkpoint(name: &str, text: &str) -> RecoveryCheckpoint {
        RecoveryCheckpoint {
            key: RecoveryKey::for_document_id(name),
            text: text.to_owned(),
            metadata: metadata(),
        }
    }

    #[test]
    fn new_memory_keys_are_unique_and_valid_recovery_identifiers() {
        let first = RecoveryKey::new_memory();
        let second = RecoveryKey::new_memory();

        assert_ne!(first, second);
        assert!(first.is_valid());
        assert!(second.is_valid());
    }

    #[test]
    fn production_limits_match_goal_02_recovery_contract() {
        assert_eq!(
            RecoveryLimits::default(),
            RecoveryLimits {
                max_records: 50,
                max_record_bytes: 32 * 1024 * 1024,
                max_total_bytes: 128 * 1024 * 1024,
                max_age: Duration::from_secs(7 * 24 * 60 * 60),
            }
        );
    }

    #[test]
    fn recovery_store_is_send_and_sync_for_background_checkpoints() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<RecoveryStore>();
    }

    #[cfg(windows)]
    #[test]
    fn production_root_rejects_unc_paths_without_network_access() {
        for root in [
            PathBuf::from(r"\\missing-host\missing-share\recovery"),
            PathBuf::from(r"\\?\UNC\missing-host\missing-share\recovery"),
        ] {
            assert!(matches!(
                RecoveryStore::open_production_at_for_test(root, Arc::new(TestProtector)),
                Err(RecoveryError::Unavailable(LOCAL_RECOVERY_ROOT_REQUIRED))
            ));
        }
    }

    #[cfg(windows)]
    #[test]
    fn production_root_parses_disk_and_verbatim_disk_volume_roots() {
        assert_eq!(
            production_volume_root(Path::new(r"C:\recovery")).unwrap(),
            vec![b'C' as u16, b':' as u16, b'\\' as u16, 0]
        );
        assert_eq!(
            production_volume_root(Path::new(r"\\?\D:\recovery")).unwrap(),
            vec![
                b'\\' as u16,
                b'\\' as u16,
                b'?' as u16,
                b'\\' as u16,
                b'D' as u16,
                b':' as u16,
                b'\\' as u16,
                0,
            ]
        );
    }

    #[cfg(windows)]
    #[test]
    fn production_root_parses_case_insensitive_volume_guid_roots() {
        for root in [
            r"\\?\Volume{01234567-89ab-cdef-0123-456789abcdef}\recovery",
            r"\\?\VOLUME{01234567-89AB-CDEF-0123-456789ABCDEF}\recovery",
        ] {
            let volume_root = production_volume_root(Path::new(root)).unwrap();
            assert_eq!(
                &volume_root[..4],
                &[b'\\' as u16, b'\\' as u16, b'?' as u16, b'\\' as u16]
            );
            assert_eq!(&volume_root[volume_root.len() - 2..], &[b'\\' as u16, 0]);
        }
    }

    #[cfg(windows)]
    #[test]
    fn production_root_rejects_non_volume_guid_and_relative_prefixes() {
        for root in [
            r"C:relative",
            r"\\?\C:relative",
            r"\\?\Volume{not-a-guid}\recovery",
            r"\\?\Volume{01234567-89ab-cdef-0123-456789abcdef}relative",
            r"\\?\GLOBALROOT\Device\HarddiskVolume1\recovery",
            r"\\?\PhysicalDrive0\recovery",
            r"\\.\PhysicalDrive0",
        ] {
            assert!(matches!(
                production_volume_root(Path::new(root)),
                Err(RecoveryError::Unavailable(LOCAL_RECOVERY_ROOT_REQUIRED))
            ));
        }
    }

    #[cfg(windows)]
    #[test]
    fn production_root_finds_existing_ancestor_without_creating_missing_tail() {
        let dir = tempfile::tempdir().unwrap();
        let ancestor = dir.path().join("ancestor");
        let missing = ancestor.join("missing");
        let root = missing.join("recovery");
        fs::create_dir(&ancestor).unwrap();

        assert_eq!(
            nearest_existing_production_ancestor(&root, production_root_path_exists).unwrap(),
            ancestor
        );
        assert!(!missing.exists());
    }

    #[cfg(windows)]
    #[test]
    fn production_root_ancestor_propagates_non_missing_errors() {
        let error = nearest_existing_production_ancestor(Path::new(r"C:\missing"), |_| {
            Err(io::Error::new(io::ErrorKind::PermissionDenied, "denied"))
        })
        .unwrap_err();

        assert_eq!(error.kind(), io::ErrorKind::PermissionDenied);
    }

    #[cfg(windows)]
    #[test]
    fn production_root_validation_precedes_directory_creation() {
        let source = include_str!("recovery.rs");
        let constructor = source
            .split("fn new_production_at(")
            .nth(1)
            .and_then(|source| source.split("fn from_parts(").next())
            .unwrap();

        assert!(
            constructor
                .find("ensure_local_production_root(&root)?;")
                .unwrap()
                < constructor.find("fs::create_dir_all(&root)?;").unwrap()
        );
    }

    #[cfg(windows)]
    #[test]
    fn production_root_allows_only_local_drive_types() {
        use windows::Win32::System::WindowsProgramming::{
            DRIVE_CDROM, DRIVE_FIXED, DRIVE_NO_ROOT_DIR, DRIVE_RAMDISK, DRIVE_REMOTE,
            DRIVE_REMOVABLE, DRIVE_UNKNOWN,
        };

        for drive_type in [DRIVE_FIXED, DRIVE_REMOVABLE, DRIVE_RAMDISK] {
            assert!(is_local_recovery_drive_type(drive_type));
        }
        for drive_type in [
            DRIVE_UNKNOWN,
            DRIVE_NO_ROOT_DIR,
            DRIVE_REMOTE,
            DRIVE_CDROM,
            7,
        ] {
            assert!(!is_local_recovery_drive_type(drive_type));
        }
    }

    #[cfg(windows)]
    #[test]
    fn production_open_accepts_a_local_temp_root() {
        let dir = tempfile::tempdir().unwrap();
        RecoveryStore::open_production_at_for_test(
            dir.path().join("recovery"),
            Arc::new(TestProtector),
        )
        .unwrap();
    }

    #[cfg(windows)]
    #[test]
    fn production_root_lease_excludes_a_second_store_until_the_first_drops() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("recovery");
        let first =
            RecoveryStore::open_production_at_for_test(root.clone(), Arc::new(TestProtector))
                .unwrap();
        first
            .checkpoint(
                &checkpoint("production-lease", "checkpoint"),
                &HashSet::new(),
            )
            .unwrap();
        let first_clone = first.clone();

        let error =
            RecoveryStore::open_production_at_for_test(root.clone(), Arc::new(TestProtector))
                .unwrap_err();
        assert!(matches!(
            error,
            RecoveryError::Unavailable("recovery is already active in another markturbo instance")
        ));
        drop(first);

        let error =
            RecoveryStore::open_production_at_for_test(root.clone(), Arc::new(TestProtector))
                .unwrap_err();
        assert!(matches!(
            error,
            RecoveryError::Unavailable("recovery is already active in another markturbo instance")
        ));
        drop(first_clone);

        RecoveryStore::open_production_at_for_test(root, Arc::new(TestProtector)).unwrap();
    }

    #[cfg(windows)]
    #[test]
    fn production_open_rejects_a_reparse_point_recovery_root() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("target");
        let root = dir.path().join("recovery");
        fs::create_dir(&target).unwrap();
        if let Err(error) = std::os::windows::fs::symlink_dir(&target, &root) {
            eprintln!("skipping reparse-root test: cannot create directory symlink: {error}");
            return;
        }

        let error =
            RecoveryStore::open_production_at_for_test(root, Arc::new(TestProtector)).unwrap_err();
        assert!(matches!(error, RecoveryError::Unavailable(_)));
    }

    #[cfg(windows)]
    #[test]
    fn root_acquisition_holds_the_raw_root_before_canonicalization() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("recovery");
        let moved = dir.path().join("recovery-moved");
        let external = dir.path().join("external");
        fs::create_dir(&root).unwrap();
        fs::create_dir(&external).unwrap();
        let hook_root = root.clone();
        let hook_moved = moved.clone();
        let hook_external = external.clone();
        set_root_acquisition_hook(Box::new(move || {
            let replacement = fs::rename(&hook_root, &hook_moved);
            if replacement.is_ok() {
                std::os::windows::fs::symlink_dir(&hook_external, &hook_root).unwrap();
            }
            assert!(
                replacement.is_err(),
                "the raw root must be guarded before canonicalization"
            );
        }));

        let store =
            RecoveryStore::open_production_at_for_test(root.clone(), Arc::new(TestProtector))
                .unwrap();
        store
            .checkpoint(
                &checkpoint("acquisition-guard", "checkpoint"),
                &HashSet::new(),
            )
            .unwrap();
        assert!(root.exists());
        assert!(fs::read_dir(&external).unwrap().next().is_none());
    }

    #[cfg(windows)]
    #[test]
    fn production_open_rejects_a_reparse_point_lease_file() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("recovery");
        let target = dir.path().join("external-lease");
        fs::create_dir(&root).unwrap();
        fs::write(&target, b"external").unwrap();
        let lease = root.join(".markturbo-recovery.lock");
        if let Err(error) = std::os::windows::fs::symlink_file(&target, &lease) {
            eprintln!("skipping reparse-lease test: cannot create file symlink: {error}");
            return;
        }

        let error =
            RecoveryStore::open_production_at_for_test(root, Arc::new(TestProtector)).unwrap_err();
        assert!(matches!(
            error,
            RecoveryError::Unavailable("recovery lease must not be a reparse point")
        ));
    }

    #[cfg(windows)]
    #[test]
    fn production_lease_preserves_non_sharing_io_errors() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("recovery");
        fs::create_dir(&root).unwrap();
        fs::create_dir(root.join(".markturbo-recovery.lock")).unwrap();

        let error =
            RecoveryStore::open_production_at_for_test(root, Arc::new(TestProtector)).unwrap_err();
        assert!(matches!(error, RecoveryError::Io(_)));
    }

    #[cfg(windows)]
    #[test]
    fn production_root_guard_blocks_replacement_without_writing_to_an_external_target() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("recovery");
        let external = dir.path().join("external");
        let moved = dir.path().join("recovery-moved");
        fs::create_dir(&external).unwrap();
        let store =
            RecoveryStore::open_production_at_for_test(root.clone(), Arc::new(TestProtector))
                .unwrap();

        assert!(fs::rename(&root, &moved).is_err());
        store
            .checkpoint(&checkpoint("guarded-root", "checkpoint"), &HashSet::new())
            .unwrap();
        assert!(fs::read_dir(&external).unwrap().next().is_none());
    }

    #[test]
    fn path_key_is_stable_and_does_not_expose_the_path() {
        let path = Path::new("workspace/secret-plan.md");
        let first = RecoveryKey::for_path(path);
        assert_eq!(first, RecoveryKey::for_path(path));
        assert!(!first.as_str().contains("secret-plan"));
    }

    #[cfg(windows)]
    #[test]
    fn path_key_uses_the_v2_windows_wide_domain_separator() {
        use std::os::windows::ffi::OsStrExt as _;

        let path = Path::new(r"C:\case-sensitive\Plan.md");
        let mut hasher = Sha256::new();
        hasher.update(b"markturbo-recovery-path-v2\0windows-osstr-wide-le\0");
        for unit in path.as_os_str().encode_wide() {
            hasher.update(unit.to_le_bytes());
        }

        assert_eq!(
            RecoveryKey::for_path(path).as_str(),
            format!("{:x}", hasher.finalize())
        );
    }

    #[cfg(windows)]
    #[test]
    fn path_keys_preserve_case_in_case_sensitive_directories() {
        let upper = RecoveryKey::for_path(Path::new(r"C:\case-sensitive\Plan.md"));
        let lower = RecoveryKey::for_path(Path::new(r"C:\case-sensitive\plan.md"));

        assert_ne!(upper, lower);
    }

    #[cfg(windows)]
    #[test]
    fn case_distinct_paths_keep_distinct_recovery_records() {
        let (_dir, store) = test_store();
        let upper = RecoveryCheckpoint {
            key: RecoveryKey::for_path(Path::new(r"C:\case-sensitive\Plan.md")),
            text: "upper".to_owned(),
            metadata: metadata(),
        };
        let lower = RecoveryCheckpoint {
            key: RecoveryKey::for_path(Path::new(r"C:\case-sensitive\plan.md")),
            text: "lower".to_owned(),
            metadata: metadata(),
        };

        store.checkpoint(&upper, &HashSet::new()).unwrap();
        store.checkpoint(&lower, &HashSet::new()).unwrap();
        let records = store.recover().unwrap().records;
        assert_eq!(records.len(), 2);
        let texts: HashSet<_> = records
            .into_iter()
            .map(|record| record.record.text)
            .collect();
        assert_eq!(texts, HashSet::from([upper.text, lower.text]));
    }

    fn write_record_at(store: &RecoveryStore, checkpoint: &RecoveryCheckpoint, at: SystemTime) {
        let disk = DiskRecord::from_checkpoint(checkpoint, at).unwrap();
        let plaintext = serde_json::to_vec(&disk).unwrap();
        let ciphertext = store.protector.protect(&plaintext).unwrap();
        store
            .atomic_write(&store.record_path(&checkpoint.key), &ciphertext)
            .unwrap();
    }

    #[test]
    fn round_trips_cjk_emoji_and_save_metadata() {
        let (_dir, store) = test_store();
        let checkpoint = checkpoint("one", "中文内容\nemoji: \u{1f680}\n");
        store.checkpoint(&checkpoint, &HashSet::new()).unwrap();

        let scan = store.recover().unwrap();
        assert!(scan.issues.is_empty());
        assert_eq!(scan.records.len(), 1);
        assert_eq!(scan.records[0].record.text, checkpoint.text);
        assert_eq!(scan.records[0].record.metadata, checkpoint.metadata);
        assert!(
            !scan.records[0].source_conflicted,
            "untitled buffers have no source conflict"
        );
    }

    #[test]
    fn disk_file_stamp_round_trips_object_identity_and_omits_none() {
        let stamp = FileStamp {
            modified: Some(UNIX_EPOCH + Duration::from_secs(42)),
            len: 9,
            digest: [7; 32],
            object_id: Some(FileObjectId {
                volume_serial_number: 12,
                file_id: [3; 16],
            }),
        };
        let disk = DiskFileStamp::from_stamp(&stamp).unwrap();
        assert_eq!(disk.into_stamp().unwrap(), stamp);

        let none = DiskFileStamp::from_stamp(&FileStamp {
            object_id: None,
            ..stamp
        })
        .unwrap();
        let json = serde_json::to_string(&none).unwrap();
        assert!(
            !json.contains("object_id"),
            "portable recovery records omit unavailable object identities"
        );
    }

    #[test]
    fn old_record_without_object_identity_recovers() {
        let (_dir, store) = test_store();
        let checkpoint = checkpoint("old-record", "preserved text\n");
        let disk = DiskRecord::from_checkpoint(&checkpoint, SystemTime::now()).unwrap();
        let plaintext = serde_json::to_vec(&disk).unwrap();
        assert!(
            !std::str::from_utf8(&plaintext)
                .unwrap()
                .contains("object_id"),
            "the old record format has no object identity field"
        );
        let ciphertext = store.protector.protect(&plaintext).unwrap();
        store
            .atomic_write(&store.record_path(&checkpoint.key), &ciphertext)
            .unwrap();

        let scan = store.recover().unwrap();
        assert_eq!(scan.records.len(), 1);
        assert_eq!(scan.records[0].record.text, checkpoint.text);
        assert_eq!(
            scan.records[0].record.metadata.original_stamp.object_id,
            None
        );
    }

    #[cfg(windows)]
    #[test]
    fn old_source_stamp_without_identity_recovers_as_conflicted() {
        let (dir, store) = test_store();
        let source = dir.path().join("source.md");
        fs::write(&source, "source\n").unwrap();
        let loaded = crate::fs::load(&source).unwrap();
        assert!(
            loaded.stamp.object_id.is_some(),
            "a normal Windows source must provide an object identity"
        );
        let mut checkpoint = checkpoint("old-source", "recovered text\n");
        checkpoint.metadata = RecoveryMetadata::from_loaded_file(&loaded);
        checkpoint.metadata.original_stamp.object_id = None;
        let disk = DiskRecord::from_checkpoint(&checkpoint, SystemTime::now()).unwrap();
        let plaintext = serde_json::to_vec(&disk).unwrap();
        assert!(
            !std::str::from_utf8(&plaintext)
                .unwrap()
                .contains("object_id")
        );
        let ciphertext = store.protector.protect(&plaintext).unwrap();
        store
            .atomic_write(&store.record_path(&checkpoint.key), &ciphertext)
            .unwrap();

        let scan = store.recover().unwrap();
        assert_eq!(scan.records.len(), 1);
        assert!(
            scan.records[0].source_conflicted,
            "a legacy stamp cannot authorize overwrite of a current identified source"
        );
    }

    #[test]
    fn detects_a_changed_source_as_conflicted() {
        let (dir, store) = test_store();
        let source = dir.path().join("source.md");
        fs::write(&source, "before\n").unwrap();
        let loaded = crate::fs::load(&source).unwrap();
        let mut checkpoint = checkpoint("source", "new text\n");
        checkpoint.metadata = RecoveryMetadata::from_loaded_file(&loaded);
        store.checkpoint(&checkpoint, &HashSet::new()).unwrap();
        fs::write(&source, "external rewrite\n").unwrap();

        let scan = store.recover().unwrap();
        assert!(scan.records[0].source_conflicted);
    }

    #[cfg(windows)]
    #[test]
    fn retargeted_symlink_to_locked_target_recovers_as_conflicted() {
        use std::os::windows::fs::OpenOptionsExt as _;

        let (dir, store) = test_store();
        let original = dir.path().join("original.md");
        let alternate = dir.path().join("alternate.md");
        let source = dir.path().join("source.md");
        fs::write(&original, "original\n").unwrap();
        fs::write(&alternate, "alternate\n").unwrap();
        if let Err(error) = std::os::windows::fs::symlink_file(&original, &source) {
            eprintln!("skipping file-symlink test: {error}");
            return;
        }
        let loaded = crate::fs::load(&source).unwrap();
        let checkpoint = RecoveryCheckpoint {
            key: RecoveryKey::for_path(&source),
            text: "recovered text\n".to_owned(),
            metadata: RecoveryMetadata::from_loaded_file(&loaded),
        };
        store.checkpoint(&checkpoint, &HashSet::new()).unwrap();

        fs::remove_file(&source).unwrap();
        std::os::windows::fs::symlink_file(&alternate, &source).unwrap();
        let _target_lock = fs::OpenOptions::new()
            .read(true)
            .write(true)
            .share_mode(0)
            .open(&alternate)
            .unwrap();

        let scan = store.recover().unwrap();
        assert_eq!(scan.records.len(), 1);
        assert!(scan.records[0].source_conflicted);
    }

    #[cfg(windows)]
    #[test]
    fn unchanged_symlinked_legacy_source_recovers_without_conflict() {
        let (dir, store) = test_store();
        let target = dir.path().join("legacy.txt");
        let source = dir.path().join("source.txt");
        fs::write(&target, b"\xD6\xD0\xCE\xC4\r\n").unwrap();
        if let Err(error) = std::os::windows::fs::symlink_file(&target, &source) {
            eprintln!("skipping file-symlink test: {error}");
            return;
        }
        let loaded = crate::fs::load(&source).unwrap();
        assert_eq!(loaded.encoding.name(), "GBK");
        let checkpoint = RecoveryCheckpoint {
            key: RecoveryKey::for_path(&source),
            text: loaded.text.clone(),
            metadata: RecoveryMetadata::from_loaded_file(&loaded),
        };
        store.checkpoint(&checkpoint, &HashSet::new()).unwrap();

        let scan = store.recover().unwrap();
        assert_eq!(scan.records.len(), 1);
        assert!(!scan.records[0].source_conflicted);
    }

    #[test]
    fn malformed_oversized_expired_and_unreadable_records_are_reported() {
        let (_dir, store) = test_store_with_limits(RecoveryLimits {
            max_records: 50,
            max_record_bytes: 10_000,
            max_total_bytes: 20_000,
            max_age: Duration::from_secs(7 * 24 * 60 * 60),
        });
        fs::write(
            store.record_path(&RecoveryKey::for_document_id("malformed")),
            b"not json after decryption",
        )
        .unwrap();
        fs::write(
            store.record_path(&RecoveryKey::for_document_id("oversized")),
            vec![0; 10_001],
        )
        .unwrap();

        let expired = checkpoint("expired", "x");
        write_record_at(
            &store,
            &expired,
            SystemTime::now() - Duration::from_secs(7 * 24 * 60 * 60),
        );
        let unreadable_store = RecoveryStore::new_at_with_limits(
            store.root().to_path_buf(),
            Arc::new(UnreadableProtector),
            RecoveryLimits {
                max_records: 50,
                max_record_bytes: 10_000,
                max_total_bytes: 20_000,
                max_age: Duration::from_secs(7 * 24 * 60 * 60),
            },
        )
        .unwrap();

        let scan = unreadable_store.recover().unwrap();
        assert!(scan.records.is_empty());
        assert!(
            scan.issues
                .iter()
                .any(|issue| matches!(issue, RecoveryIssue::Unreadable { .. }))
        );

        let scan = store.recover().unwrap();
        assert!(
            scan.issues
                .iter()
                .any(|issue| matches!(issue, RecoveryIssue::Malformed { .. }))
        );
        assert!(
            scan.issues
                .iter()
                .any(|issue| matches!(issue, RecoveryIssue::Oversized { .. }))
        );
        assert!(
            scan.issues
                .iter()
                .any(|issue| matches!(issue, RecoveryIssue::Expired { .. }))
        );
    }

    #[test]
    fn noncanonical_recovery_filenames_are_ignored_without_cleanup() {
        let (_dir, store) = test_store();
        let junk = store.root().join("junk.mtrecovery");
        let uppercase = store.root().join(format!("{}.mtrecovery", "A".repeat(64)));
        fs::write(&junk, b"not a managed recovery record").unwrap();
        fs::write(&uppercase, b"not a managed recovery record").unwrap();

        let scan = store.recover().unwrap();
        assert!(scan.records.is_empty());
        assert!(scan.issues.is_empty());
        store.prune().unwrap();
        assert!(junk.exists());
        assert!(uppercase.exists());
    }

    #[test]
    fn replayed_ciphertext_at_another_canonical_key_is_pruned_and_cannot_revive() {
        let (_dir, store) = test_store();
        let source = checkpoint("replay-source", "source checkpoint");
        let replay_key = RecoveryKey::for_document_id("replay-destination");
        store.checkpoint(&source, &HashSet::new()).unwrap();
        let replay = store.record_path(&replay_key);
        fs::copy(store.record_path(&source.key), &replay).unwrap();

        let scan = store.recover().unwrap();
        assert_eq!(scan.records.len(), 1);
        assert_eq!(scan.records[0].record.key, source.key);
        assert!(
            scan.issues
                .iter()
                .any(|issue| matches!(issue, RecoveryIssue::Malformed { path } if path == &replay))
        );

        store.invalidate_and_delete(&source.key).unwrap();
        assert!(store.recover().unwrap().records.is_empty());
        store.prune().unwrap();
        assert!(!replay.exists());
    }

    #[test]
    fn startup_maintenance_removes_invalid_records_and_reports_unreadable_ones() {
        let limits = RecoveryLimits {
            max_records: 50,
            max_record_bytes: 100,
            max_total_bytes: 1_000,
            max_age: Duration::from_secs(7 * 24 * 60 * 60),
        };
        let (_dir, store) = test_store_with_limits(limits);
        let malformed = store.record_path(&RecoveryKey::for_document_id("malformed"));
        let oversized = store.record_path(&RecoveryKey::for_document_id("oversized"));
        fs::write(&malformed, b"not json after decryption").unwrap();
        fs::write(&oversized, vec![0; 101]).unwrap();

        let maintenance = store.prune().unwrap();
        assert!(
            maintenance.issues.iter().any(
                |issue| matches!(issue, RecoveryIssue::Malformed { path } if path == &malformed)
            )
        );
        assert!(maintenance.issues.iter().any(
            |issue| matches!(issue, RecoveryIssue::Oversized { path, .. } if path == &oversized)
        ));
        assert!(!malformed.exists());
        assert!(!oversized.exists());

        let unreadable = store.record_path(&RecoveryKey::for_document_id("unreadable"));
        fs::write(&unreadable, b"encrypted checkpoint").unwrap();
        let unreadable_store = RecoveryStore::new_at_with_limits(
            store.root().to_path_buf(),
            Arc::new(UnreadableProtector),
            limits,
        )
        .unwrap();

        let maintenance = unreadable_store.prune().unwrap();
        assert!(maintenance.issues.iter().any(
            |issue| matches!(issue, RecoveryIssue::Unreadable { path } if path == &unreadable)
        ));
        assert!(unreadable.exists());
    }

    #[test]
    fn locked_retired_artifact_does_not_block_valid_recovery() {
        let (_dir, store) = test_store();
        let valid = checkpoint("valid-with-retired", "valid checkpoint");
        store.checkpoint(&valid, &HashSet::new()).unwrap();
        let retired = store
            .root()
            .join(".markturbo-recovery-retired-b000000000000000");
        fs::write(&retired, b"retired checkpoint").unwrap();
        store.fail_next_delete_for_test();

        let maintenance = store.prune().unwrap();
        assert!(maintenance.issues.iter().any(
            |issue| matches!(issue, RecoveryIssue::CleanupPending { path } if path == &retired)
        ));
        assert!(retired.exists());
        let records = store.recover().unwrap().records;
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].record.text, valid.text);
    }

    #[test]
    fn locked_expired_record_does_not_block_valid_recovery() {
        let (_dir, store) = test_store();
        let valid = checkpoint("valid-with-expired", "valid checkpoint");
        let expired = checkpoint("locked-expired", "expired checkpoint");
        store.checkpoint(&valid, &HashSet::new()).unwrap();
        write_record_at(
            &store,
            &expired,
            SystemTime::now() - RecoveryLimits::default().max_age,
        );
        let expired_path = store.record_path(&expired.key);
        store.fail_next_delete_for_test();

        let maintenance = store.prune().unwrap();
        assert!(maintenance.issues.iter().any(
            |issue| matches!(issue, RecoveryIssue::CleanupPending { path } if path == &expired_path)
        ));
        assert!(expired_path.exists());
        let records = store.recover().unwrap().records;
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].record.text, valid.text);
    }

    #[test]
    fn locked_invalid_record_does_not_block_valid_recovery() {
        let (_dir, store) = test_store();
        let valid = checkpoint("valid-with-invalid", "valid checkpoint");
        store.checkpoint(&valid, &HashSet::new()).unwrap();
        let invalid = store.record_path(&RecoveryKey::for_document_id("locked-invalid"));
        fs::write(&invalid, b"not encrypted json").unwrap();
        store.fail_next_delete_for_test();

        let maintenance = store.prune().unwrap();
        assert!(maintenance.issues.iter().any(
            |issue| matches!(issue, RecoveryIssue::CleanupPending { path } if path == &invalid)
        ));
        assert!(invalid.exists());
        let records = store.recover().unwrap().records;
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].record.text, valid.text);
    }

    #[test]
    fn undeleted_retired_artifact_keeps_its_quota_reservation() {
        let (_dir, store) = test_store_with_limits(RecoveryLimits {
            max_records: 2,
            max_record_bytes: 10_000,
            max_total_bytes: 20_000,
            max_age: Duration::from_secs(7 * 24 * 60 * 60),
        });
        let valid = checkpoint("quota-valid", "valid checkpoint");
        store.checkpoint(&valid, &HashSet::new()).unwrap();
        let retired = store
            .root()
            .join(".markturbo-recovery-retired-c000000000000000");
        fs::write(&retired, b"retired checkpoint").unwrap();
        let retired_bytes = fs::metadata(&retired).unwrap().len();

        store.fail_next_delete_for_test();
        {
            let _guard = store.lock_mutations();
            let pruned = store.prune_locked(SystemTime::now()).unwrap();
            assert_eq!(pruned.scan.known_record_count, 2);
            assert_eq!(
                pruned.scan.known_record_sizes.get(&retired),
                Some(&retired_bytes)
            );
            assert!(pruned.maintenance.issues.iter().any(
                |issue| matches!(issue, RecoveryIssue::CleanupPending { path } if path == &retired)
            ));
        }

        store.fail_next_delete_for_test();
        let error = store
            .checkpoint_at(
                &checkpoint("quota-next", "next checkpoint"),
                &HashSet::from([valid.key]),
                SystemTime::now(),
            )
            .unwrap_err();
        assert!(matches!(error, RecoveryError::QuotaExceeded { .. }));
        assert!(retired.exists());
    }

    #[test]
    fn malformed_and_oversized_records_reserve_known_quota_before_decoding() {
        let (_dir, store) = test_store_with_limits(RecoveryLimits {
            max_records: 50,
            max_record_bytes: 100,
            max_total_bytes: 1_000,
            max_age: Duration::from_secs(7 * 24 * 60 * 60),
        });
        let malformed = store.record_path(&RecoveryKey::for_document_id("malformed"));
        let oversized = store.record_path(&RecoveryKey::for_document_id("oversized"));
        fs::write(&malformed, b"not encrypted JSON").unwrap();
        fs::write(&oversized, vec![0; 101]).unwrap();

        let _guard = store.lock_mutations();
        let scan = store.scan_retention_locked(SystemTime::now()).unwrap();
        assert_eq!(scan.known_record_count, 2);
        assert_eq!(scan.known_bytes, 119);
        assert_eq!(scan.known_record_sizes.get(&malformed), Some(&18));
        assert_eq!(scan.known_record_sizes.get(&oversized), Some(&101));
        assert!(
            scan.issues.iter().any(
                |issue| matches!(issue, RecoveryIssue::Malformed { path } if path == &malformed)
            )
        );
        assert!(scan.issues.iter().any(
            |issue| matches!(issue, RecoveryIssue::Oversized { path, bytes: 101 } if path == &oversized)
        ));
    }

    #[test]
    fn unreadable_records_count_toward_the_record_limit_before_checkpointing() {
        let limits = RecoveryLimits {
            max_records: 1,
            max_record_bytes: 10_000,
            max_total_bytes: 20_000,
            max_age: Duration::from_secs(7 * 24 * 60 * 60),
        };
        let (_dir, writable_store) = test_store_with_limits(limits);
        let unreadable = checkpoint("unreadable", "saved text");
        write_record_at(&writable_store, &unreadable, SystemTime::now());
        let store = RecoveryStore::new_at_with_limits(
            writable_store.root().to_path_buf(),
            Arc::new(WriteOnlyProtector),
            limits,
        )
        .unwrap();

        let error = store
            .checkpoint(&checkpoint("next", "new text"), &HashSet::new())
            .unwrap_err();

        assert!(matches!(error, RecoveryError::QuotaExceeded { .. }));
        assert_eq!(fs::read_dir(store.root()).unwrap().count(), 1);
    }

    #[test]
    fn unreadable_records_count_toward_the_total_byte_limit_before_checkpointing() {
        let limits = RecoveryLimits {
            max_records: 50,
            max_record_bytes: 10_000,
            max_total_bytes: 1,
            max_age: Duration::from_secs(7 * 24 * 60 * 60),
        };
        let (_dir, writable_store) = test_store_with_limits(limits);
        let unreadable = checkpoint("unreadable", "saved text");
        write_record_at(&writable_store, &unreadable, SystemTime::now());
        let existing_bytes = fs::metadata(writable_store.record_path(&unreadable.key))
            .unwrap()
            .len();
        let store = RecoveryStore::new_at_with_limits(
            writable_store.root().to_path_buf(),
            Arc::new(WriteOnlyProtector),
            RecoveryLimits {
                max_total_bytes: existing_bytes,
                ..limits
            },
        )
        .unwrap();

        let error = store
            .checkpoint(&checkpoint("next", "new text"), &HashSet::new())
            .unwrap_err();

        assert!(matches!(error, RecoveryError::QuotaExceeded { .. }));
        assert_eq!(fs::read_dir(store.root()).unwrap().count(), 1);
    }

    #[test]
    fn prune_removes_expired_records_without_touching_source_files() {
        let (dir, store) = test_store();
        let source = dir.path().join("source.md");
        fs::write(&source, "source stays put\n").unwrap();
        let mut item = checkpoint("expired", "saved text");
        item.metadata.source_path = Some(source.clone());
        write_record_at(
            &store,
            &item,
            SystemTime::now() - Duration::from_secs(7 * 24 * 60 * 60),
        );

        let maintenance = store.prune().unwrap();
        assert_eq!(maintenance.removed_expired, 1);
        assert_eq!(fs::read_to_string(source).unwrap(), "source stays put\n");
    }

    #[test]
    fn completed_checkpoint_prunes_expired_records() {
        let (_dir, store) = test_store();
        let expired = checkpoint("expired", "old");
        write_record_at(
            &store,
            &expired,
            SystemTime::now() - Duration::from_secs(7 * 24 * 60 * 60),
        );
        assert_eq!(store.retention_scan_count_for_test(), 0);

        let receipt = store
            .checkpoint(&checkpoint("current", "new"), &HashSet::new())
            .unwrap();
        assert_eq!(receipt.maintenance.removed_expired, 1);
        assert_eq!(
            store.retention_scan_count_for_test(),
            1,
            "checkpoint should reuse the pre-prune retention scan instead of rescanning after commit"
        );
        let records = store.recover().unwrap().records;
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].record.text, "new");
    }

    #[test]
    fn batch_checkpoints_scan_retention_once_and_restore_each_exact_text() {
        let (_dir, store) = test_store();
        let expired = checkpoint("batch-expired", "expired text");
        let first = checkpoint("batch-first", "first exact text\n");
        let second = checkpoint("batch-second", "second exact text \u{1f680}\n");
        write_record_at(
            &store,
            &expired,
            SystemTime::now() - Duration::from_secs(7 * 24 * 60 * 60),
        );
        let first_token = store.current_token(&first.key);
        let second_token = store.current_token(&second.key);

        let batch = store.checkpoint_batch_if_current(
            [
                RecoveryCheckpointAttempt {
                    checkpoint: &first,
                    token: &first_token,
                },
                RecoveryCheckpointAttempt {
                    checkpoint: &second,
                    token: &second_token,
                },
            ],
            &HashSet::new(),
        );

        assert!(matches!(
            batch.outcomes.as_slice(),
            [
                CheckpointBatchOutcome::Written,
                CheckpointBatchOutcome::Written
            ]
        ));
        assert_eq!(store.retention_scan_count_for_test(), 1);
        assert_eq!(batch.maintenance.removed_expired, 1);
        let recovered: HashMap<_, _> = store
            .recover()
            .unwrap()
            .records
            .into_iter()
            .map(|entry| (entry.record.key, entry.record.text))
            .collect();
        assert_eq!(recovered.get(&first.key), Some(&first.text));
        assert_eq!(recovered.get(&second.key), Some(&second.text));
    }

    #[test]
    fn retention_scan_keeps_only_a_descriptor_while_recovery_keeps_the_payload() {
        let (dir, store) = test_store();
        let mut checkpoint = checkpoint("retention-descriptor", "exact retained text\\n");
        checkpoint.metadata.source_path = Some(dir.path().join("missing-source.md"));
        let expected_path = store.record_path(&checkpoint.key);

        store.checkpoint(&checkpoint, &HashSet::new()).unwrap();
        let expected_bytes = fs::metadata(&expected_path).unwrap().len();

        let retention_record = {
            let _guard = store.lock_mutations();
            let scan = store.scan_retention_locked(SystemTime::now()).unwrap();
            assert_eq!(scan.records.len(), 1);
            scan.records.into_iter().next().unwrap()
        };
        assert_eq!(retention_record.path, expected_path);
        assert_eq!(retention_record.bytes, expected_bytes);
        assert!(
            retention_record.recovered.is_none(),
            "retention scans must discard checkpoint text and metadata after validation"
        );
        assert_eq!(retention_record.key, checkpoint.key);

        let recovered = store.recover().unwrap();
        assert!(recovered.issues.is_empty());
        assert_eq!(recovered.records.len(), 1);
        assert_eq!(recovered.records[0].record.text, checkpoint.text);
        assert_eq!(recovered.records[0].record.metadata, checkpoint.metadata);
        assert_eq!(
            retention_record.checkpointed_at,
            recovered.records[0].record.checkpointed_at
        );
        assert!(recovered.records[0].source_conflicted);
    }

    #[test]
    fn recovery_wave_partition_caps_workers_and_input_bytes() {
        const MIB: u64 = 1024 * 1024;
        const BYTE_BUDGET: u64 = 128 * MIB;

        assert_eq!(recovery_worker_limit_for(3), 3);
        assert_eq!(recovery_worker_limit_for(16), 4);
        let expected_worker_limit = std::thread::available_parallelism()
            .map(|available| available.get().min(4))
            .unwrap_or(1);
        assert_eq!(recovery_worker_limit(), expected_worker_limit);

        let count_capped = recovery_wave_ranges_with_worker_limit(&[1; 5], BYTE_BUDGET, 4);
        assert_eq!(count_capped, [0..4, 4..5]);

        let byte_capped = recovery_wave_ranges_with_worker_limit(&[32 * MIB; 5], BYTE_BUDGET, 4);
        assert_eq!(byte_capped, [0..4, 4..5]);
        for range in &byte_capped {
            assert!([32 * MIB; 5][range.clone()].iter().sum::<u64>() <= BYTE_BUDGET);
        }

        let oversized =
            recovery_wave_ranges_with_worker_limit(&[BYTE_BUDGET + 1, 1], BYTE_BUDGET, 4);
        assert_eq!(oversized, [0..1, 1..2]);
    }

    #[test]
    fn batch_parallelizes_small_records_with_an_adaptive_four_worker_cap() {
        let dir = tempfile::tempdir().unwrap();
        let protector = Arc::new(TrackingProtector::default());
        let store = RecoveryStore::new_at(dir.path().join("recovery"), protector.clone()).unwrap();
        let initial: Vec<_> = (0..8)
            .map(|index| checkpoint(&format!("parallel-{index}"), "initial"))
            .collect();
        let initial_tokens: Vec<_> = initial
            .iter()
            .map(|checkpoint| store.current_token(&checkpoint.key))
            .collect();
        let initial_attempts: Vec<_> = initial
            .iter()
            .zip(&initial_tokens)
            .map(|(checkpoint, token)| RecoveryCheckpointAttempt { checkpoint, token })
            .collect();
        let initial_batch = store.checkpoint_batch_if_current(initial_attempts, &HashSet::new());
        assert!(
            initial_batch
                .outcomes
                .iter()
                .all(|outcome| matches!(outcome, CheckpointBatchOutcome::Written))
        );

        protector.reset();
        let updated: Vec<_> = (0..8)
            .map(|index| checkpoint(&format!("parallel-{index}"), "updated"))
            .collect();
        let updated_tokens: Vec<_> = updated
            .iter()
            .map(|checkpoint| store.current_token(&checkpoint.key))
            .collect();
        let updated_attempts: Vec<_> = updated
            .iter()
            .zip(&updated_tokens)
            .map(|(checkpoint, token)| RecoveryCheckpointAttempt { checkpoint, token })
            .collect();
        let batch = store.checkpoint_batch_if_current(updated_attempts, &HashSet::new());
        assert!(
            batch
                .outcomes
                .iter()
                .all(|outcome| matches!(outcome, CheckpointBatchOutcome::Written))
        );
        assert!((1..=4).contains(&protector.protect_peak.load(Ordering::SeqCst)));

        protector.reset();
        let recovered: HashMap<_, _> = store
            .recover()
            .unwrap()
            .records
            .into_iter()
            .map(|entry| (entry.record.key, entry.record.text))
            .collect();
        assert_eq!(recovered.len(), 8);
        for checkpoint in &updated {
            assert_eq!(recovered.get(&checkpoint.key), Some(&checkpoint.text));
        }
        assert!((1..=4).contains(&protector.unprotect_peak.load(Ordering::SeqCst)));
    }

    #[test]
    fn batch_prepares_current_wave_while_pruning_retained_records() {
        let dir = tempfile::tempdir().unwrap();
        let protector = Arc::new(TrackingProtector::default());
        let store = RecoveryStore::new_at(dir.path().join("recovery"), protector.clone()).unwrap();
        let initial: Vec<_> = (0..8)
            .map(|index| checkpoint(&format!("retained-overlap-{index}"), "initial"))
            .collect();
        let initial_tokens: Vec<_> = initial
            .iter()
            .map(|checkpoint| store.current_token(&checkpoint.key))
            .collect();
        let initial_attempts: Vec<_> = initial
            .iter()
            .zip(&initial_tokens)
            .map(|(checkpoint, token)| RecoveryCheckpointAttempt { checkpoint, token })
            .collect();
        let initial_batch = store.checkpoint_batch_if_current(initial_attempts, &HashSet::new());
        assert!(
            initial_batch
                .outcomes
                .iter()
                .all(|outcome| matches!(outcome, CheckpointBatchOutcome::Written))
        );

        protector.reset();
        let updated: Vec<_> = (0..8)
            .map(|index| checkpoint(&format!("overlap-{index}"), "updated"))
            .collect();
        let updated_tokens: Vec<_> = updated
            .iter()
            .map(|checkpoint| store.current_token(&checkpoint.key))
            .collect();
        let updated_attempts: Vec<_> = updated
            .iter()
            .zip(&updated_tokens)
            .map(|(checkpoint, token)| RecoveryCheckpointAttempt { checkpoint, token })
            .collect();
        let batch = store.checkpoint_batch_if_current(updated_attempts, &HashSet::new());

        assert!(
            batch
                .outcomes
                .iter()
                .all(|outcome| matches!(outcome, CheckpointBatchOutcome::Written))
        );
        assert!(
            protector.stages_overlapped.load(Ordering::SeqCst),
            "checkpoint protection should overlap retention decoding"
        );
        assert!(protector.protect_peak.load(Ordering::SeqCst) <= 4);
        assert!(protector.unprotect_peak.load(Ordering::SeqCst) <= 4);
        assert!(protector.combined_peak.load(Ordering::SeqCst) <= 8);
    }

    #[test]
    fn cancellation_during_protection_stops_after_the_current_wave() {
        let dir = tempfile::tempdir().unwrap();
        let protector = Arc::new(BlockingProtector::default());
        let store = RecoveryStore::new_at(dir.path().join("recovery"), protector.clone()).unwrap();
        let checkpoints: Vec<_> = (0..(recovery_worker_limit() + 1))
            .map(|index| checkpoint(&format!("cancel-protect-{index}"), "pending"))
            .collect();
        let tokens: Vec<_> = checkpoints
            .iter()
            .map(|checkpoint| store.current_token(&checkpoint.key))
            .collect();
        let cancellation = Arc::new(AtomicBool::new(false));
        let (started, release) = protector.protect_gate.block_next_wave();

        let receipt = std::thread::scope(|scope| {
            let worker = scope.spawn(|| {
                let attempts = checkpoints.iter().zip(&tokens).map(|(checkpoint, token)| {
                    CancellableRecoveryCheckpointAttempt {
                        checkpoint,
                        token,
                        cancelled: cancellation.as_ref(),
                    }
                });
                store.checkpoint_batch_if_current_cancellable(attempts, &HashSet::new())
            });
            started.recv().unwrap();
            cancellation.store(true, Ordering::Release);
            release.send(()).unwrap();
            worker.join().unwrap()
        });

        assert!(
            receipt
                .outcomes
                .iter()
                .all(|outcome| matches!(outcome, CheckpointBatchOutcome::Superseded))
        );
        assert!(protector.protect_gate.calls.load(Ordering::Acquire) <= recovery_worker_limit());
        for checkpoint in checkpoints {
            assert!(!store.record_path(&checkpoint.key).exists());
        }
    }

    #[test]
    fn cancelling_one_attempt_during_the_first_wave_keeps_other_batch_work_current() {
        let dir = tempfile::tempdir().unwrap();
        let protector = Arc::new(BlockingProtector::default());
        let store = RecoveryStore::new_at(dir.path().join("recovery"), protector.clone()).unwrap();
        let cancelled = AtomicBool::new(false);
        let still_current = AtomicBool::new(false);
        let cancelled_checkpoint = checkpoint("cancelled-first-wave", "discard this checkpoint");
        let current_checkpoint =
            checkpoint("current-first-wave", "retain exact 中文 text \u{1f680}");
        let cancelled_token = store.current_token(&cancelled_checkpoint.key);
        let current_token = store.current_token(&current_checkpoint.key);
        let (started, release) = protector.protect_gate.block_next_wave();

        let receipt = std::thread::scope(|scope| {
            let worker = scope.spawn(|| {
                store.checkpoint_batch_if_current_cancellable(
                    [
                        CancellableRecoveryCheckpointAttempt {
                            checkpoint: &cancelled_checkpoint,
                            token: &cancelled_token,
                            cancelled: &cancelled,
                        },
                        CancellableRecoveryCheckpointAttempt {
                            checkpoint: &current_checkpoint,
                            token: &current_token,
                            cancelled: &still_current,
                        },
                    ],
                    &HashSet::new(),
                )
            });
            started.recv().unwrap();
            cancelled.store(true, Ordering::Release);
            release.send(()).unwrap();
            worker.join().unwrap()
        });

        assert!(matches!(
            receipt.outcomes.as_slice(),
            [
                CheckpointBatchOutcome::Superseded,
                CheckpointBatchOutcome::Written
            ]
        ));
        assert!(!store.record_path(&cancelled_checkpoint.key).exists());
        let restored = store.recover().unwrap().records;
        assert_eq!(restored.len(), 1);
        assert_eq!(restored[0].record.key, current_checkpoint.key);
        assert_eq!(restored[0].record.text, current_checkpoint.text);
    }

    #[test]
    fn cancellation_during_retention_decode_stops_after_the_current_wave() {
        let dir = tempfile::tempdir().unwrap();
        let protector = Arc::new(BlockingProtector::default());
        let store = RecoveryStore::new_at(dir.path().join("recovery"), protector.clone()).unwrap();
        let retained: Vec<_> = (0..(recovery_worker_limit() + 1))
            .map(|index| checkpoint(&format!("cancel-retention-{index}"), "retained"))
            .collect();
        for checkpoint in &retained {
            store.checkpoint(checkpoint, &HashSet::new()).unwrap();
        }
        protector.unprotect_gate.reset_calls();

        let incoming = checkpoint("cancel-retention-incoming", "pending");
        let token = store.current_token(&incoming.key);
        let cancellation = Arc::new(AtomicBool::new(false));
        let (started, release) = protector.unprotect_gate.block_next_wave();

        let receipt = std::thread::scope(|scope| {
            let worker = scope.spawn(|| {
                store.checkpoint_batch_if_current_cancellable(
                    [CancellableRecoveryCheckpointAttempt {
                        checkpoint: &incoming,
                        token: &token,
                        cancelled: cancellation.as_ref(),
                    }],
                    &HashSet::new(),
                )
            });
            started.recv().unwrap();
            cancellation.store(true, Ordering::Release);
            release.send(()).unwrap();
            worker.join().unwrap()
        });

        assert!(matches!(
            receipt.outcomes.as_slice(),
            [CheckpointBatchOutcome::Superseded]
        ));
        assert!(protector.unprotect_gate.calls.load(Ordering::Acquire) <= recovery_worker_limit());
        assert!(!store.record_path(&incoming.key).exists());
    }

    #[test]
    fn cancellation_at_the_publish_boundary_drops_the_temp_unless_the_final_check_passed() {
        let (_dir, store) = test_store();
        let cancelled = Arc::new(AtomicBool::new(false));
        let before_checkpoint = checkpoint("cancel-publish-before", "pending");
        let token = store.current_token(&before_checkpoint.key);
        let (prepared, release) = store.pause_after_checkpoint_prepare_for_test();

        let receipt = std::thread::scope(|scope| {
            let worker = scope.spawn(|| {
                store.checkpoint_batch_if_current_cancellable(
                    [CancellableRecoveryCheckpointAttempt {
                        checkpoint: &before_checkpoint,
                        token: &token,
                        cancelled: cancelled.as_ref(),
                    }],
                    &HashSet::new(),
                )
            });
            prepared.recv().unwrap();
            cancelled.store(true, Ordering::Release);
            release.send(()).unwrap();
            worker.join().unwrap()
        });
        assert!(matches!(
            receipt.outcomes.as_slice(),
            [CheckpointBatchOutcome::Superseded]
        ));
        assert!(!store.record_path(&before_checkpoint.key).exists());

        let (_dir, store) = test_store();
        let cancelled = Arc::new(AtomicBool::new(false));
        let after_checkpoint = checkpoint("cancel-publish-after", "published");
        let token = store.current_token(&after_checkpoint.key);
        let (final_check, release) = store.pause_after_checkpoint_final_check_for_test();

        let receipt = std::thread::scope(|scope| {
            let worker = scope.spawn(|| {
                store.checkpoint_batch_if_current_cancellable(
                    [CancellableRecoveryCheckpointAttempt {
                        checkpoint: &after_checkpoint,
                        token: &token,
                        cancelled: cancelled.as_ref(),
                    }],
                    &HashSet::new(),
                )
            });
            final_check.recv().unwrap();
            cancelled.store(true, Ordering::Release);
            release.send(()).unwrap();
            worker.join().unwrap()
        });
        assert!(matches!(
            receipt.outcomes.as_slice(),
            [CheckpointBatchOutcome::Written]
        ));
        assert!(store.record_path(&after_checkpoint.key).exists());
    }

    #[test]
    fn cancelled_replacement_does_not_fall_back_to_full_retention_decode() {
        let dir = tempfile::tempdir().unwrap();
        let protector = Arc::new(BlockingProtector::default());
        let store = RecoveryStore::new_at(dir.path().join("recovery"), protector.clone()).unwrap();
        let initial = checkpoint("cancelled-replacement", "initial");
        store.checkpoint(&initial, &HashSet::new()).unwrap();
        protector.unprotect_gate.reset_calls();

        let replacement = checkpoint("cancelled-replacement", "replacement");
        let token = store.current_token(&replacement.key);
        let cancelled = Arc::new(AtomicBool::new(false));
        let (prepared, release) = store.pause_after_checkpoint_prepare_for_test();

        let receipt = std::thread::scope(|scope| {
            let worker = scope.spawn(|| {
                store.checkpoint_batch_if_current_cancellable(
                    [CancellableRecoveryCheckpointAttempt {
                        checkpoint: &replacement,
                        token: &token,
                        cancelled: cancelled.as_ref(),
                    }],
                    &HashSet::new(),
                )
            });
            prepared.recv().unwrap();
            cancelled.store(true, Ordering::Release);
            release.send(()).unwrap();
            worker.join().unwrap()
        });

        assert!(matches!(
            receipt.outcomes.as_slice(),
            [CheckpointBatchOutcome::Superseded]
        ));
        assert_eq!(protector.unprotect_gate.calls.load(Ordering::Acquire), 0);
        let restored = store.recover().unwrap().records;
        assert_eq!(restored.len(), 1);
        assert_eq!(restored[0].record.text, initial.text);
    }

    #[test]
    fn partial_cancellation_does_not_trigger_full_retention_fallback() {
        let dir = tempfile::tempdir().unwrap();
        let protector = Arc::new(BlockingProtector::default());
        let store = RecoveryStore::new_at(dir.path().join("recovery"), protector.clone()).unwrap();
        let cancelled_initial = checkpoint("partial-shared", "initial checkpoint");
        let cancelled_initial_token = store.current_token(&cancelled_initial.key);
        store
            .checkpoint_batch_if_current(
                [RecoveryCheckpointAttempt {
                    checkpoint: &cancelled_initial,
                    token: &cancelled_initial_token,
                }],
                &HashSet::new(),
            )
            .outcomes
            .into_iter()
            .for_each(|outcome| assert!(matches!(outcome, CheckpointBatchOutcome::Written)));
        protector.unprotect_gate.reset_calls();

        let cancelled_replacement = checkpoint("partial-shared", "discard replacement");
        let current_replacement = checkpoint("partial-shared", "keep replacement");
        let cancelled_token = store.current_token(&cancelled_replacement.key);
        let current_token = store.current_token(&current_replacement.key);
        let cancelled = AtomicBool::new(true);
        let still_current = AtomicBool::new(false);
        let receipt = store.checkpoint_batch_if_current_cancellable(
            [
                CancellableRecoveryCheckpointAttempt {
                    checkpoint: &cancelled_replacement,
                    token: &cancelled_token,
                    cancelled: &cancelled,
                },
                CancellableRecoveryCheckpointAttempt {
                    checkpoint: &current_replacement,
                    token: &current_token,
                    cancelled: &still_current,
                },
            ],
            &HashSet::new(),
        );

        assert!(matches!(
            receipt.outcomes.as_slice(),
            [
                CheckpointBatchOutcome::Superseded,
                CheckpointBatchOutcome::Written
            ]
        ));
        assert_eq!(protector.unprotect_gate.calls.load(Ordering::Acquire), 0);
        let restored: HashMap<_, _> = store
            .recover()
            .unwrap()
            .records
            .into_iter()
            .map(|entry| (entry.record.key, entry.record.text))
            .collect();
        assert_eq!(
            restored.get(&current_replacement.key),
            Some(&current_replacement.text)
        );
    }

    #[test]
    fn transaction_journal_is_the_cancellation_linearization_point() {
        let (_dir, store) = test_store_with_limits(RecoveryLimits {
            max_records: 1,
            max_record_bytes: 10_000,
            max_total_bytes: 20_000,
            max_age: Duration::from_secs(7 * 24 * 60 * 60),
        });
        let retained = checkpoint("journal-retained", "old checkpoint");
        store.checkpoint(&retained, &HashSet::new()).unwrap();
        let incoming = checkpoint("journal-incoming", "new checkpoint");
        let token = store.current_token(&incoming.key);
        let cancelled = AtomicBool::new(false);
        let (journal_written, release) = store.pause_after_transaction_journal_for_test();

        let receipt = std::thread::scope(|scope| {
            let worker = scope.spawn(|| {
                store.checkpoint_batch_if_current_cancellable(
                    [CancellableRecoveryCheckpointAttempt {
                        checkpoint: &incoming,
                        token: &token,
                        cancelled: &cancelled,
                    }],
                    &HashSet::new(),
                )
            });
            journal_written.recv().unwrap();
            cancelled.store(true, Ordering::Release);
            release.send(()).unwrap();
            worker.join().unwrap()
        });

        assert!(matches!(
            receipt.outcomes.as_slice(),
            [CheckpointBatchOutcome::Written]
        ));
        let restored = store.recover().unwrap().records;
        assert_eq!(restored.len(), 1);
        assert_eq!(restored[0].record.key, incoming.key);
        assert_eq!(restored[0].record.text, incoming.text);
    }

    #[test]
    fn activation_before_eviction_reservation_defers_without_blocking() {
        let (_dir, store) = test_store_with_limits(RecoveryLimits {
            max_records: 1,
            max_record_bytes: 10_000,
            max_total_bytes: 20_000,
            max_age: Duration::from_secs(7 * 24 * 60 * 60),
        });
        let victim = checkpoint("reservation-victim", "must survive");
        store.checkpoint(&victim, &HashSet::new()).unwrap();
        let incoming = checkpoint("reservation-incoming", "must defer");
        let token = store.current_token(&incoming.key);
        let cancelled = AtomicBool::new(false);
        let (journal_written, release) = store.pause_after_transaction_journal_for_test();

        let receipt = std::thread::scope(|scope| {
            let worker = scope.spawn(|| {
                store.checkpoint_batch_if_current_cancellable(
                    [CancellableRecoveryCheckpointAttempt {
                        checkpoint: &incoming,
                        token: &token,
                        cancelled: &cancelled,
                    }],
                    &HashSet::new(),
                )
            });
            journal_written.recv().unwrap();

            let (activated_sender, activated_receiver) = std::sync::mpsc::channel();
            let store = &store;
            let victim_key = &victim.key;
            scope.spawn(move || {
                activated_sender
                    .send(store.activate_and_current_token(victim_key))
                    .unwrap();
            });
            activated_receiver
                .recv_timeout(Duration::from_millis(250))
                .expect("activation must not wait for recovery-root I/O");

            release.send(()).unwrap();
            worker.join().unwrap()
        });

        assert!(matches!(
            receipt.outcomes.as_slice(),
            [CheckpointBatchOutcome::Deferred]
        ));
        assert!(!store.transaction_journal_path().exists());
        assert!(!store.transaction_commit_path().exists());
        assert!(!store.record_path(&incoming.key).exists());
        let restored = store.recover().unwrap().records;
        assert_eq!(restored.len(), 1);
        assert_eq!(restored[0].record.key, victim.key);
        assert_eq!(restored[0].record.text, victim.text);
    }

    #[test]
    fn activation_during_eviction_reservation_is_deferred_until_commit() {
        let (_dir, store) = test_store_with_limits(RecoveryLimits {
            max_records: 1,
            max_record_bytes: 10_000,
            max_total_bytes: 20_000,
            max_age: Duration::from_secs(7 * 24 * 60 * 60),
        });
        let victim = checkpoint("reserved-victim", "old checkpoint");
        store.checkpoint(&victim, &HashSet::new()).unwrap();
        let incoming = checkpoint("reserved-incoming", "new checkpoint");
        let token = store.current_token(&incoming.key);
        let cancelled = AtomicBool::new(false);
        let (reserved, release) = store.pause_after_eviction_reservation_for_test();

        let receipt = std::thread::scope(|scope| {
            let worker = scope.spawn(|| {
                store.checkpoint_batch_if_current_cancellable(
                    [CancellableRecoveryCheckpointAttempt {
                        checkpoint: &incoming,
                        token: &token,
                        cancelled: &cancelled,
                    }],
                    &HashSet::new(),
                )
            });
            reserved.recv().unwrap();

            let (_, protection_deferred) = store.activate_and_current_token(&victim.key);
            assert!(protection_deferred);

            release.send(()).unwrap();
            worker.join().unwrap()
        });

        assert!(matches!(
            receipt.outcomes.as_slice(),
            [CheckpointBatchOutcome::Written]
        ));
        let (_, protection_deferred) = store.activate_and_current_token(&victim.key);
        assert!(!protection_deferred);
        assert!(!store.transaction_journal_path().exists());
        assert!(!store.transaction_commit_path().exists());
    }

    #[test]
    fn cancellation_before_transaction_journal_does_not_publish() {
        let (_dir, store) = test_store_with_limits(RecoveryLimits {
            max_records: 1,
            max_record_bytes: 10_000,
            max_total_bytes: 20_000,
            max_age: Duration::from_secs(7 * 24 * 60 * 60),
        });
        let retained = checkpoint("pre-journal-retained", "old checkpoint");
        store.checkpoint(&retained, &HashSet::new()).unwrap();
        let incoming = checkpoint("pre-journal-incoming", "must not publish");
        let token = store.current_token(&incoming.key);
        let cancelled = AtomicBool::new(false);
        let (journal_pending, release) = store.pause_before_transaction_journal_for_test();

        let receipt = std::thread::scope(|scope| {
            let worker = scope.spawn(|| {
                store.checkpoint_batch_if_current_cancellable(
                    [CancellableRecoveryCheckpointAttempt {
                        checkpoint: &incoming,
                        token: &token,
                        cancelled: &cancelled,
                    }],
                    &HashSet::new(),
                )
            });
            journal_pending.recv().unwrap();
            cancelled.store(true, Ordering::Release);
            release.send(()).unwrap();
            worker.join().unwrap()
        });

        assert!(matches!(
            receipt.outcomes.as_slice(),
            [CheckpointBatchOutcome::Superseded]
        ));
        assert!(!store.transaction_journal_path().exists());
        let restored = store.recover().unwrap().records;
        assert_eq!(restored.len(), 1);
        assert_eq!(restored[0].record.key, retained.key);
        assert_eq!(restored[0].record.text, retained.text);
    }

    #[test]
    fn successful_same_key_replacements_skip_retention_unprotect_and_restore_exact_text() {
        let dir = tempfile::tempdir().unwrap();
        let protector = Arc::new(TrackingProtector::default());
        let store = RecoveryStore::new_at(dir.path().join("recovery"), protector.clone()).unwrap();
        let initial: Vec<_> = (0..4)
            .map(|index| checkpoint(&format!("replacement-{index}"), "initial"))
            .collect();
        let initial_tokens: Vec<_> = initial
            .iter()
            .map(|checkpoint| store.current_token(&checkpoint.key))
            .collect();
        let initial_attempts: Vec<_> = initial
            .iter()
            .zip(&initial_tokens)
            .map(|(checkpoint, token)| RecoveryCheckpointAttempt { checkpoint, token })
            .collect();
        assert!(
            store
                .checkpoint_batch_if_current(initial_attempts, &HashSet::new())
                .outcomes
                .iter()
                .all(|outcome| matches!(outcome, CheckpointBatchOutcome::Written))
        );

        let updated: Vec<_> = (0..4)
            .map(|index| checkpoint(&format!("replacement-{index}"), &format!("updated-{index}")))
            .collect();
        let updated_tokens: Vec<_> = updated
            .iter()
            .map(|checkpoint| store.current_token(&checkpoint.key))
            .collect();
        let updated_attempts: Vec<_> = updated
            .iter()
            .zip(&updated_tokens)
            .map(|(checkpoint, token)| RecoveryCheckpointAttempt { checkpoint, token })
            .collect();
        protector.reset();
        let receipt = store.checkpoint_batch_if_current(updated_attempts, &HashSet::new());

        assert!(
            receipt
                .outcomes
                .iter()
                .all(|outcome| matches!(outcome, CheckpointBatchOutcome::Written))
        );
        assert_eq!(protector.unprotect_calls.load(Ordering::SeqCst), 0);
        let restored: HashMap<_, _> = store
            .recover()
            .unwrap()
            .records
            .into_iter()
            .map(|entry| (entry.record.key, entry.record.text))
            .collect();
        for checkpoint in &updated {
            assert_eq!(restored.get(&checkpoint.key), Some(&checkpoint.text));
        }
    }

    #[test]
    fn replacement_scan_tracks_skipped_descriptor_quota() {
        let (_dir, store) = test_store();
        let retained = checkpoint("descriptor-retained", "initial");
        store.checkpoint(&retained, &HashSet::new()).unwrap();
        let replacement_keys = HashSet::from([retained.key.clone()]);

        let scan = store
            .scan_retention_locked_excluding(SystemTime::now(), &replacement_keys)
            .unwrap();

        assert!(scan.records.is_empty());
        assert_eq!(scan.skipped_replacements.len(), 1);
        let descriptor = &scan.skipped_replacements[0];
        assert_eq!(descriptor.key, retained.key);
        assert_eq!(
            scan.known_record_sizes.get(&descriptor.path),
            Some(&descriptor.bytes)
        );
    }

    #[test]
    fn skipped_replacement_descriptor_cannot_be_evicted() {
        let dir = tempfile::tempdir().unwrap();
        let protector = Arc::new(TrackingProtector::default());
        let store = RecoveryStore::new_at_with_limits(
            dir.path().join("recovery"),
            protector,
            RecoveryLimits {
                max_records: 1,
                max_record_bytes: 10_000,
                max_total_bytes: 20_000,
                max_age: Duration::from_secs(7 * 24 * 60 * 60),
            },
        )
        .unwrap();
        let retained = checkpoint("skipped-retained", "initial");
        store.checkpoint(&retained, &HashSet::new()).unwrap();
        let updated = checkpoint("skipped-retained", "updated");
        let incoming = checkpoint("skipped-incoming", "incoming");
        let updated_token = store.current_token(&updated.key);
        let incoming_token = store.current_token(&incoming.key);

        let receipt = store.checkpoint_batch_if_current(
            [
                RecoveryCheckpointAttempt {
                    checkpoint: &updated,
                    token: &updated_token,
                },
                RecoveryCheckpointAttempt {
                    checkpoint: &incoming,
                    token: &incoming_token,
                },
            ],
            &HashSet::new(),
        );

        assert!(matches!(
            receipt.outcomes.as_slice(),
            [
                CheckpointBatchOutcome::Written,
                CheckpointBatchOutcome::Failed(RecoveryError::QuotaExceeded { .. }),
            ]
        ));
        let records = store.recover().unwrap().records;
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].record.key, updated.key);
        assert_eq!(records[0].record.text, updated.text);
    }

    #[test]
    fn failed_replacement_falls_back_to_full_retention_validation() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("recovery");
        let key = RecoveryKey::for_document_id("fallback-malformed");
        fs::create_dir_all(&root).unwrap();
        fs::write(
            root.join(format!("{}.{}", key.as_str(), RECORD_EXTENSION)),
            b"malformed",
        )
        .unwrap();
        let store = RecoveryStore::new_at(root, Arc::new(PayloadFailingProtector)).unwrap();
        let replacement = checkpoint("fallback-malformed", "would-fail");
        let token = store.current_token(&replacement.key);

        let receipt = store.checkpoint_batch_if_current(
            [RecoveryCheckpointAttempt {
                checkpoint: &replacement,
                token: &token,
            }],
            &HashSet::new(),
        );

        assert!(matches!(
            receipt.outcomes.as_slice(),
            [CheckpointBatchOutcome::Failed(RecoveryError::Protection)]
        ));
        assert!(
            receipt
                .maintenance
                .issues
                .iter()
                .any(|issue| matches!(issue, RecoveryIssue::Malformed { .. }))
        );
    }

    #[test]
    fn batch_keeps_input_order_and_last_same_key_checkpoint_when_workers_finish_out_of_order() {
        let dir = tempfile::tempdir().unwrap();
        let protector = Arc::new(OutOfOrderProtector::default());
        let store = RecoveryStore::new_at(dir.path().join("recovery"), protector.clone()).unwrap();
        let first = checkpoint("out-of-order", "slow-first");
        let latest = checkpoint("out-of-order", "fast-last");
        let first_token = store.current_token(&first.key);
        let latest_token = store.current_token(&latest.key);

        let batch = store.checkpoint_batch_if_current(
            [
                RecoveryCheckpointAttempt {
                    checkpoint: &first,
                    token: &first_token,
                },
                RecoveryCheckpointAttempt {
                    checkpoint: &latest,
                    token: &latest_token,
                },
            ],
            &HashSet::new(),
        );

        assert!(matches!(
            batch.outcomes.as_slice(),
            [
                CheckpointBatchOutcome::Written,
                CheckpointBatchOutcome::Written
            ]
        ));
        assert!(protector.slow_observed_fast.load(Ordering::SeqCst));
        let recovered = store.recover().unwrap().records;
        assert_eq!(recovered.len(), 1);
        assert_eq!(recovered[0].record.text, latest.text);
        assert_eq!(recovered[0].record.metadata, latest.metadata);
    }

    #[test]
    fn batch_preparation_failure_is_certain_and_stale_payloads_are_not_prepared() {
        let dir = tempfile::tempdir().unwrap();
        let store = RecoveryStore::new_at(
            dir.path().join("recovery"),
            Arc::new(PayloadFailingProtector),
        )
        .unwrap();
        let stale = checkpoint("stale-payload", "would-fail stale");
        let failed = checkpoint("failed-payload", "would-fail current");
        let later = checkpoint("later-payload", "writes after failure");
        let stale_token = store.current_token(&stale.key);
        assert!(!store.invalidate_and_delete(&stale.key).unwrap());
        let failed_token = store.current_token(&failed.key);
        let later_token = store.current_token(&later.key);

        let batch = store.checkpoint_batch_if_current(
            [
                RecoveryCheckpointAttempt {
                    checkpoint: &stale,
                    token: &stale_token,
                },
                RecoveryCheckpointAttempt {
                    checkpoint: &failed,
                    token: &failed_token,
                },
                RecoveryCheckpointAttempt {
                    checkpoint: &later,
                    token: &later_token,
                },
            ],
            &HashSet::new(),
        );

        assert!(matches!(
            batch.outcomes.as_slice(),
            [
                CheckpointBatchOutcome::Superseded,
                CheckpointBatchOutcome::Failed(RecoveryError::Protection),
                CheckpointBatchOutcome::Written,
            ]
        ));
        let recovered = store.recover().unwrap().records;
        assert_eq!(recovered.len(), 1);
        assert_eq!(recovered[0].record.key, later.key);
        assert_eq!(recovered[0].record.text, later.text);
    }

    #[test]
    fn batch_skips_a_stale_token_without_blocking_a_current_checkpoint() {
        let (_dir, store) = test_store();
        let stale = checkpoint("batch-stale", "stale text");
        let current = checkpoint("batch-current", "current text");
        let stale_token = store.current_token(&stale.key);
        assert!(!store.invalidate_and_delete(&stale.key).unwrap());
        let current_token = store.current_token(&current.key);

        let batch = store.checkpoint_batch_if_current(
            [
                RecoveryCheckpointAttempt {
                    checkpoint: &stale,
                    token: &stale_token,
                },
                RecoveryCheckpointAttempt {
                    checkpoint: &current,
                    token: &current_token,
                },
            ],
            &HashSet::new(),
        );

        assert!(matches!(
            batch.outcomes.as_slice(),
            [
                CheckpointBatchOutcome::Superseded,
                CheckpointBatchOutcome::Written
            ]
        ));
        let records = store.recover().unwrap().records;
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].record.key, current.key);
        assert_eq!(records[0].record.text, current.text);
    }

    #[test]
    fn batch_updates_its_cached_scan_before_applying_later_quota_eviction() {
        let (_dir, store) = test_store_with_limits(RecoveryLimits {
            max_records: 1,
            max_record_bytes: 10_000,
            max_total_bytes: 20_000,
            max_age: Duration::from_secs(7 * 24 * 60 * 60),
        });
        let first = checkpoint("batch-evict-first", "first text");
        let second = checkpoint("batch-evict-second", "second text");
        let first_token = store.current_token(&first.key);
        let second_token = store.current_token(&second.key);

        let batch = store.checkpoint_batch_if_current(
            [
                RecoveryCheckpointAttempt {
                    checkpoint: &first,
                    token: &first_token,
                },
                RecoveryCheckpointAttempt {
                    checkpoint: &second,
                    token: &second_token,
                },
            ],
            &HashSet::new(),
        );

        assert!(matches!(
            batch.outcomes.as_slice(),
            [
                CheckpointBatchOutcome::Written,
                CheckpointBatchOutcome::Written
            ]
        ));
        assert_eq!(store.retention_scan_count_for_test(), 1);
        let records = store.recover().unwrap().records;
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].record.key, second.key);
        assert_eq!(records[0].record.text, second.text);
    }

    #[test]
    fn batch_does_not_evict_a_store_active_record() {
        let (_dir, store) = test_store_with_limits(RecoveryLimits {
            max_records: 1,
            max_record_bytes: 10_000,
            max_total_bytes: 20_000,
            max_age: Duration::from_secs(7 * 24 * 60 * 60),
        });
        let protected = checkpoint("batch-protected", "must survive");
        let incoming = checkpoint("batch-incoming", "new checkpoint");
        store.checkpoint(&protected, &HashSet::new()).unwrap();
        store.activate_and_current_token(&protected.key);
        let incoming_token = store.current_token(&incoming.key);

        let batch = store.checkpoint_batch_if_current(
            [RecoveryCheckpointAttempt {
                checkpoint: &incoming,
                token: &incoming_token,
            }],
            &HashSet::new(),
        );

        assert!(matches!(
            batch.outcomes.as_slice(),
            [CheckpointBatchOutcome::Failed(
                RecoveryError::QuotaExceeded { .. }
            )]
        ));
        let records = store.recover().unwrap().records;
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].record.key, protected.key);
        assert_eq!(records[0].record.text, protected.text);
    }

    #[test]
    fn cleanup_pending_batch_defers_later_work_after_fallback_reconciles() {
        let (_dir, store) = test_store_with_limits(RecoveryLimits {
            max_records: 2,
            max_record_bytes: 10_000,
            max_total_bytes: 20_000,
            max_age: Duration::from_secs(7 * 24 * 60 * 60),
        });
        let base = checkpoint("batch-base", "base text");
        let first = checkpoint("batch-first", "first text");
        let second = checkpoint("batch-second", "second text");
        let deferred = checkpoint("batch-deferred", "deferred text");
        store.checkpoint(&base, &HashSet::new()).unwrap();
        let first_token = store.current_token(&first.key);
        let second_token = store.current_token(&second.key);
        let deferred_token = store.current_token(&deferred.key);
        store.fail_next_delete_for_test();

        let failed = store.checkpoint_batch_if_current(
            [
                RecoveryCheckpointAttempt {
                    checkpoint: &first,
                    token: &first_token,
                },
                RecoveryCheckpointAttempt {
                    checkpoint: &second,
                    token: &second_token,
                },
                RecoveryCheckpointAttempt {
                    checkpoint: &deferred,
                    token: &deferred_token,
                },
            ],
            &HashSet::new(),
        );

        assert!(matches!(
            failed.outcomes.as_slice(),
            [
                CheckpointBatchOutcome::Written,
                CheckpointBatchOutcome::Written,
                CheckpointBatchOutcome::Deferred,
            ]
        ));
        assert!(
            failed
                .maintenance
                .issues
                .iter()
                .any(|issue| matches!(issue, RecoveryIssue::CleanupPending { .. }))
        );
        assert!(!store.record_path(&deferred.key).exists());
        assert!(!store.transaction_journal_path().exists());

        let retried = store.checkpoint_batch_if_current(
            [
                RecoveryCheckpointAttempt {
                    checkpoint: &second,
                    token: &second_token,
                },
                RecoveryCheckpointAttempt {
                    checkpoint: &deferred,
                    token: &deferred_token,
                },
            ],
            &HashSet::new(),
        );

        assert!(matches!(
            retried.outcomes.as_slice(),
            [
                CheckpointBatchOutcome::Written,
                CheckpointBatchOutcome::Written
            ]
        ));
        assert!(!store.transaction_journal_path().exists());
        let recovered: HashMap<_, _> = store
            .recover()
            .unwrap()
            .records
            .into_iter()
            .map(|entry| (entry.record.key, entry.record.text))
            .collect();
        assert_eq!(recovered.len(), 2);
        assert_eq!(recovered.get(&second.key), Some(&second.text));
        assert_eq!(recovered.get(&deferred.key), Some(&deferred.text));
    }

    #[test]
    fn intentional_save_or_discard_deletes_the_record() {
        let (_dir, store) = test_store();
        let item = checkpoint("delete", "text");
        store.checkpoint(&item, &HashSet::new()).unwrap();

        assert!(store.invalidate_and_delete(&item.key).unwrap());
        assert!(!store.invalidate_and_delete(&item.key).unwrap());
        assert!(store.recover().unwrap().records.is_empty());
    }

    #[test]
    fn begin_retirement_returns_while_a_checkpoint_holds_the_io_lock() {
        let (_dir, store) = test_store();
        let item = checkpoint("retirement-io-lock", "late checkpoint");
        let token = store.current_token(&item.key);
        let (checkpoint_paused, release_checkpoint) =
            store.pause_after_checkpoint_final_check_for_test();
        let (retirement_marker_persisted, release_retirement) =
            store.pause_after_retirement_marker_for_test();
        let store_ref = &store;
        let item_ref = &item;

        let ticket = std::thread::scope(|scope| {
            let worker = scope.spawn(move || {
                store_ref
                    .checkpoint_if_current(item_ref, &HashSet::new(), token)
                    .unwrap()
            });
            checkpoint_paused
                .recv_timeout(Duration::from_secs(1))
                .expect("checkpoint must hold the recovery I/O lock");

            let (ticket_sender, ticket_receiver) = std::sync::mpsc::channel();
            let retirement = scope.spawn(move || {
                ticket_sender
                    .send(store_ref.begin_retirement(&item_ref.key).unwrap())
                    .unwrap();
            });
            let marker_persisted = retirement_marker_persisted
                .recv_timeout(Duration::from_secs(5))
                .is_ok();
            let _ = release_retirement.send(());
            let ticket = marker_persisted
                .then(|| ticket_receiver.recv_timeout(Duration::from_secs(5)).ok())
                .flatten();

            release_checkpoint.send(()).unwrap();
            assert!(matches!(
                worker.join().unwrap(),
                CheckpointOutcome::Written(_)
            ));
            retirement.join().unwrap();
            assert!(
                marker_persisted,
                "retirement marker persistence must not wait for recovery I/O"
            );
            ticket.expect("begin_retirement must not wait for recovery I/O")
        });

        assert!(matches!(
            store.complete_retirement(ticket).unwrap(),
            RetirementCompletion::Retired { .. }
        ));
        assert!(!store.retirement_marker_path_for_key(&item.key).exists());
        assert!(store.recover().unwrap().records.is_empty());
    }

    #[test]
    fn retirement_cleanup_failure_hides_the_canonical_record() {
        let (_dir, store) = test_store();
        let item = checkpoint("retirement-cleanup-failure", "checkpoint");
        store.checkpoint(&item, &HashSet::new()).unwrap();
        let ticket = store.begin_retirement(&item.key).unwrap();
        store.fail_next_delete_for_test();

        assert!(matches!(
            store.complete_retirement(ticket),
            Ok(RetirementCompletion::CleanupPending {
                error: RecoveryError::Io(_)
            })
        ));
        assert!(store.recover().unwrap().records.is_empty());
    }

    #[test]
    fn retirement_rename_failure_keeps_the_marker_hidden_until_cleanup_succeeds() {
        let (_dir, store) = test_store();
        let item = checkpoint("retirement-rename-failure", "checkpoint");
        store.checkpoint(&item, &HashSet::new()).unwrap();
        let ticket = store.begin_retirement(&item.key).unwrap();
        store.fail_next_rename_for_test();

        assert!(matches!(
            store.complete_retirement(ticket.clone()),
            Ok(RetirementCompletion::CleanupPending {
                error: RecoveryError::Io(_)
            })
        ));
        assert!(store.recover().unwrap().records.is_empty());
        assert!(matches!(
            store.complete_retirement(ticket).unwrap(),
            RetirementCompletion::Retired { removed: false }
        ));
    }

    #[test]
    fn failed_marker_cleanup_stays_hidden_and_reports_maintenance_issue() {
        let (_dir, store) = test_store();
        let item = checkpoint("retirement-marker-cleanup", "checkpoint");
        store.checkpoint(&item, &HashSet::new()).unwrap();
        let ticket = store.begin_retirement(&item.key).unwrap();
        store.fail_next_rename_for_test();
        assert!(matches!(
            store.complete_retirement(ticket),
            Ok(RetirementCompletion::CleanupPending {
                error: RecoveryError::Io(_)
            })
        ));

        store.fail_next_delete_for_test();
        let scan = store.recover().unwrap();
        assert!(scan.records.is_empty());
        assert!(scan.issues.iter().any(|issue| matches!(
            issue,
            RecoveryIssue::CleanupPending { path }
                if path == &store.record_path(&item.key)
        )));
        assert!(store.retirement_marker_path_for_key(&item.key).exists());
    }

    #[test]
    fn a_durable_retirement_marker_hides_a_record_across_reopen_and_allows_replacement() {
        let (_dir, store) = test_store();
        let retired = checkpoint("retirement-reopen", "retired text");
        store.checkpoint(&retired, &HashSet::new()).unwrap();
        let ticket = store.begin_retirement(&retired.key).unwrap();
        let root = store.root().to_path_buf();
        assert!(store.retirement_marker_path_for_key(&retired.key).exists());
        drop(ticket);
        drop(store);

        let reopened = RecoveryStore::new_at(root, Arc::new(TestProtector)).unwrap();
        assert!(reopened.recover().unwrap().records.is_empty());

        let replacement = checkpoint("retirement-reopen", "replacement text");
        let token = reopened.current_token(&replacement.key);
        assert!(matches!(
            reopened
                .checkpoint_if_current(&replacement, &HashSet::new(), token)
                .unwrap(),
            CheckpointOutcome::Written(_)
        ));
        let records = reopened.recover().unwrap().records;
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].record.text, replacement.text);
    }

    #[test]
    fn marker_write_failure_leaves_the_original_record_recoverable() {
        let (_dir, store) = test_store();
        let item = checkpoint("retirement-marker-write-failure", "checkpoint");
        store.checkpoint(&item, &HashSet::new()).unwrap();
        store.fail_next_persist_for_test();

        assert!(matches!(
            store.begin_retirement(&item.key),
            Err(RecoveryError::Io(_))
        ));
        assert!(!store.retirement_marker_path_for_key(&item.key).exists());
        let records = store.recover().unwrap().records;
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].record.text, item.text);
    }

    #[test]
    fn corrupt_retirement_marker_fails_recovery_closed_and_preserves_artifacts() {
        let (_dir, store) = test_store();
        let item = checkpoint("corrupt-retirement-marker", "checkpoint");
        store.checkpoint(&item, &HashSet::new()).unwrap();
        let marker = store.retirement_marker_path_for_key(&item.key);
        fs::write(&marker, b"not a retirement marker").unwrap();

        assert!(matches!(
            store.recover(),
            Err(RecoveryError::Serialization(_))
        ));
        assert!(marker.exists());
        assert!(store.record_path(&item.key).exists());
    }

    #[test]
    fn malformed_retirement_marker_filename_fails_recovery_closed() {
        let (_dir, store) = test_store();
        let item = checkpoint("malformed-retirement-name", "checkpoint");
        store.checkpoint(&item, &HashSet::new()).unwrap();
        let marker = store
            .root()
            .join(format!("{RETIREMENT_MARKER_PREFIX}{}", "A".repeat(64)));
        fs::write(
            &marker,
            serde_json::to_vec(&RetirementMarker {
                version: RETIREMENT_MARKER_VERSION,
                keys: vec![item.key.0.clone()],
            })
            .unwrap(),
        )
        .unwrap();

        assert!(matches!(
            store.recover(),
            Err(RecoveryError::Unavailable(
                "recovery retirement marker filename is invalid"
            ))
        ));
        assert!(marker.exists());
        assert!(store.record_path(&item.key).exists());
    }

    #[test]
    fn retirement_marker_path_must_match_decoded_keys_before_reconciliation() {
        let (_dir, store) = test_store();
        let first = checkpoint("retirement-marker-path-first", "first checkpoint");
        let second = checkpoint("retirement-marker-path-second", "second checkpoint");
        store.checkpoint(&first, &HashSet::new()).unwrap();
        store.checkpoint(&second, &HashSet::new()).unwrap();
        let marker = store.retirement_marker_path_for_key(&first.key);
        fs::write(
            &marker,
            serde_json::to_vec(&RetirementMarker {
                version: RETIREMENT_MARKER_VERSION,
                keys: vec![second.key.0.clone()],
            })
            .unwrap(),
        )
        .unwrap();

        assert!(matches!(
            store.recover(),
            Err(RecoveryError::Unavailable(
                "recovery retirement marker keys do not match"
            ))
        ));
        assert!(store.record_path(&first.key).exists());
        assert!(store.record_path(&second.key).exists());
        assert!(marker.exists());
    }

    #[test]
    fn post_persist_marker_sync_failure_is_idempotent_on_exact_retry() {
        let (_dir, store) = test_store();
        let item = checkpoint("retirement-marker-resync", "checkpoint");
        store.checkpoint(&item, &HashSet::new()).unwrap();
        let marker = store.retirement_marker_path_for_key(&item.key);
        store.fail_next_retirement_marker_sync_for_test();

        assert!(matches!(
            store.begin_retirement(&item.key),
            Err(RecoveryError::Io(_))
        ));
        assert!(marker.exists());
        assert!(store.record_path(&item.key).exists());

        let ticket = store.begin_retirement(&item.key).unwrap();
        assert!(matches!(
            store.complete_retirement(ticket).unwrap(),
            RetirementCompletion::Retired { removed: true }
        ));
        assert!(!marker.exists());
        assert!(!store.record_path(&item.key).exists());
    }

    #[test]
    fn mismatched_existing_retirement_marker_is_rejected() {
        let (_dir, store) = test_store();
        let item = checkpoint("retirement-marker-expected", "checkpoint");
        let other = checkpoint("retirement-marker-other", "other checkpoint");
        store.checkpoint(&item, &HashSet::new()).unwrap();
        let marker = store.retirement_marker_path_for_key(&item.key);
        fs::write(
            &marker,
            serde_json::to_vec(&RetirementMarker {
                version: RETIREMENT_MARKER_VERSION,
                keys: vec![other.key.0],
            })
            .unwrap(),
        )
        .unwrap();

        assert!(matches!(
            store.begin_retirement(&item.key),
            Err(RecoveryError::Unavailable(
                "recovery retirement marker keys do not match"
            ))
        ));
        assert!(marker.exists());
        assert!(store.record_path(&item.key).exists());
    }

    #[test]
    fn batch_marker_hides_every_record_across_reopen_and_allows_new_checkpoints() {
        let (_dir, store) = test_store();
        let first = checkpoint("batch-retirement-first", "first retired text");
        let second = checkpoint("batch-retirement-second", "second retired text");
        store.checkpoint(&first, &HashSet::new()).unwrap();
        store.checkpoint(&second, &HashSet::new()).unwrap();
        let batch = store
            .begin_retirements([first.key.clone(), second.key.clone()])
            .unwrap();
        assert!(store.retirement_marker_path(&batch.keys).exists());
        let root = store.root().to_path_buf();
        drop(batch);
        drop(store);

        let reopened = RecoveryStore::new_at(root, Arc::new(TestProtector)).unwrap();
        assert!(reopened.recover().unwrap().records.is_empty());

        let first_current = checkpoint("batch-retirement-first", "first current text");
        let second_current = checkpoint("batch-retirement-second", "second current text");
        let first_token = reopened.current_token(&first_current.key);
        let second_token = reopened.current_token(&second_current.key);
        assert!(matches!(
            reopened
                .checkpoint_if_current(&first_current, &HashSet::new(), first_token)
                .unwrap(),
            CheckpointOutcome::Written(_)
        ));
        assert!(matches!(
            reopened
                .checkpoint_if_current(&second_current, &HashSet::new(), second_token)
                .unwrap(),
            CheckpointOutcome::Written(_)
        ));
        let recovered: HashMap<_, _> = reopened
            .recover()
            .unwrap()
            .records
            .into_iter()
            .map(|record| (record.record.key, record.record.text))
            .collect();
        assert_eq!(recovered.len(), 2);
        assert_eq!(recovered.get(&first_current.key), Some(&first_current.text));
        assert_eq!(
            recovered.get(&second_current.key),
            Some(&second_current.text)
        );
    }

    #[test]
    fn batch_marker_write_failure_leaves_every_record_recoverable() {
        let (_dir, store) = test_store();
        let first = checkpoint("batch-marker-write-first", "first text");
        let second = checkpoint("batch-marker-write-second", "second text");
        store.checkpoint(&first, &HashSet::new()).unwrap();
        store.checkpoint(&second, &HashSet::new()).unwrap();
        store.fail_next_persist_for_test();

        assert!(matches!(
            store.begin_retirements([first.key.clone(), second.key.clone()]),
            Err(RecoveryError::Io(_))
        ));
        let recovered: HashMap<_, _> = store
            .recover()
            .unwrap()
            .records
            .into_iter()
            .map(|record| (record.record.key, record.record.text))
            .collect();
        assert_eq!(recovered.len(), 2);
        assert_eq!(recovered.get(&first.key), Some(&first.text));
        assert_eq!(recovered.get(&second.key), Some(&second.text));
    }

    #[test]
    fn partial_batch_cleanup_keeps_every_marked_record_hidden() {
        let (_dir, store) = test_store();
        let first = checkpoint("batch-cleanup-first", "first text");
        let second = checkpoint("batch-cleanup-second", "second text");
        store.checkpoint(&first, &HashSet::new()).unwrap();
        store.checkpoint(&second, &HashSet::new()).unwrap();
        let batch = store
            .begin_retirements([first.key.clone(), second.key.clone()])
            .unwrap();
        store.fail_next_delete_for_test();
        assert!(matches!(
            store.complete_retirements(batch),
            Ok(RetirementCompletion::CleanupPending {
                error: RecoveryError::Io(_)
            })
        ));

        store.fail_next_delete_for_test();
        let scan = store.recover().unwrap();
        assert!(scan.records.is_empty());
        assert!(
            scan.issues
                .iter()
                .any(|issue| matches!(issue, RecoveryIssue::CleanupPending { .. }))
        );
    }

    #[test]
    fn abandoning_a_failed_retirement_keeps_old_tokens_stale_and_allows_replacement() {
        let (_dir, store) = test_store();
        let previous = checkpoint("abandon-retirement", "previous");
        store.checkpoint(&previous, &HashSet::new()).unwrap();
        let stale_token = store.current_token(&previous.key);
        let ticket = store.begin_retirement(&previous.key).unwrap();
        let replacement = checkpoint("abandon-retirement", "replacement");
        let current_token = store.activate_and_current_token(&replacement.key).0;
        store.fail_next_rename_for_test();

        assert!(matches!(
            store.complete_retirement(ticket.clone()),
            Ok(RetirementCompletion::CleanupPending {
                error: RecoveryError::Io(_)
            })
        ));
        assert!(store.abandon_retirement(&ticket));
        assert!(!store.abandon_retirement(&ticket));
        assert!(matches!(
            store
                .checkpoint_if_current(&replacement, &HashSet::new(), current_token)
                .unwrap(),
            CheckpointOutcome::Written(_)
        ));
        assert!(matches!(
            store
                .checkpoint_if_current(&previous, &HashSet::new(), stale_token)
                .unwrap(),
            CheckpointOutcome::Superseded
        ));
        let records = store.recover().unwrap().records;
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].record.text, replacement.text);
    }

    #[test]
    fn same_key_checkpoint_waits_for_pending_retirement() {
        let (_dir, store) = test_store();
        let previous = checkpoint("retirement-replacement", "previous");
        store.checkpoint(&previous, &HashSet::new()).unwrap();

        let ticket = store.begin_retirement(&previous.key).unwrap();
        let current = checkpoint("retirement-replacement", "current");
        let current_token = store.activate_and_current_token(&current.key).0;
        let blocked = store.checkpoint_batch_if_current(
            [RecoveryCheckpointAttempt {
                checkpoint: &current,
                token: &current_token,
            }],
            &HashSet::new(),
        );
        assert!(matches!(
            blocked.outcomes.as_slice(),
            [CheckpointBatchOutcome::Deferred]
        ));

        assert!(matches!(
            store.complete_retirement(ticket).unwrap(),
            RetirementCompletion::Retired { removed: true }
        ));
        assert!(matches!(
            store
                .checkpoint_if_current(&current, &HashSet::new(), current_token)
                .unwrap(),
            CheckpointOutcome::Written(_)
        ));
        let records = store.recover().unwrap().records;
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].record.text, current.text);
    }

    #[test]
    fn stale_token_cannot_publish_after_two_phase_retirement() {
        let (_dir, store) = test_store();
        let item = checkpoint("two-phase-stale", "must not return");
        let token = store.current_token(&item.key);
        let ticket = store.begin_retirement(&item.key).unwrap();

        assert!(matches!(
            store.complete_retirement(ticket).unwrap(),
            RetirementCompletion::Retired { removed: false }
        ));
        assert!(matches!(
            store
                .checkpoint_if_current(&item, &HashSet::new(), token)
                .unwrap(),
            CheckpointOutcome::Superseded
        ));
        assert!(store.recover().unwrap().records.is_empty());
    }

    #[test]
    fn a_stale_token_cannot_recreate_a_record_after_invalidation() {
        let (_dir, store) = test_store();
        let item = checkpoint("stale", "must not return");
        let token = store.current_token(&item.key);

        assert!(!store.invalidate_and_delete(&item.key).unwrap());
        assert!(matches!(
            store
                .checkpoint_if_current(&item, &HashSet::new(), token)
                .unwrap(),
            CheckpointOutcome::Superseded
        ));
        assert!(store.recover().unwrap().records.is_empty());
    }

    #[test]
    fn invalidation_deletes_a_checkpoint_that_was_written_first() {
        let (_dir, store) = test_store();
        let item = checkpoint("written", "checkpoint");
        let token = store.current_token(&item.key);

        assert!(matches!(
            store
                .checkpoint_if_current(&item, &HashSet::new(), token)
                .unwrap(),
            CheckpointOutcome::Written(_)
        ));
        assert!(store.invalidate_and_delete(&item.key).unwrap());
        assert!(store.recover().unwrap().records.is_empty());
    }

    #[test]
    fn failed_invalidation_cleanup_stays_hidden_after_restart_and_allows_new_work() {
        let (_dir, store) = test_store();
        let item = checkpoint("retire-on-delete-failure", "checkpoint");
        store.checkpoint(&item, &HashSet::new()).unwrap();
        let token = store.current_token(&item.key);
        store.fail_next_delete_for_test();

        assert!(matches!(
            store.invalidate_and_delete(&item.key),
            Err(RecoveryError::Io(_))
        ));
        assert!(store.recover().unwrap().records.is_empty());
        assert!(matches!(
            store
                .checkpoint_if_current(&item, &HashSet::new(), token)
                .unwrap(),
            CheckpointOutcome::Superseded
        ));
        let root = store.root().to_path_buf();
        drop(store);

        let reopened = RecoveryStore::new_at(root, Arc::new(TestProtector)).unwrap();
        assert!(reopened.recover().unwrap().records.is_empty());
        let current = checkpoint("retire-on-delete-failure", "current checkpoint");
        reopened.checkpoint(&current, &HashSet::new()).unwrap();
        let records = reopened.recover().unwrap().records;
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].record.text, current.text);
    }

    #[test]
    fn prune_cleans_retired_artifacts_after_failed_invalidation_cleanup() {
        let (_dir, store) = test_store();
        let item = checkpoint("retired-prune", "checkpoint");
        store.checkpoint(&item, &HashSet::new()).unwrap();
        store.fail_next_delete_for_test();
        assert!(store.invalidate_and_delete(&item.key).is_err());

        store.prune().unwrap();
        assert!(fs::read_dir(store.root()).unwrap().next().is_none());
    }

    #[test]
    fn invalidation_rename_failure_is_visible_but_the_marker_hides_the_record() {
        let (_dir, store) = test_store();
        let item = checkpoint("retire-rename-failure", "checkpoint");
        store.checkpoint(&item, &HashSet::new()).unwrap();
        store.fail_next_rename_for_test();

        assert!(matches!(
            store.invalidate_and_delete(&item.key),
            Err(RecoveryError::Io(_))
        ));
        assert!(store.recover().unwrap().records.is_empty());
    }

    #[test]
    fn replacing_a_checkpoint_for_the_same_key_uses_one_record_of_quota() {
        let (_dir, store) = test_store_with_limits(RecoveryLimits {
            max_records: 1,
            max_record_bytes: 10_000,
            max_total_bytes: 20_000,
            max_age: Duration::from_secs(7 * 24 * 60 * 60),
        });
        let first = checkpoint("replace", "older");
        let latest = checkpoint("replace", "newer");

        store.checkpoint(&first, &HashSet::new()).unwrap();
        store.checkpoint(&latest, &HashSet::new()).unwrap();

        let records = store.recover().unwrap().records;
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].record.key, latest.key);
        assert_eq!(records[0].record.text, latest.text);
    }

    #[test]
    fn same_key_replacement_skips_transaction_when_no_eviction_is_needed() {
        let (_dir, store) = test_store();
        let previous = checkpoint("replace-fast-path", "previous checkpoint");
        let latest = checkpoint("replace-fast-path", "latest checkpoint");
        store.checkpoint(&previous, &HashSet::new()).unwrap();
        store.fail_next_rename_for_test();

        store.checkpoint(&latest, &HashSet::new()).unwrap();

        let records = store.recover().unwrap().records;
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].record.key, latest.key);
        assert_eq!(records[0].record.text, latest.text);
        assert!(!store.transaction_journal_path().exists());
        assert!(!store.transaction_commit_path().exists());
    }

    #[test]
    fn same_key_replacement_persist_failure_keeps_old_checkpoint_recoverable() {
        let (_dir, store) = test_store();
        let previous = checkpoint("replace-persist-failure", "previous checkpoint");
        let latest = checkpoint("replace-persist-failure", "latest checkpoint");
        store.checkpoint(&previous, &HashSet::new()).unwrap();
        store.fail_next_persist_for_test();

        assert!(matches!(
            store.checkpoint(&latest, &HashSet::new()),
            Err(RecoveryError::Io(_))
        ));

        let records = store.recover().unwrap().records;
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].record.key, previous.key);
        assert_eq!(records[0].record.text, previous.text);
        assert!(!store.transaction_journal_path().exists());
        assert!(!store.transaction_commit_path().exists());
    }

    #[test]
    fn same_key_replacement_that_requires_an_eviction_still_uses_a_transaction() {
        let (_dir, store) = test_store_with_limits(RecoveryLimits {
            max_records: 2,
            max_record_bytes: 2_000,
            max_total_bytes: 1_800,
            max_age: Duration::from_secs(7 * 24 * 60 * 60),
        });
        let previous = checkpoint("replace-eviction-target", "previous checkpoint");
        let inactive = checkpoint("replace-eviction-inactive", "inactive checkpoint");
        let latest = checkpoint("replace-eviction-target", &"x".repeat(1_000));
        let start = SystemTime::now();
        store
            .checkpoint_at(&previous, &HashSet::new(), start)
            .unwrap();
        store
            .checkpoint_at(&inactive, &HashSet::new(), start + Duration::from_secs(1))
            .unwrap();
        store.fail_next_rename_for_test();

        assert!(matches!(
            store.checkpoint_at(&latest, &HashSet::new(), start + Duration::from_secs(2)),
            Err(RecoveryError::Io(_))
        ));

        let recovered: HashMap<_, _> = store
            .recover()
            .unwrap()
            .records
            .into_iter()
            .map(|entry| (entry.record.key, entry.record.text))
            .collect();
        assert_eq!(recovered.len(), 2);
        assert_eq!(recovered.get(&previous.key), Some(&previous.text));
        assert_eq!(recovered.get(&inactive.key), Some(&inactive.text));
        assert!(!store.transaction_journal_path().exists());
        assert!(!store.transaction_commit_path().exists());
    }

    #[test]
    fn evicts_the_oldest_inactive_record_after_committing_the_new_checkpoint() {
        let (_dir, store) = test_store_with_limits(RecoveryLimits {
            max_records: 2,
            max_record_bytes: 10_000,
            max_total_bytes: 20_000,
            max_age: Duration::from_secs(7 * 24 * 60 * 60),
        });
        let first = checkpoint("first", "a");
        let second = checkpoint("second", "b");
        let third = checkpoint("third", "c");
        let start = SystemTime::now();
        store.checkpoint_at(&first, &HashSet::new(), start).unwrap();
        store
            .checkpoint_at(
                &second,
                &HashSet::from([second.key.clone()]),
                start + Duration::from_secs(1),
            )
            .unwrap();
        store
            .checkpoint_at(
                &third,
                &HashSet::from([second.key.clone(), third.key.clone()]),
                start + Duration::from_secs(2),
            )
            .unwrap();
        let keys: HashSet<_> = store
            .recover()
            .unwrap()
            .records
            .into_iter()
            .map(|entry| entry.record.key)
            .collect();
        assert_eq!(keys, HashSet::from([second.key.clone(), third.key.clone()]));
        let leftovers: Vec<_> = fs::read_dir(store.root())
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(leftovers.len(), 2);
        assert!(!store.transaction_journal_path().exists());
        assert!(!store.transaction_commit_path().exists());
        assert!(
            leftovers
                .iter()
                .all(|name| name.ends_with(RECORD_EXTENSION) && !name.starts_with(ARTIFACT_PREFIX)),
            "a successful transaction checkpoint must leave only durable recovery records: {leftovers:?}"
        );
    }

    #[test]
    fn store_active_keys_protect_records_missing_from_dispatch_snapshot() {
        let (_dir, store) = test_store_with_limits(RecoveryLimits {
            max_records: 1,
            max_record_bytes: 10_000,
            max_total_bytes: 20_000,
            max_age: Duration::from_secs(7 * 24 * 60 * 60),
        });
        let protected = checkpoint("protected", "must survive");
        let incoming = checkpoint("incoming", "new checkpoint");
        let start = SystemTime::now();
        store
            .checkpoint_at(&protected, &HashSet::new(), start)
            .unwrap();

        // This models a dirty edit that lands after a worker captured its
        // dispatch snapshot but before it acquires the store mutation lock.
        store.activate_and_current_token(&protected.key);
        assert!(matches!(
            store.checkpoint_at(&incoming, &HashSet::new(), start + Duration::from_secs(1)),
            Err(RecoveryError::QuotaExceeded { .. })
        ));
        let records = store.recover().unwrap().records;
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].record.key, protected.key);

        // Intentional invalidation releases active protection. Recreating the
        // record below is a new generation and may be evicted normally.
        assert!(store.invalidate_and_delete(&protected.key).unwrap());
        store
            .checkpoint_at(&protected, &HashSet::new(), start + Duration::from_secs(2))
            .unwrap();
        store
            .checkpoint_at(&incoming, &HashSet::new(), start + Duration::from_secs(3))
            .unwrap();
        let records = store.recover().unwrap().records;
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].record.key, incoming.key);
    }

    #[test]
    fn failed_checkpoint_persist_keeps_planned_inactive_records_recoverable() {
        let (_dir, store) = test_store_with_limits(RecoveryLimits {
            max_records: 2,
            max_record_bytes: 10_000,
            max_total_bytes: 20_000,
            max_age: Duration::from_secs(7 * 24 * 60 * 60),
        });
        let first = checkpoint("first", "first text");
        let second = checkpoint("second", "second text");
        let third = checkpoint("third", "third text");
        let start = SystemTime::now();
        store.checkpoint_at(&first, &HashSet::new(), start).unwrap();
        store
            .checkpoint_at(&second, &HashSet::new(), start + Duration::from_secs(1))
            .unwrap();

        store.fail_next_persist_for_test();
        let error = store
            .checkpoint_at(&third, &HashSet::new(), start + Duration::from_secs(2))
            .unwrap_err();

        assert!(matches!(error, RecoveryError::Io(_)));
        let records = store.recover().unwrap().records;
        let recovered: HashMap<_, _> = records
            .into_iter()
            .map(|entry| (entry.record.key, entry.record.text))
            .collect();
        assert_eq!(recovered.len(), 2);
        assert_eq!(recovered.get(&first.key), Some(&first.text));
        assert_eq!(recovered.get(&second.key), Some(&second.text));
        assert!(!store.record_path(&third.key).exists());

        let leftovers: Vec<_> = fs::read_dir(store.root())
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(
            leftovers.len(),
            2,
            "unexpected recovery leftovers: {leftovers:?}"
        );
        assert!(
            leftovers
                .iter()
                .all(|name| !name.starts_with(".markturbo-recovery-"))
        );
    }

    #[test]
    fn eviction_cleanup_failure_hides_evicted_record_without_exposing_over_quota_records() {
        let (_dir, store) = test_store_with_limits(RecoveryLimits {
            max_records: 2,
            max_record_bytes: 10_000,
            max_total_bytes: 20_000,
            max_age: Duration::from_secs(7 * 24 * 60 * 60),
        });
        let first = checkpoint("first", "first");
        let second = checkpoint("second", "second");
        let third = checkpoint("third", "third");
        let start = SystemTime::now();
        store.checkpoint_at(&first, &HashSet::new(), start).unwrap();
        store
            .checkpoint_at(&second, &HashSet::new(), start + Duration::from_secs(1))
            .unwrap();
        let before_eviction = store.recover().unwrap().records;
        let first_at = before_eviction
            .iter()
            .find(|record| record.record.key == first.key)
            .unwrap()
            .record
            .checkpointed_at;
        let second_at = before_eviction
            .iter()
            .find(|record| record.record.key == second.key)
            .unwrap()
            .record
            .checkpointed_at;
        assert!(first_at < second_at);
        store.fail_next_delete_for_test();

        let receipt = store
            .checkpoint_at(&third, &HashSet::new(), start + Duration::from_secs(2))
            .unwrap();
        assert!(
            receipt
                .maintenance
                .issues
                .iter()
                .any(|issue| matches!(issue, RecoveryIssue::CleanupPending { .. }))
        );
        store.fail_next_delete_for_test();
        let scan = store.recover().unwrap();
        assert!(
            scan.issues
                .iter()
                .any(|issue| matches!(issue, RecoveryIssue::CleanupPending { .. }))
        );
        let keys: HashSet<_> = scan
            .records
            .into_iter()
            .map(|entry| entry.record.key)
            .collect();
        assert_eq!(keys, HashSet::from([second.key, third.key]));
    }

    #[test]
    fn unresolved_eviction_cleanup_blocks_later_checkpoints_until_it_can_reconcile() {
        let (_dir, store) = test_store_with_limits(RecoveryLimits {
            max_records: 2,
            max_record_bytes: 10_000,
            max_total_bytes: 20_000,
            max_age: Duration::from_secs(7 * 24 * 60 * 60),
        });
        let first = checkpoint("blocked-first", "first");
        let second = checkpoint("blocked-second", "second");
        let third = checkpoint("blocked-third", "third");
        let fourth = checkpoint("blocked-fourth", "fourth");
        let start = SystemTime::now();
        store.checkpoint_at(&first, &HashSet::new(), start).unwrap();
        store
            .checkpoint_at(&second, &HashSet::new(), start + Duration::from_secs(1))
            .unwrap();
        store.fail_next_delete_for_test();
        let receipt = store
            .checkpoint_at(&third, &HashSet::new(), start + Duration::from_secs(2))
            .unwrap();
        assert!(
            receipt
                .maintenance
                .issues
                .iter()
                .any(|issue| matches!(issue, RecoveryIssue::CleanupPending { .. }))
        );

        store.fail_next_delete_for_test();
        assert!(
            store
                .checkpoint_at(&fourth, &HashSet::new(), start + Duration::from_secs(3))
                .is_err()
        );
        assert!(!store.record_path(&fourth.key).exists());

        let records = store.recover().unwrap().records;
        assert_eq!(records.len(), 2);
        assert!(records.iter().any(|record| record.record.key == third.key));
    }

    fn transaction_for_test(
        target: &Path,
        new_staged: &Path,
        staged: Vec<(&Path, &Path)>,
    ) -> RecoveryTransaction {
        RecoveryTransaction {
            version: TRANSACTION_VERSION,
            target_name: recovery_filename(target).unwrap(),
            new_staged_name: recovery_filename(new_staged).unwrap(),
            staged: staged
                .into_iter()
                .map(|(original, staged)| RecoveryTransactionEntry {
                    original_name: recovery_filename(original).unwrap(),
                    staged_name: recovery_filename(staged).unwrap(),
                })
                .collect(),
        }
    }

    #[test]
    fn rollback_journal_cleanup_preserves_expired_target_and_blocks_checkpoint() {
        let (_dir, store) = test_store();
        let previous = checkpoint("rollback-blocked", "previous checkpoint");
        let latest = checkpoint("rollback-blocked", "latest checkpoint");
        write_record_at(
            &store,
            &previous,
            SystemTime::now() - RecoveryLimits::default().max_age - Duration::from_secs(1),
        );
        let original = store.record_path(&previous.key);
        let staged = store
            .root()
            .join(".markturbo-recovery-stage-d000000000000000");
        let new_staged = store
            .root()
            .join(".markturbo-recovery-new-e000000000000000");
        fs::rename(&original, &staged).unwrap();
        let transaction = transaction_for_test(&original, &new_staged, vec![(&original, &staged)]);
        store.write_transaction_journal(&transaction).unwrap();

        store.fail_next_delete_for_test();
        let maintenance = store.prune().unwrap();
        assert_eq!(maintenance.removed_expired, 0);
        assert!(maintenance.issues.iter().any(|issue| matches!(
            issue,
            RecoveryIssue::CleanupPending { path }
                if path == &store.transaction_journal_path()
        )));
        assert!(
            maintenance
                .issues
                .iter()
                .any(|issue| matches!(issue, RecoveryIssue::Expired { path } if path == &original))
        );
        assert!(original.exists());
        let before = fs::read(&original).unwrap();

        store.fail_next_delete_for_test();
        assert!(matches!(
            store.checkpoint(&latest, &HashSet::new()),
            Err(RecoveryError::Unavailable(
                "recovery transaction cleanup is pending"
            ))
        ));
        assert_eq!(fs::read(&original).unwrap(), before);
        assert!(store.transaction_journal_path().exists());

        let scan = store.recover().unwrap();
        assert!(scan.records.is_empty());
        assert!(
            scan.issues
                .iter()
                .any(|issue| matches!(issue, RecoveryIssue::Expired { path } if path == &original))
        );
        assert!(original.exists());
        assert!(!store.transaction_journal_path().exists());
        store.prune().unwrap();
        assert!(!original.exists());
        store.checkpoint(&latest, &HashSet::new()).unwrap();
        assert_eq!(store.recover().unwrap().records[0].record.text, latest.text);
    }

    #[test]
    fn committed_cleanup_pending_blocks_retirement_target_mutation() {
        let (_dir, store) = test_store_with_limits(RecoveryLimits {
            max_records: 2,
            max_record_bytes: 10_000,
            max_total_bytes: 20_000,
            max_age: Duration::from_secs(7 * 24 * 60 * 60),
        });
        let first = checkpoint("retirement-block-first", "first checkpoint");
        let second = checkpoint("retirement-block-second", "second checkpoint");
        let third = checkpoint("retirement-block-third", "third checkpoint");
        let start = SystemTime::now();
        store.checkpoint_at(&first, &HashSet::new(), start).unwrap();
        store
            .checkpoint_at(&second, &HashSet::new(), start + Duration::from_secs(1))
            .unwrap();
        store.fail_next_delete_for_test();
        let receipt = store
            .checkpoint_at(&third, &HashSet::new(), start + Duration::from_secs(2))
            .unwrap();
        assert!(
            receipt
                .maintenance
                .issues
                .iter()
                .any(|issue| matches!(issue, RecoveryIssue::CleanupPending { .. }))
        );

        let target = store.record_path(&third.key);
        let before = fs::read(&target).unwrap();
        let ticket = store.begin_retirement(&third.key).unwrap();
        let marker = store.retirement_marker_path_for_key(&third.key);
        store.fail_next_delete_for_test();
        assert!(matches!(
            store.complete_retirement(ticket),
            Ok(RetirementCompletion::CleanupPending {
                error: RecoveryError::Unavailable("recovery transaction cleanup is pending")
            })
        ));
        assert_eq!(fs::read(&target).unwrap(), before);
        assert!(marker.exists());
        assert!(store.transaction_journal_path().exists());
        assert!(
            fs::read_dir(store.root())
                .unwrap()
                .all(|entry| !is_retired_path(&entry.unwrap().path()))
        );

        store.fail_next_delete_for_test();
        let pending = store.recover().unwrap();
        assert_eq!(pending.records.len(), 1);
        assert_eq!(pending.records[0].record.key, second.key);
        assert!(
            pending
                .issues
                .iter()
                .any(|issue| matches!(issue, RecoveryIssue::CleanupPending { .. }))
        );
        assert_eq!(fs::read(&target).unwrap(), before);
        assert!(target.exists());
        assert!(marker.exists());
        assert!(store.transaction_journal_path().exists());

        let scan = store.recover().unwrap();
        assert_eq!(scan.records.len(), 1);
        assert_eq!(scan.records[0].record.key, second.key);
        assert!(!target.exists());
        assert!(!marker.exists());
        assert!(!store.transaction_journal_path().exists());
    }

    #[test]
    fn transaction_journal_rejects_marker_names_in_data_roles() {
        let (_dir, store) = test_store();
        let target = store.record_path(&RecoveryKey::for_document_id("protocol-target"));
        let original = store.record_path(&RecoveryKey::for_document_id("protocol-original"));
        let valid_new = store
            .root()
            .join(".markturbo-recovery-new-1000000000000000");
        let valid_stage = store
            .root()
            .join(".markturbo-recovery-stage-2000000000000000");
        let retirement_marker =
            store.retirement_marker_path_for_key(&RecoveryKey::for_document_id("protocol-marker"));
        let forbidden = [
            store.transaction_journal_path(),
            store.transaction_commit_path(),
            retirement_marker,
        ];

        for forbidden in forbidden {
            let invalid_new =
                transaction_for_test(&target, &forbidden, vec![(&original, &valid_stage)]);
            fs::write(
                store.transaction_journal_path(),
                serde_json::to_vec(&invalid_new).unwrap(),
            )
            .unwrap();
            assert!(matches!(
                store.recover(),
                Err(RecoveryError::Unavailable(
                    "recovery transaction journal is unsupported"
                ))
            ));

            let invalid_stage =
                transaction_for_test(&target, &valid_new, vec![(&original, &forbidden)]);
            fs::write(
                store.transaction_journal_path(),
                serde_json::to_vec(&invalid_stage).unwrap(),
            )
            .unwrap();
            assert!(matches!(
                store.recover(),
                Err(RecoveryError::Unavailable(
                    "recovery transaction journal is unsupported"
                ))
            ));
        }
    }

    #[test]
    fn transaction_artifact_grammar_requires_role_and_lowercase_hex_id() {
        assert!(is_transaction_artifact_name(
            ".markturbo-recovery-new-0123456789abcdef",
            "new"
        ));
        assert!(is_transaction_artifact_name(
            ".markturbo-recovery-stage-fedcba9876543210",
            "stage"
        ));
        assert!(!is_transaction_artifact_name(
            ".markturbo-recovery-stage-0123456789abcdef",
            "new"
        ));
        assert!(!is_transaction_artifact_name(
            ".markturbo-recovery-new-0123456789abcde",
            "new"
        ));
        assert!(!is_transaction_artifact_name(
            ".markturbo-recovery-new-0123456789abcdeF",
            "new"
        ));
        assert!(!is_transaction_artifact_name(
            ".markturbo-recovery-new-0123456789abcdef0",
            "new"
        ));
    }

    #[test]
    fn protocol_json_rejects_unknown_fields() {
        let (_dir, store) = test_store();
        let target = store.record_path(&RecoveryKey::for_document_id("unknown-field-target"));
        let original = store.record_path(&RecoveryKey::for_document_id("unknown-field-original"));
        let new_staged = store
            .root()
            .join(".markturbo-recovery-new-3000000000000000");
        let staged = store
            .root()
            .join(".markturbo-recovery-stage-4000000000000000");
        let transaction = transaction_for_test(&target, &new_staged, vec![(&original, &staged)]);
        let mut journal = serde_json::to_value(&transaction).unwrap();
        journal
            .as_object_mut()
            .unwrap()
            .insert("unknown".to_owned(), serde_json::Value::Bool(true));
        fs::write(
            store.transaction_journal_path(),
            serde_json::to_vec(&journal).unwrap(),
        )
        .unwrap();
        assert!(matches!(
            store.recover(),
            Err(RecoveryError::Unavailable(
                "recovery transaction journal is invalid"
            ))
        ));

        let mut journal = serde_json::to_value(&transaction).unwrap();
        journal["staged"][0]["unknown"] = serde_json::Value::Bool(true);
        fs::write(
            store.transaction_journal_path(),
            serde_json::to_vec(&journal).unwrap(),
        )
        .unwrap();
        assert!(matches!(
            store.recover(),
            Err(RecoveryError::Unavailable(
                "recovery transaction journal is invalid"
            ))
        ));

        fs::remove_file(store.transaction_journal_path()).unwrap();
        let item = checkpoint("unknown-retirement-field", "checkpoint");
        store.checkpoint(&item, &HashSet::new()).unwrap();
        let marker = store.retirement_marker_path_for_key(&item.key);
        fs::write(
            &marker,
            serde_json::to_vec(&serde_json::json!({
                "version": RETIREMENT_MARKER_VERSION,
                "keys": [item.key.as_str()],
                "unknown": true,
            }))
            .unwrap(),
        )
        .unwrap();
        assert!(matches!(
            store.recover(),
            Err(RecoveryError::Serialization(_))
        ));
        assert!(store.record_path(&item.key).exists());
    }

    #[test]
    fn transaction_journal_creation_does_not_clobber_an_existing_journal() {
        let (_dir, store) = test_store();
        let first_target = store.record_path(&RecoveryKey::for_document_id("first-target"));
        let first_original = store.record_path(&RecoveryKey::for_document_id("first-original"));
        let first_new = store
            .root()
            .join(".markturbo-recovery-new-5000000000000000");
        let first_stage = store
            .root()
            .join(".markturbo-recovery-stage-6000000000000000");
        let first = transaction_for_test(
            &first_target,
            &first_new,
            vec![(&first_original, &first_stage)],
        );
        store.write_transaction_journal(&first).unwrap();
        let original_bytes = fs::read(store.transaction_journal_path()).unwrap();

        let second_target = store.record_path(&RecoveryKey::for_document_id("second-target"));
        let second_original = store.record_path(&RecoveryKey::for_document_id("second-original"));
        let second_new = store
            .root()
            .join(".markturbo-recovery-new-7000000000000000");
        let second_stage = store
            .root()
            .join(".markturbo-recovery-stage-8000000000000000");
        let second = transaction_for_test(
            &second_target,
            &second_new,
            vec![(&second_original, &second_stage)],
        );

        assert!(matches!(
            store.write_transaction_journal(&second),
            Err(RecoveryError::Io(error)) if error.kind() == io::ErrorKind::AlreadyExists
        ));
        assert_eq!(
            fs::read(store.transaction_journal_path()).unwrap(),
            original_bytes
        );
    }

    #[test]
    fn presence_permission_denied_is_not_treated_as_absence() {
        let (_dir, store) = test_store();
        let journal = store.transaction_journal_path();
        store.fail_next_presence_for_test(journal);
        assert!(matches!(
            store.recover(),
            Err(RecoveryError::Io(error)) if error.kind() == io::ErrorKind::PermissionDenied
        ));

        for probe in ["target", "commit", "new", "original", "stage"] {
            let (_dir, store) = test_store();
            let original_checkpoint = checkpoint("presence-original", "checkpoint");
            store
                .checkpoint(&original_checkpoint, &HashSet::new())
                .unwrap();
            let target = store.record_path(&RecoveryKey::for_document_id("presence-target"));
            let original = store.record_path(&original_checkpoint.key);
            let new_staged = store
                .root()
                .join(".markturbo-recovery-new-9000000000000000");
            let staged = store
                .root()
                .join(".markturbo-recovery-stage-a000000000000000");
            let transaction =
                transaction_for_test(&target, &new_staged, vec![(&original, &staged)]);
            store.write_transaction_journal(&transaction).unwrap();
            let probe_path = match probe {
                "target" => target,
                "commit" => store.transaction_commit_path(),
                "new" => new_staged,
                "original" => original,
                "stage" => staged,
                _ => unreachable!(),
            };
            store.fail_next_presence_for_test(probe_path);
            assert!(matches!(
                store.recover(),
                Err(RecoveryError::Io(error)) if error.kind() == io::ErrorKind::PermissionDenied
            ));
        }

        let (_dir, store) = test_store();
        let item = checkpoint("presence-retirement", "checkpoint");
        store.checkpoint(&item, &HashSet::new()).unwrap();
        let ticket = store.begin_retirement(&item.key).unwrap();
        store.fail_next_presence_for_test(store.retirement_marker_path_for_key(&item.key));
        assert!(matches!(
            store.recover(),
            Err(RecoveryError::Io(error)) if error.kind() == io::ErrorKind::PermissionDenied
        ));

        let retired = store
            .root()
            .join(".markturbo-recovery-retired-0000000000000000");
        store.fail_next_presence_for_test(retired);
        assert!(matches!(
            store.complete_retirement(ticket),
            Ok(RetirementCompletion::CleanupPending {
                error: RecoveryError::Io(error)
            }) if error.kind() == io::ErrorKind::PermissionDenied
        ));
    }

    #[test]
    fn uncommitted_transaction_restores_staged_old_records() {
        let (_dir, store) = test_store();
        let first = checkpoint("transaction-old", "old checkpoint");
        let replacement = checkpoint("transaction-new", "new checkpoint");
        store.checkpoint(&first, &HashSet::new()).unwrap();
        let original = store.record_path(&first.key);
        let staged = store
            .root()
            .join(".markturbo-recovery-stage-0000000000000001");
        let new_staged = store
            .root()
            .join(".markturbo-recovery-new-0000000000000002");
        fs::rename(&original, &staged).unwrap();
        let transaction = transaction_for_test(
            &store.record_path(&replacement.key),
            &new_staged,
            vec![(&original, &staged)],
        );
        store.write_transaction_journal(&transaction).unwrap();

        let records = store.recover().unwrap().records;
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].record.text, first.text);
        assert!(original.exists());
        assert!(!staged.exists());
        assert!(!store.transaction_journal_path().exists());
    }

    #[test]
    fn committed_transaction_finishes_deferred_cleanup() {
        let (_dir, store) = test_store();
        let first = checkpoint("transaction-evicted", "old checkpoint");
        let latest = checkpoint("transaction-current", "latest checkpoint");
        store.checkpoint(&first, &HashSet::new()).unwrap();
        store.checkpoint(&latest, &HashSet::new()).unwrap();
        let original = store.record_path(&first.key);
        let staged = store
            .root()
            .join(".markturbo-recovery-stage-0000000000000003");
        let new_staged = store
            .root()
            .join(".markturbo-recovery-new-0000000000000004");
        fs::rename(&original, &staged).unwrap();
        let transaction = transaction_for_test(
            &store.record_path(&latest.key),
            &new_staged,
            vec![(&original, &staged)],
        );
        store.write_transaction_journal(&transaction).unwrap();
        store.write_transaction_commit_marker().unwrap();

        let records = store.recover().unwrap().records;
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].record.text, latest.text);
        assert!(!staged.exists());
        assert!(!store.transaction_journal_path().exists());
        assert!(!store.transaction_commit_path().exists());
    }

    #[test]
    fn published_transaction_without_commit_marker_finishes_cleanup_after_restart() {
        let (_dir, store) = test_store();
        let first = checkpoint("transaction-inferred-old", "old checkpoint");
        let latest = checkpoint("transaction-inferred-latest", "latest checkpoint");
        store.checkpoint(&first, &HashSet::new()).unwrap();
        store.checkpoint(&latest, &HashSet::new()).unwrap();
        let original = store.record_path(&first.key);
        let staged = store
            .root()
            .join(".markturbo-recovery-stage-0000000000000005");
        let new_staged = store
            .root()
            .join(".markturbo-recovery-new-0000000000000006");
        fs::rename(&original, &staged).unwrap();
        let transaction = transaction_for_test(
            &store.record_path(&latest.key),
            &new_staged,
            vec![(&original, &staged)],
        );
        store.write_transaction_journal(&transaction).unwrap();

        let records = store.recover().unwrap().records;
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].record.text, latest.text);
        assert!(!staged.exists());
        assert!(!store.transaction_journal_path().exists());
        assert!(!store.transaction_commit_path().exists());
    }

    #[test]
    fn same_key_publish_before_commit_marker_keeps_the_new_checkpoint() {
        let (_dir, store) = test_store();
        let previous = checkpoint("transaction-same-key", "previous checkpoint");
        let latest = checkpoint("transaction-same-key", "latest checkpoint");
        store.checkpoint(&previous, &HashSet::new()).unwrap();
        let original = store.record_path(&previous.key);
        let staged = store
            .root()
            .join(".markturbo-recovery-stage-0000000000000007");
        let new_staged = store
            .root()
            .join(".markturbo-recovery-new-0000000000000008");
        fs::rename(&original, &staged).unwrap();
        write_record_at(&store, &latest, SystemTime::now());
        let transaction = transaction_for_test(&original, &new_staged, vec![(&original, &staged)]);
        store.write_transaction_journal(&transaction).unwrap();

        let records = store.recover().unwrap().records;
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].record.text, latest.text);
        assert!(!staged.exists());
        assert!(!store.transaction_journal_path().exists());
    }

    #[test]
    fn invalidation_reconciles_a_published_same_key_transaction_before_retiring_it() {
        let (_dir, store) = test_store();
        let previous = checkpoint("transaction-invalidation", "previous checkpoint");
        let latest = checkpoint("transaction-invalidation", "latest checkpoint");
        store.checkpoint(&previous, &HashSet::new()).unwrap();
        let original = store.record_path(&previous.key);
        let staged = store
            .root()
            .join(".markturbo-recovery-stage-0000000000000009");
        let new_staged = store
            .root()
            .join(".markturbo-recovery-new-000000000000000a");
        fs::rename(&original, &staged).unwrap();
        write_record_at(&store, &latest, SystemTime::now());
        let transaction = transaction_for_test(&original, &new_staged, vec![(&original, &staged)]);
        store.write_transaction_journal(&transaction).unwrap();

        assert!(store.invalidate_and_delete(&latest.key).unwrap());
        let root = store.root().to_path_buf();
        drop(store);

        let reopened = RecoveryStore::new_at(root, Arc::new(TestProtector)).unwrap();
        assert!(reopened.recover().unwrap().records.is_empty());
        assert!(!staged.exists());
        assert!(!reopened.transaction_journal_path().exists());
    }

    #[test]
    fn committed_transaction_with_missing_published_record_is_fatal() {
        let (_dir, store) = test_store();
        let previous = checkpoint("transaction-commit-missing", "previous checkpoint");
        let latest = checkpoint("transaction-commit-target", "latest checkpoint");
        store.checkpoint(&previous, &HashSet::new()).unwrap();
        let original = store.record_path(&previous.key);
        let staged = store
            .root()
            .join(".markturbo-recovery-stage-000000000000000b");
        let new_staged = store
            .root()
            .join(".markturbo-recovery-new-000000000000000c");
        fs::rename(&original, &staged).unwrap();
        let transaction = transaction_for_test(
            &store.record_path(&latest.key),
            &new_staged,
            vec![(&original, &staged)],
        );
        store.write_transaction_journal(&transaction).unwrap();
        store.write_transaction_commit_marker().unwrap();

        assert!(matches!(
            store.recover(),
            Err(RecoveryError::Unavailable(
                "recovery transaction state is uncertain"
            ))
        ));
        assert!(!original.exists());
        assert!(staged.exists());
        assert!(store.transaction_journal_path().exists());
        assert!(store.transaction_commit_path().exists());
    }

    #[test]
    fn uncommitted_same_key_replacement_restores_the_previous_checkpoint() {
        let (_dir, store) = test_store();
        let previous = checkpoint("transaction-replace", "previous checkpoint");
        store.checkpoint(&previous, &HashSet::new()).unwrap();
        let original = store.record_path(&previous.key);
        let staged = store
            .root()
            .join(".markturbo-recovery-stage-000000000000000d");
        let new_staged = store
            .root()
            .join(".markturbo-recovery-new-000000000000000e");
        fs::rename(&original, &staged).unwrap();
        let transaction = transaction_for_test(&original, &new_staged, vec![(&original, &staged)]);
        store.write_transaction_journal(&transaction).unwrap();

        let records = store.recover().unwrap().records;
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].record.text, previous.text);
        assert!(original.exists());
        assert!(!staged.exists());
    }

    #[test]
    fn all_active_records_block_checkpointing_without_eviction() {
        let (_dir, store) = test_store_with_limits(RecoveryLimits {
            max_records: 2,
            max_record_bytes: 10_000,
            max_total_bytes: 20_000,
            max_age: Duration::from_secs(7 * 24 * 60 * 60),
        });
        let first = checkpoint("first", "a");
        let second = checkpoint("second", "b");
        let third = checkpoint("third", "c");
        let start = SystemTime::now();
        store
            .checkpoint_at(&first, &HashSet::from([first.key.clone()]), start)
            .unwrap();
        store
            .checkpoint_at(
                &second,
                &HashSet::from([first.key.clone(), second.key.clone()]),
                start + Duration::from_secs(1),
            )
            .unwrap();

        let error = store
            .checkpoint_at(
                &third,
                &HashSet::from([first.key.clone(), second.key.clone(), third.key.clone()]),
                start + Duration::from_secs(2),
            )
            .unwrap_err();
        assert!(matches!(error, RecoveryError::QuotaExceeded { .. }));
        let keys: HashSet<_> = store
            .recover()
            .unwrap()
            .records
            .into_iter()
            .map(|entry| entry.record.key)
            .collect();
        assert_eq!(keys, HashSet::from([first.key, second.key]));
    }

    #[test]
    fn total_byte_limit_evicts_after_committing_the_next_record() {
        let first = checkpoint("first", "same size");
        let at = SystemTime::UNIX_EPOCH
            + Duration::from_secs(
                SystemTime::now()
                    .duration_since(SystemTime::UNIX_EPOCH)
                    .unwrap()
                    .as_secs(),
            );
        let bytes = TestProtector
            .protect(
                &serde_json::to_vec(&DiskRecord::from_checkpoint(&first, at).unwrap()).unwrap(),
            )
            .unwrap()
            .len() as u64;
        let (_dir, store) = test_store_with_limits(RecoveryLimits {
            max_records: 50,
            max_record_bytes: bytes + 1,
            max_total_bytes: bytes * 2 - 1,
            max_age: Duration::from_secs(7 * 24 * 60 * 60),
        });
        let second = checkpoint("second", "same size");

        store.checkpoint_at(&first, &HashSet::new(), at).unwrap();
        store
            .checkpoint_at(&second, &HashSet::new(), at + Duration::from_secs(1))
            .unwrap();

        let records = store.recover().unwrap().records;
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].record.key, second.key);
    }

    #[test]
    fn enforces_the_per_record_limit_before_writing() {
        let (_dir, store) = test_store_with_limits(RecoveryLimits {
            max_records: 50,
            max_record_bytes: 100,
            max_total_bytes: 1_000,
            max_age: Duration::from_secs(7 * 24 * 60 * 60),
        });
        let error = store
            .checkpoint(&checkpoint("large", &"x".repeat(1_000)), &HashSet::new())
            .unwrap_err();
        assert!(matches!(error, RecoveryError::OversizedCheckpoint { .. }));
        assert!(fs::read_dir(store.root()).unwrap().next().is_none());
    }

    #[test]
    fn atomic_checkpoint_leaves_no_temp_file() {
        let (_dir, store) = test_store();
        let item = checkpoint("atomic", "text");
        store.checkpoint(&item, &HashSet::new()).unwrap();

        let names: Vec<_> = fs::read_dir(store.root())
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(names.len(), 1, "unexpected recovery leftovers: {names:?}");
        assert!(names[0].ends_with(RECORD_EXTENSION));
        assert!(!names[0].starts_with(".markturbo-recovery-"));
    }

    #[test]
    fn checkpoints_after_two_seconds_without_an_edit() {
        let now = Instant::now();
        let mut schedule = CheckpointSchedule::default();
        schedule.mark_dirty(now);

        assert!(!schedule.is_due(now + Duration::from_secs(1)));
        assert!(schedule.is_due(now + Duration::from_secs(2)));
    }

    #[test]
    fn durable_baseline_waits_ten_seconds_before_refreshing() {
        let now = Instant::now();
        let mut schedule = CheckpointSchedule::default();

        schedule.mark_durable_baseline(now);

        assert!(!schedule.is_due(now + MAX_LOSS_WINDOW - Duration::from_nanos(1)));
        assert!(schedule.is_due(now + MAX_LOSS_WINDOW));
    }

    #[test]
    fn dispatch_timing_bounds_durable_completion_to_ten_seconds_from_the_oldest_edit() {
        let now = Instant::now();
        let mut schedule = CheckpointSchedule::default();
        schedule.mark_dirty(now);
        schedule.mark_dirty(now + Duration::from_secs(1));

        let attempt = schedule
            .checkpoint_dispatched(now + Duration::from_secs(2))
            .expect("dirty schedule should dispatch");

        assert_eq!(attempt.snapshot_at, now + Duration::from_secs(2));
        assert_eq!(attempt.oldest_covered_edit, Some(now));
        assert_eq!(
            attempt.durable_complete_by,
            now + Duration::from_secs(2) + CHECKPOINT_COMMIT_BUDGET
        );
        assert_eq!(attempt.durable_complete_by, now + MAX_LOSS_WINDOW);
    }

    #[test]
    fn late_dispatch_cannot_move_a_real_edit_past_its_loss_window() {
        let now = Instant::now();
        let mut schedule = CheckpointSchedule::default();
        schedule.mark_dirty(now);

        let attempt = schedule
            .checkpoint_dispatched(now + Duration::from_secs(5))
            .expect("dirty schedule should dispatch");

        assert_eq!(attempt.oldest_covered_edit, Some(now));
        assert_eq!(attempt.durable_complete_by, now + MAX_LOSS_WINDOW);
    }

    #[test]
    fn periodic_attempt_does_not_restore_an_already_durable_edit() {
        let now = Instant::now();
        let mut schedule = CheckpointSchedule::default();
        schedule.mark_dirty(now);
        let first = schedule.checkpoint_dispatched(now).unwrap();
        schedule.checkpoint_written(first);
        let periodic = schedule
            .checkpoint_dispatched(now + MAX_LOSS_WINDOW)
            .expect("dirty schedule should refresh periodically");

        assert_eq!(periodic.oldest_covered_edit, None);
        schedule.checkpoint_superseded(periodic);
        assert_eq!(schedule.oldest_uncovered_edit, None);
    }

    #[test]
    fn continuous_edits_cannot_postpone_the_uncovered_idle_deadline() {
        let now = Instant::now();
        let mut schedule = CheckpointSchedule::default();
        schedule.mark_dirty(now);
        for second in 1..=9 {
            schedule.mark_dirty(now + Duration::from_secs(second));
        }

        assert_eq!(schedule.next_deadline(), Some(now + IDLE_CHECKPOINT_DELAY));
    }

    #[test]
    fn a_long_running_completion_does_not_delay_periodic_dispatch_cadence() {
        let now = Instant::now();
        let mut schedule = CheckpointSchedule::default();
        schedule.mark_dirty(now);
        let attempt = schedule.checkpoint_dispatched(now).unwrap();
        schedule.checkpoint_written(attempt);

        assert_eq!(schedule.next_deadline(), Some(now + MAX_LOSS_WINDOW));
        assert!(schedule.is_due(now + MAX_LOSS_WINDOW));
    }

    #[test]
    fn an_edit_after_dispatch_stays_uncovered_for_the_next_attempt() {
        let now = Instant::now();
        let mut schedule = CheckpointSchedule::default();
        schedule.mark_dirty(now);
        let attempt = schedule
            .checkpoint_dispatched(now + Duration::from_secs(2))
            .unwrap();
        schedule.mark_dirty(now + Duration::from_secs(3));
        schedule.checkpoint_written(attempt);

        assert_eq!(schedule.next_deadline(), Some(now + Duration::from_secs(5)));
    }

    #[test]
    fn superseded_attempt_immediately_catches_up_the_restored_coverage() {
        let now = Instant::now();
        let mut schedule = CheckpointSchedule::default();
        schedule.mark_dirty(now);
        let attempt = schedule
            .checkpoint_dispatched(now + Duration::from_secs(2))
            .unwrap();

        schedule.checkpoint_superseded(attempt);

        assert!(schedule.is_due(now + Duration::from_secs(2)));
    }

    #[test]
    fn failed_attempt_restores_coverage_but_retries_in_the_future() {
        let now = Instant::now();
        let mut schedule = CheckpointSchedule::default();
        schedule.mark_dirty(now);
        let attempt = schedule
            .checkpoint_dispatched(now + Duration::from_secs(2))
            .unwrap();

        schedule.checkpoint_failed(attempt, now + Duration::from_secs(3));

        assert!(!schedule.is_due(now + Duration::from_secs(3)));
        assert!(
            schedule
                .next_deadline()
                .is_some_and(|deadline| deadline > now + Duration::from_secs(3))
        );
    }

    #[test]
    fn cancelled_attempt_restores_coverage_but_retries_in_the_future() {
        let now = Instant::now();
        let mut schedule = CheckpointSchedule::default();
        schedule.mark_dirty(now);
        let attempt = schedule
            .checkpoint_dispatched(now + Duration::from_secs(2))
            .unwrap();

        schedule.checkpoint_cancelled(attempt, now + Duration::from_secs(3));

        assert!(!schedule.is_due(now + Duration::from_secs(3)));
        assert_eq!(schedule.next_deadline(), Some(now + Duration::from_secs(4)));
    }

    #[test]
    fn deadline_miss_retries_with_a_fresh_commit_budget() {
        let now = Instant::now();
        let mut schedule = CheckpointSchedule::default();
        schedule.mark_dirty(now);
        let attempt = schedule
            .checkpoint_dispatched(now + Duration::from_secs(2))
            .unwrap();
        let missed_at = attempt.durable_complete_by;

        schedule.checkpoint_deadline_missed(attempt, missed_at);

        let retry_at = missed_at + CHECKPOINT_RETRY_DELAY;
        assert_eq!(schedule.next_deadline(), Some(retry_at));
        let retry = schedule.checkpoint_dispatched(retry_at).unwrap();
        assert_eq!(retry.oldest_covered_edit, Some(now));
        assert_eq!(
            retry.durable_complete_by,
            retry_at + CHECKPOINT_COMMIT_BUDGET
        );
    }

    #[test]
    fn deadline_miss_preserves_edits_made_after_dispatch() {
        let now = Instant::now();
        let mut schedule = CheckpointSchedule::default();
        schedule.mark_dirty(now);
        let attempt = schedule
            .checkpoint_dispatched(now + Duration::from_secs(2))
            .unwrap();
        schedule.mark_dirty(now + Duration::from_secs(3));
        let missed_at = attempt.durable_complete_by;

        schedule.checkpoint_deadline_missed(attempt, missed_at);

        assert!(schedule.is_due(missed_at));
        let retry_at = missed_at;
        let retry = schedule.checkpoint_dispatched(retry_at).unwrap();
        assert_eq!(
            retry.oldest_covered_edit,
            Some(now + Duration::from_secs(3))
        );
        assert_eq!(
            retry.durable_complete_by,
            now + Duration::from_secs(3) + MAX_LOSS_WINDOW
        );
    }

    #[test]
    fn checkpoints_at_least_every_ten_seconds_while_dirty() {
        let now = Instant::now();
        let mut schedule = CheckpointSchedule::default();
        schedule.mark_dirty(now);
        let first = schedule
            .checkpoint_dispatched(now + Duration::from_secs(2))
            .unwrap();
        schedule.checkpoint_written(first);
        schedule.mark_dirty(now + Duration::from_secs(9));
        schedule.mark_dirty(now + Duration::from_secs(10));

        assert!(!schedule.is_due(now + Duration::from_secs(10)));
        assert!(schedule.is_due(now + Duration::from_secs(11)));
        schedule.mark_clean();
        assert_eq!(schedule.next_deadline(), None);
    }

    #[test]
    fn continuous_edits_cannot_postpone_the_first_checkpoint_past_two_seconds() {
        let now = Instant::now();
        let mut schedule = CheckpointSchedule::default();
        schedule.mark_dirty(now);
        for second in 1..=10 {
            schedule.mark_dirty(now + Duration::from_secs(second));
        }

        assert!(schedule.is_due(now + Duration::from_secs(2)));
    }

    #[test]
    fn an_idle_dirty_buffer_waits_ten_seconds_after_a_completed_checkpoint() {
        let now = Instant::now();
        let mut schedule = CheckpointSchedule::default();
        schedule.mark_dirty(now);
        let first = schedule
            .checkpoint_dispatched(now + Duration::from_secs(2))
            .unwrap();
        schedule.checkpoint_written(first);

        assert!(!schedule.is_due(now + Duration::from_secs(3)));
        assert!(schedule.is_due(now + Duration::from_secs(12)));
    }

    #[cfg(windows)]
    #[test]
    fn dpapi_ciphertext_does_not_contain_plaintext_and_round_trips() {
        let protector = DpapiProtector;
        let plaintext = b"recovery plaintext \xF0\x9F\x9A\x80";
        let ciphertext = protector.protect(plaintext).unwrap();
        assert!(
            !ciphertext
                .windows(plaintext.len())
                .any(|window| window == plaintext),
            "DPAPI output must not embed the plaintext"
        );
        assert_eq!(protector.unprotect(&ciphertext).unwrap(), plaintext);
    }

    #[cfg(windows)]
    #[test]
    fn dpapi_store_reopens_the_latest_completed_checkpoint() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("recovery");
        let store = RecoveryStore::new_at(root.clone(), Arc::new(DpapiProtector)).unwrap();
        let first = checkpoint("interrupted", "older\n");
        let latest = checkpoint("interrupted", "latest 中文 \u{1f680}\n");
        store.checkpoint(&first, &HashSet::new()).unwrap();
        store.checkpoint(&latest, &HashSet::new()).unwrap();
        drop(store);

        let reopened = RecoveryStore::new_at(root, Arc::new(DpapiProtector)).unwrap();
        let scan = reopened.recover().unwrap();

        assert!(scan.issues.is_empty());
        assert_eq!(scan.records.len(), 1);
        assert_eq!(scan.records[0].record.text, latest.text);
        assert_eq!(scan.records[0].record.metadata, latest.metadata);
    }

    #[cfg(windows)]
    #[test]
    #[ignore = "measures a near-capacity DPAPI checkpoint commit against the 8-second budget"]
    fn recovery_capacity_batch_commits_within_budget() {
        const ROUNDS: usize = 3;
        const RECORD_COUNT: usize = 50;
        const TEXT_BYTES: usize = 2_500_000;

        let limits = RecoveryLimits::default();
        let initial_text = "a".repeat(TEXT_BYTES);
        let updated_text = "b".repeat(TEXT_BYTES);
        let mut elapsed_rounds = Vec::with_capacity(ROUNDS);

        for round in 0..ROUNDS {
            let dir = tempfile::tempdir().unwrap();
            let root = dir.path().join("recovery");
            let store =
                RecoveryStore::open_production_at_for_test(root.clone(), Arc::new(DpapiProtector))
                    .unwrap();
            let keys: Vec<_> = (0..RECORD_COUNT)
                .map(|index| RecoveryKey::for_document_id(&format!("capacity-{round}-{index}")))
                .collect();
            let initial: Vec<_> = keys
                .iter()
                .cloned()
                .map(|key| RecoveryCheckpoint {
                    key,
                    text: initial_text.clone(),
                    metadata: metadata(),
                })
                .collect();
            let initial_tokens: Vec<_> = initial
                .iter()
                .map(|checkpoint| store.current_token(&checkpoint.key))
                .collect();
            let initial_attempts: Vec<_> = initial
                .iter()
                .zip(&initial_tokens)
                .map(|(checkpoint, token)| RecoveryCheckpointAttempt { checkpoint, token })
                .collect();
            let initial_batch =
                store.checkpoint_batch_if_current(initial_attempts, &HashSet::new());
            assert!(
                initial_batch
                    .outcomes
                    .iter()
                    .all(|outcome| matches!(outcome, CheckpointBatchOutcome::Written)),
                "round {round} failed to prepopulate every recovery record"
            );
            drop(initial);

            let canonical_root = fs::canonicalize(&root).unwrap();
            let record_sizes: Vec<_> = fs::read_dir(&canonical_root)
                .unwrap()
                .map(|entry| entry.unwrap())
                .filter_map(|entry| {
                    record_key_from_path(&entry.path()).map(|_| entry.metadata().unwrap().len())
                })
                .collect();
            assert_eq!(record_sizes.len(), limits.max_records);
            assert!(
                record_sizes
                    .iter()
                    .all(|size| *size <= limits.max_record_bytes),
                "round {round} produced a ciphertext record over the per-record limit"
            );
            let ciphertext_total = record_sizes.iter().sum::<u64>();
            println!("recovery DPAPI capacity ciphertext total: {ciphertext_total}");
            assert!(
                ciphertext_total <= limits.max_total_bytes,
                "round {round} produced ciphertext over the total recovery limit"
            );
            assert!(
                ciphertext_total >= 118 * 1024 * 1024,
                "round {round} did not exercise at least 118 MiB of canonical ciphertext"
            );

            let started = Instant::now();
            let updated: Vec<_> = keys
                .iter()
                .cloned()
                .map(|key| RecoveryCheckpoint {
                    key,
                    text: updated_text.clone(),
                    metadata: metadata(),
                })
                .collect();
            let updated_tokens: Vec<_> = updated
                .iter()
                .map(|checkpoint| store.current_token(&checkpoint.key))
                .collect();
            let updated_attempts: Vec<_> = updated
                .iter()
                .zip(&updated_tokens)
                .map(|(checkpoint, token)| RecoveryCheckpointAttempt { checkpoint, token })
                .collect();

            let updated_batch =
                store.checkpoint_batch_if_current(updated_attempts, &HashSet::new());
            let elapsed = started.elapsed();
            assert!(
                updated_batch
                    .outcomes
                    .iter()
                    .all(|outcome| matches!(outcome, CheckpointBatchOutcome::Written)),
                "round {round} did not replace every recovery record"
            );
            println!("recovery DPAPI capacity batch round {round}: {elapsed:?}");
            elapsed_rounds.push(elapsed);
            drop(updated);

            let recovered = store.recover().unwrap();
            assert!(recovered.issues.is_empty());
            assert_eq!(recovered.records.len(), RECORD_COUNT);
            let recovered_by_key: HashMap<_, _> = recovered
                .records
                .into_iter()
                .map(|entry| (entry.record.key, entry.record.text))
                .collect();
            for key in keys {
                assert_eq!(recovered_by_key.get(&key), Some(&updated_text));
            }
            let artifacts: Vec<_> = fs::read_dir(store.root())
                .unwrap()
                .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
                .filter(|name| name.starts_with(ARTIFACT_PREFIX))
                .collect();
            assert!(
                artifacts.is_empty(),
                "round {round} left recovery transaction artifacts: {artifacts:?}"
            );
        }

        elapsed_rounds.sort_unstable();
        let median = elapsed_rounds[ROUNDS / 2];
        let max = *elapsed_rounds.last().unwrap();
        println!("recovery DPAPI capacity batch median: {median:?}; max: {max:?}");
        assert!(
            max < CHECKPOINT_COMMIT_BUDGET,
            "recovery capacity batches had median {median:?} and max {max:?}, exceeding {CHECKPOINT_COMMIT_BUDGET:?}"
        );
    }

    #[cfg(not(windows))]
    #[test]
    fn dpapi_round_trip_is_skipped_off_windows() {
        eprintln!(
            "skipping DPAPI ciphertext test: current-user DPAPI is only available on Windows"
        );
    }
}
