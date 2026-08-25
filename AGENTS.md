# Agent instructions

For anyone — human or agent — changing this repository. What the code *is* lives
in `docs/architecture.md`; what the product *must do* lives in `GOAL.md`. This
file is how work gets done here.

`CLAUDE.md` is a symlink to this file. One source of truth, so the two cannot
drift — which is the failure this project exists to make visible, given that it
recognizes both by name and lists them in the Harness panel. On Windows, a clone
without Developer Mode or `git config --global core.symlinks true` checks the
symlink out as a text file containing the path; if `CLAUDE.md` reads as one line
saying `AGENTS.md`, that is why, and this file is the one to read.

## Measure, never estimate

Every performance number in this repository's documents and commit messages came
from a command someone ran. That is not a stylistic preference — it is the rule
that has caught the most defects here, and breaking it has cost real time:

- An infinite reparse loop burned 300–750% of a core forever on any document
  over 4 KiB. Nobody found it by reading; a 60-second CPU trace found it.
- A source-text guard against an allocation bomb looked correct and was
  bypassable four ways. Running the bypasses is what showed it.
- A prior investigation reported a 78 MB startup baseline. It had measured an
  instrumented build with a feature switched off and reported the number as if
  it described HEAD. Two independent re-measurements found 148.8 MB.

So: if you write a number, say where it came from. If you could not measure
something, say that instead of reasoning about it and presenting the reasoning as
a finding.

**The harness is `scripts/`** — see `scripts/README.md`. It is committed so that
a measurement is reproducible by someone who is not you.

**This machine is noisy.** A concurrent release build has been observed to change
the same measurement by 2x. Take a time series or repeat a run; a single sample
lands on startup transients and has already produced two contradictory reports in
this repository's history.

## Verification

- `cargo fmt --all` before a commit, not after every edit.
- `cargo clippy --workspace --all-targets`. Four warnings are pre-existing —
  three `collapsible_if` in `translate.rs`, one `while_let_loop` in
  `watcher.rs`. Introduce no more.
- `cargo test --release --workspace` on a clean tree before a push, and report
  the pass count. **Release, not debug**: the eight-test performance gate in
  `mt-doc` has 2s and 45s bounds calibrated against an optimised build, and it
  fails in debug for that reason alone.
- A release build takes 5–14 minutes here. Budget for it rather than abandoning
  one that is merely slow.

## Tests

**A test that can pass while asserting nothing is not a test.** Thirty-seven
source-scanning tests in this repository once passed against a path that no
longer existed. When a test must skip — a machine without the KaTeX fonts, say —
make the skip *visible* on stderr rather than returning early in silence.

**Prove a new test can fail.** Revert the fix, watch it go red, restore it. A
test written after the fix and never seen red is a test whose failure mode is
unknown.

Several tests here scan source text with `include_str!` and
`views::production_source`, because the failure they guard needs a real window, a
real WebView2 runtime, or a real GPU — none of which a unit test has. They are
tripwires against a plausible-looking simplification, and each one carries the
measurement that justifies it. If you move a function they name, they break, and
that is the point.

## Comments

Comments explain **why**, and specifically why the obvious alternative is wrong.
`// increment the counter` is noise. What earns its place:

- The measurement behind a constant or a trade-off.
- The bug a piece of defensiveness prevents, concretely enough to recognise.
- An upstream behaviour that is not visible from the call site — a global
  revision counter, a `catch_unwind` that an allocation abort defeats, a child
  window that ignores Z-order.
- A rejected alternative and the number that rejected it, so nobody re-runs the
  experiment. `lto = "fat"` was measured at 1.2 MB for triple the build time;
  that is written down so it stays settled.

Write for the reader at 3am who is about to "simplify" the thing you did on
purpose.

## Commits

One ticket, one commit. The body carries the measured before and after, the
mechanism, and what was rejected. Subject line says what changed for the user,
not which files moved.

The effort's map and its tickets live in `.scratch/perf-and-size/`. A ticket
records the question, the answer, and the evidence — including what was tried and
did not work, which is often the more useful half.

## Structural rules

These are not preferences. Breaking one breaks something that is otherwise
guaranteed:

- **`mt-doc` has no GPUI dependency.** It is what lets the document engine drive
  a native renderer, a WebView, and future headless tools without divergence.
- **`panic = "unwind"` is required.** `RendererRegistry::render` wraps
  third-party renderers in `catch_unwind` so a panic in a diagram backend becomes
  an inline diagnostic. `abort` silently defeats that. Note that an *allocation*
  failure aborts regardless, which is why `vendor/ratex-parser` carries a clamp.
- **Never touch the WebView from `render`.** It is an OS child window; on Windows
  WebView2 pumps messages, so mutating it during a draw re-enters the window
  procedure with the `App` already borrowed. Everything goes through
  `web_dirty` and a `cx.defer`. See `views/workspace/web_surface.rs`, which
  documents all three rules and the bug each one cost.
- **No content problem returns `Result` to the UI.** A malformed document opens
  and can be repaired in the editor; a failed render shows its source beside the
  explanation. Diagnostics, not errors.
- **Embed no font** the user could install instead. The two in `assets.rs` are
  the exception and a different case: gpui requests them by exact path and
  diagram labels come out blank without them. The KaTeX faces ship *beside* the
  binary — see `fonts/katex/LICENSE.md`.

## Dependencies

Prefer crates.io. `vendor/ratex-parser` is the one local patch, it carries a
README explaining exactly what was changed and when to delete it, and the
workspace `Cargo.toml` comments every non-obvious dependency choice with the
reason and, where relevant, the measurement. Read those comments before changing
a version — several of them record an experiment you would otherwise repeat.

Two dependencies that share a git source must declare it identically, or Cargo
builds two incompatible copies. `gpui` and `gpui-component` are the live example.

## Language

Code, comments, commit messages, documentation and ticket bodies are English.
