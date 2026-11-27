# MarkTurbo architecture findings (phases 1-2, read-only)

Snapshot: main `5939c2d`.

- **Method:** six parallel read-only explorers covered app state, core storage and I/O, core model requests, app UI,
  Rust tests and Python tooling; the conductor integrated their reports.
- **Re-verified by the conductor:** A1, A5, A8, A9 and A28. All other line ranges come from the explorers' reads;
  re-read them before editing.
- **Not reviewed:** the in-flight `state/disk.rs`, `state/drop.rs` and watcher branches.

## Size table (physical lines; production vs inline `#[cfg(test)] mod`)

| File | Total | Production | Inline test |
| --- | ---: | ---: | ---: |
| crates/mt-core/src/recovery.rs | 9239 | 4182 | 5057 |
| crates/mt-app/src/state.rs | 7764 | 6085 | 1679 |
| crates/mt-core/src/review/provider.rs | 5543 | 2407 | 3136 |
| crates/mt-app/src/ui/review.rs | 3618 | 1789 | 1829 |
| crates/mt-app/src/ui/consent.rs | 3549 | 1914 | 1635 |
| scripts/markturbo_tools/native/runtime.py | 3403 | 3403 | 0 |
| crates/mt-app/src/ui/shell.rs | 3250 | 995 | 2255 |
| crates/mt-core/src/document/io.rs | 3228 | 1676 | 1552 |
| crates/mt-core/src/review.rs | 3030 | 2258 | 772 |
| crates/mt-app/src/web.rs | 2963 | 1524 | 1439 |
| crates/mt-app/src/ui/revision.rs | 2813 | 1359 | 1454 |
| crates/mt-core/src/model.rs | 2569 | 1620 | 949 |
| crates/mt-app/src/ui/notices.rs | 2518 | 1859 | 659 |

Harm:

- **High:** a live bug, or it blocks parallel lanes.
- **Medium:** real drift or churn.
- **Low:** hygiene.

Size:

- **S:** less than 1 day.
- **M:** 1–3 days.
- **L:** more than 3 days.

## A1. Give each UI timer its own global slot

- **Category:** 3, 4. **Harm:** High. **Size:** S. **Order:** independent; land it first.
- **Why High:** it is a live bug. Four timers share one nsh global slot, so each `.set` cancels another timer's task.
  - The glow clock freezes.
  - The Review hint is suppressed.
  - The consent card can lose its focus step, which breaks an AGENTS.md invariant.
- **Files:**
  - `ui/consent.rs` 239-241, 396-400;
  - `ui/glow.rs` 113-119, 515-553;
  - `ui/review.rs` 137-139, 164-167, 222-224, 305-325;
  - also audit `ui/shell.rs:134` (`global::<f32>`) and `glow.rs:114` (`global::<bool>`).
- **Problem:** `card_focus_step`, `ticker_step`, `focus_step` and `hint_step` all return the same
  `Signal<Option<Spawned<()>>>`.
  - `loading_hint` sets the slot to `None` when `review_busy` turns false. That cancels the glow tick while
    `ticker_running` stays true, so the ticker never restarts.
  - Each glow tick (every 33 or 250 ms) replaces the 1 s hint timer and can cancel the 0 ms card focus step.
- **Fix:**
  - Give each slot a private newtype, or keep the handle in `Stored`/`Rc` as `document.rs:315` (`Glide`) and
    `preview.rs` (`load_delay`) already do.
  - Audit every `global::<std type>()`.
- **Tests:**
  - Add Headless cases: glow still advances after a Review completes; the hint appears while a result glows; the card
    takes focus while the glow clock runs.
  - Protect the existing consent-focus tests.

## A2. Split AppState into lifecycle submodules

- **Category:** 1, 4. **Harm:** High. **Size:** M.
- **Why High:** it is the merge hotspot. #71, #72 (five review rounds), #74 and #82 all collided here.
- **Problem:** `state.rs` 946-5625 is one `impl AppState` block, with about 142 methods over about 100 fields. It is a
  shallow module that holds every workflow, so changes have no locality.
