# Goal 16 implementation and acceptance record

Contract: [Crisp afterglow](goals/16-crisp-afterglow-motion.md).

## Frequency map

| Surface | Trigger | Enter | Exit |
| --- | --- | --- | --- |
| Typing, scrolling, rail switch, tab switch | Frequent / keyboard | Instant | Instant |
| Field swap (preview, context, review, revision) | Frequent | Instant | Instant |
| File palette (Ctrl+P) | Keyboard | Instant | Instant |
| Settings page swap | Frequent | Instant | Instant |
| Title-bar menu | Pointer | Fade and scale 0.97 to 1 from top-right, 125ms | Fade, 90ms |
| Recovery/conflict banners | Occasional | Height and fade, 200ms; reflow the column | 150ms |
| Consent card | Occasional | Scrim fade and card scale 0.96 to 1 with 4px rise, 200ms | 150ms |
| Close/quit guard | Occasional | Same as consent | Same as consent |
| Status notice | Occasional | 4px rise and fade, 200ms; transient notices hold 3000ms | Fade, 150ms |
| Findings arriving | Model | One existing Heat ignition per list, without stagger | None |
| Theme switch | Occasional | Instant throughout the app | None |

The 2026-11-21 contract amendment makes theme changes instant throughout the
app. nsh 0.6.1 cannot transition the editor's theme colours, so the shell and
editor switch together. Pointer hover and press feedback retain their own
timings.

Finding and conflict-difference navigation positions the editor immediately.
Only a model review result arriving at its first anchored finding keeps the
camera glide.

These are surface timings, not pointer feedback. A press snaps on, releases
over 120ms, and does not scale text. The 100ms hover does not animate tab or rail
selection. Errors, refusals and in-progress status remain readable until
superseded. Reduced motion removes spatial transitions and caps surface fades
at 100ms. Banners and cards dismount immediately under reduced motion.

## Local checks

Gate results are separate from desktop acceptance. The motion token scan is
in `ui/motion.rs`. `ui/prims.rs` checks the pressed plate for a quiet action,
plate button and row while the gesture is held, using the native pressed-state
report that Headless does not send automatically. It also checks the mounted
transition on press-down and on a theme change under stationary hover.
`ui/titlebar.rs` checks disabled Review and samples live menu opacity and scale
on both sides of each reversal, then advances the clock to verify that both move toward the open
target. `ui/consent.rs` checks that the production consent wrapper and its
subtree mount without transform bindings under reduced motion.
Headless checks editor typing after closing a Revision result, that reopening
Review does not replay the previous anchor, and that Accept stays visible
and non-actionable when unavailable.
The clean-profile welcome test checks that Review remains visible and disabled.
Headless checks the banner's persistent zero-height slot, that the slot and
document frame updates share one batch with the layout animation, and the
exit fade. The conflict banner switches tabs without animating and keeps
Compare mounted during resolution. These tests report target layout frames,
not native in-flight positions. The modal tests sample the
mounted animation graph through a close/reopen and check that discarding a
dirty tab does not animate removal of its rail row or editor.
The menu exit test checks that the scrim is gone and a workspace control
under the fading panel receives a press. Native pointer hit testing remains
part of desktop acceptance. The notice takeover test samples the replacement
at full opacity and checks that stale cleanup cannot clear it.

| Command | Exit | Result |
| --- | ---: | --- |
| `cargo fmt --all -- --check` | 0 | No formatting differences. |
| `cargo clippy --profile ci -p mt-core -p mt-app --all-targets` | 0 | 2 existing mt-core warnings and 1 existing mt-app test warning; no new warnings. |
| `cargo test --profile ci -p mt-app` | 0 | 42 test targets; 255 passed, 0 failed, 0 ignored (252 passed before this fix round). |
| `cargo test --profile ci -p mt-core --features model-transport` | 0 | 835 passed, 0 failed, 7 ignored. |
| `uv run --locked --project scripts scripts/mt.py check fast` | 0 | 234 passed across 8 tooling shards. |

The review fix stage ran the Rust gates directly. The earlier Goal 04
Cargo-config probe failure no longer reproduces with the fix in the merged base.

## Desktop acceptance

Pending on the owner's active, unlocked Windows 11 x64 desktop. No release
executable or receipts were generated in this implementation stage.

