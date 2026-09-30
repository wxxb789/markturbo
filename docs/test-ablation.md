# Test and harness ablation

## Goal and decision rule

Remove redundant test or verification work without reducing observable fault
detection, product behavior, or acceptance boundaries. This is not a target for
fewer test functions or a smaller pass count. A green suite after deleting a
test is insufficient evidence.

For each candidate, record the original detector, surviving detector, relevant
input space, a discriminating injected fault, full-versus-ablated outcomes,
and the resulting keep/remove/refactor decision. Production mutations are
temporary and must be restored before integration. Similar assertions across
different constructors, processes, platforms, or evidence producers/consumers
are retained unless equivalence is demonstrated.

No evaluation snapshots, privacy requirements, native case sets, acceptance
thresholds, existing ignored probes, or failing tests are weakened. No new
dependency or general-purpose experiment framework is required. Experiments
reuse the existing test runners and isolated fixture directories. Source and
test edits remain on the existing branch without commits or publication.

## Verified baseline

- Branch: `refactor/deep-workflow-modules`; HEAD remains
  `57b2848587e3afc0410d70b98600a7d7596d8fec`.
- `check ci` and `check full` both exited 0 before ablation: 386 Python tests
  and 1,083 Rust tests passed per gate; 12 existing Rust tests were ignored.
- The 15 compiled Rust test executables enumerate 1,095 test IDs, including
  ignored tests. Python's explicit 18-module manifest and discovery both
  enumerate the same 386 IDs, with no missing or extra registrations.
- The complete machine-readable baseline is
  `.scratch/test-ablation/inventory.json`; recovery evidence is summarized in
  `.scratch/resumed-work-status.md`.
- Five pre-existing Clippy warnings remain in unrelated implementation/test
  logic. Passing exit codes do not mean zero warnings.
- Windows GUI acceptance was blocked at baseline by an unavailable foreground
  desktop; final Windows results are recorded below. macOS native smoke cannot
  run on this Windows machine. Neither limitation is evidence of redundancy.
- The production environment has no nonempty privacy-scan candidates. The
  environment-secret scan is therefore not runtime document-privacy proof.

## Inventory coverage

| Family | Inventory and proof boundary | Status |
| --- | --- | --- |
| Core units, integration, evaluation binaries, ignored probes | `.scratch/ablation-core-inventory.md`: 38 test-bearing/support files; library doctests are disabled. | Complete; E1 executed. |
| App units, GPUI interaction, integration and cost probes | `.scratch/ablation-app-inventory.md`: 27 files, 370 declarations before platform filtering. | Complete; E6-E7 executed. |
| Non-native Python tooling and evaluation/verification CLIs | `.scratch/ablation-tooling-inventory.md`: 9 test modules, 170 methods, all supporting commands. | Complete; E3-E5 executed. |
| Native fake-UIA, evidence validators, runtime and provider harnesses | `.scratch/ablation-native-inventory.md`: all 18 scoped files. | Complete; E2 executed. |
| CI and platform smoke | `.github/workflows/pull-request.yml`, `release.yml`, and `version-bump.yml` | Complete; E8 retains distinct platform/build/release boundaries. |
| Vendored parser tests | `vendor/ratex-parser/src/{lib,mhchem/mod,mhchem/patterns,mhchem/texify,parse_node,parser,stack_safety,tests}.rs`, `tests/ce_line91_brace.rs` | Retain upstream snapshot and local allocation-clamp contract. |
| Fixture/sample shell scripts | `fixtures/skills/skills/valid-skill/scripts/run.sh`, `sample/.claude/skills/hello-diagrams/scripts/render.sh` | Artifact content, not development test runners; retain. |

The CI smoke checks the staged arm64 Mach-O architecture/signature and waits
for its own nonce/PID-bound first painted frame. The ordinary headless
suite cannot establish that. Release tag/version/ancestry checks and
Cargo-reported artifact selection likewise protect different delivery inputs.
Windows native UIA, GPUI headless tests, and fake-UIA harness tests are distinct
boundaries, not substitutes.

## Experiments

| ID | Experiment | Observed evidence | Decision |
| --- | --- | --- | --- |
| E1 | Skill leaf fixture test | With the leaf stop removed, all 17 fixture tests still passed, while the nested-Skill detector failed. Excluding the candidate preserved that exact failure. Restoring the production SHA-256 restored 27/27; removing the candidate left 26/26 passing. | Remove one ineffective fixture test; retain the actual nested-Skill regression. |
| E2 | Goal03 shared lifecycle forwarding | 216 native portable tests became 215. Shared partial-PASS/cleanup/retention faults still fail surviving Goal02/07 tests. New Goal03 adapter test additionally catches swapped or duplicate scenario wiring that old tests missed. Four CLI failure receipts remain equivalent. | Replace two redundant forwarding tests with one stronger adapter test. |
| E3 | Revision manifest alias and constant equality | 61 evaluation tests became 60: remove two and add one fresh exact-file-set detector. Altered corpus/anchor/set and malformed native receipt faults are rejected by survivors. Removed alias exits 2; canonical verifier remains. | Remove duplicate CLI route and constant-equality test; preserve behavioral verification. |
| E4 | ABBA arithmetic | Original/refactored output is byte-identical for the tested vectors/errors. Five injected arithmetic/scheduling faults are detected. A real overflow difference is preserved through each caller's pair reducer. Probe suite: 55 passed. | Share pair/delta/percentage computation, not caller-specific policy or exception translation. |
| E5 | Fixture self-comparison | All three generated fixtures match committed bytes. Eight deterministic drift mutations pass the old self-comparison but fail the replacement. Existing module remains 3/3 green. | Replace self-comparison with shipped benchmark byte parity. |
| E6 | App policy subsets and adapter counterexample | Empty Welcome and wrong Text-default faults remain detected after removing subset assertions. Removing the combined document test instead makes a real wrapper fault go green; retaining it restores failure. All temporary source hashes were restored. | Remove two strict subsets; retain the combined adapter test with unique detection. |
| E7 | Ignored diagnostic overlap | On one synthetic 16-file directory, standalone flat `read_dir` duplicates the depth-zero phase's operation/count/timing field. Both search probes report 500 capped common matches and zero rare matches, but one re-walks per query and measures combined elapsed work. | Remove only the flat-directory probe; retain fresh-walk search attribution. No speed gain is claimed. |
| E8 | Native startup smoke readiness | Running the workflow's actual readiness function accepts the valid trace and rejects seven missing/foreign/incomplete/malformed cases. Ablating the guard accepts all seven invalid inputs. | Retain native startup readiness and identity checks; this local experiment is not a macOS launch PASS. |