- **Clusters:**
  - notices: 1124-1540;
  - workspace: 1542-1816, 2207-2228, 2531-2610, 5802-5859;
  - tabs: 1817-2063;
  - context: 2064-2206;
  - recovery: 2295-2530, 6052-6070;
  - guard, save and conflict: 2611-3128, 3250-3338;
  - view and sidebar: 73-153, 496-600, 3129-3249;
  - translation: 3339-3509;
  - focus: 2229-2250, 3535-3730;
  - review: 3514-3534, 3729-4420, 5014-5179, plus the ReviewView projection at 5626-5795;
  - revision: 4422-5013;
  - settings, credentials and escape: 5180-5625;
  - copy (`mod s`): 5861-6050;
  - tests: 6084-7763.
- **Fix:**
  - Move each cluster mechanically into `state/{notices,workspace,tabs,context,recovery,guard,view,focus,translation,
    review,revision,settings,copy}.rs`, each with its own `impl AppState`.
  - The struct, `build`, `spawn_task`, `owned` and `set_active` stay in `state.rs`.
  - Keep the conflict, reload and overwrite code together in `guard.rs`.
  - Move the tests into `state/tests/*.rs`, one file per cluster.
- **Tests:** none change; this is behaviour-neutral, so `check ci` must pass.
- **Order:** after the disk, drop and watcher branches merge. Land it alone while `state.rs` is frozen. It precedes
  A3-A7, A14 (app half), A22, A25 (state part), A37 (state part) and A47.

## A3. One request-lifecycle slot and ticket instead of 10 counters and 7 busy flags

- **Category:** 3, 2. **Harm:** High. **Size:** M-L. **Order:** after A2, together with A4. A8 may land first.
- **Files:** `state.rs`:
  - fields: 308, 342, 378-382, 459-466, 488-489, 530;
  - bumps: 1253, 1297, 1322, 1342, 1353, 1357, 1543, 1589, 2025, 2066, 2123, 3499, 4621, 5096;
  - busy refresh: 2001-2042;
  - stale checks: 3450-3471, 4379;
  - test backdoors: 4706-4746.
- **Problem:** the request lifecycle is encoded ad hoc.
  - 10 u64 generation counters in three representations: `Signal<u64>`, `Rc<Cell<u64>>` and `TrackTween`. Some bump
    with `wrapping_add`, others with `+= 1`.
  - 7 busy flags, refreshed in 14 places.
  - Review, Revision and Translation follow different cancel, scope and stale rules.
- **Fix:** one request-slot state machine:
  - `Idle → AwaitingConsent(prepared, origin tab) → InFlight(ticket, owner tab, cancel token) → Idle`.
  - The ticket decides whether a late result is applied.
  - `busy` and `busy_elsewhere` are derived, not stored.
  - Folder, context and search reuse the ticket; status and undo use the ticket type.
- **Tests:**
  - Protect: `cancelling_a_new_review_keeps_the_revision_that_arrived`,
    `preparing_a_revision_discloses_the_tabs_own_review`, `closing_a_tab_releases_its_revision_session`, the
    wait-notice pair, `review_generation_checked`, `tests/review_stale.rs`, `review_supersede.rs` and
    `translation_cancel.rs`.
  - Replace the `arm_*_in_flight` and `finish_revision_in_flight` backdoors, and `release_the_review_slot`.
- **Invariant risk:** Translation's scope (one in flight app-wide) is product behaviour. Keep it.

## A4. One pending Request Consent value instead of nine fields and five priority chains

- **Category:** 2, 3. **Harm:** High. **Size:** M. **Order:** after A2, with A3, before A7.
- **Files:**
  - `state.rs`: fields 356-358, 383-385, 390-392, 419-420, 465-466; `consent_open` 3632-3636; `escape_top_layer`
    5226-5240; `focus_open_layer` 3586-3621; `editor_focus_blocked` 3625-3627;
  - `ui/consent.rs`: `kind_of` 174-184, `kind_of_untracked` 189-199, `waiting` 210-234, `cancel` 1095-1102.
- **Problem:** nine fields encode "at most one card waits". The Review > Revision > Translation priority is repeated in
  five if-chains across two files. The layer order was fixed in #72 and again in #79.
- **Fix:** one `Signal<Option<PendingConsent>>` that holds the kind, disclosure, manifest, prepared request and origin
  tab.
  - Cancel becomes behaviour on the kind.
  - `consent_open()` stays the single predicate.
  - This is A3's `AwaitingConsent` state.
- **Tests:** protect the `ui/consent.rs` inline tests,
  `focus_round_trips_to_the_document_view_and_refuses_behind_a_card`, `ctrl_enter_editor.rs`,
  `refine_keys_shortcuts.rs`, `translation_cancel.rs` and the #79 Escape tests.
