//! The companion CJK face: Source Han Sans CN VF 2.005 (Adobe, SIL Open Font
//! License 1.1), one variable OpenType file whose named instances cover the
//! weights the UI sets (300, 400, 500).
//!
//! It is not shipped in the executable. It is fetched only when the person
//! asks for it, from the one pinned URL, and kept privately in MarkTurbo's
//! data directory (`<data>/fonts/`); nothing is installed system-wide. The
//! request carries no document content, and no part of this module runs it
//! by itself: no background, startup or retried request.
//!
//! A file is trusted only after its size and SHA-256 match the pinned ones.
//! A download streams into a temporary file in the data directory, hard
//! capped at the pinned size, and is renamed into `fonts/` only once verified.
//! A stored file that no longer verifies is never loaded; failure to remove
//! it is reported.

use std::fs::{self, OpenOptions};
use std::io::{self, Read as _};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU8, Ordering};

use sha2::{Digest as _, Sha256};

/// The pinned download: the 2.005R release's Simplified Chinese subset.
pub const URL: &str = "https://raw.githubusercontent.com/adobe-fonts/source-han-sans/2.005R/Variable/OTF/Subset/SourceHanSansCN-VF.otf";
/// The host [`URL`] names, as the disclosure states it.
pub const HOST: &str = "raw.githubusercontent.com";
/// The pinned file's size in bytes.
pub const SIZE: u64 = 15_636_088;
/// The pinned file's SHA-256, lowercase hex.
pub const SHA256: &str = "ad10f878ea1af97d66ec8bd10eb39798784f3a1ee3d5987a570ef6083f941ef3";
/// The licence the face is distributed under.
pub const LICENSE: &str = "SIL Open Font License 1.1";
/// The file's name in the fonts directory.
pub const FILE_NAME: &str = "SourceHanSansCN-VF.otf";
const PARTIAL_PREFIX: &str = ".companion-font-";
const PARTIAL_SUFFIX: &str = ".part";
#[cfg(feature = "model-transport")]
const STALL_LIMIT: std::time::Duration = std::time::Duration::from_secs(30);

/// The companion font in one data directory: where it lives, and the source
/// and digest it must match.
///
/// Every disk operation on the companion file holds the lifecycle lock;
/// only the network fetch runs outside it.
///
/// Built from the pinned constants ([`CompanionFont::in_data_dir`]); only a
/// test build can name another source ([`CompanionFont::with_source`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompanionFont {
    dir: PathBuf,
    url: String,
    size: u64,
    sha256: String,
}

/// Why a download did not install the font. Content-free: it names the
/// stage, never a path, a URL or a byte of the response.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DownloadError {
    /// No answer, a failed or non-success response, or a stalled stream.
    Network,
    /// The response was not the pinned file: wrong size or wrong digest.
    Verification,
    /// The file could not be saved or read back.
    Disk,
    /// The newly installed file could not be removed after a failed read-back.
    Removal,
    /// The person cancelled it.
    Cancelled,
}

impl std::fmt::Display for DownloadError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::Network => "the font download did not complete",
            Self::Verification => "the downloaded file is not the pinned font",
            Self::Disk => "the font could not be saved or read back",
            Self::Removal => "the font could not be removed",
            Self::Cancelled => "the font download was cancelled",
        })
    }
}

impl std::error::Error for DownloadError {}

/// Cancellation for one companion-font download.
///
/// Cancellation succeeds only before the verified file begins committing.
#[derive(Debug, Default)]
pub struct DownloadControl(AtomicU8);

impl DownloadControl {
    const ACTIVE: u8 = 0;
    const CANCELLED: u8 = 1;
    const COMMITTING: u8 = 2;

    /// Marks the download cancelled if file commit has not started.
    pub fn cancel(&self) -> bool {
        self.0
            .compare_exchange(
                Self::ACTIVE,
                Self::CANCELLED,
                Ordering::AcqRel,
                Ordering::Acquire,
            )
            .is_ok()
    }

    /// Whether the final file-install phase has begun.
    pub fn is_committing(&self) -> bool {
        self.0.load(Ordering::Acquire) == Self::COMMITTING
    }

    #[cfg(feature = "model-transport")]
    fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::Acquire) == Self::CANCELLED
    }

    #[cfg(feature = "model-transport")]
    fn begin_commit(&self) -> Result<(), DownloadError> {
        match self.0.compare_exchange(
            Self::ACTIVE,
            Self::COMMITTING,
            Ordering::AcqRel,
            Ordering::Acquire,
        ) {
            Ok(_) => Ok(()),
            Err(_) => Err(DownloadError::Cancelled),
        }
    }
}

