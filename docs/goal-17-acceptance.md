# Goal 17 implementation and acceptance record

Contract: [Layout and content polish](goals/17-layout-content-polish.md).

## Local checks

The light ember is `#8D6240`: OKLCH lightness 0.534 and chroma 0.074, with
4.66:1 contrast against the light room. The previous lightness was 0.535.

Headless checks inspect the resolved text styles, not screenshots or widget
source.
The bundled Martian Grotesk has no `tnum` feature in GSUB/GPOS. The nsh 0.6.1
`tabular_nums()` API cannot supply that missing font feature, so changing counts
use bundled Martian Mono at the surrounding size. Tests assert the family and
verify equal digit advance.

The Revision loopback test withholds the response headers, presses Cancel,
and checks that the client disconnects and the prior Review returns. This
stops the local request; it cannot recall bytes the provider already received.

At a 1280 by 900 headless viewport, Settings actions end at the field's
right edge (x=644). The scroll viewport is 734px high and the form is 1055px
high; the usage section fits inside the viewport after scrolling. Checked
row origins include y=108, 124, 156, 316, 392, 424, 540, 564, 632, 680,
760, and 912.

Targeted regression checks detected removed states, incorrect layout, CJK scope,
low chroma, missing tabular figures, incorrect routing, and missing request
cancellation. Adding the CJK comparison to the default visual matrix failed
its isolated Python test (exit 1); restoring the opt-in behavior passed it.
Disabling Settings scrolling separately failed the last-section test because
Headless could no longer scroll the viewport (exit 1).
With routing intact, neutralizing error color and removing the status text's
tabular figures failed the two corresponding style assertions (exit 1).

The code gate is the GitHub CI run on the PR's final merged head plus
`uv run --locked --project scripts scripts/mt.py check ci`.

Verified head: (filled at merge)

## Desktop acceptance

Pending on the owner's active, unlocked Windows 11 x64 desktop. The CJK
optical-sizing capture is recorded below; no native pass is claimed.

- Run native acceptance against the final release executable and its expected
  SHA-256, as described in [verification.md](verification.md).
  Run 2026-11-24 on `cc4cf0a`: goals 02, 03 and 06 passed and goal-07 failed;
  the run exercises none of this goal's items (see
  [Desktop acceptance 2026-11-24](#desktop-acceptance-2026-11-24)).
- Run the visual matrix on the same executable. Capture before and after
  Settings, Review, and light-mode cooled model text. Check Settings at a
  900px window height, including the last section after scrolling.
  **Partly done 2026-11-24:** before captures on `1159142` covered 8 of 8 cells
  for `clean-profile-welcome, review-loading, review-result-questions,
  settings`; the after set is the `32f324d` matrix (20 of 22 cells; both
  `web-compatibility-preview` cells failed on the harness's UIA
  observation-channel gap, not a product failure). Settings' last section
  after scrolling was not captured.
- The mixed CJK/Latin optical-variant capture ran on 2026-11-23, light theme
  only (the recipe forces light), against markturbo.exe SHA-256
  `a6ef8cfb758e6934f1a7b17713f240ad9ea4255f753d5ff160a07193580b567b`. The
  owner picked **+1px with the next companion weight** (the third capture
  column); it is the production default, and the +0 and +1px columns stay
  capture-only.

```powershell
uv run --locked --project scripts scripts/mt.py visual -- --goal17-cjk-optics --exe <release-exe> --out .scratch/goal-17-cjk
```

## Native acceptance 2026-11-23

One `mt.py accept all` run on `main` `c3aab06` (Goals 14–19 merged), on the
owner's active, unlocked RDP desktop (Windows 11 x64, harness process session
2, input desktop Default). The harness hash-verified the release executable
(SHA-256 `a6ef8cfb758e6934f1a7b17713f240ad9ea4255f753d5ff160a07193580b567b`,
44,480,000 bytes). It covers goals 02, 03, 06 and 07 only: goal-02 PASS,
goal-03 PASS, goal-06 PASS (`ALL_REQUIRED_CASES`), goal-07 FAIL
(`WEB_SECURITY_APP_WINDOW_COUNT_UNEXPECTED`). It exercises none of this goal's
desktop acceptance items; the visual matrix and the before/after Settings,
Review and light-mode captures remain pending. The CJK optical capture is no
longer pending: the light-theme capture (the recipe does not support the dark
theme) ran on 2026-11-23 against the same hash-verified executable, and the
owner picked +1px with the next companion weight, now the production default
(recorded above). This is not a pass.

## Desktop acceptance 2026-11-24

One `mt.py accept all` run on `main` `cc4cf0a` (release executable SHA-256
`4b5a0747dff742eb5749dce3fdc58b13b6893a954cdc9191751287d9f744fb36`), retry
after the environment correction on 2026-11-24: goal-02 PASS, goal-03 PASS,
goal-06 PASS (`ALL_REQUIRED_CASES`), goal-07 FAIL
(`WEB_SECURITY_APP_WINDOW_COUNT_UNEXPECTED`). It covers goals 02, 03, 06 and
07 only and exercises none of this goal's items.

Before captures ran with the same harness options and states
(`clean-profile-welcome, review-loading, review-result-questions, settings`)
on `1159142` (Goal 17 before, release executable SHA-256
`dd09671496cfdeba125d754b5604d94fba23687e345f76dcc644883ddef10c5f`): 8 of 8
cells captured. The after set is the `32f324d` visual matrix: with
`--languages en-us --viewports default --modes light,dark`, 20 of 22 cells
captured, and both `web-compatibility-preview` cells failed with
`WEB_COMPATIBILITY_PREVIEW_WEB_VIEW_MISSING_UIA_CONTRACT_MISMATCH` — the
harness's UIA observation-channel gap, not a product failure, recorded in
[TODO.md](TODO.md). Settings' last section after scrolling was not captured.
Captures stay under `.scratch/` and are not committed.

Pending: the Settings last-section capture and the two
`web-compatibility-preview` cells. This is not a pass.
