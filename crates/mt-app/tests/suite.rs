//! Every integration test in this directory, linked into one binary for the
//! app suites: the per-pull-request `ci` runs and the nightly release-profile
//! run.
//!
//! Each test binary links the whole app, so one binary per
//! file dominated the suite's build time. This target compiles the same files
//! as modules and links once. It is `test = false` in `Cargo.toml`, so plain
//! `cargo test` keeps running the individual binaries and never this one; the
//! workflows select it with `--test suite` under `cargo nextest`, which runs
//! each test in its own process. That matters: most of these tests set process
//! environment variables and rely on being alone in their process.
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
#[path = "cjk_font_download.rs"]
mod cjk_font_download;
#[path = "conflict.rs"]
mod conflict;
#[path = "conflict_compare.rs"]
mod conflict_compare;
#[path = "conflict_compare_missing.rs"]
mod conflict_compare_missing;
#[path = "context_escape.rs"]
mod context_escape;
#[path = "context_new_root.rs"]
mod context_new_root;
#[path = "ctrl_enter_editor.rs"]
mod ctrl_enter_editor;
#[path = "duplicate_guard_names.rs"]
mod duplicate_guard_names;
#[path = "focus_after_close.rs"]
mod focus_after_close;
#[path = "folder_open_token.rs"]
mod folder_open_token;
#[path = "goal17_settings.rs"]
mod goal17_settings;
#[path = "goal20_settings_lane.rs"]
mod goal20_settings_lane;
#[path = "goal22_drop.rs"]
mod goal22_drop;
#[path = "goal22_live_disk.rs"]
mod goal22_live_disk;
#[path = "guard_untitled_cancel.rs"]
mod guard_untitled_cancel;
#[path = "guard_untitled_save.rs"]
mod guard_untitled_save;
#[path = "hierarchy_layouts.rs"]
mod hierarchy_layouts;
#[path = "hierarchy_welcome.rs"]
mod hierarchy_welcome;
#[path = "language_switch.rs"]
mod language_switch;
#[path = "palette_folder.rs"]
mod palette_folder;
#[path = "paste_document.rs"]
mod paste_document;
#[path = "quit_exits_macos.rs"]
mod quit_exits_macos;
#[path = "quit_guard.rs"]
mod quit_guard;
#[path = "quit_untitled.rs"]
mod quit_untitled;
#[path = "recovery_restore.rs"]
mod recovery_restore;
#[path = "recovery_undo.rs"]
mod recovery_undo;
#[path = "refine_keys_shortcuts.rs"]
mod refine_keys_shortcuts;
#[path = "refine_state_pane.rs"]
mod refine_state_pane;
#[path = "refine_state_status.rs"]
mod refine_state_status;
#[path = "reload_edit_race.rs"]
mod reload_edit_race;
#[path = "review_partial.rs"]
mod review_partial;
#[path = "review_preserve.rs"]
mod review_preserve;
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
#[path = "sidebar_view.rs"]
mod sidebar_view;
#[path = "tab_close_guard.rs"]
mod tab_close_guard;
#[path = "tab_switch.rs"]
mod tab_switch;
#[path = "test_vault.rs"]
mod test_vault;
#[path = "theme.rs"]
mod theme;
#[path = "translation_cancel.rs"]
mod translation_cancel;
#[path = "translation_flow.rs"]
mod translation_flow;
#[path = "untitled_discard.rs"]
mod untitled_discard;
#[path = "untitled_numbers.rs"]
mod untitled_numbers;
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
#[path = "window_minimum.rs"]
mod window_minimum;
