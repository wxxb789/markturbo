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
| Workspace | `mt.py check ci` | fast + fmt/clippy + workspace tests, including headless app tests | settled product change |
| Native-harness unit tests | `mt.py check native` | native acceptance tooling only; no desktop or executable | native harness change |
| Release | `mt.py check full` | release profile + build + privacy scan | release candidate |
| Native acceptance | `mt.py accept <goal> -- ...` | active Windows desktop, final executable hash | milestone |

`check native` is not acceptance. Native acceptance remains a separate,
hash-bound milestone action and may be BLOCKED by the desktop/session.

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

## Native harness status

The nsh app has stable headless test IDs but not yet a complete native
automation ID contract. The historical goal-03/06/07 harnesses target retired
GPUI IDs and source files; their unit tests are in `check native`, not the fast
gate. Rewriting the selectors and cases is a bounded follow-up before native
acceptance can pass on nsh; the absence of that evidence is recorded as pending,
not converted into a passing result.

Goal 09 also still needs its release-build screenshot matrix: the eleven states
at minimum/default/wide widths in both effective modes, with revision, viewport,
language and obstruction receipts. No `mt.py` command currently produces that
matrix; headless hierarchy tests are implementation coverage, not visual
acceptance.
