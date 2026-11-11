# Development and validation

This is the validation cadence for AGENTS.md and the ordered goals. It changes
when evidence is collected, not PRODUCT.md's safety, usefulness, visual or
release acceptance criteria. Historical reports are evidence for their named
revision, not commands to rerun every time. CLAUDE.md links to AGENTS.md.

## Short development loop

1. Identify the behavior, relevant goal and smallest affected boundary. A small
   fix needs a brief plan in the PR, not another goal or design document.
2. Implement and run focused tests. Keep state transitions, source identity,
   parsing, patch validation and request construction independent of native UI
   where practical. Use deterministic provider fixtures without credentials.
   For Rust ownership moves, follow `.agents/skills/rust-quality/SKILL.md` to
   migrate callers and preserve failure semantics before running the gate.
3. Review the diff once for unnecessary abstraction and duplicated validation.
   Remove machinery that provides no distinct correctness or product benefit.
4. Run the applicable tier once on the settled change; let PR CI provide the
   cross-platform gate. Report remaining acceptance separately, then move on.

| Change | Local default | Native verification |
| --- | --- | --- |
| Documentation or Python tooling | `check fast` | None unless native harness behavior changed |
| Native acceptance harness code | `check native` | Harness unit tests only; acceptance stays milestone-bound |
| `mt-core` logic | `check doc` | None for headless domain behavior alone |
| App logic, Rust dependency or build configuration | Focused tests, then `check ci` | Only affected native boundaries |
| Layout, strings or ordinary controls | Relevant logic/i18n tests; inspect the changed state | Focused manual smoke when available; full Goal 09 matrix at the visual milestone |
| Save/recovery, focus/IME, clipboard, WebView, consent or credentials | Deterministic regressions plus `check ci` | Affected Windows workflows before accepting that capability |
| Integrated milestone/public-quality release | `check full` | Applicable native suites, clean-machine and product evidence on the final artifact |

`check fast` does not validate Rust. `check doc` does not validate `mt-app`.
`check native` validates only native acceptance harness code; it does not run a
desktop or an executable. Neither `check ci` nor `check full` launches a GUI.
Cross-platform CI compiles and tests the nsh app on Windows, macOS and Linux;
the separate macOS arm64 PR job checks the arm64 slice and runs the headless
suite, not native acceptance or an installable `.app`. Windows 11 x64 remains
the public-quality target.

The verification layering, invariant ownership and rules for adding tests live
in [validation](verification.md). In short: current product/tooling contracts run
in parallel shards in `check fast`; native harness unit tests moved to
`check native`; large-document timing probes are opt-in and are not a PR gate.

Focused examples (replace the filter with the behavior being changed):

```sh
cargo test --locked --profile ci -p mt-core review
cargo test --locked --profile ci -p mt-core --features model-transport review::provider
cargo test --locked --profile ci -p mt-app review
uv run --locked --project scripts python -m unittest scripts.tests.test_checks
```

The `ci` profile inherits release optimization and assertion behavior but turns
off LTO and uses 16 codegen units. This avoids paying distribution-size link
costs on every PR. It is not an artifact-size or runtime-performance baseline.
`check full` runs production release validation directly, without first
rebuilding a second test profile.

The default performance integration test keeps fast 10K-line guards and
serializes its timing cases. The 100K-line parse, superlinear-overhead and
background-parse-duration probes are `#[ignore]`d; run them explicitly with
`cargo test --profile release -p mt-core --test performance -- --ignored`.
Wall-clock thresholds are measurements for a release/milestone checkpoint, not
per-PR correctness gates.
The release privacy scan takes its executable path from Cargo's build artifact
messages, including custom target directories and target triples; it never falls
back to a potentially stale `target/release/markturbo` binary.

## Keep the test boundary honest

Prefer these layers in order: pure domain/state tests, `nsh_testing::Headless`
interaction tests, then real Windows automation. A headless test can dispatch
keyboard/pointer events and check component state/focus/layout; it cannot prove
Windows IME composition, the OS clipboard, native dialogs, WebView2 focus or
DPAPI/session behavior. Keep native coverage for those boundaries.

`nsh-engine` and `nsh-testing` are dev-dependencies. A headless test starts the
real app (`Headless::start`), finds controls by `testID`, text or accessibility
label, presses/types/scrolls, and reads the text and state on screen. The shell
regression is `crates/mt-app/tests/shell.rs`; for example:

```sh
cargo test --locked -p mt-app --test shell
```

It opens a real Markdown fixture through `MARKTURBO_OPEN`, waits for the source
editor and native preview, types into the editor, saves, and checks the file.
First-use interaction tests and Welcome's recent-target behavior remain to be
ported; the old GPUI Kit fixtures are not part of the nsh app.
custom identified elements can use `TestSupportExt::test_support()` (inert in
production). Prefer IDs over coordinates and import test types explicitly to
avoid glob-importing Kit's `test` macro over Rust's built-in `#[test]`.

