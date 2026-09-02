//! Platform-owned directories for MarkTurbo runtime data.
//!
//! Settings are configuration and stay under `dirs::config_dir()` in
//! [`crate::settings`]. Browser profiles and logs are local runtime data: they
//! can grow, must not sit beside a packaged executable, and should not roam
//! between Windows machines with the user's profile.

use std::io;
#[cfg(any(not(debug_assertions), test))]
use std::path::Component;
use std::path::{Path, PathBuf};
#[cfg(any(not(debug_assertions), test))]
use std::sync::{
    OnceLock,
    atomic::{AtomicU64, Ordering},
};

#[cfg(any(not(debug_assertions), test))]
use sha2::{Digest, Sha256};

const APP_DIR: &str = "markturbo";

/// The shared per-user root for runtime data.
///
/// An absolute `$MARKTURBO_DATA_DIR` makes packaged/runtime probes hermetic. An
/// invalid nonblank override makes runtime data unavailable. Otherwise this is
/// `%LOCALAPPDATA%\markturbo`, `~/Library/Application Support/markturbo`, or
/// `$XDG_DATA_HOME/markturbo` (`~/.local/share/markturbo` by default).
pub fn data_dir() -> Option<PathBuf> {
    let value = match std::env::var("MARKTURBO_DATA_DIR") {
        Ok(value) => Some(value),
        Err(std::env::VarError::NotPresent) => None,
        Err(std::env::VarError::NotUnicode(_)) => return None,
    };

    match path_from_env_value(value.as_deref()).ok()? {
        Some(path) => Some(path),
        None => Some(dirs::data_local_dir()?.join(APP_DIR)),
    }
}

/// Persistent WebView profile shared by every MarkTurbo instance for this user.
pub fn webview_data_dir() -> Option<PathBuf> {
    Some(webview_data_dir_in(&data_dir()?))
}

/// Directory containing every process's log file.
pub fn log_dir() -> Option<PathBuf> {
    Some(data_dir()?.join("logs"))
}

/// Directory for optional encrypted dirty-buffer checkpoints.
///
/// Recovery data is application state, never a workspace file. Keeping it
/// below the same per-user root lets packaged launches and tests redirect it
/// with `$MARKTURBO_DATA_DIR` without widening the storage surface.
pub fn recovery_dir() -> Option<PathBuf> {
    Some(recovery_dir_in(&data_dir()?))
}

/// This process's log file inside the shared log directory.
///
/// One file per process avoids cross-process append and rotation races while
/// keeping all instances discoverable in one place.
pub fn log_path() -> Option<PathBuf> {
    Some(log_path_in(&log_dir()?, std::process::id()))
}

/// The sample workspace available from Welcome.
///
/// Release builds install the embedded workspace into app-owned local data on
/// first use. The replacement is a directory rename, so other processes never
/// observe a half-written sample. Debug builds and tests use the repository
/// fixture directly to keep normal developer startup free of writes.
pub fn bundled_sample_available() -> bool {
    #[cfg(any(debug_assertions, test))]
    {
        source_sample_dir().is_dir()
    }

    #[cfg(not(any(debug_assertions, test)))]
    {
        release_sample_is_available()
    }
}

/// Materialize the sample workspace when the user opens it from Welcome.
pub fn bundled_sample_dir() -> io::Result<PathBuf> {
    #[cfg(any(debug_assertions, test))]
    {
        let source_sample = source_sample_dir();
        if source_sample.is_dir() {
            return Ok(source_sample);
        }
        Err(io::Error::new(
            io::ErrorKind::NotFound,
            "repository sample fixture is unavailable",
        ))
    }

    #[cfg(not(any(debug_assertions, test)))]
    {
        let data_root = data_dir().ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::NotFound,
                "application data directory is unavailable",
            )
        })?;
        bundled_sample_dir_in(&data_root)
    }
}

#[cfg(any(debug_assertions, test))]
fn source_sample_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../sample")
}

/// Do not touch runtime data here: a bad data root must remain clickable so the
/// open action can report its error instead of silently disabling the sample.
#[cfg(any(not(debug_assertions), test))]
fn release_sample_is_available() -> bool {
    crate::assets::embedded_sample_files().next().is_some()
}

