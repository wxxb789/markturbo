# Goal 08 — Explain effective agent context

## Objective

For a user-selected target path and supported, versioned harness profile, resolve
and display the documented instruction chain for the selected execution context,
why each source applies, its scope and precedence, and which Skills are merely
available on demand rather than already included; implement one documented
`AGENTS.md` profile plus exactly one additional harness selected in Goal 01,
verify at least eight applicability and precedence fixtures against authoritative
semantics, and make the resolved context available to Intent Review without
claiming support for every discovered harness or every runtime configuration.

## User question

> For this target and this agent, what will actually be seen, in what order, and
> where should I edit it?

The current Harness inventory is useful input, but a list of files is not yet an
answer to that question.

## In scope

- A context target grounded in the open workspace that includes the target path,
  working directory or project root, supported harness/version profile, and every
  user-visible configuration input needed by its documented resolution rules.
- A harness selector limited to semantics implemented and verified by this goal.
- Ancestor discovery and ordering for one explicitly documented `AGENTS.md`
  profile; do not present it as universal behavior shared by every agent.
- Resolution for one additional owner-selected harness, including its supported
  project, user/global, local, import, frontmatter, or glob rules only where
  authoritative documentation or inspected source establishes them.
- Present effective automatic instructions separately from available Skills.
  Skill contents are not part of effective context unless the selected profile
  documents that they were invoked; availability alone must never concatenate
  them into the instruction chain.
- For every included source, show:
  - path and origin;
  - the rule that caused inclusion;
  - applicable scope;
  - precedence or ordering;
  - supported harness/profile version;
  - whether it is loaded automatically or merely available on demand.
- Preserve duplicate aliases and symlinks as provenance while avoiding duplicate
  effective content.
- Report ambiguous, malformed, missing, cyclic, or unsupported rules explicitly;
  never silently guess that a source applies.
- Navigation from each context entry to its ordinary source file.
- A deterministic serialized context view that Goal 06/07 Review can consume
  after explicit user approval and endpoint disclosure.
- Show exact duplicate directives or sources deterministically. Semantic conflict
  analysis may use the existing Review operation, but this goal must not invent a
  second model pipeline.
- Keep context resolution in a UI-independent boundary so future CLI or headless
  consumers do not need GPUI.

## Out of scope

- Exact semantics for all 80+ harnesses in the discovery catalog.
- Running an agent, emulating its complete system prompt, measuring token use, or
  guaranteeing behavior that the harness itself does not specify.
- RAG, vector indexing, repository-wide semantic search, or a knowledge graph.
- Automatically editing a different source file; existing Review may propose a
  change only after the user selects the artifact to revise.
- Hidden concatenation of context or sending it remotely without Goal 05A's
  per-source disclosure and explicit user action.
- Resolving remote imports, executing instruction content, or performing network
  access merely to compute context; unsupported remote sources remain explicit.
- A plugin system for third-party harness resolvers.

## Required fixture coverage

At least eight fixtures must collectively cover:

1. root and nested `AGENTS.md` files for a deep target;
2. a sibling instruction that must not apply;
3. a target outside the workspace;
4. a missing or removed instruction source;
5. a symlink or alias reached through multiple discovery conventions;
6. duplicate effective content with distinct provenance;
7. malformed harness-specific metadata, import, or scope syntax;
8. global/user and project-local precedence for the selected harness;
9. a cycle if the selected harness supports imports;
10. path case and separator behavior on supported platforms.

One fixture may cover several cases, but the completion report must map every
case to an assertion.

## Completion evidence

- Authoritative documentation or inspected upstream source is cited for every
  implemented applicability and precedence rule.
- The fixture suite proves inclusion, exclusion, ordering, provenance, dedup,
  malformed input, and unsupported behavior without a network key.
- A manual primary-platform run selects two different target paths and visibly
  changes the effective chain for the documented reason.
- Passing context into Review identifies every source and preserves the user's
  endpoint/content disclosure; no context is sent by merely opening this view.
- Unsupported harnesses are labeled inventory-only rather than appearing to have
  verified context support, and every supported result displays the harness
  profile/version and configuration assumptions under which it was resolved.
- `mt-doc` or another headless domain boundary remains GPUI-free.
- `cargo fmt --all -- --check`, `cargo clippy --workspace --all-targets`, focused
  context tests, and `cargo test --release --workspace` pass; the pass count is
  recorded.

## Stop and ask

Stop if Goal 01 does not name the additional harness, or if its effective-context
semantics cannot be established from authoritative documentation or source. Ask
whether to select another harness rather than generalizing from observed file
names.

## Boundary for the next goal

This goal owns context applicability and provenance, not visual redesign. Goal 09
may rename and reorganize the existing Harness/Details surfaces around this
completed behavior without changing its semantics.