- **Invariant risk:** Medium. The card takes focus when it opens; a held Ctrl+Enter counts only from the card's own
  KeyDown/KeyUp; blur cancels the hold; the no-rebuild rule holds.

## A5. One per-tab session record instead of mirror signals plus six HashMaps

- **Category:** 3, 4. **Harm:** High. **Size:** M. **Order:** after A2, with or before A6.
- **Why High:** it leaks today. `close_tab_now` (2962-2994) forgets `review_unavailable_tabs` (454) and
  `conflict_disks` (413).
- **Files:** `state.rs` maps 438-454, 471; `set_active` 1823-1858; store/load helpers 1861-1972, 2044-2062; 21
  hand-written tab lookups.
- **Problem:** each value exists twice, once as a shown signal and once in a store, kept in sync by about 12 helpers.
- **Fix:** one record per tab (review view, binding, open state, unavailable reason, revision session) in one map, with
  one show/store pair, one removal and one lookup helper.
- **Tests:** protect `a_review_unavailable_reason_stays_with_its_tab`, `RevisionPerTabProbe`,
  `closing_the_last_tab_clears_the_shown_revision` and `closing_a_tab_releases_its_revision_session`. Add: closing a
  tab leaves no entry.

## A6. Move the Revision decision stack and surface choice from ui to state

- **Category:** 1, 3, 4. **Harm:** High. **Size:** M. **Order:** after A2 and A5.
- **Problem:** there are two sources of truth for hunk decisions, and `state.rs` and `ui` depend on each other
  (13 `crate::ui::` calls in `state.rs`).
  - Accept and reject write both `RevisionLedger` and the UI `Session`.
  - Every tab switch swaps a UI global.
  - Surface priority is chosen in `shell.rs`, then re-derived in `state.rs` and `document.rs`.
- **Files:** `ui/revision.rs` 160-293, 1136-1290; `state.rs` 834-845, 1858-1895; `ui/review.rs` 169-209;
  `ui/shell.rs` 30-59, 217-220; `ui/document.rs` 396-398.
- **Fix:**
  - The ledger owns decide, undo, done and the counts.
  - `FieldSurface` and `surface()` move into state; `shows_full_preview` and `docked_surfaces` become methods there.
  - `ui::revision` keeps only presentation.
- **Tests:** protect `tests/revision_flow.rs` and the `ui/revision.rs` tests. The `hunk()` and `two_hunks()` tests
  become plain unit tests.
- **Invariant risk:** Low. Keep the `web_dirty` deferral and the `ui/mod.rs` priority semantics.

## A7. One layer stack for Escape and one table for room shortcuts

- **Category:** 2, 4. **Harm:** Medium. **Size:** M. **Order:** after A4.
- **Problem:** there are 10 Escape bindings. Review and Context close outside the single order (`review.rs:340-347`,
  `context.rs:84-86`). Nine shortcuts repeat the consent gate.
- **Files:** `state.rs` 5217-5274; `titlebar.rs` 160-224; `rail.rs` 150-240; `context.rs` 72-90; `review.rs`
  331-351; `revision.rs` 1104-1124; `settings.rs` 53-63; `notices.rs` 1241-1252, 1735-1745; `consent.rs` 1085.
- **Fix:**
  - A layer stack exposes the top layer's Escape and focus behaviour.
  - Every Escape binding goes through one helper.
  - Room shortcuts become one (key, action) table behind one gate.
- **Tests:** `context_escape.rs`, `a_waiting_consent_card_keeps_the_room_shortcuts_inert`, and `shell.rs:2154`.
- **Invariant risk:** Medium. Use no system keyboard hooks; the card keeps its own KeyDown/KeyUp handling.

## A8. Give search a ticket

- **Category:** 3, 4. **Harm:** Medium; it is a latent bug. **Size:** S. **Order:** independent.
- **Files:** `state.rs` 2553-2609 (`run_search`), 5283-5288 (`clear_search`); `ui/rail.rs` 1167-1172.
- **Problem:** `run_search` spawns its task with no generation. A slow earlier walk can overwrite newer results, refill
  a cleared palette, or clear the busy flag of a newer search. The "Search failed" message is English-only.
- **Fix:** check a ticket before applying results, and localize the message.
- **Tests:** a Headless probe that submits twice with a gated slow walk (`test_vault::wait_for_read` shows the
  pattern).

## A9. Make the settings write atomic through one atomic-file module

