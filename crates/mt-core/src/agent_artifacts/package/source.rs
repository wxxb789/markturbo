use std::fs::{self as std_fs, File};
use std::io::Read;
use std::path::{Component, Path, PathBuf};
use std::time::SystemTime;

use sha2::{Digest as _, Sha256};

use crate::document::io::{
    self as document_io, FileObjectId, SkillObjectIdentity, SkillOrigin, SkillOriginRoot,
};
use crate::review::{MAX_SKILL_FILE_BYTES, MAX_SKILL_PACKAGE_BYTES};

use super::ReviewRequestBuildError;

pub(super) const MAX_AGENT_SKILL_INVENTORY_ENTRIES: usize = 1_024;
pub(super) const MAX_AGENT_SKILL_DIRECTORY_DEPTH: usize = 32;

/// Identity captured while securely opening and reading one source file.
#[derive(PartialEq, Eq)]
pub(super) struct SkillSourceIdentity {
    pub(super) path: String,
    pub(super) byte_size: u64,
    pub(super) digest: [u8; 32],
    pub(super) modified: Option<SystemTime>,
    pub(super) object_identity: SkillObjectIdentity,
    pub(super) object_id: Option<FileObjectId>,
    pub(super) editor_transform: Option<SkillEditorTextTransform>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) struct SkillEditorTextTransform {
    pub(super) strip_utf8_bom: bool,
    pub(super) normalize_crlf: bool,
}

impl SkillEditorTextTransform {
    pub(super) const IDENTITY: Self = Self {
        strip_utf8_bom: false,
        normalize_crlf: false,
    };
}

/// A source file read through the authenticated Skill root.
pub(super) struct SkillSourceFile {
    pub(super) bytes: Vec<u8>,
    pub(super) identity: SkillSourceIdentity,
}

#[derive(Clone, Copy)]
pub(super) enum SkillOmissionKind {
    Symlink,
    NonRegular,
}

/// An inventory omission before package-specific labels are assigned.
pub(super) struct SkillSourceOmission {
    pub(super) path: String,
    pub(super) kind: SkillOmissionKind,
}

/// Successfully read bytes and inventory, still bound to the load-time origin.
pub(super) struct AcquiredSkillInventory {
    pub(super) origin: SkillOrigin,
    pub(super) files: Vec<SkillSourceFile>,
    pub(super) omissions: Vec<SkillSourceOmission>,
}

/// Verified bytes and file stamp for a file in a frozen source inventory.
pub(super) struct VerifiedFrozenSource {
    pub(super) relative_path: String,
    pub(super) bytes: Vec<u8>,
    pub(super) stamp: document_io::FileStamp,
}

struct InventoriedSkillFile {
    relative_path: String,
    file: File,
}

/// Acquire every source through the exact origin captured when the entrypoint was loaded.
///
/// Directory handles remain alive until all inventoried files have been read,
/// and the returned origin is only produced after a second root-identity check.
pub(super) fn acquire_skill_inventory(
    origin: &SkillOrigin,
    dirty_entrypoint_byte_size: Option<u64>,
) -> Result<AcquiredSkillInventory, ReviewRequestBuildError> {
    let allow_dirty_entrypoint = dirty_entrypoint_byte_size.is_some();
    let root = origin
        .open_root()
        .map_err(|_| ReviewRequestBuildError::AgentSkillSourceChanged)?;
    #[cfg(test)]
    run_skill_root_validated_hook();

    if let Some(byte_size) = dirty_entrypoint_byte_size
        && byte_size > MAX_SKILL_FILE_BYTES
    {
        return Err(ReviewRequestBuildError::AgentSkillFileTooLarge { byte_size });
    }
    // Unsaved entrypoint text is disclosed too, so reserve its bytes before
    // reading any supporting files against the shared package limit.
    let mut total_byte_size = dirty_entrypoint_byte_size.unwrap_or_default();
    if total_byte_size > MAX_SKILL_PACKAGE_BYTES {
        return Err(ReviewRequestBuildError::AgentSkillPackageTooLarge {
            byte_size: total_byte_size,
        });
    }

    let mut inventoried_files = Vec::new();
    let mut omissions = Vec::new();
    let mut held_directories = Vec::new();
    let mut visited_directories = 1;
    collect_agent_skill_paths(
        &root,
        root.directory(),
        root.canonical_root(),
        "",
        &mut inventoried_files,
        &mut omissions,
        &mut held_directories,
        &mut visited_directories,
        0,
        allow_dirty_entrypoint,
    )?;
    inventoried_files.sort_by(|left, right| {
        left.relative_path
            .as_bytes()
            .cmp(right.relative_path.as_bytes())
    });
    let mut files = Vec::with_capacity(inventoried_files.len());
    let mut found_entrypoint = false;
    for inventoried in inventoried_files {
        let relative_path = inventoried.relative_path;
        if allow_dirty_entrypoint && relative_path == "SKILL.md" {
            continue;
        }
        let source = read_opened_skill_supporting_file(inventoried.file, &relative_path)?;
        if relative_path == "SKILL.md" {
            if !origin.matches_entrypoint(
                source.identity.object_identity,
                source.identity.byte_size,
                &source.identity.digest,
                source.identity.modified,
                source.identity.object_id,
            ) {
                return Err(ReviewRequestBuildError::AgentSkillSourceChanged);
            }
            found_entrypoint = true;
        }
        total_byte_size = total_byte_size
            .checked_add(source.identity.byte_size)
            .ok_or(ReviewRequestBuildError::AgentSkillPackageTooLarge {
                byte_size: u64::MAX,
            })?;
        if total_byte_size > MAX_SKILL_PACKAGE_BYTES {
            return Err(ReviewRequestBuildError::AgentSkillPackageTooLarge {
                byte_size: total_byte_size,
            });
        }
        files.push(source);
    }
    if !allow_dirty_entrypoint && !found_entrypoint {
        return Err(ReviewRequestBuildError::AgentSkillSourceChanged);
    }

    // The canonical descriptor remained authoritative throughout traversal;
    // this second check rejects an ancestor alias changed during inventory.
    origin
        .open_root()
        .map_err(|_| ReviewRequestBuildError::AgentSkillSourceChanged)?;

    Ok(AcquiredSkillInventory {
        origin: origin.clone(),
        files,
        omissions,
    })
}

