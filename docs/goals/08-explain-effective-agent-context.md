# Goal 08 — Explain effective agent context

## Objective

For a user-selected target path and the supported, versioned Codex profile,
resolve and display the documented instruction chain for the selected execution
context, why each source applies, its scope and precedence, and which Skills are
merely available on demand rather than already included; implement the
`codex-agents-md-2026-08-29` profile, verify at least eight applicability and
precedence fixtures against authoritative semantics, and make the resolved
context available to Review without claiming verified resolution for
every discovered harness or runtime configuration.

## Product contract alignment

**Disposition:** Retained and revised on 2026-08-29.

Effective Agent Context is a core capability for the Windows 11 x64
public-quality release. Its only verified resolution profile is
`codex-agents-md-2026-08-29`, based on the OpenAI Codex `AGENTS.md` discovery and
precedence contract documented at
<https://developers.openai.com/codex/agent-configuration/agents-md>. Other
instruction files remain reviewable artifacts or inventory-only entries until a
later contract approves their semantics. A user must explicitly select the named
context sources to include in Review, and the per-request outbound disclosure
must name local/remote status, provider wire format, normalized endpoint
identity, and the document plus selected-context scope. Resolving or viewing
context sends nothing.

The named profile is a checked-in snapshot of those authoritative semantics. Its
profile data and fixtures record the source URL, retrieval date, and content
digest used to establish the rules. The resolver and tests use that snapshot;
later live-documentation changes do not mutate
`codex-agents-md-2026-08-29`. Changed semantics require a new named,
owner-approved profile and fixture set.

## User question

> For this target and this agent, what will actually be seen, in what order, and
> where should I edit it?

The current Harness inventory is useful input, but a list of files is not yet an
answer to that question.

## In scope

- A context target grounded in the open workspace that includes the target path,
  working directory or project root, supported harness/version profile, and every
  user-visible configuration input needed by its documented resolution rules.
- A harness selector whose only verified context choice is
  `codex-agents-md-2026-08-29`; every other discovered harness is labeled
  inventory-only.
- A checked-in `codex-agents-md-2026-08-29` profile and fixture set that freezes
  the authoritative source URL, retrieval date, content digest, and implemented
  applicability and precedence rules. Runtime resolution and tests must not
  consult live documentation; a later source change requires a new profile.
- Codex global and project discovery, one-file-per-directory selection,
  root-to-target ordering, override/fallback filename behavior, byte limit, and
  precedence only where authoritative documentation or inspected source
  establishes them. Do not present these rules as universal behavior shared by
  every agent.
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
- A deterministic serialized context view that Review can consume only after
  the user selects its named sources for that request, starts the operation,
  and accepts the endpoint and content-scope disclosure.
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
8. Codex global/user and project-local precedence;
9. an unsupported import or rule is reported rather than followed;
10. path case and separator behavior on supported platforms.

One fixture may cover several cases, but the completion report must map every
case to an assertion.

## Completion evidence

- The checked-in profile cites authoritative documentation or inspected upstream
  source for every implemented applicability and precedence rule, and records
  its source URL, retrieval date, and content digest.
- Tests use the checked-in profile and fixtures rather than live documentation;
  a later documentation change cannot alter the named profile's result.
- The fixture suite proves inclusion, exclusion, ordering, provenance, dedup,
  malformed input, and unsupported behavior without a network key.
- A manual Windows 11 x64 run selects two different target paths and visibly
  changes the effective chain for the documented reason.
- Passing context into Review requires explicit user selection of each named
  source, identifies every selected source in the endpoint/content disclosure,
  and preserves that disclosure; no context is sent by merely opening this view.
- Unsupported harnesses are labeled inventory-only rather than appearing to have
  verified context support, and every supported result displays the harness
  profile/version and configuration assumptions under which it was resolved.
- `mt-doc` or another headless domain boundary remains GPUI-free.
- `cargo fmt --all -- --check`, `cargo clippy --workspace --all-targets`, focused
  context tests, and `cargo test --release --workspace` pass; the pass count is
  recorded.

## Stop and ask

Stop if the `codex-agents-md-2026-08-29` effective-context semantics cannot be
established from authoritative OpenAI documentation or inspected source. Do not
substitute a different harness or generalize from observed file names.

## Boundary for the next goal

This goal owns context applicability and provenance, not visual redesign. Goal 09
may rename and reorganize the existing Harness/Details surfaces around this
completed behavior without changing its semantics. Goal 10 remains blocked until
selected context is integrated into Review with the approved disclosure.
