# Goal 09 acceptance evidence

**Status: accepted on 2026-11-13 and archived as
[Goal 09](goals/archive/09-refine-product-hierarchy-and-visuals.md).** The
release-build matrix and its automated accessibility assertions are bound to the
accepted revision; the two evidence items the completion list also names are
carried forward in [TODO](TODO.md).

## Completion

- Accepted on 2026-11-13 by the project owner, who reviewed the product manually
  and recorded no intentional exception.
- Accepted revision `eeb3b30` with a clean worktree; executable `MarkTurbo.exe`
  (61,664,768 bytes) SHA-256
  `927d8238e56f89b38b13c736dae4399c29c8a5639ce0b5df86e5e97534fc2bbd`.
- Evidence: `docs/evidence/goal-09/final5/` - one review sheet per viewport and
  the 132 cells and receipts behind them.
- Not recorded, and therefore carried into Goal 10's clean-machine acceptance:
  native keyboard traversal with visible focus and a Windows 11 x64
  screen-reader smoke, and the five first-time target-user sessions PRODUCT.md
  requires before public-quality visual acceptance.

## Implemented corrections

- Persisted light/dark preference is applied at startup instead of waiting for a
  Settings click.
- the Settings page scrolls at the minimum viewport, so lower fields remain
  reachable.
- the sidebar file tree shrinks at the minimum height, keeping Agent Context,
  Settings, and Quit visible.
- at the compact width, an open Review or Agent Context panel stacks below the
  document instead of squeezing it into an unusable column; both remain
  scrollable and the document stays editable.
- Review status, section titles, findings, and clarification priorities use the
  existing English/Chinese i18n keys.
- Editing a reviewed document now marks the retained result stale; a new
  `review_stale` headless regression covers the transition.
- visible status strings for opening a folder, save conflicts, and saved Settings
  use the interface language.
- the file-tree search field, Settings fields, Review clarification field, and
  compatibility WebView expose localized UIA names instead of falling back to a
  test ID or the document title.

## Matrix command

```sh
uv run --locked --project scripts scripts/mt.py visual -- \
    --exe crates/mt-app/.nsh/out/MarkTurbo/MarkTurbo.exe \
    --out docs/evidence/goal-09/final5
```

The packaged executable comes from the same revision:

```sh
cd crates/mt-app && nsh package --target windows --out .nsh/out
```

Each cell launches the packaged app on an isolated profile, drives the real
controls, resizes to the requested DIP viewport, verifies the effective theme
background, captures the client through `PrintWindow`, and writes one PNG and a
content-free receipt. Receipts bind the source revision, worktree tree,
executable hash, DPI, DIP and pixel viewport, language, mode, obstruction
observations, actionable UIA roles/names, and PNG hash.

For compatibility preview the harness starts WebView2 with a free remote
debugging port, captures the page through CDP `Page.captureScreenshot`, and
composites it into the UIA pane rectangle over the PrintWindow chrome. On a
portrait RDP desktop the landscape viewports are placed off-screen with
`SWP_NOSENDCHANGING`; the client area still renders at the requested size.

## Current evidence

- Captured: 132 cells = 11 states × 3 viewports × 2 modes × 2 languages.
- Viewports: 720×480 DIP (900×600 px), 1400×900 DIP (1750×1125 px), and
  1920×1080 DIP (2400×1350 px) at 120 DPI.
- Source: `eeb3b30` with a clean worktree; executable `MarkTurbo.exe`
  (61,664,768 bytes) SHA-256
  `927d8238e56f89b38b13c736dae4399c29c8a5639ce0b5df86e5e97534fc2bbd`.
- Output: `docs/evidence/goal-09/final5/matrix.json`, `final5/cells/` (132
  receipts: 120 `PrintWindow`, 12 `PrintWindow+CDP`), and one review sheet per
  viewport (`final5/contact-{minimum,default,wide}.png`, 4-column grids of the
  eleven states labelled with their cell IDs). The sheets are review aids; the
  cells and receipts are the evidence.
- The capture keeps each cell's profile (settings, logs, recovery payloads) in a
  system temporary directory that is deleted with the cell, so a cell holds the
  fixture workspace its capture shows, `capture.png`, and `receipt.json`.
- Result: 132/132 cells captured with no obstruction and no viewport mismatch;
  every cell's measured background matched its requested mode. The automated
  accessibility snapshot counted 2,980 actionable controls and 136 document task
  checkboxes; no compact control was unnamed.
- Earlier capture `docs/evidence/goal-09/final4/` pinned `883a5f3` with a dirty
  worktree `72e660d` and predates the hierarchy, logging, and version commits
  that followed. It is retained for provenance and superseded by `final5`.
- Interface-language coverage is enforced at compile time (`i18n::Key` with one
  exhaustive table per language) and by `every_key_has_a_string_in_every_language`
  and `chinese_covers_every_key` in the workspace gate; the matrix renders all
  eleven states in both languages.