fn collect_agent_skill_paths(
    root: &SkillOriginRoot,
    directory: &File,
    directory_path: &Path,
    relative_directory: &str,
    files: &mut Vec<InventoriedSkillFile>,
    omissions: &mut Vec<SkillSourceOmission>,
    held_directories: &mut Vec<File>,
    visited_directories: &mut usize,
    depth: usize,
    allow_dirty_entrypoint: bool,
) -> Result<(), ReviewRequestBuildError> {
    if depth > MAX_AGENT_SKILL_DIRECTORY_DEPTH {
        return Err(ReviewRequestBuildError::AgentSkillPackageTooLarge {
            byte_size: u64::MAX,
        });
    }
    visit_skill_directory_entries(directory, directory_path, &mut |entry| {
        collect_agent_skill_entry(
            root,
            directory,
            directory_path,
            relative_directory,
            files,
            omissions,
            held_directories,
            visited_directories,
            depth,
            allow_dirty_entrypoint,
            entry,
        )
    })
}

fn collect_agent_skill_entry(
    root: &SkillOriginRoot,
    directory: &File,
    directory_path: &Path,
    relative_directory: &str,
    files: &mut Vec<InventoriedSkillFile>,
    omissions: &mut Vec<SkillSourceOmission>,
    held_directories: &mut Vec<File>,
    visited_directories: &mut usize,
    depth: usize,
    allow_dirty_entrypoint: bool,
    entry: SkillDirectoryEntry,
) -> Result<(), ReviewRequestBuildError> {
    #[cfg(windows)]
    let _ = directory;
    let name = entry.name;
    let file_name = name
        .to_str()
        .ok_or(ReviewRequestBuildError::AgentSkillPathIsNotUtf8)?;
    let relative_path = if relative_directory.is_empty() {
        file_name.to_owned()
    } else {
        let mut path = String::with_capacity(relative_directory.len() + 1 + file_name.len());
        path.push_str(relative_directory);
        path.push('/');
        path.push_str(file_name);
        path
    };
    validated_skill_relative_components(&relative_path)?;

    #[cfg(all(unix, any(target_os = "linux", target_os = "macos")))]
    if allow_dirty_entrypoint
        && relative_path == "SKILL.md"
        && entry.kind == Some(UNIX_DT_DIRECTORY)
    {
        return Err(ReviewRequestBuildError::AgentSkillSourceChanged);
    }

    #[cfg(windows)]
    {
        let path = directory_path.join(&name);
        let metadata = match std_fs::symlink_metadata(&path) {
            Ok(metadata) => metadata,
            Err(error)
                if allow_dirty_entrypoint
                    && relative_path == "SKILL.md"
                    && error.kind() == std::io::ErrorKind::NotFound =>
            {
                return Ok(());
            }
            Err(_) => return Err(ReviewRequestBuildError::AgentSkillReadFailed),
        };
        if windows_metadata_is_reparse(&metadata) {
            push_skill_omission(
                files,
                omissions,
                allow_dirty_entrypoint,
                SkillSourceOmission {
                    path: relative_path,
                    kind: SkillOmissionKind::Symlink,
                },
            )?;
        } else if metadata.is_dir() && allow_dirty_entrypoint && relative_path == "SKILL.md" {
            return Err(ReviewRequestBuildError::AgentSkillSourceChanged);
        } else if metadata.is_dir() {
            record_skill_directory(visited_directories)?;
            let child = hold_regular_skill_directory(&path)?;
            collect_agent_skill_paths(
                root,
                &child,
                &path,
                &relative_path,
                files,
                omissions,
                held_directories,
                visited_directories,
                depth.saturating_add(1),
                allow_dirty_entrypoint,
            )?;
            held_directories.push(child);
        } else if metadata.is_file() {
            if allow_dirty_entrypoint && relative_path == "SKILL.md" {
                return Ok(());
            }
            ensure_skill_inventory_room(files, omissions, allow_dirty_entrypoint)?;
            let file = open_regular_skill_file(&path)
                .map_err(|_| ReviewRequestBuildError::AgentSkillReadFailed)?;
            if !opened_skill_file_is_regular(
                &file
                    .metadata()
                    .map_err(|_| ReviewRequestBuildError::AgentSkillReadFailed)?,
            ) {
                return Err(ReviewRequestBuildError::AgentSkillSourceChanged);
            }
            files.push(InventoriedSkillFile {
                relative_path,
                file,
            });
        } else {
            push_skill_omission(
                files,
                omissions,
                allow_dirty_entrypoint,
                SkillSourceOmission {
                    path: relative_path,
                    kind: SkillOmissionKind::NonRegular,
                },
            )?;
        }
    }

    #[cfg(all(unix, any(target_os = "linux", target_os = "macos")))]
    {
        let child = match open_skill_child_at(directory, &name) {
            Ok(child) => child,
            Err(_) if skill_child_is_symlink_at(directory, &name) => {
                push_skill_omission(
                    files,
                    omissions,
                    allow_dirty_entrypoint,
                    SkillSourceOmission {
                        path: relative_path,
                        kind: SkillOmissionKind::Symlink,
                    },
                )?;
                return Ok(());
            }
            Err(error)
                if allow_dirty_entrypoint
                    && relative_path == "SKILL.md"
                    && (error.kind() == std::io::ErrorKind::NotFound
                        || entry.kind == Some(UNIX_DT_REGULAR)) =>
            {
                return Ok(());
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Err(ReviewRequestBuildError::AgentSkillSourceChanged);
            }
            Err(_) => return Err(ReviewRequestBuildError::AgentSkillReadFailed),
        };
        let metadata = child
            .metadata()
            .map_err(|_| ReviewRequestBuildError::AgentSkillReadFailed)?;
        if entry.inode.is_some_and(|inode| {
            use std::os::unix::fs::MetadataExt as _;
            metadata.ino() != inode
        }) {
            return Err(ReviewRequestBuildError::AgentSkillSourceChanged);
        }
        if metadata.file_type().is_symlink() {
            push_skill_omission(
                files,
                omissions,
                allow_dirty_entrypoint,
                SkillSourceOmission {
                    path: relative_path,
                    kind: SkillOmissionKind::Symlink,
                },
            )?;
        } else if metadata.is_dir() && allow_dirty_entrypoint && relative_path == "SKILL.md" {
            return Err(ReviewRequestBuildError::AgentSkillSourceChanged);
        } else if metadata.is_dir() {
            record_skill_directory(visited_directories)?;
            collect_agent_skill_paths(
                root,
                &child,
                &directory_path.join(&name),
                &relative_path,
                files,
                omissions,
                held_directories,
                visited_directories,
                depth.saturating_add(1),
                allow_dirty_entrypoint,
            )?;
            held_directories.push(child);
        } else if metadata.is_file() {
            if allow_dirty_entrypoint && relative_path == "SKILL.md" {
                return Ok(());
            }
            ensure_skill_inventory_room(files, omissions, allow_dirty_entrypoint)?;
            files.push(InventoriedSkillFile {
                relative_path,
                file: child,
            });
        } else {
            push_skill_omission(
                files,
                omissions,
                allow_dirty_entrypoint,
                SkillSourceOmission {
                    path: relative_path,
                    kind: SkillOmissionKind::NonRegular,
                },
            )?;
        }
    }
    Ok(())
}

