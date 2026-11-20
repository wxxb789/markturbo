//! Owner lane: foundation.
//!
//! The typefaces the Afterglow UI is set in, embedded in the executable and
//! registered with nsh at startup.
//!
//! Martian Grotesk (300 / 400 / 500) and Martian Mono (400, condensed) are
//! by Evil Martians, licensed under the SIL Open Font License 1.1; the licence
//! texts ship beside the files in `crates/mt-app/fonts/martian/`, and
//! `SOURCE.md` there records the release, URL and hash each `.ttf` came from.
//!
//! nsh resolves each face to its family — the three Grotesk instances register
//! as `Martian Grotesk` and are told apart by weight, the mono as
//! `Martian Mono`. [`crate::ui::tokens`] holds those names, the weights and
//! the CJK fallback lists.
//!
//! Registration is asynchronous (`register_font`'s answer resolves to the
//! family name). Text laid out before it lands is measured again by the
//! engine when the faces arrive, so nothing needs to block the first frame.
//!
//! The companion CJK face ([`Companion`], Source Han Sans CN) is not
//! embedded: the settings page downloads it on request
//! (`mt_core::companion_font`), and once it is on disk it is read, verified
//! and registered off the first-frame path at startup.

use std::{
    cell::{Cell, RefCell},
    pin::pin,
    rc::Rc,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    task::Poll,
    time::Duration,
};

use mt_core::companion_font::{CompanionFont, DownloadControl, DownloadError};
use nsh_core::element::{register_font, spawn};
use nsh_core::prelude::{Owner, Signal, blocking, signal, stored, timer};

/// One embedded face: the family name it must resolve to, and its bytes.
struct Face {
    /// The family nsh must resolve it to; a mismatch is logged, not fatal.
    family: &'static str,
    /// The `.ttf` as shipped by the release.
    data: &'static [u8],
}

/// The four faces, with the family names `ui::tokens` refers to. A `static`,
/// not a `const`: a `const` is copied into each crate that uses it, which put
/// a second copy of every face in the executable.
static FACES: [Face; 4] = [
    Face {
        family: "Martian Grotesk",
        data: include_bytes!("../../fonts/martian/MartianGrotesk-StdLt.ttf"),
    },
    Face {
        family: "Martian Grotesk",
        data: include_bytes!("../../fonts/martian/MartianGrotesk-StdRg.ttf"),
    },
    Face {
        family: "Martian Grotesk",
        data: include_bytes!("../../fonts/martian/MartianGrotesk-StdMd.ttf"),
    },
    Face {
        family: "Martian Mono",
        data: include_bytes!("../../fonts/martian/MartianMono-CnRg.ttf"),
    },
];

/// The embedded faces' bytes, in registration order: what `main.rs` hands to
/// the root so the first frame is already measured with them.
pub fn faces() -> impl Iterator<Item = &'static [u8]> {
    FACES.iter().map(|face| face.data)
}

/// The family name of each face, in the same order as [`faces`].
pub fn family_names() -> impl Iterator<Item = &'static str> {
    FACES.iter().map(|face| face.family)
}

/// Registers every embedded face, in order. Call once, from the app root;
/// the registration lives as long as the app does.
///
/// Idempotent per process in practice (nsh keeps the faces), but it is only
/// called from [`crate::app::App`].
pub fn register() {
    stored(spawn(async {
        for face in &FACES {
            match register_font(face.data).await {
                Ok(family) if family == face.family => {
                    log::debug!("fonts: registered {family}");
                }
                Ok(family) => {
                    log::warn!(
                        "fonts: {} registered as {family:?}, not {:?}",
                        face.family,
                        face.family
                    );
                }
                Err(why) => log::error!("fonts: {} is no font: {why}", face.family),
            }
        }
    }));
}

/// The companion CJK face as the settings page shows it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompanionStatus {
    /// Not on this device (or not found yet at startup).
    Absent,
    /// Being downloaded: the bytes received so far.
    Downloading(u64),
    /// Verified and committed; nsh is registering the face.
    Applying,
    /// Verified and registered: Chinese text is set in it.
    Installed,
    /// Removed this session. nsh 0.5.1 cannot unregister a face, so the
    /// registered one stays until the app restarts.
    Removed,
    /// The last download or registration failed; the face is not applied.
    Failed(CompanionFailure),
}