- No state can depend on a tooltip or floating surface over the WebView: the
  desktop app uses no tooltip, popover, or hover-surface API
  (`crates/mt-app/src`, `crates/mt-app/tests`), and compatibility preview is
  captured from the page itself rather than from a floating overlay.

The compatibility-preview state is captured through the CDP composite; a blank
page is rejected as `WEBVIEW_CONTENT_MISSING` rather than recorded as a pass.

Representative images were inspected for the welcome, new document, Edit,
native Preview, compatibility preview, Review loading/result/stale, Effective
Agent Context, conflict, and Settings states in both languages and modes; the
minimum-viewport unsaved-document cells were checked for content rather than a
uniform frame. A full owner review of all eleven states has not happened; the
reviewer must inspect `docs/evidence/goal-09/final5/`.

## Remaining acceptance gates

1. Owner approval of the full eleven-state matrix, including any intentional
   exceptions.
2. Native keyboard traversal, visible focus, and a Windows 11 x64 screen-reader
   smoke for the changed surfaces.
3. The five first-time target-user sessions required by the product contract.

These are acceptance prerequisites, not follow-up feature work.

### Owner review checklist

Review `docs/evidence/goal-09/final5/contact-default.png`,
`contact-minimum.png`, and `contact-wide.png`, then open individual cells under
`docs/evidence/goal-09/final5/cells/` where a sheet is too small to judge. For
each state, confirm that the primary action is the obvious next action, the
active artifact and safety state are legible, and nothing competes for first
attention. Record the outcome of every row, not only the failures.

| # | State | Confirm |
| --- | --- | --- |
| 1 | clean-profile welcome | primary action and open/paste path are obvious |
| 2 | new unsaved document | unsaved state is visible before any save |
| 3 | ordinary Edit | editing is the primary surface, not a mode |
| 4 | native Preview or split | preview is reachable without hunting |
| 5 | web compatibility preview | the uncommon path is disclosed, not promoted |
| 6 | Review loading | the outbound request state is unambiguous |
| 7 | Review result with questions | findings, questions, and unapplied revisions read apart |
| 8 | stale or malformed Review result | staleness is visible where the result is read |
| 9 | Effective Agent Context | provenance and precedence are readable |
| 10 | external-change or destructive-close | the destructive choice is explicit |
| 11 | Settings | advanced preferences stay out of the primary path |

Approval is recorded by editing this file: state the approving owner, the date,
the revision and executable hash reviewed, any intentional exceptions with their
reason, and any state that must be recaptured.

### Keyboard and screen-reader smoke

This gate needs a person on an active, unlocked Windows 11 x64 desktop; the
capture harness asserts actionable control names but cannot observe narration or
focus visibility ([verification](../docs/verification.md)). There is no
`mt.py accept goal-09` harness: `ACCEPTANCE_GOALS` covers the retired goal-02,
goal-03, goal-06, and goal-07 selectors, and rebuilding them for nsh is not part
of this gate. With the packaged build running and Narrator enabled:

1. Tab through the welcome, editor, Review, Agent Context, Settings, and
   destructive-close surfaces without using the mouse.
2. Confirm every stop is reachable, visibly focused, and announced with a name
   and role that match what is on screen.
3. Confirm no state depends on a tooltip or floating surface over the WebView.
4. Record the desktop build, the revision and executable hash, the surfaces
   exercised, and every failure in this file.

### First-time-user sessions

Five first-time target users follow the
[First-Time-User Evidence](../docs/PRODUCT.md#first-time-user-evidence) contract
on a clean Windows 11 profile with a real or rights-cleared artifact, without
facilitator instruction after the task brief. Record one row per session and
report the aggregate against the contract thresholds:

| Field | Meaning |
| --- | --- |
| session, date, language | identification; English and Simplified Chinese are both allowed |
| clean profile, artifact | the profile and the artifact used |
| completed open/paste, Review, interpretation, accept/reject in 15 minutes | counts toward at least four of five |
| identified what leaves the machine before the request | required of all five |
| separated finding from inference; unapplied revision from source text | required of all five |
| silent send, silent source change, accidental loss | must be zero |
| facilitator notes | where the participant hesitated and what vocabulary misled them |

Do not record user document text, credentials, or local absolute paths in this
file; keep participant identity out of the repository and reference sessions by
number only.

## Verification run

On `eeb3b30` with a clean worktree and the executable above:

- `mt.py check fast`: 214 tooling tests passed in 11.2 s across 8 shards;
  `git diff --check` reported nothing for the working tree and the index.
- `mt.py visual`: 132/132 cells captured.
- `cargo test --release -p mt-app --lib logging::tests::`: 2 passed, including
  the guard regression, which fails when the guard call is removed.
- `cargo test --release -p mt-app --test translation_flow`: 1 passed.
- `mt.py check full` passed on `0f3f41e` before these fixes: production-profile
  formatting and Clippy, 853 workspace tests across 29 test binaries with 0
  failures and 7 ignored opt-in probes, the locked `-p mt-app --bin markturbo`
  build, and the binary privacy scan. The workspace gate for this revision is
  the pull-request CI run.
