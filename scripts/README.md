# MarkTurbo tooling

Use one entry point for all repeatable development commands:

```sh
uv run --project scripts scripts/mt.py <command>
```

`scripts/pyproject.toml` declares the small Python environment used by the
tooling, and `scripts/uv.lock` pins its complete resolved dependency graph.

## Validation

```sh
uv run --project scripts scripts/mt.py check fast
uv run --project scripts scripts/mt.py check doc
uv run --project scripts scripts/mt.py check ci
uv run --project scripts scripts/mt.py check full
```

- `fast`: whitespace validation and every explicit non-desktop tooling test.
- `doc`: `fast`, Rust formatting and locked `mt-doc` tests in the optimized `ci`
  profile; it does not compile the desktop app.
- `ci`: `fast`, Rust formatting, locked Clippy and workspace tests in the
  optimized `ci` profile (no LTO, 16 codegen units).
- `full`: `fast`, formatting, production release-profile Clippy and workspace
  tests, a locked production build, and the binary privacy scan. It does not
  first run the `ci` profile, and never launches the desktop app. The privacy scan
  uses Cargo's reported executable, honoring custom target directories/triples;
  missing build artifact output is an error, even if an old binary exists.

Choose the tier using [development validation](../docs/development.md).
Small tasks do not require `full` or native acceptance. Missing native evidence
is tracked separately from implementation; safety failures remain blockers.

`fast` checks both unstaged and staged whitespace changes locally. In CI, a
complete `BASE_SHA`/`HEAD_SHA` pair checks that revision range instead and does
not depend on the index. The same range can be supplied explicitly:

```sh
uv run --project scripts scripts/mt.py check fast --base <base-sha> --head <head-sha>
```

The test list lives in `markturbo_tools/checks.py`. It is deliberately explicit:
native UI acceptance cannot enter a test run through discovery. The runtime
probe is Windows-only, but its geometry unit test and the other tooling tests
validate portable input, source contracts, or fixture behavior without a desktop.

A-B-B-A scheduling, arithmetic, and paired execution-path tests are local-only
and are not part of `fast`, `ci`, or `full`:

```sh
uv run --project scripts python -m unittest scripts.tests.local_probe_abba
```

## Commands

```sh
uv run --project scripts scripts/mt.py icons
uv run --project scripts scripts/mt.py fixtures
uv run --project scripts scripts/mt.py probe -- memory
uv run --project scripts scripts/mt.py capacity
uv run --project scripts scripts/mt.py evaluation verify-manifest
uv run --project scripts scripts/mt.py evaluation scaffold --evidence .scratch/goal-06/evaluation.json
uv run --project scripts scripts/mt.py evaluation record --owner-input-dir <owner-local-dir> --evidence .scratch/goal-06/evaluation.json
uv run --project scripts scripts/mt.py revision-evaluation verify-manifest
uv run --project scripts scripts/mt.py revision-evaluation scaffold --evidence .scratch/goal-07-revision.json
uv run --project scripts scripts/mt.py revision-evaluation record --owner-input-dir <owner-local-dir> --evidence .scratch/goal-07-revision.json
uv run --project scripts scripts/mt.py accept goal-02 -- --help
uv run --project scripts scripts/mt.py accept goal-03 -- --help
uv run --project scripts scripts/mt.py accept goal-06 -- --help
uv run --project scripts scripts/mt.py accept goal-07 -- --help
```

`icons` regenerates the platform icon outputs. `fixtures` deterministically
regenerates committed performance fixtures. `probe` measures a real Windows
process. `capacity` measures the ignored Windows DPAPI capacity test in fresh
Cargo processes. The `accept` commands drive real Windows UI workflows and
write fail-closed, hash-bound evidence.

`evaluation verify-manifest` verifies the immutable `goal-01-v1` corpus and
prints only paths, byte counts, and SHA-256 values. `evaluation scaffold`
creates a 12-artifact, content-free Goal 06 evidence record. `evaluation
record` consumes one metadata-only owner judgment JSON per artifact; it never
contacts a model endpoint. Without complete owner-local inputs it writes a
fail-closed `not_evaluated` scaffold and exits with status `2`. Model output,
source text, credentials, endpoint URLs, and local absolute paths are never
written to the evidence record. Each owner file is named `<artifact-id>.json`
and contains only `artifact_id`, `decoded_completely`, sorted ID arrays for
`surfaced_item_ids` and registry-linked `unsupported_claim_ids`, plus total
`unsupported_claim_count`, `false_source_anchor_count`, and
`boilerplate_question_count`, `question_count`, `materially_misleading`,
`usefulness`, and `model_reported_id`. The evidence destination must remain
outside the immutable `evaluation/goal-01/` corpus.

