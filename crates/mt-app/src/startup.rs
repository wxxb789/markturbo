//! Content-free startup measurement signals for the Windows Goal 04 harness.

use std::collections::HashSet;
use std::fs::{File, OpenOptions};
use std::io::{self, Write};
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};

use gpui::{App, KeyBinding, Window, actions};
use serde::Serialize;

const TRACE_PATH_ENV: &str = "MARKTURBO_STARTUP_TRACE";
const TRACE_NONCE_ENV: &str = "MARKTURBO_STARTUP_NONCE";
const TRACE_SCHEMA: &str = "markturbo-startup-v1";

actions!(startup_probe, [AcknowledgeStartupInput]);

/// A content-free milestone emitted only when the Goal 04 harness opts in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum StartupEvent {
    ProcessStarted,
    InitialStateReady(InitialStartupState),
    FirstFramePainted,
    FirstInputHandled,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[allow(dead_code)] // This module is compiled into both binaries; each uses a different subset.
pub enum InitialStartupState {
    Welcome,
    Workspace,
    Bare,
}

impl InitialStartupState {
    fn as_str(self) -> &'static str {
        match self {
            Self::Welcome => "welcome",
            Self::Workspace => "workspace",
            Self::Bare => "bare",
        }
    }
}

impl StartupEvent {
    fn name(self) -> &'static str {
        match self {
            Self::ProcessStarted => "process_started",
            Self::InitialStateReady(_) => "initial_state_ready",
            Self::FirstFramePainted => "first_frame_painted",
            Self::FirstInputHandled => "first_input_handled",
        }
    }

    fn detail(self) -> Option<&'static str> {
        match self {
            Self::InitialStateReady(state) => Some(state.as_str()),
            _ => None,
        }
    }
}

#[derive(Serialize)]
struct TraceRow<'a> {
    schema: &'static str,
    nonce: &'a str,
    pid: u32,
    event: &'static str,
    counter: i64,
    frequency: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    detail: Option<&'static str>,
}

struct TraceWriter<W> {
    output: W,
    nonce: String,
    pid: u32,
    recorded: HashSet<&'static str>,
}

impl<W: Write> TraceWriter<W> {
    fn new(output: W, nonce: String, pid: u32) -> Self {
        Self {
            output,
            nonce,
            pid,
            recorded: HashSet::new(),
        }
    }

    fn record(&mut self, event: StartupEvent, counter: i64, frequency: i64) -> io::Result<bool> {
        if !self.recorded.insert(event.name()) {
            return Ok(false);
        }
        serde_json::to_writer(
            &mut self.output,
            &TraceRow {
                schema: TRACE_SCHEMA,
                nonce: &self.nonce,
                pid: self.pid,
                event: event.name(),
                counter,
                frequency,
                detail: event.detail(),
            },
        )?;
        self.output.write_all(b"\n")?;
        self.output.flush()?;
        Ok(true)
    }

    #[cfg(test)]
    fn into_inner(self) -> W {
        self.output
    }
}

static TRACE: OnceLock<Option<Mutex<TraceWriter<File>>>> = OnceLock::new();

fn trace() -> Option<&'static Mutex<TraceWriter<File>>> {
    TRACE
        .get_or_init(|| {
            let path = PathBuf::from(std::env::var_os(TRACE_PATH_ENV)?);
            let nonce = std::env::var(TRACE_NONCE_ENV).ok()?;
            let output = OpenOptions::new()
                .create_new(true)
                .write(true)
                .open(path)
                .ok()?;
            Some(Mutex::new(TraceWriter::new(
                output,
                nonce,
                std::process::id(),
            )))
        })
        .as_ref()
}

/// Whether startup tracing is active for this process.
pub fn is_enabled() -> bool {
    trace().is_some()
}

/// Record a milestone. Failures remain visible to the harness as a missing event.
pub fn record(event: StartupEvent) {
    let Some(trace) = trace() else { return };
    let Some((counter, frequency)) = performance_counter() else {
        return;
    };
    let _ = trace
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .record(event, counter, frequency);
}

/// Install the probe-only F24 binding used to prove input reached GPUI.
pub fn init(cx: &mut App) {
    if is_enabled() {
        cx.bind_keys([KeyBinding::new("f24", AcknowledgeStartupInput, None)]);
    }
}