Detailed commands, matrices and restoration evidence live in
`.scratch/test-ablation/{core,native,revision,abba,perf,app,ignored,smoke}-results.md`.
These results do not establish universal mutation coverage or replace OS
acceptance. All identified candidates have been decided and final combined
validation passed.

## Final validation and inventory reconciliation

`uv run --locked --project scripts scripts/mt.py check ci` exited 0 after all
ablations and refactors: **386 Python tests and 1,080 Rust tests passed**;
**11 existing Rust tests remain ignored**. Formatting passed, and Clippy
reported only the five pre-existing warnings described above. The final Rust
diagnostics on the changed test files reported no errors. Python LSP was
unavailable because `basedpyright-langserver` is not installed; imports,
compilation and executable tests supplied the available checks.

The compiled Rust inventory changed from 1,095 to 1,091 IDs, with no additions
and exactly these four removals:

- `i18n::tests::welcome_strings_are_translated_in_both_languages`.
- `views::document::tests::text_documents_default_to_the_source_layout`.
- `discovery_does_not_descend_into_a_skill` in the `fixtures` target.
- Ignored `read_dir_on_a_flat_directory` in the `open_folder_cost` target.

Python remains at 386 IDs: five old tests were replaced by five behavioral
tests, and discovery still matches the explicit 18-module registry exactly.
The removed checks were two Goal03 lifecycle forwarding tests, Revision's
manifest-alias and constant-equality tests, and fixture self-comparison. The
replacements cover Goal03 scenario wiring, fresh corpus file-set validation,
shipped fixture parity, structured ABBA pairing, and caller-specific overflow.
No native scenario, acceptance threshold, source/privacy guard, or immutable
fixture was deleted.

Additional real-surface checks:

- `evaluation verify-manifest` exited 0 for `goal-01-v1`.
- `revision-evaluation --help` exited 0 and lists only `scaffold` and `record`.
- The retained ignored folder diagnostic ran from the final CI test binary:
  one test passed and its depth-zero row still reported 16 synthetic entries.
- `cargo build --locked --release -p mt-app --bin markturbo` exited 0 after
  ablation. Final executable SHA-256:
  `f1ec266cc3dece1172402a5b35aadc64a8fc8c31223ec6294ff0c8834dc6f076`.

The earlier recovery-stage `check full` passed before test ablation, with
1,083 Rust tests and 12 ignored tests. That historical count is not presented
as the final suite count. No speed, memory or executable-size improvement is
claimed from these experiments.

## Remaining acceptance and execution boundaries

The foreground desktop became available at the closing checkpoint, so native
verification resumed against the final hash above:

- Full Goal02 passed **5/5**, with no blocked, failed or unrun cases:
  `.scratch/goal-02-final-ablation-f1ec266c.json`.
- The original trusted-HTML Goal07 regression case passed, including trust
  revocation, consent ordering and the complete privacy scan of 119 artifacts:
  `.scratch/goal-07-post-ablation-trust-f1ec266c.json`. Its top-level result is
  deliberately `PARTIAL_CASE_RUN`, not full acceptance.
- Full Goal07 passed its first two cases, then blocked at `accept_all_preview`
  with `CLIPBOARD_CONTAINS_NON_TEXT`; the remaining three cases were skipped
  after that block. The receipt has 2 passed, 4 blocked and 0 failed cases:
  `.scratch/goal-07-final-ablation-f1ec266c.json`. It is not a PASS receipt.
- After the user continued and the existing non-destructive clipboard guard
  confirmed restorable text, full Goal07 passed **6/6**, with no blocked,
  failed or unrun cases: `.scratch/goal-07-final-ablation-f1ec266c-r2.json`.
  Its 12 loopback requests match the six Review/Revision flows. Every case's
  complete artifact scan reports absence of document, answer, raw-response and
  ephemeral-credential sentinels in both UTF-8 and UTF-16LE; the trusted-HTML
  case scanned 123 artifacts. Original and copied executable hashes match the
  same final SHA-256 used by Goal02.

The harness protects the user's non-text clipboard rather than clearing it.
Earlier failed, partial and blocked receipts remain unchanged. The final full
Goal02 and Goal07 receipts are PASS receipts for the final hash, not an inference
from targeted runs or source scans. Legacy profile data was neither deleted nor
relocated, so no historical erasure claim is made. Maintainer judgment of the
architecture and any eventual PR approval remain separate from these verified
implementation and native gates.

Bounded inventory and Python experiment work used the runtime's `deep-low`
route (`ghc/gpt-5.6-sol-fast`, medium). The native process-ownership repair used
`deep-high` (`ghc/gpt-6-astra`, xhigh). The parent performed the Rust fault
experiments, smoke/diagnostic comparisons, integration, CLI checks and final
gates. No commit, push, PR, dependency installation or user-profile cleanup was
performed by this resumed session.
