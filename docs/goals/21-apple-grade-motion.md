# Goal 21 — Apple-grade motion

## Objective

The owner's brief, 2026-11-24: bring every state change to a refined, smooth, Apple-grade standard. Audit by
state change, not by component: anything that appears, disappears, moves, resizes, changes content, swaps an icon,
changes a number, is pressed, hovered, loading, completes or errors. For each, the user must understand it.

Good motion:
- **Explains what happened:** where something came from, where it went, and what it relates to. Never decoration.
- **One language:** the same class of change uses the same curve and rhythm, from `ui::motion` tokens.
- **Restrained:** fast and light; frequent actions faster or instant.
- **Follows the hand and can be interrupted:** retargets from the current value.
- **Costs no frames:** measured, including in long content.
- **Introduces nothing new:** no jitter, flash, reflow or overflow; honours reduced motion.
- **Holds everywhere:** at 720×480 and 1920×1080, in light and dark. Desktop only.

Source: two motion audits, judged by the Gemini motion direction (2026-11-24). The spec below is the contract.

## Owner decisions (2026-11-24)

- **Keep the editor mounted.** Opening Settings, and crossing the 1040/1280 px layout breakpoints, must not rebuild the
  source editor. Caret, selection, scroll offset and undo history survive. Tab switches keep each tab's own state
  where nsh allows; if nsh cannot keep per-tab undo, record the limit and keep the active tab's state.
- Merge to `main` only.

## Done when

1. Every class in the motion spec table is implemented with the listed tokens. Every duration and curve in `ui/` comes
   from `ui::motion`: the existing token scan, or a new one, fails on raw literals. The "stays instant" list stays
   instant.
2. Every fix in the ranked motion list is implemented, or re-scoped in the PR with the reason. Interruptions retarget
   from the current value; `Heat` never jumps upward when cooling.
3. Under reduced motion, every class takes its listed fallback; tests cover the reduced path for each animated class.
4. No layout shift where the spec reserves space. The WebView status, the Base URL note, the CJK disclosure and the
   Compare panel all keep a stable size. Nothing overflows at 720×480: the window minimum size is enforced, and the
   consent and guard cards clamp their width.
5. The editor-retention decision holds. A Headless test proves caret, scroll and undo survive opening and closing
   Settings and crossing a breakpoint.
6. Frame cost is measured on a production-profile build with a large document: at least the existing large-document
   probe, plus a frame-time measurement of a representative animation (banner reveal, rail widen, card enter) while a
   long document is open. Record p50/p95 frame time before and after in `docs/goal-21-acceptance.md`; no regression
   beyond noise. If no frame-time instrument exists in nsh 0.6.1, add the smallest probe and document it, or record
   the limitation — never claim smoothness without a number.
7. `mt.py check ci` passes. GPT review converges to P3-only. The conductor's desktop walk-through on the release build
   checks each class at 720×480 and wide, in light and dark.

## Failure control

- New motion on a "stays instant" item.
- Layout animation inside long content.
- A raw duration or curve literal.
- Any flicker or overflow introduced.
- Weakened tests or thresholds.

## Out of scope

UX behaviour changes (Goal 20), except where a motion fix needs a reserved slot. WebView2 internals.

## Delivery lanes (after Goal 20 merges; disjoint ownership)

- **21-A Emission:** `ui/glow.rs`, `ui/motion.rs` (Heat), the rail dot in `ui/rail.rs`, and the review stale drain in
  `ui/review.rs`. Fixes 1, 2, 8, 11, 12.
- **21-B Layout stability:** `ui/preview.rs`, `ui/notices.rs` (Compare), `ui/settings.rs`, `ui/fonts.rs`, the wrap rows
  in `ui/revision.rs` and `ui/review.rs`, the card width in `ui/consent.rs` and `ui/tokens.rs`, and the window
  minimum. Fixes 5, 6, 7, 10, 13.
- **21-C Pointer and keyboard motion:** `ui/prims.rs`, `ui/titlebar.rs`, `ui/rail.rs` (widen/collapse), and the
  revision scroll. Fixes 3, 4, 9.
- **21-D Editor retention:** `ui/shell.rs`, `app.rs`, `ui/document.rs`. The owner decision above.
- **21-E Measurement:** the frame-time probe and the acceptance numbers (Done-when 6). The probe lands first, then the
  baseline is recorded. The after numbers are recorded once 21-A to 21-D merge.

