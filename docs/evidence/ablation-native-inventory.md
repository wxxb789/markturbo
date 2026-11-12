# Native harness ablation inventory

Date: 2026-09-29. Scope: all eight `scripts/markturbo_tools/native/*.py`, all nine `scripts/tests/test_native*.py`, and `_native_goal02_support.py`: 18 files. This was read-only analysis; no test/build/provider/GUI run occurred. The task states that `check ci` and `check full` passed with the new OwnedProcess regressions on disk. Those checks run the explicit native unit modules, not native UI acceptance.

## Decision and invariants

Retain every production file and every test file. One method-level candidate is viable: in `test_native_goal03.py`, replace two shared-runtime forwarding tests with one direct Goal 03 run-plan wiring test, then ablate those methods only after the comparisons below. No whole-file deletion is viable.

Ablation must preserve: content-free/fail-closed errors and evidence; UTF-8 and UTF-16 sentinel/credential/answer/raw-response scans; exact original/copied EXE SHA-256 plus AMD64 PE32+; exact complete case sets and final fingerprints; 0/1/2 PASS/FAIL/BLOCKED exits and `PARTIAL_CASE_RUN`; source guards that reject comments, literals, test modules, and malformed Rust; explicit provider consent; and process ownership before scan/deletion. In particular, launch stays suspended until assignment to a non-breakaway kill-on-close Job Object with a pre-associated completion port. Cleanup must terminate that job, await `ACTIVE_PROCESS_ZERO`, preserve the parent's exit code, close all handles, and retain roots when quiescence is uncertain. PID-tree or parent-poll inference is not equivalent.

## Real shared dependencies

- `markturbo_tools/cli.py` owns `accept goal-02/03/06/07` delegation and exit propagation; external `test_cli.py` tests it. `checks.py` explicitly lists all nine scoped test modules; `fast/ci/full` do not run GUI acceptance.
- All goals use `runtime.py`; Goals 03/06/07 use `source_contract.py`; Goal 07 inherits Goal 03 clipboard behavior and uses `goal07_provider.py`.
- `test_native_goal07.py` imports `OwnedKernel`, `owned_harness`, and `launch_owned` from `test_native_goal02_runtime.py`: a real fixture dependency.
- `revision_evaluation.py` independently consumes Goal 07's exact six cases and binds both raw native-receipt and native-EXE hashes.
- Source guards read Rust under `crates/mt-app/src/views/`. Native cases also need Windows 11 x64, active unlocked Default desktop, same session/integrity, pywinauto/UIA, SendInput/Task Dialogs, isolated roots, and a release EXE.

## Production/harness inventory

| File | Proof purpose | Disposition |
|---|---|---|
| `native/__init__.py` | Package/relative-import and canonical `-m` boundary. | **Retain**; packaging ownership, not dead logic. |
| `native/runtime.py` | Atomic evidence; fingerprints/PE/hash/environment preflight; secret-scrubbed launch; UIA/Win32; exits/case lifecycle; new suspended `OwnedProcess`, Job Object completion, fail-closed cleanup/removal. | **Retain whole**; security/lifecycle root for every goal. |
| `native/source_contract.py` | Position-preserving, fail-closed Rust lexical views for comments, nested blocks, strings/raw/bytes, chars/lifetimes, escapes and ambiguity. | **Retain**; shared false-positive barrier, not GUI proof. |
| `native/goal02.py` | Five real destructive paths: pointer Cancel, keyboard Discard, WM_CLOSE Save, same-length/mtime conflict overwrite, and crash/checkpoint/recovery retirement; record/lease/log and panic/privacy scans. | **Retain**; no lower-layer substitute. |
| `native/goal03.py` | Seven first-use paths: welcome, New/Paste plus clipboard restore, Save As/reopen, picker/overwrite cancels, sample materialization, bounded/stale recents, file/directory CLI. | **Retain**; independent native-dialog/clipboard/sample/recents surfaces. |
| `native/goal06.py` | Document/selection Review in missing-credential and explicit configured-provider modes; fresh child key, persistent-credential block, consent/result, unchanged bytes and scans. | **Retain**; modes prove distinct privacy/consent contracts. |
| `native/goal07_provider.py` | Deterministic loopback oracle for exact path/stream/order, source hash/snapshot, answer, and request-after-click linearization; content-free records. | **Retain**; response mock cannot replace protocol observation. |
| `native/goal07.py` | Six revision paths: reject identity, selective plus one undo, inert preview/copy, stale no-op, safe conflict, trusted executable apply/restriction; exact/private evidence, source guards, transport counts, post-quiescence scan. | **Retain**; preserve new dismiss/discard/clean-exit and Job ordering. |