`revision-evaluation verify-manifest` verifies the same immutable corpus for
Goal 07. `revision-evaluation scaffold` writes a fail-closed v2 record without
inventing owner values. `revision-evaluation record` consumes an external
eligibility registry, a v2 machine receipt, a separately hash-bound native
Goal 07 `PASS` receipt, and one metadata-only owner JSON per artifact. The
machine receipt schema is `markturbo-goal-07-machine-receipt-v2`. Per case,
`editable_source_sha256` and `editable_source_byte_count` are the SHA-256 and
UTF-8 byte count of `proposal.source()`; `source_binding_sha256` is
`binding.source_sha256()`; and `review_context_sha256` is
`binding.review_context_digest()`. Machine facts also bind
`corpus_artifact_sha256`, `corpus_artifact_lens`, the Rust-verified request
artifact (`request_artifact_sha256` and byte count), and the serialized capture
request (`review_scope_sha256`). Proposed edits are grouped by `ChangeId` and
each group contains at least one hunk.
The union of every machine `intent_change_ids` list must exactly match the
registry annotation for that artifact; an evaluator receipt that cannot supply
that mapping fails closed.
Each composition decision file uses
`markturbo-goal-07-owner-composition-v2`. Its binding fields can be copied
mechanically from the matching machine receipt `cases[artifact_id]`:
`proposal_sha256`, `editable_source_sha256`, `editable_source_byte_count`,
`source_binding_sha256`, `source_revision`, `source_generation`,
`artifact_lens_sha256`, `review_context_sha256`, and `answers_sha256`. Add the
owner's `accepted` choices and the required, sorted, duplicate-free
`intent_change_ids` arrays; the producer never infers those IDs.
Single-file corpus artifacts bind the editable source to their sole manifest
file; Agent Skill artifacts bind the editable `SKILL.md` entrypoint and the
full request package digest to the fresh manifest.
Each owner JSON file must contain `approved_output_sha256`,
`approved_proposal_sha256`, `approved_decision_set_sha256`, and
`approved_decision_file_sha256`. Copy them unchanged from the matching machine
receipt case's `approved_output`: `result_sha256`, `proposal_sha256`,
`decision_set_sha256`, and `decision_file_sha256`, respectively. For a
`not_composed` approved output, all four owner fields are `null`.
Owner decisions are per `ChangeId`, question coverage is
explicit, and at least one eligible case must confirm
`clearer_due_to_answered_question`. Exit status `0` means the explicit
all-cases contract is satisfied, `1` means invalid or recorded-but-failed
evidence, and `2` means required owner inputs are missing. Evidence remains
content-free and must be written outside the immutable corpus.

The `record` command also requires the corresponding `--approved-*-sha256`
anchors for the registry, machine receipt and runner executable, plus
`--native-evidence`, `--approved-native-evidence-sha256`, and
`--approved-native-executable-sha256`. These anchors are supplied by the
owner; the command never derives or guesses them.

### End-to-end machine receipt and record

The following PowerShell flow builds the offline evaluator, creates one
machine receipt from every capture in a directory, computes the external
anchors, and records the owner judgments. The capture JSON files are private
owner-local inputs; the receipt and final evidence must remain outside
`evaluation/goal-01/`.

