---
title: Goal 02 Final Validation - Plan
type: test
date: 2026-08-30
---

# Goal 02 Final Validation

## Objective

Finish `goals/02-guarantee-user-text-safety.md` on the existing branch by stabilizing the oversized-checkpoint change, proving the current implementation with focused, release, capacity, and real Windows destructive-path evidence, then completing simplification, independent review, and a case-by-case audit without entering Goal 03.

## Current State and Hard Rules

- The implementation surface is `crates/mt-app/src/{app_paths,fs,lifecycle,recovery}.rs` plus `crates/mt-app/src/views/{document,tabs,workspace}.rs`; tests are embedded in those modules and supplemented by the two Python harness test files under `scripts/`.
- `crates/mt-app/src/views/document.rs`, `crates/mt-app/src/views/workspace.rs`, and `crates/mt-app/src/recovery.rs` are still being edited by the oversized-checkpoint worker. Do not review or validate them as final until that worker returns.
- Every result currently recorded in `docs/goal-02-completion.md` is stale because the working tree has changed since those commands ran. Existing pass counts, capacity timings, executable hash, and probe result must not be carried forward as current evidence.
- The old clean-document `scripts/probe.py windows` run proves only its single-window/WebView/chrome smoke contract. It satisfies none of the five destructive native workflows below and must never be cited as their substitute.
- Cargo builds, release tests, recovery-capacity runs, and native acceptance are serialized. Do not overlap release builds or capacity processes on this noisy machine.
- Any production-code or dependency edit after the native executable is hashed invalidates the hash and native evidence. Any harness edit invalidates that harness's evidence. Rerun from the applicable gate before claiming completion.
- Record only measured numbers and exact command results. A `BLOCKED`, `FAIL`, `NOT_RUN`, unexplained skip, missing evidence artifact, or review finding that can lose user text leaves Goal 02 incomplete.

## Execution Order

### 1. Accept the Worker Handoff and Freeze the Candidate

Wait for the oversized-checkpoint worker's completion notification, then read its summary, changed files, and red/green test evidence. Snapshot `git status --porcelain=v2`, `git diff --stat`, and the complete diff against `origin/main`; preserve all unrelated user or generated changes.

Review the worker's final changes together with the surrounding recovery scheduler and lifecycle code. The oversized checkpoint must leave exact editor text and source bytes untouched, produce a visible non-blocking recovery warning, avoid publishing a partial/over-limit record, avoid a retry or allocation loop, preserve privacy in logs/status/evidence, and allow a later eligible edit/checkpoint to recover normally. Require a regression test that demonstrably failed without the fix and passes with it.

Exit only when the three in-flight files are stable, the worker's change is understood, and there is no unresolved correctness or scope concern.

### 2. Run Focused Tests Before Broad Gates

Run the worker's exact oversized-checkpoint regression tests first, followed by the directly affected `recovery`, `document`, and `workspace` test filters. Also run the lifecycle/save-conflict filters if the worker touched their boundary. Run `uv run scripts/test_goal_02_native_acceptance.py`, `uv run scripts/test_recovery_capacity.py`, and `git diff --check` so harness schema/parser failures and patch defects are found before expensive gates.

Fix focused failures at the owning boundary. A Rust fix returns to the affected Rust tests; a native-harness fix returns to its Python tests. Do not proceed while the focused set is red.

### 3. Make the Two Native Harness Files Reviewable

Run the idempotent intent-to-add command even if the current porcelain snapshot already reports `.A`:

```powershell
git add -N -- scripts/goal-02-native-acceptance.py scripts/test_goal_02_native_acceptance.py
```

Verify both files appear in `git diff`, then review the complete harness and tests as part of the branch diff. Confirm that the harness is fail-closed, emits no document contents, binds evidence to the expected SHA-256 and copied executable, distinguishes blocked prerequisites from product failures, and validates all required observations before emitting `PASS`.

### 4. Establish Native Dependencies and Preflight the Exact Release Binary