/// Why downloading or applying the companion failed, as the page names it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompanionFailure {
    Network,
    Verification,
    Disk,
    Registration,
}

const PROGRESS_TICK: Duration = Duration::from_millis(150);

/// The companion CJK face of one window: its file and its status. Every
/// change to [`Companion::status`] is made on the engine thread, by the tasks
/// below, after the blocking work they await.
#[derive(Clone)]
pub struct Companion {
    pub status: Signal<CompanionStatus>,
    /// `None` without a data directory: a download then fails as a disk one.
    font: Option<CompanionFont>,
    /// The running download's cancellation and commit state.
    cancel: Rc<RefCell<Option<Arc<DownloadControl>>>>,
    /// Bumped by every download and cancel: a task whose generation is stale
    /// writes nothing, so a cancelled run cannot overwrite a newer one.
    generation: Rc<Cell<u64>>,
    owner: Owner,
}

impl Companion {
    /// The companion in this user's data directory, not yet looked at. Call
    /// it under the owner the tasks should live with (the app state's).
    pub fn new() -> Self {
        Self {
            status: signal(CompanionStatus::Absent),
            font: companion_font(),
            cancel: Rc::default(),
            generation: Rc::default(),
            owner: Owner::current(),
        }
    }

    /// The bytes the download must receive (the progress total).
    pub fn size(&self) -> u64 {
        self.font
            .as_ref()
            .map_or(mt_core::companion_font::SIZE, CompanionFont::size)
    }

    /// At startup: when the font is on disk, read and verify it on a blocking
    /// thread and register it. Never blocks the first frame and never
    /// touches the network.
    pub fn restore(&self) {
        let Some(font) = self.font.clone() else {
            return;
        };
        let this = self.clone();
        self.run(async move {
            let generation = this.generation.get();
            match blocking(move || font.load()).await {
                Ok(Ok(Some(bytes))) => {
                    let status = match register_companion(bytes).await {
                        Ok(()) => CompanionStatus::Installed,
                        Err(failure) => CompanionStatus::Failed(failure),
                    };
                    if this.generation.get() == generation {
                        this.status.set(status);
                    }
                }
                Ok(Ok(None)) => {}
                _ => log::warn!("fonts: the companion could not be read"),
            }
        });
    }

    /// Downloads, verifies, installs and registers the font: only ever on
    /// the person's press.
    pub fn download(&self) {
        if matches!(
            self.status.get(),
            CompanionStatus::Downloading(_) | CompanionStatus::Applying
        ) {
            return;
        }
        let Some(font) = self.font.clone() else {
            self.status
                .set(CompanionStatus::Failed(CompanionFailure::Disk));
            return;
        };
        let generation = self.generation.get() + 1;
        self.generation.set(generation);
        let control = Arc::new(DownloadControl::default());
        *self.cancel.borrow_mut() = Some(control.clone());
        let received = Arc::new(AtomicU64::new(0));
        let worker_control = control.clone();
        self.status.set(CompanionStatus::Downloading(0));
        let this = self.clone();
        self.run(async move {
            let heard = received.clone();
            let mut job = pin!(blocking(move || {
                fetch(
                    &font,
                    &|bytes| heard.store(bytes, Ordering::Relaxed),
                    &worker_control,
                )?;
                // Read back and verified again before the engine sees it.
                match font.load() {
                    Ok(Some(bytes)) => Ok(bytes),
                    Ok(None) => Err(DownloadError::Verification),
                    Err(_) => Err(DownloadError::Disk),
                }
            }));
            // The job's end wakes this task itself; the timer only paces the
            // progress line, so a paused clock cannot hold the result back.
            let outcome = loop {
                let mut tick = pin!(timer(PROGRESS_TICK));
                let finished = std::future::poll_fn(|context| match job.as_mut().poll(context) {
                    Poll::Ready(outcome) => Poll::Ready(Some(outcome)),
                    Poll::Pending => tick.as_mut().poll(context).map(|_| None),
                })
                .await;
                if this.generation.get() != generation {
                    return;
                }
                match finished {
                    Some(outcome) => break outcome,
                    None => this.status.set(if control.is_committing() {
                        CompanionStatus::Applying
                    } else {
                        CompanionStatus::Downloading(received.load(Ordering::Relaxed))
                    }),
                }
            };
            if let Ok(Ok(_)) = &outcome {
                this.status.set(CompanionStatus::Applying);
            }
            this.cancel.borrow_mut().take();
            let status = match outcome {
                Ok(Ok(bytes)) => match register_companion(bytes).await {
                    Ok(()) => CompanionStatus::Installed,
                    Err(failure) => CompanionStatus::Failed(failure),
                },
                Ok(Err(DownloadError::Cancelled)) => CompanionStatus::Absent,
                Ok(Err(DownloadError::Verification)) => {
                    CompanionStatus::Failed(CompanionFailure::Verification)
                }
                Ok(Err(DownloadError::Disk)) => CompanionStatus::Failed(CompanionFailure::Disk),
                Ok(Err(DownloadError::Network)) | Err(_) => {
                    CompanionStatus::Failed(CompanionFailure::Network)
                }
            };
            if this.generation.get() == generation {
                this.status.set(status);
            }
        });
    }

