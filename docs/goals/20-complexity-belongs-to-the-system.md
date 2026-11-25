# Goal 20 — Complexity belongs to the system

## Objective

The owner's rule, 2026-11-24: "Complexity belongs to the system, never the user; user experience is paramount."
Make MarkTurbo obey eight principles everywhere:

1. What the system can decide, it decides.
2. What is useless right now does not appear.
3. What the system can do for the user, it does.
4. The same thing works the same way everywhere.
5. Every press gets a response, and pressing again can take it back.
6. Never interrupt what the user is doing.
7. Common actions are easy; actions that can lose work are hard to trigger by mistake.
8. The interface explains itself.

Source: five code-reading audits (UX shell, flows, settings; motion ×2) judged by the Gemini product, copy and
interaction advisor (2026-11-24). The decisions below are the contract; the audits are background only.

## Owner decisions (2026-11-24)

- **Remove the tab preview slot.** Opening a file from `Ctrl+P`, the rail, search or Welcome always opens (or focuses)
  a normal tab. No hidden replacement rule; no italics; the Shift/double-click distinction goes away.
- Merge to `main` only; no tag or release.
- The four-tier confirmation policy below is adopted. No native `alert_dialog` remains except the OS file pickers.

## Done when

1. Every numbered decision in "UX decisions" below is implemented as specified, or explicitly re-scoped in the PR with
   the reason (never silently dropped).
2. The confirmation/undo policy, the universal Escape order and the focus-return and non-stealing invariants hold in
   every surface. Each is covered by a `nsh_testing::Headless` test (one per tier and per Escape layer at minimum).
3. All copy in the table is applied in English and Simplified Chinese; no user-visible English remains on Chinese
   paths that the audits listed (revision outcomes, refusals, credential errors, picker errors, context diagnostics).
   New strings go in the owning module's `S` enum or `i18n.rs` following existing conventions.
4. Destructive actions (Tier 2) are reversible within `UNDO_HOLD_MS` and only commit afterwards; a test proves Undo
   restores the item and that the deletion happens after the hold.
5. No behaviour required by earlier goals regresses: consent (hold-to-send, content-free evidence), credential
   confidentiality, scripts on/network off, preview never disabled, links to the system browser, Afterglow tokens.
6. `uv run --locked --project scripts scripts/mt.py check ci` passes; the owner-facing desktop walk-through
   (conductor, on the release build) covers each decision once; GPT review converges to P3-only.

## Failure control

- Any modal or native dialog added where the policy says none.
- Any confirmation added to a Tier 1 action.
- Any test or acceptance threshold weakened; any `docs/evidence/` edit.
- A new user-visible string without its Chinese counterpart.

## Out of scope

Motion and layout stability (Goal 21), the nsh WebView2 popup gap, release packaging.

## Delivery lanes (disjoint ownership; each lane is one PR)

- **20-A Shell & documents** — `ui/shell.rs`, `ui/rail.rs`, `ui/welcome.rs`, `ui/titlebar.rs`, `ui/notices.rs`
  (guard, conflict, recovery, status line), and the `state.rs` paths for tabs, open/save/close, guard, conflict,
  recovery and status. Decisions 2 (recovery half), 3, 5, 6 (guard/global part), 9, 10, 14, 15, 16 (titlebar, palette),
  18, 22, and the preview-slot removal; the Tier 2 Undo status-notice mechanism (shared by 20-C).
- **20-B Model flows** — `ui/review.rs`, `ui/revision.rs`, `ui/context.rs`, `ui/consent.rs`, and the `state.rs` paths
  for review, revision, translation and context. Decisions 1, 4, 6 (review/revision part), 8, 11, 13, 16 (context,
  review, revision parts), 17, 19 (flows part), 20.
- **20-C Settings & copy** — `ui/settings.rs`, `ui/fonts.rs`, `ui/prims.rs`, `i18n.rs`, and the `state.rs` paths for
  credentials and usage. Decisions 2 (credential, counts), 6 (Settings Escape), 7, 12, 16 (settings, fonts), 19
  (settings part), 21, and deleting the dead `*Help` keys. Uses 20-A's Undo notice API; until 20-A merges, it may add
  the minimal shared type behind the same name and resolve on merge.

`i18n.rs` edits: 20-C owns the shared `Key` strings; 20-A and 20-B put new strings in their modules' `S` enums and may
wire existing `Key`s. Lanes merge in the order A, B, C; each later lane merges `main` before review.

## Evidence plan

Headless tests per decision; `mt.py check ci`; GPT (`gpt-6.2-sol`) review per PR; conductor desktop walk-through on a
release build after all three lanes merge, recorded in `docs/goal-20-acceptance.md`.

## Mergeable alone

Each lane is mergeable alone on `main`.

## Contract amendments (GPT contract review and owner decision, 2026-11-24)

These override any conflicting text in the advisor body below.

1. **Consent authorization (owner decision).** One continuous hold of `motion::FILAMENT_CHARGE_MS` (900 ms)
   authorizes a send.
   - Releasing, blurring or cancelling resets the authorization to zero. The filament may drain visually, but a new
     press starts from 0; charge never resumes.
   - This applies to Review, Revision and Translation, with and without reduced motion. Keyboard holds
     (`Ctrl+Enter`) follow the same rule.
   - Tests: a short press does not send; split presses (for example 500 ms, release, 500 ms within the cool-down) do
     not send; a full continuous hold sends exactly once.
   - Remove the unused 1.2 s threshold (`FILAMENT_HOLD_MS` / `long_press_delay`) or align it to 900 ms. Update
     `docs/verification.md` and the native harness hold durations if they depend on 1.2 s; holding longer stays valid.
   - Lane: 20-B.
2. **Tier 2 Undo lifecycle.**
   - Each Undo binds to the exact target captured at press time. For a credential, that is the endpoint identity and
     the stored credential it removes; it is never re-resolved when it commits. If the endpoint or credential changed
     during the hold, the commit removes only the captured one, or does nothing if that is already gone.
   - There is one pending-undo slot. A second Tier 2 action commits the first pending one immediately, then starts its
     own.
   - A newer status message does not cancel a pending commit or hide its Undo: the Undo stays reachable on the status
     line until its deadline, on whichever surface is shown (decision 18 routing). Leaving Settings keeps the hold
     running.
   - `Clear counts`: events recorded during the hold count toward the cleared state; Undo restores the old counts
     plus those events.
   - Tests: two Discards in a row; an Undo notice overwritten by another notice; an endpoint switch during a
     credential hold.
3. **Save As result.** Replace the `bool` with three results: `Saved(new_tab_id)`, `Cancelled` and `Failed(reason)`.
   - Close-tab guard: `Cancelled` keeps the guard open with "Save cancelled — the document stays open."; `Failed` keeps
     it open with the failure copy; `Saved` closes the guard and acts on the new tab id.
   - Quit guard: the same rules; after `Saved`, continue to the next dirty tab.
4. **Review preservation (decision 1).**
   - Preparing a new Review snapshots the whole Review session: `review_view`, `review_binding`, the selection scope,
     the stale tracking and the Revision state. Cancel restores the snapshot unchanged, and stale tracking continues.
   - Closing the field hides the result without clearing it, and the titlebar Review reopens it.
   - Results are per tab; switching tabs shows that tab's result or none.
5. **Escape and focus.**
   - Escape on a waiting Review closes the wait and discards the late result by generation. The notice says nothing
     will be applied, and never claims the request was recalled.
   - Escape on a busy Revision cancels its transport.
   - Escape after a Translation is sent behaves like Review.
   - Focus return: if the active pane mounts the source editor (Source or Split), focus it. In Preview or Web, focus the
     document view, and never switch panes. If another layer is still open, focus goes to that layer.
6. **Settings persistence (decision 7).** Save each field independently, on blur and on choice press, never per
   keystroke. Save valid values only: an invalid Base URL shows its error, is not saved, and does not block saving the
   other fields. This replaces the `on_change` wording.
7. **Asynchronous folder open (decision 9).** Each open carries a request token, and only the latest request may
   commit the root, files, context, recents and status. Test it with a slow first open and a fast second one that
   completes in reverse order.
8. **nsh API (decision 12).** Use nsh 0.6.1's unnamed `.group()`, `.group_hover(f)` and `.group_focus_visible(f)`, as
   `ui/titlebar.rs` does, so keyboard focus gets the same label feedback as hover. The named variants are
   `.group_named` and `.group_hover_named`.
