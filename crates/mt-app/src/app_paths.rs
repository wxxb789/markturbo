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
/// `$MARKTURBO_DATA_DIR` makes packaged/runtime probes hermetic. Otherwise this
/// is `%LOCALAPPDATA%\markturbo`, `~/Library/Application Support/markturbo`, or
/// `$XDG_DATA_HOME/markturbo` (`~/.local/share/markturbo` by default).
pub fn data_dir() -> Option<PathBuf> {
    env_path("MARKTURBO_DATA_DIR").or_else(|| Some(dirs::data_local_dir()?.join(APP_DIR)))
}

/// Persistent WebView profile shared by every MarkTurbo instance for this user.
pub fn webview_data_dir() -> Option<PathBuf> {
    Some(webview_data_dir_in(&data_dir()?))
}

/// Directory containing every process's log file.
pub fn log_dir() -> Option<PathBuf> {
    Some(data_dir()?.join("logs"))
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

fn log_path_in(log_dir: &Path, process_id: u32) -> PathBuf {
    log_dir.join(format!("markturbo-{process_id}.log"))
}

fn env_path(var: &str) -> Option<PathBuf> {
    let value = std::env::var(var).ok()?;
    path_from_env_value(&value)
}

fn path_from_env_value(value: &str) -> Option<PathBuf> {
    let trimmed = value.trim();
    (!trimmed.is_empty()).then(|| PathBuf::from(trimmed))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn runtime_paths_share_one_user_data_root() {
        let root = Path::new("user-data");

        assert_eq!(webview_data_dir_in(root), root.join("webview2"));
        assert_eq!(
            log_path_in(&root.join("logs"), 42),
            root.join("logs/markturbo-42.log")
        );
    }

    #[test]
    fn blank_data_override_is_ignored() {
        // Exercise the parser without mutating the process-wide variable used by
        // real launches and parallel tests.
        assert_eq!(path_from_env_value("   "), None);
        assert_eq!(
            path_from_env_value(" data/root "),
            Some(PathBuf::from("data/root"))
        );
    }
}
