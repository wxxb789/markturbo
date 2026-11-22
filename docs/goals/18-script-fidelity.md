# Goal 18 — Script preservation fidelity follow-up

## Objective

Goal 15 shipped with three rendering-fidelity findings, deferred by the owner on 2026-11-22 (PR #53
disposition). None lets a request leave the page. Close them without adding a new parsing layer, and keep
Goal 15's guarantees unchanged: the sandboxed frame, the CSP, the kill switch, the in-page layers, and the
clause-4 rule "which scripts run follows the source grammar".

## Done when

1. **`foreignObject` raw text.** The lexical `<link>` neutralization in `crates/mt-app/src/web.rs` treats
   HTML content inside `<svg><foreignObject>…</foreignObject></svg>` as HTML. A `<script>` there is raw text
   and is left byte-for-byte; for example, a regex `/<link>/` still runs. A real `<link>` inside
   `foreignObject` is still made inert. The pass stays single, linear and parser-free.
2. **No reinterpretation of neighbouring HTML.** In one CommonMark type-1 HTML block that holds scripts plus
   other HTML, the other HTML renders as HTML, never as code. Example: `<script>void 0;</script >`, then an
   indented `<div>Keep me</div>`, then `<script>void 1;</script>`. Placeholders must not change how the
   Markdown compiler classifies the block's lines.
3. **Examples stay examples.** A `<script>` inside raw-text or escapable-raw-text content (`<textarea>`,
   `<title>`, `<xmp>`, `<noscript>`, and so on), or inside an HTML comment, is never collected for
   restoration and never executes.
4. **Degradation is unchanged.** A script that cannot be restored exactly stays in the tag-filtered output
   and does not run. The rest of the document renders normally, and no diagnostic replaces content.
5. **Tests.** Each clause gets a unit test that fails when its fix is reverted (vacuity sweep), plus the
   three reviewer inputs from PR #53. The existing linear-time tests still pass.

## Failure control

- Any new request path, or any loosening of the CSP, the sandbox tokens or the kill switch.
- A new HTML parser or tokenizer dependency.
- A whole-batch diagnostic for a script restoration failure.

## Out of scope

- Opening links in the system browser (Goal 19).
- Scripts outside the clause-4 grammar boundary.

## Evidence plan

Unit tests in `web.rs`; `mt.py check ci`. Native acceptance is unchanged by this goal.

## Mergeable alone

Yes, on `main`.
