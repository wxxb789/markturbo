//! The credential store of every test build.
//!
//! A unit test (`cfg(test)`) or an integration test (which turns on the
//! `test-vault` feature through the crate's dev-dependency on itself) gets
//! this in-memory store from [`AppState::new`](crate::state::AppState::new)
//! instead of the platform's secure store: no test reads, writes or deletes a
//! credential of the person running it. A release build compiles none of it.
//!
//! The store is one per process, as the platform's is: every state a test
//! binary builds sees the keys another stored.

use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::{
        Arc, LazyLock, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

use mt_core::credentials::{CredentialError, CredentialVault, Secret, SecureCredentialStore};

/// A secure store that keeps its secrets in this process's memory.
#[derive(Default)]
struct MemoryStore {
    values: Mutex<HashMap<String, Secret>>,
}

impl MemoryStore {
    fn values(&self) -> std::sync::MutexGuard<'_, HashMap<String, Secret>> {
        self.values
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

impl SecureCredentialStore for MemoryStore {
    fn is_supported(&self) -> bool {
        true
    }

    fn read(&self, target: &str) -> Result<Option<Secret>, CredentialError> {
        Ok(self.values().get(target).cloned())
    }

    fn write(&self, target: &str, secret: &Secret) -> Result<(), CredentialError> {
        self.values().insert(target.to_owned(), secret.clone());
        Ok(())
    }

    fn delete(&self, target: &str) -> Result<(), CredentialError> {
        self.values().remove(target);
        Ok(())
    }
}

static STORE: LazyLock<Arc<MemoryStore>> = LazyLock::new(Arc::default);

/// The vault [`AppState::new`](crate::state::AppState::new) uses in a test
/// build.
pub(crate) fn vault() -> CredentialVault {
    CredentialVault::with_test_store(STORE.clone())
}

/// The secret this process's store holds for `target`, for a test's own
/// assertions.
pub fn stored(target: &str) -> Option<String> {
    STORE
        .values()
        .get(target)
        .map(|secret| secret.expose().to_owned())
}

struct PathGate {
    path: PathBuf,
    releases: Vec<PathBuf>,
    next: usize,
}

static PATH_GATES: LazyLock<Mutex<Vec<PathGate>>> = LazyLock::new(|| Mutex::new(Vec::new()));

/// Hold every blocking read of `path` until `release` exists, so a test can
/// order a scan or a load against the work the UI thread does meanwhile.
pub fn gate_read(path: impl Into<PathBuf>, release: impl Into<PathBuf>) {
    gate_reads(path, [release])
}

/// Hold the Nth blocking read of `path` until the Nth of `releases` exists,
/// so a test can order two reads of one file; reads past the list wait for
/// its last path, as [`gate_read`] holds every read.
pub fn gate_reads(
    path: impl Into<PathBuf>,
    releases: impl IntoIterator<Item = impl Into<PathBuf>>,
) {
    PATH_GATES
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .push(PathGate {
            path: path.into(),
            releases: releases.into_iter().map(Into::into).collect(),
            next: 0,
        });
}

/// How many blocking reads of `path` have taken a release slot: each of them
/// has reached its gate.
pub fn gate_reads_started(path: &Path) -> usize {
    PATH_GATES
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .iter()
        .find(|gate| gate.path == path)
        .map_or(0, |gate| gate.next)
}

static SAVE_GATE_REACHED: AtomicBool = AtomicBool::new(false);

/// Whether the Save As test gate has been reached: the save is waiting for
/// its answer and has not read one yet ([`wait_for_save_gate`]).
pub fn save_gate_reached() -> bool {
    SAVE_GATE_REACHED.load(Ordering::Acquire)
}

/// Hold the next Save As after its picker was asked and before its answer is
/// read, until the path named by `MARKTURBO_TEST_SAVE_GATE` exists, so a test
/// can dirty another tab while the save is pending.
pub(crate) fn wait_for_save_gate() {
    let Some(release) = std::env::var_os("MARKTURBO_TEST_SAVE_GATE") else {
        return;
    };
    SAVE_GATE_REACHED.store(true, Ordering::Release);
    let release = PathBuf::from(release);
    while !release.exists() {
        std::thread::sleep(Duration::from_millis(5));
    }
}

static SUPERSEDED_OPENS: LazyLock<Mutex<Vec<PathBuf>>> = LazyLock::new(|| Mutex::new(Vec::new()));

/// Whether the folder open for `path` returned after a newer request took
/// its slot. A superseded open writes nothing, so only this tells a test
/// that the stale scan actually came back.
pub fn open_superseded(path: &Path) -> bool {
    SUPERSEDED_OPENS
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .iter()
        .any(|seen| seen == path)
}

pub(crate) fn note_superseded_open(path: &Path) {
    SUPERSEDED_OPENS
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .push(path.to_path_buf());
}

static SUPERSEDED_RELOADS: LazyLock<Mutex<Vec<PathBuf>>> = LazyLock::new(|| Mutex::new(Vec::new()));

/// Whether a reload of `path` was dropped after a newer request took its
/// slot. A superseded answer writes nothing, so only this tells a test that
/// the stale answer was handled.
pub fn reload_superseded(path: &Path) -> bool {
    SUPERSEDED_RELOADS
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .iter()
        .any(|seen| seen == path)
}

pub(crate) fn note_superseded_reload(path: &Path) {
    SUPERSEDED_RELOADS
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .push(path.to_path_buf());
}

/// Wait on this path's gate, if a test set one. Runs inside the blocking
/// read's thread, so the UI thread never waits.
pub(crate) fn wait_for_read(path: &Path) {
    let release = {
        let mut gates = PATH_GATES
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let Some(gate) = gates.iter_mut().find(|gate| gate.path == path) else {
            return;
        };
        let release = gate
            .releases
            .get(gate.next)
            .or_else(|| gate.releases.last())
            .cloned();
        gate.next += 1;
        release
    };
    if let Some(release) = release {
        while !release.exists() {
            std::thread::sleep(Duration::from_millis(5));
        }
    }
}