/// Record the first completed application frame from the following frame turn.
pub fn schedule_first_frame_milestone(window: &Window) {
    if !is_enabled() {
        return;
    }
    window.on_next_frame(|_, _| record(StartupEvent::FirstFramePainted));
}

#[cfg(target_os = "windows")]
fn performance_counter() -> Option<(i64, i64)> {
    use windows::Win32::System::Performance::{QueryPerformanceCounter, QueryPerformanceFrequency};

    let mut counter = 0;
    let mut frequency = 0;
    unsafe {
        QueryPerformanceCounter(&mut counter).ok()?;
        QueryPerformanceFrequency(&mut frequency).ok()?;
    }
    Some((counter, frequency))
}

#[cfg(not(target_os = "windows"))]
fn performance_counter() -> Option<(i64, i64)> {
    use std::time::Instant;

    static START: OnceLock<Instant> = OnceLock::new();
    let nanos = START.get_or_init(Instant::now).elapsed().as_nanos();
    Some((i64::try_from(nanos).ok()?, 1_000_000_000))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trace_writer_records_versioned_content_free_events_once() {
        let mut trace = TraceWriter::new(Vec::new(), "nonce-1".into(), 42);

        assert!(
            trace
                .record(StartupEvent::ProcessStarted, 100, 10_000_000)
                .unwrap()
        );
        assert!(
            !trace
                .record(StartupEvent::ProcessStarted, 101, 10_000_000)
                .unwrap()
        );
        assert!(
            trace
                .record(
                    StartupEvent::InitialStateReady(InitialStartupState::Welcome),
                    200,
                    10_000_000,
                )
                .unwrap()
        );

        let output = String::from_utf8(trace.into_inner()).unwrap();
        let rows: Vec<serde_json::Value> = output
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0]["schema"], "markturbo-startup-v1");
        assert_eq!(rows[0]["nonce"], "nonce-1");
        assert_eq!(rows[0]["pid"], 42);
        assert_eq!(rows[0]["event"], "process_started");
        assert_eq!(rows[0]["counter"], 100);
        assert_eq!(rows[0]["frequency"], 10_000_000);
        assert!(rows[0].get("detail").is_none());
        assert_eq!(rows[1]["detail"], "welcome");
        assert!(!output.contains("path"));
        assert!(!output.contains("text"));
    }

    #[test]
    fn production_hooks_measure_after_render_without_touching_webview() {
        let startup = include_str!("startup.rs");
        let main = include_str!("main.rs");
        let bare_shell = include_str!("bin/markturbo-gpui-shell.rs");
        let workspace = include_str!("views/workspace.rs");
        let render = workspace
            .split_once("impl Render for Workspace")
            .expect("the Workspace render implementation")
            .1;

        assert!(main.contains("schedule_first_frame_milestone(window)"));
        assert!(
            startup
                .contains("window.on_next_frame(|_, _| record(StartupEvent::FirstFramePainted))")
        );
        for shared_setup in [
            "GPUI_DISABLE_DIRECT_COMPOSITION",
            "application().with_assets(Assets)",
            "set_app_identity(APP_ID, \"markturbo\")",
            "gpui_component::init(cx)",
            "size(px(1400.0), px(900.0))",
            "gpui_component::TitleBar::window_options()",
            "window.set_window_title(\"markturbo\")",
        ] {
            assert!(
                main.contains(shared_setup),
                "main is missing {shared_setup}"
            );
            assert!(
                bare_shell.contains(shared_setup),
                "bare shell is missing {shared_setup}"
            );
        }
        assert!(bare_shell.contains("StartupEvent::ProcessStarted"));
        assert!(bare_shell.contains("startup::init(cx)"));
        assert!(bare_shell.contains("AcknowledgeStartupInput"));
        assert!(bare_shell.contains("StartupEvent::FirstInputHandled"));
        assert!(bare_shell.contains("InitialStartupState::Bare"));
        assert!(bare_shell.contains("schedule_first_frame_milestone(window)"));
        assert!(workspace.contains("AcknowledgeStartupInput"));
        assert!(workspace.contains("StartupEvent::FirstInputHandled"));
        assert!(!render.contains("StartupEvent::FirstFramePainted"));
        assert!(!render.contains("sync_webview"));
    }
}
