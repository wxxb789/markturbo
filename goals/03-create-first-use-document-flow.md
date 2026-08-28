# Goal 03 — Create a complete first-use document flow

## Objective

On a clean profile and a no-argument desktop launch, let a new user reach an
editable Markdown buffer by pasting text, opening a file or workspace, or opening
the bundled sample, then save it through Save As as an ordinary `.md` file and
reopen it from a bounded recent-target list; verify the complete flow on the
primary platform without depending on Intent Review.

## User outcome

A person with a rough prompt must not need another editor, a pre-existing file,
or terminal knowledge before markturbo becomes useful.

## In scope

- Replace an accidental installation/current-directory first screen with a
  deliberate welcome state for a no-argument desktop launch.
- Offer these explicit entry points, using terminology settled by Goal 01:
  - create or paste into a new Markdown artifact;
  - open a file;
  - open a workspace folder;
  - open the bundled sample;
  - reopen a recent workspace or file.
- Support an unsaved buffer with a useful title derived from its first meaningful
  line and a clear unsaved indicator.
- Route that buffer through Goal 02's dirty-close and recovery contract; a new
  document must not introduce a second or weaker lifecycle path.
- Implement Save As for a new buffer and for an existing document when invoked
  explicitly, including overwrite confirmation, tab/path identity updates,
  watcher/conflict-stamp updates, and collision handling when the destination is
  already open.
- Make cancellation of Open, overwrite confirmation, and Save As a no-op that
  preserves the current workspace, buffer, and focus.
- Persist only a numerically bounded, owner-approved number of recent paths and
  presentation metadata in application settings; do not copy document contents
  or create workspace metadata.
- Remove or clearly disable stale recent entries without preventing startup.
- Keep path arguments working: opening `markturbo PATH` still opens the requested
  file or directory directly.
- Make drag-and-drop consistent with the same file/folder/new-workspace rules.

## Out of scope

- Reviewing, scoring, rewriting, translating, or executing the pasted text.
- Effective Agent Context resolution.
- Crash recovery and destructive-close policy, which belong to Goal 02.
- Installer, signing, auto-update, or public release documentation.
- New proprietary file types, templates, cloud storage, or an account system.
- Broad session restoration of every clean tab and UI position.

## Completion evidence

A clean-profile acceptance run must demonstrate all of the following:

1. A no-argument desktop launch shows an intentional welcome state.
2. “New” or “Paste” creates an editable buffer without first choosing a folder.
3. Save As writes through the existing safe filesystem path and produces a normal
   file another editor can open.
4. Confirming overwrite replaces only the chosen destination; cancelling Save As
   or overwrite leaves the unsaved buffer and destination byte-identical.
5. After Save As, tab identity, watcher state, subsequent Save, relative path,
   conflict detection, and recent entries all use the new path.
6. Saving to a path already open does not create two editors that can silently
   overwrite each other.
7. Closing or interrupting an unsaved buffer follows Goal 02's decision and
   recovery behavior with its exact text preserved.
8. Reopening the saved file displays the same text, including CJK and emoji.
9. The bundled sample opens from the welcome state without a terminal.
10. The Goal 01-approved recent-path bound survives restart; a missing recent
    entry degrades visibly and safely.
11. A direct command-line path still bypasses the welcome state and opens the
    requested target.

Automated tests cover state transitions and path persistence. In addition:

- `cargo fmt --all -- --check` passes.
- `cargo clippy --workspace --all-targets` introduces no new warnings.
- `cargo test --release --workspace` passes and the pass count is recorded.

## Stop and ask

Stop and ask if Goal 01 does not identify the primary platform or if preserving
no-argument terminal behavior conflicts with the chosen desktop first-run
contract. Do not implement platform detection based on an unverified heuristic.

## Boundary for the next goal

This goal ends when a user can begin, save, and reopen ordinary Markdown. It does
not make that Markdown clearer; semantic Review begins at Goal 06.
