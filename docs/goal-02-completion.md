# Goal 02 Completion and Acceptance Report

**Date:** 2026-08-31
**Goal:** [Goal 02 - Guarantee user-text safety](goals/archive/02-guarantee-user-text-safety.md)
**Status:** COMPLETE / 16 OF 16 REQUIRED CASES PROVEN. Current debug, recovery-root, formatting, lint, diff, release, capacity, executable-build, harness-unit, parser, Python compile, clipboard-free native destructive-acceptance, final report-only review, Case 10 symlink, and requirement-by-requirement audit evidence is recorded below.

Commands below preserve the exact historical evidence. Use
[`scripts/README.md`](../scripts/README.md) for current tooling entry points.

## Delivered safety boundaries

- Dirty tab close, window close, and workspace replacement share one Save / Discard / Cancel lifecycle decision. Save must succeed for the exact prompted snapshot before destruction proceeds, while Discard and Cancel retain their explicit meanings.
- `AsyncSnapshot` combines the exact editor revision and text with source generation. Translation, reload, and reparse results cannot cross a newer edit or Save As identity boundary.
- Save safety covers same-length rewrites with unchanged/coarsened mtimes, Windows file-object identity, missing or renamed sources, encoding round trips, explicit UTF-8 conversion, and symbolic-link preservation. A concurrent replacement leaves the editor dirty and reports preserved artifacts instead of claiming success.
- Recovery remains optional application state. Dirty buffers are protected with current-user DPAPI and written atomically outside the workspace. After a successful Save or confirmed Discard, and before destruction, a durable retirement marker makes the matching checkpoint non-restorable before physical cleanup runs in the background. An unreadable, unsupported, or path/key-misbound marker fails recovery closed, while editing and source-file Save remain available.
- The requested file opens before recovery starts. Store opening, decryption, source verification, and recovered-document parsing run on the background executor; startup results cannot replace a tab edited or reloaded while that work was in flight. Watcher events mark external changes during this scan, but automatic reload is blocked until startup recovery finishes.
- A matching clean tab restores in place, including as conflicted when its disk source changed. A clean file opened while the startup scan is running can still receive its checkpoint. A successful Save or confirmed Discard queues its key until that exact intent has a durable marker; the queued origin is scoped to its document when known and is fail-closed for every destructive action when unknown. The startup scan filters queued keys before restoration. If an older single-key or batch owner already exists, the newer intent waits behind it and is replayed after that exact ticket completes. An edit cancels only the queued intent, and stale completion callbacks cannot replace or clear a newer owner. A destructive action waits through the continuation until every relevant queued intent has a durable marker. If the store remains unavailable, the document stays open with a visible status that the checkpoint could not be cleared.
- The recovery scheduler tracks the oldest edit not covered by a durable checkpoint. Early edits retain their original two-second dispatch and ten-second durable deadlines while startup recovery is unavailable. The store-return timestamp, captured on the background executor before the UI completion callback, decides whether an attempt met its durable deadline. A restored dirty record is already durable, then refreshes at ten seconds from its restored durable baseline.
- Each in-flight document attempt has its own cancellation flag. A newer edit supersedes only that stale attempt, and sibling documents in the same batch continue. One workspace owns one physical checkpoint batch at a time. When that worker is occupied past a logical deadline, recovery shows the warning and retains the latest due schedule; after the worker returns, one follow-up batch coalesces repeated edits to the latest snapshot. Only a successful current-revision checkpoint clears the warning.
- Recovery preparation and retention decoding run in bounded waves of at most four workers per stage and at most eight combined operations. Same-key replacement accounts for the existing record without decrypting ciphertext that the successful atomic write replaces; a real failure falls back to full validation and maintenance. Eviction candidates are reserved under the capability lock before recovery-root I/O; a document that becomes active during a reservation warns immediately and is protected by a later checkpoint.
- The native acceptance harness does not use clipboard APIs. It reads the exact editor source through UIA `ValuePattern.CurrentValue`, so clipboard state cannot influence acceptance readback.

## Recovery contract

On Windows, recovery records live under `%LOCALAPPDATA%\markturbo\recovery`. When an absolute `MARKTURBO_DATA_DIR` is set, it redirects the application-data root and makes `%MARKTURBO_DATA_DIR%\recovery` the candidate recovery location. Production recovery accepts only local fixed, removable, or RAM volumes. It rejects UNC paths, mapped/remote drives, and other unsupported drive types before creating storage; it validates the nearest existing ancestor before creation and the canonical volume after creation. WebView and log paths may still follow the absolute data override. Recovery failure remains visible but does not prevent editing or source-file Save.

Recovery is local-only and optional. It does not modify workspace files, is not required to open ordinary files, and does not create a proprietary document format. Records retain source path, encoding, BOM, line endings, source identity, decode state, and the original conflict stamp. A changed, missing, or unreadable source restores as conflicted rather than authorizing overwrite.

