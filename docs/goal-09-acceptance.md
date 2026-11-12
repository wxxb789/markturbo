# Goal 09 acceptance evidence

**Status: implementation complete; acceptance evidence pending.** The visual
matrix and automated accessibility assertions are reproducible, but the owner,
first-time-user, and native keyboard/screen-reader evidence required by
[Goal 09](goals/09-refine-product-hierarchy-and-visuals.md#completion-evidence)
has not been recorded.

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
    --out .scratch/goal-09/matrix
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
- Source: `883a5f3` with a dirty worktree tree `72e660d`; executable
  `MarkTurbo.exe` (61,475,328 bytes) SHA-256
  `2d2abdd91b71199e6494e4a4dc52714bd0c213a85c19c01a07c9c3c92c21679c`.
- Output: `.scratch/goal-09/final4/matrix.json` and
  `.scratch/goal-09/final4/cells/` (132 receipts: 120 `PrintWindow`, 12
  `PrintWindow+CDP`). The automated accessibility snapshot counted 2,972
  actionable controls and 128 document task checkboxes; no compact control was
  unnamed.

The compatibility-preview state is captured through the CDP composite; a blank
page is rejected as `WEBVIEW_CONTENT_MISSING` rather than recorded as a pass.

Representative images were inspected for the welcome, new document, Edit,
native Preview, compatibility preview, Review loading/result/stale, Effective
Agent Context, conflict, and Settings states in both languages and modes. A full
owner review of all eleven states has not happened; the reviewer must inspect
`.scratch/goal-09/final4/cells/`.

## Remaining acceptance gates

1. Owner approval of the full eleven-state matrix, including any intentional
   exceptions.
2. Native keyboard traversal, visible focus, and a Windows 11 x64 screen-reader
   smoke for the changed surfaces.
3. The five first-time target-user sessions required by the product contract.

These are acceptance prerequisites, not follow-up feature work.

## Verification run

- `mt.py check fast`: 183 tooling tests passed.
- `mt.py check ci`: passed (formatting, optimized Clippy, workspace tests). The
  run includes `review_stale` and the i18n tests.
