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

    fn from_bytes(meta: &std::fs::Metadata, bytes: &[u8], object_id: Option<FileObjectId>) -> Self {
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
pub(crate) fn file_object_id(file: &File) -> std::io::Result<Option<FileObjectId>> {
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
pub(crate) fn file_object_id(_file: &File) -> std::io::Result<Option<FileObjectId>> {
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

/// A file loaded into the editor.
#[derive(Debug, Clone)]
pub struct LoadedFile {
    pub path: PathBuf,
    pub text: String,
    pub stamp: FileStamp,
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
        newline,
        had_bom,
        encoding,
        decode_had_errors,
        source_identity,
    })
}

/// Check whether a recovery record still describes the source on disk.
///
/// Windows holds the final path entry open before inspecting any symbolic-link
/// target. This makes a regular-to-link replacement a clean mismatch even when
/// the replacement target cannot be opened, and keeps a matching link stable
/// until the complete loaded identity has been compared.
pub(crate) fn recovery_source_matches(
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
    /// A replacement may have raced an external writer. Every path listed here
    /// is intentionally retained for the user to inspect; the editor buffer
    /// remains authoritative until they choose what to do next.
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
    /// is at the destination. All potentially relevant paths were retained.
    Indeterminate,
}

impl std::fmt::Display for SaveError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SaveError::Conflict => {
                f.write_str("the file changed on disk since it was opened; reload or save a copy")
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
}

pub fn save_with(
    file: &LoadedFile,
    text: &str,
    authorization: &SaveAuthorization,
) -> Result<SaveOutcome, SaveError> {
    let (destination, recreated_missing, expected) = save_destination(file, authorization)?;

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
    let temp = stage(&destination, &bytes)?;
    let (stamp, source_identity) = CommitPlan {
        source_path: &file.path,
        expected_source_identity: &file.source_identity,
        destination: &destination,
        expected_destination_stamp: expected.as_ref(),
        recreates_missing: recreated_missing,
        prepared_bytes: &bytes,
    }
    .commit_staged(temp)?;

    Ok(SaveOutcome {
        stamp,
        encoding,
        had_bom,
        source_identity,
    })
}

fn save_destination(
    file: &LoadedFile,
    authorization: &SaveAuthorization,
) -> Result<(PathBuf, bool, Option<FileStamp>), SaveError> {
    if matches!(
        authorization.source,
        SourceSaveAuthorization::RecreateMissing
    ) {
        if file.source_identity != SourceIdentity::Regular {
            return Err(SaveError::SourceIdentityChanged);
        }
        return match source_identity(&file.path) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                Ok((file.path.clone(), true, None))
            }
            Ok(SourceIdentity::Regular) => Err(SaveError::Conflict),
            Ok(SourceIdentity::SymbolicLink { .. }) => Err(SaveError::SourceIdentityChanged),
            Err(error) => Err(SaveError::Io(error)),
        };
    }

    let destination = current_save_destination(file)?;
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
pub fn save_as(
    path: &Path,
    text: &str,
    newline: Newline,
    had_bom: bool,
) -> Result<LoadedFile, SaveError> {
    let (source_identity, destination, recreated_missing) = match source_identity(path) {
        Ok(identity) => {
            let destination = match &identity {
                SourceIdentity::Regular => path.to_path_buf(),
                SourceIdentity::SymbolicLink {
                    resolved_target, ..
                } => resolved_target.clone(),
            };
            (identity, destination, false)
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            (SourceIdentity::Regular, path.to_path_buf(), true)
        }
        Err(error) => return Err(SaveError::Io(error)),
    };
    let expected = if recreated_missing {
        None
    } else {
        Some(FileStamp::of(&destination).map_err(SaveError::Io)?)
    };
    let bytes = encode(text, newline, had_bom, UTF_8)?;
    let temp = stage(&destination, &bytes)?;
    let (stamp, source_identity) = CommitPlan {
        source_path: path,
        expected_source_identity: &source_identity,
        destination: &destination,
        expected_destination_stamp: expected.as_ref(),
        recreates_missing: recreated_missing,
        prepared_bytes: &bytes,
    }
    .commit_staged(temp)?;

    Ok(LoadedFile {
        path: path.to_path_buf(),
        text: text.to_string(),
        stamp,
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
}

impl CommitPlan<'_> {
    fn commit_staged(
        self,
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
        let source_identity = verify_committed_entry(
            self.source_path,
            self.expected_source_identity,
            self.destination,
            &stamp,
        )?;
        Ok((stamp, source_identity))
    }

    fn verify_preconditions(&self) -> Result<(), SaveError> {
        verify_path_preconditions(
            self.source_path,
            self.expected_source_identity,
            self.destination,
            self.expected_destination_stamp,
            self.recreates_missing,
        )
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
        SourceIdentity::SymbolicLink { .. } => {
            Some(open_source_entry_guard(source_path).map_err(|error| {
                if error.kind() == std::io::ErrorKind::NotFound {
                    SaveError::SourceIdentityChanged
                } else {
                    SaveError::Io(error)
                }
            })?)
        }
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

#[cfg(test)]
type SaveReservationHook = Box<dyn FnOnce(&Path) -> std::io::Result<()>>;

#[cfg(test)]
thread_local! {
    static SAVE_COMMIT_HOOK: std::cell::RefCell<Option<Box<dyn FnOnce()>>> = const {
        std::cell::RefCell::new(None)
    };
    static SAVE_POST_COMMIT_HOOK: std::cell::RefCell<Option<Box<dyn FnOnce()>>> = const {
        std::cell::RefCell::new(None)
    };
    #[cfg(windows)]
    static RECOVERY_SOURCE_MATCH_HOOK: std::cell::RefCell<Option<Box<dyn FnOnce()>>> = const {
        std::cell::RefCell::new(None)
    };
    #[cfg(windows)]
    static SAVE_RESERVATION_HOOK: std::cell::RefCell<Option<SaveReservationHook>> = const {
        std::cell::RefCell::new(None)
    };
}

#[cfg(test)]
fn install_save_commit_hook(hook: impl FnOnce() + 'static) {
    SAVE_COMMIT_HOOK.with(|slot| {
        assert!(
            slot.borrow().is_none(),
            "a save commit hook is already installed"
        );
        *slot.borrow_mut() = Some(Box::new(hook));
    });
}

#[cfg(test)]
fn install_save_post_commit_hook(hook: impl FnOnce() + 'static) {
    SAVE_POST_COMMIT_HOOK.with(|slot| {
        assert!(
            slot.borrow().is_none(),
            "a save post-commit hook is already installed"
        );
        *slot.borrow_mut() = Some(Box::new(hook));
    });
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
fn install_save_reservation_hook(hook: impl FnOnce(&Path) -> std::io::Result<()> + 'static) {
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

#[cfg(all(test, windows))]
fn run_recovery_source_match_hook() {
    RECOVERY_SOURCE_MATCH_HOOK.with(|slot| {
        if let Some(hook) = slot.borrow_mut().take() {
            hook();
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

#[cfg(test)]
mod tests {
    use super::*;

    fn write_file(dir: &Path, name: &str, content: &[u8]) -> PathBuf {
        let path = dir.join(name);
        std::fs::write(&path, content).unwrap();
        path
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
            save_as(&link, "editor text\n", Newline::Lf, false).unwrap();
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
        let error = save_as(&link, "editor text\n", Newline::Lf, false).unwrap_err();
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
    fn two_saves_of_one_file_cannot_collide_on_a_temp_path() {
        // The temp name used to be derived from the target, so it was the same
        // on every save: two concurrent writers truncated each other's temp
        // file and one of them renamed a half-written document into place.
        let dir = tempfile::tempdir().unwrap();
        let path = write_file(dir.path(), "a.md", b"start\n");
        let file = load(&path).unwrap();

        let source = include_str!("fs.rs");
        let body = source
            .split_once("fn stage(")
            .expect("stage must exist")
            .1
            .split_once("\n#[cfg(test)]")
            .map(|(body, _)| body)
            .unwrap_or(source);
        assert!(
            !body.contains("with_extension"),
            "the temp path must not be derived from the target name: a \
             deterministic name is the same on every save, so two saves of one \
             document race on it"
        );
        assert!(
            body.contains("tempfile::Builder"),
            "the temp file must come from `tempfile`, which randomizes the name"
        );

        // And a file with no extension no longer produces `README..markturbo-tmp`.
        let plain = write_file(dir.path(), "README", b"x\n");
        let plain_file = load(&plain).unwrap();
        save(&plain_file, "y\n", false).unwrap();
        assert_eq!(std::fs::read_to_string(&plain).unwrap(), "y\n");
        assert!(file.path.exists());
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
