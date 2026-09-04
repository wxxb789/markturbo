# Goal 09 — Refine product hierarchy and visual behavior

## Objective

Make the completed first-use, editing, Review, and Agent Context workflows read
as one calm product by applying an owner-approved information hierarchy,
user-facing terminology, progressive disclosure, and one signature light/dark
default for clean profiles; verify the welcome, edit, native/Web preview, Review
loading/result/stale/error, context, conflict, and settings states at
minimum/default/wide window widths in both effective color modes, with no new
product capability or regression in keyboard and accessibility behavior.

## Product contract alignment

**Disposition:** Retained and revised on 2026-08-29.

The hierarchy is defined by the [Capability roles](../../PRODUCT.md#capability-roles).
Visual acceptance must satisfy the [First-Time-User
Evidence](../../PRODUCT.md#first-time-user-evidence); this goal adds only the UI and
state evidence needed to show that hierarchy works in the product.

## Design standard

Taste is expressed through defaults, hierarchy, absence, reversibility, and
state quality—not through adding more palettes or controls. The interface should
make one next action obvious while preserving advanced compatibility paths for
users who need them.

## In scope

- Derive one explicit action hierarchy from the
  [Capability roles](../../PRODUCT.md#capability-roles).
- Replace implementation vocabulary with user-task vocabulary where owner review
  confirms it improves comprehension. Candidates to test include:
  - `Source` → `Edit`;
  - `Native` → `Preview`;
  - `Web` → `Compatibility Preview`;
  - `Harness` → `Agent Context` or `Skills & Instructions`.
- Keep every layout `PRODUCT.md` still requires, but progressively
  disclose uncommon variants instead of making every technical mode compete for
  first attention. Do not preserve a mode merely because it existed before the
  product decision, and do not remove one the contract retained.
- Give the right-side surface meaningful contextual states for Review, Context,
  and document details; avoid reserving a large empty panel merely to preserve a
  toggle.
- Make Review discoverable from the welcome state and active document without
  turning it into a chat sidebar.
- Keep Translation as a secondary document/selection action.
- Select and tune one signature light preset and one signature dark preset as
  defaults. Existing alternatives may remain preferences, but do not expand the
  theme catalog.
- Audit typography, spacing, truncation, focus, hover, loading, empty, error,
  disabled, stale, destructive, and success states using the existing metric and
  theme systems rather than local one-off constants.
- Preserve Windows WebView child-window constraints: no popup or tooltip may be
  required over the browser rectangle.
- Preserve complete keyboard access, visible focus, semantic roles, labels,
  contrast, and both English and Chinese interface coverage for changed copy.
- Add a reproducible screenshot/state-capture matrix suitable for review; do not
  present screenshots from obstructed or stale builds as evidence.

## Out of scope

- Changing Review semantics, model prompts, diff application, context precedence,
  filesystem safety, or startup architecture.
- Adding renderers, themes, languages, panels, a command marketplace, or a chat
  interface.
- Replacing GPUI/gpui-component, adopting a web shell, or creating a generic
  design framework.
- Installer, signing, release automation, public download pages, or auto-update;
  Goal 10 owns distribution.
- Pixel changes justified only by personal preference without state comparison
  or owner review.

## Visual acceptance matrix

Capture and review at least these eleven states:

1. clean-profile welcome;
2. new unsaved document;
3. ordinary Edit;
4. native Preview or split editing;
5. Web/compatibility Preview on a platform that supports it;
6. Review loading;
7. Review result with questions;
8. stale or malformed Review result;
9. Effective Agent Context;
10. external-change or destructive-close decision;
11. Settings.

Each state must be checked at the configured minimum, normal default, and a wide
window width, in effective light and dark modes. Platform-identical captures may
be automated; platform-specific WebView states require their real runtime.

## Completion evidence

- The project owner approves one state matrix produced from a release build and
  records any intentional exceptions.
- The [First-Time-User Evidence](../../PRODUCT.md#first-time-user-evidence) passes.
  In those sessions, participants also identify the primary action, open/paste
  path, active artifact, and current safety state without being taught internal
  terms.
- All controls remain reachable by keyboard with visible focus; automated
  accessibility assertions and a Windows 11 x64 screen-reader smoke test cover
  changed surfaces.
- Changed strings are complete in English and Chinese, or the product contract
  explicitly narrows language support before implementation.
- No state depends on a tooltip or floating GPUI surface over an active Windows
  WebView.
- Screenshot capture refuses obstructed windows and records build revision,
  viewport, theme, language, and state.
- `cargo fmt --all -- --check`, `cargo clippy --workspace --all-targets`, focused
  UI/i18n tests, and `cargo test --release --workspace` pass; the pass count is
  recorded.

## Stop and ask

Stop if owner review cannot choose a signature default, or if a proposed
simplification would remove a required capability rather than progressively
disclose it.

## Boundary for the next goal

This goal ends with an approved product experience in development builds. Goal
10 packages, signs, documents, and verifies that exact experience for users; it
must not reopen the visual design without a release-blocking defect.