## Mergeable alone

Each lane is mergeable alone on `main`.

## Contract amendments (GPT contract review and owner decision, 2026-11-24)

These override any conflicting text in the motion direction below.

1. **Consent filament (spec row 12).** The owner decided that one continuous 900 ms hold authorizes the send and that
   releasing resets authorization to zero (Goal 20, amendment 1). The filament drains visually on release. A new press
   charges from 0; it never resumes from the cooled value.
2. **Fix 1, cooling from the actual value.** When a finding loses focus, it cools from its current heat. When ignition
   had already finished, that is 1.0. When focus left mid-ignite, it is the partial value. Cooling never jumps upward.
   Test both cases: fully lit, and losing focus mid-ignite. Both must start at the displayed value and decrease
   monotonically.
3. **Frame-cost gate (Done when 6).**
   - **Build and fixture:** a production-profile build, and a generated Markdown document of 10,000 lines open in Split.
   - **Scenarios:** the banner reveal, the rail widen and collapse, and the card enter and exit, each triggered 20
     times.
   - **Metric:** the interval between presented frames during the animation windows: p50, p95 and max.
   - **Pass:** after p95 ≤ before p95 + 1 ms, and no frame above 33 ms inside an animation window.
   - **Instrument:** lane 21-E adds the smallest content-free probe. Examples: an env-gated frame-time log, or
     PresentMon if it is available on the machine. Document the probe in `docs/verification.md`.
   - **Missing data is never a pass:** without numbers, Done when 6 stays pending.
   - **Baseline:** lane 21-E's probe lands first. `before` is measured on the commit that adds the probe, before any
     motion lane. `after` is measured on the integrated head, with the same probe, machine and fixture.
   - **Per scenario:** each scenario passes on its own. Pooling the scenarios is not a pass.
4. **Cooling below `EMBER`.** When focus leaves while heat is below `EMBER` (for example 0.10, 9 ms into ignition),
   cooling ends at that value and never rises to `EMBER`. The target is `min(current, EMBER)`. Test that case alongside
   amendment 2's cases.
5. **Order after Goal 22.**
   - Goal 21 starts after Goal 22, because Goal 22 replaces the dot rail with a persistent file sidebar.
   - Before any lane starts, the owner picks one of three distinct interactive HTML motion prototypes (owner rule,
     2026-11-24). The chosen direction refines the motion spec below.
   - Fixes 4 and 11 and lane 21-C's rail widen and collapse apply to the sidebar's collapse and expand.
   - The frame scenario "rail widen and collapse" becomes "sidebar collapse and expand".
   - If a scenario is instant in the baseline, it has no animation-window frames to compare. Two measures replace the
     p95 comparison for it:
     - **Toggle latency:** from input to the first presented frame of the new state. After p95 ≤ before p95 + 1 ms.
     - **Absolute bound:** no frame above 33 ms inside the after build's animation window.

---

# Motion direction (contract body)

## 4. Motion Language Spec

### A. State-Change Motion Table

Every duration and curve in `ui/` comes from `crates/mt-app/src/ui/motion.rs`. Three small additions to `motion.rs` are authorized and justified below:
- `LOAD_LABEL_DELAY_MS: u64 = 100` — debounce before showing `"Preparing preview…"` on WebView navigation (replaces semantic borrow of `HOVER_MS`).
- `PROGRESS_TICK_MS: u64 = 150` — CJK font download progress polling cadence (moves raw literal out of `fonts.rs`).
- `UNDO_HOLD_MS: u64 = 5000` — hold window for destructive-action Undo notices (longer than the `3000 ms` `STATUS_HOLD_MS` read-only status hold).

