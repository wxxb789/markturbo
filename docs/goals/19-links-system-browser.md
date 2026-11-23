# Goal 19 — Preview links open in the system browser

## Objective

Owner decision (2026-11-22): a link the user follows in the web preview opens in the system's default browser,
with no confirmation dialog. The owner accepts the residual risk that a page script can forge a click, so a
document can make the browser open a URL. The preview itself never navigates and never gains network access.
This goal amends Goal 15 clause 3, "Nothing is opened in another app", for this one action only.

## Done when

1. **Behaviour.** Activating an `http:`, `https:` or `mailto:` link in the web preview (Markdown, MDX or HTML,
   including `target=_blank`) opens that exact URL in the OS default handler, once per activation. The preview
   page stays exactly where it was. Every other scheme (`file:`, `javascript:`, `data:`, custom schemes,
   relative URLs without a base) does nothing.
2. **Channel.** Choose the narrowest channel that the locked nsh 0.6.1 source actually offers, and verify it in
   the locked source before coding. Candidates: a WebView navigation-starting or new-window event that the
   host cancels, or a one-way message carrying only the URL. Then:
   - the host validates the scheme and parses the URL with the `url` crate (or an existing dependency);
   - the host never loads the URL in the WebView;
   - the host acts on no other message shape.

   Constraints: do **not** add `allow-popups`, `allow-top-navigation*`, `allow-same-origin` or `allow-forms` to
   the sandbox, and do not loosen the CSP or the network kill switch. If no channel exists within these
   constraints, **stop and report** with the evidence; do not invent one.
3. **Opening.** Use one OS-opening path: an existing dependency if one is present, otherwise the platform call,
   with no shell string interpolation. Make it injectable, so tests never launch a browser.
4. **Rate limit.** At most one open per 500 ms. A burst of forged activations opens one URL, not a flood.
5. **Tests.**
   - Headless or unit tests: a scheme allow/deny table, URL validation, exactly one open per activation, the
     rate limit, and that the preview does not navigate.
   - A test proving that no other message shape triggers any action.
6. **Docs.**
   - README Security and `docs/architecture.md` describe the link behaviour and the accepted forged-click risk.
   - Goal 15 clause 3's "Nothing is opened in another app" points to this goal.
   - Record native acceptance (clicking a link on the desktop opens the browser) as pending in
     `docs/goal-19-acceptance.md`. It is not a pass.

## Failure control

- Any request from the preview itself.
- Any sandbox or CSP loosening.
- An open without a user-visible link activation, apart from the accepted forged-click risk.
- A shell-interpolated open.
- A test that launches a real browser.

## Out of scope

- A confirmation dialog (the owner declined one).
- Links in the native (non-web) preview.
- The nsh popup gap (longbridge/nsh#2785, waiting for upstream).

## Evidence plan

Unit and headless tests; `mt.py check ci`; native acceptance deferred to the owner's desktop.

## Mergeable alone

Yes, on `main`.
