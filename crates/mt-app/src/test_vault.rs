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
    sync::{Arc, LazyLock, Mutex},
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
    CredentialVault::with_store(STORE.clone())
}

/// The secret this process's store holds for `target`, for a test's own
/// assertions.
pub fn stored(target: &str) -> Option<String> {
    STORE
        .values()
        .get(target)
        .map(|secret| secret.expose().to_owned())
}
