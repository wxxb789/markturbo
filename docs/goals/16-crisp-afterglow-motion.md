# Goal 16 — Crisp afterglow: interaction states and motion tokens

## Objective

The owner chose direction B, "Crisp afterglow" (not in git; read-only reference: `Q:\repos\markturbo\.scratch\design-upgrade\proposal.md` and `prototype.html` beside it). Under it,
every pressable control answers a press the moment it goes down. Occasional surfaces arrive and
leave with short ease-out motion. Frequent and keyboard-driven changes stay instant. All motion is
built on nsh 0.6.1's native transitions and layout animation, and every value comes from one token
table.

## Done when

1. **One token table.** `crates/mt-app/src/ui/motion.rs` owns every duration and curve that the UI
   uses. It adds at least:
   - `EASE_OUT` = `cubic-bezier(0.23, 1, 0.32, 1)`;
   - `ENTER_MS` = 200;
   - `EXIT_MS` = 150;
   - `MENU_ENTER_MS` = 125;
   - `MENU_EXIT_MS` = 90;
   - `PRESS_RELEASE_MS` = 120;
   - `SCALE_FLOOR` = 0.96.

   `RAIL_WIDEN_MS` is defined once, and `consent::CARD_RISE_MS` moves into the table. A test scans
   `crates/mt-app/src/ui/**` and fails if it finds a numeric `.duration(…)` or an
   `Easing::Bezier(…)` literal outside `motion.rs`.
2. **Frequency map, as an exhaustive table.** Every surface below has exactly one motion row. Rows
   marked *instant* must not animate, even when motion is on.

   | Surface | Trigger | Enter | Exit |
   | --- | --- | --- | --- |
   | Typing, scrolling, rail switch, tab switch | frequent / keyboard | instant | instant |
   | Field swap (preview / context / review / revision) | frequent | instant | instant |
   | File palette (Ctrl+P) | keyboard | instant | instant |
   | Settings page swap | frequent | instant | instant |
   | Title-bar menu | pointer | fade + scale 0.97→1, `MENU_ENTER_MS`, origin top-right | fade, `MENU_EXIT_MS` |
   | Recovery/conflict banners | occasional | height + fade, `ENTER_MS`; content below moves with it, no jump | `EXIT_MS` |
   | Consent card | occasional | scrim fade `ENTER_MS`; card scale `SCALE_FLOOR`→1 plus a 4px rise | `EXIT_MS` |
   | Close/quit guard | occasional | as consent | as consent |
   | Status notice | occasional | 4px rise + fade, `ENTER_MS`; hold 3000ms | fade, `EXIT_MS` |
   | Findings arriving (model output) | model | the existing Heat ignite; one entrance per list, no stagger | — |
   | Theme switch | occasional | 200ms colour cross-fade | — |

3. **Interaction states.**
   - **Pressed state.** Every pressable gets one, shown on press-down through nsh `.active(…)`. It
     is an immediate ink or plate step with no scale on text controls, and it is released over
     `PRESS_RELEASE_MS`. One `prims` helper supplies it, and every pressable in `ui/` uses that
     helper.
   - **Hover state.** Every pressable without one gets the existing 100ms colour hover: rail dots,
     review J/K step controls, cooled question lines, revision hunk rows, guard buttons, and the
     consent hold button.
   - **Disabled state.** An action that is unavailable stays in place, dimmed through
     `disabled_style`, instead of being removed. This covers Accept (`revision.rs`) and Review
     (`titlebar.rs`).
   - A headless test asserts the pressed style on press-down for one control of each kind
     (quiet action, plate button, row).
4. **Focus returns to the editor (owner decision, 2026-11-21).** On nsh 0.6.1 Windows, clicking
   a pressable control takes keyboard focus. When an editor is open, toolbar-type actions that do
   not open a card, menu or input hand keyboard focus back to the editor after they act: title-bar
   Review, the filename, rail dots, and field close buttons. Controls inside cards, menus, dialogs
   and Settings keep normal focus behaviour.
   - A headless test presses Review (or another toolbar action), types a character, and asserts
     that the editor received it.
5. **Reduced motion.** With Windows "Animation effects" off (`reduce_motion()`), nothing translates
   or scales: an enter or exit is only an opacity change of at most 100ms. A headless test sets
   reduced motion and asserts that no transform transition is configured on the consent card or a
   banner.
6. **Interruptible.** Re-triggering a surface mid-transition, such as closing the menu while it
   opens, retargets from the on-screen value. It never jumps to the end or restarts from zero. One
   headless test covers this for the menu.
7. **No regressions.** `mt.py check ci` passes. The native acceptance and the 22-state visual
   matrix pass on the release build, with before/after captures of settings, consent and the title
   menu kept under `.scratch/`.

## Failure control

- Any animation on a row marked *instant*.
- A duration or curve literal outside `motion.rs`.
- An ease-in curve on an entrance; a scale below 0.96; a surface that travels more than 8px. This limit applies to surfaces entering or leaving. It does not apply to the editor viewport gliding to the anchor of newly arrived model findings, which is the one place where motion is meant to be felt.
- Input blocked during a transition.
- A control that gives no feedback on press-down.

## Out of scope

- Layout and content polish: settings/review spacing, ember contrast, empty states. That is
  Goal 17.
- CJK optical sizing (an owner decision).
- Springs, stagger and slides. These belong to direction C, which the owner rejected.

## Authoring

The conductor owns the design decisions in this contract and accepts the visuals. A worker implements it. Do not touch the web pane's Trust banner, `web.rs` or `state.rs` trust code: Goal 15 removes them in parallel.

## Mergeable alone

Yes, once Goal 14 (nsh 0.6.1) and Goal 15 (scripts always on) are on main.