fn ensure_skill_inventory_room(
    files: &[InventoriedSkillFile],
    omissions: &[SkillSourceOmission],
    allow_dirty_entrypoint: bool,
) -> Result<(), ReviewRequestBuildError> {
    let omission_count = if allow_dirty_entrypoint {
        omissions
            .iter()
            .filter(|omission| omission.path != "SKILL.md")
            .count()
    } else {
        omissions.len()
    };
    if files.len().saturating_add(omission_count) >= MAX_AGENT_SKILL_INVENTORY_ENTRIES {
        Err(ReviewRequestBuildError::AgentSkillPackageTooLarge {
            byte_size: u64::MAX,
        })
    } else {
        Ok(())
    }
}

fn record_skill_directory(visited_directories: &mut usize) -> Result<(), ReviewRequestBuildError> {
    *visited_directories = visited_directories.checked_add(1).ok_or(
        ReviewRequestBuildError::AgentSkillPackageTooLarge {
            byte_size: u64::MAX,
        },
    )?;
    if *visited_directories > MAX_AGENT_SKILL_INVENTORY_ENTRIES {
        Err(ReviewRequestBuildError::AgentSkillPackageTooLarge {
            byte_size: u64::MAX,
        })
    } else {
        Ok(())
    }
}

fn push_skill_omission(
    files: &[InventoriedSkillFile],
    omissions: &mut Vec<SkillSourceOmission>,
    allow_dirty_entrypoint: bool,
    omission: SkillSourceOmission,
) -> Result<(), ReviewRequestBuildError> {
    ensure_skill_inventory_room(files, omissions, allow_dirty_entrypoint)?;
    omissions.push(omission);
    Ok(())
}

