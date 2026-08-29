# Goal 04 — Measure startup and decide model-transport modularity

## Objective

Produce a reproducible, gating go/no-go decision only for isolating the
model/network stack on Windows 11 x64. Measure the full application, a bare GPUI
shell, and a compile-time ablation of that stack; collect at least 10
quiet-gated A-B-B-A rounds, include first-visible, first-painted, first-input,
idle-memory, baseline first-use, executable, and archive-size evidence, and
adopt no model runtime architecture without a pre-registered numeric materiality
threshold. Renderer, WebView, and grammar ablations are optional diagnostics and
cannot delay later product goals.

## Question this goal must answer

Would removing the model/network dependency closure from the initial process
materially improve the user-observed launch path, or would a worker add
complexity around a startup cost dominated by GPUI/platform initialization?

## Product contract alignment

**Disposition:** Retained and revised on 2026-08-29.

`PRODUCT.md` makes only model transport a possible startup-driven architecture
decision. Renderer, WebView, and grammar measurements may be retained as
diagnostics when already available, but cannot authorize extraction or removal
and are not a completion condition.

## In scope

- Extend the startup harness to distinguish:
  - process creation;
  - titled window visible;
  - first application frame painted;
  - first input successfully handled;
  - initial workspace or welcome state ready.
- Record cold and warm behavior separately where Windows 11 x64 permits an
  honest distinction, using the Windows-only `scripts/probe.py` quiet/startup
  mechanisms as the primary evidence. Other platform measurements are
  non-release diagnostics and cannot replace this gate.
- Quantify or symmetrically include the overhead of any instrumentation added to
  observe first paint or first input, so the probe does not decide its own result.
- Measure, from the same source revision and release configuration:
  1. the full application;
  2. a bare GPUI window using the same platform setup;
  3. the application without the model/network dependency closure.
- Renderer, WebView, or grammar ablations may be recorded as explicitly
  non-gating diagnostics when the measurement apparatus already exists. They may
  be skipped without blocking Goal 05A or Goal 06 and cannot become a product
  feature matrix.
- Use compile-time removal as the upper bound on what later explicit loading can
  save. Record the exact model-backed behavior lost so an upper-bound number is
  not mistaken for a shippable product. Temporary feature gates are acceptable
  measurement apparatus but must not become a product configuration matrix.
- Treat a dynamic library as lazy only when runtime module inspection proves it
  is absent before first use; ordinary loader-time dynamic linkage does not meet
  the goal merely because code moved into another file.
- Re-run the existing `opt-level = 3` versus `opt-level = "s"` comparison using
  the repository quiet gate and matching fresh builds.
- Record executable size, complete distributable archive size, idle working set
  and private bytes, page faults or image-load evidence where available, and the
  full build's baseline model first-use latency. The ablated build has no
  first-use result and must not be assigned an estimate.
- Use current `cargo bloat`/section evidence for the model/network closure rather
  than stale historical sizes.
- Before examining the model/network A/B results, record a numeric threshold for
  a material launch or idle-memory improvement and have the project owner
  approve it.
- End with one explicit, owner-approved decision for model transport: keep
  in-process, isolate in a worker, investigate upstream, or reject.

## Out of scope

- Implementing a DLL/plugin ABI, helper process, updater, or downloadable pack.
- Changing product scope to make a favorable benchmark.
- Removing a required capability from the distributed product.
- Treating total binary size as proof of startup improvement.
- Reporting measurements from a machine that fails the pre-registered quiet
  gate as decision evidence.
- Optimizing renderer hot paths or workspace search unless tracing proves they
  are on the measured startup path.

## Evidence standard

Every reported number must include:

- source revision and dirty-tree state;
- target triple, hardware, GPU situation, OS build, and release profile;
- exact command;
- raw samples, median, and p95 where applicable;
- quiet-gate result;
- whether the run was cold or warm;
- the compared executable and archive sizes.

For a paired shipping decision, use at least 10 A-B-B-A rounds after any declared
warm-up, matching `scripts/probe.py` ordering. A single launch is not evidence.

## Completion evidence

- The enhanced harness can reproduce every startup milestone it claims to
  measure and fails visibly when it cannot.
- The bare shell, full build, and model/network ablation have recorded results;
  the model/network row is the only paired shipping decision.
- The `opt-level = "s"` deferred decision is resolved or explicitly remains
  blocked by a failed quiet gate; no noisy result is promoted to evidence.
- One owner-approved go/no-go table records the materiality threshold and
  decision for model transport. Any compatibility observation is separately
  labeled optional diagnostic evidence.
- Measurement-only code and retained harness changes are clearly separated.
- `cargo fmt --all -- --check`, relevant harness tests, and
  `cargo test --release --workspace` pass; the pass count is recorded.

## Stop and ask

Use the existing bounded quiet-machine wait, or another duration approved before
the run. If no machine passes, a model/network startup milestone cannot be
observed honestly, or no model materiality threshold is approved, preserve that
decision as inconclusive and authorize no extraction; Goal 05 then takes its
no-go path and product work may continue at Goal 05A/06. Ask only if the owner
wants to delay the model-transport decision for a new measurement environment.

## Boundary for the next goal

This is a measurement and decision goal. Goal 05 alone may change the model
transport process boundary, and only when this goal's evidence authorizes it.
