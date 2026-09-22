//! Model credential storage and resolution.
//!
//! Persistent secrets live in the platform secure store. This module never
//! serializes them, places them in process arguments, or includes them in
//! diagnostics. Session credentials are held only for the life of the process.

use std::collections::HashMap;
use std::fmt;
use std::path::Path;
use std::sync::{Arc, Mutex};

use gpui_kit::{App, Global};
use sha2::{Digest as _, Sha256};

use crate::settings::AppSettings;

/// Secret bytes with deliberately redacted formatting and best-effort clearing.
#[derive(Clone, PartialEq, Eq)]
pub struct Secret(Vec<u8>);

impl Secret {
    pub fn new(value: impl Into<String>) -> Result<Self, CredentialError> {
        let value = value.into();
        let value = value.trim();
        if value.is_empty() {
            return Err(CredentialError::empty());
        }
        Ok(Self(value.as_bytes().to_vec()))
    }

    #[cfg(target_os = "windows")]
    fn from_bytes(value: Vec<u8>) -> Result<Self, CredentialError> {
        std::str::from_utf8(&value).map_err(|_| CredentialError::invalid_encoding())?;
        if value.is_empty() {
            return Err(CredentialError::empty());
        }
        Ok(Self(value))
    }

    pub fn expose(&self) -> &str {
        // Construction validates UTF-8 and no method mutates the bytes.
        std::str::from_utf8(&self.0).expect("validated credential UTF-8")
    }

    #[cfg(target_os = "windows")]
    fn bytes(&self) -> &[u8] {
        &self.0
    }
}

impl fmt::Debug for Secret {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("<redacted>")
    }
}

impl Drop for Secret {
    fn drop(&mut self) {
        self.0.fill(0);
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CredentialErrorKind {
    Unavailable,
    Read,
    Write,
    Verify,
    Delete,
    InvalidEncoding,
    Empty,
}

/// A content-free credential error suitable for logs and user diagnostics.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CredentialError {
    kind: CredentialErrorKind,
    platform_code: Option<i32>,
}

impl CredentialError {
    fn new(kind: CredentialErrorKind, platform_code: Option<i32>) -> Self {
        Self {
            kind,
            platform_code,
        }
    }

    #[cfg(not(target_os = "windows"))]
    fn unavailable() -> Self {
        Self::new(CredentialErrorKind::Unavailable, None)
    }

    #[cfg(target_os = "windows")]
    fn invalid_encoding() -> Self {
        Self::new(CredentialErrorKind::InvalidEncoding, None)
    }

    fn empty() -> Self {
        Self::new(CredentialErrorKind::Empty, None)
    }

    pub fn kind(&self) -> CredentialErrorKind {
        self.kind
    }
}

impl fmt::Display for CredentialError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let action = match self.kind {
            CredentialErrorKind::Unavailable => "Secure credential storage is unavailable",
            CredentialErrorKind::Read => "The secure credential could not be read",
            CredentialErrorKind::Write => "The secure credential could not be written",
            CredentialErrorKind::Verify => "The secure credential write could not be verified",
            CredentialErrorKind::Delete => "The secure credential could not be deleted",
            CredentialErrorKind::InvalidEncoding => "The secure credential is not valid UTF-8",
            CredentialErrorKind::Empty => "The credential is empty",
        };
        match self.platform_code {
            Some(code) => write!(formatter, "{action} (platform code {code})"),
            None => formatter.write_str(action),
        }
    }
}

impl std::error::Error for CredentialError {}

pub trait SecureCredentialStore: Send + Sync {
    fn is_supported(&self) -> bool;
    fn read(&self, target: &str) -> Result<Option<Secret>, CredentialError>;
    fn write(&self, target: &str, secret: &Secret) -> Result<(), CredentialError>;
    fn delete(&self, target: &str) -> Result<(), CredentialError>;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CredentialSource {
    Session,
    Persistent,
    Environment,
}

pub struct ResolvedCredential {
    secret: Secret,
    source: CredentialSource,
}

impl ResolvedCredential {
    pub fn secret(&self) -> &str {
        self.secret.expose()
    }

    pub fn source(&self) -> CredentialSource {
        self.source
    }
}

impl fmt::Debug for ResolvedCredential {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ResolvedCredential")
            .field("secret", &self.secret)
            .field("source", &self.source)
            .finish()
    }
}

/// Application-global credential access with an in-memory session override.
#[derive(Clone)]
pub struct CredentialVault {
    secure: Arc<dyn SecureCredentialStore>,
    session: Arc<Mutex<HashMap<String, Secret>>>,
}

impl Global for CredentialVault {}

impl CredentialVault {
    pub fn production() -> Self {
        Self::with_store(Arc::new(PlatformCredentialStore))
    }

