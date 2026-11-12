# Native Goal03 test ablation results

Date: 2026-09-29. Scope: test-only change to
`scripts/tests/test_native_goal03.py`; experiment artifacts are the
`.scratch/test-ablation/native-*` files. No native production source, other
test, documentation, configuration, snapshot, profile, or Git state was
changed by this experiment. No build or native GUI run was performed.

## Decision

Remove the two Goal03 forwarding tests
`test_single_case_run_never_claims_acceptance_pass` and
`test_run_lifecycle_cleans_success_and_keeps_failure_or_partial_workdirs`.
Replace them with
`test_native_run_plan_wires_each_required_case_to_its_scenario_in_order`.

The removed assertions were shared `runtime.run_native_acceptance` lifecycle
assertions. Goal02 retains direct detection of partial-run FAIL, successful
cleanup, failed/partial retention, and failure reason propagation. Goal07 also
retains selected-case status and content-free transport-count detection. The
replacement directly checks Goal03's unique adapter contract: the exact seven
`REQUIRED_CASE_IDS`, exact scenario methods in matching order, and binding of
every callable to the supplied Goal03 harness instance.

## Baseline and ablated runs

Baseline command, run before editing:

```text
PYTHONHASHSEED=0 uv run --project scripts python -m unittest scripts.tests.test_native_goal02_evidence scripts.tests.test_native_goal02_uia scripts.tests.test_native_goal02_runtime scripts.tests.test_native_goal02_execution scripts.tests.test_native_goal03 scripts.tests.test_native_goal06 scripts.tests.test_native_goal07 scripts.tests.test_native_goal07_provider scripts.tests.test_native_source_contract
```

Result: exit 0; 216 tests; 0 failures; 0 errors; no skips or expected failures
reported; 9.814 seconds.

The identical command after ablation produced exit 0; 215 tests; 0 failures;
0 errors; no skips or expected failures reported; 13.449 seconds. Output is in
`native-ablated-tests.txt`. This is the required count delta of -1 (two tests
removed and one added).

Focused Goal03 command:

```text
PYTHONHASHSEED=0 uv run --project scripts python -m unittest scripts.tests.test_native_goal03
```

Result: exit 0; 31 tests; 0 failures; 0 errors; 2.659 seconds. Output is in
`native-goal03-focused.txt`. `python -m py_compile
scripts/tests/test_native_goal03.py
.scratch/test-ablation/native-mutation-runner.py` and `git diff --check --
scripts/tests/test_native_goal03.py
.scratch/test-ablation/native-mutation-runner.py` both exited 0. Python LSP
diagnostics were unavailable because `basedpyright-langserver` is not
installed; no dependency was installed under the task restrictions.

## In-memory mutation comparison

`native-mutation-runner.py` loaded either the HEAD baseline Goal03 test file
saved as `native-baseline-goal03.py` or the ablated module, installed each
mutation in imported Python objects, and ran only named detectors. Every
baseline/ablated invocation was a fresh interpreter:

```text
PYTHONHASHSEED=0 python .scratch/test-ablation/native-mutation-runner.py baseline|ablated MUTATION
```

Raw JSONL is `native-mutation-results.jsonl`; assertion output is
`native-mutation-stderr.txt`.

| Injected fault | Baseline | Ablated | Surviving detector / decision |
| --- | --- | --- | --- |
| Selected case is returned as success instead of `PARTIAL_CASE_RUN` | killed: 3/3 tests failed | killed: 2/2 tests failed | Goal02 single-case and Goal07 selected-case tests both survive; removed Goal03 assertion was redundant. |
| Successful-root cleanup becomes a no-op | killed: 2/2 tests failed | killed: 1/1 test failed | Goal02 lifecycle test detects the retained success root. |
| A requested failed/partial debug root is deleted | killed: 2/2 tests failed | killed: 1/1 test failed | Goal02 lifecycle test detects retention loss. |
| Goal03 scenarios 0 and 1 are swapped | survived: 0/2 failed | killed: 1/1 failed | Old forwarding tests could not distinguish order because their fake methods were aliases; the replacement detects the unique adapter fault. |
| Goal03 final scenario is replaced by scenario 0 | survived: 0/2 failed | killed: 1/1 failed | The replacement detects duplicate/wrong wiring that both old tests missed. |

There are no mutation survivors in the ablated detector set. OwnedProcess
guarantees remain covered by the unchanged Goal02 runtime and Goal07 tests in
the 215-test portable run. The experiment did not replace, mock out, or weaken
their subscribe/assign/resume ordering, non-breakaway kill-on-close Job Object,
completion-port `ACTIVE_PROCESS_ZERO`, exit-code preservation, handle closure,
quiescence retention, descendant-before-scan, privacy scan, or content-free
evidence assertions.

## Four CLI adapter checks

Each adapter was invoked in a separate child process with the same nonexistent
absolute executable, a 64-zero expected hash, and a fresh evidence path. The
command shape was:

```text
python scripts/mt.py accept GOAL -- --exe Q:\repos\markturbo\.scratch\test-ablation\nonexistent-native.exe --expect-exe-sha256 0000000000000000000000000000000000000000000000000000000000000000 --evidence ABSOLUTE_NATIVE_EVIDENCE_PATH
```

Actual command arrays, stdout/stderr, receipts, case IDs, statuses, reasons,
summaries, and exit codes are in `native-cli-results.json` and
`native-cli-{goal-02,goal-03,goal-06,goal-07}.{json,stdout.txt,stderr.txt}`.

| Adapter | Exit / outcome | Exact cases | Receipt result |
| --- | --- | ---: | --- |
| `goal-02` | 1 / `FAIL: EXECUTABLE_MISSING` | 5 | all `NOT_RUN`, all reason `EXECUTABLE_MISSING`, observations empty |
| `goal-03` | 1 / `FAIL: EXECUTABLE_MISSING` | 7 | all `NOT_RUN`, all reason `EXECUTABLE_MISSING`, observations empty |
| `goal-06` | 1 / `FAIL: EXECUTABLE_MISSING` | 2 | all `NOT_RUN`, all reason `EXECUTABLE_MISSING`, observations empty |
| `goal-07` | 1 / `FAIL: EXECUTABLE_MISSING` | 6 | all `NOT_RUN`, all reason `EXECUTABLE_MISSING`, observations empty, request count 0 |

The four commands were repeated to fresh `native-cli-reference-goal-*.json`
paths. `native-cli-comparisons.json` records that after removing only
`started_at_utc` and `completed_at_utc`, every per-goal receipt compared equal;
all four exit-code, exact-case, exact-reason, and empty-observation comparisons
are true, and Goal07's transport request count is exactly 0. The test-only
baseline/ablated states use the same production CLI and native modules; test
modules are not imported by these adapters.

## Boundary

This proves the bounded portable test ablation and CLI rejection behavior. It
does not claim Windows kernel behavior, a foreground desktop, native GUI
acceptance, or a release executable. Those remain outside this test-only run.