    /// Stops the running download; it leaves nothing behind.
    pub fn cancel(&self) {
        if let Some(control) = self.cancel.borrow_mut().take() {
            if control.cancel() {
                self.generation.set(self.generation.get() + 1);
                self.status.set(CompanionStatus::Absent);
            } else if control.is_committing() {
                self.status.set(CompanionStatus::Applying);
            }
        }
    }

    /// Deletes the file. The registered face stays until the app restarts.
    pub fn remove(&self) {
        let Some(font) = self.font.clone() else {
            return;
        };
        let this = self.clone();
        self.run(async move {
            match blocking(move || font.remove()).await {
                Ok(Ok(())) => this.status.set(CompanionStatus::Removed),
                _ => log::warn!("fonts: the companion could not be removed"),
            }
        });
    }

    /// Runs `future` with the state's lifetime, not a press handler's.
    fn run(&self, future: impl Future<Output = ()> + 'static) {
        self.owner.run(|| {
            spawn(future);
        });
    }
}

impl Default for Companion {
    fn default() -> Self {
        Self::new()
    }
}

/// Downloads the companion into place ([`CompanionFont::download`]).
#[cfg(feature = "model-transport")]
fn fetch(
    font: &CompanionFont,
    progress: &dyn Fn(u64),
    control: &DownloadControl,
) -> Result<(), DownloadError> {
    font.download(progress, control)
}

/// A build without the transport has no way to fetch it.
#[cfg(not(feature = "model-transport"))]
fn fetch(_: &CompanionFont, _: &dyn Fn(u64), _: &DownloadControl) -> Result<(), DownloadError> {
    Err(DownloadError::Network)
}

/// Registers the companion's verified bytes.
async fn register_companion(bytes: Vec<u8>) -> Result<(), CompanionFailure> {
    match register_font(bytes).await {
        Ok(family) => {
            log::debug!("fonts: registered the companion as {family}");
            Ok(())
        }
        Err(why) => {
            log::warn!("fonts: the companion is no font: {why}");
            Err(CompanionFailure::Registration)
        }
    }
}

/// The companion in this user's data directory. A test build never names
/// the pinned remote source: it downloads from what
/// [`serve_companion_from`] set, else from a loopback port nothing answers.
fn companion_font() -> Option<CompanionFont> {
    #[cfg(not(any(test, feature = "test-vault")))]
    {
        CompanionFont::locate()
    }
    #[cfg(any(test, feature = "test-vault"))]
    {
        let data = mt_core::runtime_paths::data_dir()?;
        let source = TEST_SOURCE
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone();
        let (url, expected) =
            source.unwrap_or_else(|| ("http://127.0.0.1:9/".to_owned(), Vec::new()));
        Some(CompanionFont::with_source(&data, &url, &expected))
    }
}

#[cfg(any(test, feature = "test-vault"))]
static TEST_SOURCE: std::sync::Mutex<Option<(String, Vec<u8>)>> = std::sync::Mutex::new(None);

/// Test builds only: the companion every later state downloads comes from
/// `url` and must be exactly `expected`.
#[cfg(any(test, feature = "test-vault"))]
#[doc(hidden)]
pub fn serve_companion_from(url: &str, expected: &[u8]) {
    *TEST_SOURCE
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) =
        Some((url.to_owned(), expected.to_vec()));
}