impl CompanionFont {
    /// The pinned font below `data_dir` (`<data_dir>/fonts/`).
    pub fn in_data_dir(data_dir: &Path) -> Self {
        Self {
            dir: data_dir.join("fonts"),
            url: URL.to_owned(),
            size: SIZE,
            sha256: SHA256.to_owned(),
        }
    }

    /// The pinned font in this user's data directory
    /// ([`crate::runtime_paths::data_dir`]); `None` without one.
    pub fn locate() -> Option<Self> {
        Some(Self::in_data_dir(&crate::runtime_paths::data_dir()?))
    }

    /// A test's own source: `url` serves the file, and `expected` is the
    /// exact file that must arrive. Not compiled into a release build.
    #[cfg(any(test, feature = "test-support"))]
    pub fn with_source(data_dir: &Path, url: &str, expected: &[u8]) -> Self {
        Self {
            dir: data_dir.join("fonts"),
            url: url.to_owned(),
            size: expected.len() as u64,
            sha256: hex_digest(expected),
        }
    }

    /// The size the file must have, in bytes (the progress total).
    pub fn size(&self) -> u64 {
        self.size
    }

    /// Where the verified file lives.
    pub fn path(&self) -> PathBuf {
        self.dir.join(FILE_NAME)
    }

    /// Removes abandoned partial downloads left behind by a crashed process.
    pub fn cleanup_stale_parts(&self) -> io::Result<()> {
        let data_dir = self.data_dir();
        match fs::metadata(data_dir) {
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
            Err(error) => return Err(error),
            Ok(_) => {}
        }
        let _lifecycle = self.lock_lifecycle()?;
        let entries = match fs::read_dir(data_dir) {
            Ok(entries) => entries,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
            Err(error) => return Err(error),
        };
        for entry in entries {
            let entry = entry?;
            let name = entry.file_name();
            let Some(name) = name.to_str() else {
                continue;
            };
            if !name.starts_with(PARTIAL_PREFIX)
                || !name.ends_with(PARTIAL_SUFFIX)
                || !entry.file_type()?.is_file()
            {
                continue;
            }
            let path = entry.path();
            let file = match OpenOptions::new().read(true).write(true).open(&path) {
                Ok(file) => file,
                Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
                Err(error) => return Err(error),
            };
            match file.try_lock() {
                Ok(()) => {}
                Err(std::fs::TryLockError::WouldBlock) => continue,
                Err(std::fs::TryLockError::Error(error)) => return Err(error),
            }
            // A live download locks its part before releasing the lifecycle lock.
            drop(file);
            match fs::remove_file(path) {
                Err(error) if error.kind() != io::ErrorKind::NotFound => return Err(error),
                _ => {}
            }
        }
        Ok(())
    }

