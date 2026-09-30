//! Reading and writing workspace files safely.
//!
//! Three invariants:
//!
//! * A save never clobbers a change made outside the app. We record the
//!   modification time and size seen when the file was loaded, and refuse to
//!   write if the file on disk no longer matches.
//! * Files stay ordinary files. No proprietary format, no reformatting, no
//!   forced newline conversion — a document written back unchanged is
//!   byte-identical to what was read.
//! * A file this app cannot decode is never silently rewritten as something
//!   else. Text is decoded through its detected encoding and re-encoded in the
//!   same one on save, so a GBK document opened and saved untouched stays GBK.

use std::{
    fs::File,
    io::Read,
    path::{Path, PathBuf},
    time::SystemTime,
};

use encoding_rs::{Encoding, UTF_8};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

mod commit;

#[cfg(all(test, windows))]
use self::commit::install_save_reservation_hook;
#[cfg(all(test, unix, any(target_os = "linux", target_os = "macos")))]
use self::commit::{
    force_skill_exchange_unsupported, install_skill_before_cleanup_hook,
    install_skill_before_exchange_hook,
};
#[cfg(test)]
use self::commit::{install_save_commit_hook, install_save_post_commit_hook};

/// Stable identity for one filesystem object.
///
/// Windows supplies this from an open file handle, so replacing a path with a
/// distinct file remains detectable even when the replacement preserves bytes,
/// length, and modification time.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileObjectId {
    pub volume_serial_number: u64,
    pub file_id: [u8; 16],
}

/// What we knew about a file when we last read or wrote it.
///
/// The cheap metadata remains useful for rejecting obvious changes, but the
/// digest is authoritative when both length and observable mtime are unchanged.
/// Filesystems with coarse timestamps make that case ordinary, not theoretical.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileStamp {
    pub modified: Option<SystemTime>,
    pub len: u64,
    pub digest: [u8; 32],
    pub object_id: Option<FileObjectId>,
}

impl FileStamp {
    pub fn of(path: &Path) -> std::io::Result<Self> {
        Ok(read_snapshot(path)?.stamp)
    }

    pub(crate) fn from_bytes(
        meta: &std::fs::Metadata,
        bytes: &[u8],
        object_id: Option<FileObjectId>,
    ) -> Self {
        Self {
            modified: meta.modified().ok(),
            len: meta.len(),
            digest: Sha256::digest(bytes).into(),
            object_id,
        }
    }

    /// True when the file on disk still matches this stamp.
    pub fn matches(&self, path: &Path) -> bool {
        Self::of(path).is_ok_and(|current| current == *self)
    }
}

/// Compare literal paths first so delete/rename notifications remain useful;
/// fall back to canonical paths while both ends still exist so equivalent link
/// spellings identify the same on-disk source.
pub fn paths_match(left: &Path, right: &Path) -> bool {
    left == right
        || std::fs::canonicalize(left)
            .ok()
            .zip(std::fs::canonicalize(right).ok())
            .is_some_and(|(left, right)| left == right)
}

struct FileSnapshot {
    bytes: Vec<u8>,
    stamp: FileStamp,
}

/// Read one internally consistent view of a path.
///
/// The bytes, metadata, digest, and Windows object identity all come from one
/// open handle. On Windows the handle denies writes and deletes while the
/// snapshot is read, then a second handle proves the path still resolves to the
/// same object before the first one is released.
fn read_snapshot(path: &Path) -> std::io::Result<FileSnapshot> {
    const MAX_ATTEMPTS: usize = 3;

    let mut last_race = None;
    for _ in 0..MAX_ATTEMPTS {
        match read_snapshot_once(path) {
            Ok(Some(snapshot)) => return Ok(snapshot),
            Ok(None) => {
                last_race = Some(std::io::Error::new(
                    std::io::ErrorKind::WouldBlock,
                    "file changed while being read",
                ));
            }
            Err(error)
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::NotFound | std::io::ErrorKind::WouldBlock
                ) =>
            {
                last_race = Some(error);
            }
            Err(error) => return Err(error),
        }
    }

    Err(last_race.unwrap_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::WouldBlock,
            "file changed while being read",
        )
    }))
}

fn read_snapshot_once(path: &Path) -> std::io::Result<Option<FileSnapshot>> {
    let mut file = open_snapshot_file(path)?;
    let metadata = file.metadata()?;
    let object_id = file_object_id(&file)?;
    let mut bytes = Vec::with_capacity(usize::try_from(metadata.len()).unwrap_or(0));
    file.read_to_end(&mut bytes)?;
    let stamp = FileStamp::from_bytes(&metadata, &bytes, object_id);

    #[cfg(windows)]
    {
        let current = open_snapshot_file(path)?;
        if file_object_id(&current)? != stamp.object_id {
            return Ok(None);
        }
    }

    Ok(Some(FileSnapshot { bytes, stamp }))
}

#[cfg(windows)]
fn open_snapshot_file(path: &Path) -> std::io::Result<File> {
    use std::os::windows::fs::OpenOptionsExt;
    use windows::Win32::Storage::FileSystem::FILE_SHARE_READ;

    // Permit concurrent readers only. A writer or deleter cannot replace the
    // path while this snapshot and its object-identity verification are live.
    std::fs::OpenOptions::new()
        .read(true)
        .share_mode(FILE_SHARE_READ.0)
        .open(path)
}

#[cfg(not(windows))]
fn open_snapshot_file(path: &Path) -> std::io::Result<File> {
    File::open(path)
}

#[cfg(windows)]
pub fn file_object_id(file: &File) -> std::io::Result<Option<FileObjectId>> {
    use std::os::windows::io::AsRawHandle;
    use windows::Win32::{
        Foundation::HANDLE,
        Storage::FileSystem::{
            BY_HANDLE_FILE_INFORMATION, FILE_ID_INFO, FileIdInfo, GetFileInformationByHandle,
            GetFileInformationByHandleEx,
        },
    };

    let handle = HANDLE(file.as_raw_handle());
    let mut extended = FILE_ID_INFO::default();
    // SAFETY: `file` owns a valid handle, and `extended` is a writable buffer
    // of exactly the size Windows requires for `FileIdInfo`.
    if unsafe {
        GetFileInformationByHandleEx(
            handle,
            FileIdInfo,
            (&raw mut extended).cast(),
            u32::try_from(std::mem::size_of::<FILE_ID_INFO>()).expect("FILE_ID_INFO fits u32"),
        )
    }
    .is_ok()
    {
        return Ok(Some(FileObjectId {
            volume_serial_number: extended.VolumeSerialNumber,
            file_id: extended.FileId.Identifier,
        }));
    }

    let mut legacy = BY_HANDLE_FILE_INFORMATION::default();
    // SAFETY: `file` owns a valid handle and `legacy` is a valid output buffer.
    unsafe { GetFileInformationByHandle(handle, &mut legacy) }.map_err(std::io::Error::other)?;
    let mut file_id = [0; 16];
    file_id[..8].copy_from_slice(
        &(((u64::from(legacy.nFileIndexHigh)) << 32) | u64::from(legacy.nFileIndexLow))
            .to_le_bytes(),
    );
    Ok(Some(FileObjectId {
        volume_serial_number: u64::from(legacy.dwVolumeSerialNumber),
        file_id,
    }))
}

#[cfg(not(windows))]
pub fn file_object_id(_file: &File) -> std::io::Result<Option<FileObjectId>> {
    Ok(None)
}

/// The filesystem object through which a document was opened.
///
/// A symbolic link is deliberately distinct from a regular file. Replacing a
/// link path atomically would turn it into a regular file, so saves through one
/// target the resolved file only after proving the link still names the same
/// target.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SourceIdentity {
    Regular,
    SymbolicLink {
        link_target: PathBuf,
        resolved_target: PathBuf,
    },
}

/// A filesystem identity captured when an editor opens a `SKILL.md` file.
///
/// The paths and identities are intentionally opaque to callers: only a
/// successful load or a verified save can create or refresh this binding.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkillOrigin {
    lexical_entrypoint: PathBuf,
    lexical_root: PathBuf,
    canonical_root: PathBuf,
    root_identity: SkillObjectIdentity,
    entrypoint_identity: SkillObjectIdentity,
    entrypoint_stamp: FileStamp,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SkillObjectIdentity {
    #[cfg(windows)]
    Windows(FileObjectId),
    #[cfg(unix)]
    Unix { device: u64, inode: u64 },
}

/// Root handle and component guards held while a Skill package is inspected.
///
/// Unix traversal uses `directory` as its descriptor-relative root. Windows
/// keeps every component handle open so the canonical route cannot be
/// renamed or replaced while path-based directory enumeration is in progress.
pub(crate) struct SkillOriginRoot {
    directory: File,
    canonical_root: PathBuf,
    _guards: Vec<File>,
}

impl SkillOriginRoot {
    pub(crate) fn directory(&self) -> &File {
        &self.directory
    }

    pub(crate) fn canonical_root(&self) -> &Path {
        &self.canonical_root
    }
}

impl SkillOrigin {
    /// The resolved root used by the frozen package and its navigation paths.
    pub fn canonical_root(&self) -> &Path {
        &self.canonical_root
    }

    pub(crate) fn lexical_root(&self) -> &Path {
        &self.lexical_root
    }

    fn canonical_entrypoint_path(&self) -> PathBuf {
        self.canonical_root.join("SKILL.md")
    }

    pub(crate) fn matches_source_path(&self, path: &Path) -> bool {
        absolute_lexical_path(path).is_ok_and(|path| path == self.lexical_entrypoint)
    }

    pub(crate) fn matches_entrypoint(
        &self,
        identity: SkillObjectIdentity,
        byte_size: u64,
        digest: &[u8; 32],
        modified: Option<SystemTime>,
        object_id: Option<FileObjectId>,
    ) -> bool {
        self.entrypoint_identity == identity
            && self.entrypoint_stamp.len == byte_size
            && self.entrypoint_stamp.digest == *digest
            && self.entrypoint_stamp.modified == modified
            && self.entrypoint_stamp.object_id == object_id
    }

    pub(crate) fn open_root(&self) -> std::io::Result<SkillOriginRoot> {
        let mut opened = open_canonical_skill_directory(&self.canonical_root)?;
        if skill_object_identity(&opened.directory)? != self.root_identity {
            return Err(skill_source_changed_error());
        }
        let lexical_root = open_lexical_skill_directory(&self.lexical_root)?;
        if skill_object_identity(&lexical_root)? != self.root_identity {
            return Err(skill_source_changed_error());
        }
        opened.guards.push(lexical_root);
        Ok(SkillOriginRoot {
            directory: opened.directory,
            canonical_root: self.canonical_root.clone(),
            _guards: opened.guards,
        })
    }

    fn open_root_for_save(&self) -> std::io::Result<SkillOriginRoot> {
        #[cfg(windows)]
        {
            let mut opened = open_canonical_skill_directory_for_save(&self.canonical_root)?;
            if skill_object_identity(&opened.directory)? != self.root_identity {
                return Err(skill_source_changed_error());
            }
            let lexical_root = open_lexical_skill_directory_for_save(&self.lexical_root)?;
            if skill_object_identity(&lexical_root)? != self.root_identity {
                return Err(skill_source_changed_error());
            }
            opened.guards.push(lexical_root);
            Ok(SkillOriginRoot {
                directory: opened.directory,
                canonical_root: self.canonical_root.clone(),
                _guards: opened.guards,
            })
        }
        #[cfg(not(windows))]
        {
            self.open_root()
        }
    }

    fn refreshed_after_save(&self, path: &Path, expected: &FileStamp) -> Option<Self> {
        if !self.matches_source_path(path) {
            return None;
        }
        let refreshed = try_capture_skill_origin(path)?;
        (refreshed.lexical_entrypoint == self.lexical_entrypoint
            && refreshed.canonical_root == self.canonical_root
            && refreshed.root_identity == self.root_identity
            && refreshed.entrypoint_stamp == *expected)
            .then_some(refreshed)
    }
}

struct OpenedSkillDirectory {
    directory: File,
    guards: Vec<File>,
}

/// A file loaded into the editor.
#[derive(Debug, Clone)]
pub struct LoadedFile {
    pub path: PathBuf,
    pub text: String,
    pub stamp: FileStamp,
    /// Original Skill package binding, present only when captured securely at
    /// load time (or refreshed by a verified Save As).
    pub skill_origin: Option<SkillOrigin>,
    /// The newline convention found in the file, so saving preserves it.
    pub newline: Newline,
    /// True when the file began with a UTF-8 BOM, which must be written back.
    pub had_bom: bool,
    /// The encoding the bytes were decoded from, so a save re-encodes in it.
    ///
    /// Almost always UTF-8. The exception is what this field exists for: a
    /// legacy GBK or Shift-JIS document decoded as UTF-8 becomes a wall of
    /// U+FFFD, and saving that back destroys the file. Carrying the encoding
    /// makes the round trip lossless instead.
    pub encoding: &'static Encoding,
    /// The decoder had to synthesize U+FFFD for bytes it could not represent.
    /// Saving to the original path is refused until the user explicitly
    /// chooses a UTF-8 conversion or Save As.
    pub decode_had_errors: bool,
    /// Whether the source path was a regular file or a symbolic link.
    pub source_identity: SourceIdentity,
}

impl LoadedFile {
    /// The origin captured when this `SKILL.md` was opened, if it could be
    /// bound to a stable root and opened source identity.
    pub fn skill_origin(&self) -> Option<&SkillOrigin> {
        self.skill_origin.as_ref()
    }
}

/// Which line ending the file uses.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Newline {
    Lf,
    Crlf,
}

impl Newline {
    pub fn as_str(self) -> &'static str {
        match self {
            Newline::Lf => "\n",
            Newline::Crlf => "\r\n",
        }
    }

    /// Detect the dominant convention.
    ///
    /// Mixed files exist; we pick the majority so a save does not rewrite every
    /// line of a file the user only touched in one place.
    pub fn detect(text: &str) -> Self {
        let crlf = text.matches("\r\n").count();
        let lf = text.matches('\n').count() - crlf;
        if crlf > lf {
            Newline::Crlf
        } else {
            Newline::Lf
        }
    }
}