| Class of State Change | `ui::motion` Token(s) | Duration | Curve | What Animates | Interruption Rule | Reduced-Motion Fallback (`reduce_motion()`) |
|---|---|---|---|---|---|---|
| **1. Appear (Modal Card / Scrim)** | `ENTER_MS`, `CARD_RISE_MS`, `SCALE_FLOOR` (0.96), `CARD_RISE_PX` (4.0) | 200 ms | `EASE_OUT` `(0.23, 1, 0.32, 1)` | Opacity `0→1`, Scale `0.96→1.0`, TranslateY `+4px→0` (`Presence` + `card_geometry`) | Re-closing mid-enter retargets opacity from current value; transforms freeze on exit and resume on reopen | Opacity `0→1` over `REDUCED_CROSSFADE_MS` (100 ms); **no scale, no translate** |
| **2. Appear (Menu Popover)** | `MENU_ENTER_MS`, `MENU_SCALE` (0.97) | 125 ms | `EASE_OUT` | Opacity `0→1`, Scale `0.97→1.0` (origin top-right) | Re-closing mid-open retargets from current on-screen opacity/scale | Opacity `0→1` over `REDUCED_CROSSFADE_MS` (100 ms); no scale |
| **3. Appear / Disappear (Banner & Disclosure Height)** | `ENTER_MS` (in) / `EXIT_MS` (out), `ease_out_layout` | 200 ms in / 150 ms out | `EASE_OUT` | Slot layout bounds (`ElementRef::animate_layout`) + child opacity (`Presence`) | Reversing mid-flight retargets from current frame; existing banners mount snapped on room return (`with_initial(..., true)`) | Instant layout height change; opacity crossfade `≤ 100 ms` (`REDUCED_CROSSFADE_MS`) |
| **4. Disappear (Card / Menu / Notice)** | `EXIT_MS` (card/notice) / `MENU_EXIT_MS` (menu) | 150 ms / 90 ms | `EASE_OUT` | Opacity `1→0` in place (transforms frozen at rest) | Re-opening mid-exit retargets opacity toward `1.0` from current value | Instant (`0 ms` opacity set to `0.0`, `visible = false`) |
| **5. Move (Editor Camera Glide on Model Arrival)** | `CAMERA_MS`, `CAMERA_TICK` | 360 ms | `EASE_OUT` (unify `camera_easing` with `EASE_OUT`) | Editor scroll offset (paint-only, no layout reflow) | Any user scroll wheel, keystroke, or new target immediately aborts/retargets from current pixel offset | Instant scroll jump to target line (`0 ms`) |
| **6. Resize (Rail Hover Widen & Collapse)** | `RAIL_WIDEN_MS` | 160 ms | `EASE_OUT` | Overlay strip background + label opacity `0↔1` (240 px strip kept mounted while fading; `pointer_events` gated on logical `hovered`) | Pointer re-enter/leave retargets opacity from current value without clipping at 44 px | Instant opacity toggle (`0 ms`) |
| **7. Content Swap (Status Notice / Web Load Text)** | `ENTER_MS`, `EXIT_MS`, `STATUS_HOLD_MS` (3000 ms) / `UNDO_HOLD_MS` (5000 ms) | 200 ms in / 150 ms out | `EASE_OUT` | Reserved-height container; opacity `0↔1` + `4 px` rise on status text | New status message retargets opacity/rise and resets hold timer; never shifts sibling layout | Opacity crossfade `≤ 100 ms`, `0 px` rise |
| **8. Icon / Mark Swap (Rail & Gutter Emitters)** | `Heat` (`IGNITE_MS` 90 ms, `COOL_MS` 1260 ms, `EMBER` 0.18) | 90 ms rise + 1260 ms cool | `LINEAR` rise, exponential `COOL_STOPS` decay | Color & alpha interpolation (`Heat::glyph` / `Heat::plate`); geometry never changes | `cool()` decays monotonically from current heat value (never jumps back to `1.0`) | `100 ms` `EASE_IN_OUT` crossfade directly to `EMBER` |
| **9. Number Change** | — (Instant) | 0 ms | — | Tabular monospace glyph replacement (`FONT_MONO`) | Immediate | Immediate |
| **10. Press (All Interactive Controls)** | `PRESS_RELEASE_MS` | 0 ms down / 120 ms up | Instant on press-down; `EASE_OUT` on release | Plate alpha (`6%→16%`) and ink color (`prims::press_feedback`); **never scales text** | Re-pressing snaps immediately to pressed state; theme change zeroes duration | Unchanged (color-only, `≤ 120 ms`, zero transform) |
| **11. Hover (All Interactive Controls & Labels)** | `HOVER_MS` | 100 ms | `EASE_OUT` | Plate alpha (`0→6%`) and child label ink (`ink_rest → ink`) via `group_hover` | Pointer leave retargets color from current value | Unchanged (color-only, `100 ms`, zero transform) |
| **12. Loading (Hold-to-Send Filament & Web Debounce)** | `FILAMENT_CHARGE_MS` (900), `FILAMENT_HOLD_MS` (1200), `FILAMENT_COOL_MS` (750), `LOAD_LABEL_DELAY_MS` (100) | 900 ms charge / 750 ms cool | `LINEAR` charge, exponential `COOL_STOPS` release | Filament `ScaleX(0→1)` and adaptive color ramp (`dull red → hot white`) | Releasing early cools from exact reached charge `from`; re-pressing resumes from cooled value | 3 discrete steps (`1/3`, `2/3`, `Sent`) + `100 ms` opacity fade on release; no `ScaleX` |
| **13. Complete (Model Arrival / Hunk Accept)** | Arrival: `IGNITE_MS` + `COOL_MS`; Accept: `EXTINGUISH_MS` | 90+1260 ms / 120 ms | Exponential cool / `Easing::EaseIn` | Heat emission (`1.0 → EMBER` on arrival; `1.0 → 0.0` on accept before advancing) | Stepping `J`/`K` during extinguish cancels auto-advance and moves immediately | `REDUCED_CROSSFADE_MS` (100 ms) fade |
| **14. Error / Stale Drain (Afterglow Afterimage)** | `AFTERIMAGE_MS` | 1200 ms | Linear stepped drain (`HEAT_TICK_MS` 33 ms) | Both column decorations **and** Review field emitters drain together to `afterimage` ink | New Review or tab switch stops drain immediately | `REDUCED_CROSSFADE_MS` (100 ms) crossfade to afterimage |
| **15. Surface Switch (Room, Tab, Pane, Field, Theme)** | — (Instant) | 0 ms | — | Direct mount/swap | Immediate | Immediate |
| **16. List Insert / Remove (Recovery Rows)** | `EXIT_MS`, `ease_out_layout` | 150 ms | `EASE_OUT` | Remaining rows slide up smoothly via `animate_layout` when a single row is restored/discarded | Subsequent press retargets layout | Instant |
| **17. Focus (Focus Ring & Selection Underline)** | — (Instant) | 0 ms | — | `FOCUS_RING` border color / `SELECTION_UNDERLINE` | Immediate | Immediate |

