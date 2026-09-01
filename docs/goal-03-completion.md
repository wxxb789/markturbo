# Goal 03 Completion and Acceptance Report

**Date:** 2026-09-01
**Goal:** [Goal 03 - Create a complete first-use document flow](../goals/archive/03-create-first-use-document-flow.md)
**Status:** NOT COMPLETE / POST-REVIEW CODE GATES PASS; FINAL NATIVE ACCEPTANCE IS BLOCKED BY THE CURRENT WINDOWS SESSION.

## Delivered first-use contract

- A no-argument launch opens Welcome. `markturbo .` remains the explicit current-directory launch, and other file or directory arguments bypass Welcome.
- Welcome provides New, Paste, Open File, Open Folder, bundled sample, and bounded Recent entry points. **Don't show this again** persists the preference and immediately opens an empty pathless Markdown buffer.
- Memory documents use the existing dirty-close and encrypted recovery lifecycle. Save As migrates the same document to a file identity rather than creating a second editor lifecycle.
- Save As uses create-only publication first. Replacing an existing destination requires an explicit decision bound to the pre-prompt file stamp and revalidated before commit.
- Successful Save As updates tab, recovery, watcher, source-generation, history, relative-path, Recent identity, layout availability, and editor language. An equivalent destination already open in another tab is rejected.
- Recent targets are MRU-deduplicated, normalized on load, and capped at ten. Missing or mismatched targets stay visible, disabled, and removable.
- File/folder pickers, Recent, the bundled sample, CLI targets, and drag-and-drop converge on the same target-opening lifecycle.
- Focused editor input is drained across two frames before reposting the native
  close message. The normal Windows destruction lifecycle then completes without
  leaving an AccessKit request on a destroyed window handle.

## Current automated evidence

| Command or artifact | Result |
|---|---:|
| `cargo fmt --all -- --check` | PASS |
| `cargo clippy --workspace --all-targets` | PASS; no warnings |
| `git diff --check` | PASS |
| `cargo test --release --workspace --locked -- --format terse` | 852 passed, 0 failed, 8 ignored |
| `py -3 -m py_compile scripts/goal-03-native-acceptance.py scripts/test_goal_03_native_acceptance.py` | PASS |
| `py -3 scripts/test_goal_03_native_acceptance.py` | 33 passed |
| `py -3 scripts/test_platform_packaging.py` | 5 passed |
| `.scratch/goal-03-native-acceptance-v1.json` | BLOCKED: `FOREGROUND_PERMISSION_DENIED`; no case ran against the listed executable hash |
| `target/release/markturbo.exe` | AMD64; 48,616,448 bytes; SHA-256 `7fa8e66808cd9e59561fd088dd153ec9e9634e331e534ff4bf0b964279e45524` |

## Post-review corrections

- Save As now reconciles an invalid old layout against the new `DocType` and refreshes the editor highlighter from the new path before rebuilding derived content.
- The native cancellation scenario names and fingerprints a real existing destination, verifies exact editor fingerprints without refocusing, and records editor-focus preservation after both picker and overwrite cancellation.
- The stale Recent scenario now fails immediately when UI Automation reports the stale target as enabled.
- Stale Recent targets now expose the AccessKit disabled state explicitly instead
  of relying only on pointer and focus suppression.
- Source-contract checks now scan only production Rust text before `#[cfg(test)]`, preventing test literals from satisfying the contract.
- Window close now drains focused input, reposts `WM_CLOSE`, and completes through
  `WM_DESTROY`; the native harness keeps UI Automation active while verifying
  clean process exit.
- Save As now removes startup and Save As recovery-key bookkeeping independently,
  preventing a later normal Save from consuming a stale source key.
- macOS archives keep one bundled sample under application resources and expose
  it through relative links beside the executable and at the archive root.
- The recovery lock test now pauses after the durable retirement marker write,
  so Windows file-system latency is not mistaken for waiting on the checkpoint
  mutation lock.

## Prior native acceptance

The last complete Windows 11 x64 run completed all seven required cases against
the pre-review executable with SHA-256
`e50cecefdf312da61b25041d515591d82eb3a20eeac0d40211ebf2c22d555f54`:

1. no-argument Welcome, persistent **Don't show this again**, and restart into a
   pathless memory buffer;
2. New and Paste into focused editable buffers with exact CJK and emoji text;
3. Save As create followed by direct reopen with byte-identical text;
4. Save As cancellation and overwrite cancellation with editor focus, source,
   destination, and unsaved text preserved, followed by confirmed replacement;
5. bundled sample opening from Welcome;
6. ten-entry Recent persistence with a stale target visibly disabled through
   UI Automation; and
7. explicit file and directory arguments bypassing Welcome.

Every case verified a foreground target, medium-integrity process context, clean
process exit, and absence of the case sentinel from application logs and
settings. That evidence does not cover the post-review recovery fix.

## Current native acceptance status

The first post-review run reached the desktop but stopped in the New/Paste case
with `WELCOME_DID_NOT_CLOSE`. A fresh run against the final hash listed above was
then blocked before any case by `FOREGROUND_PERMISSION_DENIED`. Computer Use
reported the same session boundary as `GetCursorPos failed: Access is denied.
(0x80070005)`. No final-hash native PASS is claimed.

## Completion boundary

All post-review code gates pass, including the deterministic Windows CI
regression and macOS packaging coverage. Goal 03 remains incomplete until the
current executable completes all seven native cases from an interactive Windows
session. Semantic Review remains outside this goal and begins at Goal 06.
