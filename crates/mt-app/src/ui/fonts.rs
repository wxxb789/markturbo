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
    /// No usable file on this device.
    Absent,
    /// Being downloaded: the bytes received so far.
    Downloading(u64),
    /// Checking a stored file or registering the verified face.
    Applying,
    /// Verified and registered: Chinese text is set in it.
    Installed,
    /// Deleting the stored file.
    Removing,
    /// Removed this session. nsh 0.6.1 cannot unregister a face, so the
    /// registered one stays until the app restarts.
    Removed,
    /// The last operation failed.
    Failed(CompanionFailure),
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum CompanionAction {
    Download,
    Retry,
    Cancel,
    Remove,
}

/// Why an operation on the companion failed, as the page names it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompanionFailure {
    Network,
    Verification,
    Disk,
    Registration,
    Removal,
}

const PROGRESS_TICK: Duration = Duration::from_millis(150);

struct RunningDownload {
    control: Arc<DownloadControl>,
}

/// The companion CJK face of one window: its file and its status. Every
/// change to [`Companion::status`] is made on the engine thread, by the tasks
/// below, after the blocking work they await.
#[derive(Clone)]
pub struct Companion {
    pub status: Signal<CompanionStatus>,
    on_disk: Signal<bool>,
    /// `None` without a data directory: a download then fails as a disk one.
    font: Option<CompanionFont>,
    running_download: Rc<RefCell<Option<RunningDownload>>>,
    /// Bumped by download, cancel and remove so stale tasks cannot write status.
    generation: Rc<Cell<u64>>,
    /// Registration survives removal and later failures until the app exits.
    applied: Rc<Cell<bool>>,
    owner: Owner,
}

impl Companion {
    /// The companion in this user's data directory, not yet verified. Call
    /// it under the owner the tasks should live with (the app state's).
    pub fn new() -> Self {
        Self::with_font(companion_font())
    }

    fn with_font(font: Option<CompanionFont>) -> Self {
        Self {
            status: signal(CompanionStatus::Absent),
            on_disk: signal(false),
            font,
            running_download: Rc::default(),
            generation: Rc::default(),
            applied: Rc::default(),
            owner: Owner::current(),
        }
    }

    /// The bytes the download must receive (the progress total).
    pub fn size(&self) -> u64 {
        self.font
            .as_ref()
            .map_or(mt_core::companion_font::SIZE, CompanionFont::size)
    }

    /// At startup or after cancel: read and verify the stored copy on a
    /// blocking thread, then register it. Never touches the network.
    pub fn restore(&self) {
        let Some(font) = self.font.clone() else {
            return;
        };
        self.status.set(CompanionStatus::Applying);
        let this = self.clone();
        self.run(async move {
            let outcome = blocking(move || {
                if font.cleanup_stale_parts().is_err() {
                    log::warn!("fonts: stale companion downloads could not be cleaned up");
                }
                let loaded = font.load();
                let on_disk = match &loaded {
                    Ok(bytes) => bytes.is_some(),
                    Err(_) => font.path().try_exists().unwrap_or(true),
                };
                (loaded, on_disk)
            })
            .await;
            let status = match outcome {
                Ok((loaded, on_disk)) => {
                    this.on_disk.set(on_disk);
                    match loaded {
                        Ok(Some(bytes)) => this.apply(bytes).await,
                        Ok(None) if this.applied.get() => CompanionStatus::Removed,
                        Ok(None) => CompanionStatus::Absent,
                        Err(_) if !on_disk => {
                            log::warn!("fonts: the companion could not be read");
                            CompanionStatus::Failed(CompanionFailure::Disk)
                        }
                        Err(_) => {
                            log::warn!("fonts: the stored companion could not be read or removed");
                            CompanionStatus::Failed(CompanionFailure::Removal)
                        }
                    }
                }
                Err(_) => {
                    log::warn!("fonts: the companion could not be read");
                    CompanionStatus::Absent
                }
            };
            this.status.set(status);
        });
    }

