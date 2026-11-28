//! One cross-process lock implementation for the on-disk state.
//!
//! Two names exist because the resources are unrelated: settings writes and
//! model credential operations. Windows gets a named mutex, so two markturbo
//! processes cannot interleave a read-modify-write of the same file; the other
//! platforms keep process-wide mutexes, which a single desktop instance and
//! the test binaries already need. A busy name fails closed after a short wait
//! instead of blocking the caller.
//!
//! Tests isolate themselves through names, not code paths:
//! `MARKTURBO_TEST_LOCK_NAMESPACE` selects the settings scope, and the
//! credential scope is derived from the fake store's address.

use std::io;
use std::path::Path;

/// How long a lock waits before reporting that another process holds it.
#[cfg(target_os = "windows")]
const WAIT_MILLISECONDS: u32 = 100;

/// The production settings mutex. Changing it drops the exclusion two running
/// versions of markturbo have with each other.
pub(crate) const SETTINGS_WRITE_LOCK: &str = "Global\\markturbo-settings-write-v1";
/// The production credential mutex, shared with the durable credential
/// transaction.
#[cfg(target_os = "windows")]
pub(crate) const CREDENTIAL_OPERATION_LOCK: &str =
    "Global\\markturbo-model-credential-operation-v1";

const TEST_SETTINGS_WRITE_LOCK_PREFIX: &str = "Local\\markturbo-settings-write-test-";
#[cfg(all(any(test, feature = "test-support"), target_os = "windows"))]
const TEST_CREDENTIAL_OPERATION_LOCK_PREFIX: &str = "Local\\markturbo-model-credential-test-";
const TEST_LOCK_NAMESPACE_ENV: &str = "MARKTURBO_TEST_LOCK_NAMESPACE";
const TEST_LOCK_NAMESPACE_MAX_LENGTH: usize = 64;

#[cfg(target_os = "windows")]
const SETTINGS_BUSY_MESSAGE: &str = "another markturbo process is writing settings";

/// Why a named lock could not be taken.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
pub(crate) enum LockError {
    /// Another process holds the name.
    Busy,
    /// The OS refused the operation; the code is a platform error code.
    Os(i32),
}

impl LockError {
    /// The settings surface: a busy name is a `WouldBlock` with its own text.
    #[cfg(target_os = "windows")]
    fn into_settings_io(self) -> io::Error {
        match self {
            Self::Busy => io::Error::new(io::ErrorKind::WouldBlock, SETTINGS_BUSY_MESSAGE),
            Self::Os(code) => io::Error::from_raw_os_error(code),
        }
    }

    /// The credential surface: a busy name has no platform code, an OS error
    /// carries its own.
    pub(crate) fn platform_code(self) -> Option<i32> {
        match self {
            Self::Busy => None,
            Self::Os(code) => Some(code),
        }
    }
}

/// A held lock. Dropping it releases the name.
pub(crate) struct Guard {
    #[cfg(target_os = "windows")]
    handle: windows::Win32::Foundation::HANDLE,
    #[cfg(not(target_os = "windows"))]
    _guard: std::sync::MutexGuard<'static, ()>,
}

#[cfg(target_os = "windows")]
impl Drop for Guard {
    fn drop(&mut self) {
        use windows::Win32::{Foundation::CloseHandle, System::Threading::ReleaseMutex};

        let _ = unsafe { ReleaseMutex(self.handle) };
        let _ = unsafe { CloseHandle(self.handle) };
    }
}

/// Take the global settings write lock, or fail closed while another process
/// holds it.
pub(crate) fn settings_write() -> io::Result<Guard> {
    let name = settings_write_lock_name()?;
    lock_for_settings(&name)
}

/// The settings lock a test writing to `path` must share with production code
/// writing the same file. Production uses the one global name.
pub(crate) fn settings_write_for_path(path: &Path) -> io::Result<Guard> {
    #[cfg(test)]
    {
        let root = path
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
            .unwrap_or(path);
        settings_write_for_test(root)
    }
    #[cfg(not(test))]
    {
        let _ = path;
        settings_write()
    }
}

/// The settings lock scoped to one test root, so unrelated test directories do
/// not serialize.
#[cfg(test)]
pub(crate) fn settings_write_for_test(root: &Path) -> io::Result<Guard> {
    #[cfg(target_os = "windows")]
    {
        use std::hash::{Hash, Hasher};

        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        root.hash(&mut hasher);
        lock_for_settings(&format!(
            "{TEST_SETTINGS_WRITE_LOCK_PREFIX}{:016x}",
            hasher.finish()
        ))
    }
    #[cfg(not(target_os = "windows"))]
    {
        let _ = root;
        settings_write()
    }
}