`scripts/mt.py check fast` runs the explicit current-tooling manifest as
deterministically partitioned subprocess shards. It includes CLI/privacy,
evaluation, performance-fixture and recovery-capacity contracts. Native
acceptance harness unit tests (including the retired GPUI selectors) moved to
`scripts/mt.py check native`; run that only when changing the harness. Neither
tier launches the app or a Windows UIA session. Real desktop automation runs
through `scripts/mt.py accept goal-02`, `goal-03`, `goal-06`, or `goal-07` with a
final executable hash and full required case set; `--case` is a diagnostic run,
not acceptance.

These helpers test events, state, focus and layout, not rendered pixels. Do not
add screenshot/foreground automation for ordinary state changes, or replace
all native/source assertions in one sweep. Keep integrated native acceptance
for the OS boundaries above and visual review for actual appearance changes.

Consult the test result for the relevant revision and environment. A failed
lint/test/build is not environmental merely because a GUI was unavailable.
After a repair, rerun the failed test plus its affected suite; avoid rebuilding
unrelated profiles or redoing manual acceptance without a changed dependency.

## Implementation and acceptance are separate

Use two fields in the existing PR or goal report, not a new tracking system:

- **Implementation:** in progress / implemented; link code and automated checks.
- **Acceptance:** passed / pending / failed; name the exact workflow, remaining
  evidence, reason and next eligible checkpoint (normally the integrated
  milestone or Goal 10). Include the build hash for native evidence.

Missing Windows desktop access, an unconfigured test endpoint, a pending
usability session or uncollected owner scoring is pending evidence. It does not
prevent independent work or merging an implementation with passing required CI
and explicit limitations. It does prevent claiming that goal fully accepted or
shipping the capability as public-quality. A known behavioral failure is
**failed**, not an environmental skip, and blocks delivery that depends on it.

Proceed to a later goal only where its required implementation contracts are
established. For example, Goal 07's deterministic diff/undo work may proceed
against validated Review fixtures while Goal 06 owner scoring is pending; do
not claim integrated acceptance until that scoring passes. If scoring actually
fails, fix the Review problem before accepting dependent behavior. Unknown or
changing product semantics still require a decision.

Do not infer acceptance from `docs/goals/archive/`. In particular,
`docs/goal-03-completion.md` records both a `WELCOME_DID_NOT_CLOSE` failure and a
later foreground-access block, with no final-hash PASS. Investigate the behavior
failure at the next first-use checkpoint; do not relabel it as only an
environment issue or let the folder location erase it. Historical evidence must
not be edited into a passing result.

## Keep native work bounded

- Prefer the existing harness. Its `--case` option is useful for debugging a
  changed workflow, but intentionally cannot produce full acceptance PASS.
- For ordinary UI changes, a short manual check of the changed state is enough
  for development feedback. Record what was actually observed; it does not
  replace automated safety invariants or full milestone evidence.
- Run full native suites once per integrated candidate, not per small task.
  Recheck only affected behavior after a fix while iterating; final acceptance
  remains bound to the final binary hash, never an earlier artifact.
- On a foreground/session/environment block, make at most one retry after a
  concrete corrective action. With no eligible desktop, record the command and
  reason and continue independent work. Do not loop on focus tricks, reboot the
  user's machine or redesign the harness just to finish a feature task.
- A failed behavior assertion needs diagnosis, not a retry-until-green loop.
  Do not mute the failure, weaken thresholds or change PASS/FAIL/BLOCKED codes.
- Add native automation only for a recurrent high-risk defect or release
  workflow that cannot be checked reliably below the GUI. Do not create a new
  per-goal harness, screenshot framework, evidence schema or benchmark apparatus
  by default. Reuse the shared native runtime and content-free evidence rules.

Goal 09 owns the broad visual/usability matrix. Goal 10 owns final clean-machine
integration and public-quality release evidence. Goal 11 owns recurring value;
these are different questions from whether an implementation PR can merge.

## Dependency maintenance

Prefer a small, reviewed compatible update over a workspace-wide `cargo update`.
Keep Cargo.lock and scripts/uv.lock in the PR and use locked installs in CI.
Update the nsh/runtime/WebView group coherently; keep the nsh crates on the
CLI's version, preserve the platform features (including Linux `webview`) and
the RaTeX allocation clamp. Inspect upstream changes before major
updates, then test affected behavior. Ordinary tooling updates do not require
GUI acceptance. Native stack upgrades require targeted Windows smoke at the
integration checkpoint. See [the dependency review](dependency-review.md).
