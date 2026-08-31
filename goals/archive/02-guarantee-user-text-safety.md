# Goal 02 — Guarantee user-text safety

## Objective

For every destructive lifecycle path owned by markturbo, prevent a normal UI
action, asynchronous operation, or external file change from silently discarding
newer user-authored text, and recover the latest successfully persisted checkpoint
after interruption; verify the enumerated paths with automated tests and a
Windows 11 x64 acceptance run while preserving ordinary files as the source of
truth.

## Product invariant

When markturbo must choose between convenience and preserving text, it preserves
the text or asks the user. A model result, watcher event, close action, or crash
must never become implicit permission to replace a newer buffer.

## Product contract alignment

**Disposition:** Retained and revised on 2026-08-29.

This goal preserves the local-first Markdown source-of-truth promise in
`PRODUCT.md` on the first public-quality platform, Windows 11 x64. It retains
recovery only as optional local protection for a dirty buffer, never as a
workspace format or a replacement for explicit Save and Discard decisions.

## In scope

- Handle dirty-tab closure from its button and keyboard command with an explicit
  Save / Discard / Cancel decision.
- Apply the same decision to window close and any action that would dispose of a
  dirty document or replace the active workspace.
- Make the lifecycle contract work for a dirty buffer with no disk path and
  prove that case with an in-memory document, so Goal 03 can introduce new
  documents without redesigning or bypassing the safety boundary.
- Give every asynchronous transformation a source revision, hash, or equivalent
  snapshot identity. This includes the current Translation operation.
- Refuse to apply a result when the editor no longer matches its source snapshot;
  retain the result for inspection or offer a rerun, but never overwrite.
- Preserve and strengthen external-change behavior: a dirty document is not
  auto-reloaded, and overwrite or recreation remains an explicit action after a
  rewrite, deletion, rename, or replacement.
- Detect content changes even when file length and observable modification time
  match the loaded values; a stamp optimization may avoid unnecessary reads but
  must not be the sole authority to overwrite.
- Track decoding errors and encoding representability. Invalid original bytes or
  newly typed characters that cannot round-trip through the original encoding
  must cause an explicit preserve/convert/Save As decision, never silent U+FFFD
  replacement or numeric character-reference rewriting of source text.
- Preserve a symbolic link or other explicitly supported shared-file identity on
  Save, or refuse with an actionable choice; atomic replacement must not silently
  turn `CLAUDE.md -> AGENTS.md` into two divergent regular files.
- On Windows 11 x64, maintain optional local recovery checkpoints for dirty
  buffers under application data, protected for the current user with DPAPI and
  written atomically. Checkpoint after two seconds without an edit and at least
  every ten seconds while dirty; acknowledge a maximum ten-second loss window;
  retain at most 50 records, 32 MiB per record, and 128 MiB in total; and expire
  records after seven days. Delete a record after intentional Save or Discard,
  prune expired records at startup and after a completed checkpoint, evict the
  oldest inactive record first when a bound is reached, and never discard the
  newest checkpoint for an open dirty buffer without a visible warning. Report
  unavailable, failed, or oversized recovery without blocking editing or
  damaging the source file.
- Retain enough load metadata to preserve encoding, BOM, line endings, source
  path, and the original conflict stamp. If the disk file changed while the app
  was not running, restore the buffer as conflicted rather than authorizing an
  overwrite.
- Ensure recovery storage does not modify the workspace, become required to open
  files, or create a proprietary document format.
- Avoid logging document contents, recovery contents, API keys, or model payloads.

## Out of scope

- The Review workflow, clarification questions, generated revisions, or
  per-hunk diff acceptance.
- General cloud sync, collaborative editing, version history, or continuous
  autosave to the source file.
- Session restoration for clean tabs, panel positions, or window geometry.
- A redesign of tabs, banners, or the complete application visual hierarchy.
- Changing renderer, WebView, or harness behavior except where a lifecycle action
  would otherwise lose text.

## Required cases

Automated coverage must prove at least these cases:

1. Closing a clean tab closes immediately.
2. A successful Save on dirty close writes safely and then closes; a cancelled or
   failed Save keeps the document and window open with exact text intact.
3. Discard on dirty close closes without writing.
4. Cancel on dirty close preserves the tab and exact editor text.
5. Window close with one or multiple dirty documents cannot bypass the decision.
6. A transformation result over revision N cannot replace revision N+1.
7. An external write cannot replace a dirty editor through auto-refresh.
8. A same-length rewrite with an unchanged/coarsened mtime is detected before
   Save and cannot be overwritten without explicit confirmation.
9. External deletion or rename produces an explicit recreate/Save As decision
   rather than silently resurrecting the old path.
10. Saving through a symbolic link preserves the documented shared identity or
    refuses safely; it never silently replaces the link with a regular file.
11. A file containing undecodable original bytes cannot be rewritten through a
    lossy editor round trip without an explicit conversion decision.
12. Adding text unrepresentable in the original legacy encoding cannot silently
    become replacement text or numeric character references; explicit conversion
    or Save As preserves the editor's exact source.
13. Recovery restores the latest completed checkpoint after simulated interruption
    within the approved ten-second maximum loss window, including CJK and emoji.
14. Recovery of legacy-encoded, BOM-bearing, or CRLF content retains the original
    save metadata; if its disk file changed while closed, the recovered buffer is
    conflicted and cannot overwrite implicitly.
15. Saving or explicitly discarding clears the corresponding recovery record.
16. A malformed, oversized, expired, or unreadable recovery record cannot prevent
    startup and is reported without damaging source files.

## Completion evidence

- Focused lifecycle, stale-result, filesystem-conflict, and recovery tests pass.
- A Windows 11 x64 run exercises tab close, keyboard close, window close,
  external modification, and interrupted-session recovery with the expected
  decisions and exact text retained.
- `cargo fmt --all -- --check` passes.
- `cargo clippy --workspace --all-targets` introduces no new warnings.
- `cargo test --release --workspace` passes and the completion report records the
  pass count.
- The completion report names where recovery data lives, when it is deleted, and
  demonstrates that no workspace file is required to use it.

## Stop and ask

Stop and ask if Windows 11 x64 cannot provide current-user DPAPI protection and
atomic application-data writes that satisfy the approved recovery contract. Do
not silently weaken the bounds, omit recovery, persist plaintext, or invent
cloud storage.

## Boundary for the next goal

This goal owns preservation and destructive-operation interlocks only. Goal 03
owns creating and saving a first document; Goals 06 and 07 own semantic Review
and its proposed-change UI.
