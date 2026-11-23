# Verification

This is the top-down map of what each verification layer proves, which oracle
owns each invariant, and when to pay for it. `docs/development.md` owns the
cadence; PRODUCT.md and the goals own acceptance.

## One invariant, one cheapest oracle

Test the invariant at the lowest layer that can falsify it, then stop. A higher
layer repeats only the wiring or OS behavior the lower layer cannot observe.

| Invariant | Primary oracle | Layer |
| --- | --- | --- |
| Lossless bytes: newline, BOM, encoding, save guards, conflict refusal | `crates/mt-core/src/document/io.rs` unit tests; `crates/mt-app/tests/conflict.rs` for the banner/choices | domain + headless |
| Recovery survives a crash without touching the source | `mt-core` recovery/retention tests; `mt-app/tests/shell.rs` for checkpoint -> new process scan -> restore -> save | domain + headless |
| Review request validation, consent binding, effective-context selection | `mt-core` provider/context/revision contract tests | domain |
| Review -> Revision consent, approval and apply | `crates/mt-app/tests/revision_flow.rs` (loopback provider, real UI path) | headless e2e |
| Workspace open, editor, save, first-use shell | `crates/mt-app/tests/shell.rs` | headless e2e |
| Press-down feedback, editor focus handback and surface motion | Goal 16 UI headless tests; [motion acceptance record](goal-16-acceptance.md) for native evidence | headless + native |
| Web trust, CSP, script/network exclusion | `crates/mt-app/src/web.rs` unit tests | domain |
| Settings, recent targets, credentials and endpoint authorization | `mt-core` settings/credentials tests; tooling tests for CLI evidence | domain + tooling |
| Native window, UIA, SendInput, WM_CLOSE, WebView2, DPAPI session | Windows acceptance harness on the final hash | native (milestone) |
| Large-document performance | `crates/mt-core/tests/performance.rs` opt-in probes | measurement |

No behavior should be asserted twice at different layers unless the higher layer
tests a distinct seam: for example, `revision_flow` tests consent and apply
wiring, while `mt-core` tests proposal validation and consent semantics.

## Layers and commands

| Layer | Command | Scope | Runs per change |
| --- | --- | --- | --- |
| Static | `cargo fmt --check`, `cargo clippy` | compile/lint | settled Rust change |
| Tooling contracts | `mt.py check fast` | current Python validation tooling in deterministic shards | docs/tooling |
| Headless core | `mt.py check doc` | fast + optimized `mt-core` tests | core logic |
| Workspace | `mt.py check ci` | fast + fmt/clippy + workspace tests, including headless app tests | settled product change; Ubuntu per PR |
| Native-harness unit tests | `mt.py check native` | native acceptance tooling only; no desktop or executable | native harness change |
| Release | `mt.py check full` | release profile + build + privacy scan | release candidate |
| Native acceptance | `mt.py accept all ...` (one goal: `mt.py accept <goal> -- ...` for diagnosis) | active Windows desktop, final executable hash | milestone |

`check native` is not acceptance. Native acceptance remains a separate,
hash-bound milestone action and may be BLOCKED by the desktop/session.

### Per-PR pipeline

`.github/workflows/pull-request.yml` classifies the diff before it spends runner
time: a changed path is documentation only when it is under `docs/**` or is a
Markdown file at the repository root, because `sample/` is embedded into the
executable and `fixtures/` is read by tests; everything else (`.github/**`,
`crates/**`, `scripts/**`, `installer/**`, `sample/**`, `fixtures/**`, the Cargo
manifests) is code. Within code changes it also marks whether anything can
change what a Rust build produces (`crates/**`, `vendor/**`, `sample/**`,
`.github/workflows/**`, a Cargo manifest or lock, `rust-toolchain*`); the app
suites and the cross-platform builds require that second flag, so changes to
scripts, the installer or other workflow files keep the cheap Linux checks and
leave the expensive combinations to the nightly workflow. A workflow change
counts as Rust work on purpose: it alters how the suite runs, so it must exercise
the pipeline it changes. An unresolvable range falls back to running everything.
The workflow also listens for `ready_for_review`, so marking a pull request ready
runs the Rust checks that its draft pushes skipped.

