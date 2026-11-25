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
3. **Drag-and-drop.** Use nsh 0.6.1's `FileDragEnter`, `FileDragLeave` and `FileDrop` events. `FileDropAt` is the
   `FileDrop` payload that carries the coordinates.
   - Dropping one or more supported documents opens each in a normal tab or focuses it if already open; the last one
     ends up active.
   - Dropping a folder opens it as the workspace; open tabs stay open.
   - Unsupported files show one notice that names how many files were skipped, never their paths.
   - While a drag hovers, the window shows a quiet drop affordance that clears on leave or drop. No modal.
4. **Live disk changes without locks.**
   - Use the existing `mt-core` watcher (`workspace/watcher.rs`: `notify` plus the debouncer). It is event-driven,
     with no polling loop.
     - An open workspace root is watched recursively from the moment the folder opens, even with no tabs, so the tree
       stays current.
     - Each open document outside the root has its parent directory watched non-recursively, even with no workspace
       open, for example when a file was dropped or picked directly. That watch is removed when the last such tab in
       that directory closes.
     - Never watch unrelated directories recursively. If `Watcher::new` requires a root, change its API rather than
       inventing one.
   - When a **clean** open document changes on disk, reload it automatically, keep the caret line and scroll as
     closely as possible, and show a quiet status notice ("Updated from disk" / "已从磁盘更新").
     - The reload re-checks the same tab and its edit revision after the disk read, before anything is applied.
     - If the user edited during the read, keep the edits and the recovery record, and raise the conflict banner
       instead of `adopt_disk_version`.
     - Every disk event for a tab advances that tab's disk-event generation. A reload applies only if the generation
       is unchanged since its read began. A newer delete or move notice is never cleared by an older read.
   - Symlinked documents: watch the parent directory of the resolved target, non-recursively, and map its events back
     to the tab. This is in addition to the link's own location, and also applies inside the workspace when the target
     is outside the workspace.
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
     - The same applies to the Agent Skill entrypoint path (`open_skill_entrypoint`, which `SKILL.md` loads through
       first). Its existing source-identity checks stay.
     - Saves keep their atomic replace.

5. **Design bar (owner, 2026-11-24).** The current look stays as it is; this goal fixes usability, not style.
   - New surfaces (the sidebar, the view button, the drop affordance) meet the bar of the best designers, in the spirit
     of Japanese tea-ware: restraint, space instead of lines, quiet ink, nothing ornamental.
   - They use only `ui::tokens`. No new colours, badges, icons-for-decoration or borders where spacing does the job.
   - Before implementation, the owner picks one of three distinct interactive HTML prototypes. The prototypes include
     motion and interaction, dark/light themes, narrow/wide windows, and reduced motion. The chosen direction is
     recorded here.
6. **Chosen direction: B, "Shoji" (sliding screens). Owner, 2026-11-24.**
   - The reference prototype is [`docs/design/goal-22-shoji.html`](../design/goal-22-shoji.html). Open it in a browser.
     Where it conflicts with this contract, the contract wins. The prototype's dev strip and legend are tooling, not
     product.
   - **Idea.** Every surface is a paper screen sliding on a track. Space is rearranged physically, never swapped.
   - **Sidebar.** A slightly toned panel, a different paper tone from `ui::tokens` rather than a border.
     - At 1040 px or wider it pushes the editor. Below that it slides over the editor.
     - Collapsed (`Ctrl+B`), it leaves an edge strip of about 44 px, where each open document is a short vertical tick.
       Hovering a tick shows the document's name; clicking it switches to that document.
   - **One list.** This replaces decision 1's separate "open documents" section.
     - The folder tree marks open files with a small ink dot. The active file has full ink.
     - An "Outside folder" group appears at the top only when it has members: open files outside the workspace,
       untitled buffers, or buffers whose file was deleted or moved on disk.
   - **View control.**
     - A small three-segment glyph that shows the split ratio, plus the current state's label.
     - Cycling (`Ctrl+E`, or a click) slides the divider between the panes: Edit puts it at the right edge, Split in the
       middle, Preview at the left edge.
     - The divider can also be dragged, and it snaps to those three stops only. There is no free ratio.
   - **Motion.**
     - Spatial and continuous: ease-out with a slight settle, from `ui::motion` tokens. The divider uses the camera
       duration; the sidebar uses the rail widen duration.
     - Every movement can be interrupted. It retargets from its current value toward the new stop without a jump, and
       it moves backwards only when the new stop is behind it.
     - With reduced motion, movement becomes an opacity change.
     - This goal owns the sidebar and divider motion. Goal 21 later tunes it, and does not redo it.
   - **Every open tab stays one click away.** The titlebar tab strip remains and lists every open tab, expanded or
     collapsed.
     - Untitled buffers and buffers whose file was deleted or moved on disk appear in the "Outside folder" group, as
       "Untitled N" or by their last name.
   - **Easing.** "A slight settle" means the existing monotonic curves, `motion::camera_easing()` for the divider and
     `motion::EASE_OUT` for the sidebar. There is no overshoot, and no new easing token.
   - **Drop.** The nsh 0.6.1 `FileDragEvent` carries no position, and Windows only sends enter/leave when the hit target
     changes. So:
     - The screen always slides in from the right edge, the side away from the sidebar.
     - It counts only items it can confirm it will open: supported document names, plus folders when the event
       identifies them as folders. It shows "Open plan.md" for one, or "Open 3 files" for several.
     - When nothing can be confirmed (no names, or only inferred types), it says "Drop to open". Skipped items are
       reported after the drop, by decision 3's count notice.
     - It says "Can't open this file type" only when every item is confirmed to be a file of an unsupported type. An
       item that might be a folder, because the event carries only a name and an inferred MIME, is never treated as
       unsupported.
     - The prototype's "side being dragged over" is not required.
     - The screen clears on leave or drop.
   - **Clean reload.** The changed text slips about 6 px as the old text leaves and the new text arrives. It never
     glows; only model output emits light.
     - It plays only after a new disk snapshot has been adopted successfully. It never plays for a conflict or a
       failed read.