/// Take the credential operation lock, or fail closed while another process
/// holds it.
pub(crate) fn credential_operation() -> Result<Guard, LockError> {
    #[cfg(target_os = "windows")]
    {
        lock_named(CREDENTIAL_OPERATION_LOCK)
    }
    #[cfg(not(target_os = "windows"))]
    {
        Ok(lock_process(Resource::Credentials))
    }
}

/// The credential lock for one fake store, so credentials written through
/// different stores in one test binary do not contend.
#[cfg(all(any(test, feature = "test-support"), target_os = "windows"))]
pub(crate) fn credential_operation_for_test(scope: usize) -> Result<Guard, LockError> {
    lock_named(&format!(
        "{TEST_CREDENTIAL_OPERATION_LOCK_PREFIX}{}-{scope:x}",
        std::process::id()
    ))
}

#[cfg(target_os = "windows")]
fn lock_for_settings(name: &str) -> io::Result<Guard> {
    lock_named(name).map_err(LockError::into_settings_io)
}

#[cfg(not(target_os = "windows"))]
fn lock_for_settings(name: &str) -> io::Result<Guard> {
    let _ = name;
    Ok(lock_process(Resource::Settings))
}

#[cfg(target_os = "windows")]
fn lock_named(name: &str) -> Result<Guard, LockError> {
    use windows::Win32::{
        Foundation::{CloseHandle, GetLastError, WAIT_ABANDONED, WAIT_OBJECT_0, WAIT_TIMEOUT},
        System::Threading::{CreateMutexW, WaitForSingleObject},
    };
    use windows::core::PCWSTR;

    let name: Vec<u16> = name.encode_utf16().chain(std::iter::once(0)).collect();
    let handle = unsafe { CreateMutexW(None, false, PCWSTR(name.as_ptr())) }
        .map_err(|error| LockError::Os(error.code().0))?;
    let wait = unsafe { WaitForSingleObject(handle, WAIT_MILLISECONDS) };
    if wait == WAIT_OBJECT_0 || wait == WAIT_ABANDONED {
        return Ok(Guard { handle });
    }

    let error = if wait == WAIT_TIMEOUT {
        LockError::Busy
    } else {
        LockError::Os(unsafe { GetLastError() }.0 as i32)
    };
    let _ = unsafe { CloseHandle(handle) };
    Err(error)
}

#[cfg(not(target_os = "windows"))]
enum Resource {
    Settings,
    Credentials,
}

#[cfg(not(target_os = "windows"))]
fn lock_process(resource: Resource) -> Guard {
    static SETTINGS: std::sync::Mutex<()> = std::sync::Mutex::new(());
    static CREDENTIALS: std::sync::Mutex<()> = std::sync::Mutex::new(());
    let mutex = match resource {
        Resource::Settings => &SETTINGS,
        Resource::Credentials => &CREDENTIALS,
    };
    Guard {
        _guard: mutex
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()),
    }
}

#[derive(Debug)]
enum SettingsWriteLockName {
    Global,
    Test(String),
}

impl SettingsWriteLockName {
    fn from_process_environment() -> io::Result<Self> {
        match std::env::var(TEST_LOCK_NAMESPACE_ENV) {
            Ok(namespace) => Self::from_test_namespace(&namespace),
            Err(std::env::VarError::NotPresent) => Ok(Self::Global),
            Err(std::env::VarError::NotUnicode(_)) => Err(Self::invalid_test_namespace()),
        }
    }

    fn from_test_namespace(namespace: &str) -> io::Result<Self> {
        let length_is_valid = (1..=TEST_LOCK_NAMESPACE_MAX_LENGTH).contains(&namespace.len());
        let characters_are_valid = namespace
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-');
        if length_is_valid && characters_are_valid {
            Ok(Self::Test(namespace.to_owned()))
        } else {
            Err(Self::invalid_test_namespace())
        }
    }

    fn into_lock_name(self) -> String {
        match self {
            Self::Global => SETTINGS_WRITE_LOCK.to_owned(),
            Self::Test(namespace) => format!("{TEST_SETTINGS_WRITE_LOCK_PREFIX}{namespace}"),
        }
    }

    fn invalid_test_namespace() -> io::Error {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("{TEST_LOCK_NAMESPACE_ENV} must be 1-64 ASCII letters, digits, `_` or `-`"),
        )
    }
}

fn settings_write_lock_name() -> io::Result<String> {
    SettingsWriteLockName::from_process_environment().map(SettingsWriteLockName::into_lock_name)
}