struct SkillDirectoryEntry {
    name: std::ffi::OsString,
    #[cfg(unix)]
    inode: Option<u64>,
    #[cfg(unix)]
    kind: Option<u8>,
}

#[cfg(windows)]
fn visit_skill_directory_entries(
    _directory: &File,
    path: &Path,
    visit: &mut impl FnMut(SkillDirectoryEntry) -> Result<(), ReviewRequestBuildError>,
) -> Result<(), ReviewRequestBuildError> {
    for entry in
        std_fs::read_dir(path).map_err(|_| ReviewRequestBuildError::AgentSkillReadFailed)?
    {
        let entry = entry.map_err(|_| ReviewRequestBuildError::AgentSkillReadFailed)?;
        visit(SkillDirectoryEntry {
            name: entry.file_name(),
        })?;
    }
    Ok(())
}

// This dirent layout is verified only for 64-bit Linux/macOS. In particular,
// 32-bit Linux uses a different inode layout and must fail closed below.
#[cfg(all(
    unix,
    target_pointer_width = "64",
    any(target_os = "linux", target_os = "macos")
))]
fn visit_skill_directory_entries(
    directory: &File,
    _path: &Path,
    visit: &mut impl FnMut(SkillDirectoryEntry) -> Result<(), ReviewRequestBuildError>,
) -> Result<(), ReviewRequestBuildError> {
    read_skill_directory_descriptor(directory, visit)
}

#[cfg(all(unix, target_pointer_width = "64", target_os = "linux"))]
#[repr(C)]
struct SkillDirectoryRecord {
    inode: u64,
    _offset: i64,
    _record_length: u16,
    file_type: u8,
    name: [std::ffi::c_char; 0],
}

#[cfg(all(unix, target_pointer_width = "64", target_os = "macos"))]
#[repr(C)]
struct SkillDirectoryRecord {
    inode: u64,
    _seek_offset: u64,
    _record_length: u16,
    _name_length: u16,
    file_type: u8,
    name: [std::ffi::c_char; 0],
}

#[cfg(all(
    unix,
    target_pointer_width = "64",
    any(target_os = "linux", target_os = "macos")
))]
fn read_skill_directory_descriptor(
    directory: &File,
    visit: &mut impl FnMut(SkillDirectoryEntry) -> Result<(), ReviewRequestBuildError>,
) -> Result<(), ReviewRequestBuildError> {
    use std::ffi::{CStr, c_char, c_int};
    use std::os::fd::IntoRawFd as _;
    use std::os::unix::ffi::OsStringExt as _;

    #[repr(C)]
    struct DirectoryStream {
        _opaque: [u8; 0],
    }

    unsafe extern "C" {
        #[cfg_attr(
            all(target_os = "macos", target_arch = "x86_64"),
            link_name = "fdopendir$INODE64"
        )]
        fn fdopendir(descriptor: c_int) -> *mut DirectoryStream;
        #[cfg_attr(
            all(target_os = "macos", target_arch = "x86_64"),
            link_name = "readdir$INODE64"
        )]
        fn readdir(directory: *mut DirectoryStream) -> *mut SkillDirectoryRecord;
        fn closedir(directory: *mut DirectoryStream) -> c_int;
        fn close(descriptor: c_int) -> c_int;
        #[cfg(target_os = "linux")]
        fn __errno_location() -> *mut c_int;
        #[cfg(target_os = "macos")]
        fn __error() -> *mut c_int;
    }

    struct Stream(*mut DirectoryStream);
    impl Drop for Stream {
        fn drop(&mut self) {
            // SAFETY: the stream owns the descriptor transferred to fdopendir.
            unsafe {
                closedir(self.0);
            }
        }
    }

    let descriptor = directory
        .try_clone()
        .map_err(|_| ReviewRequestBuildError::AgentSkillReadFailed)?
        .into_raw_fd();
    // SAFETY: `descriptor` is an owned duplicate of an open directory fd.
    let stream = unsafe { fdopendir(descriptor) };
    if stream.is_null() {
        // SAFETY: fdopendir did not take ownership when it failed.
        unsafe {
            close(descriptor);
        }
        return Err(ReviewRequestBuildError::AgentSkillReadFailed);
    }
    let stream = Stream(stream);
    loop {
        // POSIX readdir signals end-of-directory and errors with the same null
        // pointer, so clear and inspect errno for each call.
        #[cfg(target_os = "linux")]
        let errno = unsafe { __errno_location() };
        #[cfg(target_os = "macos")]
        let errno = unsafe { __error() };
        unsafe { *errno = 0 };
        // SAFETY: `stream` remains live until the loop exits.
        let record = unsafe { readdir(stream.0) };
        if record.is_null() {
            if unsafe { *errno } != 0 {
                return Err(ReviewRequestBuildError::AgentSkillReadFailed);
            }
            break;
        }
        let name_offset = std::mem::offset_of!(SkillDirectoryRecord, name);
        // SAFETY: `readdir` returned a valid native dirent whose name field is
        // a NUL-terminated byte string for the lifetime of the next call.
        let name =
            unsafe { CStr::from_ptr((record.cast::<u8>().add(name_offset)).cast::<c_char>()) };
        let bytes = name.to_bytes();
        if bytes == b"." || bytes == b".." {
            continue;
        }
        // SAFETY: `record` points to a valid platform dirent with the declared
        // ABI layout for this target.
        let inode = unsafe { (*record).inode };
        let kind = unsafe { (*record).file_type };
        visit(SkillDirectoryEntry {
            name: std::ffi::OsString::from_vec(bytes.to_vec()),
            inode: Some(inode),
            kind: Some(kind),
        })?;
    }
    Ok(())
}

