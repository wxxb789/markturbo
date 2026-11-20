//! The companion Chinese font: Settings says what a download would fetch
//! before anything is fetched, a failed download says why and offers to try
//! again, a verified one is installed and applied, a later launch applies it
//! without a request, and Remove deletes it.
//!
//! The font comes from a loopback server through the test-build seam
//! (`serve_companion_from`); no test build can reach the pinned remote host.

use std::fs::{self, File, FileTimes};
use std::io::{BufRead as _, BufReader, Write as _};
use std::net::TcpListener;
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicUsize, Ordering},
};
use std::time::{Duration, SystemTime};

use nsh_core::{ApproxTextEngine, AttributedText, Size, TextEngine, TextLayout, font_family};
use nsh_engine::{EngineConfig, Points, Root};
use nsh_testing::Headless;

/// A face not registered by the app stands in for the companion.
const FONT: &[u8] = include_bytes!("../../../fonts/katex/KaTeX_Main-Regular.ttf");
const INVALID_FONT: &[u8] = b"not a font";

type Registrations = Arc<Mutex<Vec<String>>>;

struct FontProbe(Registrations);

impl TextEngine for FontProbe {
    type Prepared = <ApproxTextEngine as TextEngine>::Prepared;

    fn prepare(&self, text: &AttributedText) -> Self::Prepared {
        ApproxTextEngine.prepare(text)
    }

    fn measure(&self, prepared: &Self::Prepared, max_width: Option<Points>) -> Size<Points> {
        ApproxTextEngine.measure(prepared, max_width)
    }

    fn layout(&self, prepared: &Self::Prepared, size: Size<Points>) -> TextLayout {
        ApproxTextEngine.layout(prepared, size)
    }

    fn register_font(&self, data: &[u8]) -> Result<(), String> {
        let family = font_family(data)?;
        self.0.lock().unwrap().push(family);
        Ok(())
    }
}

/// A loopback server that returns `content`, optionally corrupting the first
/// response or holding a request unanswered for Cancel. Returns its URL and
/// request count.
fn serve(
    content: &'static [u8],
    corrupt_first: bool,
    stall_at: Option<usize>,
) -> (String, Arc<AtomicUsize>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind loopback");
    let url = format!(
        "http://{}/SourceHanSansCN-VF.otf",
        listener.local_addr().unwrap()
    );
    let requests = Arc::new(AtomicUsize::new(0));
    let heard = requests.clone();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { return };
            let mut reader = BufReader::new(stream.try_clone().expect("clone stream"));
            let mut line = String::new();
            while reader.read_line(&mut line).is_ok_and(|n| n > 0) && line != "\r\n" {
                line.clear();
            }
            let request_number = heard.fetch_add(1, Ordering::SeqCst);
            if stall_at == Some(request_number) {
                stream
                    .set_read_timeout(Some(Duration::from_secs(10)))
                    .unwrap();
                let _ = reader.read_line(&mut line);
                continue;
            }
            let mut body = content.to_vec();
            if corrupt_first && request_number == 0 {
                body[0] ^= 0xff;
            }
            let _ = write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                body.len()
            )
            .and_then(|()| stream.write_all(&body));
        }
    });
    (url, requests)
}

fn start() -> (Headless, Registrations) {
    let registrations = Arc::new(Mutex::new(Vec::new()));
    let probe = FontProbe(registrations.clone());
    let mut app =
        Headless::start_with_text_engine(EngineConfig::new(Root::of::<mt_app::App>()), move || {
            probe
        })
        .expect("start the app");
    app.set_viewport(nsh_core::Size::new(Points(1280.0), Points(800.0)))
        .expect("wide viewport");
    app.wait_for("id:shell").expect("shell");
    app.press("id:title-settings").expect("open the menu");
    app.press("id:title-menu-settings").expect("open settings");
    app.wait_for("id:settings-page").expect("settings page");
    (app, registrations)
}

