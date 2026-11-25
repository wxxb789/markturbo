# MarkTurbo agent instructions

## Load only the context the change needs

- Product behavior: read the relevant PRODUCT.md contract and any active goal in
  `docs/goals/`, then the affected architecture section. Read CONCEPTS.md when
  a term is unclear or changes; update it only when vocabulary changes.
- Tooling/CI/dependencies: start with the affected script, manifest and tests.
  Do not load the entire product roadmap, corpus or historical reports.
- Rust module/crate moves: use `.agents/skills/rust-quality/SKILL.md` (read its
  GPUI wording as nsh) to trace ownership, affected callers and failure contracts
  before editing. Its pinned upstream guidance does not override product
  behavior or measured evidence.
- UI: `crates/mt-app` is a Native Shell (nsh) Rust app on the native Win32,
  AppKit and GTK 4 backends; it has no GPUI or GPUI Kit dependency. Verify nsh
  APIs in the crate source of the version Cargo.lock selects, not an older local
  registry copy; live upstream docs may describe a different version. Project
  product/architecture decisions govern over generic library examples; no
  upstream guide requires rewriting unrelated application code.

PRODUCT.md owns product scope, goals own acceptance, `docs/development.md` owns
validation cadence, and source/tests establish current implementation. History
and `docs/goals/archive/` do not establish current acceptance. Preserve user work,
immutable evaluation snapshots and existing evidence.

## Work to a bounded result

For non-trivial goals, use available subagents and multi-agent capabilities to
run independent, bounded workstreams in parallel wherever safe. Assign disjoint
write ownership, integrate the results, and verify the combined behavior; do
not serialize independent work merely for convenience.

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
| `doc` | Headless core changes: fast, formatting and optimized mt-core tests |
| `ci` | Settled Rust changes: fast, formatting, optimized Clippy/workspace tests |
| `native` | Native acceptance harness code only: parallel harness unit tests; not acceptance |
| `full` | Release artifacts: production-profile checks, build and privacy scan |

For UI interactions, reuse `nsh_testing::Headless` with stable `.test_id(..)`
ids (select `"id:…"`, accessible labels `"label:…"`) before adding native
automation; see `docs/development.md` for a runnable example. The overlay layer
is `PointerEvents::BoxNone`, so headless presses reach controls under it. Only
an open scrim or card (consent, notices guard) takes presses. A field-only probe
is optional isolation, not a hit-testing workaround. During iteration, filter the
affected Rust test or Python test module; see `docs/development.md`. Fast does
not test Rust; doc does not compile mt-app. Large-document timing probes are
opt-in and excluded from `ci`; see `docs/verification.md`. CI on the final
proposed revision can supply the workspace gate. Do not repeat a successful gate
locally merely to open a PR, require a clean worktree while iterating, or run
full/native suites for every small task. Re-run after relevant
code/configuration/dependency changes or an unresolved failure, not arbitrarily.

Native acceptance remains explicit (`mt.py accept all --exe ... --expect-exe-sha256 ...`
runs goal-02/03/06/07 in one pass; `mt.py accept <goal> -- ...` diagnoses one), on
an active, unlocked Windows 11 x64 desktop with current hash-bound evidence. The
harnesses target the nsh UI; their acceptance stays pending until a live PASS.
Check only the changed workflow while iterating; batch full suites and the visual
matrix at integrated milestones. A single-case run or BLOCKED is not acceptance.
Make at most one retry after a concrete environment correction; otherwise record
the limitation and continue independent work. Behavioral failures require diagnosis.

Report commands, actual outcomes, pending checks and their next checkpoint.
Performance claims require production-profile measurements; consult existing
`docs/evidence/perf-and-size/` evidence and avoid concurrent builds on that machine.

## Invariants and delivery

- `mt-core` stays UI-framework-free. Keep `panic = "unwind"` and the vendored
  RaTeX allocation clamp (0.1.15, matching nsh-base 0.6.1). Content failures become
  diagnostics; preserve editable text.
- Never mutate a WebView from render; mark it dirty (`web_dirty`) and defer the
  update, as `ui/preview.rs` does by writing the page from an effect into the
  `Signal<String>` that `web_view_html` reads.
- Afterglow UI (`crates/mt-app/src/ui/`; `mod.rs` and file headers supersede
  `docs/architecture.md`). Take colours, sizes and fonts from `ui::tokens`;
  read `tokens::palette()` inside style closures. Keep the overlay layer
  `PointerEvents::BoxNone`. Build editor decoration styles from declarations
  (`Style::default().with(StyleKey::Color, …)`), not typed refinements.
- The consent card takes keyboard focus when it opens. A held `Ctrl+Enter` (or `Enter`/`Space` on Send) counts only
  from the card's own `KeyDown`/`KeyUp`; blur or window deactivation cancels the hold. Use no system keyboard hooks.
  nsh 0.6.1 delivers focus and key events to ordinary views on every platform.
- On Windows, DirectWrite matches an embedded face only by its legacy family,
  one per instance (`Martian Grotesk Std Lt`/`Std Rg`/`Std Md`,
  `Martian Mono Cn Rg`). Use the `tokens::FONT_*` lists, which put the legacy
  name first, then the typographic name, then CJK fallbacks. Weight alone does
  not select a Grotesk instance. Windows `FONT_INPUT` is the exception: nsh
  0.6.1 inputs load only the first nonempty family, from system fonts.
- Preserve consent, credential confidentiality and content-free native evidence.
  Do not log test text, user documents, secrets or local paths into that evidence.
- Ship required fonts (KaTeX, Martian)/sample inside the executable. Windows 11
  x64 is the only public-quality target; Linux/macOS CI provides compatibility.
- Read Cargo.toml dependency comments (their GPUI mentions are historical).
  Prefer crates.io, retain the sole local patch (`vendor/ratex-parser`, 0.1.15),
  and track the latest nsh release: nsh ships features and fixes quickly, so
  upgrade every nsh crate and the nsh CLI together to the newest version rather
  than holding an older pin; check crates.io, not Cargo.lock, for the current
  version. Update manifests and locks together; changing dependency provenance
  requires explicit review.
- Use English in code/docs/commits/PRs. Make cohesive PRs, link the goal for
  product work, format changed Rust code, and avoid unrelated formatting churn.
