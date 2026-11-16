//! The unit-test binary's process environment.
//!
//! Every `AppState` reads its settings, recovery records and usage counters
//! from `$MARKTURBO_CONFIG_DIR` and `$MARKTURBO_DATA_DIR`, which are
//! process-global, while the tests run in parallel threads. So the binary
//! sets them once, to one temporary directory that lives as long as the
//! process, before the first state is built, and no test changes them after:
//! no test reads another's (possibly deleted) directory, and none reads or
//! writes the person's own settings.
//!
//! A test that must point another process-global variable somewhere of its
//! own holds [`lock`] for as long as its probe reads it.

use std::sync::{Mutex, MutexGuard, OnceLock};

/// Points the app's config and data directories at this process's own
/// temporary directory, once.
pub(crate) fn hermetic() {
    static ROOT: OnceLock<tempfile::TempDir> = OnceLock::new();
    ROOT.get_or_init(|| {
        let root = tempfile::tempdir().expect("the test binary's config dir");
        // SAFETY: set once, under the `OnceLock`, before any state reads the
        // variables; std's environment lock orders it with every reader.
        unsafe {
            std::env::set_var("MARKTURBO_CONFIG_DIR", root.path());
            std::env::set_var("MARKTURBO_DATA_DIR", root.path().join("data"));
        }
        root
    });
}

/// Serializes the tests that set any other process-global variable.
pub(crate) fn lock() -> MutexGuard<'static, ()> {
    static LOCK: Mutex<()> = Mutex::new(());
    LOCK.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}
