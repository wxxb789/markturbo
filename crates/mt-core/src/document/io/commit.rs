//! Race-checked document write commit protocol.

use std::{
    fs::File,
    io::{Read, Seek, SeekFrom},
    path::{Path, PathBuf},
};

use sha2::{Digest, Sha256};

#[cfg(all(unix, any(target_os = "linux", target_os = "macos")))]
use super::open_canonical_skill_directory;
use super::{
    ConcurrentCommitOutcome, FileStamp, SaveError, SkillObjectIdentity, SkillOriginRoot,
    SourceIdentity, file_object_id, open_skill_entrypoint, opened_skill_entrypoint_is_regular,
    skill_object_identity, skill_source_changed_error, source_identity,
};
#[cfg(all(unix, any(target_os = "linux", target_os = "macos")))]
use super::{UNIX_O_CLOEXEC, UNIX_O_NOFOLLOW, UNIX_O_NONBLOCK, openat_skill_file};

/// The caller's already-authorized state of the destination.
///
/// Save and Save As retain their distinct confirmation flows in `io`; this
/// capability only lets the shared commit protocol replace this exact version
/// or create an entry without clobbering anything that appeared meanwhile.
pub(super) enum DestinationAuthorization<'a> {
    ReplaceExpected(&'a FileStamp),
    CreateOnly,
}

pub(super) struct CommitRequest<'a> {
    pub(super) staged: tempfile::NamedTempFile,
    pub(super) source_path: &'a Path,
    pub(super) expected_source_identity: &'a SourceIdentity,
    pub(super) destination: &'a Path,
    pub(super) destination_authorization: DestinationAuthorization<'a>,
    pub(super) prepared_bytes: &'a [u8],
    pub(super) skill_root: Option<&'a SkillOriginRoot>,
    pub(super) expected_skill_entrypoint_identity: Option<SkillObjectIdentity>,
}

pub(super) struct CommitResult {
    pub(super) stamp: FileStamp,
    pub(super) source_identity: SourceIdentity,
}

pub(super) fn execute(request: CommitRequest<'_>) -> Result<CommitResult, SaveError> {
    let (expected_destination_stamp, recreates_missing) = match request.destination_authorization {
        DestinationAuthorization::ReplaceExpected(stamp) => (Some(stamp), false),
        DestinationAuthorization::CreateOnly => (None, true),
    };
    let (stamp, source_identity) = CommitPlan {
        source_path: request.source_path,
        expected_source_identity: request.expected_source_identity,
        destination: request.destination,
        expected_destination_stamp,
        recreates_missing,
        prepared_bytes: request.prepared_bytes,
        skill_root: request.skill_root,
        expected_skill_entrypoint_identity: request.expected_skill_entrypoint_identity,
    }
    .commit_staged(request.staged)?;
    Ok(CommitResult {
        stamp,
        source_identity,
    })
}

/// Immutable inputs for committing a fully staged document replacement.
///
/// Keeping every expected value together makes the two save entry points share
/// the same race-sensitive sequence without widening either write permission.
struct CommitPlan<'a> {
    source_path: &'a Path,
    expected_source_identity: &'a SourceIdentity,
    destination: &'a Path,
    expected_destination_stamp: Option<&'a FileStamp>,
    recreates_missing: bool,
    prepared_bytes: &'a [u8],
    skill_root: Option<&'a SkillOriginRoot>,
    expected_skill_entrypoint_identity: Option<SkillObjectIdentity>,
}

#[cfg(unix)]
struct SkillCommitPrecheck {
    destination_stamp: Option<FileStamp>,
    destination_identity: Option<SkillObjectIdentity>,
    destination_handle: Option<File>,
}

#[cfg(not(unix))]
type SkillCommitPrecheck = ();

#[cfg(all(unix, any(target_os = "linux", target_os = "macos")))]
struct PendingSkillCommit {
    staged_name: std::ffi::OsString,
    staged_path: PathBuf,
    recovery_name: std::ffi::OsString,
    recovery_path: PathBuf,
    staged_identity: SkillObjectIdentity,
    staged_stamp: FileStamp,
    displaced_identity: Option<SkillObjectIdentity>,
    displaced_stamp: Option<FileStamp>,
}

#[cfg(all(unix, any(target_os = "linux", target_os = "macos")))]
struct SkillRecoveryLinkError {
    _error: std::io::Error,
    candidate_path: Option<PathBuf>,
}