9. **Copy corrections.**
   - UTC day 20410 is `2025-11-18`.
   - `ModelHelp`: "Leave empty to use MARKTURBO_MODEL, then MARKTURBO_TRANSLATE_MODEL, then the wire format's default
     model." / "留空则依次使用 MARKTURBO_MODEL、MARKTURBO_TRANSLATE_MODEL，再使用该协议格式的默认模型。" Keep that fallback
     chain in code.
10. **Chinese error scope (Done when 3).**
    - These error categories must render in the UI language: revision outcomes and refusals, credential errors, picker
      errors, save, reload and overwrite errors, translation results and errors, and context diagnostics.
    - Each must have a localized outer message with a next step.
    - An embedded `{why}` / `{error}` may stay the operating system's text (Windows localizes it). It must never contain
      document text, secrets or credential material.
    - This exception amends Done when 3: OS-provided detail may appear in the OS language. Detail the app itself
      produces, such as `TranslationError::InvalidTranslationResponse` or provider and parser messages, is not OS text.
      It gets a localized mapping, or it is omitted.
11. **Keyboard release ends the hold (amends 1).**
    - A keyboard hold ends on the actual release of `Enter` or of the modifier. Observe it with nsh 0.6.1's view
      `KeyUp` events (`nsh-platform-windows` `keyboard.rs`) wherever the focus is.
    - The silence timer (`KEY_ALIVE_MS`) may still end a hold whose release went unobserved. It never authorizes.
    - Test: press at 0 ms, an auto-repeat at 500 ms, a `KeyUp` at 550 ms. Nothing is sent at 900 ms or later.
    - `KeyUp` reaches only a focused custom view, while `Ctrl+Enter` is focus-independent: a native text input can hold
      the focus.
      - So at the authorization moment, confirm through a platform key-state query that `Enter` and the modifier are
        still physically down. On Windows, `GetAsyncKeyState` for `VK_RETURN` and `VK_CONTROL`.
      - Unconfirmed means no send. Where no reliable query exists (the non-Windows compatibility targets), keyboard
        holds never authorize, and the pointer hold remains.
    - Test through an injectable key-state source: a key reported up at 900 ms with the text input focused sends
      nothing; a key reported down sends exactly once.
    - Continuity, not just the end state:
      - Sampling cannot see a release and re-press between two samples. So observe every transition instead:
        - On Windows, install a thread keyboard hook only while a keyboard hold charges:
          `SetWindowsHookExW(WH_KEYBOARD, …, 0, GetCurrentThreadId())` on the UI thread.
        - The hook sees `WM_KEYUP`/`WM_KEYDOWN` for the thread wherever the focus is, native text inputs included. It is
          removed when the hold ends.
      - Any up transition of `Enter` or the modifier cancels the hold to zero, and so does any non-repeat down
        transition of either key.
      - If the hook cannot be installed, keyboard holds do not authorize. On platforms without such a hook, the same
        applies, and the pointer hold remains.
      - Tests, through an injectable transition source:
        - Ctrl released at 548 ms and pressed again at 556 ms while Enter stays down, with the text input focused.
          Nothing is sent.
        - A hook that fails to install sends nothing.
      - The end-state query above stays as a final check.
      - **Queued input is never skipped.** A transition can be queued while the hook has not yet seen it, for example
        when the UI thread was busy and nsh's posted wake ran first.
        - Authorize only when all three hold at that moment:
          1. `GetQueueStatus(QS_KEY | QS_RAWINPUT)` reports no pending keyboard input;
          2. the hook has seen no transition;
          3. `GetAsyncKeyState` reports both keys down.
        - Otherwise defer the decision. Let the message loop deliver the queued input, and decide again at the next
          turn. A transition delivered then cancels the hold.
        - Test through injectable queue-status, key-state and transition sources: a release and re-press queued at
          886/894 ms, while the hook has not yet seen them at 900 ms, sends nothing.
    - **Superseded by amendment 15.** The thread hook and `GetQueueStatus` above do not work in nsh 0.6.1.
