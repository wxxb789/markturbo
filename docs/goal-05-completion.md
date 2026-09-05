# Goal 05 Completion and No-Change Decision Report

**Date:** 2026-09-04
**Goal:** [Goal 05](goals/archive/05-isolate-model-transport-on-demand.md) -
Isolate model transport on demand, if validated.
**Status:** COMPLETE VIA THE REQUIRED NO-GO PATH; NO MODEL WORKER AUTHORIZED.

Goal 05 was conditional on Goal 04 specifically authorizing a worker after a
pre-approved materiality threshold was met. That condition was not satisfied,
so this goal records the required no-change decision and advances product work
to Goal 05A and Goal 06 without adding a process boundary.

## Decision basis

The retained [Goal 04 completion and decision report](goal-04-completion.md)
records all of the gating facts:

- no numeric materiality threshold was approved and no threshold artifact was
  created;
- the current-source quiet gate was blocked, so no full/no-model warm or
  fresh-profile A-B-B-A runtime samples were collected;
- the measured 4,717,568-byte executable reduction was explicitly classified as
  a size upper bound, not launch or idle-memory evidence; and
- the project owner approved `keep in-process` on 2026-09-03 and authorized no
  model-transport extraction.

Under Goal 05's Stop and ask rule, any one of an absent threshold, inconclusive
runtime evidence, or a decision that does not select a worker requires the no-go
path. All three apply here. Reopening the question requires the fresh,
source-bound threshold, quiet-gate, paired startup, first-use, and decision
evidence specified by Goal 04; this goal does not weaken that gate.

## Retained boundary

The current in-process provider boundary remains authoritative:

- `Provider::build_with` in `crates/mt-app/src/translate.rs` resolves the selected
  wire format, endpoint, credential, and model into a `TranslationService`;
- `GenAiTranslator` performs the provider request inside `mt-app`; and
- `transport` owns the shared Tokio runtime and `genai` client inside the same
  application process.

The default `model-transport` feature remains the Goal 04 measurement apparatus:
the public build keeps it enabled, while disabling it represents only the
compile-time upper bound used by the retained measurement harness.

No model-worker executable, dynamic ABI, private process protocol, product
feature matrix, execution ledger, or placeholder abstraction was added. The
WebView and recovery code already use internal worker threads; those unrelated
implementation details are not a model-transport process boundary.

## Downstream direction

- Goal 05A now explicitly secures credentials, endpoint identity, consent, and
  outbound scope through the retained in-process provider boundary.
- Goal 06 now explicitly uses that same boundary and forbids a model worker,
  second transport path, or process protocol. Its source-size limits remain
  product request bounds rather than compatibility with an unimplemented worker
  frame.
- Goal 05A and Goal 06 create no separate execution ledger or placeholder
  implementation for a future worker.

## Validation

- `uv run --project scripts scripts/mt.py check fast`: PASS; 203 tooling tests.
- `uv run --project scripts scripts/mt.py check full`: PASS; 203 tooling tests;
  formatting and Clippy; 864 Rust tests passed, 0 failed, 9 ignored; safe local
  release binary build.

Clippy emitted the known non-fatal Windows incremental-cache finalization warning
for `vendor/ratex-parser`; it completed successfully. No native acceptance run
applies because this decision changes neither executable behavior nor process
topology.