impl CommitPlan<'_> {
    fn commit_staged(
        self,
        staged: tempfile::NamedTempFile,
    ) -> Result<(FileStamp, SourceIdentity), SaveError> {
        if let Some(root) = self.skill_root {
            #[cfg(all(unix, any(target_os = "linux", target_os = "macos")))]
            return self.commit_skill_unix(root, staged);
            #[cfg(windows)]
            return self.commit_skill_windows(root, staged);
            #[cfg(all(unix, not(any(target_os = "linux", target_os = "macos"))))]
            return Err(SaveError::SourceIdentityChanged);
        }
        self.commit_unbound(staged)
    }

    #[cfg(all(unix, any(target_os = "linux", target_os = "macos")))]
    fn commit_skill_unix(
        self,
        root: &SkillOriginRoot,
        staged: tempfile::NamedTempFile,
    ) -> Result<(FileStamp, SourceIdentity), SaveError> {
        let (staged_file, kept_staged_path) = match staged.keep() {
            Ok(kept) => kept,
            Err(error) => {
                return Err(skill_commit_indeterminate_paths(
                    root,
                    error.file.path(),
                    None,
                ));
            }
        };
        if self.verify_preconditions().is_err() {
            return Err(skill_commit_indeterminate_paths(
                root,
                &kept_staged_path,
                None,
            ));
        }
        run_save_commit_hook();
        let _source_guard = guard_source_path(self.source_path, self.expected_source_identity)?;
        let precheck = match self.verify_preconditions()? {
            Some(precheck) => precheck,
            None => {
                return Err(skill_commit_indeterminate_paths(
                    root,
                    &kept_staged_path,
                    None,
                ));
            }
        };
        let (stamp, identity, pending) = commit_skill_staged_at_root(
            root,
            staged_file,
            kept_staged_path,
            precheck,
            self.recreates_missing,
            self.prepared_bytes,
        )?;
        run_save_post_commit_hook();
        if verify_skill_committed_entry(root, &stamp, identity).is_err() {
            return Err(skill_commit_indeterminate(root, &pending));
        }
        finish_skill_commit(root, pending, &stamp, identity)?;
        Ok((stamp, SourceIdentity::Regular))
    }

    #[cfg(windows)]
    fn commit_skill_windows(
        self,
        root: &SkillOriginRoot,
        staged: tempfile::NamedTempFile,
    ) -> Result<(FileStamp, SourceIdentity), SaveError> {
        // This is the last pre-mutation check. `ReplaceFileW` does not provide
        // a compare-and-swap primitive, so Windows additionally proves what it
        // put in its backup after the replace and retains uncertain artifacts.
        self.verify_preconditions()?;
        run_save_commit_hook();
        // On Windows an open reparse point denies delete/retarget operations
        // on the link without holding the resolved target open across
        // ReplaceFileW. The guard remains live through final verification.
        let _source_guard = guard_source_path(self.source_path, self.expected_source_identity)?;
        // Test hooks model a writer acting after the user's decision. Re-check
        // every path after the hook so it cannot widen the approved write set.
        self.verify_preconditions()?;
        let stamp = commit(
            self.destination,
            staged,
            self.expected_destination_stamp,
            self.recreates_missing,
            self.prepared_bytes,
        )?;
        run_save_post_commit_hook();
        let entrypoint = open_skill_entrypoint(root.directory(), root.canonical_root())
            .map_err(SaveError::Io)?;
        let identity = skill_object_identity(&entrypoint).map_err(SaveError::Io)?;
        if verify_skill_committed_entry(root, &stamp, identity).is_err() {
            return Err(SaveError::ConcurrentCommit {
                preserved_paths: vec![root.canonical_root().join("SKILL.md")],
                outcome: ConcurrentCommitOutcome::Indeterminate,
            });
        }
        Ok((stamp, SourceIdentity::Regular))
    }

    fn commit_unbound(
        self,
        staged: tempfile::NamedTempFile,
    ) -> Result<(FileStamp, SourceIdentity), SaveError> {
        self.verify_preconditions()?;
        run_save_commit_hook();
        let _source_guard = guard_source_path(self.source_path, self.expected_source_identity)?;
        self.verify_preconditions()?;
        let stamp = commit(
            self.destination,
            staged,
            self.expected_destination_stamp,
            self.recreates_missing,
            self.prepared_bytes,
        )?;
        run_save_post_commit_hook();
        let source_identity = verify_committed_entry(
            self.source_path,
            self.expected_source_identity,
            self.destination,
            &stamp,
        )?;
        Ok((stamp, source_identity))
    }

    fn verify_preconditions(&self) -> Result<Option<SkillCommitPrecheck>, SaveError> {
        if let Some(root) = self.skill_root {
            verify_skill_path_preconditions(
                root,
                self.expected_destination_stamp,
                self.recreates_missing,
                self.expected_skill_entrypoint_identity,
            )
            .map(Some)
        } else {
            verify_path_preconditions(
                self.source_path,
                self.expected_source_identity,
                self.destination,
                self.expected_destination_stamp,
                self.recreates_missing,
            )?;
            Ok(None)
        }
    }
}

fn verify_path_preconditions(
    source_path: &Path,
    expected_source_identity: &SourceIdentity,
    destination: &Path,
    expected: Option<&FileStamp>,
    recreated_missing: bool,
) -> Result<(), SaveError> {
    if recreated_missing {
        return match source_identity(source_path) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Ok(_) => Err(SaveError::Conflict),
            Err(error) => Err(SaveError::Io(error)),
        };
    }

    if source_identity(source_path).map_err(SaveError::Io)? != *expected_source_identity {
        return Err(SaveError::SourceIdentityChanged);
    }
    match FileStamp::of(destination) {
        Ok(current) if Some(&current) == expected => Ok(()),
        Ok(_) => Err(SaveError::Conflict),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Err(SaveError::Missing),
        Err(error) => Err(SaveError::Io(error)),
    }
}

/// Keeps a source symlink from being retargeted between the last check and
/// post-commit verification. The guard deliberately opens the reparse point,
/// never the resolved target: ReplaceFileW must remain free to replace that
/// target while the link stays stable.
struct SourcePathGuard {
    #[cfg(windows)]
    _reparse_point: Option<File>,
}

fn guard_source_path(
    source_path: &Path,
    expected_source_identity: &SourceIdentity,
) -> Result<SourcePathGuard, SaveError> {
    #[cfg(windows)]
    let reparse_point = match expected_source_identity {
        SourceIdentity::Regular => None,
        SourceIdentity::SymbolicLink { .. } => Some(
            super::open_source_entry_guard(source_path).map_err(|error| {
                if error.kind() == std::io::ErrorKind::NotFound {
                    SaveError::SourceIdentityChanged
                } else {
                    SaveError::Io(error)
                }
            })?,
        ),
    };

    if matches!(
        expected_source_identity,
        SourceIdentity::SymbolicLink { .. }
    ) {
        // For Windows this runs after the reparse-point handle is acquired.
        // On other platforms it is a conservative final identity check only;
        // it does not claim a portable compare-and-swap guarantee.
        if source_identity(source_path).map_err(SaveError::Io)? != *expected_source_identity {
            return Err(SaveError::SourceIdentityChanged);
        }
    }

    Ok(SourcePathGuard {
        #[cfg(windows)]
        _reparse_point: reparse_point,
    })
}

fn verify_committed_entry(
    source_path: &Path,
    expected_source_identity: &SourceIdentity,
    destination: &Path,
    committed_stamp: &FileStamp,
) -> Result<SourceIdentity, SaveError> {
    let current_source_identity =
        source_identity(source_path).map_err(|_| SaveError::ConcurrentCommit {
            preserved_paths: existing_paths([source_path, destination]),
            outcome: ConcurrentCommitOutcome::Indeterminate,
        })?;
    let source_stamp = FileStamp::of(source_path).map_err(|_| SaveError::ConcurrentCommit {
        preserved_paths: existing_paths([source_path, destination]),
        outcome: ConcurrentCommitOutcome::Indeterminate,
    })?;
    if current_source_identity == *expected_source_identity && source_stamp == *committed_stamp {
        Ok(current_source_identity)
    } else {
        Err(SaveError::ConcurrentCommit {
            preserved_paths: existing_paths([source_path, destination]),
            outcome: ConcurrentCommitOutcome::Indeterminate,
        })
    }
}

fn prepared_matches(stamp: &FileStamp, bytes: &[u8]) -> bool {
    let digest: [u8; 32] = Sha256::digest(bytes).into();
    stamp.len == u64::try_from(bytes.len()).expect("buffer length fits u64")
        && stamp.digest == digest
}

