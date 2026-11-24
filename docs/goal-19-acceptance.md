# Goal 19 implementation evidence

Contract: [Preview links open in the system browser](goals/19-links-system-browser.md).
The implementation stage does not publish a PR and does not claim native acceptance.

## Locked-source inspection

The channel was chosen from the locked nsh 0.6.1 crate sources before coding.
Paths are relative to those crate roots on this host.

| Check | Source and result |
|---|---|
| Message channel | `nsh-core/src/element/host.rs:1696` offers `WebView::on_message` (`EventKind::Message`); `nsh-core/src/element/events.rs:512` reads its `data` string. |
| Bridge injection, Windows | `nsh-platform-windows/src/native/web.rs:183,841` installs the bridge with `AddScriptToExecuteOnDocumentCreated`; the WebView2 reference documents that script as running on all top-level document and child frame navigations. |
| Bridge injection, Linux and macOS | `nsh-platform-linux/src/native/web.rs:401` uses `UserContentInjectedFrames::TopFrame`; `nsh-platform-macos/src/native.rs:2483` passes `forMainFrameOnly: Bool::YES`. The sandboxed document frame cannot reach the bridge there, so the frame relays through the host page. |
| Navigation event | `nsh-platform-windows/src/native/web.rs:868` cancels a navigation only from its own capability check and emits `LoadStart` afterwards; no app-facing callback can cancel or divert a navigation the app hears about. Not a channel. |
| Popup event | `nsh-platform-windows/src/native/web.rs:991` marks a popup handled only once the preview owner is gone. Not an app channel. |
| Open path | `nsh-core/src/open_url.rs` (`check_url`, `MAX_URL_LEN`); `nsh-core/src/element/ambient.rs:701` is the app-facing call; Windows opens with `ShellExecuteW` (`nsh-platform-windows/src/system.rs:708`), with no shell string. |

## Implementation

- `web.rs` (page builder). The frame prelude's capture `click`/`auxclick`
  guard now cancels every anchor activation and forwards the raw `href` only
  when its scheme is `http`, `https` or `mailto`. The `submit` guard is
  unchanged. The host page carries one inline relay script after the frame
  (`HOST_RELAY`); it forwards only strings whose `event.source` is the frame
  window to `window.nsh.postMessage`. No sandbox token, CSP or network argument
  changed.
- `links.rs` (host side). `target` parses the message with the `url` crate,
  caps it at nsh's `MAX_URL_LEN`, refuses whitespace and control characters,
  and allows only the three schemes (`http`/`https` need a host). `Opener`
  opens at most once per 500 ms from a monotonic `Instant`; a denied message
  does not start the interval. Production opens through
  `nsh_core::element::open_url`; tests inject the opening closure and headless
  runs hit the nsh platform stub (`Screen::opened_urls`), so no test launches a
  browser.
- `preview.rs` registers `links::preview_message` as the WebView's only
  `on_message` handler. `main.rs` narrows `capabilities.openUrl` to the three
  schemes as defence in depth.
- `goal07.py`'s programmatic link click now uses `file:///goal07/link`; the
  Goal 15 isolation run must not launch a browser at its counted loopback
  listener. The user-visible open is the pending check below.

## Failure-first and vacuity evidence

Each new assertion was checked by reverting its feature and watching the test
fail (Windows host, `--profile ci`):

| Reverted change | Failed tests |
|---|---|
| Scheme allowlist and 500 ms interval disabled in `links.rs` | `target_denies_every_other_scheme_and_relative_reference`, `no_other_message_shape_opens_anything`, `a_burst_opens_once_and_a_later_activation_opens_again`, `a_denied_message_does_not_start_the_interval` (4 failed). |
| `.on_message(crate::links::preview_message)` removed from `preview::web` | `a_link_message_opens_once_and_the_preview_page_stays` (`left: []`, `right: ["https://example.com/from-link"]`). |
| `HOST_RELAY` removed from the host page | `the_host_carries_exactly_one_frame_and_the_frame_relay`, `the_host_relays_only_strings_sent_by_the_document_frame` (2 failed). |

The files were restored to the graded revision after each revert; the table
below is from the restored revision.

## Gates

Serial on the Windows 11 x64 host. No measurement harness ran during the test
suites.