#[cfg(all(
    unix,
    not(all(
        target_pointer_width = "64",
        any(target_os = "linux", target_os = "macos")
    ))
))]
fn visit_skill_directory_entries(
    _directory: &File,
    _path: &Path,
    _visit: &mut impl FnMut(SkillDirectoryEntry) -> Result<(), ReviewRequestBuildError>,
) -> Result<(), ReviewRequestBuildError> {
    Err(ReviewRequestBuildError::AgentSkillReadFailed)
}

#[cfg(not(any(windows, unix)))]
fn visit_skill_directory_entries(
    _directory: &File,
    _path: &Path,
    _visit: &mut impl FnMut(SkillDirectoryEntry) -> Result<(), ReviewRequestBuildError>,
) -> Result<(), ReviewRequestBuildError> {
    Err(ReviewRequestBuildError::AgentSkillReadFailed)
}

/// On Windows, directory handles deny write/delete sharing. Every directory
/// reached below the authenticated root remains held through file reads.
#[cfg(windows)]
fn hold_regular_skill_directory(path: &Path) -> Result<File, ReviewRequestBuildError> {
    use std::os::windows::fs::{MetadataExt as _, OpenOptionsExt as _};
    use windows::Win32::Storage::FileSystem::{
        FILE_ATTRIBUTE_REPARSE_POINT, FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT,
        FILE_SHARE_READ,
    };

    let file = std_fs::OpenOptions::new()
        .read(true)
        .share_mode(FILE_SHARE_READ.0)
        .custom_flags(FILE_FLAG_BACKUP_SEMANTICS.0 | FILE_FLAG_OPEN_REPARSE_POINT.0)
        .open(path)
        .map_err(|_| ReviewRequestBuildError::AgentSkillReadFailed)?;
    let metadata = file
        .metadata()
        .map_err(|_| ReviewRequestBuildError::AgentSkillReadFailed)?;
    if !metadata.is_dir() || metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT.0 != 0 {
        return Err(ReviewRequestBuildError::AgentSkillSourceChanged);
    }
    Ok(file)
}

#[cfg(test)]
pub(super) fn read_regular_skill_supporting_file(
    root: &SkillOriginRoot,
    relative_path: &str,
) -> Result<SkillSourceFile, ReviewRequestBuildError> {
    let file = open_skill_file(root, relative_path)
        .map_err(|_| ReviewRequestBuildError::AgentSkillReadFailed)?;
    read_opened_skill_supporting_file(file, relative_path)
}