fn fingerprint_open_file(file: &mut File) -> std::io::Result<FileStamp> {
    file.seek(SeekFrom::Start(0))?;
    let before = file.metadata()?;
    let object_id = file_object_id(file)?;
    let mut hasher = Sha256::new();
    let mut byte_size = 0_u64;
    let mut buffer = [0_u8; 8 * 1024];
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        byte_size = byte_size
            .checked_add(read as u64)
            .ok_or_else(|| std::io::Error::other("file size overflow"))?;
        hasher.update(&buffer[..read]);
    }
    let after = file.metadata()?;
    if before.len() != byte_size
        || after.len() != byte_size
        || before.modified().ok() != after.modified().ok()
    {
        return Err(std::io::Error::new(
            std::io::ErrorKind::WouldBlock,
            "file changed while being fingerprinted",
        ));
    }
    Ok(FileStamp {
        modified: after.modified().ok(),
        len: byte_size,
        digest: hasher.finalize().into(),
        object_id,
    })
}

fn skill_root_entrypoint_snapshot(
    root: &SkillOriginRoot,
) -> std::io::Result<(FileStamp, SkillObjectIdentity, File)> {
    let mut file = open_skill_entrypoint(root.directory(), root.canonical_root())?;
    if !opened_skill_entrypoint_is_regular(&file.metadata()?) {
        return Err(skill_source_changed_error());
    }
    let identity = skill_object_identity(&file)?;
    let stamp = fingerprint_open_file(&mut file)?;
    if skill_object_identity(&file)? != identity {
        return Err(std::io::Error::new(
            std::io::ErrorKind::WouldBlock,
            "Agent Skill entrypoint changed while being fingerprinted",
        ));
    }
    Ok((stamp, identity, file))
}

fn verify_skill_path_preconditions(
    root: &SkillOriginRoot,
    expected: Option<&FileStamp>,
    recreated_missing: bool,
    expected_identity: Option<SkillObjectIdentity>,
) -> Result<SkillCommitPrecheck, SaveError> {
    match skill_root_entrypoint_snapshot(root) {
        Ok(_) if recreated_missing => Err(SaveError::Conflict),
        Ok((_, identity, _)) if expected_identity.is_some_and(|expected| expected != identity) => {
            Err(SaveError::SourceIdentityChanged)
        }
        Ok((current, identity, file)) if Some(&current) == expected => {
            #[cfg(unix)]
            {
                Ok(SkillCommitPrecheck {
                    destination_stamp: Some(current),
                    destination_identity: Some(identity),
                    destination_handle: Some(file),
                })
            }
            #[cfg(not(unix))]
            {
                drop(file);
                let _ = identity;
                Ok(())
            }
        }
        Ok(_) => Err(SaveError::Conflict),
        Err(error) if recreated_missing && error.kind() == std::io::ErrorKind::NotFound => {
            #[cfg(unix)]
            {
                Ok(SkillCommitPrecheck {
                    destination_stamp: None,
                    destination_identity: None,
                    destination_handle: None,
                })
            }
            #[cfg(not(unix))]
            {
                Ok(())
            }
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Err(SaveError::Missing),
        Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => Err(SaveError::Conflict),
        Err(error) if error.kind() == std::io::ErrorKind::Other => {
            Err(SaveError::SourceIdentityChanged)
        }
        Err(error) => Err(SaveError::Io(error)),
    }
}

#[cfg(all(unix, any(target_os = "linux", target_os = "macos")))]
const SKILL_LINKAT_NOFOLLOW_FLAGS: std::ffi::c_int = if cfg!(target_os = "macos") {
    // Darwin <sys/fcntl.h>: AT_SYMLINK_NOFOLLOW_ANY.
    0x0800
} else {
    0
};

#[cfg(all(unix, any(target_os = "linux", target_os = "macos")))]
fn cstring_skill_name(name: &std::ffi::OsStr) -> std::io::Result<std::ffi::CString> {
    use std::os::unix::ffi::OsStrExt as _;
    std::ffi::CString::new(name.as_bytes())
        .map_err(|_| std::io::Error::new(std::io::ErrorKind::InvalidInput, "NUL in Skill name"))
}

#[cfg(all(unix, any(target_os = "linux", target_os = "macos")))]
fn skill_commit_indeterminate_paths(
    root: &SkillOriginRoot,
    staged_path: &Path,
    recovery_path: Option<&Path>,
) -> SaveError {
    let mut preserved_paths = Vec::new();
    let named_root = open_canonical_skill_directory(root.canonical_root()).ok();
    let root_was_named = named_root.as_ref().is_some_and(|opened| {
        skill_object_identity(&opened.directory)
            .ok()
            .zip(skill_object_identity(root.directory()).ok())
            .is_some_and(|(named, pinned)| named == pinned)
    });
    if root_was_named {
        for name in [
            Some(std::ffi::OsStr::new("SKILL.md")),
            (staged_path.parent() == Some(root.canonical_root()))
                .then(|| staged_path.file_name())
                .flatten(),
            recovery_path.and_then(|path| {
                (path.parent() == Some(root.canonical_root()))
                    .then(|| path.file_name())
                    .flatten()
            }),
        ]
        .into_iter()
        .flatten()
        {
            if open_skill_name_snapshot(root, name).is_ok() {
                let path = root.canonical_root().join(name);
                if !preserved_paths.contains(&path) {
                    preserved_paths.push(path);
                }
            }
        }
    }
    if root_was_named
        && !open_canonical_skill_directory(root.canonical_root()).is_ok_and(|opened| {
            skill_object_identity(&opened.directory)
                .ok()
                .zip(skill_object_identity(root.directory()).ok())
                .is_some_and(|(named, pinned)| named == pinned)
        })
    {
        preserved_paths.clear();
    }
    SaveError::ConcurrentCommit {
        preserved_paths,
        outcome: ConcurrentCommitOutcome::Indeterminate,
    }
}

#[cfg(all(unix, any(target_os = "linux", target_os = "macos")))]
fn skill_commit_indeterminate(root: &SkillOriginRoot, pending: &PendingSkillCommit) -> SaveError {
    skill_commit_indeterminate_paths(root, &pending.staged_path, Some(&pending.recovery_path))
}

#[cfg(all(unix, any(target_os = "linux", target_os = "macos")))]
fn open_skill_name_snapshot(
    root: &SkillOriginRoot,
    name: &std::ffi::OsStr,
) -> std::io::Result<(File, FileStamp, SkillObjectIdentity)> {
    let mut file = openat_skill_file(
        root.directory(),
        name,
        UNIX_O_NOFOLLOW | UNIX_O_NONBLOCK | UNIX_O_CLOEXEC,
    )?;
    if !opened_skill_entrypoint_is_regular(&file.metadata()?) {
        return Err(skill_source_changed_error());
    }
    let identity = skill_object_identity(&file)?;
    let stamp = fingerprint_open_file(&mut file)?;
    if skill_object_identity(&file)? != identity {
        return Err(std::io::Error::new(
            std::io::ErrorKind::WouldBlock,
            "Skill file identity changed while being fingerprinted",
        ));
    }
    Ok((file, stamp, identity))
}

#[cfg(all(unix, any(target_os = "linux", target_os = "macos")))]
fn verify_skill_name(
    root: &SkillOriginRoot,
    name: &std::ffi::OsStr,
    identity: SkillObjectIdentity,
    stamp: &FileStamp,
    bytes: Option<&[u8]>,
) -> std::io::Result<()> {
    let (_, current_stamp, current_identity) = open_skill_name_snapshot(root, name)?;
    if current_identity != identity
        || current_stamp != *stamp
        || bytes.is_some_and(|bytes| !prepared_matches(&current_stamp, bytes))
    {
        return Err(std::io::Error::new(
            std::io::ErrorKind::WouldBlock,
            "Skill package entry changed during commit",
        ));
    }
    Ok(())
}

#[cfg(all(unix, any(target_os = "linux", target_os = "macos")))]
fn create_skill_editor_recovery(
    root: &SkillOriginRoot,
    staged_name: &std::ffi::OsStr,
    staged_file: &File,
) -> Result<(std::ffi::OsString, PathBuf), SkillRecoveryLinkError> {
    use std::os::fd::AsRawFd as _;

    let staged_name = cstring_skill_name(staged_name).map_err(|error| SkillRecoveryLinkError {
        _error: error,
        candidate_path: None,
    })?;
    let root_fd = root.directory().as_raw_fd();
    let mut candidate_path = None;
    let recovery = tempfile::Builder::new()
        .prefix(".markturbo-editor-")
        .disable_cleanup(true)
        .make_in(root.canonical_root(), |candidate| {
            candidate_path = Some(candidate.to_path_buf());
            let name = candidate.file_name().ok_or_else(|| {
                std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    "temporary Skill recovery name is empty",
                )
            })?;
            let name = cstring_skill_name(name)?;
            let file = staged_file.try_clone()?;
            // Both names are single components relative to the pinned root.
            let linked = link_skill_name_raw(root_fd, staged_name.as_ptr(), name.as_ptr());
            if linked < 0 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(file)
        })
        .map_err(|error| SkillRecoveryLinkError {
            _error: error,
            candidate_path: candidate_path.clone(),
        })?;
    let (file, temp_path) = recovery.keep().map_err(|error| SkillRecoveryLinkError {
        _error: error.error,
        candidate_path: Some(error.file.path().to_path_buf()),
    })?;
    let name = temp_path
        .file_name()
        .ok_or_else(|| SkillRecoveryLinkError {
            _error: std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "temporary Skill recovery name is empty",
            ),
            candidate_path: Some(temp_path.clone()),
        })?
        .to_os_string();
    drop(file);
    let path = root.canonical_root().join(&name);
    Ok((name, path))
}

