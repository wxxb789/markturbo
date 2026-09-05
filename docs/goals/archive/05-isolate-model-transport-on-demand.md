# Goal 05 — Isolate model transport on demand, if validated

## Objective

Honor Goal 04's recorded model-transport decision: unless it specifically
authorizes a worker and meets its approved materiality threshold, complete this
goal as a documented no-change decision; when it does, move the
`genai`/HTTP/TLS/runtime dependency closure into one version-matched worker
bundled with markturbo, launch no worker before the first explicit Review or
Translation request, preserve current Translation behavior, and verify the
approved startup or idle-memory gain with the same paired harness.

## Required boundary when the go decision exists

```text
markturbo core
    document model · filesystem · editor · native preview · UI
            │
            │ private, versioned request/response protocol
            ▼
markturbo model worker
    provider routing · genai · async runtime · HTTP · TLS
```

The worker is a capability boundary, not a generic plugin platform.

## Product contract alignment

**Disposition:** Retained and revised on 2026-08-29.

This goal preserves the configured endpoint and Translation behavior promised by
`PRODUCT.md` while making a worker conditional on Goal 04's Windows 11 x64
evidence. It neither expands the provider catalog nor changes the explicit,
user-initiated model-data boundary established by Goal 05A.

## In scope

- One bundled worker executable for model-backed operations.
- A narrow, versioned protocol with an explicit startup handshake, an encoded
  request-frame limit of 8 MiB, and an encoded response-frame limit of 1 MiB.
  These limits bound protocol frames only. `PRODUCT.md`'s per-request outbound
  disclosure and consent requirements, and Review's structured-response
  validation, still apply.
- On-demand process creation from the first explicit model-backed action; an
  ordinary launch, edit, preview, search, or context inspection starts no worker.
- Parent-owned lifecycle: the worker exits when its parent exits and does not
  remain as a background service.
- Communication over inherited stdio or another private local channel with no
  listening network port.
- API keys and selected document content travel in the private request channel,
  never command-line arguments, process titles, or logs.
- The core sends only content required by the invoked operation and retains
  authority over whether returned changes may be applied.
- Preserve the existing configured endpoint behavior, including custom base URL,
  local OpenAI-compatible endpoint, and Translation behavior; establish and
  document timeout and cancellation semantics rather than assuming the current
  transport already provides them. Do not add a provider, wire format, or new
  provider-routing capability.
- Actionable behavior for a missing, mismatched, crashed, timed-out, or malformed
  worker; the editor and local workflows remain usable.
- Stage the worker in development and embed any required worker payload in the
  single Windows 11 x64 release executable. Runtime materialization, if needed,
  uses app-owned data rather than a sibling sidecar. Goal 10 owns production
  signing and installer policy.
- Keep protocol types small and independent of GPUI, `genai`, and renderer
  dependencies.

## Out of scope

- Moving Mermaid, D2, RaTeX, PlantUML, WebView, tree-sitter grammars, or the
  document engine into this worker.
- Third-party plugins, arbitrary worker discovery, a marketplace, or optional
  first-run downloads.
- Running a bundled local language model or managing model weights.
- Implementing Review semantics or UI; Goal 06 owns that consumer.
- Exposing a localhost server or remotely callable API.

## Completion evidence for the go path

1. Process inspection proves an idle markturbo launch has exactly the expected
   application process and no model worker.
2. The core application's desktop-target dependency tree has no model-owned path
   to `genai`, its HTTP client, TLS provider, or Tokio runtime; any unrelated
   transitive occurrence is attributed rather than hidden.
3. The first Translation request starts the worker and succeeds against the
   existing local mock server fixtures.
4. Subsequent requests reuse or deliberately restart the worker according to a
   documented lifecycle, with no orphan process after application exit.
5. Missing, version-mismatched, crashed, malformed, oversized, and timed-out
   worker cases produce diagnostics and do not change document text.
6. Logs and process arguments contain neither API keys nor request content.
7. The single release executable contains every required worker artifact and
   works after moving only that file to a clean machine.
8. The Goal 04 paired harness confirms the pre-approved improvement and records
   any first-request regression and total executable-size change.
9. `cargo fmt --all -- --check`, `cargo clippy --workspace --all-targets`, focused
   protocol/integration tests, and `cargo test --release --workspace` pass; the
   pass count is recorded.

## Completion evidence for the no-go path

- The Goal 04 decision and threshold are cited in its retained decision result.
- No dynamic ABI, process protocol, feature matrix, or placeholder abstraction is
  added.
- Goals 05A and 06 are explicitly directed to use the existing in-process
  provider boundary; no separate execution ledger or placeholder implementation
  is created.

## Stop and ask

If Goal 04 is absent, inconclusive, below threshold, or does not specifically
select a worker, take the no-go path and continue without architectural work.
Stop and ask only when an authorized worker cannot preserve single-asset
packaging or a secure private protocol on Windows 11 x64. Do not reinterpret
“dynamic” as permission to expose unstable Rust trait objects across a library
ABI.

## Boundary for the next goal

This goal changes where model transport executes, not what a Review means. Goal
05A next secures credentials and outbound requests for either architecture; Goal
06 then defines the read-only Review product behavior.