---

### B. Explicit "Stays Instant" List (0 ms — Must Never Animate)

1. **Typing and buffer edits:** Source editor text insertion/deletion, punctuation dimming scan, dirty dot appearance (`titlebar.rs`), live Preview Markdown recomputation, and Settings text inputs.
2. **Caret, selection, and keyboard stepping:** Editor caret movement, text selection, `Ctrl+P` / `Ctrl+Shift+F` highlight stepping, Guard card Left/Right ring stepping (`notices.rs`), Compare `J`/`K` difference stepping, and Review/Revision `J`/`K` finding/hunk stepping (**including** camera jump via `focus_document_line_instant` and Revision field scroll centering).
3. **Surface, room, and tab switches:** Switching tabs in the rail, switching panes (`Source` / `Preview` / `Split`), switching right-hand fields (`Context` / `Review` / `Revision` / `Preview`), and opening/closing the `Settings` page.
4. **Theme switches:** Light ↔ Dark mode transitions across the entire app (including `prims::press_feedback` zeroing transition duration on appearance change and `glow.rs` decorations updating immediately).
5. **Numbers, counts, and indicators:** Findings count (`N findings`), revision decision pips and `n of m decided`, Agent Context byte/source counts, consent byte totals, and CJK font download megabyte digits (`12.4 / 16.8 MB`).
6. **Focus rings and disabled states:** Keyboard focus rings (`FOCUS_RING`), choice-row active underlines (`SELECTION_UNDERLINE`), and disabled control dimming (`0.45` / `0.5` opacity).
7. **Consent scope switching:** Clicking `Selection` / `Document` / `Document + context` inside the open consent card (must swap the manifest table and reset the hold button in `0 ms` so the target never shifts under the pointer).

---

### C. Ranked Motion & Layout-Stability Fixes to Implement Now

