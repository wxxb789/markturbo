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
   - `git grep -nE "\bTrust::|allows_scripts|to_file_url|Run scripts|Trust scripts" crates/` is empty (`ProjectTrust` is a different concept and stays).
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
3. **No escape: sandboxed document frame.**
   - Amended 2026-11-21 after implement run 1 found that nsh 0.6.1 lets a live page's
     `window.open` through to WebView2's default popup (`nsh-platform-windows`
     `web.rs:991` handles `NewWindowRequested` only after the page closes), and that CSP does not
     govern navigation.
   - The document is never the top-level page. `web::build_html` returns a fixed **host page**
     that carries the clause-2 CSP plus `frame-src 'self'` or the narrowest value that still
     admits `srcdoc`. The host page holds exactly one `<iframe sandbox="allow-scripts">`, whose
     `srcdoc` is the document page from clause 2 and which fills the viewport.
   - The sandbox has no `allow-popups`, no `allow-top-navigation*`, no `allow-forms`, no
     `allow-same-origin` and no `allow-modals`. So `window.open`, a form submit, `top.location`
     and a `target=_top` or `_blank` link do nothing. The frame's own navigation to `http(s):` or
     `file:` is refused by the inherited CSP (`frame-src`/`default-src`).
   - The host page runs no script of its own.
   - Also configure the nsh 0.6.1 WebView capability to grant no external hosts, as defence in
     depth. Verify the API in the locked nsh source; if no such API is reachable from the app, say
     so and skip it.
   - Nothing is opened in another app. A page cannot forge a request the app would act on: the
     app reads no bridge messages.
   - Unit tests on the built host page assert: the exact sandbox token set (and that none of the
     forbidden tokens appear); that the CSP comes first; that `srcdoc` is attribute-escaped, so a
     document containing `"` and `</iframe>` cannot break out; and that the document's own CSP
     comes first inside `srcdoc`.
   - Native proof (clause 6): a fixture paints its attempt cells and then makes its one HTTPS
     self-navigation. The fixture is visible before that navigation and gone afterward, with zero
     loopback contacts and exactly one app window.
   - If WebView2 rejects any part of this design in the native check, stop and report it. Do not
     loosen the sandbox.
   - Existing preview behaviour must survive inside the frame: scrolling, theme colours, math and
     diagram rendering, and text selection.
3b. **Process-level network kill switch** (amended 2026-11-21 after the review found that CSP and
   the sandbox do not cover WebRTC/ICE, DNS prefetch or similar channels).
   - nsh 0.6.1 creates the WebView2 environment with `options = null`
     (`nsh-platform-windows` `web.rs`, `create_environment`), so the loader honours
     `WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS`. Before any web view is created, the app sets it, in
     its own process only, to arguments that leave the WebView's browser process **no route to
     any network**. Candidates:
     - a host-resolver rule that maps every name to NOTFOUND;
     - a proxy that points at a closed loopback port, with no bypass;
     - a WebRTC IP-handling policy that forbids non-proxied UDP.
   - Verify each flag against current WebView2/Chromium behaviour rather than assuming it, and
     keep only what the native proof needs.
   - If the variable is already set in the user's environment, the app overrides it for its own
     process; it does not merge with it.
   - The app's own non-WebView networking (model transport, font download) is unaffected; it does
     not run inside WebView2.
   - Native proof (clause 6). The harness listens on loopback TCP and UDP ports. A fixture
     attempts, against those ports and against a hostname:
     - `fetch`, `<img>`, `sendBeacon` and `WebSocket`;
     - a `RTCPeerConnection` with a STUN/TURN server and a data channel;
     - `<link rel=dns-prefetch/preconnect>`;
     - a form post.

     The listeners must record **zero** contacts. Unit tests assert the exact argument string and
     that it is set before the first web view.
3c. **The web preview is never turned off; close the non-browser paths** (owner decision,
   2026-11-21: the WebView preview is the core experience on every platform and must not be
   disabled for security).
   - **Where the process-level switch is unavailable**, the WebView still runs with scripts on,
     and the in-page layers carry the guarantee. Those places are: Windows at high integrity level
     (WebView2 ignores `WEBVIEW2_*` variables there), macOS (WKWebView) and Linux (WebKitGTK).
     The in-page layers are:
     - CSP and the sandboxed frame;
     - a prelude, ahead of all author code, that removes the WebRTC constructors and prototypes
       (`RTCPeerConnection`, `webkitRTCPeerConnection`, `RTCDataChannel`, `RTCIceTransport`,
       `RTCRtpSender`/`Receiver`, `RTCSctpTransport`, and so on) from the frame's realm, so author
       code cannot reach them. No fresh realm is available: nested frames and blob/data workers
       are refused by the CSP;
     - `<meta http-equiv="x-dns-prefetch-control" content="off">`.

     On Windows, the process-level switch remains the primary layer wherever WebView2 honours it.
     Document the residual risk of these platforms and modes in the README and architecture
     security text. Do not disable the preview.
   - **PlantUML.** It runs outside the browser, so the WebView switch cannot cover it. The renderer
     starts PlantUML with the `SANDBOX` security profile (the `PLANTUML_SECURITY_PROFILE`
     environment variable or the Java system property), so a diagram cannot read files or URLs.
     Test the child's command or environment; it covers both the native and web previews.
   - **In-frame navigation.** A user clicking a link, or submitting a form, in the preview leaves
     the page exactly where it is. A tiny capture-phase prelude in the frame, injected ahead of
     author markup, cancels anchor and form default navigation. A script that deliberately
     navigates the frame may land on the browser's blocked page. That is acceptable, because no
     request leaves; the next document update restores the preview.
   - **Size.** An HTML document whose page exceeds the WebView's `NavigateToString` limit shows a
     visible diagnostic instead of a blank or failed preview. Escape only what a double-quoted
     attribute needs (`&` and `"`), to keep the inflation minimal.
   - **In-frame navigation, refined.** The Navigation API does not fire in an opaque-origin
     (sandboxed) frame, so it is not used. The capture-phase click and submit guard covers links and
     forms in the document and in open shadow roots. A link inside a **closed** shadow root exists
     only because the document's own script built it, so it counts as script-driven navigation:
     it may land on the blocked page, and no request leaves.
   - **What native acceptance cannot see.** DNS egress for a hostname is not observable on
     loopback. The resolver rule's exact string, and the fact that it is applied before the first
     web view on the production entry path, are proven by unit tests; the native proof covers
     IP-literal channels.
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
   - goal07 first observes that the inline script ran and painted all attempt cells. A later
     capture must show that the final HTTPS self-navigation replaced the fixture, with zero
     loopback contacts and exactly one app window.
   - The fixture targets only loopback or reserved names and logs no document content.
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
