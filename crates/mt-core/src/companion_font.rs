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
//! A download streams into a temporary file beside the destination, hard
//! capped at the pinned size, and is renamed into place only once verified;
//! every failure leaves no partial or unverified file. A stored file that no
//! longer verifies is treated as absent and removed.

use std::fs::{self, OpenOptions};
use std::io;
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
const STALL_LIMIT: std::time::Duration = std::time::Duration::from_secs(30);
const PARTIAL_STALE_MARGIN: std::time::Duration = std::time::Duration::from_secs(30);

/// The companion font in one data directory: where it lives, and the source
/// and digest it must match.
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
    /// The data directory could not hold the file.
    Disk,
    /// The person cancelled it.
    Cancelled,
}

impl std::fmt::Display for DownloadError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::Network => "the font download did not complete",
            Self::Verification => "the downloaded file is not the pinned font",
            Self::Disk => "the font could not be saved",
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

    /// Whether the file is present. Cheap and unverified: [`Self::load`]
    /// verifies before anything uses the bytes.
    pub fn is_installed(&self) -> bool {
        self.path().is_file()
    }

    /// Removes abandoned partial downloads left behind by a crashed process.
    pub fn cleanup_stale_parts(&self) -> io::Result<()> {
        match fs::metadata(&self.dir) {
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
            Err(error) => return Err(error),
            Ok(_) => {}
        }
        let _lifecycle = self.lock_lifecycle()?;
        self.cleanup_stale_parts_unlocked(std::time::SystemTime::now())
    }

    fn cleanup_stale_parts_unlocked(&self, now: std::time::SystemTime) -> io::Result<()> {
        let entries = match fs::read_dir(&self.dir) {
            Ok(entries) => entries,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
            Err(error) => return Err(error),
        };
        let stale_after = STALL_LIMIT + PARTIAL_STALE_MARGIN;
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
            let modified = entry.metadata()?.modified()?;
            if !now
                .duration_since(modified)
                .is_ok_and(|age| age > stale_after)
            {
                continue;
            }

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
            let modified = file.metadata()?.modified()?;
            if !now
                .duration_since(modified)
                .is_ok_and(|age| age > stale_after)
            {
                continue;
            }
            drop(file);
            match fs::remove_file(path) {
                Err(error) if error.kind() != io::ErrorKind::NotFound => return Err(error),
                _ => {}
            }
        }
        Ok(())
    }

    /// The verified font, for registration: `None` when it is absent, or when
    /// the file present does not match the pinned size and digest — that file
    /// is removed, so a damaged copy is never handed to the text engine.
    pub fn load(&self) -> io::Result<Option<Vec<u8>>> {
        self.load_after_failed_verification(|| {})
    }

    fn load_after_failed_verification(
        &self,
        after_failed_verification: impl FnOnce(),
    ) -> io::Result<Option<Vec<u8>>> {
        let path = self.path();
        let bytes = match fs::read(&path) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error),
        };
        if self.verifies(&bytes) {
            return Ok(Some(bytes));
        }
        after_failed_verification();
        let _lock = self.lock_lifecycle()?;
        match fs::read(&path) {
            Ok(current) if self.verifies(&current) => Ok(Some(current)),
            Ok(_) => {
                log::warn!("companion font: the stored file does not verify; removing it");
                self.remove_unlocked()?;
                Ok(None)
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(error),
        }
    }

    /// Deletes the file; an absent file is already removed.
    pub fn remove(&self) -> io::Result<()> {
        if fs::symlink_metadata(self.path())
            .is_err_and(|error| error.kind() == io::ErrorKind::NotFound)
        {
            return Ok(());
        }
        let _lock = self.lock_lifecycle()?;
        self.remove_unlocked()
    }

    fn remove_unlocked(&self) -> io::Result<()> {
        match fs::remove_file(self.path()) {
            Err(error) if error.kind() != io::ErrorKind::NotFound => Err(error),
            _ => Ok(()),
        }
    }

    /// Locks the companion-file lifecycle across cooperating processes. The
    /// sentinel stays beside `fonts/`, outside the font directory.
    fn lock_lifecycle(&self) -> io::Result<fs::File> {
        let lock_path = self
            .dir
            .parent()
            .unwrap_or(&self.dir)
            .join(".companion-font.lock");
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

    /// Downloads, verifies and installs the font. Blocking: run it off the UI
    /// thread. `progress` hears the bytes received so far (of
    /// [`Self::size`]); `control.cancel()` stops it before file commit begins.
    ///
    /// The body streams into a temporary file in the fonts directory and is
    /// abandoned as soon as it exceeds the pinned size; it is renamed into
    /// place only after its size and digest match. Any failure, including a
    /// cancel, leaves no file behind and an existing installed copy intact.
    #[cfg(feature = "model-transport")]
    pub fn download(
        &self,
        progress: &dyn Fn(u64),
        control: &DownloadControl,
    ) -> Result<(), DownloadError> {
        self.download_after_fetch(progress, control, || {})
    }

    #[cfg(feature = "model-transport")]
    fn download_after_fetch(
        &self,
        progress: &dyn Fn(u64),
        control: &DownloadControl,
        after_fetch: impl FnOnce(),
    ) -> Result<(), DownloadError> {
        let runtime = crate::model::transport::runtime().map_err(|_| DownloadError::Network)?;
        fs::create_dir_all(&self.dir).map_err(|_| DownloadError::Disk)?;
        let lifecycle = self.lock_lifecycle().map_err(|_| DownloadError::Disk)?;
        let mut part = tempfile::Builder::new()
            .prefix(PARTIAL_PREFIX)
            .suffix(PARTIAL_SUFFIX)
            .tempfile_in(&self.dir)
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
        part.persist(self.path()).map_err(|_| DownloadError::Disk)?;
        Ok(())
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
        missing.remove().expect("an absent font is already removed");
        assert!(
            !missing_data.exists(),
            "removing does not create its data root"
        );

        let font = CompanionFont::with_source(data.path(), "http://127.0.0.1:9/", b"the font");
        assert_eq!(font.load().unwrap(), None, "absent");
        assert!(!font.is_installed());

        fs::create_dir_all(font.path().parent().unwrap()).unwrap();
        fs::write(font.path(), b"the fonT").unwrap();
        assert!(font.is_installed(), "present, not yet verified");
        assert_eq!(font.load().unwrap(), None, "a damaged copy is absent");
        assert!(!font.path().exists(), "and removed");

        fs::write(font.path(), b"the font").unwrap();
        assert_eq!(font.load().unwrap().as_deref(), Some(&b"the font"[..]));
        font.remove().unwrap();
        assert!(!font.is_installed());
        font.remove().expect("removing an absent font is fine");
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
    fn startup_cleanup_removes_stale_part_and_keeps_fresh_one() {
        let data = tempfile::tempdir().unwrap();
        let font = CompanionFont::with_source(data.path(), "http://127.0.0.1:9/", b"font");
        fs::create_dir_all(&font.dir).unwrap();
        let stale = font.dir.join(".companion-font-stale.part");
        fs::write(&stale, b"unfinished").unwrap();
        set_modified(&stale, std::time::SystemTime::UNIX_EPOCH);
        let fresh = font.dir.join(".companion-font-fresh.part");
        fs::write(&fresh, b"still writing").unwrap();

        font.cleanup_stale_parts().unwrap();

        assert!(!stale.exists(), "stale partial download was removed");
        assert!(fresh.exists(), "fresh partial download was kept");
    }

    #[test]
    fn startup_cleanup_keeps_a_locked_stale_part() {
        let data = tempfile::tempdir().unwrap();
        let font = CompanionFont::with_source(data.path(), "http://127.0.0.1:9/", b"font");
        fs::create_dir_all(&font.dir).unwrap();
        let active = font.dir.join(".companion-font-active.part");
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
        fn download(reply: Reply) -> (Result<(), DownloadError>, Vec<u64>, Vec<String>) {
            let data = tempfile::tempdir().unwrap();
            let font = CompanionFont::with_source(data.path(), &serve(reply), FONT);
            let heard = Mutex::new(Vec::new());
            let result = font.download(
                &|received| heard.lock().unwrap().push(received),
                &DownloadControl::default(),
            );
            let left = fs::read_dir(data.path().join("fonts"))
                .map(|entries| {
                    entries
                        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
                        .collect()
                })
                .unwrap_or_default();
            if result.is_ok() {
                assert_eq!(
                    font.load().unwrap().as_deref(),
                    Some(FONT),
                    "installed, verified"
                );
            }
            (result, heard.into_inner().unwrap(), left)
        }

        #[test]
        fn the_pinned_bytes_are_installed_and_verified() {
            let (result, heard, left) = download(Reply::Body(FONT.to_vec()));
            assert_eq!(result, Ok(()));
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
                        font_ref.download_after_fetch(&|_| {}, cancel_ref, move || {
                            ready_tx.send(()).unwrap();
                            resume_rx.recv().unwrap();
                        })
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
                assert!(!font.is_installed());
                assert_eq!(fs::read_dir(font.dir).unwrap().count(), 0);
            }
        }

        mod restore_interleaving {
            use super::*;

            #[test]
            fn startup_cleanup_removes_old_bytes_but_keeps_a_new_commit() {
                let data = tempfile::tempdir().unwrap();
                let font = CompanionFont::with_source(
                    data.path(),
                    &serve(Reply::Body(FONT.to_vec())),
                    FONT,
                );
                fs::create_dir_all(font.path().parent().unwrap()).unwrap();
                fs::write(font.path(), b"old bad file").unwrap();
                assert_eq!(font.load().unwrap(), None);
                assert!(!font.is_installed(), "an old invalid file is removed");

                fs::write(font.path(), b"old bad file").unwrap();
                let (read_tx, read_rx) = std::sync::mpsc::channel();
                let (resume_tx, resume_rx) = std::sync::mpsc::channel();
                let font_ref = &font;
                let (downloaded, resumed, restored) = std::thread::scope(move |scope| {
                    let load_font = font_ref;
                    let load = scope.spawn(move || {
                        load_font.load_after_failed_verification(|| {
                            read_tx.send(()).unwrap();
                            resume_rx.recv().unwrap();
                        })
                    });
                    read_rx.recv().unwrap();
                    let downloaded = font_ref.download(&|_| {}, &DownloadControl::default());
                    let resumed = resume_tx.send(());
                    let restored = load.join();
                    (downloaded, resumed, restored)
                });

                downloaded.unwrap();
                resumed.unwrap();
                assert_eq!(restored.unwrap().unwrap().as_deref(), Some(FONT));
                assert_eq!(font.load().unwrap().as_deref(), Some(FONT));
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
        }
    }
}