1. **Fix column hot-word dark-flash-and-flare on focus loss (`glow.rs:450-500`)** — *Follows Spec Row 8 (Icon/Mark Swap) & Row 13 (Complete).*
   - **Bug (`motion-b` F1):** When `J`/`K` steps away from a focused finding, `heat_at` routes the previously focused finding through `cooling(now - since)`, which replays `rise()` (`0.0 → 1.0` over `90 ms`) before decaying—causing the word in the editor to drop to `0`, flare white-hot, and then cool.
   - **Fix:** Give `glow.rs` a dedicated `cool_from_hot(elapsed_ms)` curve starting at `1.0`: `1.0 - (1.0 - EMBER) * normalized_cool(elapsed_ms)`, clamped to `[EMBER, 1.0]`. Add a unit test asserting `heat_at(since + 1ms) > 0.95` and monotonic decrease.

2. **Fix `Heat::cool()` jumping to `1.0` when called before `IGNITE_MS` finishes (`motion.rs:307-321`)** — *Follows Spec Row 8 & Row 13.*
   - **Bug (`motion-b` F2):** In `nsh-core 0.6.1`, `AnimatedValue::keyframes` always jumps to frame 0 (`1.0`). Answering a question or pressing `J`/`K` within `90 ms` of ignition snaps a mid-rise emitter to `1.0` before cooling.
   - **Fix:** Track the emitter's last started state/timestamp in `Heat` (or sample the current heat level `from` as `consent.rs::cool(from)` already does at `consent.rs:1434-1449`) and build the 16-stop keyframe track starting from `from` down to `EMBER`.

3. **Remove untokenized 300 ms scroll animation when stepping hunks in Revision (`revision.rs:597-609`)** — *Follows "Stays Instant" Item #2.*
   - **Bug (`motion-b` F3):** `RevealOptions::default()` defaults to `animated: true`, triggering `nsh-core`'s hardcoded `300 ms` scroll animation on every `J`/`K` keypress.
   - **Fix:** Pass `RevealOptions::default().animated(false)` in `revision.rs:603` so keyboard hunk stepping is instant and never fights rapid `J`/`K` repeats.

