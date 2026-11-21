# Goal 15: Scripts always run; documents never reach the network

## Objective

Owner decision, 2026-11-21: the per-tab **Trust scripts → Run scripts** flow is removed. Every
document in the web layouts (Markdown, MDX, HTML) runs its scripts from the first frame, with no
button, banner or prompt. In exchange, no document can reach the network or the file system at any
time. That keeps the PRODUCT.md guardrail "zero unconsented outbound requests", which is not waived.

## Done when

1. **One web mode.**
   - `web::Trust`, `allows_scripts`, `to_file_url`, the Restricted/Trusted banner, the
     **Trust scripts** button, the **Run scripts in this document?** alert, and the Revision-time
     trust revocation are gone.
   - No code path keeps a per-tab script or trust state.
   - `git grep -nE "Trust::|allows_scripts|to_file_url|Run scripts|Trust scripts" crates/` is empty.
2. **One page policy for all three types.**
   - `web::build_html` serves Markdown, MDX and HTML as a `data:`/string page with no base URL.
   - Each page carries one CSP: `default-src 'none'; script-src 'unsafe-inline'; style-src 'unsafe-inline'; img-src data:; font-src data:`.
   - An HTML file keeps its own document shell and styles. The injected `<meta>` CSP is the first
     element of `<head>`, before any author markup. A document whose source has no `<head>`, or
     only a partial one, still gets the CSP before its first script.
   - An author's own CSP can only tighten the policy.
   - Unit tests cover all three types, plus HTML sources with no head, with an uppercase head,
     with a `<head>` inside a comment, and with a leading `<script>`. Each test asserts that the
     CSP precedes the first author byte that could execute or load anything.
3. **No top-level escape.**
   - The WebView's nsh 0.6.1 capability grants no external hosts (`webview.hosts: []` or the
     current equivalent; verify it in the locked nsh source).
   - A page script or link that sets `location`, submits a form, follows a link or calls
     `window.open` to any `http(s):`, `file:` or other URL leaves the page where it is.
   - Nothing is opened in another app automatically. A page script can forge any bridge message,
     so the app does not trust a page's request to open a URL.
   - Tests prove the configured capability denies `https://example.com/` and `file:///C:/`.
   - The native check (clause 6) proves that a real page cannot navigate away.
4. **Scripts run without a click.** A headless or unit test shows that an MDX component, or a
   browser-side diagram that needs scripts, is in the built page for a freshly opened tab, with no
   preceding action.
5. **Docs say one thing.** The README "Security" section, `docs/architecture.md` (the trust table
   around lines 150 and 405–430, and line 984) and CONCEPTS.md (around line 156) describe the single
   policy. That policy is scripts on; no network, file or navigation access; and HTML pages that
   need remote scripts or relative assets do not load them. PRODUCT.md needs no change unless it
   names Trust.
6. **Harness and evidence.**
   - The Trust/Run-scripts steps in `scripts/markturbo_tools/native/goal07.py`, its system-alert
     handling in `runtime.py`, and `scripts/tests/test_native_goal07*.py` are retired or replaced.
   - goal07 asserts instead that a fixture with an inline script shows the script's effect, and
     that the fixture's attempt to navigate to an external URL leaves the page in place.
   - The fixture uses no network and logs no content.
   - `mt.py check ci` and `mt.py check native` pass. `mt.py accept all` passes on the release build
     (pending the owner's desktop; it is not waived).

## Failure control

- Any outbound request or `file:` load from document content, at any time.
- A navigation that leaves the app's page.
- Any remaining per-tab trust state or UI.
- A page built without the CSP ahead of the first author markup.

## Out of scope

- Opening links in the system browser. It needs a trusted user gesture that a page script cannot
  forge, so it is a separate owner decision.
- Relative images and stylesheets in HTML files.
- Remote or CDN scripts. They stay blocked by design.

## Evidence plan

- Unit tests in `web.rs` cover the CSP placement table and the capability.
- A headless test covers clause 4.
- goal07 native acceptance covers navigation and the script's effect.
- The PR notes record the before and after README Security text.

## Mergeable alone

Yes, on top of Goal 14 (nsh 0.6.1).