#[cfg(all(unix, any(target_os = "linux", target_os = "macos")))]
fn link_skill_name_raw(
    root_fd: std::os::fd::RawFd,
    source: *const std::ffi::c_char,
    target: *const std::ffi::c_char,
) -> std::ffi::c_int {
    unsafe extern "C" {
        fn linkat(
            old_directory: std::ffi::c_int,
            old_path: *const std::ffi::c_char,
            new_directory: std::ffi::c_int,
            new_path: *const std::ffi::c_char,
            flags: std::ffi::c_int,
        ) -> std::ffi::c_int;
    }
    // SAFETY: caller supplies live dirfd and NUL-terminated single components.
    unsafe {
        linkat(
            root_fd,
            source,
            root_fd,
            target,
            SKILL_LINKAT_NOFOLLOW_FLAGS,
        )
    }
}

#[cfg(all(unix, target_os = "linux"))]
fn exchange_skill_names(
    root: &SkillOriginRoot,
    source: &std::ffi::OsStr,
    target: &std::ffi::OsStr,
) -> std::io::Result<()> {
    use std::ffi::{c_int, c_long};
    use std::os::fd::AsRawFd as _;

    #[cfg(test)]
    if SKILL_FORCE_UNSUPPORTED_EXCHANGE.with(|flag| flag.replace(false)) {
        return Err(std::io::Error::new(
            std::io::ErrorKind::Unsupported,
            "atomic Skill exchange was disabled for the test",
        ));
    }

    #[cfg(any(target_arch = "x86_64", target_arch = "aarch64"))]
    unsafe extern "C" {
        fn syscall(number: c_long, ...) -> c_long;
    }
    #[cfg(target_arch = "x86_64")]
    const SYS_RENAMEAT2: c_long = 316;
    #[cfg(target_arch = "aarch64")]
    const SYS_RENAMEAT2: c_long = 276;

    #[cfg(not(any(target_arch = "x86_64", target_arch = "aarch64")))]
    {
        let _ = (root, source, target);
        return Err(std::io::Error::new(
            std::io::ErrorKind::Unsupported,
            "renameat2 exchange is not declared for this Linux architecture",
        ));
    }
    #[cfg(any(target_arch = "x86_64", target_arch = "aarch64"))]
    {
        let source = cstring_skill_name(source)?;
        let target = cstring_skill_name(target)?;
        // Linux renameat2 RENAME_EXCHANGE ABI flag.
        // SAFETY: root_fd is pinned and both names are NUL-terminated components.
        let result = unsafe {
            syscall(
                SYS_RENAMEAT2,
                root.directory().as_raw_fd(),
                source.as_ptr(),
                root.directory().as_raw_fd(),
                target.as_ptr(),
                2 as c_int,
            )
        };
        if result < 0 {
            Err(std::io::Error::last_os_error())
        } else {
            Ok(())
        }
    }
}