| Job | Trigger | Command |
| --- | --- | --- |
| Classify changed paths | every pull request | `git fetch --no-tags --depth=1 origin "$BASE_SHA"` then name-only base..head classification |
| preflight (fast + format) | every pull request | `check fast` over the same base..head range, then `cargo fmt --all -- --check` |
| clippy (Linux) | code changes, not a draft | `cargo clippy --profile ci --workspace --all-targets --locked` |
| mt-core tests (Linux) | code changes, not a draft | `cargo test --profile ci -p mt-core --features model-transport,test-support --locked` |
| mt-app tests (Linux) | Rust changes, not a draft | `cargo nextest run --locked --cargo-profile ci -p mt-app --lib --bins --test suite` |
| macOS arm64 compatibility | Rust changes, not a draft | `cargo build --release --locked -p mt-app --bin markturbo`, then `lipo -archs target/release/markturbo` must list arm64 |
| release-tests (windows-2022) | Rust changes, not a draft | `cargo nextest run --locked --cargo-profile ci -p mt-core --features model-transport,test-support` and `cargo nextest run --locked --cargo-profile ci -p mt-app --lib --bins --test suite` |

The app suites run under `cargo nextest`, which gives every test its own
process and runs them in parallel; the environment-variable tests need that
isolation. nextest does not run
doctests; the workspace has none today (the CI log shows no `Doc-tests` harness
and `cargo test --doc --workspace` compiles a separate unit set for zero tests,
measured 11m33s), so the nightly `check ci` run keeps the plain `cargo test` path
as the backstop for any doctest added later instead of paying that compile in
every pull request. The `mt-core` shard keeps plain `cargo test`, so mt-core
doctests would still run per pull request if any were added.

The three Linux Rust jobs are the pieces of `check ci`, split so the wall clock
is the slowest shard instead of the sum: `check fast` and formatting in
`preflight`, clippy, then the workspace suite as `mt-core` plus `mt-app`. The
`mt-core` shard names `model-transport,test-support` because `--workspace`
resolves that feature set for it through mt-app; a bare `-p mt-core` would skip
its model-transport tests and its `required-features` evaluation binary. Every
heavier job needs `preflight`, so a whitespace or formatting error fails in
about two minutes instead of after the workspace has compiled. A
documentation-only pull request pays only for the classifier and `preflight`,
and a draft pull request stops there too; the skipped jobs still report success,
so a required-check list keeps working.

The default MSVC linker stays: pointing `CARGO_TARGET_*_LINKER` at `rust-lld`
invalidated the build fingerprint and produced no comparable measurement.

Each platform owns one tier: Ubuntu the authoritative Rust gate, Windows the
app/UI headless suite on the public-quality platform, and macOS the arm64 release
artifact. macOS behavior runs in the nightly instead of per pull request: a
per-pull-request `cargo test -p mt-core` on that runner failed eight
`document::io::tests::skill_save_*` concurrent-save tests in both feature
configurations (runs 43630038928 and 43631880761) while the nightly-form
`cargo test --workspace` run passed the same tests (job 132527749238 printed
`... ok`). That difference is recorded in `docs/TODO.md` as a question about the
tests and the shared commit path, not as a platform flake, and the per-pull-request
job keeps only the evidence its name claims. The
combinations that used to repeat on every platform run once a day in
`.github/workflows/nightly.yml` (`check ci` on `ubuntu-24.04`,
`cargo test --locked --profile ci --workspace` and
`cargo nextest run --locked --cargo-profile release -p mt-app --lib --test
suite` on `macos-15` and `windows-2022`);
the scheduled run is a non-blocking safety net and never gates a merge. It is
also the only writer of the rust-cache entries the pull-request jobs restore, so
per-PR jobs restore from the default branch instead of each paying a cache save.

## Rules for adding verification

1. **Behavior before source text.** Assert outputs, state transitions, errors,
   file bytes, or consent/disclosure values. Source scans are reserved for
   architecture/secrecy boundaries that cannot be observed behaviorally.
2. **Deterministic first.** Prefer fixed fixtures, pure functions, bounded
   state spaces, and fixed-seed randomized cases. Avoid order dependence and
   shared mutable state.
