//! Every integration test in this directory, linked into one binary for the
//! nightly release-profile run.
//!
//! Under the release profile (thin LTO, one codegen unit) every test binary
//! re-optimizes the whole app at link time, so linking the thirty-odd one-file
//! binaries was nearly all of that run. This target compiles the same files as
//! modules and links once. It is `test = false` in `Cargo.toml`, so plain
//! `cargo test` keeps running the individual binaries and never this one; the
//! nightly selects it with `--test release_suite` under `cargo nextest`, which
//! runs each test in its own process. That matters: most of these tests set
//! process environment variables and rely on being alone in their process.
//!
//! Keep the list in step with the directory; `scripts/tests/test_checks.py`
//! fails when a test file is missing here.

// Each file declares its own `mod support;`, as it does as a standalone binary.
#![allow(clippy::duplicate_mod)]

#[path = "chrome_lane_menu.rs"]
mod chrome_lane_menu;
#[path = "chrome_lane_review.rs"]
mod chrome_lane_review;
#[path = "chrome_lane_title_row.rs"]
mod chrome_lane_title_row;
#[path = "conflict.rs"]
mod conflict;
#[path = "conflict_compare.rs"]
mod conflict_compare;
#[path = "ctrl_enter_editor.rs"]
mod ctrl_enter_editor;
#[path = "hierarchy_layouts.rs"]
mod hierarchy_layouts;
#[path = "hierarchy_welcome.rs"]
mod hierarchy_welcome;
#[path = "language_switch.rs"]
mod language_switch;
#[path = "paste_document.rs"]
mod paste_document;
#[path = "quit_guard.rs"]
mod quit_guard;
#[path = "quit_untitled.rs"]
mod quit_untitled;
#[path = "recovery_restore.rs"]
mod recovery_restore;
#[path = "review_partial.rs"]
mod review_partial;
#[path = "review_selection.rs"]
mod review_selection;
#[path = "review_selection_stale.rs"]
mod review_selection_stale;
#[path = "review_stale.rs"]
mod review_stale;
#[path = "review_supersede.rs"]
mod review_supersede;
#[path = "revision_flow.rs"]
mod revision_flow;
#[path = "save_as_overwrite.rs"]
mod save_as_overwrite;
#[path = "save_twice.rs"]
mod save_twice;
#[path = "settings_form_state.rs"]
mod settings_form_state;
#[path = "shell.rs"]
mod shell;
#[path = "tab_close_guard.rs"]
mod tab_close_guard;
#[path = "tab_switch.rs"]
mod tab_switch;
#[path = "test_vault.rs"]
mod test_vault;
#[path = "theme.rs"]
mod theme;
#[path = "translation_flow.rs"]
mod translation_flow;
#[path = "untitled_discard.rs"]
mod untitled_discard;
#[path = "untitled_recovery.rs"]
mod untitled_recovery;
#[path = "untitled_save.rs"]
mod untitled_save;
#[path = "usage_metrics.rs"]
mod usage_metrics;
#[path = "welcome.rs"]
mod welcome;
#[path = "welcome_leaves.rs"]
mod welcome_leaves;
#[path = "welcome_recents.rs"]
mod welcome_recents;
#[path = "window_close_guard.rs"]
mod window_close_guard;