#[test]
fn the_chinese_font_is_downloaded_only_on_request_and_verified() {
    let config = tempfile::tempdir().expect("config dir");
    let installed = config
        .path()
        .join("data/fonts")
        .join(mt_core::companion_font::FILE_NAME);
    // SAFETY: this binary has one test; each app is stopped before the next.
    unsafe {
        std::env::remove_var("MARKTURBO_OPEN");
        std::env::set_var("MARKTURBO_CONFIG_DIR", config.path());
        std::env::set_var("MARKTURBO_DATA_DIR", config.path().join("data"));
    }
    let wait = Duration::from_secs(10);
    fs::create_dir_all(installed.parent().unwrap()).expect("fonts directory");
    fs::write(&installed, FONT).expect("existing font");
    let (mut app, _) = start();
    app.wait(Duration::from_millis(300))
        .expect("idle with no test source");
    assert_eq!(
        fs::read(&installed).expect("the default test source left the file alone"),
        FONT
    );
    drop(app);

    let (url, requests) = serve(FONT, true, Some(2));
    mt_app::ui::fonts::serve_companion_from(&url, FONT);

    fs::write(&installed, INVALID_FONT).expect("corrupt font");
    #[cfg(windows)]
    let deny_delete = {
        use std::os::windows::fs::OpenOptionsExt as _;
        fs::OpenOptions::new()
            .read(true)
            .share_mode(1) // FILE_SHARE_READ, but not FILE_SHARE_DELETE
            .open(&installed)
            .expect("hold the corrupt font open without delete sharing")
    };
    #[cfg(not(windows))]
    {
        fs::remove_file(&installed).expect("clear the font path");
        fs::create_dir(&installed).expect("obstruct the font path");
    }
    let (mut app, registrations) = start();
    app.wait_for_within(
        "text:Removal failed: the font could not be deleted from MarkTurbo's data folder.",
        wait,
    )
    .expect("restore reports the removal failure");
    app.wait_for("id:settings-cjk-font-remove")
        .expect("retry removal on restore");
    assert!(
        !app.screen()
            .matches("id:settings-cjk-font-download")
            .expect("valid download selector"),
        "download is hidden after restore removal failure"
    );
    assert!(
        !app.screen()
            .matches("id:settings-cjk-font-retry")
            .expect("valid retry selector"),
        "retry is hidden after restore removal failure"
    );
    assert!(installed.exists());
    assert!(
        !registrations
            .lock()
            .unwrap()
            .iter()
            .any(|family| family == "KaTeX_Main")
    );
    #[cfg(windows)]
    drop(deny_delete);
    #[cfg(not(windows))]
    fs::remove_dir(&installed).expect("clear the obstruction");
    app.press("id:settings-cjk-font-remove")
        .expect("retry removal");
    app.wait_for("text:Not installed. Chinese text uses the system's font.")
        .expect("no font was applied before removal");
    assert!(!installed.exists());
    assert_eq!(requests.load(Ordering::SeqCst), 0);
    drop(app);

    // Keep restore pending while Settings opens: the stale-part sweep first
    // waits for the lifecycle lock held by another instance.
    fs::write(&installed, INVALID_FONT).expect("corrupt font");
    let stale = config.path().join("data/.companion-font-crashed.part");
    File::create(&stale)
        .and_then(|file| file.set_times(FileTimes::new().set_modified(SystemTime::UNIX_EPOCH)))
        .expect("stale partial download");
    let lock =
        File::create(config.path().join("data/.companion-font.lock")).expect("lifecycle lock file");
    lock.lock().expect("hold the lifecycle lock");
    let (mut app, registrations) = start();
    app.wait_for("text:Applying…").expect("restore pending");
    assert!(
        !app.screen()
            .matches("id:settings-cjk-font-download")
            .expect("valid download selector"),
        "download is hidden while restore is applying"
    );
    assert_eq!(requests.load(Ordering::SeqCst), 0);
    drop(lock);
    app.wait_for("id:settings-cjk-font-download")
        .expect("download offered once restore finds no valid copy");
    assert!(!installed.exists(), "restore discarded the corrupt copy");
    assert!(
        !stale.exists(),
        "restore discarded the stale partial download"
    );
    // Absent: the disclosure and the action, and nothing fetched.
    app.wait_for("id:settings-cjk-font")
        .expect("the Chinese font entry");
    app.wait_for("text:Not installed. Chinese text uses the system's font.")
        .expect("absent status");
    app.wait_for("id:settings-cjk-font-disclosure")
        .expect("the disclosure");
    let disclosure = app
        .screen()
        .texts()
        .into_iter()
        .find(|text| text.starts_with("Source Han Sans CN by Adobe"))
        .expect("the disclosure's text");
    for named in [
        "raw.githubusercontent.com",
        "15.6 MB",
        "SIL Open Font License",
        "No document content is sent",
    ] {
        assert!(disclosure.contains(named), "{named} in {disclosure:?}");
    }
    app.wait_for("id:settings-cjk-font-download")
        .expect("the download action");
    app.wait(Duration::from_millis(300)).expect("idle");
    assert_eq!(
        requests.load(Ordering::SeqCst),
        0,
        "no request before the press"
    );
    assert!(
        !registrations
            .lock()
            .unwrap()
            .iter()
            .any(|family| family == "KaTeX_Main"),
        "the app's embedded faces do not register the fixture family"
    );

    // Failed: the wrong bytes are refused, with the reason and a retry.
    app.press("id:settings-cjk-font-download")
        .expect("download");
    app.wait_for_within(
        "text:Download failed: the file was not the expected font, so it was discarded.",
        wait,
    )
    .expect("the verification failure");
    app.wait_for("id:settings-cjk-font-retry")
        .expect("the retry action");
    assert!(!installed.exists(), "an unverified file was kept");

    // Installed: the retry fetches the expected font and applies it.
    app.press("id:settings-cjk-font-retry").expect("retry");
    app.wait_for_within(
        "text:Installed and applied: Chinese text is set in Source Han Sans CN.",
        wait,
    )
    .expect("installed");
    app.wait_for("id:settings-cjk-font-remove")
        .expect("the remove action");
    assert!(
        registrations
            .lock()
            .unwrap()
            .iter()
            .any(|family| family == "KaTeX_Main"),
        "the companion face was registered"
    );
    assert!(!app.shows("Download and apply"));
    assert_eq!(std::fs::read(&installed).expect("installed file"), FONT);
    assert_eq!(requests.load(Ordering::SeqCst), 2);
    drop(app);

    // A later launch applies it from disk, without a request.
    let (mut app, registrations) = start();
    app.wait_for_within(
        "text:Installed and applied: Chinese text is set in Source Han Sans CN.",
        wait,
    )
    .expect("restored at startup");
    assert!(
        registrations
            .lock()
            .unwrap()
            .iter()
            .any(|family| family == "KaTeX_Main"),
        "the restored companion face was registered"
    );
    assert_eq!(
        requests.load(Ordering::SeqCst),
        2,
        "startup fetched nothing"
    );

    // A disk failure offers another removal, not a new download.
    fs::remove_file(&installed).expect("clear the installed file");
    fs::create_dir(&installed).expect("make the file path unremovable as a file");
    app.press("id:settings-cjk-font-remove").expect("remove");
    app.wait_for_within(
        "text:Removal failed: the font could not be deleted from MarkTurbo's data folder.",
        wait,
    )
    .expect("removal disk failure");
    app.wait_for("id:settings-cjk-font-remove")
        .expect("retry removal");
    assert!(
        !app.screen()
            .matches("id:settings-cjk-font-retry")
            .expect("valid retry selector"),
        "retry is hidden after removal failure"
    );
    assert!(
        !app.screen()
            .matches("id:settings-cjk-font-download")
            .expect("valid download selector"),
        "download is hidden after removal failure"
    );
    fs::remove_dir(&installed).expect("clear the obstruction");
    fs::write(&installed, FONT).expect("put the font back");

    // Removed: the file goes; the face stays until a restart.
    app.press("id:settings-cjk-font-remove").expect("remove");
    app.wait_for_within(
        "text:Removed. Chinese text keeps Source Han Sans CN until MarkTurbo restarts.",
        wait,
    )
    .expect("removed");
    app.wait_for("id:settings-cjk-font-download")
        .expect("downloadable again");
    assert!(!installed.exists(), "the file was removed");
    app.press("id:settings-cjk-font-download")
        .expect("download again");
    app.wait_for("id:settings-cjk-font-cancel")
        .expect("cancel offered");
    app.press("id:settings-cjk-font-cancel").expect("cancel");
    app.wait_for_within(
        "text:Removed. Chinese text keeps Source Han Sans CN until MarkTurbo restarts.",
        wait,
    )
    .expect("cancelling preserves the applied face's restart status");
    assert!(!installed.exists(), "cancelling did not install a file");
    drop(app);

    // Registration failures are reported after a verified file reaches nsh.
    let (invalid_url, invalid_requests) = serve(INVALID_FONT, false, None);
    mt_app::ui::fonts::serve_companion_from(&invalid_url, INVALID_FONT);
    let (mut app, _) = start();
    app.wait_for("id:settings-cjk-font-download")
        .expect("download offered once restore finds no valid copy");
    app.press("id:settings-cjk-font-download")
        .expect("download the invalid font");
    app.wait_for_within(
        "text:The font could not be registered and was not applied.",
        wait,
    )
    .expect("registration failure");
    app.wait_for("id:settings-cjk-font-retry")
        .expect("retry after registration failure");
    assert!(!app.shows("Installed and applied: Chinese text is set in Source Han Sans CN."));
    app.wait_for("id:settings-cjk-font-remove")
        .expect("remove after registration failure");
    assert_eq!(
        std::fs::read(&installed).expect("verified font file"),
        INVALID_FONT
    );
    assert_eq!(invalid_requests.load(Ordering::SeqCst), 1);
    drop(app);

    // Restore uses the same registration result and does not claim success.
    let (mut app, _) = start();
    app.wait_for_within(
        "text:The font could not be registered and was not applied.",
        wait,
    )
    .expect("registration failure on restore");
    app.wait_for("id:settings-cjk-font-remove")
        .expect("remove after registration failure on restore");
    app.press("id:settings-cjk-font-remove")
        .expect("remove the verified but unregistered file");
    app.wait_for("id:settings-cjk-font-download")
        .expect("downloadable again");
    app.wait_for("text:Not installed. Chinese text uses the system's font.")
        .expect("no font is applied after removing an unregistered copy");
    assert!(!installed.exists());
    assert_eq!(invalid_requests.load(Ordering::SeqCst), 1);
}