3. **Wall-clock is not a PR gate.** Timing thresholds are environment-sensitive
   and belong to explicit performance probes at a release/milestone checkpoint.
   The default workspace suite excludes the 100K-line probes; run them with:
   ```sh
   cargo test --profile release -p mt-core --test performance -- --ignored --nocapture
   ```
   Modifying the `ci` profile or the thresholds requires a measured release
   comparison, not a test-only change.
4. **Process-isolated tooling tests.** `mt.py check fast` discovers the explicit
   manifest, partitions the flattened test IDs deterministically, and runs
   independent shards in parallel. A shard owns its process and temporary paths;
   a test must not depend on another shard or on global repository mutation.
5. **Native only for native seams.** Do not duplicate domain/UI assertions in
   the native harness. Add native cases only for focus, input, window lifecycle,
   OS dialogs, WebView2, or credential/session behavior that lower layers cannot
   observe.
6. **One failing invariant is enough.** Do not add a second test with the same
   oracle and same failure mode to raise a count.

Retained acceptance, ablation, and measurement evidence is indexed in
[`evidence/`](evidence/README.md).

## Native harness status

The goal-02/03/06/07 harnesses (`scripts/markturbo_tools/native/`) are ported
to the nsh UI and synchronized with the merged Afterglow behavior below. They
have not yet run live against an nsh build: no nsh acceptance evidence exists,
and that absence stays pending, not a pass. Their unit tests run in
`check native`, not the fast gate. Pass the release binary with
`--exe target/release/markturbo.exe`.

How nsh 0.6.1 exposes views to UIA, and so how the harnesses select them:

- A view's `test_id` is the UIA AutomationId, but only when the view is itself
  an element. An explicit role sets the control type; without one, text,
  inputs, pressable or labelled views, scroll panes and views explicitly
  marked accessible can still be elements. Plain wrappers are flattened.
  The `prims` quiet/primary actions and the guard card's buttons are the
  pressable views themselves, so each is found by
  its own AutomationId (`guard-save`, `welcome-new-file`, `revision-accept`,
  ...). Rows that share an id (`welcome-recent`, `workspace-home-file`) are
  enumerated by it and told apart by their Name.
- Title-menu rows (`title-menu-*`) are reached by invoking `title-settings`
  first, so a command can move between the title row and that menu without a
  selector change. Save, Save As, Quit, Review selection and the Edit/Web
  modes go through it.
- The editor's accessible element is the `Markdown source` IME input; its UIA
  Value is not the document. The harness reads the document with Ctrl+A, Ctrl+C
  sent as one input batch, waits for the clipboard sequence number to advance
  (resending Ctrl+C once after 300 ms), then reads the text. Another writer
  can also advance the sequence, so before awaiting a text the harness
  empties a clipboard that already holds it; otherwise a read never writes
  the clipboard. The run saves the user's text clipboard once and
  restores it once at its end. Selection and Review use the editor's own keys
  (Ctrl+A, Ctrl+Enter).
- UIA Invoke is one tap. The consent card's `consent-send` sends only after a
  held press, so the harnesses hold the real pointer past the control's 1.2 s
  threshold; an Invoke is never consent.
- Ctrl+Enter depends on state: it starts Review from the title row, sends from
  the consent card, and is Done on the Revision field. The harnesses send it
  only in the state whose action they mean.
- nsh system alerts (`alert_dialog()`) are not in the UIA tree: they are drawn
  inside the window as hosted items, which have no accessibility hook. This
  covers the Overwrite confirmation after Keep mine (goal 02). While an alert shows it
  takes every key. Enter presses its first button, or the last regular
  `.action` when it has one. The harnesses therefore check that the press alone changed
  nothing, then send Enter from the main window's keyboard focus, and prove the
  decision by its effect: the file holds the edited bytes, so an Enter that
  cancels fails the case. Test-id'd,
  in-window confirmation cards would let a harness select the button directly.

The harnesses do not scan Rust source; a renamed selector fails the live run
with that control's UIA timeout or contract-mismatch code.

What each goal runs (`mt.py accept all` runs all four). Every phase of a
merged session has its own observation-key prefix and failure-code prefix
(`runtime.phase`), so a FAIL names the phase it happened in. A goal restarts
the app only where the behaviour itself needs a restart.