## Done when

1. Each decision is implemented and covered by `nsh_testing::Headless` tests for UI behaviour, and by `mt-core` tests
   for I/O:
   - the sidebar opens a file from the tree in one click; `Ctrl+B` persists; the narrow layout auto-collapses;
   - the view button cycles in order, and the shortcut cycles too;
   - Shoji, decision 6 (lane 22-A):
     - cycling moves the divider to the matching stop;
     - dragging the divider snaps to the nearest of the three stops;
     - pressing again mid-slide retargets from the current position without a jump, in both cases:
       - the next stop is in the same direction (Edit to Split, pressed again mid-slide, continues toward Preview);
       - it is the opposite direction (Split to Preview, pressed mid-slide, goes back to Edit);
     - under reduced motion there is no translation, only opacity;
     - with the sidebar expanded, every open tab, including untitled and deleted-on-disk buffers, can be switched to
       in one click;
   - drop and reload motion (lane 22-B):
     - the drop screen text follows decision 6's confirmed, unknown and unsupported cases;
     - the drop screen slides in from the right edge, and under reduced motion it fades in with no translation;
     - the reload slip plays only after a successful adoption;
   - drops:
     - a drop event opens the files, a folder drop sets the workspace, and an unsupported drop shows the count notice;
     - coordinates are optional: Windows sends `FileDrop(uris)` with no position. Test that form as well as
       `FileDropAt`;
     - a drag that enters and then leaves without dropping clears the affordance. `Headless::drop_files` only sends
       `FileDrop`, so dispatch `FileDragEnter`/`FileDragLeave` directly or test the handler state;
   - external writes:
     - for a clean document, an external write triggers a reload and a notice; for a dirty one, it raises the banner;
       our own save triggers nothing;
     - a document that turns dirty during the reload's read keeps its edits and recovery record and shows the conflict
       banner;
     - when the sequence is snapshot read → delete or move event → the old read completes, the delete or move banner
       remains;
     - an external replacement of a symlink target outside the workspace reloads the clean tab, or raises the conflict
       on a dirty one;
   - watch scope:
     - a file opened with no workspace is watched;
     - a workspace with no tabs updates its tree;
   - rename races:
     - a concurrent external rename-over during a read neither fails the external writer nor corrupts our snapshot,
       for both an ordinary Markdown file and a `SKILL.md`;
     - test it with an injected race.
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

The outline view; tab reordering by drag; motion tuning beyond decision 6. Goal 21 follows and may tune durations
and curves, but keeps decision 6's spatial behaviour.

## Delivery lanes (after Goal 20 lane 20-A merges)

- **22-A Sidebar and view button:** `ui/rail.rs` (becomes the sidebar), `ui/shell.rs` layout, `ui/titlebar.rs` (the
  view button, and removing the view items from the menu), plus the settings field for the sidebar state. It also owns
  decision 6's sidebar panel, edge strip, single list, divider with snapping, and their motion.
- **22-B Files and events:** drag-and-drop handlers (the shell root and `state.rs` open paths), the watcher wiring in
  `state.rs`, the `mt-core` watcher and `document/io.rs` share modes and retry, and the reload and conflict paths. It
  also owns decision 6's drop screen and reload slip.

## Mergeable alone

Each lane is mergeable alone on `main`.