    /// The verified font, for registration: `None` when it is absent, or when
    /// a file that does not match the pinned size and digest has been removed.
    /// Returns an error if the stored file cannot be read or discarded.
    pub fn load(&self) -> io::Result<Option<Vec<u8>>> {
        let _lock = match self.lock_lifecycle() {
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
            result => result?,
        };
        match self.read_verified(&self.path()) {
            Ok(Some(bytes)) => Ok(Some(bytes)),
            Ok(None) => {
                log::warn!("companion font: the stored file does not verify; removing it");
                self.remove_unlocked()?;
                Ok(None)
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(error),
        }
    }

    fn read_verified(&self, path: &Path) -> io::Result<Option<Vec<u8>>> {
        if fs::metadata(path)?.len() != self.size {
            return Ok(None);
        }
        let mut bytes = Vec::new();
        // The file can change after the metadata check; read at most one extra byte.
        fs::File::open(path)?
            .take(self.size + 1)
            .read_to_end(&mut bytes)?;
        Ok(self.verifies(&bytes).then_some(bytes))
    }

    /// Deletes the file; an absent file is already removed.
    pub fn remove(&self) -> io::Result<()> {
        let _lock = match self.lock_lifecycle() {
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
            result => result?,
        };
        self.remove_unlocked()
    }

    fn remove_unlocked(&self) -> io::Result<()> {
        match fs::remove_file(self.path()) {
            Err(error) if error.kind() != io::ErrorKind::NotFound => Err(error),
            _ => Ok(()),
        }
    }

    fn data_dir(&self) -> &Path {
        self.dir
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
            .unwrap_or_else(|| Path::new("."))
    }

    /// Locks the companion-file lifecycle across cooperating processes. The
    /// sentinel stays beside `fonts/`; a missing data directory means there is
    /// no companion file to inspect or remove.
    fn lock_lifecycle(&self) -> io::Result<fs::File> {
        let lock_path = self.data_dir().join(".companion-font.lock");
        let lock = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(lock_path)?;
        lock.lock()?;
        Ok(lock)
    }

    fn verifies(&self, bytes: &[u8]) -> bool {
        bytes.len() as u64 == self.size && hex_digest(bytes) == self.sha256
    }

    /// Downloads, verifies and installs the font, returning its read-back bytes.
    /// Blocking: run it off the UI thread. `progress` hears the bytes received so far (of
    /// [`Self::size`]); `control.cancel()` stops it before file commit begins.
    ///
    /// The body streams into a temporary file in the data directory and is
    /// abandoned as soon as it exceeds the pinned size; it is renamed into
    /// the fonts directory only after its size and digest match. Any failure,
    /// including a cancel, leaves no part and an existing installed copy intact.
    /// A failed read-back rolls back a new install, unless removal also fails.
    #[cfg(feature = "model-transport")]
    pub fn download(
        &self,
        progress: &dyn Fn(u64),
        control: &DownloadControl,
    ) -> Result<Vec<u8>, DownloadError> {
        self.download_with_hooks(progress, control, || {}, |_| {})
    }

    #[cfg(feature = "model-transport")]
    fn download_with_hooks(
        &self,
        progress: &dyn Fn(u64),
        control: &DownloadControl,
        after_fetch: impl FnOnce(),
        after_commit: impl FnOnce(&Path),
    ) -> Result<Vec<u8>, DownloadError> {
        let runtime = crate::model::transport::runtime().map_err(|_| DownloadError::Network)?;
        fs::create_dir_all(&self.dir).map_err(|_| DownloadError::Disk)?;
        let lifecycle = self.lock_lifecycle().map_err(|_| DownloadError::Disk)?;
        let mut part = tempfile::Builder::new()
            .prefix(PARTIAL_PREFIX)
            .suffix(PARTIAL_SUFFIX)
            .tempfile_in(self.data_dir())
            .map_err(|_| DownloadError::Disk)?;
        part.as_file().lock().map_err(|_| DownloadError::Disk)?;
        drop(lifecycle);
        let digest = runtime.block_on(self.fetch(part.as_file_mut(), progress, control))?;
        after_fetch();
        if digest != self.sha256 {
            return Err(DownloadError::Verification);
        }
        let _lock = self.lock_lifecycle().map_err(|_| DownloadError::Disk)?;
        part.as_file().sync_all().map_err(|_| DownloadError::Disk)?;
        control.begin_commit()?;
        let path = self.path();
        // Keep any verified copy another process installed, including during this download.
        match self.read_verified(&path) {
            Ok(Some(bytes)) => return Ok(bytes),
            Ok(None) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(_) => return Err(DownloadError::Disk),
        }
        part.persist(&path).map_err(|_| DownloadError::Disk)?;
        after_commit(&path);
        let failure = match self.read_verified(&path) {
            Ok(Some(bytes)) => return Ok(bytes),
            Ok(None) => DownloadError::Verification,
            Err(error) if error.kind() == io::ErrorKind::NotFound => DownloadError::Verification,
            Err(_) => DownloadError::Disk,
        };
        // Keep the lock through rollback so we only remove our own commit.
        self.remove_unlocked().map_err(|_| DownloadError::Removal)?;
        Err(failure)
    }

    /// Streams the response into `file`, returning the body's digest once
    /// exactly [`Self::size`] bytes arrived.
    #[cfg(feature = "model-transport")]
    async fn fetch(
        &self,
        file: &mut fs::File,
        progress: &dyn Fn(u64),
        control: &DownloadControl,
    ) -> Result<String, DownloadError> {
        use std::io::Write as _;

        let client = font_client_builder(&self.url)
            .connect_timeout(STALL_LIMIT)
            .build()
            .map_err(|_| DownloadError::Network)?;
        let mut response = until_cancelled(control, client.get(&self.url).send())
            .await?
            .map_err(|_| DownloadError::Network)?;
        if !response.status().is_success() {
            return Err(DownloadError::Network);
        }
        if response
            .content_length()
            .is_some_and(|length| length != self.size)
        {
            return Err(DownloadError::Verification);
        }
        let mut digest = Sha256::new();
        let mut received = 0_u64;
        while let Some(chunk) = until_cancelled(control, response.chunk())
            .await?
            .map_err(|_| DownloadError::Network)?
        {
            received += chunk.len() as u64;
            if received > self.size {
                return Err(DownloadError::Verification);
            }
            file.write_all(&chunk).map_err(|_| DownloadError::Disk)?;
            digest.update(&chunk);
            progress(received);
        }
        if received != self.size {
            return Err(DownloadError::Verification);
        }
        Ok(format!("{:x}", digest.finalize()))
    }
}

/// How often a waiting download looks at its cancel flag.
#[cfg(feature = "model-transport")]
const CANCEL_TICK: std::time::Duration = std::time::Duration::from_millis(100);

/// `future`'s output, unless `cancel` is set first or it stalls past
/// [`STALL_LIMIT`].
#[cfg(feature = "model-transport")]
async fn until_cancelled<F: std::future::Future>(
    control: &DownloadControl,
    future: F,
) -> Result<F::Output, DownloadError> {
    let mut future = std::pin::pin!(future);
    let mut waited = std::time::Duration::ZERO;
    loop {
        if control.is_cancelled() {
            return Err(DownloadError::Cancelled);
        }
        match tokio::time::timeout(CANCEL_TICK, future.as_mut()).await {
            Ok(output) => return Ok(output),
            Err(_) if waited >= STALL_LIMIT => return Err(DownloadError::Network),
            Err(_) => waited += CANCEL_TICK,
        }
    }
}

/// The system proxy for the pinned remote host; none for a loopback source,
/// as the model transport treats loopback endpoints.
#[cfg(feature = "model-transport")]
fn proxy_policy(url: &str) -> crate::model::transport::ProxyPolicy {
    use crate::model::transport::ProxyPolicy;
    let loopback = url::Url::parse(url).is_ok_and(|url| match url.host() {
        Some(url::Host::Ipv4(ip)) => ip.is_loopback(),
        Some(url::Host::Ipv6(ip)) => ip.is_loopback(),
        Some(url::Host::Domain(name)) => name.eq_ignore_ascii_case("localhost"),
        None => false,
    });
    if loopback {
        ProxyPolicy::Disabled
    } else {
        ProxyPolicy::System
    }
}

#[cfg(feature = "model-transport")]
fn font_client_builder(url: &str) -> reqwest::ClientBuilder {
    crate::model::transport::http_client_builder(proxy_policy(url)).retry(reqwest::retry::never())
}

fn hex_digest(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_pinned_source_is_the_2_005_release_over_https() {
        let font = CompanionFont::in_data_dir(Path::new("data"));
        assert_eq!(font.path(), Path::new("data").join("fonts").join(FILE_NAME));
        assert_eq!(font.size(), 15_636_088);
        assert!(URL.starts_with(&format!("https://{HOST}/")), "{URL}");
        assert!(URL.contains("/2.005R/"), "{URL}");
        assert!(URL.ends_with(FILE_NAME), "{URL}");
        assert_eq!(
            URL,
            "https://raw.githubusercontent.com/adobe-fonts/source-han-sans/2.005R/Variable/OTF/Subset/SourceHanSansCN-VF.otf"
        );
        assert_eq!(
            SHA256,
            "ad10f878ea1af97d66ec8bd10eb39798784f3a1ee3d5987a570ef6083f941ef3"
        );
        assert_eq!(SHA256.len(), 64);
        assert!(
            SHA256
                .bytes()
                .all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
        );
    }

    #[test]
    fn a_stored_file_that_does_not_verify_is_absent_and_removed() {
        let data = tempfile::tempdir().unwrap();
        let missing_data = data.path().join("not-created");
        let missing = CompanionFont::with_source(&missing_data, "http://127.0.0.1:9/", b"the font");
        missing.cleanup_stale_parts().unwrap();
        assert!(
            !missing_data.exists(),
            "cleaning up does not create its data root"
        );
        assert_eq!(missing.load().unwrap(), None);
        missing.remove().expect("an absent font is already removed");
        assert!(
            !missing_data.exists(),
            "removing does not create its data root"
        );

        let font = CompanionFont::with_source(data.path(), "http://127.0.0.1:9/", b"the font");
        assert_eq!(font.load().unwrap(), None, "absent");
        assert!(!font.path().try_exists().unwrap());

        fs::create_dir_all(font.path().parent().unwrap()).unwrap();
        fs::write(font.path(), b"the fonT").unwrap();
        assert!(font.path().is_file(), "present, not yet verified");
        assert_eq!(font.load().unwrap(), None, "a damaged copy is absent");
        assert!(!font.path().exists(), "and removed");

        fs::write(font.path(), b"the font").unwrap();
        assert_eq!(font.load().unwrap().as_deref(), Some(&b"the font"[..]));
        font.remove().unwrap();
        assert!(!font.path().try_exists().unwrap());
        font.remove().expect("removing an absent font is fine");
    }

    #[test]
    fn a_stored_file_of_the_wrong_size_is_removed_without_reading_it() {
        let data = tempfile::tempdir().unwrap();
        let font = CompanionFont::with_source(data.path(), "http://127.0.0.1:9/", b"font");
        fs::create_dir_all(&font.dir).unwrap();
        fs::write(font.path(), b"short font fixture").unwrap();

        #[cfg(windows)]
        let held = {
            use std::os::windows::fs::OpenOptionsExt as _;

            const FILE_SHARE_DELETE: u32 = 4;
            let file = OpenOptions::new()
                .write(true)
                .share_mode(FILE_SHARE_DELETE)
                .open(font.path())
                .unwrap();
            assert!(fs::File::open(font.path()).is_err());
            file
        };

        assert_eq!(font.load().unwrap(), None);
        #[cfg(windows)]
        drop(held);
        assert!(!font.path().exists(), "the wrong-size file was removed");
    }

    fn set_modified(path: &Path, modified: std::time::SystemTime) {
        OpenOptions::new()
            .write(true)
            .open(path)
            .unwrap()
            .set_times(fs::FileTimes::new().set_modified(modified))
            .unwrap();
    }

    #[test]
    fn startup_cleanup_reports_a_data_root_that_is_a_file() {
        let data = tempfile::tempdir().unwrap();
        let data_root = data.path().join("data-root");
        fs::write(&data_root, b"not a directory").unwrap();
        let font = CompanionFont::with_source(&data_root, "http://127.0.0.1:9/", b"font");

        font.cleanup_stale_parts()
            .expect_err("an existing data root that is a file is not an absent directory");
    }

    #[test]
    fn startup_cleanup_removes_stale_part_and_keeps_other_entries() {
        let data = tempfile::tempdir().unwrap();
        let font = CompanionFont::with_source(data.path(), "http://127.0.0.1:9/", b"font");
        let stale = data.path().join(".companion-font-stale.part");
        fs::write(&stale, b"unfinished").unwrap();
        set_modified(&stale, std::time::SystemTime::UNIX_EPOCH);
        let unrelated = data.path().join("unrelated.part");
        fs::write(&unrelated, b"unrelated").unwrap();
        let directory = data.path().join(".companion-font-directory.part");
        fs::create_dir(&directory).unwrap();

        font.cleanup_stale_parts().unwrap();

        assert!(!stale.exists(), "stale partial download was removed");
        assert!(unrelated.is_file(), "an unrelated file was kept");
        assert!(directory.is_dir(), "a matching directory was kept");
    }

    #[test]
    fn startup_cleanup_removes_a_fresh_unlocked_part() {
        let data = tempfile::tempdir().unwrap();
        let font = CompanionFont::with_source(data.path(), "http://127.0.0.1:9/", b"font");
        let fresh = data.path().join(".companion-font-fresh.part");
        fs::write(&fresh, b"unfinished").unwrap();

        font.cleanup_stale_parts().unwrap();

        assert!(!fresh.exists(), "a newly abandoned part was removed");
    }

    #[test]
    fn startup_cleanup_keeps_a_locked_stale_part() {
        let data = tempfile::tempdir().unwrap();
        let font = CompanionFont::with_source(data.path(), "http://127.0.0.1:9/", b"font");
        let active = data.path().join(".companion-font-active.part");
        fs::write(&active, b"still writing").unwrap();
        set_modified(&active, std::time::SystemTime::UNIX_EPOCH);

        let (locked_tx, locked_rx) = std::sync::mpsc::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        let lock_path = active.clone();
        let holder = std::thread::spawn(move || {
            let file = OpenOptions::new()
                .read(true)
                .write(true)
                .open(lock_path)
                .unwrap();
            file.lock().unwrap();
            locked_tx.send(()).unwrap();
            release_rx.recv().unwrap();
        });
        locked_rx.recv().unwrap();

        font.cleanup_stale_parts().unwrap();

        assert!(
            active.exists(),
            "a locked part survives even with stale mtime"
        );
        release_tx.send(()).unwrap();
        holder.join().unwrap();
        font.cleanup_stale_parts().unwrap();
        assert!(!active.exists(), "an unlocked part is removed");
    }

    #[cfg(feature = "model-transport")]
    mod retry {
        use super::font_client_builder;
        use std::io::{Read as _, Write as _};
        use std::net::TcpListener;
        use std::sync::atomic::AtomicUsize;
        use std::time::Duration;

        #[test]
        fn a_refused_http2_font_request_is_not_retried() {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let url = format!("http://{}/font.otf", listener.local_addr().unwrap());
            let requests = std::sync::Arc::new(AtomicUsize::new(0));
            let server_requests = requests.clone();
            let server = std::thread::spawn(move || {
                let (mut stream, _) = listener.accept().unwrap();
                stream
                    .set_read_timeout(Some(Duration::from_secs(2)))
                    .unwrap();
                let mut preface = [0; 24];
                stream.read_exact(&mut preface).unwrap();
                assert_eq!(&preface, b"PRI * HTTP/2.0\r\n\r\nSM\r\n\r\n");
                stream.write_all(&[0, 0, 0, 4, 0, 0, 0, 0, 0]).unwrap();

                loop {
                    let mut header = [0; 9];
                    if stream.read_exact(&mut header).is_err() {
                        break;
                    }
                    let length = (usize::from(header[0]) << 16)
                        | (usize::from(header[1]) << 8)
                        | usize::from(header[2]);
                    let mut payload = vec![0; length];
                    if stream.read_exact(&mut payload).is_err() {
                        break;
                    }
                    if header[3] == 4 && header[4] & 1 == 0 {
                        stream.write_all(&[0, 0, 0, 4, 1, 0, 0, 0, 0]).unwrap();
                    } else if header[3] == 1 {
                        server_requests.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                        let stream_id =
                            u32::from_be_bytes(header[5..9].try_into().unwrap()) & 0x7fff_ffff;
                        let mut refused = [0; 13];
                        refused[2] = 4;
                        refused[3] = 3;
                        refused[5..9].copy_from_slice(&stream_id.to_be_bytes());
                        refused[12] = 7;
                        stream.write_all(&refused).unwrap();
                    }
                }
            });

            let result = crate::model::transport::runtime().unwrap().block_on(async {
                let client = font_client_builder(&url)
                    .http2_prior_knowledge()
                    .timeout(Duration::from_secs(2))
                    .build()
                    .unwrap();
                let result = client.get(url).send().await;
                drop(client);
                result
            });
            assert!(result.is_err());
            server.join().unwrap();
            assert_eq!(requests.load(std::sync::atomic::Ordering::SeqCst), 1);
        }
    }

    #[cfg(feature = "model-transport")]
    mod download {
        use super::*;
        use std::io::{BufRead as _, BufReader, Write as _};
        use std::net::TcpListener;
        use std::sync::Mutex;

        const FONT: &[u8] = b"pretend this is an OpenType file of the pinned size";

        /// What the loopback server answers every request with.
        enum Reply {
            /// `200` with this body and its true `Content-Length`.
            Body(Vec<u8>),
            /// `200` with this body and no length: it ends when the
            /// connection closes, so only the size cap can stop it.
            Unsized(Vec<u8>),
            /// This status, empty.
            Status(u16),
        }

        /// A loopback HTTP server answering every request with `reply`; its
        /// base URL.
        fn serve(reply: Reply) -> String {
            let listener = TcpListener::bind("127.0.0.1:0").expect("bind loopback");
            let url = format!("http://{}/font.otf", listener.local_addr().unwrap());
            std::thread::spawn(move || {
                for stream in listener.incoming() {
                    let Ok(mut stream) = stream else { return };
                    let mut reader = BufReader::new(stream.try_clone().unwrap());
                    let mut line = String::new();
                    while reader.read_line(&mut line).is_ok_and(|n| n > 0) && line != "\r\n" {
                        line.clear();
                    }
                    let _ = match &reply {
                        Reply::Body(body) => write!(
                            stream,
                            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                            body.len()
                        )
                        .and_then(|()| stream.write_all(body)),
                        Reply::Unsized(body) => stream
                            .write_all(b"HTTP/1.1 200 OK\r\nConnection: close\r\n\r\n")
                            .and_then(|()| stream.write_all(body)),
                        Reply::Status(code) => write!(
                            stream,
                            "HTTP/1.1 {code} Nope\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                        ),
                    };
                }
            });
            url
        }

        /// Downloads from `reply` into a fresh data directory; the result,
        /// the progress heard, and what the fonts directory holds after.
        fn download(reply: Reply) -> (Result<Vec<u8>, DownloadError>, Vec<u64>, Vec<String>) {
            let data = tempfile::tempdir().unwrap();
            let font = CompanionFont::with_source(data.path(), &serve(reply), FONT);
            let heard = Mutex::new(Vec::new());
            let result = font.download(
                &|received| {
                    assert_eq!(
                        fs::read_dir(&font.dir).unwrap().count(),
                        0,
                        "fonts contains no file during the download"
                    );
                    heard.lock().unwrap().push(received);
                },
                &DownloadControl::default(),
            );
            let left = fs::read_dir(data.path().join("fonts"))
                .map(|entries| {
                    entries
                        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
                        .collect()
                })
                .unwrap_or_default();
            assert_no_staged_parts(data.path());
            if result.is_ok() {
                assert_eq!(
                    font.load().unwrap().as_deref(),
                    Some(FONT),
                    "installed, verified"
                );
            }
            (result, heard.into_inner().unwrap(), left)
        }

        fn assert_no_staged_parts(data_dir: &Path) {
            assert!(fs::read_dir(data_dir).unwrap().all(|entry| {
                let name = entry.unwrap().file_name();
                !name.to_string_lossy().starts_with(PARTIAL_PREFIX)
            }));
        }

        #[test]
        fn the_pinned_bytes_are_installed_and_verified() {
            let (result, heard, left) = download(Reply::Body(FONT.to_vec()));
            assert_eq!(result.as_deref(), Ok(FONT));
            assert_eq!(left, [FILE_NAME]);
            assert_eq!(heard.last().copied(), Some(FONT.len() as u64), "{heard:?}");
            assert!(heard.windows(2).all(|pair| pair[0] < pair[1]), "{heard:?}");
        }

        #[test]
        fn other_bytes_of_the_same_size_are_rejected_and_leave_nothing() {
            let mut wrong = FONT.to_vec();
            wrong[0] ^= 1;
            let (result, _, left) = download(Reply::Body(wrong));
            assert_eq!(result, Err(DownloadError::Verification));
            assert!(left.is_empty(), "{left:?}");
        }

        #[test]
        fn an_oversized_stream_is_aborted_and_leaves_nothing() {
            let mut long = FONT.to_vec();
            long.extend(std::iter::repeat_n(b'x', 64 * 1024));
            // A declared length that is not the pinned one is refused before
            // the body is read; an undeclared one is cut at the cap.
            for reply in [Reply::Body(long.clone()), Reply::Unsized(long)] {
                let (result, heard, left) = download(reply);
                assert_eq!(result, Err(DownloadError::Verification));
                assert!(heard.iter().all(|&n| n <= FONT.len() as u64), "{heard:?}");
                assert!(left.is_empty(), "{left:?}");
            }
        }

        #[test]
        fn a_short_stream_or_a_failed_response_leaves_nothing() {
            let (result, _, left) = download(Reply::Unsized(FONT[..8].to_vec()));
            assert_eq!(result, Err(DownloadError::Verification));
            assert!(left.is_empty(), "{left:?}");

            let (result, _, left) = download(Reply::Status(404));
            assert_eq!(result, Err(DownloadError::Network));
            assert!(left.is_empty(), "{left:?}");
        }

        #[test]
        fn a_failed_download_keeps_the_installed_copy() {
            let data = tempfile::tempdir().unwrap();
            let good =
                CompanionFont::with_source(data.path(), &serve(Reply::Body(FONT.to_vec())), FONT);
            good.download(&|_| {}, &DownloadControl::default()).unwrap();
            let bad = CompanionFont::with_source(data.path(), &serve(Reply::Status(500)), FONT);
            assert_eq!(
                bad.download(&|_| {}, &DownloadControl::default()),
                Err(DownloadError::Network)
            );
            assert_eq!(good.load().unwrap().as_deref(), Some(FONT));
        }

        #[test]
        fn a_download_reuses_an_existing_verified_copy_without_replacing_it() {
            let data = tempfile::tempdir().unwrap();
            let font =
                CompanionFont::with_source(data.path(), &serve(Reply::Body(FONT.to_vec())), FONT);
            let result = font.download_with_hooks(
                &|_| {},
                &DownloadControl::default(),
                || fs::write(font.path(), FONT).unwrap(),
                |_| panic!("the existing file must not be replaced"),
            );

            assert_eq!(result.as_deref(), Ok(FONT));
            assert_eq!(fs::read(font.path()).unwrap(), FONT);
            assert_no_staged_parts(data.path());
        }

        #[cfg(windows)]
        #[test]
        fn a_failed_post_commit_read_removes_the_new_file() {
            use std::os::windows::fs::OpenOptionsExt as _;

            const FILE_SHARE_DELETE: u32 = 4;
            assert_read_back_failure(
                OpenOptions::new().share_mode(FILE_SHARE_DELETE),
                DownloadError::Disk,
            );
        }

        #[cfg(windows)]
        #[test]
        fn a_failed_rollback_reports_removal_and_leaves_the_file() {
            use std::os::windows::fs::OpenOptionsExt as _;

            assert_read_back_failure(OpenOptions::new().share_mode(0), DownloadError::Removal);
        }

        #[cfg(windows)]
        fn assert_read_back_failure(options: &mut OpenOptions, expected: DownloadError) {
            let data = tempfile::tempdir().unwrap();
            let font =
                CompanionFont::with_source(data.path(), &serve(Reply::Body(FONT.to_vec())), FONT);
            let mut handle = None;
            let result = font.download_with_hooks(
                &|_| {},
                &DownloadControl::default(),
                || {},
                |path| handle = Some(options.read(true).open(path).unwrap()),
            );

            assert_eq!(result, Err(expected));
            drop(handle);
            assert_eq!(
                font.path().try_exists().unwrap(),
                expected == DownloadError::Removal
            );
            assert_no_staged_parts(data.path());
        }

        mod late_cancel {
            use super::*;

            #[test]
            fn cancellation_after_the_stream_ends_prevents_the_commit() {
                let data = tempfile::tempdir().unwrap();
                let font = CompanionFont::with_source(
                    data.path(),
                    &serve(Reply::Body(FONT.to_vec())),
                    FONT,
                );
                let cancel = DownloadControl::default();
                let (ready_tx, ready_rx) = std::sync::mpsc::channel();
                let (resume_tx, resume_rx) = std::sync::mpsc::channel();

                let font_ref = &font;
                let cancel_ref = &cancel;
                let (cancel_won, resumed, result) = std::thread::scope(move |scope| {
                    let job = scope.spawn(move || {
                        font_ref.download_with_hooks(
                            &|_| {},
                            cancel_ref,
                            move || {
                                ready_tx.send(()).unwrap();
                                resume_rx.recv().unwrap();
                            },
                            |_| {},
                        )
                    });
                    ready_rx.recv().unwrap();
                    let cancel_won = cancel_ref.cancel();
                    let resumed = resume_tx.send(());
                    let result = job.join().unwrap();
                    (cancel_won, resumed, result)
                });

                assert!(cancel_won);
                resumed.unwrap();
                assert_eq!(result, Err(DownloadError::Cancelled));
                assert!(!font.path().try_exists().unwrap());
                assert_eq!(fs::read_dir(font.dir).unwrap().count(), 0);
                assert_no_staged_parts(data.path());
            }
        }

        mod restore_interleaving {
            use super::*;
            use std::{sync::mpsc, thread, time::Duration};

            #[test]
            fn startup_cleanup_removes_old_bytes_and_load_waits_for_a_new_commit() {
                let data = tempfile::tempdir().unwrap();
                let font = CompanionFont::with_source(data.path(), "http://127.0.0.1:9/", FONT);
                fs::create_dir_all(&font.dir).unwrap();
                fs::write(font.path(), b"old bad file").unwrap();
                assert_eq!(font.load().unwrap(), None);
                assert!(
                    !font.path().try_exists().unwrap(),
                    "an old invalid file is removed"
                );

                let restored = while_committing(&font, CompanionFont::load).unwrap();
                assert_eq!(restored.as_deref(), Some(FONT));
                assert_eq!(font.load().unwrap().as_deref(), Some(FONT));
            }

            #[test]
            fn remove_waits_for_a_commit_even_when_the_file_is_absent() {
                let data = tempfile::tempdir().unwrap();
                let font = CompanionFont::with_source(data.path(), "http://127.0.0.1:9/", FONT);
                fs::create_dir_all(&font.dir).unwrap();

                while_committing(&font, CompanionFont::remove).unwrap();
                assert!(!font.path().exists(), "the concurrent commit was removed");
            }

            fn while_committing<T: Send>(
                font: &CompanionFont,
                operation: impl FnOnce(&CompanionFont) -> T + Send,
            ) -> T {
                thread::scope(|scope| {
                    let holder = font.lock_lifecycle().unwrap();
                    let (started_tx, started_rx) = mpsc::sync_channel(0);
                    let (finished_tx, finished_rx) = mpsc::channel();
                    let job = scope.spawn(move || {
                        started_tx.send(()).unwrap();
                        let result = operation(font);
                        finished_tx.send(()).unwrap();
                        result
                    });
                    started_rx.recv().unwrap();
                    let finished = finished_rx.recv_timeout(Duration::from_millis(250));
                    fs::write(font.path(), FONT).unwrap();
                    drop(holder);

                    assert_eq!(finished, Err(mpsc::RecvTimeoutError::Timeout));
                    job.join().unwrap()
                })
            }
        }

        #[test]
        fn a_cancelled_download_stops_and_leaves_nothing() {
            let data = tempfile::tempdir().unwrap();
            // Accepts and never answers: only the cancel ends the wait.
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let url = format!("http://{}/font.otf", listener.local_addr().unwrap());
            let _held = std::thread::spawn(move || {
                let _streams: Vec<_> = listener.incoming().take(1).collect();
                std::thread::sleep(std::time::Duration::from_secs(5));
            });
            let font = CompanionFont::with_source(data.path(), &url, FONT);
            let cancel = DownloadControl::default();
            let started = std::time::Instant::now();
            let result = std::thread::scope(|scope| {
                scope.spawn(|| {
                    std::thread::sleep(std::time::Duration::from_millis(300));
                    assert!(cancel.cancel());
                });
                font.download(&|_| {}, &cancel)
            });
            assert_eq!(result, Err(DownloadError::Cancelled));
            assert!(started.elapsed() < std::time::Duration::from_secs(3));
            assert_eq!(fs::read_dir(data.path().join("fonts")).unwrap().count(), 0);
            assert_no_staged_parts(data.path());
        }
    }
}