- **Category:** 4. **Harm:** High. **Size:** S. **Order:** wave 0, then A10.
- **Why High:** `settings.rs:548-556` truncates `settings.toml` in place.
  - A torn file loads as defaults, and every later save then fails with "malformed".
  - The file holds consent and credential authorization.
- **Files:** `settings.rs` 548-556. The same stage/sync/persist code is copied in `usage.rs` (363-376, 491-506),
  `recovery.rs` (2687-2767), `document/io.rs` (1635-1646) and `companion_font.rs` (323-346).
- **Fix:** one crate-private atomic-file module that stages, syncs and persists, with a no-clobber variant.
  - Settings, usage, the companion font and io staging use it.
  - Recovery wraps it for fault injection.
  - The `commit.rs` `ReplaceFileW` path stays separate.
- **Tests:** add "no staging file left behind" and "an existing file survives a failed persist".

## A10. One process-lock module and one test-scoping rule

- **Category:** 6, 4. **Harm:** Medium. **Size:** S-M. **Order:** after A9, before A32.
- **Files:** `settings.rs` 572-717; `credentials.rs` 189-230, 504-575; `usage.rs` 430-436; `test_env.rs` 18-34.
- **Problem:** there are two copies of the Win32 named-mutex guard and three test-isolation schemes. #50, #54 and #61
  all fixed lock contention.
- **Fix:** one lock module. The production mutex names stay exactly `Global\markturbo-settings-write-v1` and
  `Global\markturbo-model-credential-operation-v1`. Possibly derive the test scope from the config directory and drop
  `MARKTURBO_TEST_LOCK_NAMESPACE`.
- **Tests:** keep the fail-closed-when-busy tests.

## A11. One no-follow filesystem primitive module

- **Category:** 4. **Harm:** Medium. **Size:** M. **Order:** independent.
- **Problem:** #42 came from one of the copies. The fcntl constants are duplicated, and the reparse check is written 21
  times across 5 files.
- **Files:** `document/io.rs` 668-948; `agent_artifacts/package/source.rs` 645-1001; `document/io/commit.rs` 11-19,
  637-800; `recovery.rs` 936-947.
- **Fix:** move the primitives only into one private module; the origin and identity policy stays where it is.
- **Tests:** CI on all three OSes, plus the macOS nightly.

## A12. recovery: keep the locked store whole, move lock-free parts and tests out

- **Category:** 1, 2. **Harm:** Medium. **Size:** M. **Order:** after A13 and A9.
- **Fix:**
  - Turn `recovery` into a directory. The store module keeps `RecoveryStore`, `CapabilityState`, `MutationState` and
    every lock, so the lock order stays private.
  - Child modules: codec (3143-3240, 3573-4030), root guard (583-594, 732-979), waves (3366-3560) and schedule
    (4031-4180).
  - A batch-producer owner replaces the 8 repeated early exits in `checkpoint_batch_if_current_cancellable`
    (1180-1464).
  - Move the 156 tests unchanged into `recovery/tests/*.rs`, with shared `test_store*` helpers.

## A13. Move test-only surface out of production; one one-shot hook helper

- **Category:** 4, 6. **Harm:** Low. **Size:** S. **Order:** before A12.
- **Files:** `recovery.rs` 596-652 and 2807-2925 (20 `*_for_test` methods); one-shot hooks in `commit.rs`
  1333-1480, `io.rs` 1648-1675, `source.rs` 1008-1034 and `recovery.rs` 627-652.
- **Fix:** a `cfg(test)`-only faults module, plus one crate-private `cfg(test)` one-shot helper.

## A14. Move the recovery round-trip and checkpoint schedule into mt-core

- **Category:** 4. **Harm:** Medium. **Size:** S.
- **Files:** `state.rs` 2298-2319, 2323-2370, 2376-2399, 6053-6070; `recovery.rs` 189-200.
- **Problem:** the app copies `from_loaded_file` and hand-writes its inverse with an invented "recovered.md". It
  duplicates the dirty-tab collection and owns the 2 s / 10 s timing.
- **Fix:** core provides untitled metadata, record → `LoadedFile`, and "which buffers are due". The app keeps the
  tracks and the notices.
- **Tests:** add a metadata identity round-trip.

## A15. Delete the dead AsyncSnapshot, or make it the one guard

