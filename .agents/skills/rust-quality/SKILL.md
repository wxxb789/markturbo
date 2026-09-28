---
name: rust-quality
description: "Ownership-first refactoring traces data and failure contracts before changing Rust boundaries. Use when moving Rust responsibilities across module or crate boundaries, or when a changed Rust module alters ownership or error handling. Skip standalone formatting, lint, or broad style cleanup."
---

# Rust quality

Use this procedure for a scoped Rust module move or refactor. Deliver a behavior-preserving cutover for the current callers. The work is done when the responsibility is at its intended boundary, all affected in-repository users are migrated, ownership and failure behavior still match the governing contracts, and a focused check exercises the changed path or its exact blocker is reported.

## Procedure

1. **Establish the boundary before editing.** Read the requested outcome, applicable product and architecture contracts, the current implementation, its callers, and the relevant tests. In this repository's migration, move behavior that is not highly tied to GUI into `mt-core`, including headless platform I/O where applicable; keep GPUI views and GUI integration in `mt-app`, and keep `mt-core` GPUI-free. Use the product contract and the user's scope to resolve borderline cases; a module's current location or crate name alone does not decide ownership.
2. **Trace semantic ownership.** For each value crossing the boundary, identify who owns it, how long it lives, and whether callers read, mutate, transfer, or genuinely share it. Prefer moves for transferred ownership and borrows for read-only access. Clone only when independent ownership is required by the contract; use `Rc` or `Arc` only for actual shared ownership. Restructure when a borrow-checker workaround obscures the intended owner.
3. **Map and complete the cutover.** Find the definition, callers, references, related types, visibility, and module exports with available code search or LSP. Move cohesive behavior together, then update every affected import, caller, re-export, and test. Preserve a compatibility path only when the product or public API contract requires it.
4. **Preserve failure semantics.** Classify each case before choosing a type. Use `Result` for expected failures unless the product contract represents them as diagnostics. Use `Option` for normal absence. Assert only for invariant violations the contract treats as programmer bugs. Preserve the context callers need; do not change public error types or add an error crate just to satisfy generic advice. In this product, content failures are diagnostics and source text remains editable.
5. **Review only the changed flow.** Inspect new clones, panics, ignored errors, visibility, pointer sharing, and unsafe code only where the moved behavior touches them. Fix a concrete ownership, failure, or boundary defect required by the task; leave unrelated style and lint cleanup out.
6. **Verify the moved behavior.** Run a focused existing test for the changed behavior, using the repository's documented command and an appropriate filter, for example:
   - `cargo test --locked --profile ci -p mt-core <focused_filter>`
   - `cargo test --locked --profile ci -p mt-app <focused_filter>`

   Choose the affected package. If no relevant test exists, run `cargo check --locked -p mt-core` or `cargo check --locked -p mt-app` for the affected crate and say that behavior was not test-covered. Follow `docs/development.md` for the settled integration tier; a focused check is not a substitute for that gate.
7. **Report completion.** State the moved responsibility and boundary, affected public references or failure contract, the focused command and observed result, and any higher-level check still pending. Separate observed outcomes from unverified claims.

## Authority and offline use

This checklist is self-contained; the pinned sources below are optional background when available. Repository product contracts, the requested migration scope, existing behavior tests, and relevant measured evidence outrank generic Rust guidance. Claim a performance improvement only when comparable project measurements support it. Apply the checklist to the changed path, not as a reason to reformat or lint unrelated code.

## Pinned upstream sources

Selected ideas are informed by [`actionbook/rust-skills` v2.0.9 at commit `ac5853d230662cc26ace57af995efc52fe7b4309`](https://github.com/actionbook/rust-skills/tree/ac5853d230662cc26ace57af995efc52fe7b4309):

- [rust-refactor-helper](https://github.com/actionbook/rust-skills/blob/ac5853d230662cc26ace57af995efc52fe7b4309/skills/rust-refactor-helper/SKILL.md) — reference and caller impact before a move.
- [m01-ownership](https://github.com/actionbook/rust-skills/blob/ac5853d230662cc26ace57af995efc52fe7b4309/skills/m01-ownership/SKILL.md) — decide who owns data before choosing a compiler workaround.
- [m06-error-handling](https://github.com/actionbook/rust-skills/blob/ac5853d230662cc26ace57af995efc52fe7b4309/skills/m06-error-handling/SKILL.md) — distinguish expected failure, normal absence, and bugs.
- [m15-anti-pattern](https://github.com/actionbook/rust-skills/blob/ac5853d230662cc26ace57af995efc52fe7b4309/skills/m15-anti-pattern/SKILL.md) — trace suspicious patterns to changed ownership or error design.
- [coding-guidelines](https://github.com/actionbook/rust-skills/blob/ac5853d230662cc26ace57af995efc52fe7b4309/skills/coding-guidelines/SKILL.md) — consult only for a concrete Rust-specific issue in the changed flow.

The pinned upstream README advertises MIT, while its `LICENSE` endpoint at this commit returns 404. This skill uses original checklist wording and links to upstream sources; it does not reproduce upstream files or rely on their text being available.
