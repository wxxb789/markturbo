# Fragile-test audit: wxxb789/markturbo

Snapshot: main 5939c2d. CI window: last 200 runs, 2026-11-14 to 2026-11-27 (37 failed: 31 Pull request, 6 Nightly).
Method: `gh run list` plus `gh run view --log-failed` for all 37 failures; nextest `FAIL [` lines and `panicked at` messages extracted.

Caveats:
- `gh run list` returns only the latest attempt, and every run shows attempt 1. No run was re-run by hand, so there is no (a) "failed then passed on the same SHA" evidence. Evidence is (b) one-OS-only failure and (c) failure on one push, pass on the next, on code unrelated to the test.
- Most PR failures are ordinary development failures that a code change fixed. Those are not counted as flaky, only listed.
- Run ids below are GitHub Actions run ids.

## 1. CI history

### 1a. Intermittent or flaky candidates (ranked by evidence)

| Test | Evidence | Classification | Recommendation |
| --- | --- | --- | --- |
| mt-app tests/palette_folder.rs `choosing_a_palette_file_focuses_the_opened_editor` | 3 failures. Linux only on 45919399991 (3dc689b) and 45931945976 (94e3249), consecutive pushes of feat/g22b-files. Windows only on 45799655488 (2472544, fix/g20-walk-findings). Each time the other OS passed the same SHA, and later pushes passed. The assertion at palette_folder.rs:144 is "the click left the keyboard off the opened editor". The test presses a key right after `wait_for("text:# Notes")` (lines 142-143) and never waits for focus. | flaky-timing | delete. Focus-after-open is covered by `focus_after_close.rs`, which waits on the focus command history. |
| mt-app tests/reload_edit_race.rs `edits_during_the_reload_read_stay_in_the_buffer` | Linux-only on 45448141802, 45535145920, 45562845636, 45568306867 (4 PR runs on feat/goal-20-a; Windows passed each). Failure: "the reload retired the recovery record" (reload_edit_race.rs:101 at that time). The test relies on fixed `app.wait(1s)` (line 68) and `app.wait(3s)` (line 96) for the background reload and the checkpoint tick. It reads the process-global recovery dir. | flaky-timing, order-dependent | delete, or replace with a unit test on the reload/dirty-revision reducer in `state.rs`. |
| reload_edit_race.rs `a_stale_reload_answer_does_not_reconflict_a_synced_tab`, `a_stale_reload_failure_writes_no_notice` | Nightly main 45618159023 (c0b2256), macOS only: "the first reload's read to reach its gate never happened in 5000 ms" (reload_edit_race.rs:161, :261). Windows and Linux were green. The run predates #80 (marker-wait fix), which is already in the snapshot. docs/TODO.md:171 says the no-notice assertion "can still read the screen before a post-marker notice batch". | flaky-timing | delete both. Re-cover the stale-answer rule as a pure state test. |
| mt-app tests/recovery_restore.rs `recovery_actions_stay_pressable_at_narrow_widths` | Windows only, 45803915948 (6e00d52) and 45830819503 (261abc3). In the window these two pushes are the only failures of this test. The wait for the editor text times out at 5000 ms. The body starts with `if !cfg!(windows) { return; }`, so on Linux and macOS it is a green no-op. | flaky-timing, false-positive | delete. Replace with a layout assertion on the recovery row (a rect-width check in a ui unit test). |
| mt-app tests/refine_state_status.rs `the_status_line_names_the_file_and_fades_while_an_error_stays` | Nightly main 45997558749 (5939c2d, after #80), macOS only. `wait_for("id:status >> text:Opened plan.md")` timed out at 5000 ms. The test races a transient message against real elapsed time: `run_for(2000 ms)` then `run_for(1500 ms)` (refine_state_status.rs:49-54). | flaky-timing | delete. Replace with a unit test on the status-message expiry state, with an injected clock. |
| mt-app tests/recovery_undo.rs `closing_the_review_panel_carries_the_live_undo_back_to_the_editor` and `settings_opened_from_the_review_panel_keeps_the_live_undo` | Nightly 45618159023: both failed on Windows heavy tests (pre-#80). PR 45554765658 (Linux only): "open recovery store: Unavailable(current-user DPAPI is only available on Windows)" (recovery_undo.rs:76), so a Windows-only test body ran on Linux. `app.wait(500ms)` at lines 467 and 509. | order-dependent, false-positive | delete the two. The Undo-carry rule belongs in a state unit test. |
| ui::rail::tests `hovered_sidebar_and_focused_actions_track_the_current_theme` | Linux only, 45932967009 (055c4fb). The selector `text:Inactive` matched 2 views (rail.rs:1276). Fixed on the next push. A text-selector ambiguity, not a product defect. | implementation-pinned (selector) | keep, low priority. Use `id:` selectors. |

### 1b. Failed on one platform, deterministic, not flaky (listed for completeness)

| Test | Evidence | Note |
| --- | --- | --- |
| mt-core document::io::tests `skill_save_*`, `relocated_skill_root_never_reports_stale_preservation_paths`, `skill_recreation_does_not_clobber_a_creator_after_precheck`, `successful_skill_save_refreshes_only_an_existing_origin` (8 tests) | Nightly main on macOS-15 only: 43706154301, 43856286169, 44077710691, 44297016169 (4 nights in a row, 11-15 to 11-18). Windows and Linux passed. | Platform-semantic mismatch (filesystem races on macOS), cleared by later changes. Not flaky. Two of them still exist (io.rs:1777, 2031) and have run green since. |
| mt-core usage::tests `daily_schema_aggregates_events_lenses_durations_and_hunk_counts`, `export_contains_only_version_and_aggregates_and_never_overwrites_a_file` | Windows only, 44759236219 (ui/cjk-font-download). `Io(WouldBlock)` from the settings write lock (usage/tests.rs:93, :209). Not seen again. | order-dependent: tests share a settings lock with parallel tests. One occurrence. Not reproduced since. |
| `ui::document::tests::the_cjk_fallback_prefers_dengxian` (no longer in snapshot) | Windows 43814144446 and 43818911354. Failed because DengXian is absent on the runner. | environment-dependent test. Already removed. |
| `ui::preview::tests::the_field_web_pane_follows_the_active_tab` (renamed) | Windows 44910658449 and 44918233719. The web pane is disabled when the runner is elevated. | environment-dependent. Already changed. |
| goal22_live_disk / goal22_drop symlink tests (unmerged feat/g22b-files only: `deleting_a_symlink_target_inside_the_workspace_raises_the_missing_banner`, `a_differently_cased_workspace_still_sees_its_symlink_target_deleted`, `dropping_a_symlink_to_an_open_target_focuses_the_existing_tab`) | Windows only, 4 consecutive failing runs: 45942810059, 45959620248 (two tests), 45974198370, 45991144394. Not in snapshot. | Likely a real Windows symlink-semantics defect, or a runner without symlink privilege. Cause unknown. Probe: run on a runner with Developer Mode symlinks enabled. If the runner is the cause, `#[cfg(windows)]` skip these. |

### 1c. Non-test CI failures (for context, no deletion)

- 43793455029, 43810456308, 43813832033: preflight `git` exit 2 (3 consecutive on feat/afterglow-ui; tooling or ref problem).
- 45331050154: "no test target named `suite`" (the suite binary was missing from the manifest at that commit).
- 45794450610: compile error E0433 `Root` not found.
- 45004713782: Windows `Process completed with exit code 1` with no test name captured.

### 1d. Python tooling test that failed in CI

- `scripts.tests.test_privacy.CheckIntegrationTests.test_ci_does_not_run_the_release_binary_privacy_scan`: 45519280917 (perf/suite-test-gate). It broke when `checks.ci()` was restructured, because it mocks and asserts the internal call graph. Classification: implementation-pinned. Delete (see section 3).

## 2. docs mentions (TODO.md, goal-*-acceptance.md, test-ablation.md)

No "flaky" or "pre-existing failure" mention exists for `palette_folder::choosing_a_palette_file_focuses_the_opened_editor`. The CI log is the only evidence for it. The goal-*-acceptance.md files mention only pre-existing Clippy warnings (goal-10:32, goal-19:68), not tests.

| Test or topic | Source | Quote or evidence | Classification | Recommendation |
| --- | --- | --- | --- | --- |
| mt-app tests/focus_after_close.rs (`focus_lands_on_the_editor_the_next_tab_mounts`, `ctrl_n_after_escape_focuses_the_new_editor`) | docs/TODO.md:168-171 | "asserts that the new editor's first focus arrives within the app's own retry window (`FOCUS_RETRY_FRAMES`), not that a key typed at the exact frame ... lands ... Tighten both only if nsh-testing gains a frame-exact or task-completion hook." Test: focus_after_close.rs:44-60 polls `next_change(25 ms)` up to `FOCUS_RETRY_FRAMES` times. Never seen failing in CI. | implementation-pinned (frame budget), partly flaky-timing | replace with a behaviour test: assert the key lands after `wait_until` the focus command history shows the editor, with no frame budget. Keep the behaviour, drop the `FOCUS_RETRY_FRAMES` bound. |
| mt-app tests/reload_edit_race.rs | docs/TODO.md:171 | "`reload_edit_race`'s no-notice assertion can still read the screen before a post-marker notice batch." Confirmed by CI (section 1a). | flaky-timing | delete (see 1a). |
| mt-core tests/performance.rs wall-clock tests | docs/TODO.md:130-133 (nextest gate follow-ups); docs/verification.md:123 ("Wall-clock is not a PR gate") | "The `serial` test group keeps the mt-core timing tests off each other, but they can still run beside other test binaries. `performance.rs` asserts wall-clock bounds." Non-ignored ones: `parses_a_10k_line_document_quickly` (2 s, line 62), `repeated_edits_do_not_accumulate_cost`, `setting_identical_source_is_free` (<100 ms, line 190), `diagram_heavy_document_parses_without_rendering` (<500 ms, line 210), `outline_and_block_lookup_are_cheap_on_a_large_document` (<10 ms and <1 s, lines 225 and 237). No CI failure observed. | flaky-timing (latent) | delete the four with bounds of 500 ms or less. The doc's own policy says wall-clock is not a PR gate. |
| scripts/tests/test_checks.py `DoctestGateTests.test_every_enabled_workspace_doctest_has_a_gate_step` | docs/TODO.md:134-135 | "checks only that `--doc` appears. If doctests are enabled again, verify which crates the command actually covers." The test is already weak and parses Cargo manifests. | source-scan (weak) | delete. |
| scripts/tests/test_native_runtime_uia.py `test_sidebar_and_view_selectors_are_in_the_product_ui` | docs/TODO.md:175-178 | Cuts each Rust file at `"\n#[cfg(test)]\nmod tests"` without asserting the marker was found. | source-scan | delete. The Rust Headless suite already proves the ids exist. |
| `state.rs` / `TrackTween` double reduced-motion guard | docs/TODO.md:172-174 | "removing either alone is unobservable". Redundant guard, not a test. | n/a | Keep one guard. |
| mt-app tests/translation_flow.rs Quit test | docs/TODO.md:150-153 | "does not prove the translation lands while the picker is unanswered" (nsh-testing answers SaveDialog at once). | false-positive (cannot fail for the claimed reason) | delete the Quit assertion or the test. |
| goal17_settings `settings_page_rows_land_on_the_four_point_grid` | docs/TODO.md:164-167 | Pixel-grid pin that goes red when a status line changes height (PR #77 review). | implementation-pinned | replace with: assert the sections do not overlap and the API key row follows the status row. |
| docs/test-ablation.md | whole file | The 2026-08 ablation removed redundant tests but never targeted flaky or source-scan tests. Its E8 retains the startup smoke guard. | n/a | no action. |

## 3. Brittle by construction (snapshot 5939c2d)

| Location | What it does | Classification | Recommendation |
| --- | --- | --- | --- |
| crates/mt-app/src/ui/motion.rs:349-375 `ui_durations_and_beziers_are_tokenized` | Walks `src/ui/**/*.rs`, strips whitespace, and scans for `.duration(<digit>` and `Easing::Bezier(`. Any new formatting or macro form passes or fails by spelling. AGENTS.md says "source scans guard structure; they do not prove GUI behavior." | source-scan | delete. Clippy `disallowed_methods` or a code-review rule covers it. |
| scripts/tests/test_native_runtime_uia.py:574-590 `test_sidebar_and_view_selectors_are_in_the_product_ui` | Reads rail.rs, shell.rs and titlebar.rs as text and asserts `f'"{automation_id}"'` is a substring. Splits on the `mod tests` marker with no check that it was found (TODO.md:175). | source-scan | delete. |
| scripts/tests/test_native_runtime_uia.py:562-572 `test_selectors_match_the_nsh_production_source` | Asserts harness constants equal literal copies of the same strings. A tautology despite the name. | implementation-pinned | delete. |
| scripts/tests/test_native_visual.py:208-222 `test_documented_goal17_visual_command_parses_through_mt_cli` | Reads docs/goal-17-acceptance.md and picks the first line starting with `uv run ... mt.py visual`. Breaks when the doc is reworded or the goal is archived. | source-scan (doc) | delete. |
| scripts/tests/test_checks.py:145-168 `test_rust_steps_run_in_contract_order_with_the_requested_profile` | Asserts the exact argv list of 5 cargo commands (nextest flags, features). Red on any flag change. | implementation-pinned | delete. |
| scripts/tests/test_checks.py:170-190 `test_mt_app_runs_as_one_suite_binary_under_nextest`, `test_mt_core_keeps_the_workspace_feature_set_under_nextest` | Each asserts one exact argv tuple already pinned by the test above. | implementation-pinned (duplicate) | delete. |
| scripts/tests/test_checks.py:192-198 `test_no_plain_cargo_test_remains_in_the_rust_gate` | Asserts no `cargo test` appears in the command list. | implementation-pinned | delete. |
| scripts/tests/test_checks.py:102-128 `test_ci_stops_before_any_nextest_run_when_clippy_fails` | Asserts the exact 3-call sequence after a mocked Clippy failure. | implementation-pinned | replace with: assert the check raises `CheckFailure` and no `nextest run` call was made (not the full argv). |
| scripts/tests/test_checks.py:213-232 `test_nextest_is_detected_through_cargo`, `test_missing_nextest_fails_with_the_install_command` | Mocks `run` and asserts one exact call or an exact message string. | implementation-pinned | delete the first, keep the second as the user-visible message. |
| scripts/tests/test_checks.py:284-320 `test_every_enabled_workspace_doctest_has_a_gate_step` | Parses all workspace Cargo.toml files and checks `"--doc"` in argv. | source-scan | delete. |
| scripts/tests/test_checks.py:319-330 `test_suite_includes_every_app_integration_test` | Regex over `tests/suite.rs` `#[path = "..."] mod x;` and globs `tests/*.rs`. | source-scan | keep. It guards a real silent-test-loss hazard. Replace later with a build.rs or macro. |
| scripts/tests/test_privacy.py:402-411 `test_ci_does_not_run_the_release_binary_privacy_scan` | Mocks `fast`, `cargo`, `run` and `privacy.scan` and asserts `scan.assert_not_called()`. Failed in CI on 45519280917 when `ci()` was restructured. | implementation-pinned, false-positive | delete. |
| scripts/tests/test_privacy.py:444-445 `test_tooling_manifest_includes_privacy_tests` | Asserts a module name appears in `checks.TOOLING_TESTS`. | implementation-pinned | delete. |
| crates/mt-app/tests/palette_folder.rs:136-151 | Presses a key immediately after the notes document text appears, with no focus wait. | flaky-timing | see 1a, delete. |
| crates/mt-app/tests/reload_edit_race.rs:68, 96 | Fixed `app.wait(1 s)` and `app.wait(3 s)`; global recovery dir. | flaky-timing | see 1a. |
| crates/mt-app/tests/review_supersede.rs:97 | `app.wait(Duration::from_millis(3200))`, a sleep sized to a product timer. | flaky-timing | replace with a `wait_until` on the expiry state. |
| crates/mt-app/tests/{context_escape.rs:216,249 (600 ms), cjk_font_download.rs:136,248 (300 ms), translation_cancel.rs:98 (600 ms), goal20_settings_lane.rs:101 (500 ms), recovery_undo.rs:467,509 (500 ms), review_preserve.rs:537 (200 ms), sidebar_view.rs:874 (32 ms)} | Fixed-duration `app.wait` used as "let the async task finish". 24 such calls across 10 files; `revision_flow.rs` has 11. Of these tests, only the reload_edit_race and recovery_* ones have failed in CI so far. | flaky-timing (latent) | replace each with `wait_until(<observable condition>)`. Delete the ones that only assert absence after a sleep (a sleep-then-absence check cannot fail meaningfully). |
| crates/mt-app/tests/focus_after_close.rs:44-60 | Asserts the editor takes focus within `FOCUS_RETRY_FRAMES` frames at 25 ms per poll. Pins the app's internal retry constant. | implementation-pinned | see section 2. |
| crates/mt-app/src/ui/shell.rs:2731 `a_live_undo_stays_clear_of_the_preview_at_220_points` | Pixel-literal layout assertion ("the Undo starts at x36, left of the column at x44"). Failed on both OS for 45807186895, 45816760805 and 45825127315 (3 consecutive pushes) while the layout was being tuned. | implementation-pinned | replace with: assert the Undo rect and the preview-column rect do not intersect, with no literal x values. |
| crates/mt-app/tests/goal17_settings.rs:91 `settings_page_rows_land_on_the_four_point_grid` | Pins every row to a 4 px grid. The TODO notes it breaks when status text height changes. | implementation-pinned | delete, or limit to the named hero rows. |
| crates/mt-app/src/ui/rail.rs:1251-1276 | Uses `text:Inactive` selectors (a selector ambiguity failed Linux CI once). | implementation-pinned | switch to `id:` selectors. |
| crates/mt-app/tests/recovery_restore.rs:166-168, recovery_undo.rs:76 | `if !cfg!(windows) { return; }`: a green no-op on 2 of 3 CI OSes. | false-positive | delete, or mark `#[cfg(windows)]` so the skip is visible. |
| crates/mt-core/tests/performance.rs:53-66, 153-240 | Non-ignored wall-clock bounds from 10 ms to 2 s. | flaky-timing (latent) | delete the ones at or under 500 ms. |
| crates/mt-core/tests/context.rs:1519 and 1533 | `include_str!` and `CARGO_MANIFEST_DIR` reads of data files (not .rs). Data fixtures, acceptable. | fixture read | keep. |

Search note: a search for `include_str!` over `.rs` in `crates/**` found no test that asserts on Rust source text. The only Rust-source scan is motion.rs:350 (above). Python tests reading `crates/**/*.rs` are only `test_native_runtime_uia.py:575-578`.

## Ranked top-15 deletion list

Ranked by evidence of CI failures first, then by brittleness.

1. crates/mt-app/tests/palette_folder.rs `choosing_a_palette_file_focuses_the_opened_editor`. 3 CI failures on 2 OSes; no focus wait; covered by focus_after_close.
2. crates/mt-app/tests/reload_edit_race.rs `edits_during_the_reload_read_stay_in_the_buffer`. 4 consecutive Linux failures; fixed 1 s and 3 s sleeps; global recovery dir.
3. crates/mt-app/tests/reload_edit_race.rs `a_stale_reload_answer_does_not_reconflict_a_synced_tab`. macOS nightly failure; TODO-flagged.
4. crates/mt-app/tests/reload_edit_race.rs `a_stale_reload_failure_writes_no_notice`. Same nightly failure; TODO-flagged.
5. crates/mt-app/tests/refine_state_status.rs `the_status_line_names_the_file_and_fades_while_an_error_stays`. macOS nightly failure after #80; real-time fade windows.
6. crates/mt-app/tests/recovery_restore.rs `recovery_actions_stay_pressable_at_narrow_widths`. 2 Windows failures; no-op on Linux and macOS.
7. crates/mt-app/tests/recovery_undo.rs `closing_the_review_panel_carries_the_live_undo_back_to_the_editor`. Windows-only logic that failed on Linux (DPAPI) and in the Windows nightly; 500 ms sleeps.
8. crates/mt-app/tests/recovery_undo.rs `settings_opened_from_the_review_panel_keeps_the_live_undo`. Same nightly failure.
9. scripts/tests/test_privacy.py `test_ci_does_not_run_the_release_binary_privacy_scan`. Failed in CI when `ci()` was refactored; mock call-graph pin.
10. scripts/tests/test_checks.py `test_rust_steps_run_in_contract_order_with_the_requested_profile`, plus the duplicates `test_mt_app_runs_as_one_suite_binary_under_nextest`, `test_mt_core_keeps_the_workspace_feature_set_under_nextest` and `test_no_plain_cargo_test_remains_in_the_rust_gate`. Exact-argv pins that break on any gate flag change.
11. crates/mt-app/src/ui/motion.rs `ui_durations_and_beziers_are_tokenized`. Source scan over all of `src/ui`.
12. scripts/tests/test_native_runtime_uia.py `test_sidebar_and_view_selectors_are_in_the_product_ui` and `test_selectors_match_the_nsh_production_source`. Source scan plus a tautology.
13. crates/mt-core/tests/performance.rs `setting_identical_source_is_free`, `diagram_heavy_document_parses_without_rendering` and `outline_and_block_lookup_are_cheap_on_a_large_document`. Wall-clock bounds of 10 ms to 500 ms that docs/verification.md:123 says are not a PR gate.
14. scripts/tests/test_checks.py `test_every_enabled_workspace_doctest_has_a_gate_step`. TODO-flagged weak source scan, plus scripts/tests/test_native_visual.py `test_documented_goal17_visual_command_parses_through_mt_cli`, which reads a doc.
15. crates/mt-app/src/ui/shell.rs `a_live_undo_stays_clear_of_the_preview_at_220_points`. Pixel literals; 3 consecutive failures on both OSes. Replace with a non-overlap assertion rather than delete outright. Do the same for goal17_settings `settings_page_rows_land_on_the_four_point_grid`.

Not on the list, but watch: the unmerged goal22 symlink tests (Windows-only, 4 consecutive failing runs, cause unknown) and the 24 fixed-duration `app.wait` calls across mt-app tests.