/// Decide which encoding a file's bytes are in.
///
/// A BOM wins outright — it is a declaration, not a guess. Otherwise, valid
/// UTF-8 is taken at face value, because on a developer's machine that is what
/// almost every file is and running a detector over it can only introduce
/// error. Only bytes that are *not* valid UTF-8 reach the detector, which is
/// exactly the population it was built for: legacy content with no label.
///
/// Returns the encoding, the body with any BOM removed, and whether there was
/// one.
fn sniff_encoding(bytes: &[u8]) -> (&'static Encoding, &[u8], bool) {
    if let Some((encoding, bom_len)) = Encoding::for_bom(bytes) {
        return (encoding, &bytes[bom_len..], true);
    }
    if std::str::from_utf8(bytes).is_ok() {
        return (UTF_8, bytes, false);
    }
    // `Iso2022JpDetection::Allow`: the security reason for denying it is that a
    // browser can be tricked into running script from a mis-detected page. This
    // is a text editor with no script engine, and denying it would misdecode
    // exactly the Japanese mail archives the encoding still appears in.
    let mut detector = chardetng::EncodingDetector::new(chardetng::Iso2022JpDetection::Allow);
    detector.feed(bytes, true);
    // `Deny` — UTF-8 was already ruled out above, and letting the detector
    // return it anyway would put us back to lossy decoding.
    (
        detector.guess(None, chardetng::Utf8Detection::Deny),
        bytes,
        false,
    )
}

/// Read a file for editing.
///
/// Line endings are normalized to `\n` in memory — the editor and every parser
/// want one convention — and restored on save. Undecodable bytes are replaced
/// rather than rejected so a file with one bad byte still opens.
pub fn load(path: &Path) -> std::io::Result<LoadedFile> {
    if is_skill_entrypoint(path)
        && let Some((snapshot, source_identity, skill_origin)) = try_load_skill_source(path)
    {
        return loaded_file_from_snapshot(path, snapshot, source_identity, Some(skill_origin));
    }

    const MAX_ATTEMPTS: usize = 3;

    let mut stable = None;
    for _ in 0..MAX_ATTEMPTS {
        let before = source_identity(path)?;
        let snapshot = read_snapshot(path)?;
        let after = source_identity(path)?;
        if before == after {
            stable = Some((snapshot, after));
            break;
        }
    }
    let (snapshot, source_identity) = stable.ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::WouldBlock,
            "file changed while being read",
        )
    })?;

    loaded_file_from_snapshot(path, snapshot, source_identity, None)
}

fn loaded_file_from_snapshot(
    path: &Path,
    snapshot: FileSnapshot,
    source_identity: SourceIdentity,
    skill_origin: Option<SkillOrigin>,
) -> std::io::Result<LoadedFile> {
    let (encoding, body, had_bom) = sniff_encoding(&snapshot.bytes);
    let (raw, decode_had_errors) = encoding.decode_without_bom_handling(body);
    let raw = raw.into_owned();

    let newline = Newline::detect(&raw);
    let text = if newline == Newline::Crlf {
        raw.replace("\r\n", "\n")
    } else {
        raw
    };

    Ok(LoadedFile {
        path: path.to_path_buf(),
        text,
        stamp: snapshot.stamp,
        skill_origin,
        newline,
        had_bom,
        encoding,
        decode_had_errors,
        source_identity,
    })
}

pub(crate) fn loaded_file_from_frozen_snapshot(
    path: &Path,
    bytes: Vec<u8>,
    stamp: FileStamp,
) -> std::io::Result<LoadedFile> {
    loaded_file_from_snapshot(
        path,
        FileSnapshot { bytes, stamp },
        SourceIdentity::Regular,
        None,
    )
}

fn is_skill_entrypoint(path: &Path) -> bool {
    path.file_name() == Some(std::ffi::OsStr::new("SKILL.md"))
}

fn absolute_lexical_path(path: &Path) -> std::io::Result<PathBuf> {
    if path.is_absolute() {
        Ok(path.to_path_buf())
    } else {
        Ok(std::env::current_dir()?.join(path))
    }
}

fn try_capture_skill_origin(path: &Path) -> Option<SkillOrigin> {
    try_load_skill_source(path).map(|(_, _, origin)| origin)
}

/// Read `SKILL.md` through a pinned root. Failure to establish provenance is
/// deliberately distinct from failure to open the document: `load` falls back
/// to its ordinary editable-file path without granting Skill Review authority.
fn try_load_skill_source(path: &Path) -> Option<(FileSnapshot, SourceIdentity, SkillOrigin)> {
    if !is_skill_entrypoint(path) {
        return None;
    }

    let lexical_entrypoint = absolute_lexical_path(path).ok()?;
    let lexical_root = lexical_entrypoint.parent()?.to_path_buf();
    let canonical_root = std::fs::canonicalize(&lexical_root).ok()?;
    let mut root = open_canonical_skill_directory(&canonical_root).ok()?;
    let root_identity = skill_object_identity(&root.directory).ok()?;
    let lexical_root_handle = open_lexical_skill_directory(&lexical_root).ok()?;
    if skill_object_identity(&lexical_root_handle).ok()? != root_identity {
        return None;
    }
    root.guards.push(lexical_root_handle);

    let mut entrypoint = open_skill_entrypoint(&root.directory, &canonical_root).ok()?;
    let before = entrypoint.metadata().ok()?;
    if !opened_skill_entrypoint_is_regular(&before) {
        return None;
    }
    let entrypoint_identity = skill_object_identity(&entrypoint).ok()?;
    let object_id = file_object_id(&entrypoint).ok()?;
    let mut bytes = Vec::with_capacity(usize::try_from(before.len()).unwrap_or(0));
    entrypoint.read_to_end(&mut bytes).ok()?;
    let after = entrypoint.metadata().ok()?;
    if !opened_skill_entrypoint_is_regular(&after)
        || before.len() != bytes.len() as u64
        || after.len() != bytes.len() as u64
        || before.modified().ok() != after.modified().ok()
        || skill_object_identity(&entrypoint).ok()? != entrypoint_identity
    {
        return None;
    }
    let stamp = FileStamp::from_bytes(&after, &bytes, object_id);

    // Prove the directory entry still names the file read through this root.
    let reopened = open_skill_entrypoint(&root.directory, &canonical_root).ok()?;
    let reopened_metadata = reopened.metadata().ok()?;
    if !opened_skill_entrypoint_is_regular(&reopened_metadata)
        || skill_object_identity(&reopened).ok()? != entrypoint_identity
        || reopened_metadata.len() != after.len()
        || reopened_metadata.modified().ok() != after.modified().ok()
    {
        return None;
    }

    let origin = SkillOrigin {
        lexical_entrypoint,
        lexical_root,
        canonical_root,
        root_identity,
        entrypoint_identity,
        entrypoint_stamp: stamp.clone(),
    };
    // Also ensure a symlinked ancestor still resolves to the pinned directory
    // after the entrypoint has been read.
    origin.open_root().ok()?;

    Some((
        FileSnapshot { bytes, stamp },
        SourceIdentity::Regular,
        origin,
    ))
}

pub(crate) fn skill_object_identity(file: &File) -> std::io::Result<SkillObjectIdentity> {
    #[cfg(windows)]
    {
        file_object_id(file)?
            .map(SkillObjectIdentity::Windows)
            .ok_or_else(|| std::io::Error::other("Windows file identity is unavailable"))
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt as _;
        let metadata = file.metadata()?;
        Ok(SkillObjectIdentity::Unix {
            device: metadata.dev(),
            inode: metadata.ino(),
        })
    }
    #[cfg(not(any(windows, unix)))]
    {
        let _ = file;
        Err(std::io::Error::new(
            std::io::ErrorKind::Unsupported,
            "stable Skill file identity is unavailable on this target",
        ))
    }
}

fn skill_source_changed_error() -> std::io::Error {
    std::io::Error::other("Agent Skill origin no longer names its loaded directory")
}

#[cfg(windows)]
fn open_skill_directory(path: &Path) -> std::io::Result<File> {
    use windows::Win32::Storage::FileSystem::FILE_SHARE_READ;
    open_skill_directory_with_share(path, FILE_SHARE_READ.0)
}

#[cfg(windows)]
fn open_skill_directory_for_save(path: &Path) -> std::io::Result<File> {
    use windows::Win32::Storage::FileSystem::{FILE_SHARE_READ, FILE_SHARE_WRITE};
    // Allow the sibling temp-file/replace sequence to mutate directory
    // entries while denying DELETE sharing keeps the authenticated route fixed.
    open_skill_directory_with_share(path, (FILE_SHARE_READ | FILE_SHARE_WRITE).0)
}

#[cfg(windows)]
fn open_skill_directory_with_share(path: &Path, share_mode: u32) -> std::io::Result<File> {
    use std::os::windows::fs::{MetadataExt as _, OpenOptionsExt as _};
    use windows::Win32::Storage::FileSystem::{
        FILE_ATTRIBUTE_REPARSE_POINT, FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT,
    };

    let directory = std::fs::OpenOptions::new()
        .read(true)
        .share_mode(share_mode)
        .custom_flags(FILE_FLAG_BACKUP_SEMANTICS.0 | FILE_FLAG_OPEN_REPARSE_POINT.0)
        .open(path)?;
    let metadata = directory.metadata()?;
    if !metadata.is_dir() || metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT.0 != 0 {
        return Err(skill_source_changed_error());
    }
    Ok(directory)
}

#[cfg(windows)]
fn open_lexical_skill_directory(path: &Path) -> std::io::Result<File> {
    open_skill_directory(path)
}

#[cfg(windows)]
fn open_lexical_skill_directory_for_save(path: &Path) -> std::io::Result<File> {
    open_skill_directory_for_save(path)
}

#[cfg(windows)]
fn open_canonical_skill_directory(path: &Path) -> std::io::Result<OpenedSkillDirectory> {
    open_canonical_skill_directory_with(path, open_skill_directory)
}

#[cfg(windows)]
fn open_canonical_skill_directory_for_save(path: &Path) -> std::io::Result<OpenedSkillDirectory> {
    open_canonical_skill_directory_with(path, open_skill_directory_for_save)
}

#[cfg(windows)]
fn open_canonical_skill_directory_with(
    path: &Path,
    open_directory: fn(&Path) -> std::io::Result<File>,
) -> std::io::Result<OpenedSkillDirectory> {
    if !path.is_absolute() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "canonical Agent Skill root must be absolute",
        ));
    }
    let mut ancestors = path.ancestors().map(Path::to_path_buf).collect::<Vec<_>>();
    ancestors.reverse();
    let mut guards = Vec::with_capacity(ancestors.len().saturating_sub(1));
    let mut directory = None;
    for ancestor in ancestors {
        let next = open_directory(&ancestor)?;
        if let Some(previous) = directory.replace(next) {
            guards.push(previous);
        }
    }
    Ok(OpenedSkillDirectory {
        directory: directory.ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "canonical Agent Skill root has no directory components",
            )
        })?,
        guards,
    })
}

#[cfg(windows)]
fn open_skill_entrypoint(root: &File, canonical_root: &Path) -> std::io::Result<File> {
    use std::os::windows::fs::OpenOptionsExt as _;
    use windows::Win32::Storage::FileSystem::{FILE_FLAG_OPEN_REPARSE_POINT, FILE_SHARE_READ};

    let _ = root;
    std::fs::OpenOptions::new()
        .read(true)
        .share_mode(FILE_SHARE_READ.0)
        .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT.0)
        .open(canonical_root.join("SKILL.md"))
}

#[cfg(windows)]
fn opened_skill_entrypoint_is_regular(metadata: &std::fs::Metadata) -> bool {
    use std::os::windows::fs::MetadataExt as _;
    use windows::Win32::Storage::FileSystem::FILE_ATTRIBUTE_REPARSE_POINT;

    metadata.is_file() && metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT.0 == 0
}

// `OpenOptionsExt::custom_flags` exposes raw fcntl flags but std has no named
// constants; these values are the Linux and Darwin `<fcntl.h>` definitions.
#[cfg(all(unix, any(target_os = "linux", target_os = "macos")))]
const UNIX_O_NOFOLLOW: i32 = if cfg!(target_os = "linux") {
    0o400000
} else {
    0x100
};

#[cfg(all(unix, any(target_os = "linux", target_os = "macos")))]
const UNIX_O_DIRECTORY: i32 = if cfg!(target_os = "linux") {
    0o200000
} else {
    0x100000
};

#[cfg(all(unix, any(target_os = "linux", target_os = "macos")))]
const UNIX_O_CLOEXEC: i32 = if cfg!(target_os = "linux") {
    0o2000000
} else {
    0x1000000
};

#[cfg(all(unix, any(target_os = "linux", target_os = "macos")))]
const UNIX_O_NONBLOCK: i32 = if cfg!(target_os = "linux") {
    0o4000
} else {
    0x4
};

#[cfg(all(unix, any(target_os = "linux", target_os = "macos")))]
fn openat_skill_file(parent: &File, name: &std::ffi::OsStr, flags: i32) -> std::io::Result<File> {
    use std::ffi::{CString, c_char, c_int};
    use std::os::fd::{AsRawFd as _, FromRawFd as _};
    use std::os::unix::ffi::OsStrExt as _;

    unsafe extern "C" {
        fn openat(directory: c_int, path: *const c_char, flags: c_int) -> c_int;
    }

    let name = CString::new(name.as_bytes()).map_err(|_| {
        std::io::Error::new(std::io::ErrorKind::InvalidInput, "NUL in Agent Skill path")
    })?;
    // SAFETY: `parent` owns a live directory descriptor and `name` is a
    // NUL-terminated single path component.
    let descriptor = unsafe { openat(parent.as_raw_fd(), name.as_ptr(), flags) };
    if descriptor < 0 {
        return Err(std::io::Error::last_os_error());
    }
    // SAFETY: `openat` returned a new owned descriptor.
    Ok(unsafe { File::from_raw_fd(descriptor) })
}

#[cfg(all(unix, any(target_os = "linux", target_os = "macos")))]
fn open_lexical_skill_directory(path: &Path) -> std::io::Result<File> {
    use std::os::unix::fs::OpenOptionsExt as _;

    std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(UNIX_O_NOFOLLOW | UNIX_O_DIRECTORY)
        .open(path)
}

