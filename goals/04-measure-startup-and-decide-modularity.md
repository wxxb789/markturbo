# Goal 04 — Measure startup and decide capability modularity

## Objective

Produce a reproducible go/no-go decision on extracting optional capabilities by
measuring the full application, a bare GPUI shell, and applicable compile-time
ablations of the model/network stack, diagram/math backends, WebView integration,
and non-core tree-sitter grammars on the primary platform; collect at least 10
quiet-gated A-B-B-A rounds for each shipping candidate, include first-visible,
first-painted, first-input, idle-memory, baseline first-use, executable, and
archive-size evidence, and adopt no runtime module architecture without a
pre-registered numeric materiality threshold.

## Question this goal must answer

Would removing a capability from the initial process materially improve the
user-observed launch path, or would dynamic loading add complexity around a
startup cost dominated by GPUI/platform initialization?

## In scope

- Extend the startup harness to distinguish:
  - process creation;
  - titled window visible;
  - first application frame painted;
  - first input successfully handled;
  - initial workspace or welcome state ready.
- Record cold and warm behavior separately where the platform permits an honest
  distinction. If the selected platform cannot use the Windows-only
  `scripts/probe.py` quiet/startup mechanisms, build and validate an equivalent
  platform-native harness rather than translating its claims by assumption.
- Quantify or symmetrically include the overhead of any instrumentation added to
  observe first paint or first input, so the probe does not decide its own result.
- Measure, from the same source revision and release configuration:
  1. the full application;
  2. a bare GPUI window using the same platform setup;
  3. the application without the model/network dependency closure;
  4. the application without Mermaid, D2, and RaTeX;
  5. the application without WebView integration, when that capability exists on
     the selected platform;
  6. the application with only the minimum Markdown/editor grammar set, retaining
     any temporary source adjustments solely to make the ablation compile.
- Use compile-time removal as the upper bound on what later explicit loading can
  save. For each ablation, record the exact user behavior lost—especially fenced
  code highlighting—so an upper-bound number is not mistaken for a shippable
  product. Temporary feature gates are acceptable measurement apparatus but must
  not become an accidental product configuration matrix.
- Treat a dynamic library as lazy only when runtime module inspection proves it
  is absent before first use; ordinary loader-time dynamic linkage does not meet
  the goal merely because code moved into another file.
- Re-run the existing `opt-level = 3` versus `opt-level = "s"` comparison using
  the repository quiet gate and matching fresh builds.
- Record executable size, complete distributable archive size, idle working set
  and private bytes, page faults or image-load evidence where available, and the
  full build's baseline first-use latency for each candidate capability. An
  ablated build has no first-use result and must not be assigned an estimate.
- Use current `cargo bloat`/section evidence rather than stale historical sizes.
- Before examining A/B results, record a numeric threshold for a material launch
  or idle-memory improvement and have the project owner approve it.
- End with one explicit decision for each candidate: keep static, explicitly load,
  isolate in a worker, investigate upstream, or reject.

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
- The bare shell, full build, and each of the four ablation rows have recorded
  results or an explicit platform-not-applicable result; a platform-inapplicable
  WebView case is labeled rather than fabricated.
- The `opt-level = "s"` deferred decision is resolved or explicitly remains
  blocked by a failed quiet gate; no noisy result is promoted to evidence.
- One owner-approved go/no-go table records the materiality threshold and the
  decision for model transport, renderers, WebView, and grammar work.
- Measurement-only code and retained harness changes are clearly separated.
- `cargo fmt --all -- --check`, relevant harness tests, and
  `cargo test --release --workspace` pass; the pass count is recorded.

## Stop and ask

Use the existing bounded quiet-machine wait, or another duration approved before
the run. If no machine passes, a startup milestone cannot be observed honestly,
or no numeric threshold is approved, preserve the result as inconclusive and
authorize no extraction; Goal 05 then takes its no-go path and product work may
continue at Goal 05A/06. Ask only if the owner wants to delay product work for a
new measurement environment rather than accept that conservative outcome.

## Boundary for the next goal

This is a measurement and decision goal. Goal 05 alone may change the model
transport process boundary, and only when this goal's evidence authorizes it.
