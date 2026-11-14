# TODO

## Deferred

- [ ] **First-time-user and screen-reader evidence for the shipped UI** — Goal
  09 was accepted on 2026-11-13 without the five first-time-target-user sessions
  and the Windows 11 x64 keyboard/screen-reader smoke its completion evidence
  lists. PRODUCT.md still requires the first-time-user evidence before
  public-quality visual acceptance, so record both against the shipped artifact
  during Goal 10's clean-machine acceptance; see
  [goal-09-acceptance.md](goal-09-acceptance.md).
- [ ] **Downloadable macOS arm64 app** — Build a `.app`, sign it with an Apple
  Developer ID, notarize and staple it, publish the artifact, and verify a
  downloaded copy launches from Finder on a clean Mac. Requires Apple Developer
  Program credentials and a macOS signing environment. Diagnose the reported
  non-running binary separately; a new package does not explain its failure.
- [ ] **静默 A/B** — Re-run the `opt-level = 3` versus `opt-level = "s"`
  startup and first-formula comparisons after
  `uv run --project scripts scripts/mt.py probe -- quiet --wait-seconds 3600`
  passes. Keep the current
  decision not to adopt `s` until the quiet-machine measurements are complete.
  See [ticket 05](../docs/evidence/perf-and-size/issues/05-opt-level-s.md).
- [ ] **Pele Mermaid renderer** — Evaluate [Pele](https://github.com/obsidianmd/pele)
  as a replacement for `mermaid-svg` only after a pure-Rust or embeddable
  (WASM) build matching current Pele syntax exists. Pele is a TypeScript/npm
  package; the native path deliberately has no JS runtime and untrusted
  documents keep scripts blocked, so the WebView is not a substitute. The
  available port, `bezel-mermaid` 0.2.25 (Pele 1.6.0), measured slower than
  `mermaid-svg` 0.7 on this repo's diagrams (flowchart 130 µs vs 104 µs,
  40-node flow 638 µs vs 436 µs) with larger SVG output. Revisit with a
  benchmark against `mermaid-svg` on representative documents, a measured
  improvement threshold, and no new JS/script-execution requirement.
- [ ] **Retired native acceptance adapters** — `mt.py accept goal-02`,
  `goal-03`, `goal-06`, `goal-07` and their `scripts/markturbo_tools/native/*`
  modules still drive the retired GPUI shell (`crates/mt-app/src/views/` no
  longer exists; `docs/goal-09-acceptance.md` states the selectors are retired
  and that rebuilding them for nsh is not part of that gate). They cannot
  produce acceptance for the current nsh app; `check native` still compiles and
  unit-tests them. Decide per adapter whether to rebuild on the nsh UIA surface
  or delete the adapter, its module, and its `ACCEPTANCE_GOALS` entry, so the
  CLI stops advertising an acceptance path that cannot pass. Revalidate the
  candidate before removing anything.
- [ ] **`cargo run --release -p mt-app` panics at startup** — `README.md`
  documents the plain binary as "the same app without the nsh CLI", but it exits
  101 with `no reactive runtime: signals are used on the engine thread, while
  components run` (`nsh-core` 0.4.0 `element/reactive.rs:164`) from
  `markturbo::main`, while the `nsh package` runner binary from the same revision
  starts and shows its window normally (measured 2026-11-14). Confirm whether it
  predates the usage-counters change, then either fix the entry point or correct
  the documentation so the documented dev path is not a panic.