#[cfg(all(unix, target_os = "macos"))]
fn exchange_skill_names(
    root: &SkillOriginRoot,
    source: &std::ffi::OsStr,
    target: &std::ffi::OsStr,
) -> std::io::Result<()> {
    use std::ffi::{c_char, c_int, c_uint};
    use std::os::fd::AsRawFd as _;

    #[cfg(test)]
    if SKILL_FORCE_UNSUPPORTED_EXCHANGE.with(|flag| flag.replace(false)) {
        return Err(std::io::Error::new(
            std::io::ErrorKind::Unsupported,
            "atomic Skill exchange was disabled for the test",
        ));
    }
    unsafe extern "C" {
        fn renameatx_np(
            from_directory: c_int,
            from_path: *const c_char,
            to_directory: c_int,
            to_path: *const c_char,
            flags: c_uint,
        ) -> c_int;
    }
    let source = cstring_skill_name(source)?;
    let target = cstring_skill_name(target)?;
    // Darwin <sys/stdio.h>: RENAME_SWAP | RENAME_NOFOLLOW_ANY.
    const RENAME_SWAP_NOFOLLOW: c_uint = 0x2 | 0x10;
    // SAFETY: root_fd is pinned and both names are NUL-terminated components.
    let result = unsafe {
        renameatx_np(
            root.directory().as_raw_fd(),
            source.as_ptr(),
            root.directory().as_raw_fd(),
            target.as_ptr(),
            RENAME_SWAP_NOFOLLOW,
        )
    };
    if result < 0 {
        Err(std::io::Error::last_os_error())
    } else {
        Ok(())
    }
}

#[cfg(all(unix, any(target_os = "linux", target_os = "macos")))]
fn link_skill_name(
    root: &SkillOriginRoot,
    source: &std::ffi::OsStr,
    target: &std::ffi::OsStr,
) -> std::io::Result<()> {
    use std::os::fd::AsRawFd as _;
    let source = cstring_skill_name(source)?;
    let target = cstring_skill_name(target)?;
    let result = link_skill_name_raw(
        root.directory().as_raw_fd(),
        source.as_ptr(),
        target.as_ptr(),
    );
    if result < 0 {
        Err(std::io::Error::last_os_error())
    } else {
        Ok(())
    }
}

#[cfg(all(unix, any(target_os = "linux", target_os = "macos")))]
fn unlink_skill_name(root: &SkillOriginRoot, name: &std::ffi::OsStr) -> std::io::Result<()> {
    use std::ffi::{c_char, c_int};
    use std::os::fd::AsRawFd as _;
    unsafe extern "C" {
        fn unlinkat(directory: c_int, path: *const c_char, flags: c_int) -> c_int;
    }
    let name = cstring_skill_name(name)?;
    // SAFETY: root is pinned and name is one NUL-terminated component.
    let result = unsafe { unlinkat(root.directory().as_raw_fd(), name.as_ptr(), 0) };
    if result < 0 {
        Err(std::io::Error::last_os_error())
    } else {
        Ok(())
    }
}

fn verify_skill_committed_entry(
    root: &SkillOriginRoot,
    committed_stamp: &FileStamp,
    committed_identity: SkillObjectIdentity,
) -> Result<(), SaveError> {
    let mut file = open_skill_entrypoint(root.directory(), root.canonical_root())
        .map_err(|_| SaveError::SourceIdentityChanged)?;
    if !opened_skill_entrypoint_is_regular(
        &file
            .metadata()
            .map_err(|_| SaveError::SourceIdentityChanged)?,
    ) {
        return Err(SaveError::SourceIdentityChanged);
    }
    let identity = skill_object_identity(&file).map_err(|_| SaveError::SourceIdentityChanged)?;
    let stamp = fingerprint_open_file(&mut file).map_err(|_| SaveError::SourceIdentityChanged)?;
    if stamp == *committed_stamp && identity == committed_identity {
        Ok(())
    } else {
        Err(SaveError::SourceIdentityChanged)
    }
}

#[cfg(all(unix, any(target_os = "linux", target_os = "macos")))]
fn finish_skill_commit(
    root: &SkillOriginRoot,
    pending: PendingSkillCommit,
    committed_stamp: &FileStamp,
    committed_identity: SkillObjectIdentity,
) -> Result<(), SaveError> {
    let verify_retained = || {
        verify_skill_committed_entry(root, committed_stamp, committed_identity).is_ok()
            && verify_skill_name(
                root,
                &pending.recovery_name,
                pending.staged_identity,
                &pending.staged_stamp,
                None,
            )
            .is_ok()
            && if let (Some(identity), Some(stamp)) =
                (pending.displaced_identity, pending.displaced_stamp.as_ref())
            {
                verify_skill_name(root, &pending.staged_name, identity, stamp, None).is_ok()
            } else {
                verify_skill_name(
                    root,
                    &pending.staged_name,
                    pending.staged_identity,
                    &pending.staged_stamp,
                    None,
                )
                .is_ok()
            }
    };
    if !verify_retained() {
        return Err(skill_commit_indeterminate(root, &pending));
    }
    #[cfg(test)]
    run_skill_before_cleanup_hook(root.canonical_root());
    if !verify_retained() {
        return Err(skill_commit_indeterminate(root, &pending));
    }
    if unlink_skill_name(root, &pending.staged_name).is_err() {
        return Err(skill_commit_indeterminate(root, &pending));
    }
    if unlink_skill_name(root, &pending.recovery_name).is_err() {
        return Err(skill_commit_indeterminate(root, &pending));
    }
    Ok(())
}

