# Agent instructions

Read the relevant source before working:

- `PRODUCT.md`: current owner-approved product contract and success thresholds.
- `goals/`: ordered delivery goals and acceptance evidence.
- `docs/architecture.md`: architecture and decisions.
- `CONCEPTS.md`: project vocabulary.

Read `docs/history/v0.1-product-direction.md` only when tracing the original
implementation brief. Its feature list is historical, not current product scope.

`CLAUDE.md` links to this file. On Windows without symlink support it may contain
only `AGENTS.md`; read this file instead.

## Workflow

- Inspect relevant code and documentation first. Keep changes scoped and
  preserve unrelated work.
- For numbered product work, read the goal and its prerequisites. Its scope,
  evidence, stop conditions, and next-goal boundary are binding; later goals do
  not bypass earlier gates.
- Keep each goal canonical. Link to it from tickets, plans, commits, and PRs
  instead of duplicating it.
- Update `CONCEPTS.md` only when project vocabulary changes.

## Evidence and validation

- Every reported number must come from a cited command or harness. State when it
  was not measured.
- Use `scripts/` for measurements; see `scripts/README.md`. Repeat performance
  runs on this noisy machine and avoid concurrent release builds.
- Consult `.scratch/perf-and-size/` before revisiting its recorded decisions.
- Run `cargo fmt --all` once before committing.
- Run `cargo clippy --workspace --all-targets`; add no warnings.
- Run `cargo test --release --workspace` on a clean tree before pushing and
  report the pass count. Let slow release builds finish.
- Report validation that could not run and why.

## Tests

- Prove each regression test fails without the fix, then passes with it.
- Explain every skip on stderr.
- Source-scanning tests protect behavior requiring a real window, WebView2, or
  GPU. Preserve equivalent coverage when moving named code.

## Structural invariants

- `mt-doc` has no GPUI dependency.
- Keep `panic = "unwind"` so renderer panics become diagnostics. Retain the
  allocation clamp in `vendor/ratex-parser`.
- Never mutate the WebView from `render`. Set `web_dirty` and use `cx.defer`; see
  `crates/mt-app/src/views/workspace/web_surface.rs`.
- Content problems become diagnostics, not UI-level errors. Preserve the source
  and keep broken documents editable.
- Ship installable fonts beside the binary. Only the two GPUI-required fonts in
  `assets.rs` are embedded; KaTeX fonts remain under `fonts/katex/`.

## Dependencies

- Prefer crates.io and read the workspace `Cargo.toml` comments before changing
  dependencies or features.
- `vendor/ratex-parser` is the only local patch; its README defines removal.
- Dependencies sharing a git source must use the same source selector. This
  applies to `gpui` and `gpui-component`.

## Code and commits

- Use English for code, comments, documentation, tickets, commits, and PRs.
- Comments explain evidence, non-obvious behavior, failure prevention, or
  rejected alternatives; omit code narration.
- Use one bounded ticket and one commit. Name the user-visible impact in the
  subject; put mechanism, evidence, and rejected alternatives in the body.
- Product commits name the exact file under `goals/` they advance.
