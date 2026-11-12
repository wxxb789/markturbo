# Non-native Python tooling ablation inventory

## Scope

Static, read-only inventory of tracked non-native Python in `scripts/tests`,
`scripts/markturbo_tools`, `scripts/mt.py`, and `evaluation/goal-01`. Excluded:
`test_native*`, `_native_goal02_support.py`, and `markturbo_tools/native/`, except
where a scoped consumer validates native evidence. No tests/builds were run.
There are **9 scoped test modules and 170 test methods**.

Disposition: **retain** means a distinct fault/proof boundary; **candidate** means
removal is viable only after its paired experiment; **consolidate** preserves the
contract while deleting duplicate implementation.

## Every scoped test module

| Module (count) | Owned contract | Disposition |
|---|---|---|
| `test_checks.py` (11) | Cargo lookup; worktree/index vs CI diff range; fail-fast tier ordering; unique Cargo-reported release executable; stale/ambiguous artifact rejection. | **Retain.** Artifact identity before scanning is not privacy matching. |
| `test_cli.py` (4) | `mt.py` dispatch; check error/exit 1; native 0/1/2 preservation and unexpected-exit normalization; real subprocess forwarding to Goal 07 with persisted fail-closed evidence. | **Retain.** This crosses launcher, subprocess, and child evidence boundaries. |
| `test_evaluation.py` (23) | Goal 06 immutable corpus/manifest; 12 artifacts and 60 scoring items; content-free scaffold/record; safe model IDs; fixed eligibility; fresh verification; protected output path; CLI help, persistence and exits 0/2. | **Retain; canonical corpus verifier.** |
| `test_icons.py` (5) | Platform ImageMagick selection and all-or-nothing icon publication, including generation failure, partial publication failure, and interruption restoring existing/missing files. | **Retain; distinct state transitions.** |
| `test_perf_fixtures.py` (3) | Generator shape, unique diagrams, deterministic/LF bytes. | **Candidate:** replace two-call determinism with committed-byte parity; retain shape/LF tests. |
| `test_privacy.py` (20) | Secrets in index/worktree/untracked/release binary/path/symlink/gitlink; UTF encodings; exclusions; overlap-safe redaction; no secret in argv/errors; `full` integration and test registration. | **Retain.** Each storage/leak channel is separate. |
| `test_probe.py` (53) | Geometry, quiet gate, formula env, startup traces, path redaction, Goal 04 source/host/toolchain/build/threshold/cache/evidence binding, A-B-B-A, cleanup, model-first-use and decision evidence. | **Retain tests; consolidate pair arithmetic.** |
| `test_recovery_capacity.py` (13) | Rust duration/output validation, exactly three ordered rounds/totals, recomputed median/max, global budget, Windows blocked 2/fail 1/pass 0. | **Retain.** Independently validates Rust output. |
| `test_revision_evaluation.py` (38) | Goal 07 v2 registry/machine/native anchors; instance-bound trust; content-free schemas/source bindings/change order/owner decisions/question coverage; full native receipt validation; safe JSON/paths/create-new output; CLI exits. | **Retain except exact candidates below.** |

## Every supporting module and non-native harness

