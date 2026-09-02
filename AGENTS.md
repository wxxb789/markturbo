# MarkTurbo agent instructions

## Authority and scope

Read `PRODUCT.md`, the relevant ordered goal in `goals/`,
`docs/architecture.md`, and `CONCEPTS.md` before changing product behavior.
Goals are canonical: link to them from PRs and commits rather than duplicating
their requirements. `docs/history/` is historical context, not current scope.
Update `CONCEPTS.md` only when project vocabulary changes.

Keep changes scoped. Preserve user work and generated evidence. Do not change
later goals to bypass an earlier goal's acceptance gate.

## Platform and release status

Windows 11 x64 is the only public-quality target. CI tests Linux, macOS, and
Windows for compatibility, not as a release promise. CD publishes one
Windows `markturbo-windows-x64.exe` asset. Installers, signing, notarization, and
multi-platform distributables are future Goal 10 work.

Use the canonical tooling entry point:

```sh
uv run --project scripts scripts/mt.py check fast
uv run --project scripts scripts/mt.py check ci
uv run --project scripts scripts/mt.py check full
```

`fast` checks unstaged and staged whitespace changes plus explicit tooling
tests. With `BASE_SHA` and `HEAD_SHA` (or `--base` and `--head`), it checks only
that explicit CI revision range without changing the index. `ci` adds formatting,
Clippy, and locked release-profile workspace tests. `full` adds the safe local
release binary build. Native UI acceptance is always explicit:

```sh
uv run --project scripts scripts/mt.py accept goal-02 -- <arguments>
uv run --project scripts scripts/mt.py accept goal-03 -- <arguments>
```

Never claim native acceptance from source tests or CI. It requires an active,
unlocked Windows desktop, a current hash-bound executable, and a PASS evidence
file. A BLOCKED result is not acceptance evidence.

## Validation and evidence

Run the narrowest relevant tier while iterating. Before opening or merging a PR,
run `check ci` on the proposed commit from a clean tree. Before a goal completion
or release, run `check full` and report the actual pass count. Explain any test
not run.

Every reported measurement must name the command or evidence file. Performance
work uses `scripts/`; consult `.scratch/perf-and-size/`, repeat measurements, and
do not run concurrent release builds on this noisy machine. Regression tests
should demonstrate the failure first when practical; for desktop, WebView, GPU,
or foreground-gated behavior, state why a safe red proof is unavailable.
Source-scanning tests are coverage for those boundaries; preserve equivalent
coverage when moving the code they name.

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
