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
| Native acceptance | `mt.py accept <goal> -- ...` | active Windows desktop, final executable hash | milestone |

`check native` is not acceptance. Native acceptance remains a separate,
hash-bound milestone action and may be BLOCKED by the desktop/session.

### Per-PR pipeline

`.github/workflows/pull-request.yml` classifies the diff before it spends runner
time: a changed path is documentation only when it is under `docs/**` or is a
Markdown file at the repository root, because `sample/` is embedded into the
executable and `fixtures/` is read by tests; everything else (`.github/**`,
`crates/**`, `scripts/**`, `installer/**`, `sample/**`, `fixtures/**`, the Cargo
manifests) is code. Within code changes it also marks whether anything can
change what a Rust build produces (`crates/**`, `vendor/**`, `sample/**`, a
Cargo manifest or lock, `rust-toolchain*`); the app suites and the cross-platform
builds require that second flag, so script-, installer- and workflow-only
changes keep the cheap Linux checks and leave the expensive combinations to the
nightly workflow. An unresolvable range falls back to running everything. The
workflow also listens for `ready_for_review`, so marking a pull request ready
runs the Rust checks that its draft pushes skipped.

| Job | Trigger | Command |
| --- | --- | --- |
| Classify changed paths | every pull request | `git fetch --no-tags --depth=1 origin "$BASE_SHA"` then name-only base..head classification |
| preflight (fast + format) | every pull request | `check fast` over the same base..head range, then `cargo fmt --all -- --check` |
| clippy (Linux) | code changes, not a draft | `cargo clippy --profile ci --workspace --all-targets --locked` |
| mt-core tests (Linux) | code changes, not a draft | `cargo test --profile ci -p mt-core --features model-transport,test-support --locked` |
| mt-app tests (Linux) | Rust changes, not a draft | `cargo test --profile ci -p mt-app --locked` |
| macOS arm64 compatibility | Rust changes, not a draft | `cargo build --release --locked -p mt-app --bin markturbo`, `cargo test --profile ci -p mt-core --locked`, `lipo -archs` |
| release-tests (windows-2022) | Rust changes, not a draft | `cargo test --locked --profile ci -p mt-app -p mt-core` |

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

Each platform owns one tier: Ubuntu the authoritative Rust gate, Windows the
app/UI headless suite on the public-quality platform, and macOS the arm64
release build plus headless `mt-core` behavior under default features. The
combinations that used to repeat on every platform run once a day in
`.github/workflows/nightly.yml` (`check ci` on `ubuntu-24.04`,
`cargo test --locked --profile ci --workspace` and
`cargo test --release --locked -p mt-app` on `macos-15` and `windows-2022`);
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

The nsh app has stable headless test IDs but not yet a complete native
automation ID contract. The historical goal-03/06/07 harnesses target retired
GPUI IDs and source files; their unit tests are in `check native`, not the fast
gate. Rewriting the selectors and cases is a bounded follow-up before native
acceptance can pass on nsh; the absence of that evidence is recorded as pending,
not converted into a passing result.

Goal 09's release-build screenshot matrix is produced by:

```sh
uv run --locked --project scripts scripts/mt.py visual -- \
    --exe crates/mt-app/.nsh/out/MarkTurbo/MarkTurbo.exe \
    --out target/native-acceptance/goal-09
```

`--out` is a scratch directory: the capture overwrites the cells it writes and
has no existing-output guard, so it never points at the retained matrix under
`docs/evidence/`.

It records each cell's source revision, executable hash, DPI, DIP and pixel
viewport, language, effective theme, obstruction observations, control names and
roles, PNG hash and status. All eleven states are exercised end to end through
the real packaged app at minimum/default/wide widths in both modes and
languages.

WebView2 renders through a DirectComposition visual that `PrintWindow` omits.
For the compatibility-preview state the harness starts the app with
`WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS=--remote-debugging-port=<free port>` and
captures the page through CDP `Page.captureScreenshot`, then composites it into
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