| Surface | Owned machine/operator contract | Disposition |
|---|---|---|
| `scripts/mt.py` | Canonical entry point; disables bytecode before import; propagates `main` status. | **Retain.** |
| `markturbo_tools/__init__.py` | Package marker, no logic. | **Retain while package imports remain.** |
| `cli.py` | Dispatches `check`, six utility modules, and four native acceptance goals; strips one optional `--`; repository cwd; utility exit propagation; native exits constrained to 0/1/2. | **Retain command/exit contract.** |
| `checks.py` | Explicit tooling-test manifest. `fast`: diff + Python; `doc`: fast + fmt + `mt-core`; `ci`: fast + CI-profile workspace Rust; `full`: fast + release Rust + unique release build + privacy scan. | **Retain ordering, registration, artifact and scan boundaries.** |
| `privacy.py` | Candidate secrets from environment; scans Git index blobs, tracked workspace, eligible untracked paths, symlink text, path names and exact release binary in UTF-8/16; redacts values. | **Retain security boundary.** |
| `icons.py` / `mt.py icons` | Same-volume staging, format validation, atomic publish/rollback. | **Retain.** |
| `perf_fixtures.py` / `mt.py fixtures` | Deterministically regenerates three committed parser/performance fixtures with LF. | **Retain generator; strengthen parity test.** |
| `metrics.py` | Canonical nearest-rank/inclusive p95 and A-B-B-A scheduler/scalar comparison. | **Retain; absorb duplicate structured pair arithmetic.** |
| `goal04.py` | Content-free trace, quiet/build/threshold/startup/model-first-use/decision schemas and validators; source/executable/host/cache/toolchain provenance; safe metadata; controlled builds. No standalone CLI. | **Retain validation library.** |
| `probe.py` / `mt.py probe` | Windows-only (unsupported exit 2): `memory`, `startup`, `build-goal04`, `decide-goal04`, `formula`, `model-first-use`, `cpu`, `quiet`, `windows`, `shot`; real-process measurement and evidence. | **Retain CLI; consolidate arithmetic only.** |
| `recovery_capacity.py` / `mt.py capacity` | Windows-only fresh Cargo processes; parse/cross-check three DPAPI rounds each; aggregate max; pass 0, evidence/launch/budget fail 1, unsupported 2. | **Retain.** |
| `evaluation.py` / `mt.py evaluation` | Canonical `verify-manifest`, `scaffold`, `record`; approved manifest and exact corpus; content-free JSON. Record exits: complete 0, invalid 1, owner input absent/incomplete 2 with scaffold. | **Retain all.** |
| `revision_evaluation.py` / `mt.py revision-evaluation` | Goal 07 `scaffold`/`record` plus duplicate `verify-manifest`; binds corpus, registry, machine/native receipts and executable hashes; rejects unsafe JSON/paths; create-new evidence. Record: satisfied 0, invalid/unsatisfied 1, missing 2. | **Retain scaffold/record; candidate-remove manifest alias.** |

## Immutable corpus contract (retain unchanged)

`evaluation/goal-01/MANIFEST.sha256` anchors exactly 45 regular files: corpus
metadata, owner/scoring records, notices, 12 artifact roots, task prompts, goals,
agent instructions, licenses, skills, and references. `evaluation.verify_manifest`
requires the approved manifest SHA-256, exact regular-file coverage, no symlinks or
escapes, every file hash, stable directory digests, exactly 12 artifacts, and 60
unique sorted scoring IDs. Rewriting a snapshot and its manifest still fails the
independent approved digest. Both evaluator flows freshly verify before record or
write. Never ablate snapshot bytes, manifest, approved digest, exact coverage, or
freshness.

## Exact viable ablations and deterministic paired experiments

For every experiment use one checkout/interpreter and fresh temp roots. Capture
selected test IDs, exit codes, normalized stdout/stderr hashes, and JSON hashes.
Timing/repeated green runs are not evidence.

### A1 - remove revision's duplicate `verify-manifest` forwarding route

Delete that parser branch and `test_cli_verify_manifest_returns_zero`; keep
`evaluation verify-manifest` canonical. Revision scaffold/record still call the
same verifier.

**Surviving coverage:** evaluation manifest/hash/trust-anchor tests and canonical
CLI; revision stale-manifest, scaffold, record, and write tests.

**Experiment:** Baseline both routes on untouched corpus, changed snapshot,
changed snapshot plus recomputed manifest entry, and added file; decisions must
match. Ablation runs canonical verify plus revision scaffold on all fixtures;
results stay identical and removed alias gives argparse exit 2. Mutate separately
the approved-manifest comparison, per-file comparison, exact-file-set comparison,
and revision scaffold's verifier call. Surviving named tests must kill all four.

### A2 - remove tautological native-case constant equality test