- **Category:** 3, 4. **Harm:** Low. **Size:** S.
- **Files:** `document/lifecycle.rs` 43-71 (only its own test uses it); `docs/architecture.md` ~568; `review.rs`
  325-345.
- **Fix:** delete it and fix the docs, or make it one source-version type that A3's ticket uses.

## A16. One workspace directory walker

- **Category:** 4. **Harm:** Low. **Size:** S.
- **Files:** `workspace/tree.rs` 49-128; `workspace/search.rs` 377-408.
- **Fix:** one private entry iterator. The symlink rule has already drifted once.

## A17. One model-call module for Review, Revision and Translation

- **Category:** 4, 3. **Harm:** Medium. **Size:** M-L. **Order:** after A18 and the core part of A34.
- **Problem:** the three copies have drifted:
  - Translation has no cancel, no per-call timeout and no response cap;
  - the Revision stream drops HTTP hints;
  - the app keeps two classifiers (`state.rs` 1500-1538);
  - 10 error variants are duplicated.
- **Files:** `review/provider.rs` 439-508, 598-618, 939-1164, 2170-2325; `provider/revision.rs` 1288-1396,
  1482-1494; `translate/provider.rs` 64-127, 233-252, 304-510; `model/transport.rs`.
- **Fix:** a deeper `model::transport` with a frozen prepared model, one content-free core error, and one exchange
  (prompt, payload, schema, stream, cap, cancel, timeout). Payload, decode and Request Consent stay per operation, as
  strategies.
- **Invariant risk:** Low-Medium. Diagnostics stay content-free; consent stays one-use per operation and per Model
  Endpoint Identity.

## A18. Fix the lossy Review error taxonomy

- **Category:** 2, 4. **Harm:** Medium. **Size:** S. **Order:** before A17.
- **Problem:**
  - 6 of the 12 `ReviewRequestError` variants are dead.
  - `ScopeFrameMismatch` is used at 20 sites.
  - 6 of the 9 `ReviewDecodeError` variants are dead.
  - Every decode failure becomes `InvalidJson`, so an unfound quote is reported as invalid JSON.
- **Fix:** one content-free mapping; delete the dead variants; rename the duplicates.

## A19. One owner for Agent Skill package rules

- **Category:** 4. **Harm:** Medium. **Size:** M.
- **Files:** `review.rs` 35-37, 355-735, 1997-2022; `model.rs` 193-196, 815-1123, 1422-1459; `provider.rs` 55-58,
  1675-1778; `provider/revision.rs` 1148-1186.
- **Problem:** `normalize_relative_path` is written twice, the 512 KiB/4 MiB limits three times and the framing twice.
  Revision bypasses `AgentSkillProviderAdapter`. About 10 public methods are dead or test-only.
- **Fix:** the package module in `review.rs` owns these rules, with one Serialize frame struct.
- **Invariant risk:** Medium. Consent digests must stay byte-identical.

## A20. Derive wire decoding; typed payloads

- **Category:** 4. **Harm:** Low. **Size:** S-M. **Order:** after A18.
- **Files:** `provider.rs` 1780-1869, 1925-2134; `provider/revision.rs` 712-777, 972-1073, 1197-1273.
- **Fix:** replace the 65-line manual Deserialize with a tagged derive, and use typed Serialize payloads.

## A21. One digest/hex helper

- **Category:** 4. **Harm:** Low. **Size:** S.
- **Problem:** about 10 copies with different parse strictness, in `review.rs`, `provider.rs`,
  `provider/revision.rs`, `model.rs`, `package.rs`, the goal06/goal07 binaries and `companion_font.rs`.

## A22. Outbound Scope manifests as pure functions

- **Category:** 4, 1. **Harm:** Medium. **Size:** S. **Order:** after A2, with A4.
- **Files:** `state.rs` 4090-4304, 3376-3386, 673-725.
- **Problem:** this is a privacy disclosure, and today it is testable only through Headless.
- **Fix:** a pure module (root + context + bound request → manifest) with unit tests. Translation reuses
  `scope_items`. The output must stay byte-identical.

## A23. Deepen the prims actions; delete the local copies

- **Category:** 4. **Harm:** Medium. **Size:** S-M. **Order:** lane U wave 1.
- **Files:** `prims.rs` 176-221. Copies in `settings.rs` 1035-1137, `context.rs` 638-668, `revision.rs` 978-1000 and
  `consent.rs` 929-971. Card frames in `notices.rs` 1355-1431 and `consent.rs` 984-1002.