fn read_opened_skill_supporting_file(
    mut file: File,
    relative_path: &str,
) -> Result<SkillSourceFile, ReviewRequestBuildError> {
    let before = file
        .metadata()
        .map_err(|_| ReviewRequestBuildError::AgentSkillReadFailed)?;
    if !opened_skill_file_is_regular(&before) {
        return Err(ReviewRequestBuildError::AgentSkillSourceChanged);
    }
    if before.len() > MAX_SKILL_FILE_BYTES {
        return Err(ReviewRequestBuildError::AgentSkillFileTooLarge {
            byte_size: before.len(),
        });
    }
    let object_id = document_io::file_object_id(&file)
        .map_err(|_| ReviewRequestBuildError::AgentSkillReadFailed)?;
    let object_identity = document_io::skill_object_identity(&file)
        .map_err(|_| ReviewRequestBuildError::AgentSkillReadFailed)?;
    let mut bytes = Vec::with_capacity(usize::try_from(before.len()).unwrap_or(0));
    file.by_ref()
        .take(MAX_SKILL_FILE_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| ReviewRequestBuildError::AgentSkillReadFailed)?;
    if bytes.len() as u64 > MAX_SKILL_FILE_BYTES {
        return Err(ReviewRequestBuildError::AgentSkillFileTooLarge {
            byte_size: bytes.len() as u64,
        });
    }
    let after = file
        .metadata()
        .map_err(|_| ReviewRequestBuildError::AgentSkillReadFailed)?;
    if !opened_skill_file_is_regular(&after)
        || before.len() != bytes.len() as u64
        || after.len() != bytes.len() as u64
        || before.modified().ok() != after.modified().ok()
        || document_io::skill_object_identity(&file)
            .map_err(|_| ReviewRequestBuildError::AgentSkillReadFailed)?
            != object_identity
    {
        return Err(ReviewRequestBuildError::AgentSkillSourceChanged);
    }
    Ok(SkillSourceFile {
        identity: SkillSourceIdentity {
            path: relative_path.to_owned(),
            byte_size: after.len(),
            digest: Sha256::digest(&bytes).into(),
            modified: after.modified().ok(),
            object_identity,
            object_id,
            editor_transform: skill_editor_text_transform(&bytes),
        },
        bytes,
    })
}

pub(super) fn read_frozen_source(
    origin: &SkillOrigin,
    candidate: &Path,
    entrypoint: Option<&SkillSourceIdentity>,
    supporting_files: &[SkillSourceIdentity],
) -> Option<VerifiedFrozenSource> {
    let relative_path = normalized_skill_origin_path(origin, candidate).ok()?;
    let expected = if relative_path == "SKILL.md" {
        entrypoint
    } else {
        supporting_files
            .iter()
            .find(|source| source.path == relative_path)
    }?;
    let root = origin.open_root().ok()?;
    let file = open_skill_file(&root, &relative_path).ok()?;
    let (bytes, stamp) = read_opened_frozen_source(file, expected)?;
    origin.open_root().ok()?;
    Some(VerifiedFrozenSource {
        relative_path,
        bytes,
        stamp,
    })
}

fn read_opened_frozen_source(
    mut file: File,
    expected: &SkillSourceIdentity,
) -> Option<(Vec<u8>, document_io::FileStamp)> {
    let Ok(before) = file.metadata() else {
        return None;
    };
    if !opened_skill_file_is_regular(&before)
        || before.len() != expected.byte_size
        || before.modified().ok() != expected.modified
    {
        return None;
    }
    let Ok(object_identity) = document_io::skill_object_identity(&file) else {
        return None;
    };
    let Ok(object_id) = document_io::file_object_id(&file) else {
        return None;
    };
    if object_identity != expected.object_identity || object_id != expected.object_id {
        return None;
    }

    let mut bytes = Vec::with_capacity(usize::try_from(expected.byte_size).ok()?);
    file.by_ref()
        .take(MAX_SKILL_FILE_BYTES + 1)
        .read_to_end(&mut bytes)
        .ok()?;
    let byte_size = bytes.len() as u64;
    if byte_size > MAX_SKILL_FILE_BYTES {
        return None;
    }

    let Ok(after) = file.metadata() else {
        return None;
    };
    let digest: [u8; 32] = Sha256::digest(&bytes).into();
    if byte_size != expected.byte_size
        || !opened_skill_file_is_regular(&after)
        || after.len() != byte_size
        || after.modified().ok() != expected.modified
        || document_io::skill_object_identity(&file).ok() != Some(expected.object_identity)
        || document_io::file_object_id(&file).ok() != Some(expected.object_id)
        || digest != expected.digest
    {
        return None;
    }
    let stamp = document_io::FileStamp::from_bytes(&after, &bytes, object_id);
    Some((bytes, stamp))
}

fn open_skill_file(root: &SkillOriginRoot, relative_path: &str) -> std::io::Result<File> {
    let components =
        validated_skill_relative_components(relative_path).map_err(invalid_skill_path_error)?;
    open_skill_file_components(root, &components)
}

#[cfg(windows)]
fn open_skill_file_components(
    root: &SkillOriginRoot,
    components: &[&str],
) -> std::io::Result<File> {
    let (filename, directories) = components
        .split_last()
        .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::InvalidInput, "empty Skill path"))?;
    let mut path = root.canonical_root().to_path_buf();
    let mut held = Vec::with_capacity(directories.len());
    for component in directories {
        path.push(component);
        held.push(hold_regular_skill_directory(&path).map_err(review_error_to_io)?);
    }
    path.push(filename);
    open_regular_skill_file(&path)
}

#[cfg(windows)]
fn open_regular_skill_file(path: &Path) -> std::io::Result<File> {
    use std::os::windows::fs::OpenOptionsExt as _;
    use windows::Win32::Storage::FileSystem::{FILE_FLAG_OPEN_REPARSE_POINT, FILE_SHARE_READ};

    std_fs::OpenOptions::new()
        .read(true)
        .share_mode(FILE_SHARE_READ.0)
        .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT.0)
        .open(path)
}