#[cfg(all(test, target_os = "windows"))]
pub(crate) fn test_lock_named(name: &str) -> io::Result<Guard> {
    lock_named(name).map_err(LockError::into_settings_io)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(target_os = "windows")]
    #[test]
    fn settings_writes_fail_closed_while_the_global_lock_is_busy() {
        let name = format!(
            "Local\\markturbo-settings-write-test-{}",
            std::process::id()
        );
        let _guard = test_lock_named(&name).unwrap();
        let error =
            std::thread::spawn(move || test_lock_named(&name).err().expect("the lock is busy"))
                .join()
                .unwrap();

        assert_eq!(error.kind(), std::io::ErrorKind::WouldBlock);
    }

    #[test]
    fn settings_write_lock_names_follow_the_global_and_test_contract() {
        assert_eq!(
            SettingsWriteLockName::Global.into_lock_name(),
            "Global\\markturbo-settings-write-v1"
        );
        assert_eq!(
            SettingsWriteLockName::from_test_namespace("alpha-1_B")
                .expect("a valid namespace")
                .into_lock_name(),
            "Local\\markturbo-settings-write-test-alpha-1_B"
        );
        assert!(
            SettingsWriteLockName::from_test_namespace(&"a".repeat(64)).is_ok(),
            "64 characters is the maximum"
        );
    }

    #[test]
    fn invalid_test_lock_namespaces_fail_closed() {
        for namespace in [
            "",
            "   ",
            "with space",
            "with\\slash",
            "with/slash",
            "unicode-é",
        ] {
            let error = SettingsWriteLockName::from_test_namespace(namespace)
                .expect_err("an invalid namespace");
            assert_eq!(
                error.kind(),
                std::io::ErrorKind::InvalidInput,
                "{namespace:?}"
            );
        }
        assert!(
            SettingsWriteLockName::from_test_namespace(&"a".repeat(65)).is_err(),
            "65 characters exceeds the maximum"
        );
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn different_test_namespaces_do_not_contend() {
        let process = std::process::id().to_string();
        let first_name = SettingsWriteLockName::from_test_namespace(&format!("first-{process}"))
            .unwrap()
            .into_lock_name();
        let second_name = SettingsWriteLockName::from_test_namespace(&format!("second-{process}"))
            .unwrap()
            .into_lock_name();
        let first = test_lock_named(&first_name).unwrap();

        let (second, first_error) = std::thread::scope(|scope| {
            let second = scope.spawn(|| test_lock_named(&second_name).map(|_| ()));
            let first_probe = scope.spawn(|| test_lock_named(&first_name).err());
            (second.join().unwrap(), first_probe.join().unwrap())
        });
        assert!(second.is_ok(), "a different namespace must not contend");
        assert_eq!(
            first_error.expect("the same namespace is busy").kind(),
            std::io::ErrorKind::WouldBlock
        );
        drop(first);
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn the_test_namespace_environment_selects_an_isolated_write_lock() {
        // SAFETY: no other test in this binary reads the variable.
        unsafe { std::env::remove_var(TEST_LOCK_NAMESPACE_ENV) };
        assert_eq!(
            settings_write_lock_name().unwrap(),
            "Global\\markturbo-settings-write-v1"
        );
        let global = test_lock_named(&SettingsWriteLockName::Global.into_lock_name()).unwrap();
        let global_error =
            std::thread::spawn(|| settings_write().err().expect("the global lock is busy"))
                .join()
                .unwrap();
        assert_eq!(global_error.kind(), std::io::ErrorKind::WouldBlock);

        let namespace = format!("environment-selected-{}", std::process::id());
        unsafe { std::env::set_var(TEST_LOCK_NAMESPACE_ENV, &namespace) };
        assert_eq!(
            settings_write_lock_name().unwrap(),
            format!("Local\\markturbo-settings-write-test-{namespace}")
        );
        let selected_name = settings_write_lock_name().unwrap();
        let uncontended = std::thread::spawn(|| settings_write().map(|_| ()))
            .join()
            .unwrap();
        assert!(
            uncontended.is_ok(),
            "the selected namespace must be free while only the global lock is held: {uncontended:?}"
        );

        let selected = test_lock_named(&selected_name).unwrap();
        drop(global);
        let selected_error =
            std::thread::spawn(|| settings_write().err().expect("the selected lock is busy"))
                .join()
                .unwrap();
        assert_eq!(selected_error.kind(), std::io::ErrorKind::WouldBlock);
        drop(selected);

        unsafe { std::env::set_var(TEST_LOCK_NAMESPACE_ENV, "invalid value") };
        let error = settings_write().err().expect("an invalid namespace");
        assert_eq!(error.kind(), std::io::ErrorKind::InvalidInput);

        // SAFETY: restore the environment for the rest of the binary.
        unsafe { std::env::remove_var(TEST_LOCK_NAMESPACE_ENV) };
    }
}