4. **Fix Rail collapse clipping the exit fade (`rail.rs:142-157, 279`)** — *Follows Spec Row 6 (Resize: Rail Hover Widen).*
   - **Bug (`motion-a` Top 10 #4):** On pointer leave, the rail container width snaps from `240 px` to `44 px` with `overflow_hidden` in the same frame the `160 ms` opacity fade begins, clipping the labels immediately so only the entrance ever animates.
   - **Fix:** Drive the `240 px` expanded strip container through a `Presence`-style visibility signal (keeping width `240 px` while `opacity` fades `1.0 → 0.0` over `RAIL_WIDEN_MS` with `EASE_OUT`, then collapsing width to `44 px` when the fade finishes) while gating `pointer_events` on logical `hovered`.

5. **Wrap narrow-width overflowing rows in Revision and Review (`revision.rs:1025-1049`, `review.rs:517-547, 1017-1061`)** — *Fixes 720×480 overflow (`motion-b` F4, F6).*
   - **Bug:** At `720×480`, the overlay field inner width is `~259 px`, causing the `~313 px` Revision key-hints row (`A Accept · R Reject · J/K Move · Ctrl+Z Undo`) and the `~305 px` Review stale banner row (`Text changed since this review` + `Review again`) to clip/overflow horizontally.
   - **Fix:** Add `.flex_wrap().row_gap(px(SPACE_2))` to the Revision key-hints row (`revision.rs:1025`), Review stale banner row (`review.rs:533`), and Review findings stepper row (`review.rs:1017`).

6. **Reserve WebView status line height to stop 34 px viewport jump on keystrokes (`preview.rs:146-215`)** — *Follows Spec Row 7 (Content Swap).*
   - **Bug (`motion-a` Top 10 #2, #10):** In `Split · Compatibility`, every keystroke triggers a WebView navigation; when a load takes `>100 ms`, inserting the status message above the WebView shrinks the WebView height by `~34 px` and snaps it back when ready.
   - **Fix:** Add `LOAD_LABEL_DELAY_MS = 100` to `motion.rs` (replacing the borrowed `HOVER_MS`); overlay or reserve the status line height and crossfade its opacity with `Presence` (`ENTER_MS` / `EXIT_MS`) so the WebView frame never resizes during navigation.

7. **Animate Compare panel open/close and lock its height during `J`/`K` stepping (`notices.rs:362, 447-527`)** — *Follows Spec Row 3 (Banner & Disclosure Height) & "Stays Instant" #2.*
   - **Bug (`motion-a` Top 10 #3):** Opening/closing `Compare` jumps the editor column below it, and stepping between a 2-line diff and a 12-line diff with `J`/`K` bounces the editor up and down on every keystroke.
   - **Fix:** Wrap the comparison sub-panel in `animate_layout(ease_out_layout(ENTER_MS / EXIT_MS))` for open/close, and give the diff lines container a fixed height (`COMPARE_MAX_LINES` rows, scrollable if needed) while open so `J`/`K` stepping never reflows the source editor below.

8. **Synchronize Review field stale transition with the column's `AFTERIMAGE_MS` drain (`review.rs:1559-1633`, `glow.rs:139-146`)** — *Follows Spec Row 14 (Error / Stale Drain).*
   - **Bug (`motion-b` F5):** On the first edit after a Review, the field snaps to `afterimage` ink in `0 ms` while the column's highlighted words drain over `1200 ms` (`AFTERIMAGE_MS`).
   - **Fix:** When `review.stale` flips to `true`, drive the field's `Emitters` to `0.0` over `AFTERIMAGE_MS` (or `REDUCED_CROSSFADE_MS` under reduced motion) so the field and the document drain together as one light source.

9. **Apply `group_hover` + `motion::ease_out` to button labels and menu rows (`prims.rs:170-214`, `titlebar.rs:726, 771`, `rail.rs:328`)** — *Follows Spec Row 11 (Hover).*
   - **Bug (`motion-a` Top 10 #7, #8):** Button hover ink declared on parent views never reaches child labels with explicit `text_color`; `titlebar.rs` uses `nsh` default `ease-in-out` instead of `motion::ease_out`; `rail.rs` label hovers snap in `0 ms`.
   - **Fix:** Move hover ink transitions onto the child `text` nodes via `.group_hover(...)` with `motion::ease_out(text.transition_colors().duration(HOVER_MS))` in `prims.rs`, `titlebar.rs`, and `rail.rs`.

10. **Stabilize Settings Base URL validation note and CJK Font disclosure height (`settings.rs:254-268, 745-800`, `fonts.rs:145`)** — *Follows Spec Row 3 & Token Table Rule.*
    - **Bug (`motion-a` Top 10 #6, #9, #10):** Typing in `Base URL` flips the note beneath it between a 2-line hint and a 1-line error, shifting the entire form down and up by `18 px` per keystroke; clicking `Download and apply` collapses the 3-line CJK font disclosure, jumping the action row up `~54 px` under the pointer.
    - **Fix:** Give the Base URL note container `.min_h(px(2.0 * TYPE_UI_LINE))` so valid↔invalid flips never move the form below; keep the CJK font disclosure visible during `Downloading` / `Applying` so the action buttons stay in a fixed position; move `PROGRESS_TICK_MS = 150` from `fonts.rs:145` into `motion.rs`.

11. **Drive the Rail's live-model dot from `Heat` (`rail.rs:226-250`)** — *Follows Spec Row 8 (Icon/Mark Swap).*
    - **Bug (`motion-a` Top 10 #5):** `motion.rs:228-232` specifies that the rail's mark interpolates `Heat`, but `rail.rs` snaps between flat colors on boolean `holds_live_model_output`.
    - **Fix:** Bind the rail dot's glyph/bloom color to a `Heat` instance that calls `ignite()` when a result arrives on that tab and `extinguish()` when closed or drained.

12. **Repaint editor glow decorations immediately on OS theme switch (`glow.rs:526-552`)** — *Follows "Stays Instant" Item #4.*
    - **Bug (`motion-b` F8):** When the OS switches between Light and Dark while a Review result is resting, the editor repaints in `0 ms` except for the glow-annotated words, which lag in the old palette for up to `250 ms` (`HEAT_WATCH_MS`).
    - **Fix:** Read `tokens::is_dark()` inside a reactive `effect` in `glow.rs` so an appearance flip triggers an immediate decoration pulse on the same frame.

13. **Clamp Consent and Guard card width on narrow windows (`tokens.rs:285`, `consent.rs`, `notices.rs`)** — *Fixes narrow window geometry (`motion-b` §c).*
    - **Bug:** `CONSENT_CARD_MAX_W` (`640 px`) is applied as a fixed `.w(px(640.0))` rather than `.w_full().max_w(px(640.0))` with horizontal margin.
    - **Fix:** Use `.w_full().max_w(px(CONSENT_CARD_MAX_W)).mx(px(SPACE_5))` on the card wrapper and set a minimum window size of `720×480` on the native window in `app.rs`.

---

## 5. What Not To Do (Rejected Audit Proposals)

1. **Reject adding a confirmation dialog or `"Discard review"` label to the Review `Close` button (`ux-flows` #10).**
   - *Why:* Closing a side panel is a routine navigation action. Guarding panel closes with confirmation prompts causes confirmation fatigue and violates Principle A (*"never interrupt what the user is doing"*). Keeping `review_view` in session state (UX Decision #1) makes closing non-destructive without adding friction.

2. **Reject adding confirmation modals for Recovery `Discard`, Settings `Remove` credential, and `Clear counts` (`ux-shell` #16 modal option, `ux-settings` F1, F7 modal options).**
   - *Why:* Popping up a modal card to confirm deleting a crash checkpoint or clearing local usage counters adds a two-step interruption to secondary maintenance actions. An immediate action paired with a 5-second inline `Undo` status notice (Section 2, Tier 2) protects against stray clicks with zero modal interruption.

3. **Reject adding an explanation legend or hover tooltips to editor gutter ticks (`ux-settings` F15).**
   - *Why:* Look at Screenshot 5 (`en-us-review-result-questions-default-light`): the Review field already renders a clean, self-explaining inline legend (`words Stated in the text | Inferred`) directly above the findings count. Adding a second legend at the top of the panel or hover popups on 2 px gutter ticks duplicates existing UI and violates the Afterglow principle that light in the margin is felt, not turned into a dashboard.

4. **Reject rendering all 16 unused `*Help` strings on the Settings page (`ux-settings` F4).**
   - *Why:* Look at Screenshot 9 (`en-us-settings-default-dark`): the Settings page is calm and scannable because it avoids walls of explanatory prose. Rendering obvious explanations like `LightThemeHelp` (*"Used whenever the effective mode is light"*) or strings for unmounted legacy controls (`IncludeGlobalSkillsHelp`, `ShowInternalSkillsHelp`, `GroupByHelp`, `SyncScrollingHelp`, `AutoRefreshHelp`) would turn Settings into a manual. Render **only** `ProviderHelp` and `ModelHelp` (which explain non-obvious fallback behavior), and **delete** the remaining 14 dead `*Help` keys from `i18n.rs`.

5. **Reject widening the Rail on keyboard focus or making background-tab `×` buttons permanently focusable in the tab order (`ux-shell` #17).**
   - *Why:* If every background tab in the 44 px rail has a second focusable `×` stop, pressing `Tab` to move from the titlebar to the editor requires tabbing through `2 × N` controls. Instead, keep one focus stop per rail tab dot and bind `Delete` / `Middle-Click` / `Ctrl+W` on a focused rail dot to close that tab.

6. **Reject keeping a background WebView instance mounted per open tab (`motion-a` F2 / Runner-up #9).**
   - *Why:* On Windows, each mounted WebView spawns a multi-process WebView2 browser tree consuming 80–150 MB of RAM. Retaining one WebView2 instance per open Markdown tab would blow up memory footprint for a compatibility-only preview mode (`PRODUCT.md` role: *Compatibility*).

7. **Reject rewriting `glow.rs` from a stepped clock to per-span `AnimatedValue` bindings now (`motion-b` F9).**
   - *Why:* `nsh-base` editor decorations take resolved `Color` values, not `AnimatedNode` bindings. Replacing that architecture is high-cost (`L`) and speculative until a production-profile timing probe on a large document demonstrates actual frame drops.

8. **Reject animating the Review footer button swap (`Prepare revision` quiet → primary) or the 1-second `"Usually 10–30 seconds"` hint (`motion-b` M25, F10).**
   - *Why:* Both occur at rare task boundaries where the user's eye is on the question input or the endpoint host line. Adding bespoke color-interpolation scaffolding to a button variant swap adds code complexity with negligible perceptual benefit.
