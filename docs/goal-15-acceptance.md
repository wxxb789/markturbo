# Goal 15 implementation evidence

Contract: [Scripts always run; documents never reach the network](goals/15-scripts-always-on.md).

## Sandboxed-frame candidate

The amended contract replaces the first run's top-level document with one
static host and one opaque-origin `srcdoc` iframe. The Rust page builder no
longer takes a trust argument. Source tests cover the host policy and exact
sandbox token, attribute escaping, document policy ordering for all three
formats, shell and author-policy preservation, theme, diagrams and math. An
inline script needed by an authored Markdown diagram is present on the first
build. General MDX JSX remains a structural placeholder; authored `<script>`
bodies are preserved without adding an MDX runtime.

README Security before: the per-tab Restricted/Trusted table described a
prompt, Revision-time revocation, and trusted HTML with no CSP. After: the
same base-free host, sandboxed frame and resource policy apply to Markdown,
MDX and HTML on first render; remote scripts and relative assets do not load.
The exact wording change is in `git diff -- README.md`.

The Rust root declares an empty external WebView host list. In nsh 0.6.1,
`nsh-engine/src/lib.rs:4301` consumes it for engine access checks;
`nsh-platform-windows/src/app.rs:441` separately installs the default native
access for Rust roots. The declaration denies external app-side loads, but
is not an override for the Windows renderer's native navigation handler.

The current contract search, with a word boundary before `Trust::`, returns
zero matches in `crates/`. The unrelated `ProjectTrust` type was not renamed.

### Candidate gates

| Command | Exit | Result |
|---|---:|---|
| `cargo fmt --all -- --check` | 0 | No output after formatting the new import. |
| `cargo clippy --profile ci -p mt-core -p mt-app --all-targets` | 0 | Isolated target; 3 existing warnings: 2 argument-count warnings in mt-core and 1 clone-on-Copy warning in mt-app tests. |
| `cargo test --profile ci -p mt-app --lib` | 0 | Isolated target; 167 passed, 0 failed, including all 12 Web page tests. |
| `cargo test --profile ci -p mt-app --test hierarchy_layouts` | 0 | Isolated target; 1 passed, 0 failed. |
| `cargo test --profile ci -p mt-app` | 0 | Isolated target; 222 passed, 0 failed, 0 ignored across 42 test targets. |
| `cargo test --profile ci -p mt-core --features model-transport` | 0 | 833 passed, 0 failed, 7 ignored across 9 test targets. No `WouldBlock` or `PoisonError` on this run. |
| `uv run --locked --project scripts scripts/mt.py check fast` | 1 | 1 of 8 tooling shards failed; 28 passed and 1 failed in that shard. |
| `uv run --locked --project scripts scripts/mt.py check native` | 0 | 288 passed across 8 shards. No desktop acceptance run. |
| `uv run --locked --project scripts scripts/mt.py check ci` | 1 | Same tooling shard failure as `fast`; workspace Rust checks did not start. |

`fast` and `ci` stop at the existing
`test_cargo_config_context_records_only_content_free_fingerprints` failure:
the host's ancestor Cargo configuration yields two configs instead of one.
The test and shared configuration are unchanged.

The first focused `cargo test --profile ci -p mt-app --lib web::tests`
exited 101 (11 passed, 1 failed): the GFM compiler escaped the Markdown
fixture's script. The Web renderer now preserves inline script markup for
the sandbox to govern. The first full mt-app run against the shared target
also exited 101 (185 passed, 1 failed), but ran a titlebar test absent from
this tree and reported a line beyond the end of the local source file.
Those counts are not evidence for the candidate.

The retry used a worktree-local Cargo target (four build jobs initially,
eight for the final full run) without changing the shared Cargo config. The
first isolated run exited 101 before tests: the capability test called
private or unavailable methods on `Capabilities`. It now queries the
public `Access` type, as the nsh engine does. The next isolated run exited
101 (165 passed, 2 failed): a buffer test expected CRLF instead of LF,
and the Markdown fixture placed its script in a paragraph, where the
parser escaped the JavaScript string literals. The fixture now uses a
raw HTML script block and checks its exact contents and position.

The next app run reached integration tests, then exited 101 (178 passed,
1 failed). The hierarchy test still waited for the retired Restricted
banner. It now checks the WebView, keeps its layout-transition assertions,
and verifies the banner is absent. The visual harness also waits for the
WebView instead of the removed button.

One `check native` run after the visual change exited 1: the unchanged
Goal 06 clean-close test timed out in two subcases in a 36-test shard.
A focused replay reproduced one subcase timeout. The full retry passed
all 288 tests without altering the timeout or Goal 06 code.

### Review-fix startup diagnostic

The locked release build was measured with `mt.py probe -- startup --rounds 5
--warmup 1` using isolated data and configuration directories. Time to the
first visible titled window was 586.0, 591.9, 572.1, 551.2, and 566.1 ms
(median 572.1 ms, p95 590.7 ms). This is a startup diagnostic, not a
quiet-gated regression comparison or native isolation proof.

### Pending release checks

The contract stays active. The CI tier has not passed and the candidate has
no release-bound native isolation evidence.

- On the owner's active, unlocked Windows 11 x64 desktop, run
  `mt.py accept all --exe <release exe> --expect-exe-sha256 <release sha256>`.
  The Goal 07 phase must see the script's visible effect and confirm that
  user-style link and form navigation leave the fixture in place. It then
  attempts deliberate script navigations; those may leave the frame on the
  browser's blocked page, so the verdict must use fixture markers emitted
  before those attempts. They must produce zero loopback contacts and open no
  additional window. The acceptance harness unit tests do not run the desktop
  phase.
