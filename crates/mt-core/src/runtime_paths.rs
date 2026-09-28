//! Platform-owned directories for MarkTurbo runtime data.
//!
//! Settings are configuration and stay under `dirs::config_dir()` in
//! [`crate::settings`]. Browser profiles and logs are local runtime data: they
//! can grow, must not sit beside a packaged executable, and should not roam
//! between Windows machines with the user's profile.

use std::borrow::Cow;
use std::io;
use std::path::{Component, Path, PathBuf};
use std::sync::{
    OnceLock,
    atomic::{AtomicU64, Ordering},
};

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

/// Installs a content-versioned embedded sample workspace below app-owned data.
///
/// The version is computed once per installer from sorted relative paths and
/// contents. Files are written to a unique sibling staging directory and made
/// visible with one directory rename, so another process cannot observe a
/// partially materialized sample. Existing sample directories are never
/// rewritten, preserving user edits.
///
/// Reuse an installer only with the same immutable embedded file set; its
/// version digest is intentionally cached for the installer's lifetime.
pub struct EmbeddedSampleInstaller {
    version: OnceLock<String>,
}

impl Default for EmbeddedSampleInstaller {
    fn default() -> Self {
        Self::new()
    }
}

impl EmbeddedSampleInstaller {
    /// Creates an installer whose embedded content version is computed lazily.
    pub const fn new() -> Self {
        Self {
            version: OnceLock::new(),
        }
    }

    /// Materializes the sample below `data_root` and returns its versioned path.
    ///
    /// `embedded_files` is called once to compute the cached version and once
    /// whenever a new destination must be written. Each item is a relative path
    /// and its embedded bytes, both owned or borrowed with static lifetime so
    /// sorting does not copy their contents.
    pub fn materialize_in<F, I>(&self, data_root: &Path, embedded_files: F) -> io::Result<PathBuf>
    where
        F: Fn() -> I,
        I: IntoIterator<Item = (Cow<'static, str>, Cow<'static, [u8]>)>,
    {
        let version = self.version(&embedded_files);
        let destination = data_root.join("sample").join(version);
        materialize_embedded_sample(&destination, embedded_files)?;
        Ok(destination)
    }

