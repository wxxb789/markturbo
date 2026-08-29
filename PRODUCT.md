# Product Contract

**Status:** Current owner-approved product authority, approved on 2026-08-29.

This document governs product scope and success. `GOAL.md` is retained only as
a compatibility pointer to this contract and the historical v0.1 direction.

## First Recurring User

The first recurring user is a developer who uses coding agents at least weekly
and maintains task prompts, specifications or plans, agent instruction files,
or Agent Skills.

Their primary job is:

> Before handing an artifact to an agent, make its intended outcome, context,
> constraints, and success evidence explicit, then accept only changes they
> have inspected and approved.

## Product Promise

markturbo helps developers turn rough intent into clear, testable,
context-aware Markdown by showing what the configured reviewer understood and
which agent instructions apply, asking only questions that could materially
change the outcome, and applying only changes the user explicitly approves.

"Context-aware" is a release-level promise. Document-only Review may ship as an
intermediate implementation step, but the first public-quality release does not
make this promise until Effective Agent Context is visible and can be included
in Review.

## North Star Outcome

A target developer returns to markturbo and leaves with an artifact they judge
ready for an agent to act on, with fewer avoidable clarification cycles or
rework than their previous workflow.

Near-term product success means retained use by representative developers. Raw
downloads, GitHub stars, repository traffic, and commercial interest are useful
context but are not success measures for this contract.

Leading measures are first-Review activation, useful Review, deliberate
acceptance or rejection of proposed revisions, and a later Review on a separate
day. Guardrails are user-text preservation, informed outbound-data consent,
credential confidentiality, grounded findings, and user trust.

## First Release Scope

### Primary platform

The first public-quality platform is Windows 11 x64. Windows 10 x64 may remain
compatible, and macOS and Linux may continue to build, but none is described as
public-quality until it receives its own clean-machine acceptance evidence.

### Artifact types

The first Review lenses are:

- task prompt;
- specification or plan;
- agent instructions, including `AGENTS.md`, `CLAUDE.md`, scoped instruction
  files, and equivalent conventions;
- Agent Skill, with `SKILL.md` and its supporting directory treated as one
  artifact.

