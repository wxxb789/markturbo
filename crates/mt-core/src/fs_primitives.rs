//! No-follow filesystem primitives shared by the document, Agent Skill and
//! recovery paths.
//!
//! These are the OS spellings — flags, descriptor-relative opens, and the
//! predicates on metadata read through an already-open handle — not a policy.
//! Each caller still chooses its share mode, its identity checks and its error
//! surface. `O_NOFOLLOW` on Unix and `FILE_FLAG_OPEN_REPARSE_POINT` on Windows
//! are what make an opened handle authoritative over the path it came from,
//! which is why the same spellings had grown copies in five files.

use std::fs::{File, Metadata};
use std::io;
#[cfg(windows)]
use std::path::Path;

/// Linux and Darwin `<fcntl.h>` `O_NOFOLLOW`.
#[cfg(all(unix, any(target_os = "linux", target_os = "macos")))]
pub(crate) const O_NOFOLLOW: i32 = if cfg!(target_os = "linux") {
    0o400000
} else {
    0x100
};

/// Linux and Darwin `<fcntl.h>` `O_DIRECTORY`.
#[cfg(all(unix, any(target_os = "linux", target_os = "macos")))]
pub(crate) const O_DIRECTORY: i32 = if cfg!(target_os = "linux") {
    0o200000
} else {
    0x100000
};

/// Linux and Darwin `<fcntl.h>` `O_CLOEXEC`.
#[cfg(all(unix, any(target_os = "linux", target_os = "macos")))]
pub(crate) const O_CLOEXEC: i32 = if cfg!(target_os = "linux") {
    0o2000000
} else {
    0x1000000
};

/// Linux and Darwin `<fcntl.h>` `O_NONBLOCK`.
///
/// Kept here because opening a path that a race replaced with a FIFO must
/// return at once instead of waiting for a writer.
#[cfg(all(unix, any(target_os = "linux", target_os = "macos")))]
pub(crate) const O_NONBLOCK: i32 = if cfg!(target_os = "linux") {
    0o4000
} else {
    0x4
};

/// `dirent.d_type` values for a directory and a regular file.
#[cfg(all(unix, any(target_os = "linux", target_os = "macos")))]
pub(crate) const DT_DIRECTORY: u8 = 4;
#[cfg(all(unix, any(target_os = "linux", target_os = "macos")))]
pub(crate) const DT_REGULAR: u8 = 8;

/// Open one path component below an owned directory descriptor.
///
/// The returned file is authoritative for the component that was opened:
/// callers pass `O_NOFOLLOW` when the final component must not be a link, and
/// the descriptor is what later identity checks compare against.
#[cfg(all(unix, any(target_os = "linux", target_os = "macos")))]
pub(crate) fn open_at(directory: &File, name: &std::ffi::OsStr, flags: i32) -> io::Result<File> {
    use std::ffi::{CString, c_char, c_int};
    use std::os::fd::{AsRawFd as _, FromRawFd as _};
    use std::os::unix::ffi::OsStrExt as _;

    unsafe extern "C" {
        fn openat(directory: c_int, path: *const c_char, flags: c_int) -> c_int;
    }

    let name = CString::new(name.as_bytes())
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "NUL in entry name"))?;
    // SAFETY: `directory` owns a live descriptor and `name` is one
    // NUL-terminated path component.
    let descriptor = unsafe { openat(directory.as_raw_fd(), name.as_ptr(), flags) };
    if descriptor < 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: `openat` returned a new owned descriptor.
    Ok(unsafe { File::from_raw_fd(descriptor) })
}

/// Whether one component below `directory` is a symbolic link.
///
/// Called after an `open_at` without `O_NOFOLLOW` failed, to tell a link that
/// was refused from a path that is simply absent. `readlinkat` never follows
/// the final component, and a one-byte probe is enough.
#[cfg(all(unix, any(target_os = "linux", target_os = "macos")))]
pub(crate) fn name_is_symlink_at(directory: &File, name: &std::ffi::OsStr) -> bool {
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
    // SAFETY: `directory` owns a live descriptor, the name is
    // NUL-terminated, and `probe` supplies one writable byte.
    unsafe {
        readlinkat(
            directory.as_raw_fd(),
            name.as_ptr(),
            (&raw mut probe).cast::<c_void>().cast::<c_char>(),
            1,
        ) >= 0
    }
}

/// Open a directory without following a final reparse point, with `share_mode`
/// as the caller's denial of rename or delete operations.
#[cfg(windows)]
pub(crate) fn open_directory_no_follow(path: &Path, share_mode: u32) -> io::Result<File> {
    use std::os::windows::fs::OpenOptionsExt as _;
    use windows::Win32::Storage::FileSystem::{
        FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT,
    };

    std::fs::OpenOptions::new()
        .read(true)
        .share_mode(share_mode)
        .custom_flags((FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT).0)
        .open(path)
}

/// Open a file entry without following a final reparse point.
#[cfg(windows)]
pub(crate) fn open_entry_no_follow(path: &Path, share_mode: u32) -> io::Result<File> {
    use std::os::windows::fs::OpenOptionsExt as _;
    use windows::Win32::Storage::FileSystem::FILE_FLAG_OPEN_REPARSE_POINT;

    std::fs::OpenOptions::new()
        .read(true)
        .share_mode(share_mode)
        .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT.0)
        .open(path)
}

/// True when opened metadata names a reparse point (Windows) or a symbolic
/// link (Unix). Reading it through the handle is what binds the answer to the
/// object the caller actually has open.
pub(crate) fn opened_metadata_is_reparse(metadata: &Metadata) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt as _;
        use windows::Win32::Storage::FileSystem::FILE_ATTRIBUTE_REPARSE_POINT;

        metadata.file_type().is_symlink()
            || metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT.0 != 0
    }
    #[cfg(not(windows))]
    {
        metadata.file_type().is_symlink()
    }
}

/// True when opened metadata is a regular file and not a reparse point.
///
/// The Unix spelling is stricter than a bare `is_file`: the `O_NOFOLLOW` open
/// already refuses a link, and the extra check keeps a caller that opened
/// without it from accepting one.
pub(crate) fn opened_metadata_is_regular_file(metadata: &Metadata) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt as _;
        use windows::Win32::Storage::FileSystem::FILE_ATTRIBUTE_REPARSE_POINT;

        metadata.is_file() && metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT.0 == 0
    }
    #[cfg(not(windows))]
    {
        metadata.is_file() && !metadata.file_type().is_symlink()
    }
}

/// True when opened metadata is a directory and not a reparse point.
///
/// Windows-only: the directory opens that consult it are the reparse-guarded
/// ones, and Unix reaches directories through `O_DIRECTORY` instead.
#[cfg(windows)]
pub(crate) fn opened_metadata_is_regular_directory(metadata: &Metadata) -> bool {
    use std::os::windows::fs::MetadataExt as _;
    use windows::Win32::Storage::FileSystem::FILE_ATTRIBUTE_REPARSE_POINT;

    metadata.is_dir() && metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT.0 == 0
}