#[cfg(all(unix, any(target_os = "linux", target_os = "macos")))]
const UNIX_O_NOFOLLOW: i32 = if cfg!(target_os = "linux") {
    0o400000
} else {
    0x100
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
const UNIX_DT_DIRECTORY: u8 = 4;

#[cfg(all(unix, any(target_os = "linux", target_os = "macos")))]
const UNIX_DT_REGULAR: u8 = 8;

#[cfg(all(unix, any(target_os = "linux", target_os = "macos")))]
fn open_skill_child_at(directory: &File, name: &std::ffi::OsStr) -> std::io::Result<File> {
    use std::ffi::{CString, c_char, c_int};
    use std::os::fd::{AsRawFd as _, FromRawFd as _};
    use std::os::unix::ffi::OsStrExt as _;

    unsafe extern "C" {
        fn openat(directory: c_int, path: *const c_char, flags: c_int) -> c_int;
    }

    let name = CString::new(name.as_bytes())
        .map_err(|_| std::io::Error::new(std::io::ErrorKind::InvalidInput, "NUL in Skill name"))?;
    // O_NOFOLLOW makes the opened descriptor authoritative; O_NONBLOCK keeps
    // a raced FIFO from stalling inventory before it can be classified.
    // SAFETY: `directory` is a live directory fd and `name` is one component.
    let descriptor = unsafe {
        openat(
            directory.as_raw_fd(),
            name.as_ptr(),
            UNIX_O_NOFOLLOW | UNIX_O_NONBLOCK | UNIX_O_CLOEXEC,
        )
    };
    if descriptor < 0 {
        return Err(std::io::Error::last_os_error());
    }
    // SAFETY: `openat` returned a new owned descriptor.
    Ok(unsafe { File::from_raw_fd(descriptor) })
}

#[cfg(all(unix, any(target_os = "linux", target_os = "macos")))]
fn skill_child_is_symlink_at(directory: &File, name: &std::ffi::OsStr) -> bool {
    use std::ffi::{CString, c_char, c_int, c_void};
    use std::os::fd::AsRawFd as _;
    use std::os::unix::ffi::OsStrExt as _;

    unsafe extern "C" {
        fn readlinkat(
            directory: c_int,
            path: *const c_char,
            buffer: *mut c_char,
            size: usize,
        ) -> isize;
    }

    let Ok(name) = CString::new(name.as_bytes()) else {
        return false;
    };
    let mut probe = 0_u8;
    // SAFETY: `directory` is a live directory fd, the name is NUL-terminated,
    // and `probe` supplies one writable byte; no target is followed.
    unsafe {
        readlinkat(
            directory.as_raw_fd(),
            name.as_ptr(),
            (&raw mut probe).cast::<c_void>().cast::<c_char>(),
            1,
        ) >= 0
    }
}

#[cfg(all(unix, any(target_os = "linux", target_os = "macos")))]
fn open_skill_file_components(
    root: &SkillOriginRoot,
    components: &[&str],
) -> std::io::Result<File> {
    let mut directory = root.directory().try_clone()?;
    for (index, component) in components.iter().enumerate() {
        let next = open_skill_child_at(&directory, std::ffi::OsStr::new(component))?;
        let metadata = next.metadata()?;
        if index + 1 == components.len() {
            if !metadata.is_file() || metadata.file_type().is_symlink() {
                return Err(std::io::Error::other(
                    "Agent Skill source is not a regular file",
                ));
            }
            return Ok(next);
        }
        if !metadata.is_dir() || metadata.file_type().is_symlink() {
            return Err(std::io::Error::other(
                "Agent Skill path component is not a directory",
            ));
        }
        directory = next;
    }
    Err(std::io::Error::new(
        std::io::ErrorKind::InvalidInput,
        "Agent Skill supporting path is empty",
    ))
}

#[cfg(all(unix, not(any(target_os = "linux", target_os = "macos"))))]
fn open_skill_file_components(_: &SkillOriginRoot, _: &[&str]) -> std::io::Result<File> {
    Err(std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        "no descriptor-relative Agent Skill file open is available on this target",
    ))
}

#[cfg(not(any(windows, unix)))]
fn open_skill_file_components(_: &SkillOriginRoot, _: &[&str]) -> std::io::Result<File> {
    Err(std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        "no no-follow Agent Skill file open is available on this target",
    ))
}

#[cfg(windows)]
fn windows_metadata_is_reparse(metadata: &std_fs::Metadata) -> bool {
    use std::os::windows::fs::MetadataExt as _;
    use windows::Win32::Storage::FileSystem::FILE_ATTRIBUTE_REPARSE_POINT;

    metadata.file_type().is_symlink()
        || metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT.0 != 0
}