```powershell
$runner = "target\release\markturbo-goal07-evaluate.exe"
$captureDir = ".scratch\goal-07\captures"
$decisionDir = ".scratch\goal-07\composition-decisions"
$machineReceipt = ".scratch\goal-07\machine-receipt.json"
$registry = ".scratch\goal-07\eligibility.json"
$ownerDir = ".scratch\goal-07\owner-input"
$nativeEvidence = ".scratch\goal-07\goal-07-native-acceptance-v1.json"
$evidence = ".scratch\goal-07\revision-evaluation.json"

cargo build --release --locked -p mt-app --bin markturbo-goal07-evaluate
$runnerSha = (Get-FileHash $runner -Algorithm SHA256).Hash.ToLowerInvariant()
$captureArgs = Get-ChildItem $captureDir -Filter *.json | Sort-Object Name | ForEach-Object {
  "--input"; $_.FullName
}
$decisionArgs = Get-ChildItem $decisionDir -Filter *.json | Sort-Object Name | ForEach-Object {
  "--decisions"; $_.FullName
}
& $runner @captureArgs @decisionArgs --receipt $machineReceipt --runner-sha256 $runnerSha
if ($LASTEXITCODE -ne 0) { throw "machine receipt generation failed" }

$machineSha = (Get-FileHash $machineReceipt -Algorithm SHA256).Hash.ToLowerInvariant()
$registrySha = (uv run --project scripts python -c "import json,sys; from pathlib import Path; from scripts.markturbo_tools.revision_evaluation import canonical_registry_digest; print(canonical_registry_digest(json.loads(Path(sys.argv[1]).read_text(encoding='utf-8'))))" $registry).Trim()
$nativeEvidenceSha = (Get-FileHash $nativeEvidence -Algorithm SHA256).Hash.ToLowerInvariant()
$nativeExecutableSha = (Get-FileHash "target\release\markturbo.exe" -Algorithm SHA256).Hash.ToLowerInvariant()

uv run --project scripts scripts/mt.py revision-evaluation record `
  --owner-input-dir $ownerDir `
  --evidence $evidence `
  --eligibility-registry $registry `
  --approved-registry-sha256 $registrySha `
  --machine-receipt $machineReceipt `
  --approved-machine-receipt-sha256 $machineSha `
  --approved-runner-executable-sha256 $runnerSha `
  --native-evidence $nativeEvidence `
  --approved-native-evidence-sha256 $nativeEvidenceSha `
  --approved-native-executable-sha256 $nativeExecutableSha
```

`probe formula` measures the embedded KaTeX path by default. Pass `--font-dir`
only when intentionally measuring a complete external development override.

Goal 04 startup evidence uses app-acknowledged, content-free QPC milestones.
It requires the same active, unlocked Windows 11 x64 desktop as native
acceptance because the harness sends `F24` through `SendInput` and records the
GPUI action acknowledgement. Build each variant through the controlled command;
it requires an empty target directory and writes source, lockfile, feature,
release-profile, Cargo/Rustc versions, hashed Cargo configuration, dependency
graph, executable hash, size, and PE-section provenance. Compared manifests
must use the same recorded toolchain. The `full` and `no-model` manifests also include current
`cargo-bloat 0.12.1` crate attribution and selected dependency features. The
ablation runs the exact no-default-features unavailable-diagnostic test, proves
`genai`/`reqwest`/`rustls` leave the target, and reports Tokio's remaining
non-model features rather than falsely claiming Tokio disappears:

```sh
uv run --project scripts scripts/mt.py probe -- build-goal04 \
  --variant full --target-dir .scratch/goal-04/full \
  --evidence .scratch/goal-04/full-build.json
uv run --project scripts scripts/mt.py probe -- build-goal04 \
  --variant no-model --target-dir .scratch/goal-04/no-model \
  --evidence .scratch/goal-04/no-model-build.json
```

The `no-model` build is measurement apparatus, not a supported product
configuration: provider-backed selection, block, and document Translation all
return the explicit Goal 04 unavailable diagnostic. Before any full/no-model
result is visible, the owner-approved numeric materiality rule must be stored in
the source-bound `markturbo-goal-04-threshold-v1` artifact. Its cache rule must
require both the warm and fresh-profile runs to meet the same threshold before
extraction can be authorized. Generate a passing quiet-gate record immediately
before each comparison:

```sh
uv run --project scripts scripts/mt.py probe -- quiet \
  --wait-seconds 3600 \
  --evidence .scratch/goal-04/quiet.json
uv run --project scripts scripts/mt.py probe -- startup \
  --milestones \
  --exe .scratch/goal-04/full/x86_64-pc-windows-msvc/release/markturbo.exe \
  --compare .scratch/goal-04/no-model/x86_64-pc-windows-msvc/release/markturbo.exe \
  --label full \
  --compare-label no-model \
  --build-evidence .scratch/goal-04/full-build.json \
  --compare-build-evidence .scratch/goal-04/no-model-build.json \
  --threshold-evidence .scratch/goal-04/threshold.json \
  --cache-state warm \
  --rounds 10 \
  --quiet-evidence .scratch/goal-04/quiet.json \
  --evidence .scratch/goal-04/full-vs-no-model-warm.json
```