    async fn apply(&self, bytes: Vec<u8>) -> CompanionStatus {
        self.status.set(CompanionStatus::Applying);
        match register_companion(bytes).await {
            Ok(()) => {
                self.applied.set(true);
                CompanionStatus::Installed
            }
            Err(failure) => CompanionStatus::Failed(failure),
        }
    }

    /// Downloads, verifies, installs and registers the font: only ever on
    /// the person's press.
    pub fn download(&self) {
        self.download_with(fetch);
    }

    fn download_with(
        &self,
        fetch: impl FnOnce(
            &CompanionFont,
            &dyn Fn(u64),
            &DownloadControl,
        ) -> Result<Vec<u8>, DownloadError>
        + Send
        + 'static,
    ) {
        if !matches!(
            self.action(),
            Some(CompanionAction::Download | CompanionAction::Retry)
        ) {
            return;
        }
        let previous_status = self.status.get();
        let Some(font) = self.font.clone() else {
            self.status
                .set(CompanionStatus::Failed(CompanionFailure::Disk));
            return;
        };
        let generation = self.generation.get() + 1;
        self.generation.set(generation);
        let control = Arc::new(DownloadControl::default());
        *self.running_download.borrow_mut() = Some(RunningDownload {
            control: control.clone(),
        });
        let received = Arc::new(AtomicU64::new(0));
        let worker_control = control.clone();
        self.status.set(CompanionStatus::Downloading(0));
        let this = self.clone();
        self.run(async move {
            let heard = received.clone();
            let mut job = pin!(blocking(move || {
                let result = fetch(
                    &font,
                    &|bytes| heard.store(bytes, Ordering::Relaxed),
                    &worker_control,
                );
                let on_disk = result.is_ok() || font.path().try_exists().unwrap_or(true);
                (result, on_disk)
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
            let status = match outcome {
                Ok((result, on_disk)) => {
                    this.on_disk.set(on_disk);
                    match result {
                        Ok(bytes) => this.apply(bytes).await,
                        Err(DownloadError::Cancelled) => previous_status,
                        Err(DownloadError::Verification) => {
                            CompanionStatus::Failed(CompanionFailure::Verification)
                        }
                        Err(DownloadError::Disk) => CompanionStatus::Failed(CompanionFailure::Disk),
                        Err(DownloadError::Removal) => {
                            CompanionStatus::Failed(CompanionFailure::Removal)
                        }
                        Err(DownloadError::Network) => {
                            CompanionStatus::Failed(CompanionFailure::Network)
                        }
                    }
                }
                Err(_) => CompanionStatus::Failed(CompanionFailure::Network),
            };
            if this.generation.get() == generation {
                this.running_download.borrow_mut().take();
                this.status.set(status);
            }
        });
    }

    /// Stops the running download and checks what is already on disk.
    pub fn cancel(&self) {
        let mut running = self.running_download.borrow_mut();
        let Some(download) = running.as_ref() else {
            return;
        };
        if download.control.cancel() {
            running.take();
            drop(running);
            self.generation.set(self.generation.get() + 1);
            self.restore();
        } else if download.control.is_committing() {
            drop(running);
            self.status.set(CompanionStatus::Applying);
        }
    }

    pub(super) fn action(&self) -> Option<CompanionAction> {
        match (self.status.get(), self.on_disk.get()) {
            (CompanionStatus::Downloading(_), _) => Some(CompanionAction::Cancel),
            (CompanionStatus::Applying | CompanionStatus::Removing, _) => None,
            // A failed install keeps Retry first; when its file is on disk
            // the page offers Remove beside it (`offers`).
            (CompanionStatus::Failed(_), _) => Some(CompanionAction::Retry),
            (_, true) => Some(CompanionAction::Remove),
            (_, false) => Some(CompanionAction::Download),
        }
    }

    /// Whether `action` is one of the page's controls right now: a failed
    /// install with the file on disk offers both Retry and Remove.
    pub(super) fn offers(&self, action: CompanionAction) -> bool {
        let status = self.status.get();
        let busy = matches!(
            status,
            CompanionStatus::Downloading(_) | CompanionStatus::Applying | CompanionStatus::Removing
        );
        match action {
            CompanionAction::Download => {
                !busy && !matches!(status, CompanionStatus::Failed(_)) && !self.on_disk.get()
            }
            CompanionAction::Retry => matches!(status, CompanionStatus::Failed(_)),
            CompanionAction::Cancel => matches!(status, CompanionStatus::Downloading(_)),
            CompanionAction::Remove => self.on_disk.get() && !busy,
        }
    }

    /// Deletes the file. The registered face stays until the app restarts.
    pub fn remove(&self) {
        if !self.offers(CompanionAction::Remove) {
            return;
        }
        let Some(font) = self.font.clone() else {
            return;
        };
        self.generation.set(self.generation.get() + 1);
        self.status.set(CompanionStatus::Removing);
        let this = self.clone();
        self.run(async move {
            let removed = blocking(move || {
                let result = font.remove();
                (
                    result.is_ok(),
                    result.is_err() && font.path().try_exists().unwrap_or(true),
                )
            })
            .await;
            let status = match removed {
                Ok((true, _)) if this.applied.get() => CompanionStatus::Removed,
                Ok((true, _)) => CompanionStatus::Absent,
                _ => {
                    log::warn!("fonts: the companion could not be removed");
                    CompanionStatus::Failed(CompanionFailure::Removal)
                }
            };
            if let Ok((_, on_disk)) = removed {
                this.on_disk.set(on_disk);
            }
            this.status.set(status);
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
) -> Result<Vec<u8>, DownloadError> {
    font.download(progress, control)
}

/// A build without the transport has no way to fetch it.
#[cfg(not(feature = "model-transport"))]
fn fetch(
    _: &CompanionFont,
    _: &dyn Fn(u64),
    _: &DownloadControl,
) -> Result<Vec<u8>, DownloadError> {
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
/// the pinned remote source and leaves any installed file alone unless a
/// test explicitly sets its source with [`serve_companion_from`].
fn companion_font() -> Option<CompanionFont> {
    #[cfg(not(any(test, feature = "test-vault")))]
    {
        CompanionFont::locate()
    }
    #[cfg(any(test, feature = "test-vault"))]
    {
        let data = mt_core::runtime_paths::data_dir()?;
        let (url, expected) = TEST_SOURCE
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()?;
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

#[cfg(test)]
mod tests {
    use std::{
        fs,
        path::Path,
        sync::{Arc, Mutex, mpsc},
    };

    use mt_core::settings::Language;
    use nsh_core::prelude::*;
    use nsh_engine::{EngineConfig, Root};
    use nsh_testing::Headless;

    use super::{Companion, CompanionFailure, CompanionFont, CompanionStatus, DownloadError};
    use crate::ui::{prims, settings::cjk_font_section};

    const FONT: &[u8] = include_bytes!("../../../../fonts/katex/KaTeX_Main-Regular.ttf");
    const ABSENT: &str = "text:Not installed. Chinese text uses the system's font.";
    const REMOVE: &str = "id:settings-cjk-font-remove";

    #[derive(Clone)]
    enum Fetch {
        NetworkFailure,
        WaitForCancel(Arc<Mutex<mpsc::Receiver<()>>>),
        RollbackFailure,
    }

    #[derive(Clone, PartialEq, Props)]
    struct FontHarnessProps {
        content: Option<Renderer<bool>>,
    }

    #[component]
    fn FontHarness(props: FontHarnessProps) -> impl IntoElement {
        props.content.unwrap().render("font-controls", true)
    }

    fn font_controls(font: CompanionFont, fetch: Fetch) -> impl IntoElement {
        let companion = Companion::with_font(Some(font));
        companion.restore();
        view()
            .flex_col()
            .child(cjk_font_section(companion.clone(), Language::English))
            .child(prims::primary_action(
                "test-fetch",
                "Test fetch",
                move || {
                    let fetch = fetch.clone();
                    companion.download_with(move |font, _, _| match fetch {
                        Fetch::NetworkFailure => Err(DownloadError::Network),
                        Fetch::WaitForCancel(release) => {
                            let _ = release.lock().unwrap().recv();
                            Err(DownloadError::Cancelled)
                        }
                        Fetch::RollbackFailure => {
                            fs::write(font.path(), FONT).unwrap();
                            Err(DownloadError::Removal)
                        }
                    });
                },
            ))
    }

    fn font(data: &Path, expected: &[u8]) -> CompanionFont {
        let font = CompanionFont::with_source(data, "http://127.0.0.1:0/font", expected);
        fs::create_dir_all(font.path().parent().unwrap()).unwrap();
        font
    }

    fn start(font: CompanionFont, fetch: Fetch) -> Headless {
        start_with(move || font_controls(font.clone(), fetch.clone()))
    }

    fn start_with<T: IntoElement + 'static>(
        render: impl Fn() -> T + Clone + Send + Sync + 'static,
    ) -> Headless {
        Headless::start(EngineConfig::new(Root::new::<FontHarness>(move || {
            FontHarnessProps {
                content: Some(Renderer::new({
                    let render = render.clone();
                    move |_| render()
                })),
            }
        })))
        .unwrap()
    }

    #[test]
    fn restore_lock_open_failure_without_a_file_offers_retry() {
        let data = tempfile::tempdir().unwrap();
        let font = font(data.path(), FONT);
        let lock_path = data.path().join(".companion-font.lock");
        #[cfg(windows)]
        let _lock = {
            use std::os::windows::fs::OpenOptionsExt as _;
            fs::OpenOptions::new()
                .create_new(true)
                .write(true)
                .share_mode(0)
                .open(lock_path)
                .unwrap()
        };
        #[cfg(not(windows))]
        fs::create_dir(lock_path).unwrap();

        assert!(!font.path().exists());
        let mut app = start(font, Fetch::NetworkFailure);
        app.wait_for("id:settings-cjk-font-retry")
            .expect("an absent font can be retried after a restore lock failure");
        app.wait_for("label:Retry").expect("retry label");
        app.wait_for(
            "text:The font file could not be saved or read back in MarkTurbo's data folder.",
        )
        .expect("restore reports a disk failure, not a removal failure");
        app.wait_for("id:settings-cjk-font-disclosure")
            .expect("the retry has a download disclosure");
    }

    #[test]
    fn settings_font_actions_follow_busy_and_file_presence() {
        use CompanionStatus::*;

        let download = "id:settings-cjk-font-download";
        let retry = "id:settings-cjk-font-retry";
        let cancel = "id:settings-cjk-font-cancel";
        let remove = REMOVE;
        for (status, absent, present) in [
            (Downloading(0), &[cancel][..], &[cancel][..]),
            (Applying, &[][..], &[][..]),
            (Removing, &[][..], &[][..]),
            (Absent, &[download][..], &[remove][..]),
            (Installed, &[download][..], &[remove][..]),
            (Removed, &[download][..], &[remove][..]),
            (
                Failed(CompanionFailure::Network),
                &[retry][..],
                &[retry, remove][..],
            ),
            (
                Failed(CompanionFailure::Verification),
                &[retry][..],
                &[retry, remove][..],
            ),
            (
                Failed(CompanionFailure::Disk),
                &[retry][..],
                &[retry, remove][..],
            ),
            (
                Failed(CompanionFailure::Registration),
                &[retry][..],
                &[retry, remove][..],
            ),
            (
                Failed(CompanionFailure::Removal),
                &[retry][..],
                &[retry, remove][..],
            ),
        ] {
            for (on_disk, expected) in [(false, absent), (true, present)] {
                let mut app = start_with(move || {
                    let companion = Companion::with_font(None);
                    companion.status.set(status);
                    companion.on_disk.set(on_disk);
                    cjk_font_section(companion, Language::English)
                });
                app.wait_for("id:settings-cjk-font-status").unwrap();
                for selector in [download, retry, cancel, remove] {
                    assert_eq!(
                        app.screen().matches(selector).unwrap(),
                        expected.contains(&selector),
                        "{status:?}, on_disk={on_disk}, {selector}"
                    );
                }
            }
        }
    }

    #[test]
    fn remove_is_hidden_while_applying_a_stored_font() {
        let data = tempfile::tempdir().unwrap();
        let font = font(data.path(), FONT);
        fs::write(font.path(), FONT).unwrap();
        let mut app = start_with(move || {
            let companion = Companion::with_font(Some(font.clone()));
            companion.status.set(CompanionStatus::Applying);
            companion.on_disk.set(true);
            cjk_font_section(companion, Language::English)
        });
        app.wait_for("text:Applying…").unwrap();
        app.wait_for_gone(REMOVE, std::time::Duration::ZERO)
            .unwrap();
    }

    #[test]
    fn retry_and_remove_are_offered_when_retry_fails_with_a_file_still_on_disk() {
        let data = tempfile::tempdir().unwrap();
        let font = font(data.path(), FONT);
        let mut app = start(font.clone(), Fetch::NetworkFailure);
        app.wait_for(ABSENT).unwrap();
        app.press("id:test-fetch").unwrap();
        app.wait_for("id:settings-cjk-font-retry").unwrap();
        // Another instance can install the font after Settings offers Retry.
        fs::write(font.path(), FONT).unwrap();
        app.press("id:test-fetch").unwrap();
        app.wait_for(REMOVE)
            .expect("the retry found the file on disk");
        app.wait_for(
            "text:Download failed: could not reach the font server. Nothing was installed — check your connection and press Retry.",
        )
        .unwrap();
        app.wait_for("id:settings-cjk-font-retry")
            .expect("retry stays offered with the file on disk");
        assert!(font.path().is_file());
        app.press(REMOVE).expect("remove the copy left on disk");
        app.wait_for(ABSENT).unwrap();
        assert!(!font.path().exists());
    }

    #[test]
    fn remove_is_offered_when_file_presence_cannot_be_checked() {
        let data = tempfile::tempdir().unwrap();
        let font = CompanionFont::with_source(
            &data.path().join("invalid\0path"),
            "http://127.0.0.1:0/font",
            FONT,
        );
        assert!(font.path().try_exists().is_err());
        let mut app = start(font, Fetch::NetworkFailure);
        app.wait_for(
            "text:Removal failed: the font could not be deleted from MarkTurbo's data folder.",
        )
        .unwrap();
        app.wait_for(REMOVE).expect("the file may still be present");
    }

    #[test]
    fn remove_is_offered_when_post_commit_rollback_fails() {
        let data = tempfile::tempdir().unwrap();
        let font = font(data.path(), FONT);
        let mut app = start(font.clone(), Fetch::RollbackFailure);
        app.wait_for(ABSENT).unwrap();
        app.press("id:test-fetch").unwrap();
        app.wait_for(
            "text:Removal failed: the font could not be deleted from MarkTurbo's data folder.",
        )
        .unwrap();
        app.wait_for(REMOVE)
            .expect("remove offered after rollback fails");
        assert_eq!(fs::read(font.path()).unwrap(), FONT);
        app.press(REMOVE).expect("remove the committed copy");
        app.wait_for(ABSENT).expect("the copy was never applied");
        assert!(!font.path().exists());
    }

    #[test]
    fn cancel_restores_a_newly_committed_font() {
        let data = tempfile::tempdir().unwrap();
        let font = font(data.path(), FONT);
        let (release, wait) = mpsc::channel();
        let mut app = start(
            font.clone(),
            Fetch::WaitForCancel(Arc::new(Mutex::new(wait))),
        );
        app.wait_for(ABSENT).unwrap();
        app.press("id:test-fetch").unwrap();
        app.wait_for("id:settings-cjk-font-cancel").unwrap();
        fs::write(font.path(), FONT).unwrap();
        app.press("id:settings-cjk-font-cancel").unwrap();
        release.send(()).unwrap();
        app.wait_for("text:Installed and applied: Chinese text is set in Source Han Sans CN.")
            .expect("Cancel restores the copy committed while the download was pending");
        assert_eq!(fs::read(font.path()).unwrap(), FONT);
    }
}