Use an active, unlocked Windows 11 x64 console session with the default input desktop. Confirm `uv` can run Python 3.11+ and resolve the pinned `pywinauto==0.6.9`; the harness itself must accept the Windows build, AMD64 PE32+ executable, 64-bit Python, WTS-active session, process session/integrity context, and foreground control.

With no other Cargo or capacity job running, build and hash the current candidate:

```powershell
cargo build --release --locked -p mt-app --bin markturbo
$exeHash = (Get-FileHash -Algorithm SHA256 'target/release/markturbo.exe').Hash.ToLowerInvariant()
```

Treat the hash as valid only for the unchanged production/dependency tree that produced it. A preflight failure is a blocker to resolve, not permission to reuse earlier native evidence.

### 5. Run the Real Destructive Native Acceptance

Run one complete harness invocation against the hashed release executable and a freshly generated evidence file:

```powershell
uv run scripts/goal-02-native-acceptance.py --exe target/release/markturbo.exe --expect-exe-sha256 $exeHash --evidence .scratch/goal-02-native-acceptance-v1.json
```

Require `PASS: ALL_REQUIRED_CASES`, overall JSON status `PASS`, the current executable and copy hash verified, and all five cases passing in that same run:

1. Pointer tab-close, Cancel, then `SendInput Ctrl+S`, preserving and saving exact text.
2. `SendInput Ctrl+W`, then Discard, closing without writing.
3. `WM_CLOSE`, then Save, exiting only after exact bytes reach disk.
4. Same-length/same-mtime external rewrite, watcher conflict, then explicit Overwrite.
5. CJK/emoji edit, checkpoint success log, `TerminateProcess`, restart and exact restore, Discard, then a second restart after the startup-finished log proving retirement.

If any case blocks or fails, do not substitute Rust tests, manual reasoning, or the old clean probe. Resolve the cause and rerun the complete five-case harness against a newly built and hashed executable when code changed.

### 6. Run the Serialized Final Gates

On the unchanged candidate, run these one at a time and retain the complete result needed for the report:

```powershell
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test -p mt-app --lib -- --test-threads=1
cargo test -p mt-app --lib dpapi_ -- --test-threads=1 --nocapture
cargo test -p mt-app --lib fs::tests::saving_through_a_symbolic_link_preserves_the_link -- --exact --nocapture
cargo test -p mt-app --lib views::workspace::tests::watcher_auto_reload_waits_for_startup_recovery_and_preserves_conflict -- --exact --nocapture
cargo test -p mt-app --lib views::workspace::tests::resolved_symlink_target_change_marks_dirty_document_without_replacing_text -- --exact --nocapture
uv run scripts/test_goal_02_native_acceptance.py
uv run scripts/test_recovery_capacity.py
cargo test --release --workspace --locked -- --format terse
uv run scripts/recovery-capacity.py --invocations 3
```

Record exact pass/fail/ignored counts, every skip explanation, all nine capacity rounds, aggregate median and maximum, and the eight-second decision. Before any later push, repeat the full release workspace gate on a clean tree and report its pass count as required by the repository contract.

If a gate requires a source fix or formatting edit, return to focused tests, rebuild and rehash, rerun all five native cases, then repeat every affected final gate. Never update evidence around a failing command.

### 7. Replace Stale Documentation with Fresh Evidence

Update `scripts/README.md` to list the native acceptance harness and parser tests, prerequisites, exact invocation, evidence path/schema, exit semantics, privacy properties, and the five-workflow requirement.

Rewrite the evidence and status portions of `docs/goal-02-completion.md` from the fresh outputs only. Include the current release SHA-256, the five-case JSON result and path, focused/debug/release pass counts, ignored/skipped counts, named Windows test outcomes, capacity measurements, recovery location/deletion behavior, and proof that recovery is optional and outside the workspace. Remove or explicitly label superseded numbers. The clean `probe.py windows` row may remain only as ancillary chrome smoke evidence and must state that it covers none of the five Goal 02 destructive native workflows.

### 8. Run `ce-simplify-code` Within the Goal 02 Diff