- Clause 3b native proof is pending: the fixture must visibly report its
  network attempts and the loopback TCP and UDP listeners must record zero
  contacts on the release build. Unit tests do not verify WebView2 egress,
  and the loopback counters do not observe hostname DNS resolver traffic or
  file loads. The resolver-rule string test covers hostname DNS egress.
- Clause 3c elevated-host check: run the release app at high integrity. The
  web layouts must show the elevated-host reason and create no WebView; the
  native preview remains available.
- Verify frame scrolling, text selection, light and dark colours, math and
  diagrams in the actual preview. If WebView2 rejects the frame policy, stop
  and record the result without loosening the sandbox.
- Verify zero document-driven outbound activity and file loads independently
  of the visibility check. The locked Windows backend checks top-level
  navigation against its own native access value, but does not install a
  `FrameNavigationStarting` handler or a general resource filter for
  `NavigateToString`. CSP fetch restrictions are not evidence about other
  browser network facilities: the CSP draft defines a separate
  [RTC pre-connect check](https://w3c.github.io/webappsec-csp/#should-block-rtc-connection),
  which the prescribed policy does not configure. The app does not register
  a handler for the bridge messages nsh can deliver.
- The shared diagram registry runs before the WebView receives the host.
  The PlantUML child gets `PLANTUML_SECURITY_PROFILE=SANDBOX`; browser policy
  does not govern that subprocess. Include configured external renderers in any
  document-wide no-network/no-file verification.

## Run 1 (before the sandboxed-frame amendment)

The first attempt was blocked by the live-popup behavior described below.
It changed no application, dependencies, tests, native harness or product docs.

### Locked source inspection

The following paths are relative to the locked nsh 0.6.1 crate roots.

| Check | Source and result |
|---|---|
| Empty host list | `nsh-core/src/access.rs:2333` allows an inline WebView when a declaration exists. `allows_webview` at line 2343 delegates external URL checking to the host matcher at line 1638. An empty list can deny HTTPS and file URLs at that check. This is source inspection, not a MarkTurbo capability test. |
| Base-free carrier | `nsh-platform-windows/src/native/web.rs:1690` calls `NavigateToString`. The resource filter at line 1779 serves the alternate base-URL document carrier and is not a request firewall. |
| Navigation | Windows `src/native/web.rs:868` cancels a navigation that fails `allows`. `allows` at line 1553 accepts `about:blank` and `data:` before consulting app access and falls back to allowing URLs in the absence of an access declaration. It does not distinguish an app string load from a page's own request. |
| Popups | Windows `src/native/web.rs:991` handles a popup only if the preview owner is no longer live. `nsh-core/src/element/host.rs:1701` offers a navigation-state notification, not a cancellable policy or popup callback. An empty host list is not proof of popup suppression. |

The inspection used the locked crate source, not a live page. No browser
requests or external handlers were exercised. Goal 15 cannot enable script
execution by default using only the verified APIs. The next checkpoint is a
native backend with verified navigation, popup and resource enforcement, then
the app changes and release-bound owner's desktop acceptance.

The crates.io API for `nsh-platform-windows` returned `0.6.1` for both
`max_version` and `max_stable_version` during this stage. No newer published
Windows backend was available to evaluate.

The baseline `git grep -nE "Trust::|allows_scripts|to_file_url|Run scripts|Trust scripts" -- crates/`
in the Goal 15 worktree returned 52 lines: 43 document-trust matches and 9
unrelated `ProjectTrust::` matches. The latter are a substring collision in the
contract's search, not evidence of additional per-tab state.

### Gate results

These commands check the existing application at `d70f645`, not an implemented
Goal 15 candidate. Native acceptance (`mt.py accept all`) is not run in this
stage; it remains pending a release build and the owner's desktop session.

| Command | Exit | Result |
|---|---:|---|
| `cargo fmt --all -- --check` | 0 | No formatting diagnostics. |
| `cargo clippy --profile ci -p mt-core -p mt-app --all-targets` | 0 | 3 distinct warnings: 2 argument-count warnings in mt-core and 1 clone-on-Copy warning in mt-app tests. |
| `cargo test --profile ci -p mt-app` | 0 | 236 passed, 0 failed, 0 ignored across 42 test targets. |
| `cargo test --profile ci -p mt-core --features model-transport` | 101 | 720 passed, 1 failed, 3 ignored. |
| `uv run --locked --project scripts scripts/mt.py check fast` | 1 | 1 of 8 tooling shards failed; 28 passed and 1 failed in that shard. |
| `uv run --locked --project scripts scripts/mt.py check native` | 0 | 282 passed across 8 shards. No desktop acceptance run. |
| `uv run --locked --project scripts scripts/mt.py check ci` | 1 | 1 of 8 tooling shards failed in `fast`; workspace Cargo checks did not start. |

Cargo commands ran serially with the specified offline Cargo home. No
measurement harness ran. The core suite stopped at
`usage::tests::save_and_clear_remove_only_usage_owned_staging_files`, where
`record` returned `Io(WouldBlock)` (`crates/mt-core/src/usage/tests.rs:447`).
Its cause was not established. The fast and CI tooling failures were both
`Goal04BuildEvidenceTests.test_cargo_config_context_records_only_content_free_fingerprints`
(`scripts/tests/test_probe.py:1126`). The test expects one config; the
collector also walks workspace ancestors and this host has an additional
`.cargo/config.toml` in the immediate ancestor. No shared configuration was
changed to alter the result.