The first public-quality Effective Agent Context profile verifies only
`codex-agents-md-2026-08-29`, using Codex's documented
[AGENTS.md discovery rules](https://developers.openai.com/codex/agent-configuration/agents-md).
Other discovered instruction files remain reviewable as standalone artifacts,
but their context-resolution output is inventory-only: it must not claim that
they apply or present them as verified Effective Agent Context.

Ordinary Markdown remains the source format. No import, export, database, or
proprietary workspace conversion is required.

### Capability roles

| Capability | Role | Contract |
|---|---|---|
| Review | Core | Expose understood intent, grounded findings, assumptions, and consequential questions. |
| Effective Agent Context | Core | Show the instructions that apply and let the user include that disclosed context in Review. |
| Markdown editing | Core | Preserve ordinary files and make only predictable, user-approved text changes. |
| Agent Skills | Core artifact support | Review the full skill artifact and preserve its ecosystem format. |
| Native rendering | Supporting | Help users inspect structure and content without defining product success. |
| Workspace, search, and Skills explorer | Supporting | Help users locate and inspect the artifacts used by the core workflow. |
| Translation | Supporting | Preserve the existing explicit, document-aware operation without making it a release thesis. |
| Web rendering, MDX, diagrams, and math | Compatibility | Preserve useful existing compatibility and smoke coverage; do not let catalog growth order the roadmap. |

## Local-First And Model-Data Policy

Opening, editing, rendering, searching, discovering Skills, resolving Effective
Agent Context, and creating recovery checkpoints are local-only operations.
They send no document or context content.

Semantic Review and Translation may send content to a configured endpoint only
after a user explicitly starts that operation. Before sending, the UI discloses:

- whether the endpoint is local or remote;
- the provider wire format and normalized endpoint identity;
- whether the scope is a selection, document, or document plus named Effective
  Agent Context sources;
- when the artifact is an Agent Skill package, every file that will leave the
  device by normalized relative path, byte size, and inclusion reason;
- that protocol framing and the disclosed source content cross the boundary.

Consent is scoped to the operation, endpoint identity, and displayed content
scope. A material change requires confirmation again. The application performs
no background model request, content telemetry, training upload, or hidden
workspace scan. A file that is not in the displayed Agent Skill package list
must not be sent. If a normally in-scope supporting file is omitted, Review
labels the result partial rather than representing the package as fully
reviewed.

Without a configured model, all local-only capabilities remain available.
Semantic Review explains that model configuration is missing rather than
simulating a result.

## Credential Policy

On the primary platform, persistent API credentials are stored in Windows
Credential Manager. A credential identity includes the application, provider
wire format, scheme, normalized host, effective port, and normalized API base
path. A credential for one identity is never reused for another silently.

Environment credentials are valid only for their vendor-default endpoint unless
the user explicitly authorizes the custom endpoint. A session-memory credential
is an allowed non-persistent fallback. If secure persistence is unavailable,
markturbo offers environment or session-only behavior and never falls back to a
plaintext settings file.

An API base URL is parsed before use and rejects userinfo, query, and fragment
components. A non-loopback endpoint must use HTTPS with certificate validation.
HTTP is allowed only for a verified loopback endpoint and is disclosed as
unencrypted with no proxy. The client never automatically follows redirects;
any redirect response is a diagnostic.

An existing plaintext key is migrated only after explicit approval and a
verified secure write. It is removed from settings only after that write
succeeds. Credentials and request bodies are excluded from settings, recovery,
logs, process arguments, errors, screenshots, fixtures, and metrics.

## Recovery Policy

Dirty-buffer recovery is local, optional application state and never a source
file or workspace requirement. On Windows, checkpoint content is protected for
the current user with DPAPI and written atomically under the application data
directory.

- checkpoint after two seconds without an edit and at least every ten seconds
  while a buffer remains dirty;
- acknowledge a maximum ten-second recovery-loss window after interruption;
- retain at most 50 records, 32 MiB per record, and 128 MiB in total;
- expire records after seven days;
- delete a record after an intentional Save or Discard;
- prune expired records at startup and after a completed checkpoint;
- evict the oldest inactive record first when a count or byte bound is reached;
- never discard the newest checkpoint for an open dirty buffer without a
  visible warning;
- report an unavailable, failed, or oversized checkpoint without blocking the
  editor or damaging the source file.

A recovered buffer retains its source identity, encoding, BOM, line endings,
and conflict stamp. If disk content changed while the application was closed,
the recovered buffer is conflicted and cannot overwrite implicitly.

## Review Evaluation Contract

The initial reference configuration is:

- OpenAI Responses wire format;
- model `gpt-5.6-terra`, with the response-reported model identifier recorded;
- `reasoning.effort=medium`;
- no tools, browsing, memory, or agent actions;
- sampling controls omitted so provider defaults apply and are recorded as such;
- structured schema and system prompt version `review-v1`;
- one request per artifact, with the exact artifact bytes, configuration, and
  result retained in owner-local evaluation records when permitted.

The checked-in evaluation set is immutable corpus version `goal-01-v1`. It is
defined by
`evaluation/goal-01/CORPUS.md` and fixed by
`evaluation/goal-01/MANIFEST.sha256`. The final manifest hashes `CORPUS.md`,
`OWNER-ANNOTATIONS.md`, `THIRD-PARTY-NOTICES.md`, the four task prompts, the
four listed specification/plan artifacts, the two listed instruction documents,
and every regular file below each of the two listed Agent Skill directories.
Any hash-covered artifact, annotation, notice, provenance, or other corpus
metadata change requires a new owner-approved corpus version; `goal-01-v1` is
never updated in place. Exact wording is never compared. Goal 06 passes only when:

1. all 12 results decode completely against the structured schema;
2. at least 10 of 12 are judged useful by the project owner;
3. at least 75% of the pre-annotated high-impact findings or questions are
   surfaced with equivalent grounded meaning;
4. no result invents a source anchor;
5. no more than one artifact contains a materially misleading finding;
6. no result contains more than five clarification questions.

The threshold is fixed before Review implementation and is not weakened after
results are seen.

Before sending a corpus artifact for evaluation or scoring a result as threshold
evidence, Goal 06 verifies the version's manifest. Each evaluation record names
the corpus version and records the manifest digest. A hash mismatch makes the
artifact and any associated result ineligible as threshold evidence.

## First-Time-User Evidence

Before visual acceptance, five first-time target users use a clean Windows 11
profile with a real or rights-cleared artifact. Without facilitator instruction
after the task brief:

- at least four of five complete open or paste, Review, interpret, and deliberate
  accept or reject within 15 minutes;
- all five correctly identify what will leave the machine before the request;
- all five can distinguish a finding from an inference and an unapplied revision
  from source text;
- there are zero silent sends, silent source changes, or accidental-loss events.

English and Simplified Chinese may both be used in evidence sessions. Review
output follows the selected interface language while quoted source remains
byte-exact.

## Post-Release Validation

The observation window is 42 days from the first public-quality Windows release.
The pre-registered cohort is 12 distinct target users. Fewer than 12 produces an
inconclusive recruitment result, not validation.

The release supports a `continue` decision only when all of these hold:

- activation: at least 9 of 12 complete a first Review and deliberate revision
  decision within 15 minutes;
- usefulness: at least 8 of 12 report that Review exposed or resolved a material
  assumption, decision, scope boundary, or success measure, or correctly
  confirmed that no material revision was needed;
- retention: at least 6 of 12 complete another Review on a separate day at least
  seven days after first use and within the 42-day window;
- trust: at least 10 of 12 rate control over outbound scope and source changes at
  least 4 of 5;
- guardrails: zero unconsented outbound requests, silent source mutations,
  accidental text-loss events, credential disclosures, or content-bearing
  telemetry records.

Evidence comes from consented moderated sessions, follow-up interviews, and
optional local content-free metrics deliberately exported by the user. There is
no hidden telemetry and no account requirement. Recordings require separate
consent and are deleted within 30 days; de-identified research notes and exported
event summaries are deleted within 90 days after the `continue`, `revise`, or
`stop` decision.

## Explicit Non-Goals

- running prompts or acting as a coding agent;
- a generic chat interface, autonomous agent, or model-output comparison tool;
- a full IDE, WYSIWYG editor, PKM system, knowledge graph, or collaboration suite;
- cloud storage, cloud recovery, accounts, sync, or a proprietary document format;
- hidden telemetry or uploading source/model content for product improvement;
- automatic edits, whole-document replacement, or applying a model result
  without source-snapshot validation and explicit approval;
- expanding platforms, providers, renderers, diagram types, themes, or harness
  catalogs before recurring value is validated;
- commercial validation as the near-term success gate.

## Approval Record

- Project owner: repository owner, approved in the Goal 01 thread
- Contract approved: 2026-08-29
- Evaluation set approved: 2026-08-29
- Rights to retain and redistribute every checked-in evaluation artifact:
  approved 2026-08-29 under the terms recorded in the corpus
- Project-owner annotation set adopted and complete: 2026-08-29 in
  `evaluation/goal-01/OWNER-ANNOTATIONS.md`