#[cfg(all(unix, any(target_os = "linux", target_os = "macos")))]
fn commit_skill_staged_at_root(
    root: &SkillOriginRoot,
    staged_file: File,
    kept_staged_path: PathBuf,
    mut precheck: SkillCommitPrecheck,
    recreated_missing: bool,
    bytes: &[u8],
) -> Result<(FileStamp, SkillObjectIdentity, PendingSkillCommit), SaveError> {
    let staged_name = kept_staged_path
        .file_name()
        .ok_or_else(|| skill_commit_indeterminate_paths(root, &kept_staged_path, None))?
        .to_os_string();
    let staged_identity = skill_object_identity(&staged_file)
        .map_err(|_| skill_commit_indeterminate_paths(root, &kept_staged_path, None))?;
    let (_, staged_stamp, stage_identity) = open_skill_name_snapshot(root, &staged_name)
        .map_err(|_| skill_commit_indeterminate_paths(root, &kept_staged_path, None))?;
    if stage_identity != staged_identity || !prepared_matches(&staged_stamp, bytes) {
        return Err(skill_commit_indeterminate_paths(
            root,
            &kept_staged_path,
            None,
        ));
    }

    let (prepared_destination_stamp, prepared_destination_identity) = if recreated_missing {
        if precheck.destination_stamp.is_some() || precheck.destination_identity.is_some() {
            return Err(skill_commit_indeterminate_paths(
                root,
                &kept_staged_path,
                None,
            ));
        }
        (None, None)
    } else {
        let (Some(stamp), Some(identity), Some(handle)) = (
            precheck.destination_stamp.as_ref(),
            precheck.destination_identity,
            precheck.destination_handle.as_mut(),
        ) else {
            return Err(skill_commit_indeterminate_paths(
                root,
                &kept_staged_path,
                None,
            ));
        };
        if skill_object_identity(handle)
            .map_err(|_| skill_commit_indeterminate_paths(root, &kept_staged_path, None))?
            != identity
            || fingerprint_open_file(handle)
                .map_err(|_| skill_commit_indeterminate_paths(root, &kept_staged_path, None))?
                != *stamp
        {
            return Err(skill_commit_indeterminate_paths(
                root,
                &kept_staged_path,
                None,
            ));
        }
        (Some(stamp.clone()), Some(identity))
    };

    let (recovery_name, recovery_path) =
        create_skill_editor_recovery(root, &staged_name, &staged_file).map_err(|error| {
            skill_commit_indeterminate_paths(
                root,
                &kept_staged_path,
                error.candidate_path.as_deref(),
            )
        })?;
    verify_skill_name(
        root,
        &recovery_name,
        staged_identity,
        &staged_stamp,
        Some(bytes),
    )
    .map_err(|_| skill_commit_indeterminate_paths(root, &kept_staged_path, Some(&recovery_path)))?;

    #[cfg(test)]
    run_skill_before_exchange_hook(root.canonical_root(), &kept_staged_path);

    // The staged name may have been replaced after its initial verification.
    verify_skill_name(
        root,
        &staged_name,
        staged_identity,
        &staged_stamp,
        Some(bytes),
    )
    .map_err(|_| skill_commit_indeterminate_paths(root, &kept_staged_path, Some(&recovery_path)))?;
    verify_skill_name(
        root,
        &recovery_name,
        staged_identity,
        &staged_stamp,
        Some(bytes),
    )
    .map_err(|_| skill_commit_indeterminate_paths(root, &kept_staged_path, Some(&recovery_path)))?;
    if let (Some(expected_stamp), Some(expected_identity), Some(destination)) = (
        prepared_destination_stamp.as_ref(),
        prepared_destination_identity,
        precheck.destination_handle.as_mut(),
    ) {
        if skill_object_identity(destination).map_err(SaveError::Io)? != expected_identity
            || fingerprint_open_file(destination).map_err(SaveError::Io)? != *expected_stamp
        {
            return Err(skill_commit_indeterminate_paths(
                root,
                &kept_staged_path,
                Some(&recovery_path),
            ));
        }
    }

    if recreated_missing {
        if let Err(_error) = link_skill_name(root, &recovery_name, std::ffi::OsStr::new("SKILL.md"))
        {
            return Err(skill_commit_indeterminate_paths(
                root,
                &kept_staged_path,
                Some(&recovery_path),
            ));
        }
    } else if exchange_skill_names(root, &staged_name, std::ffi::OsStr::new("SKILL.md")).is_err() {
        // Never fall back to a rename-overwrite when the atomic exchange is unavailable.
        return Err(skill_commit_indeterminate_paths(
            root,
            &kept_staged_path,
            Some(&recovery_path),
        ));
    }

    verify_skill_name(
        root,
        std::ffi::OsStr::new("SKILL.md"),
        staged_identity,
        &staged_stamp,
        Some(bytes),
    )
    .map_err(|_| skill_commit_indeterminate_paths(root, &kept_staged_path, Some(&recovery_path)))?;
    if let (Some(identity), Some(stamp)) = (
        prepared_destination_identity,
        prepared_destination_stamp.as_ref(),
    ) {
        verify_skill_name(root, &staged_name, identity, stamp, None).map_err(|_| {
            skill_commit_indeterminate_paths(root, &kept_staged_path, Some(&recovery_path))
        })?;
    } else {
        verify_skill_name(
            root,
            &staged_name,
            staged_identity,
            &staged_stamp,
            Some(bytes),
        )
        .map_err(|_| {
            skill_commit_indeterminate_paths(root, &kept_staged_path, Some(&recovery_path))
        })?;
    }

    let (_, committed_stamp, committed_identity) =
        open_skill_name_snapshot(root, std::ffi::OsStr::new("SKILL.md")).map_err(|_| {
            skill_commit_indeterminate_paths(root, &kept_staged_path, Some(&recovery_path))
        })?;
    let pending = PendingSkillCommit {
        staged_name,
        staged_path: kept_staged_path,
        recovery_name,
        recovery_path,
        staged_identity,
        staged_stamp,
        displaced_identity: prepared_destination_identity,
        displaced_stamp: prepared_destination_stamp,
    };
    Ok((committed_stamp, committed_identity, pending))
}

#[cfg(not(windows))]
fn commit(
    path: &Path,
    temp: tempfile::NamedTempFile,
    _expected: Option<&FileStamp>,
    recreated_missing: bool,
    bytes: &[u8],
) -> Result<FileStamp, SaveError> {
    if recreated_missing {
        temp.persist_noclobber(path).map_err(|error| {
            if error.error.kind() == std::io::ErrorKind::AlreadyExists {
                SaveError::Conflict
            } else {
                SaveError::Io(error.error)
            }
        })?;
    } else {
        // The caller made the same preflight check immediately before this
        // atomic persist. POSIX does not offer a portable compare-and-swap.
        temp.persist(path)
            .map_err(|error| SaveError::Io(error.error))?;
    }

    verify_prepared_destination(path, bytes)
}

#[cfg(windows)]
fn commit(
    path: &Path,
    temp: tempfile::NamedTempFile,
    expected: Option<&FileStamp>,
    recreated_missing: bool,
    bytes: &[u8],
) -> Result<FileStamp, SaveError> {
    if recreated_missing {
        return temp
            .persist_noclobber(path)
            .map_err(|error| {
                if error.error.kind() == std::io::ErrorKind::AlreadyExists {
                    SaveError::Conflict
                } else {
                    SaveError::Io(error.error)
                }
            })
            .and_then(|persisted| {
                drop(persisted);
                verify_prepared_destination(path, bytes)
            });
    }

    let expected = expected.expect("existing saves have an approved fingerprint");
    let (file, replacement) = temp.keep().map_err(|error| SaveError::Io(error.error))?;
    drop(file);
    let backup = match reserve_sibling_path(path, ".markturbo-backup-") {
        Ok(backup) => backup,
        Err(reservation) => {
            let mut preserved_paths = reservation.preserved_paths;
            extend_existing_paths(&mut preserved_paths, [path, &replacement]);
            return Err(SaveError::ConcurrentCommit {
                preserved_paths,
                outcome: ConcurrentCommitOutcome::Indeterminate,
            });
        }
    };

    if replace_file(path, &replacement, &backup).is_err() {
        // `ReplaceFileW` reports an error but does not expose a transaction
        // result, so do not delete the prepared bytes or assert which file won.
        return Err(SaveError::ConcurrentCommit {
            preserved_paths: existing_paths([path, &replacement, &backup]),
            outcome: ConcurrentCommitOutcome::Indeterminate,
        });
    }

    let destination = FileStamp::of(path).ok();
    let backup_stamp = FileStamp::of(&backup).ok();
    if let (Some(destination), Some(backup_stamp)) = (&destination, &backup_stamp)
        && backup_stamp == expected
        && prepared_matches(destination, bytes)
    {
        remove_verified_backup(&backup, expected).map_err(|_| SaveError::ConcurrentCommit {
            preserved_paths: vec![backup],
            outcome: ConcurrentCommitOutcome::Indeterminate,
        })?;
        return Ok(destination.clone());
    }

    rollback_after_unverified_replace(path, &backup, bytes)
}

