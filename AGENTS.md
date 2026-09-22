# MarkTurbo agent instructions

## Load only the context the change needs

- Product behavior: read the relevant PRODUCT.md contract and goal in
  `docs/goals/`, then the affected architecture section. Read CONCEPTS.md when
  a term is unclear or changes; update it only when vocabulary changes.
- Tooling/CI/dependencies: start with the affected script, manifest and tests.
  Do not load the entire product roadmap, corpus or historical reports.
- UI: use the relevant checked-in GPUI Skill guidance, but verify APIs in the
  source selected by Cargo.lock. Live upstream docs may describe a different
  version. Project product/architecture decisions govern over generic library
  examples; no upstream guide requires rewriting unrelated application code.

PRODUCT.md owns product scope, goals own acceptance, `docs/development.md` owns
validation cadence, and source/tests establish current implementation. History
and `docs/goals/archive/` do not establish current acceptance. Preserve user work,
immutable evaluation snapshots and existing evidence. CLAUDE.md shares this file.

## Work to a bounded result

Identify the behavior, smallest useful change, affected invariant and verification
command before editing; a brief task/PR note is enough. Prefer an existing module
and public library capability over a new layer. Do not add a goal file, harness,
evidence schema, configuration option or abstraction for every small task.

Fix a regression at the lowest boundary that demonstrates it. Prefer behavior
assertions over source-text/spelling assertions. Source scans guard structure;
they do not prove GUI behavior. During final review, remove machinery that adds
no distinct value and preserve only changes needed by the task.

Continue independent implementation when a desktop, credential or owner session
is unavailable. Record pending acceptance in the PR or existing report. Known
safety failures block affected delivery; missing evidence is never a pass.
Do not weaken a goal threshold or claim completion because its file is archived.

## Validation

Use `uv run --locked --project scripts scripts/mt.py check <tier>`:

| Tier | Use |
| --- | --- |
| `fast` | Docs/tooling: whitespace and non-desktop Python tests |
| `doc` | Headless document changes: fast, formatting and optimized mt-doc tests |
| `ci` | Settled Rust changes: fast, formatting, optimized Clippy/workspace tests |
| `full` | Release artifacts: production-profile checks, build and privacy scan |

For UI interactions, reuse GPUI Kit headless fixtures and stable element IDs
before adding native automation; see `docs/development.md` for a runnable example.
During iteration, filter the affected Rust test or Python test module; see
`docs/development.md`. Fast does not test Rust; doc does not compile mt-app.
CI on the final proposed revision can supply the workspace gate. Do not repeat a
successful gate locally merely to open a PR, require a clean worktree while
iterating, or run full/native suites for every small task. Re-run after relevant
code/configuration/dependency changes or an unresolved failure, not arbitrarily.

Native acceptance remains explicit (`mt.py accept <goal> -- ...`), on an active,
unlocked Windows 11 x64 desktop with current hash-bound evidence. Check only the
changed workflow while iterating; batch full suites and the visual matrix at
integrated milestones. A single-case run or BLOCKED is not acceptance. Make at
most one retry after a concrete environment correction; otherwise record the
limitation and continue independent work. Behavioral failures require diagnosis.

Report commands, actual outcomes, pending checks and their next checkpoint.
Performance claims require production-profile measurements; consult existing
`.scratch/perf-and-size/` evidence and avoid concurrent builds on that machine.

## Invariants and delivery

- `mt-doc` stays GPUI-free. Keep `panic = "unwind"` and the vendored RaTeX
  allocation clamp. Content failures become diagnostics; preserve editable text.
- Never mutate a WebView from render; mark `web_dirty` and defer the update.
- Preserve consent, credential confidentiality and content-free native evidence.
  Do not log test text, user documents, secrets or local paths into that evidence.
- Ship required fonts/sample inside the executable. Windows 11 x64 is the only
  public-quality target; Linux/macOS CI provides compatibility coverage.
- Read Cargo.toml dependency comments. Prefer crates.io, retain the sole local
  patch, and keep the Kit/GPUI snapshot family on one registry source. Update manifests and
  locks together; changing dependency provenance requires explicit review.
- Use English in code/docs/commits/PRs. Make cohesive PRs, link the goal for
  product work, format changed Rust code, and avoid unrelated formatting churn.
