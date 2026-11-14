# TODO

## Deferred

- [ ] **First-time-user and screen-reader evidence for the shipped UI** — Goal
  09 was accepted on 2026-11-13 without the five first-time-target-user sessions
  and the Windows 11 x64 keyboard/screen-reader smoke its completion evidence
  lists. PRODUCT.md still requires the first-time-user evidence before
  public-quality visual acceptance, so record both against the eventual
  published artifact before any future public-quality release; the Goal 10
  sequence was archived execution-deferred on 2026-11-14, so the renewal entry
  below carries these prerequisites. See
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
- [ ] **Release renewal after the archived Goal 10/11 sequence** — On
  2026-11-14 the owner archived Goals 10 and 11 as execution deferred, not
  accepted: the product is not considered ready, no target users are available
  to try it, and further UI adjustments are intended. Renewal prerequisites, in
  order: owner authorization for a bounded UI iteration (none is recorded yet);
  resolve the observed behavioral failures, including the collapsed Source pane
  decided on an active desktop session; decide the signing route for the stable
  channel; produce a new candidate with its own installer, CI, clean-machine and
  owner-run evidence — including the keyboard/Narrator smoke, the five
  first-time-user sessions, the credential-path four-lens Review with the Agent
  Skill disclosure-versus-transmitted comparison, named Effective Agent Context
  selection and the installed mixed accepted/rejected revision; publish only
  after owner approval of the artifact, screenshots, demonstration and release
  copy; and run Goal 11 only with a compliant cohort once the approved window
  starts. `v0.1.4-alpha.4` and `v0.1.4-alpha.5` are retained as historical
  unpublished candidates and must not be published, rebuilt or asset-replaced;
  any future candidate uses a new version/tag with its own acceptance evidence.
  See [goal-10-acceptance.md](goal-10-acceptance.md) and the archived
  [Goal 10](goals/archive/10-ship-primary-platform-release.md) and
  [Goal 11](goals/archive/11-validate-recurring-user-value.md).
