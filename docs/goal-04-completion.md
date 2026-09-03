# Goal 04 Completion and Decision Report

**Date:** 2026-09-03
**Goal:** [Goal 04 - Measure startup and decide model-transport modularity](../goals/04-measure-startup-and-decide-modularity.md)
**Status:** COMPLETE VIA INCONCLUSIVE STOP RULE; PROJECT-OWNER-APPROVED NO-GO DECISION.

Goal 04 produced reproducible build and measurement tooling, but the current
Windows host did not pass the pre-registered quiet-machine gate. No startup,
bare-shell, model-first-use, or optimization timing from this host is promoted
to decision evidence. Goal 05 therefore takes the no-go path.

## Decision

| Scope | Threshold status | Runtime evidence | Decision | Authorization |
|---|---|---|---|---|
| Model transport | No numeric threshold was approved; no threshold artifact was created | Not collected because the current-source quiet gate was blocked; no passing quiet evidence exists | Keep in-process; the performance question remains inconclusive | Project-owner approved on 2026-09-03; no model-transport extraction is authorized |

The absence of an approved numeric threshold was not bypassed: no full/no-model
A/B samples were collected or examined. The project owner approved this
inconclusive no-go disposition on 2026-09-03. Reopening the decision on another
host requires owner approval of a source-bound numeric threshold before any A/B
run.

## Source-bound build evidence

The final controlled builds used revision
`9c8ff0966ed42c47bdc348f708ceb5d482d9992f` with dirty-worktree SHA-256
`c5d7780ffd64edb69a5ddc9d866a41b62bcb447b90df5f4705631fff9d08f533`.
Each target directory was absent before its build.

| Variant | Evidence | Executable | Result |
|---|---|---|---|
| Full application | `.scratch/goal-04/final-v6/full-build.json` | 49,090,048 bytes; SHA-256 `dd194d9a6d423dcc03032066b022fc7bd2f3d3bec1288b4c9cd05ee2f65e924e` | Default `model-transport` feature; current PE sections, dependency features, and `cargo-bloat 0.12.1` attribution recorded |
| No-model upper bound | `.scratch/goal-04/final-v6/no-model-build.json` | 44,372,480 bytes; SHA-256 `6dd5f0c3d8653e3078a6b9fc2bd03e2f3777841c41b44d7e22db336f9642dee9` | `genai`, `reqwest`, `rustls`, `hyper`, and `tokio-rustls` absent; exact unavailable-diagnostic test passed 1/1 |
| Bare GPUI shell | `.scratch/goal-04/final-v6/bare-build.json` | 12,511,744 bytes; SHA-256 `62141e3dd7e2e2a9466fb7c71b4428a1fa8b54e4649e8545f0008121828c0327` | Reuses product identity, assets, DirectComposition setting, component initialization, and window options |
| Model first-use test | `.scratch/goal-04/final-v6/model-first-use-build.json` | 7,519,232 bytes; SHA-256 `b9513df0c9a8cb43e981c3238a8f75e12171d8171baa2e8bba7ddf84bb8f355d` | Matching-source loopback transport executable built; runtime measurement blocked |

Compile-time removal reduced the executable by 4,717,568 bytes, or 9.61% of
the full executable. This is a size upper bound, not evidence of launch or
idle-memory improvement.

All four manifests record the same toolchain: Cargo `1.98.0` commit
`797e8a9bca276c1c9f9f738d2a20f484fa4eea9d`, Rustc `1.98.0` commit
`88d9e12ae178fab0fb5cc050a94da85685d449ea`, LLVM `22.1.8`, host
`x86_64-pc-windows-msvc`, and no discovered Cargo configuration file.

This report was updated after these artifacts were generated, so it changes
the repository worktree digest without changing any built input. Any reopened
measurement must create fresh builds and evidence for its then-current source
state rather than rebinding these artifacts.

## Quiet-gate evidence

Command:

```sh
uv run --project scripts scripts/mt.py probe -- quiet \
  --wait-seconds 3600 \
  --evidence .scratch/goal-04/final-v6/quiet.json
```

The current-source command was blocked before sampling with
`WTS_SESSION_NOT_ACTIVE`. It wrote no PASS/FAIL JSON and produced no timing or
machine-load sample.

A prior 3,600-second gate against the pre-toolchain-provenance source failed and
is retained only as a non-current diagnostic:

| Metric | Observed | Maximum |
|---|---:|---:|
| CPU median | 46.84% | 5.00% |
| CPU p95 | 86.33% | 10.00% |
| Disk median | 2.34% | 2.00% |
| Disk p95 | 9.53% | 10.00% |

Diagnostic artifact: `.scratch/goal-04/final-v5/quiet.json`. It is not current
evidence and is not used to authorize a decision.

## Runtime disposition

| Required observation | Result |
|---|---|
| Full versus no-model, warm, at least 10 A-B-B-A rounds | BLOCKED by inactive WTS session; no samples collected |
| Full versus no-model, fresh-profile, at least 10 A-B-B-A rounds | BLOCKED by inactive WTS session; no samples collected |
| Bare GPUI startup milestones and memory | BLOCKED by inactive WTS session; no samples collected |
| Full-build model first-use baseline | BLOCKED by inactive WTS session; no samples collected |
| `opt-level = 3` versus `opt-level = "s"` | Explicitly remains BLOCKED; current session is ineligible and the prior quiet gate failed |

The harness also requires an active unlocked Windows 11 x64 desktop and fails
visibly when foreground input acknowledgement cannot be observed. Source tests
or CI do not substitute for this native evidence.

## Delivered harness

- Content-free QPC startup traces cover process start, initial state readiness,
  first GPUI post-render frame, and `F24` input acknowledgement.
- Formal evidence requires fresh controlled build manifests, immutable private
  executable copies, matching source/host/session, a quiet record no older than
  five minutes, at least ten full-to-no-model A-B-B-A rounds, and an approved
  pre-measurement threshold.
- Warm and fresh-profile evidence are distinct. Warm runs reuse one isolated
  profile per variant after at least one warmup; fresh-profile runs create a new
  profile for every launch and do not claim to flush the Windows file cache.
- `decide-goal04` requires both cache modes, matching builds and threshold, the
  current source state, and explicit owner approval. Below-threshold evidence
  can only select `keep in-process`.
- Build evidence records Cargo.lock, release profile, features, dependency
  attribution, PE sections, executable identity, and normalized content-free
  `cargo-bloat` labels. The no-model manifest records its exact behavior test.
- Evidence outputs cannot overwrite executables, input evidence, or the document
  supplied through `--open`.

## Automated validation

| Command | Result |
|---|---:|
| `uv run --project scripts scripts/mt.py check full` | PASS |
| Tooling tests within `check full` | 208 passed |
| `cargo fmt --all -- --check` | PASS |
| `cargo clippy --workspace --all-targets --locked` | PASS; no warnings |
| `cargo test --release --workspace --locked` | 864 passed, 0 failed, 9 ignored |
| Safe local release binary build within `check full` | PASS |
| Exact no-model unavailable-diagnostic test recorded by `build-goal04` | 1 passed, 0 failed |

Clippy emitted a non-fatal Windows incremental-cache finalization warning for
`vendor/ratex-parser`; the command completed successfully and reported that the
next build would not reuse that incremental session.

## Completion boundary

No product process boundary changed in Goal 04. The temporary no-model feature
remains measurement apparatus, while the default build retains model transport.
Because the current-source quiet gate was blocked and no passing quiet evidence
exists, the project-owner-approved downstream decision is to keep model
transport in-process and authorize no extraction. Goal 04 is complete via its
inconclusive Stop Rule. A future owner-approved rerun on a qualifying host must
generate fresh source-bound builds, threshold, quiet, startup, first-use, and
decision artifacts before Goal 05 may consider extraction.