Every milestone sample also records idle working set, private bytes, peak
working set, page faults, and thread count after `--idle-settle`. The default
`warm` mode reuses one isolated data/config profile per variant across warmups
and measured launches. Repeat the command after a new quiet gate with
`--cache-state fresh-profile` and write
`.scratch/goal-04/full-vs-no-model-fresh-profile.json`. This creates fresh
isolated profiles but explicitly does not claim to flush the Windows file cache.
After both runs, bind the owner's final decision to both evidence files:

```sh
uv run --project scripts scripts/mt.py probe -- decide-goal04 \
  --warm-evidence .scratch/goal-04/full-vs-no-model-warm.json \
  --fresh-profile-evidence .scratch/goal-04/full-vs-no-model-fresh-profile.json \
  --decision "keep in-process" \
  --owner-approved \
  --evidence .scratch/goal-04/model-transport-decision.json
```

Build and measure the model first-use test against a deterministic loopback
endpoint, bound to the matching full application build. Every reported sample
starts a fresh process and performs cold transport initialization; declared
warmups affect only the Windows file cache, which is not flushed:

```sh
uv run --project scripts scripts/mt.py probe -- build-goal04 \
  --variant model-first-use --target-dir .scratch/goal-04/model-first-use \
  --evidence .scratch/goal-04/model-first-use-build.json
uv run --project scripts scripts/mt.py probe -- model-first-use \
  --exe .scratch/goal-04/model-first-use/goal04-artifacts/model-first-use.exe \
  --build-evidence .scratch/goal-04/model-first-use-build.json \
  --app-exe .scratch/goal-04/full/x86_64-pc-windows-msvc/release/markturbo.exe \
  --app-build-evidence .scratch/goal-04/full-build.json \
  --quiet-evidence .scratch/goal-04/quiet.json \
  --rounds 10 \
  --evidence .scratch/goal-04/model-first-use.json
```

Forward an underlying script's options after `--`. For example:

```sh
uv run --project scripts scripts/mt.py probe -- startup --rounds 10
uv run --project scripts scripts/mt.py accept goal-03 -- \
  --exe target/release/markturbo.exe \
  --expect-exe-sha256 <sha256> \
  --evidence .scratch/goal-03-native-acceptance-v1.json

uv run --project scripts scripts/mt.py accept goal-06 -- \
  --exe target/release/markturbo.exe \
  --expect-exe-sha256 <sha256> \
  --evidence .scratch/goal-06-native-acceptance-v1.json

uv run --project scripts scripts/mt.py accept goal-07 -- \
  --exe target/release/markturbo.exe \
  --expect-exe-sha256 <sha256> \
  --evidence .scratch/goal-07-native-acceptance-v1.json
```

Delegated commands run from the repository root. Therefore paths supplied to
`--exe`, `--evidence`, and `--open` are repository-relative, just as they are
when the underlying module is invoked directly.

## Native evidence

Native acceptance requires Windows 11 x64, an active unlocked interactive
desktop, `pywinauto`, and a current `target/release/markturbo.exe`. Build first:

```sh
cargo build --release --locked -p mt-app --bin markturbo
sha256sum target/release/markturbo.exe
```

Use the measured hash in the `accept` command. `PASS` is possible only when all
required cases complete against that hash. `BLOCKED`, including inaccessible
foreground/input-desktop access, is not acceptance and must be rerun in an
eligible interactive session. The harnesses preserve user data isolation and
record only content-free timings, hashes, byte counts, status observations, and
OS/session/integrity metadata needed to validate the run.

The native command exit status is part of that contract: `0` is `PASS`, `1` is
`FAIL`, and `2` is `BLOCKED`. Any unexpected child-process exit is reported as
`FAIL` by the CLI.

## Scratch data

`.scratch/` is disposable by default. Keep measured performance evidence under
`.scratch/perf-and-size/` when it is intended to be versioned.
