# Revision evaluation ablation results

All paths below are repository-relative. Experiments use Python 3.12.10 on Windows and temporary copies of `evaluation/goal-01`; the checked-in corpus was not modified.

## Executed baseline

- Command: `python -m unittest -v scripts.tests.test_evaluation scripts.tests.test_revision_evaluation`
- Result: exit 0; 61 tests passed in 54.791s.
- Baseline inventory: 23 evaluation tests plus 38 revision-evaluation tests. The latter still includes `test_cli_verify_manifest_returns_zero` and `test_native_case_contract_matches_the_goal07_harness` at this point.

## A1 original-route versus canonical-route fixture matrix

- Command: `python .scratch/test-ablation/revision-route-experiment.py`
- Artifact: `.scratch/test-ablation/revision-route-baseline.json`.
- Result: the command intentionally exited 1 because its initial strict output-hash check exposed route-specific launcher text and error formatting. This is a measured result, not a product failure.
- Decisions matched in all four isolated fixtures: untouched `0/0`; changed `CORPUS.md` `1/1`; changed `CORPUS.md` plus recomputed manifest entry `1/1`; unmanifested extra file `1/1` (canonical/revision alias).
- Raw stdout/stderr hashes and byte counts are in the JSON artifact. Raw hashes differ because `scripts/mt.py` prints the selected module name. More importantly, malformed fixtures expose a defect in the removable alias: `revision_evaluation.main` does not translate `evaluation.EvaluationError`, so its stderr contains a traceback while the canonical route emits the concise evaluation error. Removal in favor of the canonical route therefore preserves the retained command's error/exit contract rather than carrying this alias-only defect forward.
- A separate direct untouched run confirmed that, after removing the one launcher line, both routes print the same manifest JSON; artifacts: `revision-canonical.stdout`, `revision-canonical.stderr`, `revision-alias.stdout`, and `revision-alias.stderr`.

## A1 exact-file-set survivor added

- Added `RevisionEvaluationV2Tests.test_scaffold_rechecks_manifest_exact_file_set` because the read-only evaluation tests had approved-anchor and per-file hash cases but no named unmanifested-extra-file test.
- Command: `python -m unittest -v scripts.tests.test_revision_evaluation.RevisionEvaluationV2Tests.test_scaffold_rechecks_manifest_exact_file_set`
- Result: exit 0; 1 test passed in 8.310s. It verifies a clean temporary corpus copy, adds an unmanifested regular file, and proves revision scaffold freshly rejects it through the canonical verifier.

## A1/A2 survivor mutation experiment

- Command: `python .scratch/test-ablation/revision-mutation-experiment.py`
- Result: exit 0. Full output and per-test failure traces: `.scratch/test-ablation/revision-mutation-results.json`.
- A1 mutation matrix (all killed): removing the approved-manifest comparison failed `test_recomputed_tampered_manifest_is_rejected_by_approved_trust_anchor`; removing per-file hash comparison failed `test_manifest_hash_mismatch_is_rejected`; removing exact-file-set comparison failed `test_scaffold_rechecks_manifest_exact_file_set`; bypassing revision scaffold's fresh verifier call failed that same scaffold test. Each injected mutant ran one named survivor and produced one assertion failure because the expected rejection disappeared.
- A2 baseline receipt matrix: `test_native_acceptance_requires_the_exact_goal07_case_set` passed while rejecting missing, duplicate, and invented case lists; `test_native_schema_version_and_unhashable_case_id_fail_closed` passed while rejecting wrong schema version, an altered required observation, and an unhashable ID.
- A2 changed-set control: reducing `revision.NATIVE_REQUIRED_CASE_IDS` by `trusted_executable_apply_revokes_trust` was killed during fixture loading by the retained full native validator (`top-level transport request count is not synchronized`). This proves receipt behavior, not constant equality, binds the consumer to the native case contract.
- A2 faulty-validator control: replacing `native_goal07.validate_evidence` with a no-op failed `test_native_acceptance_runs_full_validator_without_legacy_request_key`; the altered provider request count was no longer rejected. The full validator call remains necessary and retained.
- Faults were injected in cloned modules or with `unittest.mock`; production source and immutable corpus bytes were not mutated.

## A1 ablated route matrix

- Command: `python .scratch/test-ablation/revision-route-ablated.py`
- Result: exit 0. Artifact: `.scratch/test-ablation/revision-route-ablated.json`.
- Canonical `evaluation verify-manifest` and surviving `revision-evaluation scaffold` decisions matched on every isolated fixture: untouched `0/0`; changed snapshot `1/1`; changed snapshot plus recomputed manifest entry `1/1`; unmanifested extra file `1/1`.
- The removed `revision-evaluation verify-manifest` command now produces argparse exit 2. Raw stdout/stderr hashes and byte counts for every command are recorded in the JSON artifact.
- Diagnostics were attempted on both changed Python files, but the configured `basedpyright-langserver` is not installed. Dependencies are prohibited for this task, so syntax/behavior verification proceeds with the focused runtime suites rather than installing it.

## Post-change focused gate

- Command: `python -m unittest -v scripts.tests.test_evaluation scripts.tests.test_revision_evaluation`
- Result: exit 0; 60 tests passed in 51.713s.
- Count reconciliation: baseline 61, removed 2 obsolete tests, added 1 behavioral scaffold freshness/exact-file-set test, final 60.

## Decision

- **A1 remove.** `revision-evaluation verify-manifest` was a duplicate forwarding route. `evaluation verify-manifest` remains canonical; revision scaffold/record continue to verify freshly. The removed alias now exits 2 through argparse, while all retained malformed-corpus decisions remain fail-closed.
- **A2 remove.** `test_native_case_contract_matches_the_goal07_harness` only compared constants. Missing, duplicate, invented, malformed-observation, reduced accepted-set, and bypassed-validator controls are all detected by surviving receipt behavior.

## Residual limits

- Raw CLI output hashes differ across command names because `scripts/mt.py` prints the selected module invocation; comparisons therefore use exit decisions and retain all raw hashes/byte counts for audit.
- Malformed revision scaffold commands retain their pre-existing traceback formatting for the initial canonical `EvaluationError`; this ablation did not broaden scope into error translation. Exit remains 1.
- LSP diagnostics were unavailable because `basedpyright-langserver` is not installed, and dependency installation was prohibited. Both changed Python files were imported and exercised by the passing focused suite.
- Parent session owns documentation updates and the final combined gate.