- Run the native acceptance suite on the final release executable with its
  expected SHA-256 using the command in [verification.md](verification.md).
  Run 2026-11-24 on `cc4cf0a`: goals 02, 03 and 06 passed and goal-07 failed;
  the run exercises none of this goal's items (see
  [Desktop acceptance 2026-11-24](#desktop-acceptance-2026-11-24)).
- Reveal a recovery or conflict banner with Animation effects on. Verify that
  the banner grows from zero height while fading in and that the document
  below moves continuously with it, without a jump.
  **Done 2026-11-24:** the conflict banner revealed with its animation and moved
  the content below without a jump (see
  [Desktop acceptance 2026-11-24](#desktop-acceptance-2026-11-24)).
- Dismiss a recovery or conflict banner with Animation effects on. Around the
  midpoint of the exit, verify the content below is between its start and end
  positions while the banner is still fading. With Animation effects off,
  verify the banner dismounts immediately without animated reflow.
- Open **Compare** on a conflict banner and choose **Use disk version**.
  Confirm the reload and verify that the comparison remains intact while
  the banner fades and collapses with the document reflow.
  **Partly done 2026-11-24:** **Compare** → **Use disk version** dismissed the
  banner with the comparison intact; the reload confirmation and the banner's
  exit motion (fade and collapse with the document reflow) were not recorded
  (see [Desktop acceptance 2026-11-24](#desktop-acceptance-2026-11-24)).
- With two tabs open, edit the active tab, close it and choose **Don't save**.
  Verify that its rail row and editor disappear immediately while the guard
  fades, and that the remaining editor accepts input during the fade.
  **Partly done 2026-11-24:** the rail row and editor were removed while the
  guard faded, and the other editor accepted input; the removal immediacy and
  whether the other editor accepted input during the fade were not recorded
  (see [Desktop acceptance 2026-11-24](#desktop-acceptance-2026-11-24)).
- Close and reopen the guard or consent before its fade ends. Verify that
  opacity, scale and vertical position do not jump on the first frame back,
  and that the fading surface does not intercept pointer or keyboard input.
- Run the 22-state visual matrix on that release executable. Keep receipts
  separate from the historical Goal 09 captures. Use the existing harness with
  `--languages en-us --viewports default --modes light,dark` for eleven states
  in both appearances, and a fresh `--out .scratch/goal-16-visual`.
  **Partly done 2026-11-24:** 20 of 22 cells captured on `32f324d`; both
  `web-compatibility-preview` cells failed on the harness's UIA
  observation-channel gap
  (`WEB_COMPATIBILITY_PREVIEW_WEB_VIEW_MISSING_UIA_CONTRACT_MISMATCH`), not a
  product failure; recorded in [TODO.md](TODO.md).
- Retain before/after Settings, consent, and title menu captures under
  `.scratch/`, using `dede9c6` for the before build and the same release profile,
  viewport, language, and appearance for both builds. Neither the matrix nor
  captures have been attempted here.
  **Partly done 2026-11-24:** before captures on `dede9c6` covered 8 of 8 cells
  for `clean-profile-welcome, review-loading, review-result-questions, settings`;
  the after set is the `32f324d` matrix above. The consent card and title menu
  were not captured because no matrix state reaches them.

## Native acceptance 2026-11-23

One `mt.py accept all` run on `main` `c3aab06` (Goals 14–19 merged), on the
owner's active, unlocked RDP desktop (Windows 11 x64, harness process session
2, input desktop Default). The harness hash-verified the release executable
(SHA-256 `a6ef8cfb758e6934f1a7b17713f240ad9ea4255f753d5ff160a07193580b567b`,
44,480,000 bytes). It covers goals 02, 03, 06 and 07 only: goal-02 PASS,
goal-03 PASS, goal-06 PASS (`ALL_REQUIRED_CASES`), goal-07 FAIL
(`WEB_SECURITY_APP_WINDOW_COUNT_UNEXPECTED`). It exercises none of this goal's
desktop acceptance items; the visual matrix, the before/after captures and the
banner, card and menu checks listed above remain pending. This is not a pass.

## Desktop acceptance 2026-11-24

Owner manual checks on the `32f324d` executable (release executable SHA-256
`d9dc951ffe19fb1a3e3fe64456658f00f9e40d6398437f5b80673b8c2c6d77bd`) on
2026-11-24, all observed as expected: after an unsaved edit and an on-disk
change, saving raised the conflict banner, which revealed with its animation
and moved the content below without a jump; **Compare** → **Use disk version**
dismissed the banner with the comparison intact; and closing an edited tab
with **Don't save** removed its rail row and editor while the guard faded, and
the other editor accepted input. Not recorded: the reload after **Use disk
version**, the banner's exit motion, the close-tab removal immediacy and
whether the other editor accepted input during its guard fade. Not checked:
Animation effects off, and reopening a guard or consent before its fade ends.

Before captures ran with the same harness options and states
(`clean-profile-welcome, review-loading, review-result-questions, settings`)
on `dede9c6` (Goal 16 before, release executable SHA-256
`cb736a67076d24978aac9652c81c87467dc20dad083da5b8d035ea8b165a7636`): 8 of 8
cells captured. The after set is the `32f324d` matrix below. The consent card
and the title menu were not captured because no matrix state reaches them.
Captures stay under `.scratch/` and are not committed.

The 22-state visual matrix ran on the `32f324d` executable with
`--languages en-us --viewports default --modes light,dark`: 20 of 22 cells
captured. Both `web-compatibility-preview` cells failed with
`WEB_COMPATIBILITY_PREVIEW_WEB_VIEW_MISSING_UIA_CONTRACT_MISMATCH` — the
harness's UIA observation-channel gap, not a product failure; it is recorded
in [TODO.md](TODO.md).

This is not a pass: the reload confirmation, the banner-exit checks, the
close-tab removal immediacy and input during its guard fade, the guard or
consent reopen, and the two `web-compatibility-preview` cells stay pending.
