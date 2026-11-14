//! Helpers shared by the headless integration tests.
//!
//! A headless app only advances while a step or a pump runs: `press`,
//! `answer_dialog` and `settle` can return before the task they started has
//! shown its dialog or written its file, and a `thread::sleep` in the test
//! thread never advances the app at all. These waits pump frames until the
//! observable outcome lands, bounded, and report the on-screen texts when it
//! never does.
#![allow(dead_code)] // Each test binary uses a subset of these helpers.

use std::path::Path;
use std::time::{Duration, Instant};

use nsh_testing::Headless;

/// How long one pumped frame waits for the app before the loop looks again.
const STEP: Duration = Duration::from_millis(25);

fn pump(app: &mut Headless) {
    app.next_change(STEP).expect("settle while waiting");
}

/// Waits until `path` exists.
pub fn wait_for_path(app: &mut Headless, path: &Path, what: &str, timeout: Duration) {
    let deadline = Instant::now() + timeout;
    while !path.exists() {
        assert!(
            Instant::now() < deadline,
            "{what}: {:?} on screen",
            app.screen().texts()
        );
        pump(app);
    }
}

/// Waits until the text at `path` satisfies `holds`.
pub fn wait_for_file(
    app: &mut Headless,
    path: &Path,
    holds: impl Fn(&str) -> bool,
    what: &str,
    timeout: Duration,
) {
    let deadline = Instant::now() + timeout;
    loop {
        let text = std::fs::read_to_string(path).unwrap_or_default();
        if holds(&text) {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "{what}: {path:?} holds {text:?} with {:?} on screen",
            app.screen().texts()
        );
        pump(app);
    }
}

/// Waits until the app shows a dialog.
pub fn wait_for_pending_dialog(app: &mut Headless, what: &str, timeout: Duration) {
    let deadline = Instant::now() + timeout;
    while app.pending_dialog().is_none() {
        assert!(
            Instant::now() < deadline,
            "{what}: {:?} on screen",
            app.screen().texts()
        );
        pump(app);
    }
}