- **Fix:** prims gains a reactive label, selected, disabled, a separate accessible name, a ref, blur and a card frame.
  Delete the copies.

## A24. Move the conflict-compare diff out of notices

- **Category:** 1, 4. **Harm:** Low. **Size:** S-M.
- **Files:** `notices.rs` 648-1027.
- **Fix:** an mt-core text-diff module, optionally on `imara-diff`.

## A25. Reconcile the copy catalogs with the docs; remove hardcoded English

- **Category:** 2, 4. **Harm:** Low. **Size:** S.
- **Fix:** record per-surface catalogs as the pattern, fold `mod s` into `flow_copy`, and localize the hardcoded
  English at `state.rs` 2599, 3802, 3810 and `web.rs` 99-104.

## A26. Replace web.rs's hand-written HTML tokenizer (Speculative)

- **Category:** 4, 1. **Harm:** Medium. **Size:** L. **Order:** after A27.
- **Why:** #53, #57, #58 and #63 fixed scope bugs in the tokenizer.
- **Fix:** evaluate html5ever's tokenizer.
- **Invariant risk:** High. The CSP, the sandbox and Web Preview Isolation must not weaken.

## A27. Parse preview HTML and SVG in tests

- **Category:** 5. **Harm:** Medium. **Size:** M. **Order:** before A26.
- **Problem:** the `web.rs` tests (1526-2962) make 94 `.contains` checks and use a hand-written CSS parser.
  - `!contains("<script")` misses `<SCRIPT`.
  - `"#000"` also matches `#000080`.
- **Fix:**
  - Parse with html5ever + markup5ever_rcdom as dev-dependencies: assert one iframe, `sandbox == allow-scripts`, the
    CSP meta first in head, and no host script.
  - Use roxmltree for the SVG checks.
  - Keep the exact-byte escape checks.

## A28. Fix the vacuous and fragile JSON/TOML substring assertions (Rust)

- **Category:** 5. **Harm:** Medium. **Size:** S.
- **Problem:** two privacy checks can never fail:
  - `provider.rs:5073-5083`, `!serialized.contains("\"answer\":null")`;
  - `usage_metrics.rs:112`, where the escaped Windows path never matches.
  Other substring checks: `provider.rs` 4219-4238, 5099-5101, 5343-5347, 3239-3247; `settings.rs` 858-862,
  1113-1120; `recovery.rs` 5388-5441.
- **Fix:** use serde_json pointers plus a recursive key walk, and `toml::Table` lookups. Keep the raw sentinel scans.

## A29. Python: one settings.toml writer; parse TOML and JSON

- **Category:** 5, 4. **Harm:** Medium. **Size:** S.
- **Files:** writers in `goal03.py` 191-203, `goal06.py` 139-150, `goal07.py` 829-837 and `visual.py` 1148-1162 (which
  writes `theme` unescaped); `ReviewServer` matching on bytes; the tests that check these.
- **Fix:** one escaping writer in `native/evidence.py`, `tomllib`/`json.loads` comparisons, and a recursive key walk.

## A30. Goal 07 fixture: parsed HTML and observed URLs

- **Category:** 5. **Harm:** Low. **Size:** M.
- **Files:** `test_native_goal07.py` 1172-1530: about 22 `assertIn` calls, plus a Node harness that is silently
  skipped when Node is missing.
- **Fix:** use `html.parser` facts and record the URLs actually requested.

## A31. One native selector module, guarded against the Rust UI

- **Category:** 4, 5. **Harm:** High. **Size:** M. **Order:** before A41, A42 and A44.
- **Why High:** #82 touched 11 tooling files, and the goal and visual ids have no guard. `consent-send`,
  `title-file`, `welcome-new-file` and `CONSENT_HOLD_SECONDS` are each defined twice.
- **Fix:** one `native/selectors` module, guarded by a scan of `crates/mt-app/src/ui` with the test modules stripped,
  or by a Headless mount test.

## A32. One app-start fixture for the integration tests

- **Category:** 6, 4. **Harm:** High. **Size:** M. **Order:** after A10; before A33, the app part of A34, and A36.
- **Why High:** the 57 test files repeat the same setup.
  - 78 `EngineConfig::new(Root::of::<mt_app::App>())` calls;
  - 77 unsafe env blocks;
  - 44 `"mod+p"` and 82 `"id:title-settings"` strings;
  - duplicated helpers and three isolation idioms.
  #61 edited 40 test files and #82 touched 19.