## Test/support inventory

| File | Proof purpose and overlap | Disposition |
|---|---|---|
| `_native_goal02_support.py` | Shared aliases, complete five-case receipt, minimal PE fixture. | **Retain**; support, not duplicate assertions. |
| `test_native_goal02_evidence.py` | Goal 02 case completeness, safe fields, hashes/PE/environment/process, flows/timing, recovery barrier/record/lease. | **Retain**; only complete Goal 02 validator detector. |
| `test_native_goal02_execution.py` | Serialization/privacy, safe exceptions, env scrub, CLI, partial run, success/failure root lifecycle, copy-failure persisted evidence. | **Retain**; surviving shared lifecycle detector. |
| `test_native_goal02_runtime.py` | Chunk scans, recovery/panic evidence, cleanup fan-out. OwnedProcess tests separately cover subscribe-assign-resume, exited-parent descendants, crash exit, job-only kill, every startup failure, UIA startup ownership, quiescence retry/interruption, explicit stdio handles and creation failures. | **Retain all**; also supplies Goal 07 fixtures. Distinct guarantees must survive prior `ci/full` success. |
| `test_native_goal02_uia.py` | Exact raw UIA ID/type/name/class, stale retry, absence-vs-error, ValuePattern fingerprints, one-shot click, foreground, owned dialogs, FAIL-vs-BLOCKED. | **Retain**; harness interpretation only, never GUI acceptance. |
| `test_native_goal03.py` | Goal schema/privacy, clipboard/native dialog input, samples, source decoys, recents and WebView/data/config scanning. | **Retain file. Candidate:** replace `test_single_case_run_never_claims_acceptance_pass` and `test_run_lifecycle_cleans_success_and_keeps_failure_or_partial_workdirs` with one direct test asserting the seven `native_run_plan().scenarios` callables match `REQUIRED_CASE_IDS` order. Then remove only those two forwarding methods. |
| `test_native_goal06.py` | Both evidence modes, byte identity, two shortcuts, consent/result, keyless config, fresh key/env scrub, Credential Manager metadata-only preflight, leak scan, Send dialog and source/aria decoys. | **Retain**; no other goal covers this fixed target or opt-in split. |
| `test_native_goal07.py` | Exact/private evidence/finalization, source order/inert/stale guards, UI keyboard/scroll/foreground, dynamic identity, consent click order, six mechanics, and post-job scan/removal. | **Retain all**; new tests separately prove descendant-before-scan, scan/provider suppression on quiescence failure, and deletion suppression. |
| `test_native_goal07_provider.py` | Actual loopback HTTP, content-free records, wrong path/stream/order/binding/answer, arrival race, SSE sentinel, UTF-8 ranges and CRLF/CJK/emoji preservation. | **Retain**; harness mocks cannot cover concurrency/protocol. |
| `test_native_source_contract.py` | Lexer positive/negative matrix and offset/newline preservation. | **Retain**; per-goal fixtures do not cover malformed lexical forms. |

## Necessary source/adapter/native distinctions

- Fake `CurrentAutomationId` tests prove selector rejection, not that GPUI exposes the ID, control is visible/enabled, or ValuePattern returns exact CJK/emoji. The hash-bound EXE and real UI case prove those facts.
- `.disabled(revision_stale)` in comment-free production source cannot prove UIA reports Apply or that clicking it leaves editor/file hashes unchanged. The Goal 07 stale native case is required.
- A fake exact Task Dialog ID/name/class cannot prove real process/window ownership, focus, or Save/Discard/Send behavior.
- CLI exit forwarding cannot prove desktop/session/integrity, Job ownership, consent timing, artifact privacy, or final bytes. Thus `ci/full` is not GUI PASS.

