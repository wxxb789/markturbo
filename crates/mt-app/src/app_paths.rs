//! Platform-owned directories for MarkTurbo runtime data.
//!
//! Settings are configuration and stay under `dirs::config_dir()` in
//! [`crate::settings`]. Browser profiles and logs are local runtime data: they
//! can grow, must not sit beside a packaged executable, and should not roam
//! between Windows machines with the user's profile.

use std::path::{Path, PathBuf};

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