- **Fix:** `tests/support` becomes a fixture: an app builder that owns the dirs, env and lock, with options for OS,
  idiom, scheme, viewport, clock, motion and open path, plus journey helpers.

## A33. Wire the integration suite once

- **Category:** 6, 1. **Harm:** Medium. **Size:** M. **Order:** after A32.
- **Problem:** each test file compiles twice, and `support` compiles 24 times.
- **Fix:** one `tests/app/main.rs`; drop `test = false`; shrink the regex guard in `test_checks.py`; fix the docs.

## A34. One loopback model-endpoint fake in mt-core test-support

- **Category:** 6, 4. **Harm:** Medium. **Size:** M.
- **Problem:** there is a hang risk: only 4 of the 13 files set a read timeout. The helpers are duplicated: 12
  `read_http_body`, 9 `write_review_json`, 20 blocking accepts.
- **Fix:** a std-only module behind `test-support` that provides a bounded accept, a parsed request, canned
  JSON/SSE/status responses, and context and skill fixtures.

## A35. One scriptable fake credential store

- **Category:** 6. **Harm:** Low. **Size:** S.
- **Problem:** there are 13–17 separate fakes.
- **Fix:** one test-support store (empty, unsupported, failing, counting) that `test_vault` wraps.

## A36. Group and merge the integration tests by behaviour

- **Category:** 6. **Harm:** Medium. **Size:** M. **Order:** after A32 and A33.
- **Fix:** group the settings (×6), close-guard (×9), welcome, conflict (×3) and review tests; rename the `chrome_lane_*`,
  `refine_*` and `hierarchy_*` files. Delete duplicates: `ctrl_enter_editor.rs` 314-345, and `tests/theme.rs`.

## A37. Shared inline Headless probe helpers and runner

- **Category:** 6. **Harm:** Medium. **Size:** M.
- **Problem:** `press_without_settling` exists 4 times, `probe_path` 3 times and `start` 13 times. `state.rs` has 24
  start blocks and 15 `NoStore` vault copies.
- **Fix:** one `cfg(test)` `test_support` module with a closure runner.

## A38. mt-core tests follow the module split

- **Category:** 6, 1. **Harm:** Low. **Size:** S-M. **Order:** after A34.
- **Fix:** move the Revision tests into `provider/revision.rs`, and share one Skill-tree fixture.

## A39. Prove the Python test lists are complete

- **Category:** 6. **Harm:** Medium. **Size:** S. **Order:** wave 0.
- **Files:** `checks.py` 32-57, 237-241; `test_privacy.py:445`.
- **Fix:** a set-equality test, and delete the self-check.

## A40. Visual capture uses the fail-closed case lifecycle

- **Category:** 4, 3. **Harm:** High. **Size:** M.
- **Problem:** in `native/visual.py` 1368-1596, cleanup errors are swallowed before `remove_profile`, raw detail goes
  into the receipt, and the CJK branch bypasses the prepare table.
- **Fix:** reuse the runtime cleanup policy and `safe_failure_type`, and turn the CJK branch into a plan entry.

## A41. One consent-hold step for the native harnesses

- **Category:** 4. **Harm:** Medium. **Size:** S-M. **Order:** after A31.
- **Problem:** there are three copies, and only the visual one retries.
- **Fix:** one step that retries explicitly, and only while the card is open and nothing has been sent.

## A42. Split native/runtime.py along its seams

- **Category:** 1. **Harm:** Medium. **Size:** L. **Order:** after A31, A39 and A40.
- **Fix:** separate modules for Win32 and process control, the lifecycle, and the app page object.

## A43. One parametrized evidence-contract suite

- **Category:** 6. **Harm:** Medium. **Size:** M.
- **Fix:** a builder registry in `_native_support.py` that checks four properties for every goal and every key.

## A44. Share the UI Automation fakes at one seam

- **Category:** 6. **Harm:** Medium. **Size:** M-L. **Order:** after A31.
- **Fix:** fake at the UIA root, with one construction helper.

## A45. A run-outcome value in the native runtime

- **Category:** 3. **Harm:** Low. **Size:** M.
- **Files:** `runtime.py` 2946-3251.
- **Fix:** an outcome value with a worst-wins combine.

## A46. Shared helpers between the evaluation tools (Speculative)

- **Category:** 4. **Harm:** Low. **Size:** S.

## A47. Guard choices as an enum