    pub fn with_store(secure: Arc<dyn SecureCredentialStore>) -> Self {
        Self {
            secure,
            session: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    pub fn init(cx: &mut App) {
        if !cx.has_global::<Self>() {
            cx.set_global(Self::production());
        }
    }

    pub fn global(cx: &App) -> &Self {
        cx.global::<Self>()
    }

    pub fn secure_store_supported(&self) -> bool {
        self.secure.is_supported()
    }

    /// Resolve the key without ever moving an environment credential to a
    /// custom endpoint unless the caller proves that exact identity was
    /// authorized.
    pub fn resolve(
        &self,
        target: &str,
        environment_value: Option<String>,
        environment_allowed: bool,
    ) -> Result<Option<ResolvedCredential>, CredentialError> {
        if let Some(secret) = self
            .session
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .get(target)
            .cloned()
        {
            return Ok(Some(ResolvedCredential {
                secret,
                source: CredentialSource::Session,
            }));
        }

        let _operation = lock_persistent_operations()?;
        let secure_read = self.read_persistent_unless_pending(target);
        if let Ok(Some(secret)) = secure_read.as_ref() {
            return Ok(Some(ResolvedCredential {
                secret: secret.clone(),
                source: CredentialSource::Persistent,
            }));
        }

        if environment_allowed
            && let Some(value) = environment_value
            && let Ok(secret) = Secret::new(value)
        {
            return Ok(Some(ResolvedCredential {
                secret,
                source: CredentialSource::Environment,
            }));
        }

        secure_read.map(|_| None)
    }

    pub fn replace_session(
        &self,
        target: impl Into<String>,
        value: String,
    ) -> Result<(), CredentialError> {
        let secret = Secret::new(value)?;
        self.session
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .insert(target.into(), secret);
        Ok(())
    }

    /// Write, read back, and compare before reporting persistent success.
    pub fn replace_persistent(&self, target: &str, value: String) -> Result<(), CredentialError> {
        let secret = Secret::new(value)?;
        let _operation = lock_persistent_operations()?;
        let marker_target = Self::pending_target(target);
        let marker_present = self.secure.read(&marker_target)?.is_some();
        let previous = if marker_present {
            None
        } else {
            self.secure.read(target)?
        };
        if !marker_present {
            self.write_pending_marker(&marker_target)?;
        }
        if let Err(error) = self.secure.write(target, &secret) {
            if !marker_present {
                self.restore_and_clear_marker(target, previous.as_ref(), &marker_target);
            }
            return Err(error);
        }
        let verified = self.secure.read(target);
        if !matches!(verified.as_ref(), Ok(Some(stored)) if stored == &secret) {
            if !marker_present {
                self.restore_and_clear_marker(target, previous.as_ref(), &marker_target);
            }
            return Err(CredentialError::new(CredentialErrorKind::Verify, None));
        }
        if !self.clear_pending_marker(&marker_target) {
            return Err(CredentialError::new(CredentialErrorKind::Verify, None));
        }
        self.session
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .remove(target);
        Ok(())
    }

    pub fn has_persistent(&self, target: &str) -> Result<bool, CredentialError> {
        let _operation = lock_persistent_operations()?;
        Ok(self.read_persistent_unless_pending(target)?.is_some())
    }

    pub fn delete(&self, target: &str) -> Result<(), CredentialError> {
        let _operation = lock_persistent_operations()?;
        if self.secure.is_supported() {
            self.secure.delete(target)?;
            self.secure.delete(&Self::pending_target(target))?;
        }
        self.session
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .remove(target);
        Ok(())
    }

    fn pending_target(target: &str) -> String {
        let digest = Sha256::digest(target.as_bytes());
        format!("markturbo:credential-write-pending:v1:{digest:x}")
    }

    fn read_persistent_unless_pending(
        &self,
        target: &str,
    ) -> Result<Option<Secret>, CredentialError> {
        let marker_target = Self::pending_target(target);
        if self.secure.read(&marker_target)?.is_some() {
            return Err(CredentialError::new(CredentialErrorKind::Verify, None));
        }
        let stored = self.secure.read(target);
        if self.secure.read(&marker_target)?.is_some() {
            return Err(CredentialError::new(CredentialErrorKind::Verify, None));
        }
        stored
    }

    fn write_pending_marker(&self, marker_target: &str) -> Result<(), CredentialError> {
        let marker = Secret::new("pending credential write")
            .expect("the credential transaction marker is non-empty");
        self.secure.write(marker_target, &marker)?;
        if matches!(self.secure.read(marker_target), Ok(Some(stored)) if stored == marker) {
            return Ok(());
        }
        self.clear_pending_marker(marker_target);
        Err(CredentialError::new(CredentialErrorKind::Verify, None))
    }

    fn restore_and_clear_marker(
        &self,
        target: &str,
        previous: Option<&Secret>,
        marker_target: &str,
    ) {
        if self.restore_previous(target, previous) {
            self.clear_pending_marker(marker_target);
        }
    }

    fn restore_previous(&self, target: &str, previous: Option<&Secret>) -> bool {
        match previous {
            Some(previous) => self
                .secure
                .write(target, previous)
                .and_then(|_| self.secure.read(target))
                .is_ok_and(|stored| stored.as_ref() == Some(previous)),
            None => self
                .secure
                .delete(target)
                .and_then(|_| self.secure.read(target))
                .is_ok_and(|stored| stored.is_none()),
        }
    }

    fn clear_pending_marker(&self, marker_target: &str) -> bool {
        self.secure
            .delete(marker_target)
            .and_then(|_| self.secure.read(marker_target))
            .is_ok_and(|stored| stored.is_none())
    }
}

#[cfg(target_os = "windows")]
struct CredentialOperationGuard {
    handle: windows::Win32::Foundation::HANDLE,
}

#[cfg(target_os = "windows")]
impl Drop for CredentialOperationGuard {
    fn drop(&mut self) {
        use windows::Win32::{Foundation::CloseHandle, System::Threading::ReleaseMutex};

        let _ = unsafe { ReleaseMutex(self.handle) };
        let _ = unsafe { CloseHandle(self.handle) };
    }
}

#[cfg(target_os = "windows")]
fn lock_persistent_operations() -> Result<CredentialOperationGuard, CredentialError> {
    use windows::Win32::{
        Foundation::{CloseHandle, GetLastError, WAIT_ABANDONED, WAIT_OBJECT_0, WAIT_TIMEOUT},
        System::Threading::{CreateMutexW, WaitForSingleObject},
    };
    use windows::core::PCWSTR;

    const WAIT_MILLISECONDS: u32 = 100;
    let name = wide("Global\\markturbo-model-credential-operation-v1");
    let handle = unsafe { CreateMutexW(None, false, PCWSTR(name.as_ptr())) }.map_err(|error| {
        CredentialError::new(CredentialErrorKind::Unavailable, Some(error.code().0))
    })?;
    let wait = unsafe { WaitForSingleObject(handle, WAIT_MILLISECONDS) };
    if wait == WAIT_OBJECT_0 || wait == WAIT_ABANDONED {
        return Ok(CredentialOperationGuard { handle });
    }

    let platform_code = (wait != WAIT_TIMEOUT).then(|| unsafe { GetLastError() }.0 as i32);
    let _ = unsafe { CloseHandle(handle) };
    Err(CredentialError::new(
        CredentialErrorKind::Unavailable,
        platform_code,
    ))
}

#[cfg(not(target_os = "windows"))]
struct CredentialOperationGuard {
    _guard: std::sync::MutexGuard<'static, ()>,
}

#[cfg(not(target_os = "windows"))]
fn lock_persistent_operations() -> Result<CredentialOperationGuard, CredentialError> {
    static OPERATION: Mutex<()> = Mutex::new(());
    Ok(CredentialOperationGuard {
        _guard: OPERATION
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()),
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LegacyCredentialMigration {
    NotPresent,
    Migrated,
}

#[derive(Debug)]
pub enum LegacyCredentialMigrationError {
    SecureStore(CredentialError),
    SettingsWrite(std::io::Error),
}

impl fmt::Display for LegacyCredentialMigrationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::SecureStore(error) => write!(formatter, "{error}"),
            Self::SettingsWrite(_) => formatter.write_str(
                "The credential was stored securely, but the plaintext settings value could not be removed",
            ),
        }
    }
}

impl std::error::Error for LegacyCredentialMigrationError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::SecureStore(error) => Some(error),
            Self::SettingsWrite(error) => Some(error),
        }
    }
}

