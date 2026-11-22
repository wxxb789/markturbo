# Goal 14 implementation evidence

Contract: [Upgrade every nsh crate to 0.6.1](goals/14-upgrade-nsh-0-6.md).
The implementation stage does not publish a PR or assert native acceptance.

## Dependency and API changes

- Seven direct nsh dependencies are pinned exactly; 13 nsh crates in
  `Cargo.lock` resolve to 0.6.1. No non-nsh versions changed. Cargo removed
  17 transitive packages no longer needed by the selected feature graph.
- `nsh-base` 0.6.1's published manifest still specifies RaTeX 0.1.15.
  MarkTurbo's exact pins, crates.io provenance and the vendored
  `alignat`/`alignedat` allocation clamp are unchanged.
- The Windows backend explicitly enables `webview`, which selects
  `webview2-com`. MarkTurbo uses no native media surface, so it leaves
  `media` disabled. Linux keeps its existing `webview` feature.
- Existing transition builders do not construct `Transition` directly.
  The alert callers await `show()` immediately and still match the resulting
  action; in 0.6.1 `AlertDialogAnswer` implements `Future` for that same
  result. There are no MarkTurbo call sites for the changed context menu,
  `clip_utf16`, max-length, modal config or rich editor APIs. `nsh.json`
  declares no capabilities, and the shipped binary runs a Rust root, so
  there is no `application` or `homeDir` grant to add.
- nsh 0.6.1 gives focused press targets precedence over ordinary Return
  and Space bindings (`nsh-core/src/keymap.rs`, `find_with`). On a consent
  resize the body cleared the hold's focus flag, but Headless still sent a
  native press to the hold and bypassed the card's default Cancel. The
  consent and guard bindings now use `KeyOptions::for_focus` to preserve
  their existing precedence. The existing consent resize assertion failed
  on the first CI run and in isolation before this migration, then passed
  with its original assertions in both subsequent CI runs.
- The close guard also consumes Space without acting. A Windows-configured
  Headless regression test verified that Space re-pressed the focused close
  control behind a pointer-opened guard before the fix.
- The release workflow uses the Cargo-built binary and has no CLI pin to
  update. The optional CLI path in the README names `nsh-cli` 0.6.1's
  matching release.

## Data path resolution

Compared the 0.5.1 and 0.6.1 published source and unchanged app options.
These are symbolic paths, with no machine-specific profile information.

| Consumer | Before (0.5.1) | After (0.6.1) |
|---|---|---|
| MarkTurbo settings | `%APPDATA%/markturbo/settings.toml` | Same |
| MarkTurbo recovery | `%LOCALAPPDATA%/markturbo/recovery` | Same |
| MarkTurbo bundled fonts | Embedded Martian and KaTeX, with system fallback | Same |
| MarkTurbo companion font | `<data_dir>/fonts/SourceHanSansCN-VF.otf` | Same |
| nsh engine on Windows (direct Cargo runner) | `%APPDATA%/default` | Same |
| Windows preview profile | `%LOCALAPPDATA%/com.markturbo.app/WebView2` | Same |

`mt-core` owns settings, recovery, and companion font resolution, including the existing
`MARKTURBO_CONFIG_DIR` and `MARKTURBO_DATA_DIR` overrides. It does not use
the new nsh app data directory. `CompanionFont::locate` resolves the optional
font via `runtime_paths::data_dir`. In nsh 0.5.1 the Windows runner passed
the platform data root to the engine, which appended the root's app ID or
`default`; in 0.6.1 the runner passes the same platform root as `support_dir`
and the engine applies the same rule when `data_dir` is unset. The direct
runner's `Root::of::<App>()` has no app ID; `AppOptions.app_id` identifies the
native surface and does not set the root's ID. See the two versions'
`nsh-platform-windows/src/app.rs`, `nsh-engine/src/storage.rs` and engine
initialization in `nsh-engine/src/lib.rs`. Windows `native/web.rs` computes
the WebView2 profile separately and is unchanged for the given ID.

An observed before/after runtime resolution is pending and will be recorded
during native acceptance.

## Windows behavior checked against 0.6.1 source

