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
| Theme switch | Occasional | Instant (see deviation below) | None |

Theme deviation: nsh 0.6.1's editor resolves theme foreground, caret and
per-range decoration colours without a shared transition. The shell switches
palette with the editor instead of fading the ground underneath the new ink
for 200ms. Pointer hover and press feedback retain their own timings.

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
`ui/titlebar.rs` checks
disabled Review and samples live menu opacity and scale on both sides of each
reversal without advancing the clock between samples. `ui/consent.rs` checks
that a reduced-motion card mounts without transform bindings in its subtree.
Headless checks editor typing after closing a Revision result, that reopening
Review does not replay the previous anchor, and that Accept stays visible
and non-actionable when unavailable.
The clean-profile welcome test checks that Review remains visible and disabled.
Headless checks the banner's persistent zero-height slot, the scoped frame
updates for that slot and the document, and the exit fade. It reports target
layout frames, not native in-flight positions. The modal tests sample the
mounted animation graph through a close/reopen and check that discarding a
dirty tab does not animate removal of its rail row or editor.
The menu exit test checks that the scrim is gone and the workspace action
still executes. Headless routes those presses semantically; native pointer
hit testing remains part of desktop acceptance.

| Command | Exit | Result |
| --- | ---: | --- |
| `cargo fmt --all -- --check` | 0 | No formatting differences. |
| `cargo clippy --profile ci -p mt-core -p mt-app --all-targets` | 0 | 2 existing mt-core warnings and 1 existing mt-app test warning; no new warnings. |
| `cargo test --profile ci -p mt-app` | 0 | 42 test targets; 252 passed, 0 failed, 0 ignored (245 passed before the review fixes). |
| `cargo test --profile ci -p mt-core --features model-transport` | 0 | 833 passed, 0 failed, 7 ignored. |
| `uv run --locked --project scripts scripts/mt.py check fast` | 1 | 232 passed; 1 known failure in 1 of 8 tooling shards. |

During implementation, `mt.py check ci` stopped at the same tooling failure
before the Rust phase. The review fix stage reran the Rust gates directly.

The failing probe was
`Goal04BuildEvidenceTests.test_cargo_config_context_records_only_content_free_fingerprints`.
It reproduced in isolation (1 test, 1 failure, exit 1): the enclosing
worktree directory's shared Cargo config is fingerprinted alongside the
test's temporary Cargo-home config, so the count is 2 instead of 1. No shared
configuration or probe assertions were changed.

## Desktop acceptance

Pending on the owner's active, unlocked Windows 11 x64 desktop. No release
executable or receipts were generated in this implementation stage.

- Run the native acceptance suite on the final release executable with its
  expected SHA-256 using the command in [verification.md](verification.md).
- Reveal a recovery or conflict banner with Animation effects on. Verify that
  the banner grows from zero height while fading in and that the document
  below moves continuously with it, without a jump.
- Dismiss a recovery or conflict banner with Animation effects on. Around the
  midpoint of the exit, verify the content below is between its start and end
  positions while the banner is still fading. With Animation effects off,
  verify the banner dismounts immediately without animated reflow.
- With two tabs open, edit the active tab, close it and choose **Don't save**.
  Verify that its rail row and editor disappear immediately while the guard
  fades, and that the remaining editor accepts input during the fade.
- Close and reopen the guard or consent before its fade ends. Verify that
  opacity, scale and vertical position do not jump on the first frame back,
  and that the fading surface does not intercept pointer or keyboard input.
- Run the 22-state visual matrix on that release executable. Keep receipts
  separate from the historical Goal 09 captures. Use the existing harness with
  `--languages en-us --viewports default --modes light,dark` for eleven states
  in both appearances, and a fresh `--out .scratch/goal-16-visual`.
- Retain before/after Settings, consent, and title menu captures under
  `.scratch/`, using `dede9c6` for the before build and the same release profile,
  viewport, language, and appearance for both builds. Neither the matrix nor
  captures have been attempted here.