| Command | Exit | Result |
|---|---:|---|
| `cargo fmt --all -- --check` | 0 | No output. |
| `cargo test -p mt-app --profile ci` | 0 | 277 passed, 0 failed, 0 ignored across 42 test targets. Includes the 7 `links::` tests, the 10 `ui::preview::` tests and the 52 Web page tests. |
| `uv run --locked --project scripts scripts/mt.py check fast` | 0 | 234 tooling tests passed across 8 shards in 19.8 s, including the 85 `test_native_goal07` tests. |
| `uv run --locked --project scripts scripts/mt.py check ci` | 0 | Tooling 234 passed in 15.3 s; `cargo fmt --all -- --check` clean; workspace clippy with the 3 pre-existing warnings (2 `too_many_arguments` in mt-core, 1 `clone_on_copy` in `ui/shell.rs`); `cargo test --profile ci --workspace` 1114 passed, 0 failed, 7 ignored across 51 targets. |

## Pending native checks

Not a pass. On the owner's active, unlocked Windows 11 x64 desktop with the
release build:

- In a Markdown, MDX and HTML document's Web preview (column and field), click
  an `https:` and an `http:` link, one of them `target=_blank`: the exact URL
  opens once in the default browser, and the preview stays at the same
  position without navigating.
  **Partly done 2026-11-24:** in Markdown, an `https:` link and an `http:`
  `target=_blank` link each opened once in the default browser and the preview
  did not navigate; an `https:` link also opened from an HTML and an MDX
  document. The preview mode (column or field) was not recorded, so the other
  mode stays pending (see
  [Desktop acceptance 2026-11-24](#desktop-acceptance-2026-11-24)).
- Click a `mailto:` link: the system's default mail handler opens.
  **Done 2026-11-24:** the mail handler opened (see
  [Desktop acceptance 2026-11-24](#desktop-acceptance-2026-11-24)).
- Activate `file:`, `javascript:`, `data:`, a custom scheme and a relative
  link: nothing opens, the preview does not navigate, and no request leaves.
  **Partly done 2026-11-24:** nothing opened and the preview did not
  navigate; network egress was not measured, so "no request leaves" stays
  pending (see [Desktop acceptance 2026-11-24](#desktop-acceptance-2026-11-24)).
- Have the document forge a burst (repeated `link.click()` or repeated
  `parent.postMessage`): at most one open per 500 ms. The owner accepted the
  forged-click risk, so this check bounds a flood; it cannot distinguish a
  forged activation from a user's.
  **Done 2026-11-24:** a forged 20-click burst opened at most one URL (see
  [Desktop acceptance 2026-11-24](#desktop-acceptance-2026-11-24)).
- The Goal 15 release checks in
  [goal-15-acceptance.md](goal-15-acceptance.md#pending-release-checks) still
  apply; the goal07 fixture no longer opens a browser during that run.

## Native acceptance 2026-11-23

One `mt.py accept all` run on `main` `c3aab06` (Goals 14–19 merged), on the
owner's active, unlocked RDP desktop (Windows 11 x64, harness process session
2, input desktop Default). The harness hash-verified the release executable
(SHA-256 `a6ef8cfb758e6934f1a7b17713f240ad9ea4255f753d5ff160a07193580b567b`,
44,480,000 bytes). It covers goals 02, 03, 06 and 07 only: goal-02 PASS,
goal-03 PASS, goal-06 PASS (`ALL_REQUIRED_CASES`), goal-07 FAIL
(`WEB_SECURITY_APP_WINDOW_COUNT_UNEXPECTED`, the nsh popup gap this goal's
contract lists as out of scope, not the preview link bridge). It exercises
none of the link checks listed above; those remain pending. This is not a
pass.

## Desktop acceptance 2026-11-24

Owner manual checks on the `32f324d` executable (release executable SHA-256
`d9dc951ffe19fb1a3e3fe64456658f00f9e40d6398437f5b80673b8c2c6d77bd`) on
2026-11-24, all observed as expected:

- In a Markdown document's Web preview, an `https:` link and an `http:`
  `target=_blank` link each opened once in the default browser and the preview
  did not navigate; the preview mode (column or field) was not recorded.
- `mailto:` opened the mail handler.
- `file:`, `javascript:`, `data:`, a custom scheme and a relative link did
  nothing; network egress was not measured.
- A forged 20-click burst opened at most once.
- An `https:` link also opened in the browser from an HTML and an MDX
  document.

The Goal 15 release checks in
[goal-15-acceptance.md](goal-15-acceptance.md#pending-release-checks) still
apply and remain pending; the goal-07 re-run on 2026-11-24 failed the same
way. Still pending from the checks above: the preview mode (column or field)
and network egress for the inert schemes. This is not a full pass for the
goal set.
