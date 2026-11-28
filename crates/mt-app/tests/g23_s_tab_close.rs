//! Closing a tab drops every per-tab entry its request owns: its Review
//! failure, its conflict disk text and its open comparison.

use std::{path::PathBuf, sync::Mutex};

use mt_app::state::AppState;
use nsh_core::prelude::*;
use nsh_engine::EngineConfig;
use nsh_testing::Headless;

static TEST_LOCK: Mutex<()> = Mutex::new(());

static CLOSED_REVIEW_UNAVAILABLE: Mutex<Option<(bool, bool, bool)>> = Mutex::new(None);

#[component]
fn ClosedReviewUnavailableProbe() -> impl IntoElement {
    let state = AppState::new();
    state.settings.update(|settings| {
        settings.model_provider = "no-such-provider".to_owned();
    });
    state.new_document();
    let a = state.active.get().expect("A");
    state.start_review();
    let recorded = state.review_unavailable.get().is_some();
    state.new_document();
    let b = state.active.get().expect("B");
    state.request_close_tab(a);
    state.set_active(a);
    let leaked = state.review_unavailable.get().is_some();
    state.set_active(b);
    let b_clean = state.review_unavailable.get().is_none();
    *CLOSED_REVIEW_UNAVAILABLE.lock().expect("the outcome") = Some((recorded, leaked, b_clean));
    view()
}

#[test]
fn closing_a_tab_drops_its_review_unavailable_reason() {
    let _guard = TEST_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    let config = tempfile::tempdir().expect("config dir");
    // SAFETY: TEST_LOCK serializes app lifetimes and environment changes.
    unsafe {
        std::env::remove_var("MARKTURBO_OPEN");
        std::env::set_var("MARKTURBO_CONFIG_DIR", config.path());
        std::env::set_var("MARKTURBO_DATA_DIR", config.path().join("data"));
        std::env::set_var(
            "MARKTURBO_TEST_LOCK_NAMESPACE",
            std::process::id().to_string(),
        );
    }

    let mut app = Headless::start(EngineConfig::new(nsh_testing::windowed_root::<
        ClosedReviewUnavailableProbe,
    >()))
    .expect("start the probe");
    app.first_render().expect("render the probe");

    let (recorded, leaked, b_clean) = CLOSED_REVIEW_UNAVAILABLE
        .lock()
        .expect("the outcome")
        .take()
        .expect("the probe ran");
    assert!(recorded, "the probe recorded no reason to drop");
    assert!(
        !leaked,
        "the closed tab's reason survived the close and came back on activation"
    );
    assert!(b_clean, "another tab inherited the closed tab's reason");
}

#[component]
fn ClosedConflictDiskProbe() -> impl IntoElement {
    let state = AppState::new();
    let disks = state.conflict_disks;
    let compare = state.conflict_compare;
    let tabs = state.tabs;
    let open = state.clone();
    let read = state.clone();
    let close = state.clone();
    view()
        .flex_col()
        .child(text("Open file").on_press(move || {
            open.open_file(PathBuf::from(
                std::env::var_os("MARKTURBO_OPEN").expect("document path"),
            ));
        }))
        .child(text("Read disk").on_press(move || {
            if let Some(tab) = read.active_tab() {
                let id = tab.id;
                let _ = read.refresh_conflict_disk(&tab);
                read.conflict_compare.set(Some(id));
            }
        }))
        .child(text("Close tab").on_press(move || {
            if let Some(tab) = close.active_tab() {
                close.request_close_tab(tab.id);
            }
        }))
        .child(text(move || format!("tabs:{}", tabs.get().len())))
        .child(text(move || format!("disks:{}", disks.get().len())))
        .child(text(move || {
            format!("compare:{}", u8::from(compare.get().is_some()))
        }))
}

#[test]
fn closing_a_tab_drops_its_conflict_disk_and_comparison() {
    let _guard = TEST_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    let workspace = tempfile::tempdir().expect("workspace");
    let config = tempfile::tempdir().expect("config dir");
    let document = workspace.path().join("plan.md");
    std::fs::write(&document, "# Plan\n").expect("write the document");
    // SAFETY: TEST_LOCK serializes app lifetimes and environment changes.
    unsafe {
        std::env::set_var("MARKTURBO_OPEN", &document);
        std::env::set_var("MARKTURBO_CONFIG_DIR", config.path());
        std::env::set_var("MARKTURBO_DATA_DIR", config.path().join("data"));
        std::env::set_var(
            "MARKTURBO_TEST_LOCK_NAMESPACE",
            std::process::id().to_string(),
        );
    }

    let mut app = Headless::start(EngineConfig::new(nsh_testing::windowed_root::<
        ClosedConflictDiskProbe,
    >()))
    .expect("start the probe");
    app.set_viewport(nsh_core::Size::new(
        nsh_engine::Points(1280.0),
        nsh_engine::Points(800.0),
    ))
    .expect("wide viewport");
    app.first_render().expect("render the probe");

    app.press("text:Open file").expect("open the document");
    app.wait_for("text:tabs:1").expect("the tab opens");
    app.press("text:Read disk").expect("read the disk version");
    app.wait_for("text:disks:1")
        .expect("the disk text is stored");
    app.wait_for("text:compare:1")
        .expect("the comparison is open");

    app.press("text:Close tab").expect("close the tab");
    app.wait_for("text:tabs:0").expect("the tab closes");
    app.wait_for("text:disks:0")
        .expect("the closed tab's disk text is dropped");
    app.wait_for("text:compare:0")
        .expect("the closed tab's comparison is closed");
}