/// Copy one legacy plaintext key into secure storage and verify it there.
pub(crate) fn secure_legacy_credential(
    settings: &AppSettings,
    vault: &CredentialVault,
    target: &str,
) -> Result<LegacyCredentialMigration, LegacyCredentialMigrationError> {
    let Some(legacy) = settings.legacy_model_api_key().map(str::to_owned) else {
        return Ok(LegacyCredentialMigration::NotPresent);
    };

    vault
        .replace_persistent(target, legacy)
        .map_err(LegacyCredentialMigrationError::SecureStore)?;

    Ok(LegacyCredentialMigration::Migrated)
}

/// Remove only the legacy field from the latest settings snapshot.
pub(crate) fn remove_migrated_legacy_credential(
    settings: &mut AppSettings,
    settings_path: &Path,
) -> Result<(), LegacyCredentialMigrationError> {
    let mut migrated = settings.clone();
    migrated.clear_legacy_model_api_key();
    let migrated = migrated
        .try_save_after_legacy_migration(settings_path)
        .map_err(LegacyCredentialMigrationError::SettingsWrite)?;
    *settings = migrated;
    Ok(())
}

struct PlatformCredentialStore;

#[cfg(not(target_os = "windows"))]
impl SecureCredentialStore for PlatformCredentialStore {
    fn is_supported(&self) -> bool {
        false
    }

    fn read(&self, _: &str) -> Result<Option<Secret>, CredentialError> {
        Ok(None)
    }

    fn write(&self, _: &str, _: &Secret) -> Result<(), CredentialError> {
        Err(CredentialError::unavailable())
    }

    fn delete(&self, _: &str) -> Result<(), CredentialError> {
        Err(CredentialError::unavailable())
    }
}

#[cfg(target_os = "windows")]
impl SecureCredentialStore for PlatformCredentialStore {
    fn is_supported(&self) -> bool {
        true
    }

    fn read(&self, target: &str) -> Result<Option<Secret>, CredentialError> {
        use std::ptr;
        use windows::Win32::Foundation::ERROR_NOT_FOUND;
        use windows::Win32::Security::Credentials::{
            CRED_TYPE_GENERIC, CREDENTIALW, CredFree, CredReadW,
        };
        use windows::core::{HRESULT, PCWSTR};

        let target = wide(target);
        let mut credential = ptr::null_mut::<CREDENTIALW>();
        let read = unsafe {
            CredReadW(
                PCWSTR(target.as_ptr()),
                CRED_TYPE_GENERIC,
                None,
                &mut credential,
            )
        };
        if let Err(error) = read {
            if error.code() == HRESULT::from_win32(ERROR_NOT_FOUND.0) {
                return Ok(None);
            }
            return Err(CredentialError::new(
                CredentialErrorKind::Read,
                Some(error.code().0),
            ));
        }

        struct CredentialGuard(*mut CREDENTIALW);
        impl Drop for CredentialGuard {
            fn drop(&mut self) {
                unsafe { CredFree(self.0.cast()) };
            }
        }
        let guard = CredentialGuard(credential);
        let credential = unsafe { &*guard.0 };
        secret_from_credential_blob(
            credential.CredentialBlob,
            credential.CredentialBlobSize as usize,
        )
        .map(Some)
    }