- Goal 06, default mode: one case `fresh_endpoint_not_set_up`, one launch.
  Phase `SHORTCUT` (Ctrl+Enter shows `No model set up`); the source file's
  hash is unchanged and a clean WM_CLOSE with no guard proves the document
  stayed clean. Headless: its Close removing the card and the title menu's
  `Review selection` row showing it again, with the document still clean
  (`state.rs` `review_without_a_model_closes_and_the_menu_row_shows_it_again`).
  `--configured-provider` (opt-in, `accept goal-06` only) runs
  `configured_provider_session` against a real provider in one launch:
  `DOCUMENT` (Ctrl+Enter, consent hold, result, Close) then `SELECTION`
  (Select All, the menu row; the consent card shows the `Selection` scope);
  each keeps the source file's hash. Localized findings, clarification
  questions and the keyless Review variants are headless mt-core/mt-app tests.
- Goal 02, one case `lifecycle-session`, two launches (the interruption is
  the only restart). First launch: `EXTERNAL_MODIFICATION` (a same-length
  rewrite with the loaded mtime is refused, then Keep mine and the Overwrite
  alert write the exact bytes), `TAB_CLOSE` (the rail close raises the guard;
  Cancel keeps the file; the following Save writes the exact edited bytes) and
  `CHECKPOINT` (an encrypted canonical record within 10 000 ms, no sentinel
  leak, then TerminateProcess). Relaunch on the workspace folder: `RESTORE`
  (Restore brings back the exact checkpointed bytes), `KEYBOARD_CLOSE`
  (Ctrl+W, Don't save closes the tab without writing and retires the record)
  and `WINDOW_CLOSE` (the workspace home's row reopens the file; Alt+F4 and
  WM_CLOSE raise the guard; Cancel keeps the file; Save writes the exact bytes
  and the process exits 0; no recovery record survives). The editor is not
  read after a refusal or a Cancel: the exact bytes of the next write prove
  the buffer survived. Launch failures carry `FIRST_LAUNCH_` or
  `RESTORE_LAUNCH_`. Headless: `tests/conflict.rs` (the refused save leaves
  the edit in the editor and nothing is written before Overwrite),
  `tests/tab_close_guard.rs` (Cancel keeps the dirty mark, which nsh 0.6.1's
  UIA folds into the `title-file` Button), `tests/recovery_restore.rs` (a
  discarded record is not offered on the next launch),
  `tests/save_as_overwrite.rs` and `ui/revision.rs` `SaveConflictProbe`.
- Goal 07, one case `revision_session`, one launch on `security-fixture.html` from
  argv, one Review→Revision chain (two loopback provider requests) over a
  two-hunk HTML fixture. `WEB_SECURITY`: the inline script changes a visible
  marker without a prompt and attempts popup, top-level and frame navigation
  and form submission. The first screenshot shows the attempt cells and the
  pre-navigation cell; the second detects no fixture cells, consistent with a
  browser block page. Exactly one app window remains, and the counted
  loopback listeners record no contacts, including on the final HTTPS port.
  Browser argument-string tests verify configuration only.
  Edit mode then hands the keyboard back. `DECISIONS`:
  Accept(h1), the executable script value after the CJK/emoji line, writes at
  once across the multibyte offsets; Reject(h2) reads `2 of 2 decided`;
  `Undo` (`revision-undo`) takes it back to `1 of 2 decided`; `Copy`
  (`revision-copy`) puts exactly the approved text on the OS clipboard; an
  edit over the whole document shows `Can't apply — this part changed` with
  no Accept; `Done` leaves the file on disk unchanged. `QUIT`: one
  guard, Discard, the process exits. A consent hold whose request never reaches the loopback
  fails at once (`REVIEW_REQUEST_NOT_SENT` / `REVISION_REQUEST_NOT_SENT`).
  Headless: Reject and Undo restoring exact bytes after a write, reject-all
  and every hunk accepted (`tests/revision_flow.rs`); an edit over every hunk
  offering no Accept and Done writing nothing
  (`an_edit_over_every_hunk_offers_no_accept_and_done_writes_nothing`); the
  dirty mark, folded away by nsh 0.6.1's UIA (`tests/revision_flow.rs`); the
  A, R and Ctrl+Z keys and A refused on a changed part (`ui/revision.rs`); a
  safe save over a file changed on disk (`ui/revision.rs`); frontmatter,
  fence and link anchoring (mt-core).
- Goal 03, one case `first_use_lifecycle`, three launches (clean profile;
  cold reopen through argv; restart onto the recent list). Phases: `WELCOME`
  (the four entry points, New file among them, each one enabled Button, and
  no recent row, in one UIA query on a clean profile), `PASTE`
  (CF_UNICODETEXT with CJK and a surrogate-pair emoji), `OVERWRITE_NO` (the
  native dialog's No, then the picker's Escape: the cancel proof; focus and
  the destination unchanged), `OVERWRITE_YES` (Yes writes the pasted bytes,
  then `title-file` names the new file), `SUBSEQUENT_SAVE` (a typed edit
  saved to the new path), `SAVED_CLOSE` (one recent row), `SAMPLE`
  (workspace home), `REOPEN` (argv: exact CJK/emoji text, no welcome, the
  10-entry bound) and `RECENTS` (five rows; the stale one disabled and named
  `File <name> — Missing`; a real press on it, then a real click on the saved
  row, leaves exactly one open tab, the saved file). Headless: New file
  saving through Save As (`tests/untitled_save.rs`); item 5, Save after Save
  As writing only the new path (`tests/save_as_overwrite.rs`); item 7, an
  untitled buffer going through the close decision and leaving no recent
  entry (`tests/untitled_discard.rs`), with Ctrl+W, the guard and Don't save
  proven natively by Goal 02's `KEYBOARD_CLOSE`; a directory argument
  (`tests/welcome_leaves.rs`) and LRU eviction order
  (`tests/welcome_recents.rs`, mt-core settings tests). Launch failures
  carry `FIRST_LAUNCH_`, `REOPEN_LAUNCH_` or `RECENTS_LAUNCH_`.

All four harnesses write one evidence schema (`native/evidence.py`,
`schema_version` 4) to `target/native-acceptance/goal-NN-native-acceptance.json`
by default; `revision_evaluation.py` binds only that version. Every earlier
file, including the GPUI-era records under `docs/evidence/`, is rejected.

The former known gaps are fixed in the merged UI and are now expected by the
cases above: the selection Review command, the Revision Copy and the labelled
proposal group, the WM_CLOSE/Alt+F4 guard, trust revocation before every
Revision write, Save As replacing a confirmed existing destination, the marked
stale recent row, and the workspace home replacing the welcome. Whether the
packaged app passes them is unknown until the first live run on an active,
unlocked Windows 11 desktop.

## Running native acceptance

One command runs every goal on one executable:

```sh
uv run --locked --project scripts scripts/mt.py accept all \
    --exe target/release/markturbo.exe \
    --expect-exe-sha256 <sha256> --evidence-dir target/native-acceptance \
    [--keep-going] [--ui-timeout 15]
```

It runs goal-06, goal-02, goal-07 and goal-03 in that order (cheapest and most
stable first, so a late BLOCKED loses the least evidence) in one process. The
executable is hashed and the environment checked once, the executable copied
once, and the user's clipboard saved once; every goal still gets its own
isolated temp root and writes its own `goal-0N-native-acceptance.json` with the
same hash binding. Exit status: `0` PASS (every goal passed), `1` FAIL (any
goal failed, or the clipboard could not be restored), `2` BLOCKED (the
environment stopped the run and nothing failed). FAIL outranks BLOCKED. A goal
BLOCKED stops the later goals, and a goal FAIL does too unless `--keep-going`;
a goal whose cleanup failed (its processes or workdir may remain) stops them
even with `--keep-going`. A stopped goal still writes its evidence, every case
marked `SKIPPED_AFTER_BLOCKED`, `SKIPPED_AFTER_FAILURE` or
`SKIPPED_AFTER_CLEANUP_FAILURE`. With `--keep-going` a case
FAIL also lets the goal's later cases run; the first failure is the reported
code and the goal stays FAIL. BLOCKED and a cleanup failure always stop.
`mt.py accept goal-0N -- ...` (with `--case` or `--keep-going`) remains for
diagnosing one goal; a single-case run is never acceptance.

Before the run, on the Windows 11 x64 desktop that will run it:

- Use the console session or an RDP window. A disconnected or locked session
  reports `SESSION_NOT_ACTIVE` or an `INPUT_DESKTOP_*` code instead of
  `FOREGROUND_PERMISSION_DENIED`. On the Remote Desktop client machine, set
  the REG_DWORD value `RemoteDesktop_SuppressWhenMinimized` = 2 under
  `HKCU\Software\Microsoft\Terminal Server Client` (and, for system-wide
  use, `HKLM\SOFTWARE\Microsoft\Terminal Server Client` and
  `HKLM\SOFTWARE\WOW6432Node\Microsoft\Terminal Server Client`). A
  minimized mstsc client then keeps the remote session rendering and
  accepting input. Without it the session has no foreground window
  (foreground hwnd 0); the harness then prompts once on the console and waits
  up to 30 s for the RDP window to be restored before it BLOCKs with
  `FOREGROUND_PERMISSION_DENIED`. Another window holding the foreground
  blocks at once.
- Turn off clipboard history (Win+V), RDP clipboard redirection,
  notifications and Focus Assist's banners; they take the clipboard or the
  foreground. The harness asks Windows to keep the display and system awake
  for the run and releases the request at its end.
- Leave the clipboard empty or holding plain text: a clipboard with other
  formats blocks as `CLIPBOARD_CONTAINS_NON_TEXT`. A clipboard held by another
  program past the ~3 s retry window blocks as `CLIPBOARD_UNAVAILABLE`, with
  the attempt count and whether a window held it. Do not use the clipboard
  during the run; a harness killed mid-run cannot restore it.

Timing rules the runtime enforces: the foreground has one ~3 s timeout; a
wait nested inside another (the copy inside an exact-bytes editor wait) is
bounded by the outer deadline and at most 2 s; an editor-text duration ends
when the matching copy was requested, not after the harness's clipboard
retries. Each goal's evidence `metrics` records every phase's wall time
(`phase_timings`) and the content-free counters `editor_reads`,
`copy_resends`, `foreground_recoveries` and `uia_queries`. Goal thresholds (the 10 000 ms recovery checkpoint, the consent hold,
the 15 s default UI timeout, exact bytes) are unchanged.

Goal 09's release-build screenshot matrix is produced by:

```sh
uv run --locked --project scripts scripts/mt.py visual -- \
    --exe target/release/markturbo.exe \
    --out target/native-acceptance/goal-09
```

`--out` is a scratch directory: the capture overwrites the cells it writes and
has no existing-output guard, so it never points at the retained matrix under
`docs/evidence/`.

It records each cell's source revision, executable hash, DPI, DIP and pixel
viewport, language, effective theme, obstruction observations, control names and
roles, PNG hash and status. All eleven states are exercised end to end through
the real packaged app at minimum/default/wide widths in both modes and
languages. The cells drive the Afterglow UI with the native acceptance
harness's steps (`title-menu-*` rows, AutomationIds, a pointer hold on
`consent-send` against a loopback that refuses an unconsented Review) and prove
each state by its own controls before capturing; a cell that cannot reach its
state fails with a code prefixed by the state, such as
`SETTINGS_PAGE_MISSING`, and leaves a `failure.png`. A receipt keeps the
proving `state_markers` even when a later check (such as accessibility)
fails, and a Review receipt counts its `consent_hold_count`: the harness holds
once more only while the card is still open and nothing was sent.

WebView2 renders through a DirectComposition visual that `PrintWindow` omits.
For the compatibility-preview state the harness starts the app with
`WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS=--remote-debugging-port=<free port>` and
its own `LOCALAPPDATA` (nsh keeps the WebView2 profile there, and a browser
still serving a previous cell's profile ignores the new port), and captures the page through CDP `Page.captureScreenshot`, then composites it into
the UIA-reported pane rectangle over the PrintWindow chrome. It rejects a blank
page as `WEBVIEW_CONTENT_MISSING`; it does not record an empty pane as a pass.
The capture is release-build visual evidence, not owner approval. Headless
hierarchy tests remain implementation coverage, not visual acceptance.

Every cell also enumerates the actionable UIA controls and fails as
`ACCESSIBILITY_UNNAMED_CONTROL` when a compact button, field, link, or menu item
has no accessible name. Pane-sized pressable containers exposed by the text
engine and the WebView document are structural and are not counted. Markdown
task checkboxes are also excluded because the nsh text engine exposes them
beside, not with, their task text; the receipt counts them separately. This is
automated assertion coverage; keyboard traversal and a screen-reader smoke still
need a person on an active Windows 11 desktop.
