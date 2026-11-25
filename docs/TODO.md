# TODO

## Deferred

- [ ] **Keyboard press feedback on Win32**: nsh 0.6.1 dispatches only `Press`
  for keyboard activation, so pressables show no press-down visual for keyboard
  use. Revisit when nsh exposes keyboard press-down and release events.
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
  package; the native path deliberately has no JS runtime, and the WebView's
  isolated inline-script policy is not a substitute for a native renderer. The
  available port, `bezel-mermaid` 0.2.25 (Pele 1.6.0), measured slower than
  `mermaid-svg` 0.7 on this repo's diagrams (flowchart 130 µs vs 104 µs,
  40-node flow 638 µs vs 436 µs) with larger SVG output. Revisit with a
  benchmark against `mermaid-svg` on representative documents, a measured
  improvement threshold, and no new JS/script-execution requirement.
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
- [ ] **Collapsed Source pane for every document** — Measured on 2026-11-14 on
  the packaged runner and on the installed `0.1.4-alpha.5`: the editor reports a
  **0×25** rectangle with an empty UIA value even after the Edit/Markdown-source
  toggle and after maximizing, for both a file-backed open and a `New`
  document, so it is not specific to the file-open path. `ValuePattern` still
  accepted and stored 44 UTF-8 bytes and the buffer was checkpointed, so the
  editing path works and the failure is layout/accessibility. Deciding whether
  it is a disconnected-session artifact or a real defect needs an **active,
  unlocked desktop**; do that before any future release, keeping the installed
  executable, document and profile unchanged, and record the result without
  converting the recorded failure into a pass. On 2026-11-24, on an active,
  unlocked desktop, a new document's Markdown source pane had a normal size;
  that supports the disconnected-session explanation, but it used a different
  build and profile from the recorded failure, so this entry stays open.
  Evidence:
  [goal-10-acceptance.md](goal-10-acceptance.md) environment dispositions 2, 3
  and 7 plus its post-acceptance fixes section.
- [ ] **Finish verifying the uninstall close helper** — `close-instances.ps1`
  aborts the uninstall while an instance still responds and force-closes only
  pinned, path-checked unresponsive survivors. Measured: a run with no instance
  exits 0, and a silent uninstall with a running instance completed with exit 0
  without a force-kill while that instance's recovery checkpoints survived.
  Still unexercised: the **abort** path (the app closes on `WM_CLOSE` without
  the interactive save/discard guard, so nothing in the disconnected session
  refused the close) and the kill path for an unresponsive survivor. Exercise
  both on a live session; if the app should prompt on a close request, fix the
  app rather than the helper.
- [ ] **Unattended native acceptance runner** — the Goal 10 sessions ran in a
  disconnected desktop where `SendInput` inserts zero events, `SetCursorPos`
  fails, `GetForegroundWindow` is None, the clipboard is unavailable and the
  visual matrix returns blank cells for low-content states, while the harness
  itself requires an active, unlocked input desktop
  (`scripts/markturbo_tools/native/runtime.py:973-998,1393`). Measured on
  2026-11-14: Windows **virtual desktops do not isolate input** (`EnumDesktops`
  returns only `Default`), `tscon <id> /dest:console` needs administrator rights
  (Error 5 on this machine), `Get-VM` is permission-denied and this account has
  no admin rights. The realistic unattended arrangement is a dedicated Hyper-V
  Windows 11 x64 guest whose own logged-in desktop runs the app, UIA, input and
  capture, with fresh baseline restores for install acceptance; it needs
  one-time elevation to provision and cannot replace the human items
  (keyboard/Narrator smoke, five first-time users, owner approval).
- [ ] **Decide whether installer support files belong in `release.json`** —
  `installer/close-instances.ps1` is installed into `{app}\installer-support\`
  from a direct `[Files]` source outside the staged payload directory, so it is
  part of the installed application files but absent from the manifest payload
  inventory (`scripts/markturbo_tools/release.py:281-285,771`). Either stage it
  with the payload so the inventory covers every installed file, or document the
  exclusion in the release-notes template; decide before the next candidate.
- [ ] **Upgrade nsh for the Goal 15 WebView2 popup fix** — the 2026-11-23
  native run failed goal-07 (`WEB_SECURITY_APP_WINDOW_COUNT_UNEXPECTED`, case
  `revision_session`): the fixture's `window.open` leaves a visible WebView2
  popup, and nsh 0.6.1 handles `NewWindowRequested` only after the page
  closes. Upgrade every nsh crate to the first release containing
  [longbridge/nsh#2423](https://github.com/longbridge/nsh/pull/2423) (issue
  [#2785](https://github.com/longbridge/nsh/issues/2785) closed 2026-11-23),
  then re-run goal-07. See
  [goal-15-acceptance.md](goal-15-acceptance.md#native-acceptance-2026-11-23).
- [ ] **Visual harness observation channel for the WebView states** — the
  2026-11-24 visual matrix captured 20 of 22 cells; both
  `web-compatibility-preview` cells failed with
  `WEB_COMPATIBILITY_PREVIEW_WEB_VIEW_MISSING_UIA_CONTRACT_MISMATCH` because
  the harness looks for the WebView through UIA, and nsh 0.6.1 hosts WebView2
  in a composition controller with no automation provider. Rebuild that proof
  on an observation channel nsh 0.6.1 exposes (for example pixels from a window
  capture), and add consent-card and title-menu states if before/after
  captures of them are still wanted. See
  [goal-16-acceptance.md](goal-16-acceptance.md#desktop-acceptance-2026-11-24).

- [ ] **nextest gate follow-ups (PR #75 review, P3)**:
  - `.config/nextest.toml` is not in the PR workflow's `rust` path filter, so a PR that changes only the nextest
    config skips the jobs that use it.
  - The `serial` test group keeps the mt-core timing tests off each other, but they can still run beside other test
    binaries. `crates/mt-core/tests/performance.rs` asserts wall-clock bounds, so give those tests the whole pool.
  - The doctest guard in `scripts/tests/test_checks.py` checks only that `--doc` appears. If doctests are enabled
    again, verify which crates the command actually covers.
- [ ] **Goal 20 lane 20-A test hardening (PR #71 review P3s)**:
  - The nightly runs plain `cargo test --workspace`, so `recovery_undo.rs`, `folder_open_token.rs` and
    `reload_edit_race.rs` share one process. Cases in one binary race on `MARKTURBO_CONFIG_DIR`, `MARKTURBO_DATA_DIR`
    and `MARKTURBO_OPEN`. Move the nightly to nextest, or give these fixtures per-test isolation.
  - `nsh-testing` 0.6.1 `Headless::react()` answers a `SaveDialog` at once. The Quit test in `translation_flow.rs` can
    gate only the app's read of the answer, not the platform's reply, so it does not prove the translation lands while
    the picker is unanswered.
  - The Chinese Save As guard formats a raw OS error through `io::Error::Display`, which appends the English
    ` (os error N)`.
  - `wait_until()` gates on test markers that schedule no UI batch (`folder_open_token.rs`, `reload_edit_race.rs`,
    `translation_flow.rs`). Under the system clock they can time out after the condition already holds.