fn verify_prepared_destination(path: &Path, bytes: &[u8]) -> Result<FileStamp, SaveError> {
    let stamp = FileStamp::of(path).map_err(SaveError::Io)?;
    if prepared_matches(&stamp, bytes) {
        Ok(stamp)
    } else {
        Err(SaveError::ConcurrentCommit {
            preserved_paths: vec![path.to_path_buf()],
            outcome: ConcurrentCommitOutcome::Indeterminate,
        })
    }
}

#[cfg(windows)]
struct ReservationError {
    preserved_paths: Vec<PathBuf>,
}

#[cfg(windows)]
fn reserve_sibling_path(path: &Path, prefix: &str) -> Result<PathBuf, ReservationError> {
    let dir = path.parent().unwrap_or(Path::new("."));
    let temporary = tempfile::Builder::new()
        .prefix(prefix)
        .tempfile_in(dir)
        .map_err(|_| ReservationError {
            preserved_paths: Vec::new(),
        })?;
    let (file, reserved) = match temporary.keep() {
        Ok(reserved) => reserved,
        Err(error) => {
            let (file, mut temporary_path) = error.file.into_parts();
            drop(file);
            let retained = temporary_path.to_path_buf();
            temporary_path.disable_cleanup(true);
            drop(temporary_path);
            return Err(ReservationError {
                preserved_paths: existing_paths([&retained]),
            });
        }
    };
    drop(file);
    if run_save_reservation_hook(&reserved)
        .and_then(|_| std::fs::remove_file(&reserved))
        .is_err()
    {
        return Err(ReservationError {
            preserved_paths: existing_paths([&reserved]),
        });
    }
    Ok(reserved)
}

#[cfg(windows)]
fn replace_file(destination: &Path, replacement: &Path, backup: &Path) -> std::io::Result<()> {
    use std::{iter, os::windows::ffi::OsStrExt};
    use windows::{
        Win32::Storage::FileSystem::{REPLACEFILE_WRITE_THROUGH, ReplaceFileW},
        core::PCWSTR,
    };

    let wide = |path: &Path| -> Vec<u16> {
        path.as_os_str()
            .encode_wide()
            .chain(iter::once(0))
            .collect()
    };
    let destination = wide(destination);
    let replacement = wide(replacement);
    let backup = wide(backup);
    // SAFETY: all paths are NUL-terminated UTF-16 buffers that remain live for
    // this call. `ReplaceFileW` performs one filesystem replacement.
    unsafe {
        ReplaceFileW(
            PCWSTR(destination.as_ptr()),
            PCWSTR(replacement.as_ptr()),
            PCWSTR(backup.as_ptr()),
            REPLACEFILE_WRITE_THROUGH,
            None,
            None,
        )
    }
    .map_err(std::io::Error::other)
}

#[cfg(windows)]
fn rollback_after_unverified_replace(
    destination: &Path,
    backup: &Path,
    bytes: &[u8],
) -> Result<FileStamp, SaveError> {
    let destination_stamp = FileStamp::of(destination).ok();
    let backup_stamp = FileStamp::of(backup).ok();
    if !destination_stamp.is_some_and(|stamp| prepared_matches(&stamp, bytes)) {
        return Err(SaveError::ConcurrentCommit {
            preserved_paths: existing_paths([destination, backup]),
            outcome: ConcurrentCommitOutcome::Indeterminate,
        });
    }
    let Some(backup_stamp) = backup_stamp else {
        return Err(SaveError::ConcurrentCommit {
            preserved_paths: existing_paths([destination, backup]),
            outcome: ConcurrentCommitOutcome::Indeterminate,
        });
    };

    let rollback = match reserve_sibling_path(destination, ".markturbo-rollback-") {
        Ok(rollback) => rollback,
        Err(reservation) => {
            let mut preserved_paths = reservation.preserved_paths;
            extend_existing_paths(&mut preserved_paths, [destination, backup]);
            return Err(SaveError::ConcurrentCommit {
                preserved_paths,
                outcome: ConcurrentCommitOutcome::Indeterminate,
            });
        }
    };
    if replace_file(destination, backup, &rollback).is_ok()
        && FileStamp::of(destination).is_ok_and(|stamp| stamp == backup_stamp)
        && FileStamp::of(&rollback).is_ok_and(|stamp| prepared_matches(&stamp, bytes))
    {
        return Err(SaveError::ConcurrentCommit {
            preserved_paths: vec![rollback],
            outcome: ConcurrentCommitOutcome::ExternalVersionRestored,
        });
    }

    Err(SaveError::ConcurrentCommit {
        preserved_paths: existing_paths([destination, backup, &rollback]),
        outcome: ConcurrentCommitOutcome::Indeterminate,
    })
}

fn existing_paths<const N: usize>(paths: [&Path; N]) -> Vec<PathBuf> {
    let mut existing = Vec::new();
    extend_existing_paths(&mut existing, paths);
    existing
}

fn extend_existing_paths<const N: usize>(existing: &mut Vec<PathBuf>, paths: [&Path; N]) {
    for path in paths.into_iter().filter(|path| path.exists()) {
        let path = path.to_path_buf();
        if !existing.contains(&path) {
            existing.push(path);
        }
    }
}