/// Materialize the release sample below an explicit application data root.
///
/// Keeping the release path separate from `bundled_sample_dir` makes its
/// single-file distribution behavior testable without changing process-wide
/// environment variables or consulting the development fixture.
#[cfg(any(not(debug_assertions), test))]
fn bundled_sample_dir_in(data_root: &Path) -> io::Result<PathBuf> {
    let destination = embedded_sample_dir_in(data_root);
    materialize_embedded_sample(&destination)?;
    Ok(destination)
}

#[cfg(any(not(debug_assertions), test))]
fn embedded_sample_dir_in(root: &Path) -> PathBuf {
    root.join("sample").join(embedded_sample_version())
}

#[cfg(any(not(debug_assertions), test))]
fn embedded_sample_version() -> &'static str {
    static VERSION: OnceLock<String> = OnceLock::new();
    VERSION.get_or_init(|| {
        let mut files: Vec<_> = crate::assets::embedded_sample_files().collect();
        files.sort_by(|left, right| left.0.cmp(&right.0));

        let mut digest = Sha256::new();
        for (path, contents) in files {
            digest.update(path.as_bytes());
            digest.update([0]);
            digest.update(contents.as_ref());
            digest.update([0]);
        }
        digest.finalize()[..12]
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect()
    })
}

#[cfg(any(not(debug_assertions), test))]
fn materialize_embedded_sample(destination: &Path) -> io::Result<()> {
    if destination.is_dir() {
        return Ok(());
    }

    let parent = destination.parent().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "sample destination must have a parent directory",
        )
    })?;
    std::fs::create_dir_all(parent)?;
    let staging = create_sample_staging_dir(parent)?;
    if let Err(error) = write_embedded_sample(&staging) {
        let _ = std::fs::remove_dir_all(&staging);
        return Err(error);
    }

    match std::fs::rename(&staging, destination) {
        Ok(()) => Ok(()),
        // Another instance may have completed its identical materialization
        // between our first existence check and the rename.
        Err(_) if destination.is_dir() => {
            let _ = std::fs::remove_dir_all(staging);
            Ok(())
        }
        Err(error) => {
            let _ = std::fs::remove_dir_all(staging);
            Err(error)
        }
    }
}

#[cfg(any(not(debug_assertions), test))]
fn create_sample_staging_dir(parent: &Path) -> io::Result<PathBuf> {
    static NEXT_SAMPLE_INSTALL: AtomicU64 = AtomicU64::new(0);

    for _ in 0..16 {
        let sequence = NEXT_SAMPLE_INSTALL.fetch_add(1, Ordering::Relaxed);
        let staging = parent.join(format!(".sample-{}-{sequence}", std::process::id()));
        match std::fs::create_dir(&staging) {
            Ok(()) => return Ok(staging),
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error),
        }
    }
    Err(io::Error::new(
        io::ErrorKind::AlreadyExists,
        "could not create a unique sample staging directory",
    ))
}

#[cfg(any(not(debug_assertions), test))]
fn write_embedded_sample(destination: &Path) -> io::Result<()> {
    let mut files: Vec<_> = crate::assets::embedded_sample_files().collect();
    files.sort_by(|left, right| left.0.cmp(&right.0));

    for (relative, contents) in files {
        let relative_path = Path::new(relative.as_ref());
        if !is_safe_embedded_relative_path(relative_path) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("embedded sample contains unsafe path: {relative}"),
            ));
        }
        let output = destination.join(relative_path);
        let output_parent = output.parent().expect("relative file paths have a parent");
        std::fs::create_dir_all(output_parent)?;
        std::fs::write(output, contents)?;
    }
    Ok(())
}

#[cfg(any(not(debug_assertions), test))]
fn is_safe_embedded_relative_path(path: &Path) -> bool {
    !path.as_os_str().is_empty()
        && path
            .components()
            .all(|component| matches!(component, Component::Normal(_)))
}

fn webview_data_dir_in(root: &Path) -> PathBuf {
    root.join("webview2")
}

fn recovery_dir_in(root: &Path) -> PathBuf {
    root.join("recovery")
}

fn log_path_in(log_dir: &Path, process_id: u32) -> PathBuf {
    log_dir.join(format!("markturbo-{process_id}.log"))
}