- **Category:** 2. **Harm:** Low. **Size:** S. **Order:** after A2.

## A48. Token-aware motion source scan (Speculative)

- **Category:** 5. **Harm:** Low. **Size:** S.
- **Fix:** a `syn` visit, or strip comments before scanning.

## Checked and rejected

- Splitting the recovery store, retirement and transaction code: it would expose the lock order.
- One epoch type for recovery's internal counters.
- Splitting mt-core `review.rs`.
- A core consent state machine: `ConsentState` already is one.
- Building production JSON from strings: none found.
- Duplicated CSP: none found.
- Splitting `ui/review.rs` or `consent.rs` by size alone.
- Python Cargo.lock regex parsing: it already uses `tomllib`.
- Splitting the cold Python tools.
- `app.rs`, `revision_ledger.rs`, `review/revision.rs`, the credentials vault transaction and `DestructiveRequest` are
  cohesive.

## Counts (primary category)

| Cat | High | Medium | Low |
| --- | ---: | ---: | ---: |
| 1 | 1 | 2 | 0 |
| 2 | 1 | 1 | 2 |
| 3 | 3 | 1 | 2 |
| 4 | 3 | 9 | 6 |
| 5 | 1 | 3 | 2 |
| 6 | 1 | 8 | 2 |
| Total (48) | 10 | 24 | 14 |

## Lane split (disjoint file ownership)

| Lane | Owns | Order |
| --- | --- | --- |
| S app state | `mt-app/src/state.rs`, `state/**`, `app.rs`; the A6 parts of `ui/revision.rs` and `ui/shell.rs` | A8 → A2 → A3+A4+A22 → A5 → A6 → A7 → A14 app → A47 → A25 state |
| U app UI | `mt-app/src/ui/**` (minus lane S's A6 parts), `web.rs`, `i18n.rs` | A1 → A23 → A24 → A27 → A37 → A25 ui → A26 |
| C core storage and I/O | mt-core settings, credentials, usage, recovery, companion_font, runtime_paths; `document/**`; `workspace/**`; `agent_artifacts/package/source.rs` | A9 → A10 → A11 → A13 → A15 → A16 → A12 → A14 core; A28 settings and recovery |
| M core model | mt-core `review.rs`, `review/**`, `translate/**`, `model.rs`, `model/**`; `package.rs`; `bin/**`; the test-support module; `mt-core/tests/**` | A28 provider → A18 → A34+A35 core → A20 → A21 → A19 → A38 → A17 |
| T app integration tests | `mt-app/tests/**`; the `[[test]]` sections of `Cargo.toml`; `test_vault.rs`; `test_env.rs` | A28 usage → A32 → A34 app → A33 → A36 |
| P Python tooling | `scripts/**` | A39 → A31 → A29 → A40 → A41 → A43 → A42 → A44 → A45 → A30 → A46 |

Waves:

| Wave | Work |
| --- | --- |
| 0 | A1, A8, A9, A28, A39 |
| 1 | A2 alone in `state.rs`. In parallel: C (A10, A11, A13, A15, A16), M (A18, A34/A35 core, A20, A21), U (A23, A24, A27), T (A32), P (A31, A29, A40, A41) |
| 2 | S (A3, A4, A22), C (A12, A14 core), M (A19, A38, A17), T (A34 app, A33), P (A43, A42) |
| 3 | S (A5, A6, A7, A14 app, A47, A25 state), U (A37, A25 ui, A26), T (A36), P (A44, A45, A30, A46) |

## Owner decisions

- **(a)** Dependency provenance: html5ever/markup5ever_rcdom, libc, roxmltree, imara-diff, syn, schemars.
- **(b)** Whether Translation becomes per-tab.
- **(c)** Whether CI runs the native unit tier.

## Owner decisions (2026-11-27)

- **(a)** All listed dependencies are allowed: html5ever, markup5ever_rcdom, libc, roxmltree, imara-diff, syn and
  schemars. Record each provenance change in its PR.
- **(b)** Translation keeps its current model:
  - one global dispatcher for the API call, so at most one request is in flight app-wide, which keeps scheduling
    and concurrency simple;
  - results bound to the origin tab, independent per tab: `translate_active` binds `ReviewOrigin { tab_id, revision }`
    and writes the result only to that tab, after a revision check.

  A3's state machine must keep exactly this split: a global request slot, and per-tab result ownership.
- **(c)** CI does not run `mt.py check native`.