#[cfg(windows)]
fn remove_verified_backup(path: &Path, expected: &FileStamp) -> Result<(), std::io::Error> {
    use std::os::windows::{fs::OpenOptionsExt, io::AsRawHandle};
    use windows::Win32::{
        Foundation::HANDLE,
        Storage::FileSystem::{
            DELETE, FILE_DISPOSITION_INFO, FILE_GENERIC_READ, FILE_SHARE_READ, FileDispositionInfo,
            SetFileInformationByHandle,
        },
    };

    let mut file = std::fs::OpenOptions::new()
        .read(true)
        .access_mode((FILE_GENERIC_READ | DELETE).0)
        .share_mode(FILE_SHARE_READ.0)
        .open(path)?;
    let metadata = file.metadata()?;
    let object_id = file_object_id(&file)?;
    let mut bytes = Vec::with_capacity(usize::try_from(metadata.len()).unwrap_or(0));
    file.read_to_end(&mut bytes)?;
    if FileStamp::from_bytes(&metadata, &bytes, object_id) != *expected {
        return Err(std::io::Error::new(
            std::io::ErrorKind::WouldBlock,
            "backup changed before cleanup",
        ));
    }

    let disposition = FILE_DISPOSITION_INFO { DeleteFile: true };
    // SAFETY: `file` owns a DELETE-capable handle. The structure is initialized
    // and lives for the call, which marks this verified object for deletion.
    unsafe {
        SetFileInformationByHandle(
            HANDLE(file.as_raw_handle()),
            FileDispositionInfo,
            (&raw const disposition).cast(),
            u32::try_from(std::mem::size_of::<FILE_DISPOSITION_INFO>())
                .expect("FILE_DISPOSITION_INFO fits u32"),
        )
    }
    .map_err(std::io::Error::other)
}

#[cfg(all(test, windows))]
type SaveReservationHook = Box<dyn FnOnce(&Path) -> std::io::Result<()>>;

#[cfg(test)]
thread_local! {
    static SAVE_COMMIT_HOOK: std::cell::RefCell<Option<Box<dyn FnOnce()>>> = const {
        std::cell::RefCell::new(None)
    };
    static SAVE_POST_COMMIT_HOOK: std::cell::RefCell<Option<Box<dyn FnOnce()>>> = const {
        std::cell::RefCell::new(None)
    };
    #[cfg(all(unix, any(target_os = "linux", target_os = "macos")))]
    static SKILL_BEFORE_EXCHANGE_HOOK: std::cell::RefCell<Option<Box<dyn FnOnce(&Path, &Path)>>> = const {
        std::cell::RefCell::new(None)
    };
    #[cfg(all(unix, any(target_os = "linux", target_os = "macos")))]
    static SKILL_BEFORE_CLEANUP_HOOK: std::cell::RefCell<Option<Box<dyn FnOnce(&Path)>>> = const {
        std::cell::RefCell::new(None)
    };
    #[cfg(all(unix, any(target_os = "linux", target_os = "macos")))]
    static SKILL_FORCE_UNSUPPORTED_EXCHANGE: std::cell::Cell<bool> = const {
        std::cell::Cell::new(false)
    };
    #[cfg(windows)]
    static SAVE_RESERVATION_HOOK: std::cell::RefCell<Option<SaveReservationHook>> = const {
        std::cell::RefCell::new(None)
    };
}

#[cfg(test)]
pub(super) fn install_save_commit_hook(hook: impl FnOnce() + 'static) {
    SAVE_COMMIT_HOOK.with(|slot| {
        assert!(
            slot.borrow().is_none(),
            "a save commit hook is already installed"
        );
        *slot.borrow_mut() = Some(Box::new(hook));
    });
}

#[cfg(test)]
pub(super) fn install_save_post_commit_hook(hook: impl FnOnce() + 'static) {
    SAVE_POST_COMMIT_HOOK.with(|slot| {
        assert!(
            slot.borrow().is_none(),
            "a save post-commit hook is already installed"
        );
        *slot.borrow_mut() = Some(Box::new(hook));
    });
}

#[cfg(all(test, unix, any(target_os = "linux", target_os = "macos")))]
pub(super) fn install_skill_before_exchange_hook(hook: impl FnOnce(&Path, &Path) + 'static) {
    SKILL_BEFORE_EXCHANGE_HOOK.with(|slot| {
        assert!(
            slot.borrow().is_none(),
            "a Skill exchange hook is already installed"
        );
        *slot.borrow_mut() = Some(Box::new(hook));
    });
}

#[cfg(all(test, unix, any(target_os = "linux", target_os = "macos")))]
pub(super) fn install_skill_before_cleanup_hook(hook: impl FnOnce(&Path) + 'static) {
    SKILL_BEFORE_CLEANUP_HOOK.with(|slot| {
        assert!(
            slot.borrow().is_none(),
            "a Skill cleanup hook is already installed"
        );
        *slot.borrow_mut() = Some(Box::new(hook));
    });
}

#[cfg(all(test, unix, any(target_os = "linux", target_os = "macos")))]
pub(super) fn force_skill_exchange_unsupported() {
    SKILL_FORCE_UNSUPPORTED_EXCHANGE.with(|flag| flag.set(true));
}

#[cfg(all(test, windows))]
pub(super) fn install_save_reservation_hook(
    hook: impl FnOnce(&Path) -> std::io::Result<()> + 'static,
) {
    SAVE_RESERVATION_HOOK.with(|slot| {
        assert!(
            slot.borrow().is_none(),
            "a save reservation hook is already installed"
        );
        *slot.borrow_mut() = Some(Box::new(hook));
    });
}

#[cfg(test)]
fn run_save_commit_hook() {
    SAVE_COMMIT_HOOK.with(|slot| {
        if let Some(hook) = slot.borrow_mut().take() {
            hook();
        }
    });
}

#[cfg(test)]
fn run_save_post_commit_hook() {
    SAVE_POST_COMMIT_HOOK.with(|slot| {
        if let Some(hook) = slot.borrow_mut().take() {
            hook();
        }
    });
}

#[cfg(all(test, unix, any(target_os = "linux", target_os = "macos")))]
fn run_skill_before_exchange_hook(root: &Path, staged: &Path) {
    SKILL_BEFORE_EXCHANGE_HOOK.with(|slot| {
        if let Some(hook) = slot.borrow_mut().take() {
            hook(root, staged);
        }
    });
}

#[cfg(all(test, unix, any(target_os = "linux", target_os = "macos")))]
fn run_skill_before_cleanup_hook(root: &Path) {
    SKILL_BEFORE_CLEANUP_HOOK.with(|slot| {
        if let Some(hook) = slot.borrow_mut().take() {
            hook(root);
        }
    });
}

#[cfg(all(test, windows))]
fn run_save_reservation_hook(path: &Path) -> std::io::Result<()> {
    SAVE_RESERVATION_HOOK.with(|slot| {
        if let Some(hook) = slot.borrow_mut().take() {
            hook(path)
        } else {
            Ok(())
        }
    })
}

#[cfg(not(test))]
fn run_save_commit_hook() {}

#[cfg(not(test))]
fn run_save_post_commit_hook() {}

#[cfg(all(windows, not(test)))]
fn run_save_reservation_hook(_path: &Path) -> std::io::Result<()> {
    Ok(())
}