## Deterministic full-versus-ablated plan

Use disposable trees, `PYTHONHASHSEED=0`, and the same Python/lockfile. Run in both trees the nine native modules exactly as listed in `checks.TOOLING_TESTS`:

```powershell
uv run --project scripts python -m unittest scripts.tests.test_native_goal02_evidence scripts.tests.test_native_goal02_uia scripts.tests.test_native_goal02_runtime scripts.tests.test_native_goal02_execution scripts.tests.test_native_goal03 scripts.tests.test_native_goal06 scripts.tests.test_native_goal07 scripts.tests.test_native_goal07_provider scripts.tests.test_native_source_contract
```

Require exit 0 in both; ablated discovered count = full count - 1 (two removed, one adapter test added); no skip/expected-failure delta. Run `mt.py check ci` and `mt.py check full` once in the ablated tree. Compare SHA-256 of every `native/*.py` across trees: all must be identical.

For all four `mt.py accept goal-*` adapters, use the same nonexistent absolute EXE, 64-zero expected hash, and fresh temp evidence. Require exit 1 and `FAIL: EXECUTABLE_MISSING`; after removing only the two UTC fields, require per-goal JSON equality, exact cases/reasons, and Goal 07 request count 0.

Real GUI evidence remains mandatory after production harness/Rust UI changes: build once, hash the release EXE, run every affected full native goal on an eligible desktop, and validate the receipt. Do not duplicate GUI runs merely for test-only ablation because native source hashes are identical. Existing receipts remain valid only for their externally approved receipt and EXE hashes.

## Injected-fault comparison

Inject each fault identically into disposable full/ablated trees, run the named focused modules, and require the same detector failure. Restore between faults.

| Fault | Surviving detector(s) |
|---|---|
| Selected case returns PASS instead of `PARTIAL_CASE_RUN`. | Goal 02 execution `test_single_case...` and Goal 07 `test_partial_case_counts...`. This demonstrates equivalent coverage for removed Goal 03 forwarding. |
| Ignore retention or delete root before cleanup. | Goal 02 execution lifecycle test; Goal 03 has no separate implementation. |
| Parent exit skips job termination/active-zero wait. | Goal 02 exited-parent test plus Goal 07 descendant-before-scan/removal tests. |
| Completion timeout succeeds or root is removed after quiescence failure. | Goal 02 fail-closed retry plus Goal 07 scan/provider and deletion suppression. |
| Resume before assignment, allow breakaway, or leak startup/stdio handles. | Goal 02 startup order, limit, all-stage closure and explicit-handle tests. |
| Scan/finalize Goal 07 before `reap`. | Goal 07 descendant-before-scan and quiescence-failure tests. |
| Permit request after gate but before click, or reorder arrival. | Goal 07 consent tests and provider arrival-race test. |
| Drop chunk overlap/read failure, or serialize sentinel/key/request body. | Goal 02 runtime plus Goal 03/06/07 goal-specific privacy tests. |
| Accept copied-hash mismatch, missing case, or partial PASS. | Each goal evidence suite plus external `test_revision_evaluation` exact-case/native-anchor tests. |
| Let comment/string/test-module/malformed Rust satisfy markers. | Source lexer and Goal 03/06/07 decoy fixtures. |
| Change delegated exit 2 or pass unexpected exit through. | External `test_cli`; keep it in `TOOLING_TESTS`. |

## Coverage limits and gate

The deterministic kernel fake proves API order, handles, state, and consumer ordering, not Windows kernel behavior; only real native runs close that gap. Source guards are lexical contracts, not semantic/control-flow proof. Goal 06 configured mode assumes an already-running fixed provider; Goal 07 owns a dynamic provider, so they are not interchangeable. Historical `.scratch` logs or receipts do not bind current uncommitted native changes unless approved receipt and EXE hashes match; none was overwritten or revalidated here.

Proceed only with the two-method Goal 03 candidate plus replacement adapter test, and only if every comparison/fault result above matches and the explicit nine-module registry remains intact. Otherwise retain the suite unchanged.
