# Goal 23: Vertical slices, less code, tests that help

## Objective

The owner's direction (2026-11-27): clean up the architecture before more feature lanes. Three requirements:

- Move the codebase toward a Vertical Slice Architecture. Each user-facing capability owns its state, UI, copy and
  tests.
- Use the right design patterns to cut production lines and duplication, and lower the coupling between slices.
- Delete fragile tests. Tests exist to speed up iteration. A test that fails without a real defect, or that cannot
  fail, slows us down and adds no quality.

Evidence:

- [`docs/design/goal-23-architecture-findings.md`](../design/goal-23-architecture-findings.md): an Opus 5.5
  `improve-codebase-architecture` review of main `5939c2d`, with 48 candidates (A1–A48), the lane split and the owner
  decisions.
- [`docs/design/goal-23-fragile-tests.md`](../design/goal-23-fragile-tests.md): CI history over 37 failed runs, the
  TODO entries, and brittle-by-construction scans.

Ordering: this goal starts after Goal 22 lane 22-B merges, and Goal 21 starts after this goal.

## Owner decisions (2026-11-27)

1. **Fix everything harmful the review found.** Do not stop at a report. The owner's categories:
   - oversized files;
   - if/else piles;
   - missing patterns;
   - shorter paths and better abstractions;
   - regex or substring tests where a parser belongs;
   - test consolidation.
2. **Vertical slices.**
   - The target slices are: shell and view, documents and files, recovery, review, revision, translation, context,
     settings and credentials, and the native harness.
   - A slice owns its `AppState` methods (in `state/<slice>.rs`), its UI module, its copy catalog and its tests.
   - Slices talk through narrow interfaces: request tickets, the layer stack, and per-tab records. They do not read
     each other's fields.
3. **Fragile tests are deleted by default.** A test listed in the fragile-tests evidence, or found later, is deleted.
   It is replaced only when the behaviour it guards is a product contract (for example the Goal 20 F3 recovery hit
   test, or a privacy assertion). The replacement asserts behaviour and is deterministic: no wall-clock bounds, no
   fixed sleeps, no exact pixel literals, and no source-text scans when a parser or a Headless query can answer.
4. **Dependencies:** html5ever, markup5ever_rcdom, libc, roxmltree, imara-diff, syn and schemars are allowed. Each PR
   names the provenance change it makes.
5. **Translation's model stays.** One global dispatcher for the API request (at most one in flight). Results are bound
   to their origin tab and are independent per tab. A3's state machine keeps exactly this split.
6. **CI does not run the native harness unit tier.**

## Done when

1. **Behaviour unchanged.**
   - Every test that is not deleted as fragile passes before and after each PR, including the Headless, mt-core and
     tooling suites.
   - Native acceptance results do not regress: the goal-06 PASS stays a PASS.
   - The AGENTS.md invariants hold. These include the consent card's own key handling, `web_dirty` deferral,
     `panic = "unwind"`, the RaTeX clamp, content-free evidence, the tokens-only UI, and the overlay
     `PointerEvents::BoxNone`.
2. **Live bugs fixed first, each with a test that fails before its fix:**
   - A1: the shared timer global slot;
   - A8: the search ticket;
   - A9: the atomic `settings.toml` write;
   - A28: the two vacuous privacy assertions;
   - A5: the `close_tab_now` leak.
3. **Slices exist.**
   - `state.rs` holds only the struct, construction and shared helpers. No file under `crates/mt-app/src/state/` or
     `crates/mt-app/src/ui/` exceeds 1500 production lines (inline tests excluded).
   - `crates/mt-core/src/recovery/` follows A12. Its lock order stays private to the store module.
   - Request lifecycles use one slot/ticket type (A3).
   - Consent uses one pending value (A4).
   - Per-tab state uses one record (A5).
   - Revision decisions live in state (A6).
   - Escape and focus use one layer stack (A7).
4. **Less code, less coupling.** These are measured on the merged result against `5939c2d` and recorded in the final
   PR, with the command that produced each number:
   - production Rust lines (inline tests excluded) fall by at least 10%;
   - `crate::ui::` references from `state` fall to zero, except `ui::motion` tokens;
   - every duplicate listed in A9, A10, A11, A16, A17, A19, A21 and A23 has one owner.
5. **Tests help.**
   - Every test in the fragile-tests top-15 is deleted or replaced as decision 3 says.
   - No mt-app test uses a fixed `app.wait(duration)` as a synchronization point.
   - Integration tests start through one fixture (A32), are compiled once (A33), and use the shared fakes (A34, A35).
   - Python tests parse TOML, JSON and HTML instead of matching substrings (A29, A30).
6. `mt.py check ci` passes, and the GPT review converges to P3-only per PR.

## Failure control

Any of these fails the goal:

- A behaviour change that is not listed in this contract.
- A weakened assertion that is presented as a consolidation.
- A deleted test whose behaviour is a product contract and that has no replacement.
- A lock-order or consent-invariant change.
- Any `docs/evidence/` edit.
- A slice that reaches into another slice's fields.

## Out of scope

- Motion (Goal 21).
- New features.
- UI look.
- The WebView2 popup diagnosis (goal-07; tracked in `docs/goal-15-acceptance.md`).
- The accepted Goal 22 lane 22-B residuals in `docs/TODO.md`.

## Delivery lanes (disjoint ownership; one PR per wave per lane)

The findings document defines the lanes and their order; this goal adopts them with one change. A2 is reshaped into
the slice layout of decision 2.

| Lane | Owns | First wave |
| --- | --- | --- |
| S app state | `state.rs`, `state/**`, `app.rs` | A8, then A2 alone while `state.rs` is frozen |
| U app UI | `ui/**` (minus S's A6 parts), `web.rs`, `i18n.rs` | A1, A23, A24, A27 |
| C core storage and I/O | settings, credentials, usage, recovery, `document/**`, `workspace/**` | A9, A10, A11, A13, A15, A16 |
| M core model | `review*`, `translate/**`, `model*`, `package.rs`, `bin/**`, mt-core test-support | A28 (provider), A18, A34 and A35 (core), A20, A21 |
| T app integration tests | `mt-app/tests/**`, `test_vault.rs`, `test_env.rs` | delete the fragile tests, A28 (usage), A32 |
| P Python tooling | `scripts/**` | delete the fragile tests, A39, A31, A29, A40, A41 |

Waves 2 and 3 follow the findings document.

The conductor owns `docs/architecture.md` (the slice map) and `CONCEPTS.md`.

## Evidence plan

- Each PR:
  - its own before and after test counts, with the list of deleted fragile tests and the reason for each;
  - `check ci`;
  - a GPT review.
- The final PR:
  - the measurements from Done when 4;
  - one `mt.py accept all` run on the merged release build, compared with the last baseline;
  - the slice map in `docs/architecture.md`.

## Mergeable alone

Each wave of each lane is mergeable alone on `main`.