    fn write(&self, target: &str, secret: &Secret) -> Result<(), CredentialError> {
        use windows::Win32::Security::Credentials::{
            CRED_PERSIST_LOCAL_MACHINE, CRED_TYPE_GENERIC, CREDENTIALW, CredWriteW,
        };
        use windows::core::PWSTR;

        let mut target = wide(target);
        let mut username = wide("markturbo");
        let mut blob = secret.bytes().to_vec();
        let credential = CREDENTIALW {
            Type: CRED_TYPE_GENERIC,
            TargetName: PWSTR(target.as_mut_ptr()),
            CredentialBlobSize: blob.len() as u32,
            CredentialBlob: blob.as_mut_ptr(),
            Persist: CRED_PERSIST_LOCAL_MACHINE,
            UserName: PWSTR(username.as_mut_ptr()),
            ..Default::default()
        };
        let result = unsafe { CredWriteW(&credential, 0) };
        blob.fill(0);
        result
            .map_err(|error| CredentialError::new(CredentialErrorKind::Write, Some(error.code().0)))
    }

    fn delete(&self, target: &str) -> Result<(), CredentialError> {
        use windows::Win32::Foundation::ERROR_NOT_FOUND;
        use windows::Win32::Security::Credentials::{CRED_TYPE_GENERIC, CredDeleteW};
        use windows::core::{HRESULT, PCWSTR};

        let target = wide(target);
        match unsafe { CredDeleteW(PCWSTR(target.as_ptr()), CRED_TYPE_GENERIC, None) } {
            Ok(()) => Ok(()),
            Err(error) if error.code() == HRESULT::from_win32(ERROR_NOT_FOUND.0) => Ok(()),
            Err(error) => Err(CredentialError::new(
                CredentialErrorKind::Delete,
                Some(error.code().0),
            )),
        }
    }
}

#[cfg(target_os = "windows")]
fn wide(value: &str) -> Vec<u16> {
    value.encode_utf16().chain(std::iter::once(0)).collect()
}