Run `ce-simplify-code` over the current Goal 02 production-code diff only. Preserve every data-loss guard, fail-closed check, privacy rule, ordering guarantee, cancellation boundary, and recovery limit; skip any simplification whose behavioral equivalence is uncertain. Record whether reuse, quality, or efficiency findings changed code.

If simplification changes Rust code, dependencies, or the native harness, invalidate the corresponding evidence and repeat Steps 2 through 7 as required. If it changes only documentation, rerun documentation checks and the final case audit. Do not let the simplification result claim success using pre-change hashes or counts.

### 9. Run Report-Only Review and an Independent Validator

Run bare/report-only `ce-code-review` against the final branch diff and `goals/02-guarantee-user-text-safety.md`; do not use `apply:local`. Then run a separate read-only validator over the final diff, the goal, `docs/goal-02-completion.md`, `scripts/README.md`, and `.scratch/goal-02-native-acceptance-v1.json`.

The validator must confirm that review findings were not silently ignored, every evidence claim matches an artifact or command result, no safety check was thinned, the release hash matches the native JSON, and the five native workflows were actually executed. Any actionable finding returns to implementation, focused validation, all invalidated broad/native gates, evidence refresh, and a new report-only review plus validator pass.

After both passes are clean, replace every pending-review statement in the completion report with the measured review/validation outcome.

### 10. Perform the 16-Case Final Audit

Build a final matrix for Required Cases 1 through 16 with one row per case: owning exact Rust/Python test names, command and result, native workflow where applicable, evidence artifact, and any justified platform skip. Aggregate suite counts alone are not sufficient.

Check the matrix by concern: clean/dirty destructive lifecycle (1-5), stale async results (6), external change and save identity (7-10), decode/encoding safety (11-12), and recovery restore/metadata/retirement/corrupt-record handling (13-16). Cross-check the matrix against the final source and report so every case is current, no case relies on the clean probe, and the native rows point to the five-case JSON bound to the current hash.

Also recheck the Goal 02 completion contract: formatting, zero-warning clippy, release workspace pass count, Windows 11 x64 acceptance, current-user DPAPI, atomic application-data writes, exact recovery location and deletion semantics, no workspace dependency, no content/key/model-payload logging, and no triggered `Stop and ask` condition.

### 11. Complete Goal 02 and Stop at Its Boundary

Mark `docs/goal-02-completion.md` complete only when all gates, all five native workflows, both reviews, and all 16 required-case rows are green and current. Take a final `git status --short --branch` and `git diff --check` snapshot, ensure only intended Goal 02 files remain, and report any validation that could not run.

Do not begin Goal 03, commit, push, or open a PR without separate authorization. The final handoff should name the exact goal file, fresh pass counts, release hash, native evidence path and 5/5 result, capacity maximum, review outcome, and any residual risk already documented in the completion report.

## Revalidation Rules

| Change after evidence | Minimum work to repeat |
|---|---|
| Rust production code, Cargo manifest, or lockfile | Focused tests; release build/hash; all five native cases; fmt/clippy/debug/release gates; affected capacity evidence; report and audit |
| Native acceptance harness or its parser tests | Python harness tests; all five native cases; report, review, and audit |
| Recovery-capacity harness or recovery performance path | Parser tests; nine-round capacity run; affected Rust gates; report and audit |
| Evidence/architecture documentation only | Diff/document validation, report-only review as applicable, and the 16-case audit; do not invent new command results |

## Definition of Done

- The oversized-checkpoint regression is reviewed, red/green proven, and covered by the settled test suite.
- Fresh focused, clippy, debug, release, capacity, and platform-specific results pass with recorded counts and no unexplained skip.
- A single current native run passes all five destructive workflows against the verified current release hash.
- `docs/goal-02-completion.md` contains no stale or overstated evidence and no longer claims the clean probe as destructive native acceptance.
- `ce-simplify-code`, report-only `ce-code-review`, the independent validator, and the 16-case matrix are complete with no unresolved data-loss issue.
- Goal 02 is declared complete without crossing into Goal 03 or performing an unauthorized commit/push/PR action.
