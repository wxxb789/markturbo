//! The companion Chinese font: Settings says what a download would fetch
//! before anything is fetched, a failed download says why and offers to try
//! again, a verified one is installed and applied, a later launch applies it
//! without a request, and Remove deletes it.
//!
//! The font comes from a loopback server through the test-build seam
//! (`serve_companion_from`); no test build can reach the pinned remote host.

use std::io::{BufRead as _, BufReader, Write as _};
use std::net::TcpListener;
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};
use std::time::Duration;

use nsh_engine::{EngineConfig, Points, Root};
use nsh_testing::Headless;

/// A real face stands in for the companion, so registration succeeds.
const FONT: &[u8] = include_bytes!("../fonts/martian/MartianMono-CnRg.ttf");

/// A loopback server whose first answer is the wrong bytes and every later
/// one the expected font; its URL and the count of requests it heard.
fn serve() -> (String, Arc<AtomicUsize>) {
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
            let mut body = FONT.to_vec();
            if heard.fetch_add(1, Ordering::SeqCst) == 0 {
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

fn start() -> Headless {
    let mut app =
        Headless::start(EngineConfig::new(Root::of::<mt_app::App>())).expect("start the app");
    app.set_viewport(nsh_core::Size::new(Points(1280.0), Points(800.0)))
        .expect("wide viewport");
    app.wait_for("id:shell").expect("shell");
    app.press("id:title-settings").expect("open the menu");
    app.press("id:title-menu-settings").expect("open settings");
    app.wait_for("id:settings-page").expect("settings page");
    app
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
    let (url, requests) = serve();
    mt_app::ui::fonts::serve_companion_from(&url, FONT);
    let wait = Duration::from_secs(10);

    // Absent: the disclosure and the action, and nothing fetched.
    let mut app = start();
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
    assert!(!app.shows("Download and apply"));
    assert_eq!(std::fs::read(&installed).expect("installed file"), FONT);
    assert_eq!(requests.load(Ordering::SeqCst), 2);
    drop(app);

    // A later launch applies it from disk, without a request.
    let mut app = start();
    app.wait_for_within(
        "text:Installed and applied: Chinese text is set in Source Han Sans CN.",
        wait,
    )
    .expect("restored at startup");
    assert_eq!(
        requests.load(Ordering::SeqCst),
        2,
        "startup fetched nothing"
    );

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
}