#[cfg(all(unix, any(target_os = "linux", target_os = "macos")))]
fn open_canonical_skill_directory(path: &Path) -> std::io::Result<OpenedSkillDirectory> {
    use std::os::unix::fs::OpenOptionsExt as _;

    if !path.is_absolute() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "canonical Agent Skill root must be absolute",
        ));
    }
    let mut directory = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(UNIX_O_NOFOLLOW | UNIX_O_DIRECTORY)
        .open(Path::new("/"))?;
    let mut guards = Vec::new();
    for component in path.components() {
        match component {
            std::path::Component::RootDir | std::path::Component::CurDir => {}
            std::path::Component::Normal(name) => {
                let next = openat_skill_file(
                    &directory,
                    name,
                    UNIX_O_NOFOLLOW | UNIX_O_DIRECTORY | UNIX_O_CLOEXEC,
                )?;
                if !next.metadata()?.is_dir() {
                    return Err(skill_source_changed_error());
                }
                guards.push(directory);
                directory = next;
            }
            std::path::Component::ParentDir | std::path::Component::Prefix(_) => {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    "canonical Agent Skill root contains an invalid component",
                ));
            }
        }
    }
    Ok(OpenedSkillDirectory { directory, guards })
}

#[cfg(all(unix, any(target_os = "linux", target_os = "macos")))]
fn open_skill_entrypoint(root: &File, _canonical_root: &Path) -> std::io::Result<File> {
    openat_skill_file(
        root,
        std::ffi::OsStr::new("SKILL.md"),
        UNIX_O_NOFOLLOW | UNIX_O_NONBLOCK | UNIX_O_CLOEXEC,
    )
}

#[cfg(all(unix, any(target_os = "linux", target_os = "macos")))]
fn opened_skill_entrypoint_is_regular(metadata: &std::fs::Metadata) -> bool {
    metadata.is_file()
}

#[cfg(all(unix, not(any(target_os = "linux", target_os = "macos"))))]
fn open_lexical_skill_directory(_: &Path) -> std::io::Result<File> {
    Err(std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        "no no-follow Agent Skill directory open is available on this target",
    ))
}

#[cfg(all(unix, not(any(target_os = "linux", target_os = "macos"))))]
fn open_canonical_skill_directory(_: &Path) -> std::io::Result<OpenedSkillDirectory> {
    Err(std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        "no descriptor-root Agent Skill traversal is available on this target",
    ))
}

#[cfg(all(unix, not(any(target_os = "linux", target_os = "macos"))))]
fn open_skill_entrypoint(_: &File, _: &Path) -> std::io::Result<File> {
    Err(std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        "no descriptor-relative Agent Skill file open is available on this target",
    ))
}

#[cfg(all(unix, not(any(target_os = "linux", target_os = "macos"))))]
fn opened_skill_entrypoint_is_regular(_: &std::fs::Metadata) -> bool {
    false
}

#[cfg(not(any(windows, unix)))]
fn open_lexical_skill_directory(_: &Path) -> std::io::Result<File> {
    Err(std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        "no no-follow Agent Skill directory open is available on this target",
    ))
}

#[cfg(not(any(windows, unix)))]
fn open_canonical_skill_directory(_: &Path) -> std::io::Result<OpenedSkillDirectory> {
    Err(std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        "no descriptor-root Agent Skill traversal is available on this target",
    ))
}

#[cfg(not(any(windows, unix)))]
fn open_skill_entrypoint(_: &File, _: &Path) -> std::io::Result<File> {
    Err(std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        "no no-follow Agent Skill file open is available on this target",
    ))
}

#[cfg(not(any(windows, unix)))]
fn opened_skill_entrypoint_is_regular(_: &std::fs::Metadata) -> bool {
    false
}

#[cfg(windows)]
fn open_source_entry_guard(path: &Path) -> std::io::Result<File> {
    use std::os::windows::fs::OpenOptionsExt;
    use windows::Win32::Storage::FileSystem::{FILE_FLAG_OPEN_REPARSE_POINT, FILE_SHARE_READ};

    // FILE_FLAG_OPEN_REPARSE_POINT opens the final entry itself. Sharing only
    // reads prevents DeleteFile/rename or a reparse-data write until this guard
    // drops, while allowing ordinary readers.
    std::fs::OpenOptions::new()
        .read(true)
        .share_mode(FILE_SHARE_READ.0)
        .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT.0)
        .open(path)
}

/// Check whether a recovery record still describes the source on disk.
///
/// Windows holds the final path entry open before inspecting any symbolic-link
/// target. This makes a regular-to-link replacement a clean mismatch even when
/// the replacement target cannot be opened, and keeps a matching link stable
/// until the complete loaded identity has been compared.
pub fn recovery_source_matches(
    path: &Path,
    expected_stamp: &FileStamp,
    expected_source_identity: &SourceIdentity,
) -> std::io::Result<bool> {
    #[cfg(windows)]
    let _source_guard = {
        let guard = open_source_entry_guard(path)?;
        let is_name_surrogate = guard.metadata()?.file_type().is_symlink();
        match expected_source_identity {
            SourceIdentity::Regular if is_name_surrogate => return Ok(false),
            SourceIdentity::SymbolicLink { .. } if !is_name_surrogate => return Ok(false),
            SourceIdentity::SymbolicLink { link_target, .. } => {
                if std::fs::read_link(path)? != *link_target {
                    return Ok(false);
                }
            }
            SourceIdentity::Regular => {}
        }
        guard
    };

    #[cfg(not(windows))]
    if source_identity(path)? != *expected_source_identity {
        return Ok(false);
    }

    let loaded = load(path)?;
    let matches =
        loaded.stamp == *expected_stamp && loaded.source_identity == *expected_source_identity;
    #[cfg(all(test, windows))]
    run_recovery_source_match_hook();
    Ok(matches)
}

/// Why a save did not happen.
#[derive(Debug)]
pub enum SaveError {
    /// The file changed on disk since it was loaded. The caller must resolve
    /// this with the user before overwriting.
    Conflict,
    /// Save As names an existing filesystem entry. Replacing it requires a
    /// separate, explicit overwrite decision.
    DestinationExists,
    /// The path no longer exists. Recreating it is a separate user decision.
    Missing,
    /// The path changed between regular-file and symbolic-link identity, or a
    /// link now points somewhere else.
    SourceIdentityChanged,
    /// Original bytes could not be decoded exactly.
    DecodeLoss,
    /// The editor contains text the original encoding cannot represent.
    Unrepresentable {
        encoding: &'static str,
    },
    /// A replacement may have raced an external writer. Listed paths are only
    /// locations whose current entries could be identity-checked; the list may
    /// be empty when a pinned root no longer has a trustworthy pathname. The
    /// editor buffer remains authoritative until the user resolves the result.
    ConcurrentCommit {
        preserved_paths: Vec<PathBuf>,
        outcome: ConcurrentCommitOutcome,
    },
    Io(std::io::Error),
}

/// What we could prove after a concurrent Windows replace.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConcurrentCommitOutcome {
    /// The external bytes were put back at the original save destination and our prepared
    /// bytes were retained at one of `SaveError::ConcurrentCommit`'s paths.
    ExternalVersionRestored,
    /// The filesystem did not provide enough evidence to state which version
    /// is at the destination. `preserved_paths` lists only entries whose
    /// identities were still verifiable; it may be empty.
    Indeterminate,
}

impl std::fmt::Display for SaveError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SaveError::Conflict => {
                f.write_str("the file changed on disk since it was opened; reload or save a copy")
            }
            SaveError::DestinationExists => {
                f.write_str("the save destination already exists; choose whether to replace it")
            }
            SaveError::Missing => {
                f.write_str("the source path no longer exists; recreate it or save a copy")
            }
            SaveError::SourceIdentityChanged => {
                f.write_str("the source path or symbolic-link target changed; save a copy")
            }
            SaveError::DecodeLoss => f.write_str(
                "the original bytes could not be decoded exactly; convert to UTF-8 or save a copy",
            ),
            SaveError::Unrepresentable { encoding } => write!(
                f,
                "the editor text cannot be represented as {encoding}; convert to UTF-8 or save a copy"
            ),
            SaveError::ConcurrentCommit {
                outcome: ConcurrentCommitOutcome::ExternalVersionRestored,
                ..
            } => f.write_str(
                "a concurrent write was restored to the original save destination; inspect the retained copy and save a copy",
            ),
            SaveError::ConcurrentCommit {
                outcome: ConcurrentCommitOutcome::Indeterminate,
                ..
            } => f.write_str(
                "a concurrent write made the save outcome indeterminate; inspect retained files and save a copy",
            ),
            SaveError::Io(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for SaveError {}

/// Write `text` back to `file.path`, refusing if the file changed externally.
///
/// Returns the new stamp on success. `force` permits overwriting only the
/// fresh fingerprint observed for the user's explicit decision.
pub fn save(file: &LoadedFile, text: &str, force: bool) -> Result<FileStamp, SaveError> {
    let authorization = if force {
        SaveAuthorization::normal().authorize_current_overwrite(file)?
    } else {
        SaveAuthorization::normal()
    };
    save_with(file, text, &authorization).map(|saved| saved.stamp)
}

/// Explicit, composable permissions for one save attempt.
///
/// The source permission retains the exact object state observed when the user
/// made a destructive choice. Adding UTF-8 conversion never broadens that
/// source permission to a later external writer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SaveAuthorization {
    source: SourceSaveAuthorization,
    convert_to_utf8: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum SourceSaveAuthorization {
    Normal,
    Overwrite { stamp: FileStamp },
    RecreateMissing,
}

impl SaveAuthorization {
    pub fn normal() -> Self {
        Self {
            source: SourceSaveAuthorization::Normal,
            convert_to_utf8: false,
        }
    }

    /// Record the exact current version the user chose to overwrite.
    pub fn authorize_current_overwrite(&self, file: &LoadedFile) -> Result<Self, SaveError> {
        let destination = current_save_destination(file)?;
        let mut authorization = self.clone();
        authorization.source = SourceSaveAuthorization::Overwrite {
            stamp: FileStamp::of(&destination).map_err(SaveError::Io)?,
        };
        Ok(authorization)
    }

    /// Record that the user chose to recreate this currently missing regular
    /// source. A path that reappears before commit remains a conflict.
    pub fn authorize_missing_recreation(&self, file: &LoadedFile) -> Result<Self, SaveError> {
        if file.source_identity != SourceIdentity::Regular {
            return Err(SaveError::SourceIdentityChanged);
        }
        match source_identity(&file.path) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                let mut authorization = self.clone();
                authorization.source = SourceSaveAuthorization::RecreateMissing;
                Ok(authorization)
            }
            Ok(SourceIdentity::Regular) => Err(SaveError::Conflict),
            Ok(SourceIdentity::SymbolicLink { .. }) => Err(SaveError::SourceIdentityChanged),
            Err(error) => Err(SaveError::Io(error)),
        }
    }

    /// Add the user's explicit permission to replace the source encoding.
    pub fn enable_utf8_conversion(mut self) -> Self {
        self.convert_to_utf8 = true;
        self
    }
}

impl Default for SaveAuthorization {
    fn default() -> Self {
        Self::normal()
    }
}

/// Metadata produced by a successful save.
#[derive(Debug, Clone)]
pub struct SaveOutcome {
    pub stamp: FileStamp,
    pub encoding: &'static Encoding,
    pub had_bom: bool,
    pub source_identity: SourceIdentity,
    /// Refreshed only when this save began with a verified Skill origin and
    /// the original root still denotes the same directory after commit.
    pub skill_origin: Option<SkillOrigin>,
}

pub fn save_with(
    file: &LoadedFile,
    text: &str,
    authorization: &SaveAuthorization,
) -> Result<SaveOutcome, SaveError> {
    let skill_root = open_loaded_skill_root(file)?;
    let (destination, recreated_missing, expected) =
        save_destination(file, authorization, skill_root.as_ref())?;

    if file.decode_had_errors && !authorization.convert_to_utf8 {
        return Err(SaveError::DecodeLoss);
    }

    let encoding = if authorization.convert_to_utf8 {
        UTF_8
    } else {
        file.encoding
    };
    let had_bom = if authorization.convert_to_utf8 {
        false
    } else {
        file.had_bom
    };
    let bytes = encode(text, file.newline, had_bom, encoding)?;
    let staged = stage(&destination, &bytes)?;
    let canonical_source = file
        .skill_origin
        .as_ref()
        .map(SkillOrigin::canonical_entrypoint_path);
    let source_path = canonical_source.as_deref().unwrap_or(&file.path);
    let regular_source_identity = SourceIdentity::Regular;
    let expected_source_identity = if file.skill_origin.is_some() {
        &regular_source_identity
    } else {
        &file.source_identity
    };
    let expected_skill_entrypoint_identity =
        if matches!(&authorization.source, SourceSaveAuthorization::Normal) {
            file.skill_origin
                .as_ref()
                .map(|origin| origin.entrypoint_identity)
        } else {
            None
        };
    let destination_authorization = match (recreated_missing, expected.as_ref()) {
        (true, None) => commit::DestinationAuthorization::CreateOnly,
        (false, Some(stamp)) => commit::DestinationAuthorization::ReplaceExpected(stamp),
        _ => return Err(SaveError::Conflict),
    };
    let committed = commit::execute(commit::CommitRequest {
        staged,
        source_path,
        expected_source_identity,
        destination: &destination,
        destination_authorization,
        prepared_bytes: &bytes,
        skill_root: skill_root.as_ref(),
        expected_skill_entrypoint_identity,
    })?;
    let stamp = committed.stamp;
    let source_identity = committed.source_identity;
    let skill_origin = file
        .skill_origin
        .as_ref()
        .and_then(|origin| origin.refreshed_after_save(&file.path, &stamp));

    Ok(SaveOutcome {
        stamp,
        encoding,
        had_bom,
        source_identity,
        skill_origin,
    })
}

fn save_destination(
    file: &LoadedFile,
    authorization: &SaveAuthorization,
    skill_root: Option<&SkillOriginRoot>,
) -> Result<(PathBuf, bool, Option<FileStamp>), SaveError> {
    if matches!(
        authorization.source,
        SourceSaveAuthorization::RecreateMissing
    ) {
        if file.source_identity != SourceIdentity::Regular {
            return Err(SaveError::SourceIdentityChanged);
        }
        let source_path = file
            .skill_origin
            .as_ref()
            .map(SkillOrigin::canonical_entrypoint_path)
            .unwrap_or_else(|| file.path.clone());
        return match source_identity(&source_path) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                Ok((source_path, true, None))
            }
            Ok(SourceIdentity::Regular) => Err(SaveError::Conflict),
            Ok(SourceIdentity::SymbolicLink { .. }) => Err(SaveError::SourceIdentityChanged),
            Err(error) => Err(SaveError::Io(error)),
        };
    }

    let destination = current_save_destination_with_root(
        file,
        skill_root,
        matches!(&authorization.source, SourceSaveAuthorization::Normal),
    )?;
    let current = FileStamp::of(&destination).map_err(SaveError::Io)?;
    let expected = match &authorization.source {
        SourceSaveAuthorization::Normal => {
            if current != file.stamp {
                return Err(SaveError::Conflict);
            }
            file.stamp.clone()
        }
        SourceSaveAuthorization::Overwrite { stamp } => {
            if current != *stamp {
                return Err(SaveError::Conflict);
            }
            stamp.clone()
        }
        SourceSaveAuthorization::RecreateMissing => unreachable!("handled above"),
    };
    Ok((destination, false, Some(expected)))
}