fn path_from_env_value(value: Option<&str>) -> Result<Option<PathBuf>, ()> {
    let Some(trimmed) = value.map(str::trim).filter(|value| !value.is_empty()) else {
        return Ok(None);
    };
    let path = PathBuf::from(trimmed);
    path.is_absolute().then_some(Some(path)).ok_or(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn release_sample_materializes_into_an_isolated_data_root_without_overwriting_edits() {
        let data_root = tempfile::tempdir().unwrap();
        let sample = bundled_sample_dir_in(data_root.path()).expect("sample must materialize");
        assert_eq!(sample, embedded_sample_dir_in(data_root.path()));
        let readme = sample.join("README.md");
        for (relative, contents) in crate::assets::embedded_sample_files() {
            assert_eq!(
                std::fs::read(sample.join(relative.as_ref()))
                    .unwrap()
                    .as_slice(),
                contents.as_ref(),
                "embedded file {relative} must materialize exactly"
            );
        }

        std::fs::write(&readme, "my local notes").unwrap();
        assert_eq!(
            bundled_sample_dir_in(data_root.path()).unwrap(),
            sample.clone()
        );
        assert_eq!(std::fs::read_to_string(readme).unwrap(), "my local notes");
    }

    #[test]
    fn release_sample_availability_does_not_materialize_or_write_to_its_data_root() {
        let temporary = tempfile::tempdir().unwrap();
        let data_root = temporary.path().join("new-data-root");

        assert!(release_sample_is_available());
        assert!(!data_root.exists());
        assert!(!embedded_sample_dir_in(&data_root).exists());
    }

    #[test]
    fn release_sample_materialization_preserves_io_failure() {
        let temporary = tempfile::tempdir().unwrap();
        let data_root = temporary.path().join("not-a-directory");
        std::fs::write(&data_root, "file").unwrap();

        assert!(bundled_sample_dir_in(&data_root).is_err());
    }

    #[test]
    fn embedded_sample_paths_cannot_escape_the_destination() {
        for path in [
            "README.md",
            ".claude/skills/example/SKILL.md",
            "docs/diagrams.md",
        ] {
            assert!(is_safe_embedded_relative_path(Path::new(path)), "{path}");
        }
        for path in ["", "../README.md", "docs/../README.md", "/README.md"] {
            assert!(
                !is_safe_embedded_relative_path(Path::new(path)),
                "{path} must be rejected"
            );
        }
        #[cfg(windows)]
        assert!(
            !is_safe_embedded_relative_path(Path::new(r"C:\README.md")),
            "a drive-qualified path must be rejected"
        );
    }

    #[test]
    fn embedded_sample_uses_a_content_versioned_app_owned_directory() {
        let root = Path::new("app-data");
        let version = embedded_sample_version();

        assert_eq!(version.len(), 24);
        assert!(version.bytes().all(|byte| byte.is_ascii_hexdigit()));
        assert!(std::ptr::eq(version, embedded_sample_version()));
        assert_eq!(
            embedded_sample_dir_in(root),
            root.join("sample").join(version)
        );
    }

    #[test]
    fn runtime_paths_share_one_user_data_root() {
        let root = Path::new("user-data");

        assert_eq!(webview_data_dir_in(root), root.join("webview2"));
        assert_eq!(recovery_dir_in(root), root.join("recovery"));
        assert_eq!(
            log_path_in(&root.join("logs"), 42),
            root.join("logs/markturbo-42.log")
        );
    }

    #[test]
    fn absent_or_blank_data_override_uses_platform_default() {
        // Exercise the parser without mutating the process-wide variable used by
        // real launches and parallel tests.
        for value in [None, Some(""), Some("   "), Some("\t\r\n")] {
            assert_eq!(path_from_env_value(value), Ok(None), "value: {value:?}");
        }
    }

    #[test]
    fn absolute_data_override_is_accepted() {
        let path = std::env::temp_dir().join("markturbo-data");
        let value = format!(" {} ", path.display());

        assert!(path.is_absolute());
        assert_eq!(path_from_env_value(Some(&value)), Ok(Some(path)));
    }

    #[cfg(windows)]
    #[test]
    fn windows_data_override_must_be_fully_qualified() {
        let cases = [
            (r"data\root", Err(())),
            (r"C:data", Err(())),
            (r"\data", Err(())),
            (r"C:\data", Ok(Some(PathBuf::from(r"C:\data")))),
            (
                r"\\server\share\data",
                Ok(Some(PathBuf::from(r"\\server\share\data"))),
            ),
        ];

        for (value, expected) in cases {
            assert_eq!(
                path_from_env_value(Some(value)),
                expected,
                "value: {value:?}"
            );
        }
    }
}
