# Goal 22 — Files feel native: sidebar, view switching, drag-and-drop, live disk changes

## Objective

The owner's GUI feedback (2026-11-24): after opening a folder and then a file, the folder tree disappears and
switching files is awkward; the view modes hide in a dropdown; dropping a file on the window does nothing; and the
app must never lock files while picking up other editors' changes promptly, without heavy polling. Desktop only.

## Owner decisions (2026-11-24)

1. **Persistent file sidebar.** The left side becomes a sidebar:
   - It holds **open documents** at the top and the **folder tree** below. A single click opens a file in a normal
     tab; Goal 20 removed the preview slot.
   - `Ctrl+B` (`⌘B` on macOS) collapses and expands it, and the choice is remembered.
   - Below the 1040 px layout breakpoint it collapses to today's dot rail automatically. Expanding it there overlays
     the editor and does not push it.
   - No outline for now.
2. **One view button.** A single titlebar button cycles **Edit → Split → Preview → Edit**. Its label or icon shows the
   current view, and its accessible name is "View: {current} — switch to {next}". A shortcut does the same: use
   `Ctrl+E` (`⌘E`) unless it is already bound, in which case pick a free one and document it.
   - The Web "compatibility" preview is not part of the cycle. It stays a toggle in the titlebar menu that applies to
     Preview and Split.
3. **Drag-and-drop.** Use nsh 0.6.1's `FileDragEnter`, `FileDrop` and `FileDropAt` events.
   - Dropping one or more supported documents opens each in a normal tab or focuses it if already open; the last one
     ends up active.
   - Dropping a folder opens it as the workspace; open tabs stay open.
   - Unsupported files show one notice that names how many files were skipped, never their paths.
   - While a drag hovers, the window shows a quiet drop affordance that clears on leave or drop. No modal.
4. **Live disk changes without locks.**
   - Watch the open documents' parent directories with the existing `mt-core` watcher (`workspace/watcher.rs`,
     `notify` plus the debouncer). It is event-driven, with no polling loop.
   - When a **clean** open document changes on disk, reload it automatically, keep the caret line and scroll as
     closely as possible, and show a quiet status notice ("Updated from disk" / "已从磁盘更新").
   - When a **dirty** document changes on disk, raise the conflict banner immediately, not at save.
   - When a document is moved or deleted on disk, raise a banner that offers Save As.
   - The app's own saves never trigger a reload or conflict: ignore events whose disk stamp matches our last write.
   - If the watcher cannot start, fall back to today's save-time check, record a diagnostic, and never block the user.
   - The folder tree follows creates, deletes and renames from the same events.
   - **No file locks.** The app never holds a document handle beyond a single read or write call.
     - Opening snapshot reads with `FILE_SHARE_READ` only (`mt-core/src/document/io.rs` `open_snapshot_file`) can make
       another editor's atomic save (write temp, then rename over) fail during our read. Open with
       `FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE` instead, keep consistency by re-checking the
       file-object identity and stamp after the read, and retry once if they changed.
     - Saves keep their atomic replace.

## Done when

1. Each decision is implemented and covered by `nsh_testing::Headless` tests for UI behaviour, and by `mt-core` tests
   for I/O:
   - the sidebar opens a file from the tree in one click; `Ctrl+B` persists; the narrow layout auto-collapses;
   - the view button cycles in order, and the shortcut cycles too;
   - a drop event opens the files, a folder drop sets the workspace, and an unsupported drop shows the count notice;
   - for a clean document, an external write triggers a reload and a notice; for a dirty one, it raises the banner;
     our own save triggers nothing;
   - a concurrent external rename-over during a read neither fails the external writer nor corrupts our snapshot.
     Test it with an injected race.
2. On Windows, a manual check: while MarkTurbo has the file open, another editor (Notepad) saves, renames and deletes
   it, and MarkTurbo updates within about 1 s without errors.
3. Goal 20 invariants hold. Docs (`README.md` usage and `docs/architecture.md`) describe the sidebar, the view button,
   drag-and-drop and live reload.
4. `mt.py check ci` passes. GPT review converges to P3-only.

## Failure control

- Any polling loop for file changes.
- Any long-held file handle or new share-mode restriction.
- A reload that discards unsaved edits.
- The view cycle including the compatibility preview.
- A drop that executes or opens anything other than documents and folders.

## Out of scope

The outline view; tab reordering by drag; motion polish (Goal 21 follows and covers the sidebar's motion).

## Delivery lanes (after Goal 20 lane 20-A merges)

- **22-A Sidebar and view button:** `ui/rail.rs` (becomes the sidebar), `ui/shell.rs` layout, `ui/titlebar.rs` (the
  view button, and removing the view items from the menu), plus the settings field for the sidebar state.
- **22-B Files and events:** drag-and-drop handlers (the shell root and `state.rs` open paths), the watcher wiring in
  `state.rs`, the `mt-core` watcher and `document/io.rs` share modes and retry, and the reload and conflict paths.

## Mergeable alone

Each lane is mergeable alone on `main`.
