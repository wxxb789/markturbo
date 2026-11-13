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

