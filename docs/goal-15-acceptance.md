# Goal 15 implementation evidence

Contract: [Scripts always run; documents never reach the network](goals/15-scripts-always-on.md).
Plan: [Goal 15 implementation plan](plans/2026-11-21-goal-15-scripts-always-on-plan.md).

Status: blocked on native isolation. No application, dependency, test or native
harness changes were made. The active contract, its index and the README
Security section are unchanged. No after text exists to compare.

## Locked source inspection

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

## Gate results

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
