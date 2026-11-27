//! Counters and gates a test reads to prove that UI-thread paths resolve
//! nothing and wait for no file system.
//!
//! A `canonicalize` or a watch registration on a slow or disconnected volume
//! blocks its caller, so the live-disk paths must reach one only off the UI
//! thread. The counters and gates are process-wide and present only in test
//! builds.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{LazyLock, Mutex};
use std::time::Duration;

static PATH_RESOLUTIONS: AtomicUsize = AtomicUsize::new(0);

/// Records one path resolution (`canonicalize`) attempt.
pub fn note_path_resolution() {
    PATH_RESOLUTIONS.fetch_add(1, Ordering::Relaxed);
}

/// How many path resolution attempts were recorded.
pub fn path_resolutions() -> usize {
    PATH_RESOLUTIONS.load(Ordering::Relaxed)
}

struct RegistrationGate {
    path: PathBuf,
    release: PathBuf,
    reached: bool,
}

static REGISTRATION_GATES: LazyLock<Mutex<Vec<RegistrationGate>>> =
    LazyLock::new(|| Mutex::new(Vec::new()));

/// Hold the next filesystem-watch registration or removal of `path` until
/// `release` exists, so a test can prove the UI thread never waits for one.
pub fn gate_watch_registration(path: impl Into<PathBuf>, release: impl Into<PathBuf>) {
    REGISTRATION_GATES
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .push(RegistrationGate {
            path: crate::workspace::normalize_path(&path.into()),
            release: release.into(),
            reached: false,
        });
}

/// Whether the gated watch operation for `path` has been reached.
pub fn watch_registration_reached(path: &Path) -> bool {
    let path = crate::workspace::normalize_path(path);
    REGISTRATION_GATES
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .iter()
        .any(|gate| crate::workspace::paths_match(&gate.path, &path) && gate.reached)
}

// Runs on the watcher's worker thread; the UI thread never waits.
pub(crate) fn wait_for_watch_registration(path: &Path) {
    let path = crate::workspace::normalize_path(path);
    let release = {
        let mut gates = REGISTRATION_GATES
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let Some(gate) = gates
            .iter_mut()
            .find(|gate| !gate.reached && crate::workspace::paths_match(&gate.path, &path))
        else {
            return;
        };
        gate.reached = true;
        gate.release.clone()
    };
    while !release.exists() {
        std::thread::sleep(Duration::from_millis(5));
    }
}

static READ_ERRORS: LazyLock<Mutex<Vec<PathBuf>>> = LazyLock::new(|| Mutex::new(Vec::new()));

/// Make the next dropped-document read of `path` fail before the file is
/// opened, so a test can order a failure against the notice that owns it.
pub fn fail_next_read(path: impl Into<PathBuf>) {
    READ_ERRORS
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .push(path.into());
}

/// The error one queued [`fail_next_read`] asks this path's read to report,
/// consumed on the first read to ask.
pub fn injected_read_error(path: &Path) -> Option<std::io::Error> {
    let mut errors = READ_ERRORS
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let index = errors.iter().position(|failing| failing == path)?;
    errors.swap_remove(index);
    Some(std::io::Error::new(
        std::io::ErrorKind::PermissionDenied,
        "injected read failure",
    ))
}