fn current_save_destination(file: &LoadedFile) -> Result<PathBuf, SaveError> {
    let skill_root = open_loaded_skill_root(file)?;
    current_save_destination_with_root(file, skill_root.as_ref(), false)
}

fn open_loaded_skill_root(file: &LoadedFile) -> Result<Option<SkillOriginRoot>, SaveError> {
    let Some(origin) = file.skill_origin.as_ref() else {
        return Ok(None);
    };
    if file.source_identity != SourceIdentity::Regular || !origin.matches_source_path(&file.path) {
        return Err(SaveError::SourceIdentityChanged);
    }
    origin
        .open_root_for_save()
        .map(Some)
        .map_err(|_| SaveError::SourceIdentityChanged)
}

fn current_save_destination_with_root(
    file: &LoadedFile,
    skill_root: Option<&SkillOriginRoot>,
    require_loaded_entrypoint_identity: bool,
) -> Result<PathBuf, SaveError> {
    if let Some(origin) = file.skill_origin.as_ref() {
        let root = skill_root.ok_or(SaveError::SourceIdentityChanged)?;
        let entrypoint = open_skill_entrypoint(root.directory(), root.canonical_root())
            .map_err(|_| SaveError::SourceIdentityChanged)?;
        let metadata = entrypoint
            .metadata()
            .map_err(|_| SaveError::SourceIdentityChanged)?;
        if !opened_skill_entrypoint_is_regular(&metadata)
            || (require_loaded_entrypoint_identity
                && skill_object_identity(&entrypoint)
                    .map_err(|_| SaveError::SourceIdentityChanged)?
                    != origin.entrypoint_identity)
        {
            return Err(SaveError::SourceIdentityChanged);
        }
        return Ok(origin.canonical_entrypoint_path());
    }

    let current = match source_identity(&file.path) {
        Ok(identity) => identity,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            return Err(if file.source_identity == SourceIdentity::Regular {
                SaveError::Missing
            } else {
                SaveError::SourceIdentityChanged
            });
        }
        Err(err) => return Err(SaveError::Io(err)),
    };

    if current != file.source_identity {
        return Err(SaveError::SourceIdentityChanged);
    }

    let destination = match current {
        SourceIdentity::Regular => file.path.clone(),
        SourceIdentity::SymbolicLink {
            resolved_target, ..
        } => resolved_target,
    };
    if !destination.exists() {
        return Err(if file.source_identity == SourceIdentity::Regular {
            SaveError::Missing
        } else {
            SaveError::SourceIdentityChanged
        });
    }
    Ok(destination)
}

fn source_identity(path: &Path) -> std::io::Result<SourceIdentity> {
    let metadata = std::fs::symlink_metadata(path)?;
    if metadata.file_type().is_symlink() {
        let link_target = std::fs::read_link(path)?;
        let unresolved_target = if link_target.is_absolute() {
            link_target.clone()
        } else {
            path.parent().unwrap_or(Path::new(".")).join(&link_target)
        };
        Ok(SourceIdentity::SymbolicLink {
            link_target,
            resolved_target: std::fs::canonicalize(path).unwrap_or(unresolved_target),
        })
    } else {
        Ok(SourceIdentity::Regular)
    }
}

/// Write to a new path (Save As) and return the verified file identity.
///
/// Save As creates a new entry only. The caller must obtain a separate user
/// decision and call [`overwrite_as`] before replacing an existing entry.
pub fn save_as(
    path: &Path,
    text: &str,
    newline: Newline,
    had_bom: bool,
) -> Result<LoadedFile, SaveError> {
    match std::fs::symlink_metadata(path) {
        Ok(_) => Err(SaveError::DestinationExists),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => save_as_with_snapshot(
            path,
            text,
            newline,
            had_bom,
            SaveAsTarget {
                source_identity: SourceIdentity::Regular,
                destination: path.to_path_buf(),
                expected: None,
                recreates_missing: true,
            },
        ),
        Err(error) => Err(SaveError::Io(error)),
    }
}

/// Immutable permission to replace a Save As destination.
///
/// This is captured while the confirmation is shown, not when its Replace
/// button is later pressed. Reusing it keeps a changed, removed, or retargeted
/// destination outside the user's original destructive decision.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SaveAsOverwriteAuthorization {
    source_path: PathBuf,
    source_identity: SourceIdentity,
    destination: PathBuf,
    expected: FileStamp,
}

impl SaveAsOverwriteAuthorization {
    /// Capture the exact existing entry the user may choose to replace.
    pub fn capture(path: &Path) -> Result<Self, SaveError> {
        let source_identity = match source_identity(path) {
            Ok(identity) => identity,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Err(SaveError::Missing);
            }
            Err(error) => return Err(SaveError::Io(error)),
        };
        let destination = match &source_identity {
            SourceIdentity::Regular => path.to_path_buf(),
            SourceIdentity::SymbolicLink {
                resolved_target, ..
            } => resolved_target.clone(),
        };
        let expected = FileStamp::of(&destination).map_err(|error| {
            if error.kind() == std::io::ErrorKind::NotFound {
                SaveError::Missing
            } else {
                SaveError::Io(error)
            }
        })?;

        Ok(Self {
            source_path: path.to_path_buf(),
            source_identity,
            destination,
            expected,
        })
    }

    /// Refuse a later change before the document layer starts its write.
    pub fn verify(&self) -> Result<(), SaveError> {
        match source_identity(&self.source_path) {
            Ok(identity) if identity == self.source_identity => {}
            Ok(_) => return Err(SaveError::Conflict),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Err(SaveError::Conflict);
            }
            Err(error) => return Err(SaveError::Io(error)),
        }
        match FileStamp::of(&self.destination) {
            Ok(stamp) if stamp == self.expected => Ok(()),
            Ok(_) => Err(SaveError::Conflict),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Err(SaveError::Conflict),
            Err(error) => Err(SaveError::Io(error)),
        }
    }
}

/// Replace an existing Save As destination after an explicit user decision.
///
/// Prefer [`overwrite_as_authorized`] when the confirmation was asynchronous,
/// so the final commit uses the snapshot observed before the prompt appeared.
pub fn overwrite_as(
    path: &Path,
    text: &str,
    newline: Newline,
    had_bom: bool,
) -> Result<LoadedFile, SaveError> {
    let authorization = SaveAsOverwriteAuthorization::capture(path)?;
    overwrite_as_authorized(&authorization, text, newline, had_bom)
}

/// Replace a Save As destination using an earlier explicit authorization.
pub fn overwrite_as_authorized(
    authorization: &SaveAsOverwriteAuthorization,
    text: &str,
    newline: Newline,
    had_bom: bool,
) -> Result<LoadedFile, SaveError> {
    authorization.verify()?;
    save_as_with_snapshot(
        &authorization.source_path,
        text,
        newline,
        had_bom,
        SaveAsTarget {
            source_identity: authorization.source_identity.clone(),
            destination: authorization.destination.clone(),
            expected: Some(authorization.expected.clone()),
            recreates_missing: false,
        },
    )
}

struct SaveAsTarget {
    source_identity: SourceIdentity,
    destination: PathBuf,
    expected: Option<FileStamp>,
    recreates_missing: bool,
}

fn save_as_with_snapshot(
    path: &Path,
    text: &str,
    newline: Newline,
    had_bom: bool,
    target: SaveAsTarget,
) -> Result<LoadedFile, SaveError> {
    let bytes = encode(text, newline, had_bom, UTF_8)?;
    let staged = stage(&target.destination, &bytes)?;
    let destination_authorization = match (target.recreates_missing, target.expected.as_ref()) {
        (true, None) => commit::DestinationAuthorization::CreateOnly,
        (false, Some(stamp)) => commit::DestinationAuthorization::ReplaceExpected(stamp),
        _ => return Err(SaveError::Conflict),
    };
    let committed = commit::execute(commit::CommitRequest {
        staged,
        source_path: path,
        expected_source_identity: &target.source_identity,
        destination: &target.destination,
        destination_authorization,
        prepared_bytes: &bytes,
        skill_root: None,
        expected_skill_entrypoint_identity: None,
    })?;
    let stamp = committed.stamp;
    let source_identity = committed.source_identity;
    let skill_origin =
        try_capture_skill_origin(path).filter(|origin| origin.entrypoint_stamp == stamp);

    Ok(LoadedFile {
        path: path.to_path_buf(),
        text: text.to_string(),
        stamp,
        skill_origin,
        newline,
        had_bom,
        encoding: UTF_8,
        decode_had_errors: false,
        source_identity,
    })
}

fn encode(
    text: &str,
    newline: Newline,
    had_bom: bool,
    encoding: &'static Encoding,
) -> Result<Vec<u8>, SaveError> {
    let restored;
    let text = if newline == Newline::Crlf {
        // The in-memory text uses `\n`; restore the file's convention. Guard
        // against a stray `\r\n` already present so we never emit `\r\r\n`.
        restored = text.replace("\r\n", "\n").replace('\n', "\r\n");
        restored.as_str()
    } else {
        text
    };

    let mut bytes = Vec::with_capacity(text.len() + 3);
    if had_bom {
        // The BOM that belongs to *this* encoding, not always UTF-8's. A
        // UTF-16 file written back with a UTF-8 BOM would be undecodable by
        // whatever reads it next.
        bytes.extend_from_slice(match encoding.name() {
            "UTF-16LE" => &[0xFF, 0xFE][..],
            "UTF-16BE" => &[0xFE, 0xFF][..],
            _ => &[0xEF, 0xBB, 0xBF][..],
        });
    }

    match encoding.name() {
        // `encoding_rs` has no UTF-16 *encoder*: `Encoding::encode` silently
        // falls back to UTF-8 output for these two, which would write a
        // UTF-8 body under a UTF-16 BOM — a file nothing can read. Encoding
        // the code units directly is the whole of what the encoder would do.
        name @ ("UTF-16LE" | "UTF-16BE") => {
            let big_endian = name == "UTF-16BE";
            for unit in text.encode_utf16() {
                bytes.extend_from_slice(&if big_endian {
                    unit.to_be_bytes()
                } else {
                    unit.to_le_bytes()
                });
            }
        }
        // Refuse an unmappable edit. Encoding it as a numeric character
        // reference or replacement text would silently change the source.
        _ => {
            let (encoded, _, had_errors) = encoding.encode(text);
            if had_errors {
                return Err(SaveError::Unrepresentable {
                    encoding: encoding.name(),
                });
            }
            bytes.extend_from_slice(&encoded);
        }
    }

    Ok(bytes)
}

fn stage(path: &Path, bytes: &[u8]) -> Result<tempfile::NamedTempFile, SaveError> {
    // A sibling temporary file keeps the eventual rename/replace atomic. The
    // randomized name avoids collisions between concurrent saves.
    let dir = path.parent().unwrap_or(Path::new("."));
    let mut temp = tempfile::Builder::new()
        .prefix(".markturbo-")
        .tempfile_in(dir)
        .map_err(SaveError::Io)?;
    std::io::Write::write_all(&mut temp, bytes).map_err(SaveError::Io)?;
    temp.as_file_mut().sync_all().map_err(SaveError::Io)?;
    Ok(temp)
}

#[cfg(test)]
thread_local! {
    #[cfg(windows)]
    static RECOVERY_SOURCE_MATCH_HOOK: std::cell::RefCell<Option<Box<dyn FnOnce()>>> = const {
        std::cell::RefCell::new(None)
    };
}

#[cfg(all(test, windows))]
fn install_recovery_source_match_hook(hook: impl FnOnce() + 'static) {
    RECOVERY_SOURCE_MATCH_HOOK.with(|slot| {
        assert!(
            slot.borrow().is_none(),
            "a recovery source match hook is already installed"
        );
        *slot.borrow_mut() = Some(Box::new(hook));
    });
}

