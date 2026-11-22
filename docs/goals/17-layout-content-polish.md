# Goal 17 — Layout and content polish, with CJK optical sizing

## Objective

This goal finishes the "Crisp afterglow" design upgrade with the layout and content items from the
design audit. Every number below is a design decision. Read values from `ui::tokens`; do not
invent new ones.

## Done when

1. **Tabular figures.**
   - Add a `tokens` font-feature token for tabular numerals (`tnum`). Use the nsh 0.6.1 text API;
     verify it exists in the locked source.
   - Apply it to every changing count:
     - consent byte counts;
     - "n of m decided";
     - the findings count;
     - context bytes;
     - status-line byte counts.
   - If nsh cannot set OpenType features, report that and use the `Martian Mono` family for those
     figures instead.
   - A test asserts the style on each listed text.
2. **Settings.**
   - Anchor Save and Close to the field column's right edge.
   - Make the page scroll, so the last section is never clipped at the 900px-tall window.
   - Mark the selected choice with ink plus a 2px underline in `palette().ink`.
   - Align rows that are offset by 3px onto the 4px grid.
   - Headless tests: the action buttons share the column's right x, within 0.5px; the last section is
     reachable by scrolling; the selected choice has the underline.
3. **Review pane.**
   - Remove the vertical dead bands, about 200px, so that no empty band is taller than
     `tokens::SPACE_XL` between sections.
   - Hide "J / K to step through" when there are 0 findings.
   - Left-align all section edges.
   - Place "Prepare revision" in the same row group as Close.
   - Write headless tests for each.
4. **Light-mode ember.** Raise the chroma of the light-mode ember (today `#7A6A5C`) so that cooled
   model text is visibly warm. Target OKLCH chroma ≥ 0.06 at the same lightness, ±0.02. Keep WCAG
   AA contrast (≥ 4.5:1) against the light background. A unit test computes both.
5. **Missing states.** Each state gets a designed rendering:
   - the review body when it is empty;
   - the preview when it is empty;
   - the preview while it is loading;
   - a labelled diagram failure (the diagram kind and "could not render", never a blank box);
   - **Cancel while revising**, which stops the revision request and restores the pre-revision
     state.

   Write headless tests for each.
6. **Status line.**
   - Colour it by kind: error is `danger`, success and info are neutral.
   - Show it beside the surface that produced it (the review pane, settings, or the editor
     column), not in a fixed global spot.
   - Headless tests for colour and placement.
7. **CJK optical sizing** (owner: yes, decide from captures).
   - Add one token for a per-script size step and weight for CJK runs. The default is unchanged, so
     today's rendering is preserved until the owner picks.
   - Provide variants for capture: +0, +1px, and +1px with the next companion weight.
   - Add a `mt.py visual` capture recipe that renders the same mixed CJK/Latin paragraph with each
     variant, side by side. **Capturing is pending the owner's desktop**; the owner picks a
     variant, and a follow-up sets it as the default.
   - Unit-test that the token reaches CJK runs only.
8. **No regressions.** `mt.py check ci` passes. Native acceptance, the visual matrix and the
   before/after captures of settings, review and light-mode ember are **deferred by the owner**.
   Record them in `docs/goal-17-acceptance.md` as pending; they are not passes.

## Failure control

- A value that does not come from `ui::tokens`.
- A clipped settings section.
- A blank diagram failure.
- Contrast below AA.
- Any motion change. Goal 16 owns motion; do not alter it.

## Out of scope

- Motion and interaction states (Goal 16).
- Script and WebView security (Goal 15).
- Opening links in the system browser (a later goal).

## Mergeable alone

Yes, after Goal 16. This branch is stacked on `feat/crisp-afterglow-motion`.