#[cfg(target_os = "windows")]
fn secret_from_credential_blob(pointer: *const u8, size: usize) -> Result<Secret, CredentialError> {
    if size == 0 {
        return Err(CredentialError::empty());
    }
    if pointer.is_null() {
        return Err(CredentialError::new(CredentialErrorKind::Read, None));
    }
    let bytes = unsafe { std::slice::from_raw_parts(pointer, size) };
    Secret::from_bytes(bytes.to_vec())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Default)]
    struct FakeStore {
        values: Mutex<HashMap<String, Secret>>,
        fail_read: Mutex<bool>,
        fail_write: Mutex<bool>,
        fail_delete: Mutex<bool>,
        corrupt_read_target: Mutex<Option<String>>,
        publish_pending_on_read: Mutex<Option<(String, String)>>,
    }

    impl SecureCredentialStore for FakeStore {
        fn is_supported(&self) -> bool {
            true
        }

        fn read(&self, target: &str) -> Result<Option<Secret>, CredentialError> {
            if *self
                .fail_read
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
            {
                return Err(CredentialError::new(CredentialErrorKind::Read, Some(5)));
            }
            let pending = {
                let mut pending = self
                    .publish_pending_on_read
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                if pending
                    .as_ref()
                    .is_some_and(|(pending_target, _)| pending_target == target)
                {
                    pending.take()
                } else {
                    None
                }
            };
            if let Some((pending_target, marker_target)) = pending {
                let mut values = self
                    .values
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                values.insert(
                    pending_target,
                    Secret::new("unverified-cross-process-secret").unwrap(),
                );
                values.insert(
                    marker_target,
                    Secret::new("pending credential write").unwrap(),
                );
            }
            let stored = self
                .values
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .get(target)
                .cloned();
            if stored.is_some()
                && self
                    .corrupt_read_target
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                    .as_deref()
                    == Some(target)
            {
                return Ok(Some(Secret::new("different-secret").unwrap()));
            }
            Ok(stored)
        }

        fn write(&self, target: &str, secret: &Secret) -> Result<(), CredentialError> {
            if *self
                .fail_write
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
            {
                return Err(CredentialError::new(CredentialErrorKind::Write, Some(5)));
            }
            self.values
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .insert(target.to_string(), secret.clone());
            Ok(())
        }

        fn delete(&self, target: &str) -> Result<(), CredentialError> {
            if *self
                .fail_delete
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
            {
                return Err(CredentialError::new(CredentialErrorKind::Delete, Some(5)));
            }
            self.values
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .remove(target);
            Ok(())
        }
    }

    #[test]
    fn session_then_persistent_then_authorized_environment_define_precedence() {
        let store = Arc::new(FakeStore::default());
        let vault = CredentialVault::with_store(store.clone());
        let target = "markturbo:model:test";

        assert!(
            vault
                .resolve(target, Some("environment".into()), false)
                .unwrap()
                .is_none(),
            "an unauthorized custom endpoint must not receive the ambient key"
        );
        assert_eq!(
            vault
                .resolve(target, Some("environment".into()), true)
                .unwrap()
                .unwrap()
                .source(),
            CredentialSource::Environment
        );

        vault
            .replace_persistent(target, "persistent".into())
            .unwrap();
        assert_eq!(
            vault
                .resolve(target, Some("environment".into()), true)
                .unwrap()
                .unwrap()
                .source(),
            CredentialSource::Persistent
        );

        vault.replace_session(target, "session".into()).unwrap();
        let resolved = vault
            .resolve(target, Some("environment".into()), true)
            .unwrap()
            .unwrap();
        assert_eq!(resolved.source(), CredentialSource::Session);
        assert_eq!(resolved.secret(), "session");
    }

    #[test]
    fn environment_and_session_credentials_never_write_the_secure_store_implicitly() {
        let store = Arc::new(FakeStore::default());
        let vault = CredentialVault::with_store(store.clone());
        let target = "markturbo:model:nonpersistent";

        let environment = vault
            .resolve(target, Some("environment-only".into()), true)
            .unwrap()
            .unwrap();
        assert_eq!(environment.source(), CredentialSource::Environment);
        vault
            .replace_session(target, "session-only".into())
            .unwrap();

        assert!(
            store
                .values
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .is_empty()
        );
    }

    #[test]
    fn an_authorized_environment_key_remains_a_secure_store_failure_fallback() {
        let store = Arc::new(FakeStore::default());
        *store
            .fail_read
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = true;
        let vault = CredentialVault::with_store(store);

        let resolved = vault
            .resolve("target", Some("environment-fallback".into()), true)
            .unwrap()
            .unwrap();
        assert_eq!(resolved.source(), CredentialSource::Environment);
        assert_eq!(resolved.secret(), "environment-fallback");
        assert!(
            vault
                .resolve("target", Some("blocked".into()), false)
                .is_err(),
            "a secure-store failure must not silently authorize an ambient key"
        );
    }

    #[test]
    fn persistent_success_requires_a_verified_read_back() {
        let store = Arc::new(FakeStore::default());
        let vault = CredentialVault::with_store(store.clone());

        *store
            .corrupt_read_target
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some("target".into());
        let error = vault
            .replace_persistent("target", "sentinel-secret".into())
            .unwrap_err();
        assert_eq!(error.kind(), CredentialErrorKind::Verify);
        assert!(!error.to_string().contains("sentinel-secret"));
        *store
            .corrupt_read_target
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = None;
        assert!(vault.resolve("target", None, false).unwrap().is_none());
    }

    #[test]
    fn failed_compensation_quarantines_an_unverified_value_across_restart() {
        let store = Arc::new(FakeStore::default());
        let vault = CredentialVault::with_store(store.clone());

        *store
            .corrupt_read_target
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some("target".into());
        *store
            .fail_delete
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = true;
        let error = vault
            .replace_persistent("target", "unverified-secret".into())
            .unwrap_err();
        assert_eq!(error.kind(), CredentialErrorKind::Verify);

        *store
            .corrupt_read_target
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = None;
        drop(vault);
        let restarted = CredentialVault::with_store(store.clone());
        let error = restarted.resolve("target", None, false).unwrap_err();
        assert_eq!(error.kind(), CredentialErrorKind::Verify);
        assert_eq!(
            restarted
                .resolve("target", Some("environment-fallback".into()), true)
                .unwrap()
                .unwrap()
                .source(),
            CredentialSource::Environment
        );
        assert_eq!(
            restarted.has_persistent("target").unwrap_err().kind(),
            CredentialErrorKind::Verify
        );

        *store
            .fail_delete
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = false;
        restarted.delete("target").unwrap();
        assert!(restarted.resolve("target", None, false).unwrap().is_none());
    }

    #[test]
    fn replacing_a_quarantined_target_never_clears_its_existing_marker_early() {
        let store = Arc::new(FakeStore::default());
        let target = "target";
        let marker_target = CredentialVault::pending_target(target);
        {
            let mut values = store
                .values
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            values.insert(target.into(), Secret::new("old-unverified-secret").unwrap());
            values.insert(
                marker_target.clone(),
                Secret::new("pending credential write").unwrap(),
            );
        }
        *store
            .corrupt_read_target
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(marker_target);
        let vault = CredentialVault::with_store(store.clone());

        vault
            .replace_persistent(target, "verified-replacement".into())
            .unwrap();

        *store
            .corrupt_read_target
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = None;
        let resolved = vault.resolve(target, None, false).unwrap().unwrap();
        assert_eq!(resolved.secret(), "verified-replacement");
    }

    #[test]
    fn failed_write_to_a_quarantined_target_preserves_the_target_and_marker() {
        let store = Arc::new(FakeStore::default());
        let target = "target";
        let marker_target = CredentialVault::pending_target(target);
        {
            let mut values = store
                .values
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            values.insert(target.into(), Secret::new("existing-secret").unwrap());
            values.insert(
                marker_target.clone(),
                Secret::new("pending credential write").unwrap(),
            );
        }
        *store
            .fail_write
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = true;
        let vault = CredentialVault::with_store(store.clone());

        let error = vault
            .replace_persistent(target, "replacement-secret".into())
            .unwrap_err();

        assert_eq!(error.kind(), CredentialErrorKind::Write);
        let values = store
            .values
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        assert_eq!(
            values.get(target).map(Secret::expose),
            Some("existing-secret")
        );
        assert!(values.contains_key(&marker_target));
        drop(values);
        *store
            .fail_write
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = false;
        drop(vault);
        let restarted = CredentialVault::with_store(store);
        assert_eq!(
            restarted.resolve(target, None, false).unwrap_err().kind(),
            CredentialErrorKind::Verify
        );
        assert_eq!(
            restarted.has_persistent(target).unwrap_err().kind(),
            CredentialErrorKind::Verify
        );
    }

    #[test]
    fn failed_verification_of_a_quarantined_target_preserves_the_quarantine() {
        let store = Arc::new(FakeStore::default());
        let target = "target";
        let marker_target = CredentialVault::pending_target(target);
        {
            let mut values = store
                .values
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            values.insert(target.into(), Secret::new("existing-secret").unwrap());
            values.insert(
                marker_target.clone(),
                Secret::new("pending credential write").unwrap(),
            );
        }
        *store
            .corrupt_read_target
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(target.into());
        let vault = CredentialVault::with_store(store.clone());

        let error = vault
            .replace_persistent(target, "replacement-secret".into())
            .unwrap_err();

        assert_eq!(error.kind(), CredentialErrorKind::Verify);
        let values = store
            .values
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        assert_eq!(
            values.get(target).map(Secret::expose),
            Some("replacement-secret")
        );
        assert!(values.contains_key(&marker_target));
        drop(values);
        *store
            .corrupt_read_target
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = None;
        drop(vault);
        let restarted = CredentialVault::with_store(store);
        assert_eq!(
            restarted.resolve(target, None, false).unwrap_err().kind(),
            CredentialErrorKind::Verify
        );
        assert_eq!(
            restarted.has_persistent(target).unwrap_err().kind(),
            CredentialErrorKind::Verify
        );
    }

    #[test]
    fn a_pending_marker_double_check_blocks_a_cross_process_write_race() {
        let store = Arc::new(FakeStore::default());
        let target = "target";
        *store
            .publish_pending_on_read
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) =
            Some((target.into(), CredentialVault::pending_target(target)));
        let vault = CredentialVault::with_store(store);

        let error = vault.resolve(target, None, false).unwrap_err();

        assert_eq!(error.kind(), CredentialErrorKind::Verify);
    }

    #[test]
    fn a_failed_secure_write_never_reports_success_or_the_secret() {
        let store = Arc::new(FakeStore::default());
        *store
            .fail_write
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = true;
        let vault = CredentialVault::with_store(store);

        let error = vault
            .replace_persistent("target", "write-failure-secret".into())
            .unwrap_err();
        assert_eq!(error.kind(), CredentialErrorKind::Write);
        assert!(!format!("{error:?} {error}").contains("write-failure-secret"));
    }

    #[test]
    fn deleting_one_identity_leaves_every_other_identity_untouched() {
        let store = Arc::new(FakeStore::default());
        let vault = CredentialVault::with_store(store);
        vault.replace_persistent("first", "one".into()).unwrap();
        vault.replace_persistent("second", "two".into()).unwrap();
        vault
            .replace_session("first", "session-one".into())
            .unwrap();

        vault.delete("first").unwrap();

        assert!(vault.resolve("first", None, false).unwrap().is_none());
        assert_eq!(
            vault
                .resolve("second", None, false)
                .unwrap()
                .unwrap()
                .secret(),
            "two"
        );
    }

    #[test]
    fn failed_persistent_delete_preserves_the_session_override() {
        let store = Arc::new(FakeStore::default());
        let vault = CredentialVault::with_store(store.clone());
        vault
            .replace_persistent("target", "persistent".into())
            .unwrap();
        vault.replace_session("target", "session".into()).unwrap();
        *store
            .fail_delete
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = true;

        let error = vault.delete("target").unwrap_err();

        assert_eq!(error.kind(), CredentialErrorKind::Delete);
        let resolved = vault.resolve("target", None, false).unwrap().unwrap();
        assert_eq!(resolved.source(), CredentialSource::Session);
        assert_eq!(resolved.secret(), "session");
    }

    #[test]
    fn replacing_one_identity_leaves_every_other_identity_untouched() {
        let store = Arc::new(FakeStore::default());
        let vault = CredentialVault::with_store(store);
        vault.replace_persistent("first", "old".into()).unwrap();
        vault.replace_persistent("second", "two".into()).unwrap();

        vault.replace_persistent("first", "new".into()).unwrap();

        assert_eq!(
            vault
                .resolve("first", None, false)
                .unwrap()
                .unwrap()
                .secret(),
            "new"
        );
        assert_eq!(
            vault
                .resolve("second", None, false)
                .unwrap()
                .unwrap()
                .secret(),
            "two"
        );
    }

    #[test]
    fn debug_output_never_contains_secret_bytes() {
        let secret = Secret::new("debug-secret-sentinel").unwrap();
        let resolved = ResolvedCredential {
            secret,
            source: CredentialSource::Session,
        };

        let debug = format!("{resolved:?}");
        assert!(!debug.contains("debug-secret-sentinel"));
        assert!(debug.contains("<redacted>"));
    }

    #[test]
    fn legacy_migration_clears_plaintext_only_after_verified_secure_write() {
        let store = Arc::new(FakeStore::default());
        let vault = CredentialVault::with_store(store);
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.toml");
        std::fs::write(&path, "translate-api-key = \"legacy-migration-sentinel\"\n").unwrap();
        let mut settings = AppSettings::load_from(&path);

        assert_eq!(
            secure_legacy_credential(&settings, &vault, "target").unwrap(),
            LegacyCredentialMigration::Migrated
        );
        remove_migrated_legacy_credential(&mut settings, &path).unwrap();
        assert!(settings.legacy_model_api_key().is_none());
        let persisted = std::fs::read_to_string(path).unwrap();
        assert!(!persisted.contains("legacy-migration-sentinel"));
        assert_eq!(
            vault
                .resolve("target", None, false)
                .unwrap()
                .unwrap()
                .secret(),
            "legacy-migration-sentinel"
        );
    }

    #[test]
    fn goal_05a_credential_lifecycle_failed_migration_preserves_the_only_plaintext_copy() {
        let store = Arc::new(FakeStore::default());
        *store
            .fail_write
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = true;
        let vault = CredentialVault::with_store(store);
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.toml");
        std::fs::write(&path, "translate-api-key = \"failed-migration-sentinel\"\n").unwrap();
        let settings = AppSettings::load_from(&path);

        let error = secure_legacy_credential(&settings, &vault, "target")
            .expect_err("the fake secure write fails");
        assert!(matches!(
            error,
            LegacyCredentialMigrationError::SecureStore(_)
        ));
        assert_eq!(
            settings.legacy_model_api_key(),
            Some("failed-migration-sentinel")
        );
        assert!(
            std::fs::read_to_string(path)
                .unwrap()
                .contains("failed-migration-sentinel")
        );
        assert!(!error.to_string().contains("failed-migration-sentinel"));
    }

    #[test]
    fn legacy_migration_preserves_settings_changed_while_secure_storage_runs() {
        let store = Arc::new(FakeStore::default());
        let vault = CredentialVault::with_store(store);
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.toml");
        std::fs::write(
            &path,
            "translate-api-key = \"migration-race-sentinel\"\ntheme = \"dark\"\n",
        )
        .unwrap();
        let original = AppSettings::load_from(&path);

        assert_eq!(
            secure_legacy_credential(&original, &vault, "target").unwrap(),
            LegacyCredentialMigration::Migrated
        );
        let mut latest = original;
        latest.theme = crate::settings::ThemePreference::Light;
        latest.watch_auto_reload = true;
        remove_migrated_legacy_credential(&mut latest, &path).unwrap();

        let persisted = AppSettings::load_from(&path);
        assert_eq!(persisted.theme, crate::settings::ThemePreference::Light);
        assert!(persisted.watch_auto_reload);
        assert!(persisted.legacy_model_api_key().is_none());
    }

    #[test]
    fn stale_process_cannot_restore_a_migrated_plaintext_credential() {
        let store = Arc::new(FakeStore::default());
        let vault = CredentialVault::with_store(store);
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.toml");
        std::fs::write(
            &path,
            "translate-api-key = \"stale-process-sentinel\"\ntheme = \"light\"\n",
        )
        .unwrap();
        let mut migrating = AppSettings::load_from(&path);
        let mut stale = AppSettings::load_from(&path);

        assert_eq!(
            secure_legacy_credential(&migrating, &vault, "target").unwrap(),
            LegacyCredentialMigration::Migrated
        );
        remove_migrated_legacy_credential(&mut migrating, &path).unwrap();

        stale.theme = crate::settings::ThemePreference::Dark;
        stale.try_save_general_to(&path).unwrap();

        let persisted = std::fs::read_to_string(&path).unwrap();
        assert!(!persisted.contains("stale-process-sentinel"));
        let persisted = AppSettings::load_from(&path);
        assert_eq!(persisted.theme, crate::settings::ThemePreference::Dark);
        assert!(persisted.legacy_model_api_key().is_none());
    }

    #[test]
    fn resolved_credentials_never_enter_the_settings_file() {
        const PERSISTENT: &str = "persistent-settings-sentinel";
        const SESSION: &str = "session-settings-sentinel";
        const ENVIRONMENT: &str = "environment-settings-sentinel";

        let store = Arc::new(FakeStore::default());
        let vault = CredentialVault::with_store(store);
        let target = "markturbo:model:settings-privacy";
        vault.replace_persistent(target, PERSISTENT.into()).unwrap();
        assert_eq!(
            vault
                .resolve(target, None, false)
                .unwrap()
                .unwrap()
                .secret(),
            PERSISTENT
        );
        vault.replace_session(target, SESSION.into()).unwrap();
        assert_eq!(
            vault
                .resolve(target, Some(ENVIRONMENT.into()), true)
                .unwrap()
                .unwrap()
                .secret(),
            SESSION
        );
        let environment_target = "markturbo:model:environment-settings-privacy";
        assert_eq!(
            vault
                .resolve(environment_target, Some(ENVIRONMENT.into()), true)
                .unwrap()
                .unwrap()
                .secret(),
            ENVIRONMENT
        );

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.toml");
        let mut settings = AppSettings::default();
        settings.model_provider = "openai-chat".into();
        settings.model_name = "model-settings-privacy".into();
        settings.model_base_url = "https://gateway.example/v1/".into();
        settings.model_environment_key_identity = environment_target.into();
        settings.save_to(&path);
        let persisted = std::fs::read_to_string(path).unwrap();
        for sentinel in [PERSISTENT, SESSION, ENVIRONMENT] {
            assert!(!persisted.contains(sentinel));
        }
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn credential_blob_validation_rejects_empty_or_null_storage() {
        assert_eq!(
            secret_from_credential_blob(std::ptr::null(), 0)
                .unwrap_err()
                .kind(),
            CredentialErrorKind::Empty
        );
        assert_eq!(
            secret_from_credential_blob(std::ptr::null(), 1)
                .unwrap_err()
                .kind(),
            CredentialErrorKind::Read
        );
        let bytes = b"synthetic-blob";
        assert_eq!(
            secret_from_credential_blob(bytes.as_ptr(), bytes.len())
                .unwrap()
                .expose(),
            "synthetic-blob"
        );
    }

    #[cfg(target_os = "windows")]
    #[test]
    #[ignore = "Goal 05A Windows Credential Manager acceptance; run explicitly"]
    fn goal_05a_credential_lifecycle_global_lock_fails_closed_when_busy() {
        let first = lock_persistent_operations().unwrap();
        let (result_sender, result_receiver) = std::sync::mpsc::sync_channel(0);
        let contender = std::thread::spawn(move || {
            result_sender
                .send(lock_persistent_operations().map(drop))
                .unwrap();
        });

        let error = result_receiver
            .recv_timeout(std::time::Duration::from_secs(5))
            .unwrap()
            .unwrap_err();
        assert_eq!(error.kind(), CredentialErrorKind::Unavailable);
        contender.join().unwrap();

        drop(first);
        drop(lock_persistent_operations().unwrap());
    }

    #[cfg(target_os = "windows")]
    #[test]
    #[ignore = "Goal 05A Windows Credential Manager acceptance; run explicitly"]
    fn goal_05a_credential_lifecycle_windows_round_trip() {
        let host = format!(
            "acceptance-{}-{}.invalid",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        );
        let upper = crate::model::EndpointIdentity::parse(
            crate::model::Provider::OpenAiChat,
            Some(&format!("https://{host}/Case/")),
        )
        .unwrap()
        .credential_target()
        .to_string();
        let lower = crate::model::EndpointIdentity::parse(
            crate::model::Provider::OpenAiChat,
            Some(&format!("https://{host}/case/")),
        )
        .unwrap()
        .credential_target()
        .to_string();
        assert_ne!(upper.to_ascii_lowercase(), lower.to_ascii_lowercase());
        let store = PlatformCredentialStore;
        struct Cleanup<'a> {
            store: &'a PlatformCredentialStore,
            targets: Vec<String>,
        }
        impl Drop for Cleanup<'_> {
            fn drop(&mut self) {
                for target in &self.targets {
                    let _ = self.store.delete(target);
                }
            }
        }
        let cleanup = Cleanup {
            store: &store,
            targets: vec![
                upper.clone(),
                CredentialVault::pending_target(&upper),
                lower.clone(),
            ],
        };
        let dir = tempfile::tempdir().unwrap();
        let settings_path = dir.path().join("settings.toml");
        std::fs::write(
            &settings_path,
            "translate-api-key = \"synthetic-credential-manager-upper\"\n",
        )
        .unwrap();
        let mut settings = AppSettings::load_from(&settings_path);
        let vault = CredentialVault::production();
        let lower_secret = Secret::new("synthetic-credential-manager-lower").unwrap();

        assert_eq!(
            secure_legacy_credential(&settings, &vault, &upper).unwrap(),
            LegacyCredentialMigration::Migrated
        );
        remove_migrated_legacy_credential(&mut settings, &settings_path).unwrap();
        assert!(settings.legacy_model_api_key().is_none());
        assert!(
            !std::fs::read_to_string(&settings_path)
                .unwrap()
                .contains("synthetic-credential-manager-upper")
        );
        store.write(&lower, &lower_secret).unwrap();
        assert_eq!(
            store.read(&upper).unwrap(),
            Some(Secret::new("synthetic-credential-manager-upper").unwrap())
        );
        assert_eq!(store.read(&lower).unwrap(), Some(lower_secret));

        vault
            .replace_persistent(&upper, "synthetic-credential-manager-replacement".into())
            .unwrap();
        assert_eq!(
            store.read(&upper).unwrap(),
            Some(Secret::new("synthetic-credential-manager-replacement").unwrap())
        );

        vault
            .replace_session(&upper, "synthetic-session-override".into())
            .unwrap();
        assert_eq!(
            vault
                .resolve(&upper, None, false)
                .unwrap()
                .unwrap()
                .source(),
            CredentialSource::Session
        );
        vault.delete(&upper).unwrap();
        assert!(vault.resolve(&upper, None, false).unwrap().is_none());
        store.delete(&lower).unwrap();
        assert!(store.read(&lower).unwrap().is_none());
        drop(cleanup);
    }
}
