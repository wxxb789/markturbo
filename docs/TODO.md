# TODO

## Deferred

- [ ] **Escape with an editor selection during an in-flight Translation collapses the selection first**; revisit if
  users expect one Escape to cancel.
- [ ] **Keyboard press feedback on Win32**: nsh 0.7.1 dispatches only `Press`
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
- [ ] **Re-run goal-07 for the WebView2 popup fix** — every nsh crate is now
  0.7.1, the first release containing
  [longbridge/nsh#2423](https://github.com/longbridge/nsh/pull/2423) (issue
  [#2785](https://github.com/longbridge/nsh/issues/2785) closed 2026-11-23):
  a `window.open` no longer opens a WebView2 popup, so the goal-07
  `revision_session` case should pass. The upgrade has landed; only the
  native goal-07 re-run remains. See
  [goal-15-acceptance.md](goal-15-acceptance.md#native-acceptance-2026-11-23).
- [ ] **Visual harness observation channel for the WebView states** — the
  2026-11-24 visual matrix (commit `32f324d`, nsh 0.6.1) captured 20 of 22
  cells; both `web-compatibility-preview` cells failed with
  `WEB_COMPATIBILITY_PREVIEW_WEB_VIEW_MISSING_UIA_CONTRACT_MISMATCH` because
  the harness looks for the WebView through UIA, and nsh 0.6.1 hosts WebView2
  in a composition controller with no automation provider. A source reading of
  nsh 0.7.1 suggests the composition-controller hosting is unchanged, but
  that is unverified natively. Rebuild that proof on an observation channel
  nsh 0.7.1 exposes (for example pixels from a window capture), and add
  consent-card and title-menu states if before/after captures of them are
  still wanted. See
  [goal-16-acceptance.md](goal-16-acceptance.md#desktop-acceptance-2026-11-24).

- [ ] **nextest gate follow-ups (PR #75 review, P3)**:
  - `.config/nextest.toml` is not in the PR workflow's `rust` path filter, so a PR that changes only the nextest
    config skips the jobs that use it.
  - The `serial` test group keeps the mt-core timing tests off each other, but they can still run beside other test
    binaries. `crates/mt-core/tests/performance.rs` asserts wall-clock bounds, so give those tests the whole pool.
  - The doctest guard in `scripts/tests/test_checks.py` checks only that `--doc` appears. If doctests are enabled
    again, verify which crates the command actually covers.
- [ ] **Goal 20 lane 20-A test hardening (PR #71 review P3s)**:
  - `nsh-testing` 0.7.1 `Headless::react()` answers a `SaveDialog` at once. The Quit test in `translation_flow.rs` can
    gate only the app's read of the answer, not the platform's reply, so it does not prove the translation lands while
    the picker is unanswered.
  - The Chinese Save As guard formats a raw OS error through `io::Error::Display`, which appends the English
    ` (os error N)`.
- [ ] **Goal 20 lane 20-C follow-ups (PR #74 review)**:
  - Accepted residual: on Windows, `delete_resolved()` (`crates/mt-core/src/credentials.rs`) compares the secret and
    `LastWritten`, then calls `CredDeleteW` by target name. If the Credential Manager replaces the entry between the
    compare and the delete, the newer credential is deleted.
  - A staging temp file held open past the 5 s usage Undo window: `clear()` ignores the cleanup failure, but
    `restore()` still needs the same file removed. The Undo can expire unrestored, and the staged counts can stay on
    disk.
  - The usage-failure notice is matched by its translated text. If the UI language changes between a failed and a
    successful Undo, the red Settings notice stays. Match on a notice key instead of the text.
- [ ] **Goal 20 lane 20-B wait-state polish (PR #72 review P3s)**:
  - The disabled titlebar Review and the menu Review selection state their wait reason only in the accessible
    description, so pointer users never see it. Show it visibly.
  - The "waiting for another document" notice is a persistent Info status, and it stays after the other request
    finishes. Clear it when the slot frees.
  - While a Review is in flight elsewhere, "Review with context" in the Context panel closes the panel and opens no
    card. Disable it with the same reason instead.
- [ ] **`acceptance-exe.yml` pull-ref guard (PR #78 review P3).** The PowerShell `Trim()` does not strip a leading
  U+FEFF, but `actions/checkout` does, so a dispatcher can still check out a `refs/pull/*` ref. With
  `cache-mode: none`, no cache path remains. Harden the guard (for example, reject any input not matching
  `^[A-Za-z0-9._/-]+$`) if the workflow gains more trust.
- [ ] **Settings 4 px grid after a provider is set (PR #77 review P3).**
  - At 1280 pt, switching from `Best available` to a configured provider, with secure storage available and no saved
    credential, turns the 36 pt `Unconfigured` hint into an 18 pt `Missing` one. The API key field and the sections
    below it then sit 2 pt off the 4 px grid.
  - Size single-line status variants to a 4 px multiple.
  - Extend the `goal17_settings` grid test to cover that switch.
- [ ] **`focus_after_close` strictness (PR #80 review).**
  - The test asserts that the new editor's first focus arrives within the app's own retry window
    (`FOCUS_RETRY_FRAMES`), not that a key typed at the exact frame the editor appears lands.
  - `reload_edit_race`'s no-notice assertion can still read the screen before a post-marker notice batch.
  - Tighten both only if nsh-testing gains a frame-exact or task-completion hook.
- [ ] **Drops while a card or Settings is open (Goal 22 lane 22-B).** The drop handlers live on the
  shell root, so a file dragged over an open consent card, the close guard, or the Settings page still
  shows the drop screen (the overlay layer draws it above the card) and a drop opens documents behind
  the open surface; the card's decision keeps referring to the tab it was opened for, so nothing is
  misattributed. Decide whether those surfaces should swallow drags instead.
- [ ] **`palette_folder::choosing_a_palette_file_focuses_the_opened_editor` fails on this workstation,
  at the base commit `c049c0ed` too (not a Goal 22 lane 22-B regression).** A palette-row click opens
  the file, and the plain key after it lands nowhere: `AppState::retry_editor_focus` reaches the new
  editor's `api()` one frame before the editor's own input has a node, nsh drops the deferred
  `focus()` call then, and the retry treats that first `api()` match as success. The app-side trace
  shows a `Focus` for the palette input and none for the editor; a frame of extra work anywhere in
  that window (an added `set_interval`, or the disk drain) flips the outcome. Verifying the focus
  against the editor box's `on_focus_in` signal instead of `api()` breaks `focus_after_close`'s
  "first focus" assertion, so the repair is a design decision for a Goal 20 follow-up (nsh may also
  fix the dropped call upstream).
- [ ] **Goal 22 lane 22-A residuals (implementation review, 2026-11-27).**
  - Two guards implement one rule: `state.rs::observe_view_stop` snaps the track under reduced motion and
    `TrackTween::to` does the same, so removing either alone is unobservable (the lane's mutation check had to
    remove both to redden the reduced-motion acceptance test). Keep one guard when either is refactored.
  - `scripts/tests/test_native_runtime_uia.py::test_sidebar_and_view_selectors_are_in_the_product_ui` cuts each
    Rust file at `"\n#[cfg(test)]\nmod tests"` without asserting the marker was found, so a renamed test module
    would let test-only literals satisfy it. Assert the split produced two parts.
  - The stage's real width comes from flex layout while its children compute `client_width - sidebar_width`; the
    two agree today but one source of truth would be better. `Pane::web()` also duplicates `Tab::web_preference`
    outside Edit, and `chosen_pane`/`chosen_web` mirror each other.
  - The renderer choice made in Edit is not persisted (`document_pane` stores `Source`), so it is lost across a
    restart; before this lane a stored `Web`/`SplitWeb` survived only while the last view was not Edit.
  - Crossing the 1280 px key mid-slide re-keys the shell, so the sidebar lands at its target instead of
    continuing the slide; accepted (a re-key is not a movement), but noted.
  - Two quick opens of one path before its read returns can still create two tabs, and the tree's path -> tab
    map keeps one of them (pre-existing race in the open path).
  - `document`'s new identity test mounts `document::column` directly, so it cannot catch a shell-level remount,
    and the old "a remounted editor cuts to the held line" oracle is gone; `shell`'s field-gap test measures the
    source screen rather than the Review column inside the dock.

## Goal 22 lane 22-B residuals (owner accepted 2026-11-27)

- [ ] **Registration-window recheck race** — when a document's watch is registered while its opening read's reload
  is queued, the ticker's post-subscription recheck and the earlier recheck can commit out of order: one reads an
  intermediate version and the other the final pre-registration write, and if the intermediate one commits later it
  advances the generation and drops the final answer; no event remains to correct a clean tab. Fix by giving both
  rechecks one generation so only the newest read commits.
- [ ] **Symlink target inside an ignored directory** — `link.md` resolving into `.venv/target.md` (or another
  `SKIP_DIRS` directory) inside the workspace: the recursive root covers the target's parent, so no separate document
  watch is added, and the root's event for the target is discarded as noise. Editing the target never reloads the
  clean tab or closes a stale Compare. Fix by registering the resolved target's parent as an exact context path when
  it sits under an ignored directory.
- [ ] **Directory rename holding a symlink's target** — a `Removed` event naming only the renamed parent of a link's
  target (`drafts/`) does not match the link tab, which keeps its old content; moving the directory back reports only
  a directory `Created`, which re-checks no child tabs, so the tab stays `Outside folder`. Fix by matching tabs whose
  path or resolved target is a descendant of a directory event, and re-checking them on a directory creation.
- [ ] **macOS FSEvents rename handling on the UI tick** — registration and `canonicalize` run on the worker, but the
  UI ticker still calls `Watcher::poll`. notify 8.2.2's FSEvents backend reports renames as `RenameMode::Any`, whose
  classification calls `path.exists()` synchronously; a queued event on a dropped network volume can still freeze the
  window. Fix by moving the existence probe off the UI thread.
- [ ] **Windows case-only directory spellings and reparse aliases still split one file's event spellings** — 8.3
  short names are closed: an open expands a short spelling to the file system's long one. `GetLongPathNameW` keeps
  the caller's case for a directory component, so a case-only spelling of a directory still names the same file with
  a different spelling, and it resolves no junction or directory symbolic link, so an open through an alias keeps
  the alias's spelling too. The mismatched event never marks the tab missing, and a symlink tab's `canonicalize`d
  target under the real path reloads no clean tab and raises no conflict on a dirty one. Fix by identifying a file by
  its file ID (volume serial number plus file index) at event and open matching instead of its path spelling.
- [ ] **A directory alias shares an inotify descriptor with the real path** — on Linux, a tab opened through `alias/plan.md`
  where `alias` (outside the workspace) points at `A/notes`, then `A` opened as the workspace: both registrations share one
  inotify descriptor, and closing the last `alias` tab unwatches it, so `A/notes` loses live updates. Fix by keying watch
  registrations by the resolved directory and reference-counting them.
- [ ] **A failed recursive registration restores the old watch without a recheck** — if registering the new root fails, the
  rollback re-registers the old document-directory watch with an unchanged generation, so a write made in the gap is never
  read. Fix by advancing the generation on rollback so affected tabs re-check.
- [ ] **A renamed symbolic link focuses its stale tab** — after `a.md`, a link to `real.md`, is renamed to `b.md`,
  opening `b.md` focuses the old tab by its resolved target and discards the new read. No text is lost; Save on the
  stale tab fails visibly. The fix needs a recovery key per tab instead of per path, so two tabs on one path cannot
  share one record.
