# MarkTurbo agent instructions

## Authority and scope

Read `PRODUCT.md`, the relevant ordered goal in `docs/goals/`,
`docs/architecture.md`, and `CONCEPTS.md` before changing product behavior.
Goals are canonical: link to them from PRs and commits rather than duplicating
their requirements. `docs/history/` is historical context, not current scope.
Update `CONCEPTS.md` only when project vocabulary changes.

Keep changes scoped. Preserve user work and generated evidence. Goal numbers
order product delivery, not every coding task. Follow `docs/development.md` for
validation cadence and pending acceptance; do not weaken product thresholds or
represent missing evidence as a pass. Read only the relevant code and guidance,
not all historical completion reports or every Skill reference.

## Platform and release status

Windows 11 x64 is the only public-quality target. CI tests Linux, macOS, and
Windows for compatibility, not as a release promise. CD publishes one
Windows `markturbo-windows-x64.exe` asset. Installers, signing, notarization, and
multi-platform distributables are future Goal 10 work.

Use the canonical tooling entry point:

```sh
uv run --project scripts scripts/mt.py check fast
uv run --project scripts scripts/mt.py check doc
uv run --project scripts scripts/mt.py check ci
uv run --project scripts scripts/mt.py check full
```

`fast` runs whitespace and non-desktop tooling tests. `doc` adds formatting and
optimized `mt-doc` tests without compiling the desktop app. `ci` adds formatting,
Clippy and optimized workspace tests without distribution LTO. `full` uses the
production release profile for Clippy, tests, build and binary privacy scan.
With `--base`/`--head` or `BASE_SHA`/`HEAD_SHA`, whitespace checks use that range.

## Validation and evidence

Choose validation from the changed behavior, not the goal number:

- Docs/tooling: `check fast` plus any focused tooling tests.
- Headless document logic: `check doc`; use a test-name filter while iterating.
- Application logic or dependencies: focused tests, then `check ci` once for
  the reviewable change. CI on that commit can supply the full workspace gate;
  do not duplicate a successful run locally just to open a PR.
- Native interaction: test the changed workflow when a suitable desktop is
  available. Do not run every old goal harness or the visual matrix for a small
  UI change. Batch full native acceptance at an integrated milestone/release.
- Release or performance claims: `check full` and the relevant native/probe
  evidence on the exact artifact. Never use the `ci` binary for those claims.

Do not require a clean worktree while iterating or to open a draft PR. Before
merge, required checks must cover the final proposed revision. Report commands,
actual results and anything not run; never invent a test count.

Separate **implementation status** from **native/product acceptance status**.
A missing desktop or owner evaluation does not block independent implementation
or an accurately scoped PR. Record the pending check in the PR or existing goal
report and continue. Known data-loss, privacy, trust or destructive-interaction
failures block affected delivery; absence of evidence never makes them safe.
A full goal acceptance claim still requires its stated evidence. See
`docs/development.md` before handling `BLOCKED` or a partially verified goal.

Prefer deterministic behavior tests at the lowest useful boundary. Add a
regression test for a real failure mode, not every small edit. Source-scanning
tests are structural guardrails, not proof of runtime GUI behavior; do not add
new spelling/layout assertions when a behavior test is practical. Preserve the
invariant when refactoring, rather than freezing implementation text.

Performance work uses `scripts/`; consult `.scratch/perf-and-size/`, repeat
measurements, and do not run concurrent release builds on the measured machine.
Native acceptance stays explicit (`mt.py accept goal-02|goal-03|goal-06 -- ...`).
It requires an active unlocked Windows desktop, a current hash-bound executable
and a PASS evidence file. `BLOCKED` and single-case runs are not acceptance.
Do not spend a feature task repairing unrelated GUI automation: record the
concrete limitation and follow the retry policy in `docs/development.md`.

## Structural and privacy invariants

- `mt-doc` has no GPUI dependency.
- Keep `panic = "unwind"` and the allocation clamp in `vendor/ratex-parser`.
- Never mutate a WebView from `render`; set `web_dirty` and defer the update.
- Content failures are diagnostics. Broken documents remain editable and source
  text remains preserved.
- Native evidence may record content-free timings, hashes, byte counts, status,
  and OS/session/integrity metadata needed to validate a run. Test text,
  filesystem paths, credentials, and user documents must not leak.
- The shipped executable includes the fonts and bundled sample it needs; do not
  reintroduce a sidecar release layout.

## Dependencies and delivery

Read workspace `Cargo.toml` comments before changing dependencies. Prefer
crates.io; `vendor/ratex-parser` is the sole local patch. Dependencies sharing
a git source must use the same source selector.

Use English in code, comments, docs, commits, and PRs. Make cohesive PRs, not
an arbitrary one-commit rule. Commit subjects name the user-visible impact and,
for product work, the goal file advanced. Run the formatter once before the
commit that contains its intended edits.