    fn version<F, I>(&self, embedded_files: &F) -> &str
    where
        F: Fn() -> I,
        I: IntoIterator<Item = (Cow<'static, str>, Cow<'static, [u8]>)>,
    {
        self.version.get_or_init(|| {
            let mut files: Vec<_> = embedded_files().into_iter().collect();
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
}

fn materialize_embedded_sample<F, I>(destination: &Path, embedded_files: F) -> io::Result<()>
where
    F: Fn() -> I,
    I: IntoIterator<Item = (Cow<'static, str>, Cow<'static, [u8]>)>,
{
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
    if let Err(error) = write_embedded_sample(&staging, embedded_files()) {
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

fn write_embedded_sample(
    destination: &Path,
    files: impl IntoIterator<Item = (Cow<'static, str>, Cow<'static, [u8]>)>,
) -> io::Result<()> {
    let mut files: Vec<_> = files.into_iter().collect();
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

    fn sample_files() -> impl Iterator<Item = (Cow<'static, str>, Cow<'static, [u8]>)> {
        [
            ("README.md", &b"# Embedded sample\n"[..]),
            ("docs/guide.md", &b"sample guide\n"[..]),
            (".claude/skills/example/SKILL.md", &b"skill body\n"[..]),
        ]
        .into_iter()
        .map(|(path, contents)| (Cow::Borrowed(path), Cow::Borrowed(contents)))
    }

    fn reversed_sample_files() -> impl Iterator<Item = (Cow<'static, str>, Cow<'static, [u8]>)> {
        [
            (".claude/skills/example/SKILL.md", &b"skill body\n"[..]),
            ("docs/guide.md", &b"sample guide\n"[..]),
            ("README.md", &b"# Embedded sample\n"[..]),
        ]
        .into_iter()
        .map(|(path, contents)| (Cow::Borrowed(path), Cow::Borrowed(contents)))
    }

    #[test]
    fn sample_materializes_content_versioned_files_and_preserves_user_edits() {
        let data_root = tempfile::tempdir().unwrap();
        let installer = EmbeddedSampleInstaller::new();
        let sample = installer
            .materialize_in(data_root.path(), sample_files)
            .expect("sample must materialize");

        assert_eq!(sample.parent().unwrap(), data_root.path().join("sample"));
        let version = sample.file_name().unwrap().to_str().unwrap();
        assert_eq!(version.len(), 24);
        assert!(version.bytes().all(|byte| byte.is_ascii_hexdigit()));
        assert!(std::ptr::eq(
            installer.version(&sample_files),
            installer.version(&sample_files)
        ));
        for (relative, contents) in sample_files() {
            assert_eq!(
                std::fs::read(sample.join(relative.as_ref()))
                    .unwrap()
                    .as_slice(),
                contents.as_ref(),
                "embedded file {relative} must materialize exactly"
            );
        }

        let readme = sample.join("README.md");
        std::fs::write(&readme, "my local notes").unwrap();
        assert_eq!(
            installer
                .materialize_in(data_root.path(), sample_files)
                .unwrap(),
            sample
        );
        assert_eq!(std::fs::read_to_string(readme).unwrap(), "my local notes");
    }

    #[test]
    fn sample_version_is_independent_of_input_order_and_sensitive_to_content() {
        let installer = EmbeddedSampleInstaller::new();
        let reordered_installer = EmbeddedSampleInstaller::new();
        let changed_installer = EmbeddedSampleInstaller::new();
        let original = installer.version(&sample_files);
        let reordered = reordered_installer.version(&reversed_sample_files);
        let changed_files = || {
            [
                ("README.md", &b"different bytes\n"[..]),
                ("docs/guide.md", &b"sample guide\n"[..]),
                (".claude/skills/example/SKILL.md", &b"skill body\n"[..]),
            ]
            .into_iter()
            .map(|(path, contents)| (Cow::Borrowed(path), Cow::Borrowed(contents)))
        };
        let changed = changed_installer.version(&changed_files);

        assert_eq!(original, reordered);
        assert_ne!(original, changed);
    }

    #[test]
    fn simultaneous_sample_installs_publish_complete_directories() {
        let temporary = tempfile::tempdir().unwrap();
        let data_root = temporary.path().to_owned();
        let barrier = std::sync::Arc::new(std::sync::Barrier::new(4));
        let workers: Vec<_> = (0..4)
            .map(|_| {
                let data_root = data_root.clone();
                let barrier = barrier.clone();
                std::thread::spawn(move || {
                    let installer = EmbeddedSampleInstaller::new();
                    barrier.wait();
                    installer.materialize_in(&data_root, sample_files)
                })
            })
            .collect();

        let samples: Vec<_> = workers
            .into_iter()
            .map(|worker| worker.join().unwrap().unwrap())
            .collect();
        assert!(samples.iter().all(|sample| sample == &samples[0]));
        for (relative, contents) in sample_files() {
            assert_eq!(
                std::fs::read(samples[0].join(relative.as_ref()))
                    .unwrap()
                    .as_slice(),
                contents.as_ref()
            );
        }
        assert!(
            std::fs::read_dir(samples[0].parent().unwrap())
                .unwrap()
                .all(|entry| !entry
                    .unwrap()
                    .file_name()
                    .to_string_lossy()
                    .starts_with(".sample-"))
        );
    }

    #[test]
    fn sample_materialization_preserves_data_root_io_failure() {
        let temporary = tempfile::tempdir().unwrap();
        let data_root = temporary.path().join("not-a-directory");
        std::fs::write(&data_root, "file").unwrap();

        assert!(
            EmbeddedSampleInstaller::new()
                .materialize_in(&data_root, sample_files)
                .is_err()
        );
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