| Check | Source and disposition |
|---|---|
| Input font resolution | `nsh-platform-windows/src/text_input.rs`: `family` takes the first nonempty family and `format` supplies the default system collection to DirectWrite. Keep `FONT_INPUT`. |
| Focus and key release | `nsh-platform-windows/src/keyboard.rs`: `is_stop`, `focus_control`, and `release` now cover ordinary focusable controls and dispatch `KeyUp`. Existing focus requests can take effect; MarkTurbo retains the consent repeat/watchdog logic, focus-independent Ctrl+Enter and guard selection ring. No new key-up handler or focus-dependent shortcut is added. |
| UIA | `nsh-platform-windows/src/access.rs` and `access/uia.rs`: IDs map on exposed elements, plain wrappers flatten, Button content is grouped, and Value comes only from native inputs. UIA Invoke is a discrete press, not a hold; SetFocus also supports focusable views. Retain the current harness selectors and actions. |
| Editor clipboard and alerts | `nsh-base/src/editor/view.rs` uses an IME sink, and `editor/commands.rs` still copies a bare caret line-wise. `nsh-platform-windows/src/system.rs` hosts alerts outside the UIA tree and Enter chooses its selected button. Keep the existing clipboard and alert checks. |

The Settings layout spacer remains unchanged. The general upstream
`justify_space_between_puts_the_space_between` test is not evidence that
the previous Windows Settings layout issue is fixed.

On Windows, pressing a pressable control can now move keyboard focus away
from the source editor (`nsh-platform-windows/src/input.rs`, `focus_pointer`
call). For example, with Review not configured (so no consent card opens),
clicking Review in the title bar and then typing no longer inserts text into
the editor until it is refocused. This also applies to the pressable filename
and Settings controls in the title bar. Existing focus
requests now take effect on the welcome screen's Open Folder control
(`ui/welcome.rs`), the first Open Folder row of the Settings menu
(`ui/titlebar.rs`), the Store securely control after successfully removing a key
(`ui/settings.rs`), and the Review Again and Prepare controls following their
existing Review transitions (`ui/review.rs`). The Review answer field is a
text input and already accepted focus on Windows in 0.5.1. No app focus
policy was changed.

## Verification status

Windows x64, Rust 1.100.0. The host-configured registry mirror timed out;
Cargo used a worktree-local Cargo home pointed at crates.io instead. The
lockfile's source URLs were not changed.

| Gate | Result |
|---|---|
| `cargo fmt --all -- --check` | Exit 0. |
| `cargo tree -d -p mt-app` | Exit 0; no duplicate nsh crate. |
| `uv run --locked --project scripts scripts/mt.py check ci` | Exit 0 on the final run: 233 tooling tests passed; 1,041 Rust tests passed (227 mt-app), 0 failed, 7 existing ignores. Optimized Clippy reported 0 mt-app warnings and 2 existing mt-core warnings. |
| PR-head CI | Pending: this stage has no published PR head. |
| Native acceptance and visual matrix | Pending. Native acceptance was deferred by the owner on 2026-11-22 and was not run. The Windows 11 session could open the input desktop, but `GetForegroundWindow` returned no foreground HWND. No native PASS or capture is claimed. |
| Native keyboard focus | Pending: on Windows 11, with Review not configured, focus the editor, click Review, and verify typing does not edit the document until the editor is refocused. Also verify the existing focus requests on the welcome screen, Settings menu and Review Again and Prepare transitions listed above. In Settings, verify that focus lands on Store securely after successfully removing a key. |

The first CI run exited 1 on the consent regression above. After the fix,
the next full run exited 1 on the unchanged mt-core recovery test
`batch_keeps_input_order_and_last_same_key_checkpoint_when_workers_finish_out_of_order`
(928 passed, 1 failed, 3 ignored before the stop). Its 40 ms scheduling
assertion passed on a single-test replay and on the final full run. No
test or test-runner setting was changed between those runs.

Next checkpoint: run the per-PR pipeline on the actual proposed revision.
When the owner resumes native acceptance on an unlocked Windows 11 x64
desktop with a foreground window, build the
release executable and run `mt.py accept all` with its exact SHA-256, then
`mt.py visual -- --exe <release exe> --out .scratch/goal-14-visual`.
Keep the goal active until the required evidence exists.