15. **Keyboard hold mechanism (replaces the mechanism in amendment 11; PR #72 review).**
    - **Why the old one fails.** nsh 0.6.1 receives keyboard messages on its Win32 window thread and runs app callbacks
      on a separate engine thread (`nsh-platform-windows` `surface.rs`).
      - A `WH_KEYBOARD` thread hook installed from a callback watches the wrong thread.
      - `GetQueueStatus` only describes the calling thread's queue.
    - **Mechanism.**
      - While a keyboard hold charges, run a low-level keyboard hook (`WH_KEYBOARD_LL`) on a dedicated hook thread
        that pumps its own message loop. The hook is installed when the hold starts and removed when it ends.
      - The hook proc only records `Enter`/modifier transitions into an atomic, and always calls `CallNextHookEx`.
      - A low-level hook sees input before any thread queue and before `GetAsyncKeyState` changes. So at the decision
        moment, "no transition recorded and both keys down per `GetAsyncKeyState`" proves a continuous hold, up to a
        release that lands at the deadline itself, which is acceptable.
      - Drop `GetQueueStatus`.
    - **Fail closed.** If the thread or hook cannot start, or is removed early, the keyboard hold never authorizes.
      Removal failure keeps the handle and refuses a new install until it is removed.
      - Install and remove are paired on every path: success, cancel, card rebuild, unmount. Test that pairing with a
        counting fake.
    - **One input path per hold.**
      - A hold is owned by the input that started it: the pointer, `Ctrl+Enter`, or `Enter`/`Space` on the focused
        Send control.
      - A press through another path, or a second non-repeat press on the same path, cancels the hold to zero and its
        completion timer with it. A new hold starts from 0.
      - `Enter`/`Space` on the Send control use the same hook-plus-key-state proof as `Ctrl+Enter`.
    - **Window deactivation cancels.** Losing the foreground window (`WM_ACTIVATE` inactive or nsh's window-blur
      event), from any focus, cancels any hold. At decision time the foreground window must belong to this process.
    - **Tests** use injectable hook, key-state and foreground sources:
      - Ctrl released and pressed again mid-hold sends nothing;
      - a pointer press during a keyboard hold, followed by holding the pointer for the rest, sends nothing;
      - two short `Enter` presses on Send, 500 ms apart, send nothing;
      - window deactivation with the keys still down sends nothing;
      - a hook that fails to start, or fails to remove, never authorizes and does not leak;
      - a clean continuous hold sends exactly once.
16. **Consent keys follow focus (owner decision, 2026-11-25; replaces amendments 11 and 15).**
    - **Why.** Five review rounds found authorization gaps in every focus-independent design: thread hooks, queue
      checks, and low-level hooks racing the async key state. The owner chose the focus-based mechanism instead.
    - **Focus.** The consent card takes keyboard focus when it opens. Focus stays inside the card (Send and Cancel)
      while it is open.
    - **Hold.** A keyboard hold is measured only from `KeyDown`/`KeyUp` events delivered to the focused card:
      - `Ctrl+Enter` anywhere in the card;
      - `Enter`/`Space` on Send.
    - **Cancel.** Any of these cancels the hold to zero, together with its completion timer, and a new hold starts
      from 0:
      - a `KeyUp` of `Enter` or the modifier;
      - a non-repeat `KeyDown` of either key;
      - blur, card unmount, or window deactivation;
      - a press through another input path.
    - **First press.** The press that opened the card never counts. The card arms only after that key is released, as
      it does today.
    - **Removed.**
      - The system keyboard hook, the hook thread, `GetQueueStatus`, the `GetAsyncKeyState` and foreground polling,
        and the `test-harness` keyboard source injection.
      - Headless tests drive real `KeyDown`/`KeyUp` events into the focused card instead.
    - **Unchanged.** The pointer hold works on every platform, and one input path per hold still applies.
    - **`AGENTS.md`.** Its consent-key invariant is updated in the same PR.
    - **Tests:**
      - continuous hold sends once;
      - release and re-press mid-hold sends nothing;
      - blur or window deactivation mid-hold sends nothing;
      - a mixed pointer/keyboard path sends nothing;
      - a short press sends nothing;
      - the opening press does not arm;
      - the card holds focus when it opens.
12. **Undo commit boundary (amends Done when 4 and amendment 2).**
    - A Tier 2 action commits when its `UNDO_HOLD_MS` ends, or earlier when a newer Tier 2 action takes the single
      pending slot. When the slot is taken, the older action's Undo ends at the same moment.
    - Test both paths.
13. **Quit re-checks dirty tabs (amends 3).**
    - After each Save As and before closing the window, Quit reads the current set of dirty tabs, not a list taken when
      the quit started.
    - A tab that became dirty during the guard is guarded too, for example when a translation returns.
    - Test that interleaving.
14. **Restored Review recomputes stale (amends 4).**
    - Cancel restores the results, answers and Revision state from the snapshot.
    - Stale is recomputed against the document's current revision, so a result the document has moved past shows as
      stale.
    - Test: an edit or a translation write while the new consent card is open.
17. **`Clear counts` commits at press; Undo adds the captured counts back (conductor, 2026-11-25; amends 2 and 12 for
    this action only).**
    - Usage counts are local statistics.
      - With a deferred commit, each review round surfaced another multi-instance or crash race in a generation and
        watermark scheme.
      - That scheme also kept a counter that leaked past activity volume.
    - `Clear counts` captures the current aggregate, then clears the store at once with the existing atomic clear.
      - A corrupt or unsupported `usage.json` is deleted.
      - A read failure deletes the file when it is a regular file, and otherwise reports the error.
    - The Undo, live for `UNDO_HOLD_MS` in the shared single slot, **adds** the captured counts back into whatever the
      store holds by then. Events recorded during the hold are kept either way.
    - Remove the clear-generation and watermark state, and any persisted counter that survives a clear.
    - Tests:
      - clear, then record an event, then Undo restores old plus new;
      - clear, then record an event, then expiry keeps only the new event;
      - a corrupt file is cleared;
      - nothing survives a clear except events recorded after it.
18. **An Undo shows on the visible surface (conductor, 2026-11-25; replaces the routing in the contract notes
    below).**
    - The live Undo renders on exactly one status line: the one on the surface the user currently sees. Settings when
      Settings is open; otherwise the Review status line when the Review panel is open; otherwise the editor's.
    - This is computed from the current UI state on every change, so it never depends on where its notice was first
      routed.
    - Lane 20-A owns this resolver in the shared Undo API. Lane 20-C adopts it when it merges `main`.
    - Tests: Review closed while an Undo is live; Settings opened from the Review panel while an Undo is live; a
      background notice arriving while an Undo is live.

17. **`Clear counts` commits at press; Undo adds the captured counts back (conductor, 2026-11-25; amends 2 and 12 for
    this action only).**
    - Usage counts are local statistics.
      - With a deferred commit, each review round surfaced another multi-instance or crash race in a generation and
        watermark scheme.
      - That scheme also kept a counter that leaked past activity volume.
    - `Clear counts` captures the current aggregate, then clears the store at once with the existing atomic clear.
      - A corrupt or unsupported `usage.json` is deleted.
      - A read failure deletes the file when it is a regular file, and otherwise reports the error.
    - The Undo, live for `UNDO_HOLD_MS` in the shared single slot, **adds** the captured counts back into whatever the
      store holds by then. Events recorded during the hold are kept either way.
    - Remove the clear-generation and watermark state, and any persisted counter that survives a clear.
    - Tests:
      - clear, then record an event, then Undo restores old plus new;
      - clear, then record an event, then expiry keeps only the new event;
      - a corrupt file is cleared;
      - nothing survives a clear except events recorded after it.
18. **An Undo shows on the visible surface (conductor, 2026-11-25; replaces the routing in the contract notes
    below).**
    - The live Undo renders on exactly one status line: the one on the surface the user currently sees. Settings when
      Settings is open; otherwise the Review status line when the Review panel is open; otherwise the editor's.
    - This is computed from the current UI state on every change, so it never depends on where its notice was first
      routed.
    - Lane 20-A owns this resolver in the shared Undo API. Lane 20-C adopts it when it merges `main`.
    - Tests: Review closed while an Undo is live; Settings opened from the Review panel while an Undo is live; a
      background notice arriving while an Undo is live.

## Contract notes (conductor, 2026-11-24)

- **Decision 22, untitled numbering.** Use the smallest positive integer not taken by another open untitled tab (`Untitled 1`, `Untitled 2`...), not the internal tab id.
- **Decision 20, context assumptions.** Collapse them behind the disclosure; never delete them. `PRODUCT.md` keeps them inspectable.
- **Tier 2 Undo notices.** Show them on the status line of the surface where the action happened (editor, Settings, Review), following decision 18's routing.
- **Decision 7, Settings auto-save.** Persist only valid values. An invalid Base URL shows its inline error and keeps the last valid value saved. Save fields on blur and on choice press, never on every keystroke.

---

# Advisor decisions (contract body)

## 1. UX Decisions (Ranked Change List)

### Theme 1: Destructive Actions and Undo
*Principle A: Common actions are easy and actions that lose work are hard to trigger by mistake; every press gets a response and pressing again can undo.*

1. **Preserve shown Review results when preparing a new Review or closing the Review field**
   - **What changes for the user:** Triggering a new Review (`Ctrl+Enter` or `Review`) no longer erases the current result before consent completes, and pressing `Close` in the Review footer does not permanently destroy the result.
   - **Principle:** A (actions that lose work are hard to trigger by mistake; pressing again / backing out can undo).
   - **Resolves:** `ux-flows` #2, #3, #10.
   - **Conflict resolution:** `ux-flows` #10 suggested adding a confirmation dialog or relabeling `Close` to `"Discard review"`. **Rejected:** Closing a reading pane is a frequent navigation act and must never trigger a confirmation card. Instead, preserve state so closing and canceling are non-destructive.
   - **Behaviour to build:**
     - In `state.rs::start_review`, do **not** call `review_view.set(None)` or clear `review_unavailable` during preparation. Keep the existing `review_view` mounted behind the consent card.
     - Replace `review_view` only inside `send_review` when the user completes the hold-to-send gesture and `review_busy` becomes `true`.
     - If `cancel_review` is called from the consent card: if `review_view.get().is_some()`, keep `review_open = true` and leave the previous result on screen; if `review_view.get().is_none()`, set `review_open = false` and call `focus_editor()` (avoiding the dead-end `"No review result yet."` screen).

2. **Make Recovery `Discard` and Settings `Clear counts` / `Remove` credential reversible via a 5-second Undo status notice**
   - **What changes for the user:** Stray clicks on `Discard` (recovery banner), `Remove` (saved API credential), or `Clear counts` (local usage counters) no longer destroy data immediately or pop up modal dialogs; they take effect immediately and show an inline `Undo` action in the status bar.
   - **Principle:** A (every press gets a response and pressing again can undo; never interrupt what the user is doing).
   - **Resolves:** `ux-shell` #16, `ux-settings` F1, F7.
   - **Conflict resolution:** `ux-shell` #16 and `ux-settings` F1/F7 proposed either a confirmation card or an undo notice. **Decision:** Use soft-delete + Undo status notice instead of confirmation modals. Reserve the in-app guard card strictly for active buffer vs. disk/close loss.
   - **Behaviour to build:**
     - Extend `StatusMessage` in `state.rs` and `notices::status_line_at` with an optional `action: Option<(SharedString, Rc<dyn Fn()>)>`.
     - When the user presses `Discard` on a recovery row (or `Discard all`), remove the row(s) from `recovery_records` in memory and start a `5000 ms` (`UNDO_HOLD_MS`) timer before calling `checkpoint_store.invalidate_and_delete()`. Show `"Discarded recovered draft · Undo"`. Pressing `Undo` cancels the timer and restores the row(s).
     - Apply the same 5-second soft-delete buffer to `remove_key` in `settings.rs` and `clear_usage` in `state.rs`. Style `Remove` and `Clear counts` labels with `tokens::palette().danger` on hover/focus.

3. **Unify external disk conflict confirmations (`Keep mine` / `Use disk version`) into the in-app guard card**
   - **What changes for the user:** Resolving a file conflict uses the quiet in-app Afterglow guard card instead of jarring OS-native modal alerts (`alert_dialog()`), respecting the app's theme, typography, and Chinese localization.
   - **Principle:** A (the same thing works the same way everywhere; never interrupt with native OS modals).
   - **Resolves:** `ux-shell` #7, #6 (dialog half).
   - **Behaviour to build:**
     - Expand `GuardRequest` in `state.rs` from `{ tab, choice, busy, notice }` into a `GuardKind` enum: `CloseTab(u64)`, `QuitApp`, `ReloadFromDisk(u64)`, and `OverwriteDisk(u64)`.
     - Remove `alert_dialog()` from `reload_active` and `overwrite_active`; set `state.guard` with the corresponding `GuardKind`, localized title/body (`Key::ReloadFromDisk`, `Key::Overwrite`), and two actions (`Reload` / `Cancel` or `Overwrite` / `Cancel`, with `Cancel` ringed by default).

4. **Unify Translation outbound consent into `ui::consent::card`**
   - **What changes for the user:** Translation uses the exact same in-app hold-to-send consent card as Review and Revision instead of a one-click OS native modal dialog.
   - **Principle:** A (the same thing works the same way everywhere; owner invariant: explicit hold-to-send consent before any model request).
   - **Resolves:** `ux-flows` #9, `ux-shell` #7 (consent contrast).
   - **Behaviour to build:**
     - Add `ConsentKind::Translation` to `ui/consent.rs` (with `S::HeadlineTranslation`: `"Send for translation?"` / `"发送以进行翻译？"`).
     - In `state.rs::translate_active`, populate `translation_consent` / `translation_manifest` and mount `consent::card` via `shell.rs` instead of calling `alert_dialog()`.

5. **Keep close/quit guard open when untitled `Save As` picker is cancelled**
   - **What changes for the user:** If a user chooses `Save` on the close/quit guard for an untitled buffer and then cancels the native `Save As` file dialog, the app no longer silently drops the guard; it returns to the guard card with a clear status line.
   - **Principle:** A (every press gets a response; actions that lose work are clear).
   - **Resolves:** `ux-shell` #12.
   - **Behaviour to build:**
     - In `state.rs::guard_save`, keep `guard` open (`busy = true`) while `save_as_tab` awaits the native file picker. If `save_as_tab` returns `false` (picker cancelled), set `busy = false` and set `request.notice` to localized `"Save cancelled — the document stays open."` / `"已取消保存——文档保持打开。"`.

---

### Theme 2: Consistency of Confirmations, Verbs, Escape, and Focus
*Principle A: The same thing works the same way everywhere.*

6. **Universal `Escape` and focus-restoration contract across Guard, Settings, Review, and Revision**
   - **What changes for the user:** Pressing `Escape` consistently backs out of the topmost active layer (`Settings`, `Review` result, `Revision` busy wait, `Guard`, `Compare`), and closing any modal or overlay returns keyboard focus to the source editor so typing works immediately.
   - **Principle:** A (the same thing works the same way everywhere).
   - **Resolves:** `ux-shell` #11, `ux-flows` #4, `ux-settings` F11.
   - **Behaviour to build:**
     - `state.rs::guard_cancel` and post-close paths: clear `self.guard.set(None)` first, then call `self.focus_editor()` if `self.active.get().is_some()`.
     - `ui/settings.rs`: bind `escape` (`When::Always`) to `state.close_settings()`; in `close_settings()`, call `self.focus_editor()`. Add `mod+,` (`Ctrl+,` / `⌘,`) global shortcut to toggle Settings.
     - `ui/review.rs`: bind `escape` (`When::NotEditing`) when a review result or unavailable/cancelled state is open to `state.close_review()` + `state.focus_editor()`.
     - `ui/revision.rs`: bind `escape` (`When::Always`) while `state.revision_busy.get()` is true to `state.cancel_revision()`, and render the `Esc` kbd hint in the busy row matching `review.rs`.

7. **Eliminate dual save models on the Settings page (Auto-save all settings on edit)**
   - **What changes for the user:** Endpoint wire format, Base URL, Model, and Target language persist automatically as they are edited—matching Language, Theme, and Usage toggles. The redundant `Save` button is removed from the Settings header, leaving a single `Close` (`Esc`) action.
   - **Principle:** A (what the system can decide it decides; the same thing works the same way everywhere).
   - **Resolves:** `ux-settings` F5.
   - **Behaviour to build:**
     - In `ui/settings.rs`, trigger `state.save_settings_quietly()` on provider choice press and on input `on_change` / blur (persisting valid edits immediately without flashing a `"Settings saved."` toast on every keystroke).
     - Remove `settings-save` button from `settings.rs:82-86`. Keep `settings-close` (`Close` / `Esc`).

8. **Standardize footer action placement and disambiguate `Dismiss` vs. `Close`**
   - **What changes for the user:** Primary actions always sit at the leading (left) edge of field footers, and dismissing a revision status notice says `Dismiss` (`关闭提示`) instead of `Close` (`关闭`).
   - **Principle:** A (the same thing works the same way everywhere; self-explaining copy).
   - **Resolves:** `ux-flows` #6, #11, #18; `ux-shell` #8.
   - **Behaviour to build:**
     - In `revision.rs:522`, change the outcome status button from `Key::Close` to `Key::RevisionDismiss` (`"Dismiss"` / `"关闭提示"`).
     - In `revision.rs:922-941`, left-align the footer row (`Done` primary first, then `Copy`, then `Close`) to match `review.rs` (`Prepare revision` first, `Close` second) and `context.rs` (`Review` first, `Refresh` second, `Close` third).
     - Unify `S::TryAgain` and `S::ReviewAgain` in `review.rs`: use `"Try again"` / `"重试"` only when a request failed (timeout/network/malformed), and `"Review again"` / `"重新审查"` when re-running after edits or cancellation. Fix `revision.rs` `S::ReviewAgain` Chinese from `"重新审阅"` to `"重新审查"`.
     - Unify Welcome and Titlebar labels: `"Open folder…"` / `"打开文件夹…"`, `"Paste as new document"` / `"粘贴为新文档"`, and `"Compatibility preview"` / `"兼容预览"`.

---

### Theme 3: Feedback and Responsiveness
*Principle A: Every press gets a response and pressing again can undo.*

9. **Run `open_folder` directory walk on a background worker with immediate status**
   - **What changes for the user:** Opening a large repository no longer freezes the window on click; the UI stays responsive and shows `"Opening folder…"` in the status bar.
   - **Principle:** A (every press gets a response) + B (costs no frames).
   - **Resolves:** `ux-shell` #13.
   - **Behaviour to build:**
     - In `state.rs::open_folder`, immediately set status `"Opening folder…"` / `"正在打开文件夹…"` and spawn the depth-4 `read_dir_deep` walk on `blocking(...)` inside `spawn_task`, updating `self.root` and `self.files` upon completion.

10. **Make Conflict `Compare` stateful (`Compare` ↔ `Hide comparison`) and report disk-read failures**
    - **What changes for the user:** Pressing `Compare` on the conflict banner visibly toggles the button to `Hide comparison` (and shows selected underline/ink); if the file on disk cannot be read, an error appears instead of a silent no-op.
    - **Principle:** A (every press gets a response and pressing again can undo).
    - **Resolves:** `ux-shell` #10, `ux-settings` (Usage `Inspect counts` toggle).
    - **Behaviour to build:**
      - In `notices.rs:332`, bind the button label to `compare_open`: `S::Compare` (`"Compare"` / `"比较"`) when closed, `S::HideComparison` (`"Hide comparison"` / `"收起比较"`) when open. If `disk_text` read fails in `toggle_compare`, call `state.notify_failure(Key::ReloadFailed, &err)`.
      - Similarly, in `settings.rs:892`, toggle `Key::UsageInspect` between `"Show details"` / `"显示明细"` and `"Hide details"` / `"隐藏明细"`.

11. **Show status feedback when pressing `Copy` in Revision**
    - **What changes for the user:** Pressing `Copy` in the Revision footer shows `"Approved result copied to the clipboard."` in the status line.
    - **Principle:** A (every press gets a response).
    - **Resolves:** `ux-flows` #5.
    - **Behaviour to build:**
      - In `revision.rs:1169-1174`, after writing `revision_approved_text()` to the clipboard, call `state.notify_at(StatusSurface::Review, i18n::t(language, Key::RevisionCopied))`.

12. **Fix button hover/focus ink inheritance in `prims.rs`**
    - **What changes for the user:** Hovering or focusing any `quiet_action`, `primary_action`, or `choice_button` smoothly transitions its text label to full ink (`tokens::palette().ink`), matching menu rows.
    - **Principle:** A (every press/hover gets a response) + B (one motion language project-wide).
    - **Resolves:** `ux-settings` F13, `motion-a` Top 10 #7.
    - **Behaviour to build:**
      - Because `nsh-core` stops `text_color` inheritance at any child `text(...)` node that sets its own color, add `.group("action")` to the outer button in `prims::quiet_action`, `prims::primary_action`, `settings::choice_button`, and `settings::reactive_action`, and apply `.group_hover("action", |s| s.text_color(|| tokens::palette().ink).underline())` plus `motion::ease_out(text.transition_colors().duration(HOVER_MS))` directly on the child `text` element (as `titlebar.rs:731` already does).

---

### Theme 4: Do It For Them
*Principle A: What the system can decide it decides; what it can do for the user it does.*

13. **Preserve `"Include in review"` selections across Agent Context `Refresh`**
    - **What changes for the user:** Pressing `Refresh` in the Context field keeps previously checked instruction files selected if they still exist in the resolved chain, instead of wiping the user's choices.
    - **Principle:** A (what it can do for the user it does; never discard user work).
    - **Resolves:** `ux-flows` #7.
    - **Behaviour to build:**
      - In `state.rs:1334`, instead of `self.context_selected.set(Vec::new())`, retain any path in `context_selected` that still matches a source's canonical/normalized path in the newly resolved `ResolvedContext.sources`.

14. **Add `Restore all` and `Discard all` to the recovery banner when 2+ files are recovered**
    - **What changes for the user:** After a crash with multiple unsaved drafts, the user can restore or discard all of them in one click.
    - **Principle:** A (what it can do for the user it does).
    - **Resolves:** `ux-shell` #3.
    - **Behaviour to build:**
      - In `notices.rs::recovery_banner`, when `rows.get().len() > 1`, render a header action row with `prims::primary_action("recovery-restore-all", i18n::t(language, Key::RestoreAll), ...)` calling `state.restore_all_recovered()` and `prims::quiet_action("recovery-discard-all", i18n::t(language, Key::DiscardAll), ...)` calling `state.discard_all_recovered()` (wired to the Undo toast from Item #2).

15. **Auto-prune or offer one-click removal for missing Recent entries, and remember last Pane**
    - **What changes for the user:** Missing files/folders in the Welcome screen's 5-slot Recent list have a `Remove` action (and are removed automatically if clicked and missing), and MarkTurbo remembers whether the user last worked in `Source`, `Preview`, or `Split`.
    - **Principle:** A (what the system can decide it decides; what it can do for the user it does).
    - **Resolves:** `ux-shell` #1, #4, #5.
    - **Behaviour to build:**
      - Persist `chosen_pane` in `SettingsData` on change and restore it on startup.
      - On `welcome.rs` Recent rows where `missing` is true, replace the static `"Missing"` badge with a pressable `prims::quiet_action` labeled `"Missing · Remove"` / `"已不存在 · 移除"` calling `state.remove_recent_target(&path)`.
      - Add `Open sample` (`Key::OpenSample`) to the Titlebar menu so the bundled sample remains reachable after a workspace is opened.

---

### Theme 5: Hide What Is Useless Right Now
*Principle A: What is useless right now is not shown.*

16. **Gate unavailable controls across Titlebar menu, Context field, Settings credentials, and Revision keys**
    - **What changes for the user:** Controls that can only fail if clicked are hidden or disabled in place with a clear reason.
    - **Principle:** A (what is useless right now is not shown).
    - **Resolves:** `ux-shell` #2, #21; `ux-flows` #8, #14, #16; `ux-settings` F2, F12, F16.
    - **Behaviour to build:**
      - **Titlebar menu `Context` (`titlebar.rs:553`):** Gate inside the existing `when(move || state.root.get().is_some() || state.active.get().is_some())` block so it is not shown on an empty Welcome screen.
      - **Context field `Review` (`context.rs:1022`):** Disable `review_action` (`disabled_style`) when `state.active.get().is_none()`.
      - **Settings `Store securely` (`settings.rs:542`):** Hide `Store securely` when `!state.secure_store_supported()` (macOS/Linux without keychain persistence) and show `Key::SecureStoreUnavailable` in the status line; disable both `Store securely` and `Use for session` when `CredentialStatus::Unconfigured` (no provider selected or invalid Base URL).
      - **Settings CJK Font (`fonts.rs:366-374`):** When `Failed(_)` and `on_disk == true`, show **both** `Retry` (primary) and `Remove` (quiet), never `Remove` alone.
      - **Review `"No model set up"` (`review.rs:1286-1338`):** Render `Try again` (`start_review`) alongside `Open settings` and `Close`.
      - **Revision hotkeys (`revision.rs:376`):** Gate `A`, `R`, `J`, `K`, and `Ctrl+Z` keybindings on `!state.revision_hunks.get().is_empty()`.
      - **Palette (`rail.rs:713`, `state.rs:1632`):** When `root` is `None` and no tabs are open, show `"Open a folder first."` inside the palette instead of a blank list or `"No matches."`, and do not set `search_ran = true` before the `root` check.

---

### Theme 6: Never Interrupt What the User Is Doing
*Principle A: Never interrupt what the user is doing.*

17. **Do not steal keyboard focus from the source editor when a Review result arrives**
    - **What changes for the user:** If the user continues typing in the Markdown editor while waiting 10–30 seconds for a Review, the arriving result no longer yanks focus into the question input box mid-sentence.
    - **Principle:** A (never interrupt what the user is doing).
    - **Resolves:** `ux-flows` #17.
    - **Behaviour to build:**
      - In `review.rs:241-245` and `1649-1652`, check whether the source editor currently holds focus before calling `focus_soon` on the first question's answer input. Only auto-focus the answer input if the user is not actively focused in the source editor (e.g., focus was on the Review panel, titlebar, or nowhere).

18. **Surface background errors (such as recovery checkpoint failure) on the active room**
    - **What changes for the user:** If a background recovery checkpoint fails while Settings is open, the notice appears on the Settings status line rather than being silently dropped into the unmounted editor status slot.
    - **Principle:** A (every failure is visible without interrupting).
    - **Resolves:** `ux-shell` #15.
    - **Behaviour to build:**
      - In `state.rs::notify_error`, route the target `StatusSurface` to `StatusSurface::Settings` when `self.settings_open.get()` is true, and `StatusSurface::Column` otherwise.

---

### Theme 7: Self-Explaining Copy and Localization
*Principle A: The interface explains itself; what the system can decide it decides.*

19. **Route all hardcoded English state/error strings and resolver diagnostics through `i18n`**
    - **What changes for the user:** When the interface is set to 简体中文, revision outcomes, hunk refusals, credential errors, file picker errors, and Agent Context diagnostics all appear in natural Simplified Chinese with a concrete recovery step.
    - **Principle:** A (the interface explains itself; same language everywhere).
    - **Resolves:** `ux-shell` #6, #20, #23; `ux-flows` #1, #12, #13, #19; `ux-settings` F3.
    - **Behaviour to build:**
      - Wire the unused `Key::RevisionReady`, `Key::RevisionAppliedSavePending`, `Key::RevisionNoApprovedChanges`, `Key::RevisionCopied`, `Key::QuitTitle`, `Key::QuitSaveFailed`, `Key::CredentialWriteFailed`, `Key::CredentialDeleteFailed`, etc. in `state.rs` using `self.settings.get().language`.
      - Map `context.rs` diagnostic codes (`context.duplicate-source`, `context.duplicate-directive`, file read errors) to localized `i18n` templates instead of printing raw internal English strings.

20. **Hide internal contract assumptions in the Agent Context field behind a collapsible disclosure**
    - **What changes for the user:** Opening `Context` (see screenshot 7: `en-us-effective-agent-context-default-light`) no longer floods the panel with 5 paragraphs of internal verification boilerplate (`"Assumption: This profile freezes inspected source semantics at upstream revision b8c86376a258e55efc8e5ecfbabc21c16c07d814..."`). The user sees the clean list of instruction files (`1 AGENTS.md`) and a quiet toggle for verification details.
    - **Principle:** A (complexity belongs to the system, never the user; what is useless right now is not shown; the interface explains itself).
    - **Resolves:** `ux-flows` #15 + **Visual Audit of Screenshot 7**.
    - **Behaviour to build:**
      - In `ui/context.rs`, remove the 5 `Assumption:` paragraphs from the primary flow. If `PRODUCT.md` contract transparency requires keeping them inspectable, place them behind a single `prims::quiet_action("context-profile-details", "Profile rules & assumptions" / "规则与假设说明", ...)` toggle (collapsed by default), matching `All fields` in `review.rs`.
      - Rewrite `S::StatusInventory` from `"Inventory only — no verified profile applies"` to `"Listed for information — Codex rules are not verified for this file."` / `"仅供查看——尚未验证此文件的 Codex 规则。"`.

21. **Render essential Settings help lines, rename `Provider` → `Wire format`, and format Usage UTC days as dates**
    - **What changes for the user:** Settings explains what `"Best available"` and empty `Model` / `Base URL` fields do, explains that HTTP requires a numeric loopback address (`127.0.0.1`), uses `"Wire format"` / `"协议格式"` consistently with the consent card, and shows `2025-11-20` instead of `UTC day since 1970-01-01: 20410`.
    - **Principle:** A (the interface explains itself).
    - **Resolves:** `ux-settings` F4, F6, F8, F9, F10, F14.
    - **Behaviour to build:**
      - Render `Key::ProviderHelp` and `Key::ModelHelp` in `prims::faint` under their respective rows in `settings.rs`.
      - Rename `Key::Provider` to `"Wire format"` / `"协议格式"`.
      - Convert `day_key * 86_400` in `i18n::usage_details` into `YYYY-MM-DD` civil date formatting (using standard Howard Hinnant civil-from-days arithmetic in pure Rust, zero dependencies).
      - Add accessible label `i18n::t(language, Key::UsageCounters)` to `choice_row("usage-toggle", ...)`.
      - Use platform-aware `titlebar::shortcut("mod+p")` in `welcome.rs` (`Ctrl+P` on Windows/Linux, `⌘P` on macOS).

22. **Distinguish numbered Untitled buffers (`Untitled 1`, `Untitled 2`) and show parent folders for duplicate filenames**
    - **What changes for the user:** Multiple unsaved drafts are named `Untitled 1`, `Untitled 2` in the rail, titlebar, and quit guard; same-named files (`plan.md` in two folders) include their parent folder (`drafts/plan.md`, `archive/plan.md`) in the guard card and Recent list.
    - **Principle:** A (the interface explains itself; actions that lose work are clear).
    - **Resolves:** `ux-shell` #18, #19, #22, #24.
    - **Behaviour to build:**
      - In `state.rs:1142-1170`, format untitled tab titles using their counter ID (`Untitled {id}` / `未命名 {id}`).
      - In `notices.rs::documents_line` and `welcome.rs` recents, when two entries share a `file_name()`, prepend the parent directory name (`parent/name.md`).
      - Give the titlebar filename button an accessible label `"Show agent context for {name}"` / `"显示 {name} 的智能体上下文"`, and give the dimmed `Review` button an accessible description when blocked by a docked revision (`"Finish or close the current revision first"` / `"请先完成或关闭当前修订"`).

---

> Resolved by the owner on 2026-11-24: Decision A — remove the preview slot; Decision B — keep the editor mounted (Goal 21, lane 21-D).


## 2. One Confirmation and Undo Policy

Every action in MarkTurbo belongs to **one of four tiers**. Native OS modal alerts (`alert_dialog()`) are **never** used; only native file/folder pickers (`OpenDialog` / `SaveDialog`) use OS dialogs.

| Tier | Guard Mechanism | When Used | Exact Actions in MarkTurbo |
|---|---|---|---|
| **1. No Guard (Immediate + Direct Reverse)** | None. Takes effect on press; pressing the toggle again, `Esc`, or `Ctrl+Z` reverses it. | Any action that does not destroy unrecoverable user data or send data off-device. | • Closing a Review result or Context panel (`review_view` is retained in session)<br>• Canceling a Review/Revision consent card (restores previous `review_view`)<br>• Accepting or rejecting a Revision hunk (`A`/`R` writes immediately; `Ctrl+Z` or `Undo` reverts it)<br>• Toggling Settings (auto-saved; clicking another choice reverses it)<br>• Removing a missing Recent path |
| **2. Immediate + Undo Status Notice (5 s)** | Action takes effect immediately; bottom status line shows `"<Outcome> · Undo"` for `5000 ms` (`UNDO_HOLD_MS`) before committing irreversible deletion. | Deleting secondary local state (crash checkpoints, saved credentials, usage counters) where a modal is heavy-handed but a stray click shouldn't be fatal. | • Recovery banner `Discard` and `Discard all`<br>• Settings `Remove` credential<br>• Settings `Clear counts` (local usage metrics) |
| **3. In-App Guard Card (`notices::guard`)** | Centred in-app modal card (`640 px` max width, scrim fade, `SCALE_FLOOR` 0.96→1). **Never glowing.** `Cancel` is always focused and ringed first. | Destroying unsaved buffer edits or overwriting external disk changes. | • Closing a dirty tab (`Close without saving?` → `Save` / `Don't save` / `Cancel`)<br>• Quitting with dirty tabs (`Quit without saving?` → `Save all` / `Don't save` / `Cancel`)<br>• Conflict `Use disk version` (`Reload from disk?` → `Reload` / `Cancel`)<br>• Conflict `Keep mine` (`Overwrite disk changes?` → `Overwrite` / `Cancel`) |
| **4. Hold-to-Send Consent Card (`consent::card`)** | Centred in-app card with full outbound disclosure (endpoint, wire format, encryption, proxy, byte-counted manifest). Requires **1.2 s continuous press or `Ctrl+Enter` hold** (`FILAMENT_HOLD_MS`) as filament charges dull red → hot white. | **Every** operation that sends document or context content to a model endpoint (local or remote). Single-use per scope/endpoint snapshot. | • `Review` (Document, Selection, Document + Context, Agent Skill package)<br>• `Prepare revision` (sends Review context + user answers)<br>• `Translate` (sends document segments) |

### Universal `Escape` and Focus-Return Rules
1. **Topmost-layer `Escape` priority:** Pressing `Escape` dismisses at most **one** layer in strict z-order:
   - Open Titlebar Menu → closes menu.
   - Open In-App Guard Card or Consent Card → cancels card (`Nothing was sent.` / stays open).
   - Open File/Search Palette (`Ctrl+P` / `Ctrl+Shift+F`) → closes palette.
   - Open Settings Page → closes Settings.
   - Active Review / Revision in-flight request → cancels the network request (`Cancelled — nothing changed.`).
   - Open Disk Comparison panel (`Compare`) → closes comparison panel.
   - Open Field (`Revision` outcome, `Review` result, or `Context`) when editor is not actively handling an internal Escape → closes the field.
2. **Focus restoration invariant:** Whenever **any** overlay, card, menu, palette, Settings page, or side field closes and a document tab is active (`state.active.get().is_some()`), the closing handler **must** call `state.focus_editor()` after clearing its open flag so the user's next keystroke lands in the document.
3. **Non-stealing arrival invariant:** Background completions (Review result arriving, Context resolving, Recovery checkpointing) must **never** call `focus_soon()` or `blur_source_editor()` if the source editor currently has focus.

---

## 3. Copy Rewrites

Product terms kept invariant across all strings:
- **Review** → `审查` | **Revision** → `修订` | **Agent Context** → `智能体上下文` | **Effective Agent Context** → `有效智能体上下文` | **Wire format** → `协议格式` | **Endpoint** → `端点` | **Credential** → `凭据` | **Lens** → `视角`

| Key or `S::Variant` | Current EN | Proposed EN | Proposed ZH (Natural Simplified Chinese) |
|---|---|---|---|
| **Welcome, Titlebar & Rail** | | | |
| `welcome::S::OpenFolder` | Open folder | Open folder… | 打开文件夹… |
| `welcome::S::Paste` | Paste | Paste as new document | 粘贴为新文档 |
| `welcome::S::FindAFile` | Ctrl+P finds any file in this folder. | {mod+P} finds any file in this folder. | 按 {mod+P} 查找此文件夹中的任意文件。 |
| `welcome::S::Missing` (action) | Missing | Missing · Remove | 已不存在 · 移除 |
| `titlebar::S::WebPreview` | Web preview | Compatibility preview | 兼容预览 |
| `titlebar::S::SplitWeb` | Split · Web | Split · Compatibility | 分栏 · 兼容预览 |
| `titlebar.rs` filename a11y | `{filename}` | Show agent context for {name} | 显示 {name} 的智能体上下文 |
| `titlebar.rs` Review blocked a11y | *(none)* | Finish or close the current revision before running Review. | 请先完成或关闭当前修订，再运行审查。 |
| `rail::S::FindFile` / `Key::SearchFiles` | Find a file / Search files | Find a file | 查找文件 |
| `rail.rs` palette no-folder empty | *(empty)* / No matches. | Open a folder first to search its files. | 请先打开文件夹以搜索其中的文件。 |
| `Key::Untitled` (per tab) | Untitled | Untitled {id} | 未命名 {id} |
| `state.rs` opening folder status | *(none)* | Opening folder… | 正在打开文件夹… |
| `welcome.rs:369` picker error | Folder picker failed: {why} | Could not open the folder dialog: {why}. Try again. | 无法打开文件夹对话框：{why}。请重试。 |
| `state.rs:2082` Save As error | Save As picker failed: {error} | Could not open the Save As dialog: {error}. Try again. | 无法打开“另存为”对话框：{error}。请重试。 |
| `state.rs:1378` sample error | Sample unavailable: {error} | Could not open the bundled sample: {error}. | 无法打开内置示例：{error}。 |
| `state.rs:2116` untitled save | Save this untitled document with Save As first. | This draft has no file yet. Choose Save As… to save it. | 此草稿尚未关联文件，请使用“另存为…”保存。 |
| **Notices, Guard & Conflict** | | | |
| `notices::S::CloseWithoutSaving` (tab) | Close without saving? | Close without saving? | 不保存就关闭？ |
| `notices::S::QuitWithoutSaving` (app) | Close without saving? | Quit without saving? | 不保存就退出？ |
| `notices::S::Compare` (closed / open) | Compare | Compare / Hide comparison | 比较 / 收起比较 |
| `notices::S::UseDiskVersion` | Use disk version | Use disk version | 采用磁盘版本 |
| `notices::S::KeepMine` | Keep mine | Keep mine | 保留我的版本 |
| `Key::SaveConflictDetail` | Save refused: the file changed on disk. Reload, overwrite, or save as a new file. | Save refused: the file changed on disk. Compare versions, keep yours, use the disk version, or choose Save As…. | 保存被拒绝：磁盘上的文件已更改。请比较版本、保留你的版本、采用磁盘版本，或选择“另存为…”。 |
| `state.rs:1584` recovered conflict | Recovered text differs from disk; reload, overwrite, or save as a new file. | Recovered draft differs from the file on disk. Compare versions, keep yours, use the disk version, or choose Save As…. | 恢复的草稿与磁盘文件不同。请比较版本、保留你的版本、采用磁盘版本，或选择“另存为…”。 |
| `state.rs:1919` reload guard | Reload from disk? / This discards the current buffer and loads the file again. / Reload | Discard your edits and reload from disk? / Your unsaved changes in this tab will be replaced by the file on disk. / Reload | 放弃当前修改并从磁盘重新加载？ / 此标签页中未保存的更改将被磁盘上的文件替换。 / 重新加载 |
| `state.rs:1978` overwrite guard | Overwrite disk changes? / The file changed after it was opened. Overwriting discards those changes. / Overwrite | Overwrite external changes on disk? / The file on disk changed since you opened it. Saving will replace those disk changes with your version. / Overwrite | 覆盖磁盘上的外部更改？ / 文件在打开后已被外部修改。保存将用你的版本覆盖磁盘上的更改。 / 覆盖 |
| `state.rs:1806` guard save failed | Still open: the document could not be saved. | Could not save this document, so it stays open. Check file permissions or try Save As…. | 文档未能保存，已保持打开。请检查文件权限或尝试“另存为…”。 |
| `state.rs:1795` guard Save As cancel | *(silent)* | Save cancelled — the document stays open. | 已取消保存——文档保持打开。 |
| `Key::QuitSaveFailed` | Quit cancelled: a document could not be saved. | Quit cancelled: a document could not be saved. Check the open tabs and try again. | 已取消退出：有文档未能保存。请检查打开的标签页后重试。 |
| `state.rs:1508` checkpoint error | Recovery checkpoint failed: {error} | Automatic recovery backup failed ({error}). Your edits are still in this window; save the file to disk soon. | 自动恢复备份失败（{error}）。你的编辑仍在此窗口中，请尽快保存文件。 |
| `notices.rs` Discard undo toast | *(none)* | Discarded recovered draft · Undo | 已丢弃恢复的草稿 · 撤销 |
| **Consent, Review, Revision & Context** | | | |
| `consent.rs` sent header *(Screenshot 4/5/6)* | Sent · 110 B content bytes | Sent · 110 B | 已发送 · 110 B |
| `consent::S::ScopeContext` | Document + context / 文档 + context | Document + context | 文档 + 上下文 |
| `consent::S::HeadlineTranslation` | *(native modal title: Translate)* | Send for translation? | 发送以进行翻译？ |
| `review::S::Empty` | No review result yet. | No review result yet. Run Review to inspect this document. | 暂无审查结果。运行“审查”以检查此文档。 |
| `review::S::TryAgain` (failures) | Try again | Try again | 重试 |
| `review::S::ReviewAgain` / `revision::S::ReviewAgain` | Review again (ZH: 重新审查 / 重新审阅) | Review again | 重新审查 |
| `review::S::NoModelHint` | Add a provider and an API key in Settings, then run Review again. | Choose a wire format and enter an API credential in Settings, then run Review again. | 请在“设置”中选择协议格式并配置 API 凭据，然后重新运行审查。 |
| `review::S::CouldNotRead` | Couldn't read the response — Try again | The model returned an invalid response. Nothing was changed — try again. | 模型返回了无效响应，文档未做任何更改——请重试。 |
| `review::step_hint(0)` | No findings / 0 个发现 | No findings | 没有发现 |
| `revision::S::Hunk(n)` a11y | Revision hunk {n} | Revision hunk {n} | 修订片段 {n} |
| `Key::RevisionDismiss` | Dismiss / 关闭 | Dismiss | 关闭提示 |
| `Key::RevisionAppliedSavePending` | Approved Revision applied; save is still pending | Approved changes applied to the document — press {mod+S} to save to disk. | 已批准的修改已写入文档——按 {mod+S} 保存到磁盘。 |
| `Key::RevisionNoApprovedChanges` | No approved changes to apply | No changes were approved — the document is unchanged. | 未批准任何修改——文档保持不变。 |
| `Key::RevisionCopied` | Copied approved Revision preview | Approved result copied to the clipboard. | 已将批准后的文本复制到剪贴板。 |
| `revision::S::CannotApply` / `state.rs:3270` | Can't apply — this part changed | Can't apply — this section was edited since the review. | 无法应用——此段落自审查后已被修改。 |
| `state.rs:3273` undo refusal | Can't undo — this part was edited after it was accepted. | Can't undo — this section was edited after the change was accepted. | 无法撤销——此段落在接受修订后又被修改过。 |
| `state.rs:3260` other tab refusal | This Revision belongs to another document. Return to it to decide. | This revision belongs to another tab. Switch back to that tab to accept or reject changes. | 此修订属于另一个标签页。请切回该标签页以接受或拒绝修改。 |
| `state.rs:75` needs review | Run Review first to prepare a Revision. | Run Review first, then prepare a revision. | 请先运行审查，再准备修订。 |
| `state.rs:3077,3185` rev failed | Revision failed: {error} | Revision request failed ({error}). Your answers are kept — try again. | 修订请求失败（{error}）。你的回答已保留——请重试。 |
| `state.rs:2206,2280` trans failed | Translation failed: {error} | Translation failed ({error}). The document was not changed — try again. | 翻译失败（{error}）。文档未做任何更改——请重试。 |
| `state.rs:2273` trans success | Translated {segments} segment(s) into {target}. | Translated {segments} segments into {target}. Press {mod+S} to save. | 已将 {segments} 个段落翻译为 {target}。按 {mod+S} 保存。 |
| `Key::AgentContext` / `Key::PanelHarness` | Agent Context / 代理上下文 | Agent Context | 智能体上下文 |
| `Key::EffectiveAgentContext` | Effective Agent Context / 有效代理上下文 | Effective Agent Context | 有效智能体上下文 |
| `context::S::StatusInventory` | Inventory only — no verified profile applies | Listed for information — Codex rules are not verified for this file. | 仅供查看——尚未验证适用于此文件的 Codex 规则。 |
| `context::S::StatusPartial` | Partially resolved — project instructions are unavailable | Partially resolved — some project instruction files could not be read. | 部分解析——部分项目指令文件无法读取。 |
| `context::S::StatusInvalid` | Not resolved for this file | Could not resolve instructions for this file. Open a workspace folder and try Refresh. | 无法解析此文件的指令。请打开工作区文件夹并点击“刷新”。 |
| **Settings & Usage** | | | |
| `Key::Provider` | Provider / 服务商 | Wire format | 协议格式 |
| `settings::S::ProviderDefault` | Provider default / 服务商默认值 | Wire format default | 协议格式默认值 |
| `Key::ChooseProviderForCredential` | Choose a provider before configuring credentials. | Choose a wire format above (not Best available) to configure its credential. | 请先在上方选择具体的协议格式（非“自动选择”），再配置其凭据。 |
| `Key::ProviderHelp` | The wire format to speak, not the vendor... | Selects the API protocol. Choose a specific format to store its credential; Best available automatically picks a configured format when Review starts. | 选择 API 协议格式。指定具体格式后即可配置其凭据；“自动选择”会在审查开始时自动选用已配置凭据的格式。 |
| `settings::S::BaseUrlHint` | Leave empty for the provider default. Remote hosts need HTTPS; include the API path, e.g. /v1. | Leave empty for the default endpoint. Remote hosts require HTTPS; plain HTTP is allowed only for numeric loopback addresses (e.g. 127.0.0.1:11434/v1). | 留空则使用默认端点。远程主机必须使用 HTTPS；仅数字回环地址（如 127.0.0.1:11434/v1）可使用 HTTP。 |
| `Key::EndpointInsecureRemoteTransport` | Remote endpoints must use HTTPS. | HTTPS is required for remote hosts. For a local server on this machine, use a numeric loopback address such as http://127.0.0.1:11434/v1. | 远程主机必须使用 HTTPS。如果连接本机服务，请使用数字回环地址（例如 http://127.0.0.1:11434/v1）。 |
| `Key::ModelHelp` | Leave empty to use MARKTURBO_MODEL, then MARKTURBO_TRANSLATE_MODEL, then the provider default. | Leave empty to use MARKTURBO_MODEL or the wire format's default model. | 留空则使用环境变量 MARKTURBO_MODEL 或该协议格式的默认模型。 |
| `settings::S::NoKeySaved` | No key saved / 未保存密钥 | No credential stored | 未存储凭据 |
| `settings::S::ReplacementKey` | Enter a replacement key / 输入替换密钥 | Enter API key or token | 输入 API 密钥或令牌 |
| `settings::S::KeySavedStored` | Key saved in Windows Credential Manager | Credential stored securely for this endpoint | 此端点的凭据已安全存储 |
| `Key::SecureStoreUnavailable` | Secure storage is unavailable. Use a session or environment credential. | OS keychain storage is unavailable on this system. Use "Use for session" or set an environment variable. | 当前系统不支持安全凭据存储。请点击“仅本次会话使用”或配置环境变量。 |
| `Key::CredentialWriteFailed` | The secure credential could not be written. | Could not save to secure storage. Use "Use for session" instead, or check your OS keychain permissions. | 无法写入系统安全存储。请改用“仅本次会话使用”，或检查系统凭据管理器权限。 |
| `settings::S::FontFailedNetwork` | Download failed: the network request did not complete. Nothing was installed. | Download failed: could not reach the font server. Nothing was installed — check your connection and press Retry. | 下载失败：无法连接字体服务器。未安装任何内容——请检查网络连接后点击“重试”。 |
| `Key::UsageDisclosure` | *(90-word dense paragraph)* | Optional local-only daily counts (opens, saves, Reviews, answers, revision decisions, and errors). Never records document text, filenames, paths, credentials, or timestamps. Stays on this device unless you export it. | 可选的纯本地按日计数（打开、保存、审查、回答、修订决定及错误次数）。绝不记录文档正文、文件名、路径、凭据或时间戳。数据仅保存在本机，除非你主动导出。 |
| `Key::UsageRetention` | Retain counts for 180 days; prune older days on load/save. Day keys are UTC days since 1970-01-01... | Counts are kept for 180 days and pruned automatically. Turning this Off stops recording; Clear counts deletes local history without affecting exported files. | 计数保留 180 天，过期自动清理。关闭即停止记录；“清除计数”仅删除本机记录，不影响已导出的文件。 |
| `i18n::usage_summary` | N retained UTC days · N Reviews completed · N revision decisions · N events | Last 180 days: {days} days active · {reviews} Reviews · {decisions} revision decisions · {events} total events | 近 180 天：活跃 {days} 天 · 完成审查 {reviews} 次 · 修订决定 {decisions} 次 · 共 {events} 条记录 |
| `Key::UsageUtcDay` / `usage_details` | UTC day since 1970-01-01: 20410 | {YYYY-MM-DD} | {YYYY-MM-DD} |
| `Key::UsageInspect` (closed / open) | Inspect counts / 查看计数 | Show details / Hide details | 显示明细 / 隐藏明细 |

---

## 5. What Not To Do (Rejected Audit Proposals)

1. **Reject adding a confirmation dialog or `"Discard review"` label to the Review `Close` button (`ux-flows` #10).**
   - *Why:* Closing a side panel is a routine navigation action. Guarding panel closes with confirmation prompts causes confirmation fatigue and violates Principle A (*"never interrupt what the user is doing"*). Keeping `review_view` in session state (UX Decision #1) makes closing non-destructive without adding friction.

2. **Reject adding confirmation modals for Recovery `Discard`, Settings `Remove` credential, and `Clear counts` (`ux-shell` #16 modal option, `ux-settings` F1, F7 modal options).**
   - *Why:* Popping up a modal card to confirm deleting a crash checkpoint or clearing local usage counters adds a two-step interruption to secondary maintenance actions. An immediate action paired with a 5-second inline `Undo` status notice (Section 2, Tier 2) protects against stray clicks with zero modal interruption.

3. **Reject adding an explanation legend or hover tooltips to editor gutter ticks (`ux-settings` F15).**
   - *Why:* Look at Screenshot 5 (`en-us-review-result-questions-default-light`): the Review field already renders a clean, self-explaining inline legend (`words Stated in the text | Inferred`) directly above the findings count. Adding a second legend at the top of the panel or hover popups on 2 px gutter ticks duplicates existing UI and violates the Afterglow principle that light in the margin is felt, not turned into a dashboard.

4. **Reject rendering all 16 unused `*Help` strings on the Settings page (`ux-settings` F4).**
   - *Why:* Look at Screenshot 9 (`en-us-settings-default-dark`): the Settings page is calm and scannable because it avoids walls of explanatory prose. Rendering obvious explanations like `LightThemeHelp` (*"Used whenever the effective mode is light"*) or strings for unmounted legacy controls (`IncludeGlobalSkillsHelp`, `ShowInternalSkillsHelp`, `GroupByHelp`, `SyncScrollingHelp`, `AutoRefreshHelp`) would turn Settings into a manual. Render **only** `ProviderHelp` and `ModelHelp` (which explain non-obvious fallback behavior), and **delete** the remaining 14 dead `*Help` keys from `i18n.rs`.

5. **Reject widening the Rail on keyboard focus or making background-tab `×` buttons permanently focusable in the tab order (`ux-shell` #17).**
   - *Why:* If every background tab in the 44 px rail has a second focusable `×` stop, pressing `Tab` to move from the titlebar to the editor requires tabbing through `2 × N` controls. Instead, keep one focus stop per rail tab dot and bind `Delete` / `Middle-Click` / `Ctrl+W` on a focused rail dot to close that tab.

6. **Reject keeping a background WebView instance mounted per open tab (`motion-a` F2 / Runner-up #9).**
   - *Why:* On Windows, each mounted WebView spawns a multi-process WebView2 browser tree consuming 80–150 MB of RAM. Retaining one WebView2 instance per open Markdown tab would blow up memory footprint for a compatibility-only preview mode (`PRODUCT.md` role: *Compatibility*).

7. **Reject rewriting `glow.rs` from a stepped clock to per-span `AnimatedValue` bindings now (`motion-b` F9).**
   - *Why:* `nsh-base` editor decorations take resolved `Color` values, not `AnimatedNode` bindings. Replacing that architecture is high-cost (`L`) and speculative until a production-profile timing probe on a large document demonstrates actual frame drops.

8. **Reject animating the Review footer button swap (`Prepare revision` quiet → primary) or the 1-second `"Usually 10–30 seconds"` hint (`motion-b` M25, F10).**
   - *Why:* Both occur at rare task boundaries where the user's eye is on the question input or the endpoint host line. Adding bespoke color-interpolation scaffolding to a button variant swap adds code complexity with negligible perceptual benefit.