When the recovery store is available, a successful Save or confirmed Discard uses a two-phase retirement protocol before its document or window is destroyed. A versioned single-key marker, or one atomic batch marker for a multi-document Discard, is the non-restorable linearization point. Marker publication failure keeps the document or window open; after a post-persist sync failure, retry resyncs only an exact existing marker and rejects a misbound one. Cleanup failure after publication does not restore the record and is retried in the background. `pending_recovery_retirements` records a newer intent with its originating document when known; unknown origin blocks every destructive action. The single-key and batch maps hold the exact tickets that own durable cleanup. Owner plus queued intent means a later Save or Discard is waiting for a fresh marker after the current ticket completes. Editing removes only the queued intent, and exact ticket matching makes stale callbacks no-ops. Decisions made before startup finishes are also queued and filtered from the scan. The destructive continuation rechecks all relevant keys and resumes the action only after every remaining queued intent has a durable marker. If startup storage is unavailable, that action remains open.

Retention remains capped at 50 records, 32 MiB per record, and 128 MiB total, with seven-day expiry. Startup and completed checkpoint work prune expired or invalid records; unreadable records are reported and retained rather than silently discarded. Active records are not eviction candidates. A checkpoint transaction reserves its eviction candidates under the capability lock, releases that lock before I/O, and makes an immediately activated victim visibly unprotected until a later checkpoint succeeds. Quota failures remain visible without changing the editor or source files.

## Observed evidence

| Command or batch | Result | What it proves |
|---|---:|---|
| `cargo fmt --all -- --check` | PASS | Formatting gate passed. |
| `cargo clippy --workspace --all-targets -- -D warnings` | PASS | Workspace clippy gate passed. |
| `git diff --check` | PASS | The current change set has no whitespace errors. |
| Actual final command: `cargo test -p mt-app --lib --locked production_root -- --test-threads=1` | 10 passed, 0 failed, 0 ignored | The production recovery-root policy, including local-volume gating, passed. |
| Reconstructed seven-command batch: `cargo test -p mt-app --lib --locked recovery::tests::<test-name> -- --exact --test-threads=1` | 7 / 7 passed | `production_root_rejects_unc_paths_without_network_access`, `production_root_parses_disk_and_verbatim_disk_volume_roots`, `production_root_parses_case_insensitive_volume_guid_roots`, `production_root_rejects_non_volume_guid_and_relative_prefixes`, `production_root_finds_existing_ancestor_without_creating_missing_tail`, `production_root_ancestor_propagates_non_missing_errors`, and `production_root_allows_only_local_drive_types` each passed. |
| Case 10 exact release command: `C:/Users/lhan/.cargo/bin/cargo.exe test --release --locked -p mt-app --lib fs::tests::saving_through_a_symbolic_link_preserves_the_link -- --exact --nocapture --test-threads=1` | 1 passed, 0 failed, 0 ignored; exit 0 | Saving through a symbolic link preserved the link. Evidence: `.scratch/goal-02-case-10-symlink-evidence.txt`. |
| Case 10 exact release command: `C:/Users/lhan/.cargo/bin/cargo.exe test --release --locked -p mt-app --lib views::workspace::tests::save_action_refuses_retargeted_symlink_without_overwriting_either_target -- --exact --nocapture --test-threads=1` | 1 passed, 0 failed, 0 ignored; exit 0 | A retargeted symbolic link was refused without overwriting either target. Evidence: `.scratch/goal-02-case-10-symlink-evidence.txt`. The final audit's Case 10 skip scan found 0 unexplained skips. |
| Actual final command: `cargo test -p mt-app --lib --locked recovery::tests -- --test-threads=1` | 134 passed, 0 failed, 1 ignored | Current recovery suite passed. |
| Actual final command: `cargo test -p mt-app --lib --locked -- --test-threads=1` | 556 passed, 0 failed, 1 ignored | Current debug `mt-app` library suite passed. |
| Normalized command: `uv run scripts/test_goal_02_native_acceptance.py` | 64 passed, 0 failed | The actual batch resolved `uv.exe` from a temporary tool environment. The native destructive-acceptance harness schema and fail-closed behavior passed unit coverage without launching a UI. |
| Normalized command: `uv run scripts/test_recovery_capacity.py` | 9 passed | The actual batch resolved `uv.exe` from a temporary tool environment. The capacity-output parser passed its current unit coverage without invoking Cargo or DPAPI. |
| Normalized command: `uv run python -m py_compile scripts/goal-02-native-acceptance.py scripts/test_goal_02_native_acceptance.py` | PASS | The actual batch resolved `uv.exe` from a temporary tool environment. The current native harness and its unit test file compile successfully. |
| `cargo test --release --workspace --locked -- --format terse` | 780 passed, 0 failed, 8 ignored | Current release workspace gate passed. |
| Actual command: `C:\Users\lhan\.cache\codex-runtimes\codex-primary-runtime\dependencies\python\python.exe scripts/recovery-capacity.py --invocations 3` | PASS: 9 rounds; 125,036,300 bytes each; median 5.720007000s; max 5.770878600s < 8s | The current near-capacity DPAPI checkpoint batch met the post-dispatch durable-store budget. Raw durations: `5.759912500`, `5.727670300`, `5.690301200`, `5.736668300`, `5.681700800`, `5.714535800`, `5.711840600`, `5.720007000`, and `5.770878600` seconds. |
| Normalized command sequence: `cargo build --release --locked -p mt-app --bin markturbo`; measure byte length, SHA-256, and PE machine | AMD64; 48,202,240 bytes; `9a4b45835a8bc113cd2ac7815adb2863fe1dc1bee0ec25955764b1e5ae38f24d` | The current release executable identity bound to the native evidence below. |
| Normalized command: `uv run scripts/goal-02-native-acceptance.py --exe target/release/markturbo.exe --expect-exe-sha256 9a4b45835a8bc113cd2ac7815adb2863fe1dc1bee0ec25955764b1e5ae38f24d --evidence .scratch/goal-02-native-acceptance-v1.json` | PASS `ALL_REQUIRED_CASES`; 5 passed, 0 failed, 0 blocked, 0 not run; total 25540.081ms | The actual invocation used the readiness environment's Python executable directly. The current JSON records document fingerprints, timings, environment results, executable hashes, and content-free scans. Its timing and fingerprint fields record case durations of `6337.424`, `2954.705`, `2702.602`, `3514.733`, and `8034.147` ms; recovery checkpoint `2405.823ms`; exact restore `481.768ms`; retirement `448.093ms`; one canonical record plus one lease; and two restarts. Its environment records WTS active; case observations record foreground verification; and executable fields record verified AMD64 original and copied hashes. The JSON does not serialize `UOI_IO` or UIA availability. Harness source and the 64 unit tests separately establish clipboard-free UIA `ValuePattern.CurrentValue` exact readback. |
| Report-only `ce-code-review` run `20260831-140132-86b635b0` | 8 reviewers; 1 validated P2 documentation wording finding; 0 remaining validated code, security, correctness, or adversarial findings | The outer goal workflow corrected the P2. Local evidence: `C:\Users\lhan\AppData\Local\Temp\compound-engineering-codex\ce-code-review\20260831-140132-86b635b0\report.md`; it is not a repository requirement. |
| Requirement-by-requirement completion audit | COMPLETE: 16 of 16 required cases proven | The final audit found current implementation and evidence for every required case; Case 10 was closed by the two exact release tests recorded above. |