#[cfg(windows)]
fn opened_skill_file_is_regular(metadata: &std_fs::Metadata) -> bool {
    use std::os::windows::fs::MetadataExt as _;
    use windows::Win32::Storage::FileSystem::FILE_ATTRIBUTE_REPARSE_POINT;

    metadata.is_file() && metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT.0 == 0
}

#[cfg(not(windows))]
fn opened_skill_file_is_regular(metadata: &std_fs::Metadata) -> bool {
    metadata.is_file() && !metadata.file_type().is_symlink()
}

#[cfg(windows)]
fn review_error_to_io(error: ReviewRequestBuildError) -> std::io::Error {
    std::io::Error::other(error.to_string())
}

#[cfg(test)]
thread_local! {
    static SKILL_ROOT_VALIDATED_HOOK: std::cell::RefCell<Option<Box<dyn FnOnce()>>> = const {
        std::cell::RefCell::new(None)
    };
}

#[cfg(test)]
pub(super) fn install_skill_root_validated_hook(hook: impl FnOnce() + 'static) {
    SKILL_ROOT_VALIDATED_HOOK.with(|slot| {
        assert!(
            slot.borrow().is_none(),
            "a Skill root validation hook is already installed"
        );
        *slot.borrow_mut() = Some(Box::new(hook));
    });
}

#[cfg(test)]
fn run_skill_root_validated_hook() {
    SKILL_ROOT_VALIDATED_HOOK.with(|slot| {
        if let Some(hook) = slot.borrow_mut().take() {
            hook();
        }
    });
}

pub(super) fn package_path(
    root: &Path,
    relative_path: &str,
) -> Result<PathBuf, ReviewRequestBuildError> {
    let mut path = root.to_path_buf();
    for component in validated_skill_relative_components(relative_path)? {
        path.push(component);
    }
    Ok(path)
}

pub(super) fn navigation_path(
    origin: &SkillOrigin,
    relative_path: &str,
) -> Result<PathBuf, ReviewRequestBuildError> {
    let lexical = package_path(origin.lexical_root(), relative_path)?;
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt as _;

        // Keep the original user-facing spelling for normal paths, but use
        // the canonical extended-length path when the lexical spelling would
        // exceed Win32's traditional MAX_PATH limit.
        if lexical.as_os_str().encode_wide().count() >= 260 {
            return package_path(origin.canonical_root(), relative_path);
        }
    }
    Ok(lexical)
}

fn validated_skill_relative_components(
    relative_path: &str,
) -> Result<Vec<&str>, ReviewRequestBuildError> {
    let components = relative_path.split('/').collect::<Vec<_>>();
    if components.is_empty()
        || components
            .iter()
            .any(|component| component.is_empty() || matches!(*component, "." | ".."))
    {
        return Err(ReviewRequestBuildError::AgentSkillPathIsNotUtf8);
    }
    // Backslash is a legal Unix filename byte, but the package contract
    // normalizes separators to `/`; accepting it would reopen another path.
    #[cfg(unix)]
    if components.iter().any(|component| component.contains('\\')) {
        return Err(ReviewRequestBuildError::AgentSkillPathIsNotUtf8);
    }
    Ok(components)
}

fn normalized_skill_relative_path(
    root: &Path,
    path: &Path,
) -> Result<String, ReviewRequestBuildError> {
    let relative_path = path
        .strip_prefix(root)
        .map_err(|_| ReviewRequestBuildError::AgentSkillPathIsNotUtf8)?;
    let mut components = Vec::new();
    for component in relative_path.components() {
        let Component::Normal(component) = component else {
            return Err(ReviewRequestBuildError::AgentSkillPathIsNotUtf8);
        };
        components.push(
            component
                .to_str()
                .ok_or(ReviewRequestBuildError::AgentSkillPathIsNotUtf8)?,
        );
    }
    if components.is_empty() {
        return Err(ReviewRequestBuildError::AgentSkillPathIsNotUtf8);
    }
    let normalized = components.join("/");
    validated_skill_relative_components(&normalized)?;
    Ok(normalized)
}

fn normalized_skill_origin_path(
    origin: &SkillOrigin,
    path: &Path,
) -> Result<String, ReviewRequestBuildError> {
    normalized_skill_relative_path(origin.canonical_root(), path)
        .or_else(|_| normalized_skill_relative_path(origin.lexical_root(), path))
}

fn invalid_skill_path_error(error: ReviewRequestBuildError) -> std::io::Error {
    std::io::Error::new(std::io::ErrorKind::InvalidInput, error.to_string())
}

fn skill_editor_text_transform(bytes: &[u8]) -> Option<SkillEditorTextTransform> {
    let (body, strip_utf8_bom) = if bytes.starts_with(&[0xef, 0xbb, 0xbf]) {
        (&bytes[3..], true)
    } else {
        (bytes, false)
    };
    let text = std::str::from_utf8(body).ok()?;
    Some(SkillEditorTextTransform {
        strip_utf8_bom,
        normalize_crlf: document_io::Newline::detect(text) == document_io::Newline::Crlf,
    })
}
