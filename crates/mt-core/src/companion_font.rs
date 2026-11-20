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

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
#[cfg(feature = "model-transport")]
use std::sync::atomic::{AtomicBool, Ordering};

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

    /// The verified font, for registration: `None` when it is absent, or when
    /// the file present does not match the pinned size and digest — that file
    /// is removed, so a damaged copy is never handed to the text engine.
    pub fn load(&self) -> io::Result<Option<Vec<u8>>> {
        let path = self.path();
        let bytes = match fs::read(&path) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error),
        };
        if self.verifies(&bytes) {
            return Ok(Some(bytes));
        }
        log::warn!("companion font: the stored file does not verify; removing it");
        self.remove()?;
        Ok(None)
    }

    /// Deletes the file; an absent file is already removed.
    pub fn remove(&self) -> io::Result<()> {
        match fs::remove_file(self.path()) {
            Err(error) if error.kind() != io::ErrorKind::NotFound => Err(error),
            _ => Ok(()),
        }
    }

    fn verifies(&self, bytes: &[u8]) -> bool {
        bytes.len() as u64 == self.size && hex_digest(bytes) == self.sha256
    }

    /// Downloads, verifies and installs the font. Blocking: run it off the UI
    /// thread. `progress` hears the bytes received so far (of
    /// [`Self::size`]); setting `cancel` stops it within a moment.
    ///
    /// The body streams into a temporary file in the fonts directory and is
    /// abandoned as soon as it exceeds the pinned size; it is renamed into
    /// place only after its size and digest match. Any failure, including a
    /// cancel, leaves no file behind and an existing installed copy intact.
    #[cfg(feature = "model-transport")]
    pub fn download(
        &self,
        progress: &dyn Fn(u64),
        cancel: &AtomicBool,
    ) -> Result<(), DownloadError> {
        let runtime = crate::model::transport::runtime().map_err(|_| DownloadError::Network)?;
        fs::create_dir_all(&self.dir).map_err(|_| DownloadError::Disk)?;
        // Dropped (and deleted) on every early return below.
        let mut part = tempfile::Builder::new()
            .prefix(".companion-font-")
            .suffix(".part")
            .tempfile_in(&self.dir)
            .map_err(|_| DownloadError::Disk)?;
        let digest = runtime.block_on(self.fetch(part.as_file_mut(), progress, cancel))?;
        if digest != self.sha256 {
            return Err(DownloadError::Verification);
        }
        part.as_file().sync_all().map_err(|_| DownloadError::Disk)?;
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
        cancel: &AtomicBool,
    ) -> Result<String, DownloadError> {
        use std::io::Write as _;

        let client = crate::model::transport::http_client_builder(proxy_policy(&self.url))
            .connect_timeout(STALL_LIMIT)
            .build()
            .map_err(|_| DownloadError::Network)?;
        let mut response = until_cancelled(cancel, client.get(&self.url).send())
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
        while let Some(chunk) = until_cancelled(cancel, response.chunk())
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
/// How long a connection or the stream may stall before the download fails.
#[cfg(feature = "model-transport")]
const STALL_LIMIT: std::time::Duration = std::time::Duration::from_secs(30);

/// `future`'s output, unless `cancel` is set first or it stalls past
/// [`STALL_LIMIT`].
#[cfg(feature = "model-transport")]
async fn until_cancelled<F: std::future::Future>(
    cancel: &AtomicBool,
    future: F,
) -> Result<F::Output, DownloadError> {
    let mut future = std::pin::pin!(future);
    let mut waited = std::time::Duration::ZERO;
    loop {
        if cancel.load(Ordering::Relaxed) {
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
                &AtomicBool::new(false),
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
            good.download(&|_| {}, &AtomicBool::new(false)).unwrap();
            let bad = CompanionFont::with_source(data.path(), &serve(Reply::Status(500)), FONT);
            assert_eq!(
                bad.download(&|_| {}, &AtomicBool::new(false)),
                Err(DownloadError::Network)
            );
            assert_eq!(good.load().unwrap().as_deref(), Some(FONT));
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
            let cancel = AtomicBool::new(false);
            let started = std::time::Instant::now();
            let result = std::thread::scope(|scope| {
                scope.spawn(|| {
                    std::thread::sleep(std::time::Duration::from_millis(300));
                    cancel.store(true, Ordering::Relaxed);
                });
                font.download(&|_| {}, &cancel)
            });
            assert_eq!(result, Err(DownloadError::Cancelled));
            assert!(started.elapsed() < std::time::Duration::from_secs(3));
            assert_eq!(fs::read_dir(data.path().join("fonts")).unwrap().count(), 0);
        }
    }
}