## Current acceptance basis

- **Current debug evidence:** the `mt-app` debug library suite passed with `556 passed, 0 failed, 1 ignored`; the recovery suite passed with `134 passed, 0 failed, 1 ignored`.
- **Current recovery-root evidence:** the `production_root` filter passed with `10 passed, 0 failed, 0 ignored`, and the reconstructed seven-command exact batch passed.
- **Formatting, lint, and diff:** `cargo fmt --all -- --check`, `cargo clippy --workspace --all-targets -- -D warnings`, and `git diff --check` passed.
- **Release, capacity, and executable evidence:** current results, including the build, byte-length, SHA-256, and matching native-harness command, are recorded in the adjacent command rows above.
- **Native harness unit and parser evidence:** the harness passed with 64 tests, the capacity parser passed with 9 tests, and the current native Python files passed `py_compile`.
- **Native destructive acceptance:** the current hash-bound JSON reports `PASS` for every required case. Harness source and its 64 unit tests establish that clipboard APIs are removed and UIA `ValuePattern.CurrentValue` provides exact readback.
- **Final report-only review:** run `20260831-140132-86b635b0` completed with one validated P2 documentation wording finding, corrected by the outer goal workflow, and no remaining validated code, security, correctness, or adversarial findings.
- **Case 10 symlink evidence:** both exact release tests passed individually with exit code 0; the final skip scan found no unexplained skip, and `.scratch/goal-02-case-10-symlink-evidence.txt` records both commands and results.
- **Completion audit:** complete; all 16 required cases are proven against the settled implementation and current evidence.

## Goal 02 case coverage

The final 16-case audit, focused debug suite, recovery-root checks, Case 10
exact release tests, current release suite, near-capacity recovery-store run,
and current hash-bound native JSON cover the Goal 02 lifecycle, stale-result,
filesystem-conflict, recovery, retirement, scheduler, source-scanning, and
required destructive workflows. `probe.py windows` remains separate from
destructive workflow acceptance.

## Simplification result

The latest harness simplification removed unused `FakeInfo` and an unused
startup-timing return. Broader polling changes were skipped to preserve the
established safety and acceptance behavior.

## Residual risks and limits

- Recovery snapshot materialization still copies complete editor text on the UI thread; the broader immutable editor-snapshot redesign was not mixed into this safety goal.
- A recovery record explicitly retired while the startup scan is already parsing may still pay that background parse cost before the UI-side filter removes it; it is not restored.
- Non-record recovery artifacts can retain physical disk space after a persistent cleanup failure even though they remain hidden from recovery results.
- A second markturbo instance loses recovery availability because the production store uses an exclusive lease; editing remains available and the condition is reported.
- Global per-user recovery restore is not filtered by workspace root, and open-tab path deduplication remains lexical.