#[cfg(all(test, windows))]
fn run_recovery_source_match_hook() {
    RECOVERY_SOURCE_MATCH_HOOK.with(|slot| {
        if let Some(hook) = slot.borrow_mut().take() {
            hook();
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_file(dir: &Path, name: &str, content: &[u8]) -> PathBuf {
        let path = dir.join(name);
        std::fs::write(&path, content).unwrap();
        path
    }

    #[test]
    fn skill_load_captures_origin_while_ordinary_documents_remain_unbound() {
        let directory = tempfile::tempdir().unwrap();
        let skill = write_file(directory.path(), "SKILL.md", b"entrypoint\n");
        let loaded_skill = load(&skill).unwrap();
        let origin = loaded_skill
            .skill_origin()
            .expect("loaded Skill has an origin");
        assert_eq!(
            origin.canonical_root(),
            std::fs::canonicalize(directory.path()).unwrap()
        );

        let ordinary = write_file(directory.path(), "notes.md", b"ordinary\n");
        assert!(load(&ordinary).unwrap().skill_origin().is_none());
    }

    #[cfg(unix)]
    #[test]
    fn skill_origin_failure_keeps_text_editable_for_a_symlinked_root_leaf() {
        use std::os::unix::fs::symlink;

        let directory = tempfile::tempdir().unwrap();
        let actual_root = directory.path().join("actual");
        std::fs::create_dir(&actual_root).unwrap();
        write_file(&actual_root, "SKILL.md", b"still editable\n");
        let linked_root = directory.path().join("linked-root");
        symlink(&actual_root, &linked_root).unwrap();

        let loaded = load(&linked_root.join("SKILL.md")).unwrap();
        assert_eq!(loaded.text, "still editable\n");
        assert!(loaded.skill_origin().is_none());
    }

    #[cfg(any(unix, windows))]
    #[test]
    fn skill_save_rejects_a_retargeted_ancestor_before_writing_the_alternate_root() {
        let directory = tempfile::tempdir().unwrap();
        let original_tree = directory.path().join("original");
        let alternate_tree = directory.path().join("alternate");
        let original_skill_root = original_tree.join("skill");
        let alternate_skill_root = alternate_tree.join("skill");
        std::fs::create_dir_all(&original_skill_root).unwrap();
        std::fs::create_dir_all(&alternate_skill_root).unwrap();
        let original_skill = write_file(&original_skill_root, "SKILL.md", b"original\n");
        let alternate_skill = alternate_skill_root.join("SKILL.md");
        std::fs::hard_link(&original_skill, &alternate_skill).unwrap();

        let alias = directory.path().join("alias");
        #[cfg(unix)]
        std::os::unix::fs::symlink(&original_tree, &alias).unwrap();
        #[cfg(windows)]
        let uses_symlink = if std::os::windows::fs::symlink_dir(&original_tree, &alias).is_ok() {
            true
        } else {
            std::fs::rename(&original_tree, &alias).unwrap();
            false
        };
        let loaded = load(&alias.join("skill/SKILL.md")).unwrap();
        assert!(loaded.skill_origin().is_some());

        #[cfg(unix)]
        {
            std::fs::remove_file(&alias).unwrap();
            std::os::unix::fs::symlink(&alternate_tree, &alias).unwrap();
        }
        #[cfg(windows)]
        if uses_symlink {
            std::fs::remove_dir(&alias).unwrap();
            std::os::windows::fs::symlink_dir(&alternate_tree, &alias).unwrap();
        } else {
            std::fs::rename(&alias, directory.path().join("retained-original")).unwrap();
            std::fs::rename(&alternate_tree, &alias).unwrap();
        }

        assert!(matches!(
            save_with(
                &loaded,
                "must not overwrite alternate\n",
                &SaveAuthorization::normal()
            ),
            Err(SaveError::SourceIdentityChanged)
        ));
        assert_eq!(
            std::fs::read(alias.join("skill/SKILL.md")).unwrap(),
            b"original\n"
        );
    }

    #[cfg(unix)]
    #[test]
    fn relocated_skill_root_never_reports_stale_preservation_paths() {
        let directory = tempfile::tempdir().unwrap();
        let original_tree = directory.path().join("original");
        let alternate_tree = directory.path().join("alternate");
        let original_skill_root = original_tree.join("skill");
        let alternate_skill_root = alternate_tree.join("skill");
        std::fs::create_dir_all(&original_skill_root).unwrap();
        std::fs::create_dir_all(&alternate_skill_root).unwrap();
        let original_skill = write_file(&original_skill_root, "SKILL.md", b"original\n");
        let alternate_skill = alternate_skill_root.join("SKILL.md");
        std::fs::hard_link(&original_skill, &alternate_skill).unwrap();
        let loaded = load(&original_skill).unwrap();
        let late_writer_directory = tempfile::tempdir().unwrap();
        let late_writer = write_file(late_writer_directory.path(), "late.md", b"late writer\n");
        let original_tree_for_hook = original_tree.clone();
        let alternate_tree_for_hook = alternate_tree.clone();
        let retained_tree = directory.path().join("retained-original");
        let retained_tree_for_hook = retained_tree.clone();
        let late_writer_for_hook = late_writer.clone();
        install_save_post_commit_hook(move || {
            std::fs::rename(&original_tree_for_hook, &retained_tree_for_hook).unwrap();
            std::fs::rename(&alternate_tree_for_hook, &original_tree_for_hook).unwrap();
            std::fs::rename(
                &late_writer_for_hook,
                retained_tree_for_hook.join("skill/SKILL.md"),
            )
            .unwrap();
        });

        let error =
            save_with(&loaded, "editor version\n", &SaveAuthorization::normal()).unwrap_err();
        let SaveError::ConcurrentCommit {
            preserved_paths, ..
        } = error
        else {
            panic!("post-commit root relocation must be reported as indeterminate")
        };
        assert!(preserved_paths.is_empty());
        assert_eq!(
            std::fs::read(original_tree.join("skill/SKILL.md")).unwrap(),
            b"original\n"
        );
        assert_eq!(
            std::fs::read(retained_tree.join("skill/SKILL.md")).unwrap(),
            b"late writer\n"
        );
        assert!(
            std::fs::read_dir(retained_tree.join("skill"))
                .unwrap()
                .any(|entry| {
                    let entry = entry.unwrap();
                    entry
                        .file_name()
                        .to_string_lossy()
                        .starts_with(".markturbo-editor-")
                        && std::fs::read(entry.path())
                            .is_ok_and(|bytes| bytes == b"editor version\n")
                })
        );
    }

    #[cfg(unix)]
    #[test]
    fn normal_skill_save_rechecks_the_loaded_entrypoint_identity_before_commit() {
        let directory = tempfile::tempdir().unwrap();
        let external = tempfile::tempdir().unwrap();
        let skill = write_file(directory.path(), "SKILL.md", b"same source\n");
        let identical = write_file(external.path(), "same.md", b"same source\n");
        let loaded = load(&skill).unwrap();
        let skill_for_hook = skill.clone();
        install_save_commit_hook(move || {
            std::fs::remove_file(&skill_for_hook).unwrap();
            std::fs::hard_link(&identical, &skill_for_hook).unwrap();
        });

        assert!(matches!(
            save_with(&loaded, "changed\n", &SaveAuthorization::normal()),
            Err(SaveError::SourceIdentityChanged)
        ));
        assert_eq!(std::fs::read(&skill).unwrap(), b"same source\n");
    }

    #[cfg(all(unix, any(target_os = "linux", target_os = "macos")))]
    fn preserved_path_with_bytes(paths: &[PathBuf], expected: &[u8]) -> PathBuf {
        paths
            .iter()
            .find(|path| std::fs::read(path).is_ok_and(|bytes| bytes == expected))
            .cloned()
            .expect("the exact file bytes must remain at a reported preserved path")
    }

    #[cfg(all(unix, any(target_os = "linux", target_os = "macos")))]
    #[test]
    fn skill_save_preserves_destination_replaced_after_the_final_precheck() {
        let directory = tempfile::tempdir().unwrap();
        let skill = write_file(directory.path(), "SKILL.md", b"loaded version\n");
        let loaded = load(&skill).unwrap();
        let external_directory = tempfile::tempdir().unwrap();
        let external = write_file(
            external_directory.path(),
            "writer.md",
            b"concurrent version\n",
        );
        let external_identity =
            skill_object_identity(&std::fs::File::open(&external).unwrap()).unwrap();
        let skill_for_hook = skill.clone();
        let external_for_hook = external.clone();
        let editor_identity = std::rc::Rc::new(std::cell::Cell::new(None));
        let editor_identity_for_hook = editor_identity.clone();
        install_skill_before_exchange_hook(move |_, staged_path| {
            let staged = std::fs::File::open(staged_path).unwrap();
            editor_identity_for_hook.set(Some(skill_object_identity(&staged).unwrap()));
            std::fs::rename(&external_for_hook, &skill_for_hook).unwrap();
        });

        let error =
            save_with(&loaded, "editor version\n", &SaveAuthorization::normal()).unwrap_err();
        let SaveError::ConcurrentCommit {
            preserved_paths, ..
        } = error
        else {
            panic!("race must be reported as an indeterminate commit")
        };
        let writer_path = preserved_path_with_bytes(&preserved_paths, b"concurrent version\n");
        let editor_path = preserved_path_with_bytes(&preserved_paths, b"editor version\n");
        assert_eq!(
            skill_object_identity(&std::fs::File::open(writer_path).unwrap()).unwrap(),
            external_identity
        );
        assert_eq!(
            skill_object_identity(&std::fs::File::open(editor_path).unwrap()).unwrap(),
            editor_identity.get().unwrap()
        );
    }

    #[cfg(all(unix, any(target_os = "linux", target_os = "macos")))]
    #[test]
    fn skill_save_preserves_in_place_writer_bytes_after_the_final_precheck() {
        let directory = tempfile::tempdir().unwrap();
        let skill = write_file(directory.path(), "SKILL.md", b"loaded version\n");
        let loaded = load(&skill).unwrap();
        let skill_for_hook = skill.clone();
        install_skill_before_exchange_hook(move |_, _| {
            std::fs::write(&skill_for_hook, b"concurrent in-place version\n").unwrap();
        });

        let error =
            save_with(&loaded, "editor version\n", &SaveAuthorization::normal()).unwrap_err();
        let SaveError::ConcurrentCommit {
            preserved_paths, ..
        } = error
        else {
            panic!("in-place writer must be detected before replacement")
        };
        assert_eq!(
            std::fs::read(&skill).unwrap(),
            b"concurrent in-place version\n"
        );
        assert!(
            preserved_paths.iter().any(|path| {
                std::fs::read(path).is_ok_and(|bytes| bytes == b"editor version\n")
            })
        );
    }

    #[cfg(all(unix, any(target_os = "linux", target_os = "macos")))]
    #[test]
    fn skill_save_preserves_editor_bytes_when_the_staged_name_is_substituted() {
        let directory = tempfile::tempdir().unwrap();
        let skill = write_file(directory.path(), "SKILL.md", b"loaded version\n");
        let loaded = load(&skill).unwrap();
        let staged_substitute = directory.path().join("attacker-file.md");
        let editor_identity = std::rc::Rc::new(std::cell::Cell::new(None));
        let editor_identity_for_hook = editor_identity.clone();
        let substitute_for_hook = staged_substitute.clone();
        install_skill_before_exchange_hook(move |_, staged_path| {
            let staged = std::fs::File::open(staged_path).unwrap();
            editor_identity_for_hook.set(Some(skill_object_identity(&staged).unwrap()));
            std::fs::rename(staged_path, &substitute_for_hook).unwrap();
            std::fs::write(staged_path, b"substituted staged name\n").unwrap();
        });

        let error =
            save_with(&loaded, "editor version\n", &SaveAuthorization::normal()).unwrap_err();
        let SaveError::ConcurrentCommit {
            preserved_paths, ..
        } = error
        else {
            panic!("substituted staged name must fail closed")
        };
        let editor_path = preserved_path_with_bytes(&preserved_paths, b"editor version\n");
        assert_eq!(
            skill_object_identity(&std::fs::File::open(editor_path).unwrap()).unwrap(),
            editor_identity.get().unwrap()
        );
        assert_eq!(std::fs::read(&skill).unwrap(), b"loaded version\n");
    }

    #[cfg(all(unix, any(target_os = "linux", target_os = "macos")))]
    #[test]
    fn unsupported_skill_exchange_does_not_fall_back_to_rename() {
        let directory = tempfile::tempdir().unwrap();
        let skill = write_file(directory.path(), "SKILL.md", b"loaded version\n");
        let loaded = load(&skill).unwrap();
        force_skill_exchange_unsupported();

        let error =
            save_with(&loaded, "editor version\n", &SaveAuthorization::normal()).unwrap_err();
        let SaveError::ConcurrentCommit {
            preserved_paths, ..
        } = error
        else {
            panic!("unsupported exchange must retain both versions and fail closed")
        };
        assert_eq!(std::fs::read(&skill).unwrap(), b"loaded version\n");
        assert!(
            preserved_paths.iter().any(|path| {
                std::fs::read(path).is_ok_and(|bytes| bytes == b"editor version\n")
            })
        );
    }

    #[cfg(all(unix, any(target_os = "linux", target_os = "macos")))]
    #[test]
    fn skill_recreation_does_not_clobber_a_creator_after_precheck() {
        let directory = tempfile::tempdir().unwrap();
        let skill = write_file(directory.path(), "SKILL.md", b"loaded version\n");
        let loaded = load(&skill).unwrap();
        std::fs::remove_file(&skill).unwrap();
        let recreate = SaveAuthorization::normal()
            .authorize_missing_recreation(&loaded)
            .unwrap();
        let skill_for_hook = skill.clone();
        install_skill_before_exchange_hook(move |_, _| {
            std::fs::write(&skill_for_hook, b"concurrent creator\n").unwrap();
        });

        let error = save_with(&loaded, "editor version\n", &recreate).unwrap_err();
        let SaveError::ConcurrentCommit {
            preserved_paths, ..
        } = error
        else {
            panic!("create-only link must not replace a concurrent entry")
        };
        assert_eq!(std::fs::read(&skill).unwrap(), b"concurrent creator\n");
        assert!(
            preserved_paths.iter().any(|path| {
                std::fs::read(path).is_ok_and(|bytes| bytes == b"editor version\n")
            })
        );
    }

    #[cfg(all(unix, any(target_os = "linux", target_os = "macos")))]
    #[test]
    fn skill_save_keeps_recovery_until_outer_post_commit_verification() {
        let directory = tempfile::tempdir().unwrap();
        let skill = write_file(directory.path(), "SKILL.md", b"loaded version\n");
        let external_directory = tempfile::tempdir().unwrap();
        let external = write_file(external_directory.path(), "writer.md", b"late writer\n");
        let skill_for_hook = skill.clone();
        let external_for_hook = external.clone();
        install_save_post_commit_hook(move || {
            std::fs::rename(&external_for_hook, &skill_for_hook).unwrap();
        });
        let loaded = load(&skill).unwrap();

        let error =
            save_with(&loaded, "editor version\n", &SaveAuthorization::normal()).unwrap_err();
        let SaveError::ConcurrentCommit {
            preserved_paths, ..
        } = error
        else {
            panic!("post-commit writer must leave a stale result and retained versions")
        };
        assert_eq!(std::fs::read(&skill).unwrap(), b"late writer\n");
        assert!(
            preserved_paths.iter().any(|path| {
                std::fs::read(path).is_ok_and(|bytes| bytes == b"editor version\n")
            })
        );
    }

    #[cfg(all(unix, any(target_os = "linux", target_os = "macos")))]
    #[test]
    fn skill_save_keeps_recovery_when_a_writer_arrives_before_cleanup() {
        let directory = tempfile::tempdir().unwrap();
        let skill = write_file(directory.path(), "SKILL.md", b"loaded version\n");
        let external_directory = tempfile::tempdir().unwrap();
        let external = write_file(
            external_directory.path(),
            "writer.md",
            b"late cleanup writer\n",
        );
        let external_for_hook = external.clone();
        let skill_parent_for_hook = directory.path().to_path_buf();
        install_skill_before_cleanup_hook(move |_| {
            std::fs::rename(&external_for_hook, skill_parent_for_hook.join("SKILL.md")).unwrap();
        });
        let loaded = load(&skill).unwrap();

        let error =
            save_with(&loaded, "editor version\n", &SaveAuthorization::normal()).unwrap_err();
        let SaveError::ConcurrentCommit {
            preserved_paths, ..
        } = error
        else {
            panic!("cleanup-window writer must preserve the staged editor version")
        };
        assert_eq!(std::fs::read(&skill).unwrap(), b"late cleanup writer\n");
        assert!(
            preserved_paths.iter().any(|path| {
                std::fs::read(path).is_ok_and(|bytes| bytes == b"editor version\n")
            })
        );
    }

    #[test]
    fn round_trips_lf_files_byte_identically() {
        let dir = tempfile::tempdir().unwrap();
        let original = "# Title\n\nBody with trailing spaces  \n\n\n";
        let path = write_file(dir.path(), "a.md", original.as_bytes());

        let file = load(&path).unwrap();
        assert_eq!(file.text, original);
        assert_eq!(file.newline, Newline::Lf);

        save(&file, &file.text, false).unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), original.as_bytes());
    }

    #[test]
    fn round_trips_crlf_files_byte_identically() {
        let dir = tempfile::tempdir().unwrap();
        let original = b"# Title\r\n\r\nBody\r\n";
        let path = write_file(dir.path(), "a.md", original);

        let file = load(&path).unwrap();
        assert_eq!(file.newline, Newline::Crlf);
        assert_eq!(file.text, "# Title\n\nBody\n", "normalized in memory");

        save(&file, &file.text, false).unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), original, "CRLF restored");
    }

    #[test]
    fn preserves_a_bom() {
        let dir = tempfile::tempdir().unwrap();
        let original = b"\xEF\xBB\xBF# Title\n";
        let path = write_file(dir.path(), "a.md", original);

        let file = load(&path).unwrap();
        assert!(file.had_bom);
        assert_eq!(file.text, "# Title\n", "BOM stripped in memory");

        save(&file, &file.text, false).unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), original);
    }

    #[cfg(windows)]
    #[test]
    fn recovery_match_rejects_a_regular_source_replaced_by_a_locked_target_symlink() {
        use std::os::windows::fs::OpenOptionsExt as _;

        let dir = tempfile::tempdir().unwrap();
        let source = write_file(dir.path(), "source.md", b"original\n");
        let loaded = load(&source).unwrap();
        let target = write_file(dir.path(), "locked.md", b"locked\n");
        std::fs::remove_file(&source).unwrap();
        if let Err(error) = std::os::windows::fs::symlink_file(&target, &source) {
            eprintln!("skipping file-symlink test: {error}");
            return;
        }
        let _target_lock = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .share_mode(0)
            .open(&target)
            .unwrap();

        assert!(
            !recovery_source_matches(&source, &loaded.stamp, &loaded.source_identity).unwrap(),
            "the entry-kind mismatch must be returned before opening the locked target"
        );
    }

    #[cfg(windows)]
    #[test]
    fn recovery_match_guard_prevents_retarget_until_the_check_returns() {
        let dir = tempfile::tempdir().unwrap();
        let target = write_file(dir.path(), "target-a.md", b"original\n");
        let alternate = write_file(dir.path(), "target-b.md", b"alternate\n");
        let link = dir.path().join("source.md");
        if let Err(error) = std::os::windows::fs::symlink_file(&target, &link) {
            eprintln!("skipping file-symlink test: {error}");
            return;
        }
        let loaded = load(&link).unwrap();
        let retarget_failure = std::rc::Rc::new(std::cell::RefCell::new(None));
        let retarget_failure_for_hook = retarget_failure.clone();
        let guarded_link = link.clone();
        install_recovery_source_match_hook(move || {
            let error = std::fs::remove_file(&guarded_link)
                .expect_err("the live recovery guard must reject a retarget");
            *retarget_failure_for_hook.borrow_mut() = Some(error.kind());
        });

        assert!(recovery_source_matches(&link, &loaded.stamp, &loaded.source_identity).unwrap());
        assert!(retarget_failure.borrow().is_some());

        std::fs::remove_file(&link).unwrap();
        std::os::windows::fs::symlink_file(&alternate, &link).unwrap();
        assert_eq!(std::fs::read(&link).unwrap(), b"alternate\n");
    }

    #[cfg(windows)]
    #[test]
    fn recovery_match_accepts_an_unchanged_symlinked_legacy_source() {
        let dir = tempfile::tempdir().unwrap();
        let target = write_file(dir.path(), "legacy.txt", b"\xD6\xD0\xCE\xC4\r\n");
        let link = dir.path().join("source.txt");
        if let Err(error) = std::os::windows::fs::symlink_file(&target, &link) {
            eprintln!("skipping file-symlink test: {error}");
            return;
        }
        let loaded = load(&link).unwrap();

        assert_eq!(loaded.encoding.name(), "GBK");
        assert!(matches!(
            &loaded.source_identity,
            SourceIdentity::SymbolicLink { .. }
        ));
        assert!(recovery_source_matches(&link, &loaded.stamp, &loaded.source_identity).unwrap());
    }

    #[test]
    fn external_change_blocks_a_save() {
        let dir = tempfile::tempdir().unwrap();
        let path = write_file(dir.path(), "a.md", b"original\n");
        let file = load(&path).unwrap();

        // An agent rewrites the file behind our back.
        std::thread::sleep(std::time::Duration::from_millis(20));
        std::fs::write(&path, b"rewritten by an agent, much longer than before\n").unwrap();

        let err = save(&file, "my edit\n", false).unwrap_err();
        assert!(matches!(err, SaveError::Conflict));
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            "rewritten by an agent, much longer than before\n",
            "the external change must survive"
        );
    }

    #[test]
    fn same_length_rewrite_with_restored_mtime_blocks_a_save() {
        let dir = tempfile::tempdir().unwrap();
        let path = write_file(dir.path(), "a.md", b"original\n");
        let file = load(&path).unwrap();
        let original_modified = std::fs::metadata(&path).unwrap().modified().unwrap();

        std::fs::write(&path, b"changed!\n").unwrap();
        let handle = std::fs::OpenOptions::new().write(true).open(&path).unwrap();
        handle
            .set_times(std::fs::FileTimes::new().set_modified(original_modified))
            .unwrap();
        drop(handle);
        assert_eq!(
            std::fs::metadata(&path).unwrap().modified().unwrap(),
            original_modified,
            "the test must neutralize the stamp optimization"
        );

        assert!(matches!(
            save(&file, "my edit\n", false),
            Err(SaveError::Conflict)
        ));
        assert_eq!(std::fs::read(&path).unwrap(), b"changed!\n");
    }

    #[cfg(windows)]
    #[test]
    fn same_bytes_and_mtime_path_replacement_blocks_a_save_by_object_identity() {
        let dir = tempfile::tempdir().unwrap();
        let path = write_file(dir.path(), "a.md", b"original\n");
        let file = load(&path).unwrap();
        let original_id = file
            .stamp
            .object_id
            .expect("a normal Windows file must provide an object identity");
        let original_modified = file.stamp.modified.expect("test file has an mtime");

        // Keep the first object alive under another name, then recreate the
        // original path with byte-for-byte identical data and its old mtime.
        let preserved_original = dir.path().join("original-aside.md");
        std::fs::rename(&path, &preserved_original).unwrap();
        std::fs::write(&path, b"original\n").unwrap();
        let replacement = std::fs::OpenOptions::new().write(true).open(&path).unwrap();
        replacement
            .set_times(std::fs::FileTimes::new().set_modified(original_modified))
            .unwrap();
        drop(replacement);

        let replacement_stamp = FileStamp::of(&path).unwrap();
        assert_eq!(replacement_stamp.digest, file.stamp.digest);
        assert_eq!(replacement_stamp.len, file.stamp.len);
        assert_eq!(replacement_stamp.modified, file.stamp.modified);
        assert_ne!(
            replacement_stamp
                .object_id
                .expect("a normal Windows file must provide an object identity"),
            original_id
        );

        assert!(matches!(
            save(&file, "my edit\n", false),
            Err(SaveError::Conflict)
        ));
        assert_eq!(std::fs::read(&path).unwrap(), b"original\n");
    }

    #[test]
    fn forced_save_overwrites_after_the_user_decides() {
        let dir = tempfile::tempdir().unwrap();
        let path = write_file(dir.path(), "a.md", b"original\n");
        let file = load(&path).unwrap();
        std::fs::write(&path, b"external change\n").unwrap();

        save(&file, "my edit\n", true).unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "my edit\n");
    }

    #[cfg(windows)]
    #[test]
    fn post_approval_regular_rewrite_is_rejected_before_commit() {
        let dir = tempfile::tempdir().unwrap();
        let path = write_file(dir.path(), "a.md", b"original\n");
        let file = load(&path).unwrap();
        std::fs::write(&path, b"approved external version\n").unwrap();
        let external = b"newer external writer\n";
        let editor = "my exact editor text\n";
        let rewritten_path = path.clone();
        install_save_commit_hook(move || std::fs::write(rewritten_path, external).unwrap());

        let error = save(&file, editor, true).unwrap_err();
        assert!(matches!(error, SaveError::Conflict));
        assert_eq!(std::fs::read(&path).unwrap(), external);
        assert!(
            std::fs::read_dir(dir.path())
                .unwrap()
                .flatten()
                .all(|entry| entry.file_name() == "a.md"),
            "a pre-commit rejection must not leave a staged editor version behind"
        );
    }

    #[cfg(windows)]
    #[test]
    fn post_commit_regular_path_replacement_is_not_reported_as_a_save() {
        let dir = tempfile::tempdir().unwrap();
        let path = write_file(dir.path(), "a.md", b"original\n");
        let file = load(&path).unwrap();
        let replaced_path = path.clone();
        install_save_post_commit_hook(move || {
            let aside = replaced_path.with_extension("committed-by-markturbo");
            std::fs::rename(&replaced_path, aside).unwrap();
            std::fs::write(&replaced_path, b"external after commit\n").unwrap();
        });

        let error = save(&file, "editor text\n", false).unwrap_err();
        let SaveError::ConcurrentCommit {
            preserved_paths,
            outcome,
        } = error
        else {
            panic!("a post-commit replacement must not report success");
        };
        assert_eq!(outcome, ConcurrentCommitOutcome::Indeterminate);
        assert_eq!(std::fs::read(&path).unwrap(), b"external after commit\n");
        assert!(preserved_paths.contains(&path));
    }

    #[cfg(any(target_os = "windows", unix))]
    #[test]
    fn post_commit_symlink_retarget_is_not_reported_as_a_save() {
        let dir = tempfile::tempdir().unwrap();
        let target = write_file(dir.path(), "target-a.md", b"old\n");
        let alternate = write_file(dir.path(), "target-b.md", b"other\n");
        let link = dir.path().join("linked.md");

        #[cfg(target_os = "windows")]
        if let Err(err) = std::os::windows::fs::symlink_file(&target, &link) {
            eprintln!("skipping file-symlink test: {err}");
            return;
        }
        #[cfg(unix)]
        std::os::unix::fs::symlink(&target, &link).unwrap();

        let file = load(&link).unwrap();
        let retargeted_link = link.clone();
        #[cfg(windows)]
        let retarget_failure = std::rc::Rc::new(std::cell::RefCell::new(None));
        #[cfg(windows)]
        let retarget_failure_for_hook = retarget_failure.clone();
        install_save_post_commit_hook(move || {
            #[cfg(windows)]
            {
                let error = std::fs::remove_file(&retargeted_link)
                    .expect_err("the guarded link must reject a post-commit retarget");
                *retarget_failure_for_hook.borrow_mut() = Some(error.kind());
            }
            #[cfg(unix)]
            {
                std::fs::remove_file(&retargeted_link).unwrap();
                std::os::unix::fs::symlink(&alternate, &retargeted_link).unwrap();
            }
        });

        #[cfg(windows)]
        {
            save(&file, "editor text\n", false).unwrap();
            assert!(
                retarget_failure.borrow().is_some(),
                "the guarded link must reject the retarget attempt"
            );
            assert!(
                std::fs::symlink_metadata(&link)
                    .unwrap()
                    .file_type()
                    .is_symlink()
            );
            assert_eq!(std::fs::read(&target).unwrap(), b"editor text\n");
            assert_eq!(std::fs::read(&link).unwrap(), b"editor text\n");
            assert_eq!(std::fs::read(&alternate).unwrap(), b"other\n");
        }

        #[cfg(unix)]
        let error = save(&file, "editor text\n", false).unwrap_err();
        #[cfg(unix)]
        let SaveError::ConcurrentCommit {
            preserved_paths,
            outcome,
        } = error
        else {
            panic!("a post-commit retarget must not report success");
        };
        #[cfg(unix)]
        assert_eq!(outcome, ConcurrentCommitOutcome::Indeterminate);
        #[cfg(unix)]
        assert_eq!(std::fs::read(&target).unwrap(), b"editor text\n");
        #[cfg(unix)]
        assert_eq!(std::fs::read(&link).unwrap(), b"other\n");
        #[cfg(unix)]
        assert!(preserved_paths.contains(&link));
        #[cfg(unix)]
        assert!(
            preserved_paths
                .iter()
                .any(|path| std::fs::read(path).is_ok_and(|bytes| bytes == b"editor text\n")),
            "the committed target must be one of the reported paths: {preserved_paths:?}"
        );
    }

    #[cfg(any(target_os = "windows", unix))]
    #[test]
    fn pre_commit_symlink_retarget_is_rejected_before_mutating_the_original_target() {
        let dir = tempfile::tempdir().unwrap();
        let target = write_file(dir.path(), "target-a.md", b"old\n");
        let alternate = write_file(dir.path(), "target-b.md", b"other\n");
        let link = dir.path().join("linked.md");

        #[cfg(target_os = "windows")]
        if let Err(err) = std::os::windows::fs::symlink_file(&target, &link) {
            eprintln!("skipping file-symlink test: {err}");
            return;
        }
        #[cfg(unix)]
        std::os::unix::fs::symlink(&target, &link).unwrap();

        let file = load(&link).unwrap();
        let retargeted_link = link.clone();
        install_save_commit_hook(move || {
            std::fs::remove_file(&retargeted_link).unwrap();
            #[cfg(target_os = "windows")]
            std::os::windows::fs::symlink_file(&alternate, &retargeted_link).unwrap();
            #[cfg(unix)]
            std::os::unix::fs::symlink(&alternate, &retargeted_link).unwrap();
        });

        let error = save(&file, "editor text\n", false).unwrap_err();
        assert!(matches!(error, SaveError::SourceIdentityChanged));
        assert_eq!(std::fs::read(&target).unwrap(), b"old\n");
        assert_eq!(std::fs::read(&link).unwrap(), b"other\n");
    }

    #[cfg(windows)]
    #[test]
    fn reservation_failure_reports_the_still_existing_placeholder() {
        let dir = tempfile::tempdir().unwrap();
        let path = write_file(dir.path(), "a.md", b"original\n");
        let file = load(&path).unwrap();
        install_save_reservation_hook(|_| {
            Err(std::io::Error::other(
                "injected placeholder deletion failure",
            ))
        });

        let error = save(&file, "editor text\n", false).unwrap_err();
        let SaveError::ConcurrentCommit {
            preserved_paths,
            outcome,
        } = error
        else {
            panic!("an unremoved reservation must be reported");
        };
        assert_eq!(outcome, ConcurrentCommitOutcome::Indeterminate);
        assert!(
            preserved_paths.iter().any(|path| {
                path.file_name()
                    .is_some_and(|name| name.to_string_lossy().starts_with(".markturbo-backup-"))
                    && path.exists()
            }),
            "the residual placeholder must be retained and reported: {preserved_paths:?}"
        );
    }

    #[cfg(windows)]
    #[test]
    fn normal_and_explicit_overwrite_verify_the_returned_stamp_and_cleanup() {
        let dir = tempfile::tempdir().unwrap();
        let path = write_file(dir.path(), "a.md", b"original\n");
        let file = load(&path).unwrap();

        let normal = save(&file, "normal\n", false).unwrap();
        assert_eq!(normal, FileStamp::of(&path).unwrap());
        let normal_digest: [u8; 32] = Sha256::digest(b"normal\n").into();
        assert_eq!(normal.digest, normal_digest);
        assert!(normal.object_id.is_some());
        assert!(
            std::fs::read_dir(dir.path())
                .unwrap()
                .flatten()
                .all(|entry| entry.file_name() == "a.md"),
            "a successful normal save must clean its temp and backup"
        );

        std::fs::write(&path, b"external\n").unwrap();
        let overwrite = save(&file, "overwritten\n", true).unwrap();
        assert_eq!(overwrite, FileStamp::of(&path).unwrap());
        let overwrite_digest: [u8; 32] = Sha256::digest(b"overwritten\n").into();
        assert_eq!(overwrite.digest, overwrite_digest);
        assert!(overwrite.object_id.is_some());
        assert!(
            std::fs::read_dir(dir.path())
                .unwrap()
                .flatten()
                .all(|entry| entry.file_name() == "a.md"),
            "a successful explicit overwrite must clean its temp and backup"
        );
    }

    #[test]
    fn saving_an_unchanged_file_succeeds_and_updates_the_stamp() {
        let dir = tempfile::tempdir().unwrap();
        let path = write_file(dir.path(), "a.md", b"one\n");
        let mut file = load(&path).unwrap();

        let new_stamp = save(&file, "one\ntwo\n", false).unwrap();
        file.stamp = new_stamp;
        // A second save with the refreshed stamp must not report a conflict.
        save(&file, "one\ntwo\nthree\n", false).unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "one\ntwo\nthree\n");
    }

    #[test]
    fn save_as_writes_a_new_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("new.md");
        let saved = save_as(&path, "content\n", Newline::Lf, false).unwrap();
        assert_eq!(saved.path, path);
        assert_eq!(saved.text, "content\n");
        assert_eq!(saved.stamp, FileStamp::of(&path).unwrap());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "content\n");
    }

    #[test]
    fn save_as_captures_a_new_skill_origin_from_the_committed_source() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("SKILL.md");

        let saved = save_as(&path, "saved skill\n", Newline::Lf, false).unwrap();

        let origin = saved
            .skill_origin()
            .expect("Save As binds its new Skill source");
        assert_eq!(origin.entrypoint_stamp, saved.stamp);
        assert_eq!(
            origin.canonical_root(),
            std::fs::canonicalize(dir.path()).unwrap()
        );
    }

    #[test]
    fn successful_skill_save_refreshes_only_an_existing_origin() {
        let dir = tempfile::tempdir().unwrap();
        let path = write_file(dir.path(), "SKILL.md", b"before\n");
        let loaded = load(&path).unwrap();
        let original_origin = loaded.skill_origin().unwrap().clone();

        let saved = save_with(&loaded, "after\n", &SaveAuthorization::normal()).unwrap();

        let refreshed = saved
            .skill_origin
            .as_ref()
            .expect("a valid in-place save refreshes its existing origin");
        assert_eq!(refreshed.root_identity, original_origin.root_identity);
        assert_eq!(refreshed.entrypoint_stamp, saved.stamp);

        let mut unbound = load(&path).unwrap();
        unbound.skill_origin = None;
        let saved_unbound = save_with(&unbound, "again\n", &SaveAuthorization::normal()).unwrap();
        assert!(saved_unbound.skill_origin.is_none());
    }

    #[test]
    fn save_as_refuses_an_existing_destination_without_modifying_it() {
        let dir = tempfile::tempdir().unwrap();
        let path = write_file(dir.path(), "existing.md", b"external bytes\xFF\n");

        let error = save_as(&path, "editor text\n", Newline::Lf, false).unwrap_err();

        assert!(matches!(error, SaveError::DestinationExists));
        assert_eq!(std::fs::read(&path).unwrap(), b"external bytes\xFF\n");
    }

    #[cfg(any(target_os = "windows", unix))]
    #[test]
    fn save_as_refuses_an_existing_symbolic_link_without_modifying_its_target() {
        let dir = tempfile::tempdir().unwrap();
        let target = write_file(dir.path(), "target.md", b"external version\n");
        let link = dir.path().join("existing.md");

        #[cfg(target_os = "windows")]
        if let Err(error) = std::os::windows::fs::symlink_file(&target, &link) {
            eprintln!("skipping file-symlink test: {error}");
            return;
        }
        #[cfg(unix)]
        std::os::unix::fs::symlink(&target, &link).unwrap();

        let error = save_as(&link, "editor text\n", Newline::Lf, false).unwrap_err();

        assert!(matches!(error, SaveError::DestinationExists));
        assert!(
            std::fs::symlink_metadata(&link)
                .unwrap()
                .file_type()
                .is_symlink()
        );
        assert_eq!(std::fs::read(&target).unwrap(), b"external version\n");
    }

    #[test]
    fn save_as_confirmation_can_be_cancelled_without_mutating_the_destination() {
        let dir = tempfile::tempdir().unwrap();
        let path = write_file(dir.path(), "existing.md", b"external version\n");

        let error = save_as(&path, "editor text\n", Newline::Lf, false).unwrap_err();

        assert!(matches!(error, SaveError::DestinationExists));
        // A cancelled confirmation deliberately does not call `overwrite_as`.
        assert_eq!(std::fs::read(&path).unwrap(), b"external version\n");
    }

    #[test]
    fn overwrite_as_replaces_an_existing_destination() {
        let dir = tempfile::tempdir().unwrap();
        let path = write_file(dir.path(), "existing.md", b"external version\n");

        let saved = overwrite_as(&path, "editor text\n", Newline::Lf, false).unwrap();

        assert_eq!(saved.path, path);
        assert_eq!(std::fs::read(&path).unwrap(), b"editor text\n");
    }

    #[test]
    fn overwrite_as_authorization_refuses_a_destination_changed_while_confirming() {
        let dir = tempfile::tempdir().unwrap();
        let path = write_file(dir.path(), "existing.md", b"version shown in the prompt\n");
        let authorization = SaveAsOverwriteAuthorization::capture(&path).unwrap();
        std::fs::write(&path, b"later external version\n").unwrap();

        let error = overwrite_as_authorized(&authorization, "editor text\n", Newline::Lf, false)
            .unwrap_err();

        assert!(matches!(error, SaveError::Conflict));
        assert_eq!(std::fs::read(&path).unwrap(), b"later external version\n");
    }

    #[test]
    fn overwrite_as_authorization_refuses_a_destination_removed_while_confirming() {
        let dir = tempfile::tempdir().unwrap();
        let path = write_file(dir.path(), "existing.md", b"version shown in the prompt\n");
        let authorization = SaveAsOverwriteAuthorization::capture(&path).unwrap();
        std::fs::remove_file(&path).unwrap();

        let error = overwrite_as_authorized(&authorization, "editor text\n", Newline::Lf, false)
            .unwrap_err();

        assert!(matches!(error, SaveError::Conflict));
        assert!(!path.exists());
    }

    #[cfg(any(target_os = "windows", unix))]
    #[test]
    fn overwrite_as_authorization_refuses_a_symlink_retargeted_while_confirming() {
        let dir = tempfile::tempdir().unwrap();
        let target = write_file(dir.path(), "target-a.md", b"version shown in the prompt\n");
        let alternate = write_file(dir.path(), "target-b.md", b"other version\n");
        let link = dir.path().join("existing.md");

        #[cfg(target_os = "windows")]
        if let Err(error) = std::os::windows::fs::symlink_file(&target, &link) {
            eprintln!("skipping file-symlink test: {error}");
            return;
        }
        #[cfg(unix)]
        std::os::unix::fs::symlink(&target, &link).unwrap();

        let authorization = SaveAsOverwriteAuthorization::capture(&link).unwrap();
        std::fs::remove_file(&link).unwrap();
        #[cfg(target_os = "windows")]
        std::os::windows::fs::symlink_file(&alternate, &link).unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(&alternate, &link).unwrap();

        let error = overwrite_as_authorized(&authorization, "editor text\n", Newline::Lf, false)
            .unwrap_err();

        assert!(matches!(error, SaveError::Conflict));
        assert_eq!(
            std::fs::read(&target).unwrap(),
            b"version shown in the prompt\n"
        );
        assert_eq!(std::fs::read(&alternate).unwrap(), b"other version\n");
    }

    #[test]
    fn overwrite_as_refuses_a_destination_changed_after_confirmation() {
        let dir = tempfile::tempdir().unwrap();
        let path = write_file(dir.path(), "existing.md", b"confirmed version\n");
        let external_path = path.clone();
        install_save_commit_hook(move || {
            std::fs::write(external_path, b"later external version\n").unwrap();
        });

        let error = overwrite_as(&path, "editor text\n", Newline::Lf, false).unwrap_err();

        assert!(matches!(error, SaveError::Conflict));
        assert_eq!(std::fs::read(&path).unwrap(), b"later external version\n");
    }

    #[test]
    fn save_as_refuses_a_destination_created_after_preflight() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("new.md");
        let external_path = path.clone();
        install_save_commit_hook(move || {
            std::fs::write(external_path, b"external version\n").unwrap();
        });

        let error = save_as(&path, "editor text\n", Newline::Lf, false).unwrap_err();

        assert!(matches!(error, SaveError::Conflict));
        assert_eq!(std::fs::read(&path).unwrap(), b"external version\n");
    }

    #[cfg(any(target_os = "windows", unix))]
    #[test]
    fn save_as_refuses_a_post_commit_symlink_retarget() {
        let dir = tempfile::tempdir().unwrap();
        let target = write_file(dir.path(), "target-a.md", b"old\n");
        let alternate = write_file(dir.path(), "target-b.md", b"other\n");
        let link = dir.path().join("save-as.md");

        #[cfg(target_os = "windows")]
        if let Err(err) = std::os::windows::fs::symlink_file(&target, &link) {
            eprintln!("skipping file-symlink test: {err}");
            return;
        }
        #[cfg(unix)]
        std::os::unix::fs::symlink(&target, &link).unwrap();

        let retargeted_link = link.clone();
        #[cfg(windows)]
        let retarget_failure = std::rc::Rc::new(std::cell::RefCell::new(None));
        #[cfg(windows)]
        let retarget_failure_for_hook = retarget_failure.clone();
        install_save_post_commit_hook(move || {
            #[cfg(windows)]
            {
                let error = std::fs::remove_file(&retargeted_link)
                    .expect_err("the guarded link must reject a post-commit retarget");
                *retarget_failure_for_hook.borrow_mut() = Some(error.kind());
            }
            #[cfg(unix)]
            {
                std::fs::remove_file(&retargeted_link).unwrap();
                std::os::unix::fs::symlink(&alternate, &retargeted_link).unwrap();
            }
        });

        #[cfg(windows)]
        {
            overwrite_as(&link, "editor text\n", Newline::Lf, false).unwrap();
            assert!(
                retarget_failure.borrow().is_some(),
                "the guarded link must reject the retarget attempt"
            );
            assert!(
                std::fs::symlink_metadata(&link)
                    .unwrap()
                    .file_type()
                    .is_symlink()
            );
            assert_eq!(std::fs::read(&target).unwrap(), b"editor text\n");
            assert_eq!(std::fs::read(&link).unwrap(), b"editor text\n");
            assert_eq!(std::fs::read(&alternate).unwrap(), b"other\n");
        }

        #[cfg(unix)]
        let error = overwrite_as(&link, "editor text\n", Newline::Lf, false).unwrap_err();
        #[cfg(unix)]
        assert!(matches!(error, SaveError::ConcurrentCommit { .. }));
        #[cfg(unix)]
        assert_eq!(std::fs::read(&target).unwrap(), b"editor text\n");
        #[cfg(unix)]
        assert_eq!(std::fs::read(&link).unwrap(), b"other\n");
    }

    #[test]
    fn no_temp_file_is_left_behind() {
        let dir = tempfile::tempdir().unwrap();
        let path = write_file(dir.path(), "a.md", b"x\n");
        let file = load(&path).unwrap();
        save(&file, "y\n", false).unwrap();

        let leftovers: Vec<_> = std::fs::read_dir(dir.path())
            .unwrap()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|n| n != "a.md")
            .collect();
        assert!(leftovers.is_empty(), "found {leftovers:?}");
    }

    #[test]
    fn detects_the_majority_newline_in_mixed_files() {
        assert_eq!(Newline::detect("a\r\nb\r\nc\n"), Newline::Crlf);
        assert_eq!(Newline::detect("a\nb\nc\r\n"), Newline::Lf);
        assert_eq!(Newline::detect("no newlines"), Newline::Lf);
    }

    #[test]
    fn invalid_utf8_still_opens() {
        let dir = tempfile::tempdir().unwrap();
        let path = write_file(dir.path(), "a.md", b"valid \xFF\xFE invalid\n");
        let file = load(&path).unwrap();
        assert!(file.text.contains("valid"), "must not fail to open");
    }

    #[test]
    fn bom_declared_invalid_utf8_cannot_be_saved_lossily() {
        let dir = tempfile::tempdir().unwrap();
        let path = write_file(dir.path(), "invalid.md", b"\xEF\xBB\xBFvalid \xFF byte\n");
        let file = load(&path).unwrap();

        assert!(
            file.text.contains('\u{FFFD}'),
            "the editor exposes the decode problem"
        );
        assert!(
            save(&file, &file.text, false).is_err(),
            "saving must require an explicit conversion decision"
        );
        assert_eq!(
            std::fs::read(&path).unwrap(),
            b"\xEF\xBB\xBFvalid \xFF byte\n",
            "the original bytes must remain untouched"
        );
    }

    #[test]
    fn explicit_utf8_conversion_preserves_the_exact_editor_text() {
        let dir = tempfile::tempdir().unwrap();
        let path = write_file(dir.path(), "invalid.md", b"\xEF\xBB\xBFvalid \xFF byte\n");
        let file = load(&path).unwrap();
        let editor_text = file.text.clone();

        let saved = save_with(
            &file,
            &editor_text,
            &SaveAuthorization::normal().enable_utf8_conversion(),
        )
        .unwrap();

        assert_eq!(saved.encoding, UTF_8);
        assert!(!saved.had_bom);
        assert_eq!(std::fs::read_to_string(path).unwrap(), editor_text);
    }

    #[test]
    fn overwrite_then_utf8_conversion_preserves_the_exact_editor_text_after_a_conflict() {
        let dir = tempfile::tempdir().unwrap();
        let path = write_file(dir.path(), "invalid.md", b"\xEF\xBB\xBFvalid \xFF byte\n");
        let file = load(&path).unwrap();
        let editor_text = "exact editor text \u{4e2d}\u{6587} \u{1f680}\n";
        std::fs::write(&path, b"external version\n").unwrap();

        assert!(matches!(
            save(&file, editor_text, false),
            Err(SaveError::Conflict)
        ));
        let overwrite = SaveAuthorization::normal()
            .authorize_current_overwrite(&file)
            .unwrap();
        assert!(matches!(
            save_with(&file, editor_text, &overwrite),
            Err(SaveError::DecodeLoss)
        ));

        save_with(&file, editor_text, &overwrite.enable_utf8_conversion()).unwrap();

        assert_eq!(std::fs::read_to_string(path).unwrap(), editor_text);
    }

    #[test]
    fn recreate_then_utf8_conversion_preserves_the_exact_editor_text_after_deletion() {
        let dir = tempfile::tempdir().unwrap();
        let path = write_file(dir.path(), "invalid.md", b"\xEF\xBB\xBFvalid \xFF byte\n");
        let file = load(&path).unwrap();
        let editor_text = "exact recreated text \u{4e2d}\u{6587} \u{1f680}\n";
        std::fs::remove_file(&path).unwrap();

        assert!(matches!(
            save(&file, editor_text, false),
            Err(SaveError::Missing)
        ));
        let recreate = SaveAuthorization::normal()
            .authorize_missing_recreation(&file)
            .unwrap();
        assert!(matches!(
            save_with(&file, editor_text, &recreate),
            Err(SaveError::DecodeLoss)
        ));

        save_with(&file, editor_text, &recreate.enable_utf8_conversion()).unwrap();

        assert_eq!(std::fs::read_to_string(path).unwrap(), editor_text);
    }

    #[test]
    fn overwrite_then_utf8_conversion_preserves_unrepresentable_gbk_text_after_conflict() {
        let dir = tempfile::tempdir().unwrap();
        let path = write_file(dir.path(), "legacy.txt", b"\xD6\xD0\xCE\xC4\r\n");
        let file = load(&path).unwrap();
        let editor_text = "\u{4e2d}\u{6587} with emoji \u{1f680}\n";
        std::fs::write(&path, b"external legacy version\r\n").unwrap();

        assert!(matches!(
            save(&file, editor_text, false),
            Err(SaveError::Conflict)
        ));
        let overwrite = SaveAuthorization::normal()
            .authorize_current_overwrite(&file)
            .unwrap();
        assert!(matches!(
            save_with(&file, editor_text, &overwrite),
            Err(SaveError::Unrepresentable { encoding: "GBK" })
        ));

        save_with(&file, editor_text, &overwrite.enable_utf8_conversion()).unwrap();

        assert_eq!(
            std::fs::read_to_string(path).unwrap(),
            "\u{4e2d}\u{6587} with emoji \u{1f680}\r\n"
        );
    }

    #[test]
    fn conversion_cannot_reuse_an_overwrite_authorization_after_another_external_write() {
        let dir = tempfile::tempdir().unwrap();
        let path = write_file(dir.path(), "invalid.md", b"\xEF\xBB\xBFvalid \xFF byte\n");
        let file = load(&path).unwrap();
        let editor_text = "editor text \u{4e2d}\u{6587} \u{1f680}\n";
        std::fs::write(&path, b"first external version\n").unwrap();
        let overwrite = SaveAuthorization::normal()
            .authorize_current_overwrite(&file)
            .unwrap();
        assert!(matches!(
            save_with(&file, editor_text, &overwrite),
            Err(SaveError::DecodeLoss)
        ));

        let external = b"newer external version\n";
        std::fs::write(&path, external).unwrap();
        assert!(matches!(
            save_with(&file, editor_text, &overwrite.enable_utf8_conversion()),
            Err(SaveError::Conflict)
        ));
        assert_eq!(std::fs::read(&path).unwrap(), external);
    }

    #[test]
    fn conversion_cannot_recreate_a_path_that_reappeared_after_recreate_authorization() {
        let dir = tempfile::tempdir().unwrap();
        let path = write_file(dir.path(), "invalid.md", b"\xEF\xBB\xBFvalid \xFF byte\n");
        let file = load(&path).unwrap();
        let editor_text = "editor text \u{4e2d}\u{6587} \u{1f680}\n";
        std::fs::remove_file(&path).unwrap();
        let recreate = SaveAuthorization::normal()
            .authorize_missing_recreation(&file)
            .unwrap();
        assert!(matches!(
            save_with(&file, editor_text, &recreate),
            Err(SaveError::DecodeLoss)
        ));

        let external = b"reappeared external version\n";
        std::fs::write(&path, external).unwrap();
        assert!(matches!(
            save_with(&file, editor_text, &recreate.enable_utf8_conversion()),
            Err(SaveError::Conflict)
        ));
        assert_eq!(std::fs::read(&path).unwrap(), external);
    }

    #[test]
    fn a_gbk_document_survives_an_open_and_save() {
        // The data-loss path this closes: decoding through `from_utf8_lossy`
        // turned every GBK byte into U+FFFD, and `save` wrote that back — so
        // opening a legacy Chinese document and pressing Ctrl+S destroyed it.
        let dir = tempfile::tempdir().unwrap();
        // "中文" in GBK. Not valid UTF-8, which is what routes it to the detector.
        let original = b"\xD6\xD0\xCE\xC4\r\n";
        let path = write_file(dir.path(), "legacy.txt", original);

        let file = load(&path).unwrap();
        assert_eq!(file.encoding.name(), "GBK", "detected as GBK, not UTF-8");
        assert_eq!(file.text, "中文\n", "decoded, not replaced");
        assert!(
            !file.text.contains('\u{FFFD}'),
            "no replacement characters: {:?}",
            file.text
        );

        save(&file, &file.text, false).unwrap();
        assert_eq!(
            std::fs::read(&path).unwrap(),
            original,
            "an untouched save is byte-identical, still GBK"
        );
    }

    #[test]
    fn unrepresentable_text_cannot_be_rewritten_as_character_references() {
        let dir = tempfile::tempdir().unwrap();
        let original = b"\xD6\xD0\xCE\xC4\r\n";
        let path = write_file(dir.path(), "legacy.txt", original);
        let file = load(&path).unwrap();

        assert!(
            save(&file, "\u{4e2d}\u{6587} \u{1f680}\n", false).is_err(),
            "the user must choose conversion or Save As"
        );
        assert_eq!(std::fs::read(&path).unwrap(), original);
    }

    #[test]
    fn explicit_conversion_of_legacy_text_writes_exact_utf8() {
        let dir = tempfile::tempdir().unwrap();
        let path = write_file(dir.path(), "legacy.txt", b"\xD6\xD0\xCE\xC4\r\n");
        let file = load(&path).unwrap();
        let editor_text = "\u{4e2d}\u{6587} \u{1f680}\n";

        save_with(
            &file,
            editor_text,
            &SaveAuthorization::normal().enable_utf8_conversion(),
        )
        .unwrap();

        assert_eq!(
            std::fs::read_to_string(path).unwrap(),
            "\u{4e2d}\u{6587} \u{1f680}\r\n"
        );
    }

    #[test]
    fn a_utf16_document_keeps_its_encoding_and_its_bom() {
        // `encoding_rs` has no UTF-16 encoder — `Encoding::encode` silently
        // emits UTF-8 for it. Writing a UTF-8 body under a UTF-16 BOM produces
        // a file nothing can read, so `write` encodes the code units itself.
        let dir = tempfile::tempdir().unwrap();
        let original = b"\xFF\xFEh\x00i\x00\n\x00";
        let path = write_file(dir.path(), "a.txt", original);

        let file = load(&path).unwrap();
        assert_eq!(file.encoding.name(), "UTF-16LE");
        assert!(file.had_bom);
        assert_eq!(file.text, "hi\n");

        save(&file, &file.text, false).unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), original);
    }

    #[test]
    fn a_utf8_file_is_never_handed_to_the_detector() {
        // Valid UTF-8 is taken at face value. A detector run over it can only
        // introduce error, and on a developer's machine nearly every file is
        // UTF-8 — including ones whose bytes a detector would happily read as
        // some legacy single-byte encoding.
        let dir = tempfile::tempdir().unwrap();
        let path = write_file(dir.path(), "a.md", "café ünicode ①②③\n".as_bytes());
        let file = load(&path).unwrap();
        assert_eq!(file.encoding.name(), "UTF-8");
        assert_eq!(file.text, "café ünicode ①②③\n");
    }

    #[test]
    fn save_without_extension_preserves_document() {
        let dir = tempfile::tempdir().unwrap();
        let plain = write_file(dir.path(), "README", b"x\n");
        let plain_file = load(&plain).unwrap();
        save(&plain_file, "y\n", false).unwrap();
        assert_eq!(std::fs::read_to_string(&plain).unwrap(), "y\n");
    }

    #[test]
    fn cjk_content_round_trips() {
        let dir = tempfile::tempdir().unwrap();
        let original = "# 中文标题\n\n段落 🎉\n";
        let path = write_file(dir.path(), "a.md", original.as_bytes());
        let file = load(&path).unwrap();
        save(&file, &file.text, false).unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), original);
    }

    #[test]
    fn deleted_file_requires_an_explicit_recreate_decision() {
        let dir = tempfile::tempdir().unwrap();
        let path = write_file(dir.path(), "a.md", b"x\n");
        let file = load(&path).unwrap();
        std::fs::remove_file(&path).unwrap();

        assert!(save(&file, "restored\n", false).is_err());
        assert!(
            !path.exists(),
            "ordinary Save must not resurrect the old path"
        );
    }

    #[test]
    fn an_explicit_recreate_decision_restores_a_deleted_regular_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = write_file(dir.path(), "a.md", b"x\n");
        let file = load(&path).unwrap();
        std::fs::remove_file(&path).unwrap();

        let recreate = SaveAuthorization::normal()
            .authorize_missing_recreation(&file)
            .unwrap();
        save_with(&file, "restored\n", &recreate).unwrap();

        assert_eq!(std::fs::read_to_string(path).unwrap(), "restored\n");
    }

    #[test]
    fn recreate_never_overwrites_a_path_that_reappeared_after_the_prompt() {
        let dir = tempfile::tempdir().unwrap();
        let path = write_file(dir.path(), "a.md", b"original\n");
        let file = load(&path).unwrap();
        std::fs::remove_file(&path).unwrap();
        let recreate = SaveAuthorization::normal()
            .authorize_missing_recreation(&file)
            .unwrap();
        let reappeared_path = path.clone();
        install_save_commit_hook(move || std::fs::write(reappeared_path, "external\n").unwrap());

        let error = save_with(&file, "my edit\n", &recreate).unwrap_err();

        assert!(matches!(error, SaveError::Conflict));
        assert_eq!(std::fs::read_to_string(path).unwrap(), "external\n");
    }

    #[cfg(any(target_os = "windows", unix))]
    #[test]
    fn saving_through_a_symbolic_link_preserves_the_link() {
        let dir = tempfile::tempdir().unwrap();
        let target = write_file(dir.path(), "AGENTS.md", b"old\n");
        let link = dir.path().join("CLAUDE.md");

        #[cfg(target_os = "windows")]
        if let Err(err) = std::os::windows::fs::symlink_file(&target, &link) {
            eprintln!("skipping file-symlink test: {err}");
            return;
        }
        #[cfg(unix)]
        std::os::unix::fs::symlink(&target, &link).unwrap();

        let file = load(&link).unwrap();
        save(&file, "new\n", false).unwrap();

        assert!(
            std::fs::symlink_metadata(&link)
                .unwrap()
                .file_type()
                .is_symlink(),
            "Save must not replace the link with a regular file"
        );
        assert_eq!(std::fs::read_to_string(&target).unwrap(), "new\n");
    }

    #[cfg(any(target_os = "windows", unix))]
    #[test]
    fn a_missing_symbolic_link_target_is_never_recreated_implicitly() {
        let dir = tempfile::tempdir().unwrap();
        let target = write_file(dir.path(), "AGENTS.md", b"old\n");
        let link = dir.path().join("CLAUDE.md");

        #[cfg(target_os = "windows")]
        if let Err(err) = std::os::windows::fs::symlink_file(&target, &link) {
            eprintln!("skipping file-symlink test: {err}");
            return;
        }
        #[cfg(unix)]
        std::os::unix::fs::symlink(&target, &link).unwrap();

        let file = load(&link).unwrap();
        std::fs::remove_file(&target).unwrap();
        let error = save_with(&file, "new\n", &SaveAuthorization::normal()).unwrap_err();

        assert!(matches!(error, SaveError::SourceIdentityChanged));
        assert!(!target.exists());
        assert!(
            std::fs::symlink_metadata(link)
                .unwrap()
                .file_type()
                .is_symlink()
        );
    }
}