Delete only `test_native_case_contract_matches_the_goal07_harness`. Production
assigns `NATIVE_REQUIRED_CASE_IDS` directly from
`native_goal07.REQUIRED_CASE_IDS`; equality merely restates the assignment.

**Surviving coverage:** exact-case-set receipt test mutates missing, duplicate and
invented cases; schema/full-validator tests mutate IDs and observations; producer
tests remain native-owned.

**Experiment:** Baseline/ablation per-test outcomes must match. Against both sets,
mutate fixture receipts by deleting, duplicating and inventing cases and changing
a required observation; consumer tests must reject each. Then mutate the accepted
production case set. If no behavior test kills that mutation, add a receipt-level
behavior mutation before deleting the equality test.

### A3 - consolidate duplicate A-B-B-A pair arithmetic

`metrics.measure_abba` and `goal04.milestone_comparison` separately pair adjacent
A/B samples, average, subtract, and calculate percentages. Add one pure
`metrics.summarize_abba_samples` and route both callers through it. Keep scheduler,
structured validator, and JSON names.

**Surviving coverage:** `AbbaMeasurementTests` owns order, separation, means,
deltas, percentages and zero baseline; startup evidence tests own structured
fields, recomputation, forged comparisons and machine JSON.

**Experiment:** Baseline/refactor serialize fixed numeric and `StartupSample`
vectors, including unequal/odd lengths, zero A, and data where raw means differ
from pair means; require byte-identical results/exceptions. Mutate schedule to
A-B-A-B, cross-pair indexes, use raw median, reverse delta, and allow zero; the
scheduler/statistics/forged-comparison tests must kill corresponding mutations.

### A4 - replace weak fixture determinism with immutable byte parity

Replace `test_document_generation_is_deterministic` (two calls in one process)
with exact generated-byte comparison for all three committed `fixtures/perf/*`
files. Keep generator/snapshots, unique-diagram and LF tests.

**Experiment:** Baseline hash generated and committed outputs; stop rather than
bless if intentionally out of sync. Ablation requires all three byte pairs equal.
Mutate numbering, Unicode, block/fence mix, diagram IDs/count, default count,
truncation, and newline mode separately. Parity must kill every content mutation;
shape catches reused IDs and LF catches CRLF.

## Different-proof-boundary counterexamples: do not ablate

1. Privacy matching does not prove `checks.full` scans the sole Cargo-reported
   binary after build or maps failure to `CheckFailure`.
2. Index, changed worktree, untracked, tracked `.scratch`, release binary, symlink
   text and gitlink paths are distinct inputs. Keep all privacy cases.
3. Full/partial path overlap, content redaction, and secret-free process arguments
   are different disclosure channels.
4. Function tests do not prove `mt.py` forwarding, persisted output, help, or
   process exits. Keep one end-to-end case per materially different result.
5. Goal 06 and Goal 07 share corpus verification but own different evidence/trust
   schemas. Share the verifier; do not merge validators.
6. Native producer tests do not replace Goal 07 consumer checks for malformed,
   wrongly anchored, wrong-case-set, or forged-instance receipts.
7. Icon generation, partial publication, and interruption prove different rollback
   boundaries.
8. Rust capacity code measures; Python checks completeness/consistency across fresh
   processes and global budget. Neither subsumes the other.
9. Probe preflight prevents invalid measurement; persisted evidence validation
   protects later consumers. Keep both.
10. `mt.py` owns canonical dispatch/exit normalization; module `main` functions own
    command schemas. Thin forwarding is intentional except A1.

## Recommended order and gaps

Apply A2, then A1, then A3. Apply A4 only after byte parity is established. Never
reduce `TOOLING_TESTS`, privacy scans, corpus snapshots/anchors, exit assertions,
content-free validators, create-new/symlink safety, or dispatch coverage. Static
inspection found no other exact duplicate with the same mutation-killing boundary.
The only evidence gap is execution: prohibited here, so all experiments remain
actionable plans rather than measured results.
