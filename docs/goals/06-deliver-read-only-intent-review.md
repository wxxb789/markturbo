# Goal 06 — Deliver read-only Review

## Objective

Let a user review the active document or selection without changing one document
source byte: present a validated structured account of understood intent,
source-grounded findings, assumptions, and no more than five prioritized
clarification questions through the artifact lenses in `PRODUCT.md`; process all
12 artifacts in `evaluation/goal-01/CORPUS.md` under a recorded model,
model-version, Review prompt-version, and sampling configuration, meet the fixed
usefulness threshold, and degrade explicitly when no model is configured or a
response is stale, malformed, cancelled, or unavailable.

## Product behavior

Review is an inspection before it is an editor. It should help the user compare
what they meant with what another reasoner understood.

## Product contract alignment

**Disposition:** Retained and revised on 2026-08-29.

This is the document-only intermediate Review promised by `PRODUCT.md`, not the
first public-quality context-aware release. It keeps source text unchanged,
supports an Agent Skill as its `SKILL.md` plus supporting directory artifact, and
renders generated output in the selected interface language (English or
Simplified Chinese) while quoted source remains byte-exact. Until Goal 08,
Review must neither resolve nor include Effective Agent Context.

A successful result distinguishes:

- stated goal;
- relevant document-derived context and inputs;
- constraints and non-goals;
- expected deliverable;
- success evidence;
- inferred assumptions;
- unresolved or contradictory decisions.

It asks only questions whose answers could materially change the requested
outcome. It does not reward length, inject generic prompt boilerplate, or claim a
universal prompt score.

An Agent Skill package has deterministic request semantics. Include regular
files below the selected Skill root only, ordered by normalized UTF-8 relative
path in byte order, and never follow a symbolic link. Every source frame must
unambiguously contain the path length and path bytes followed by the content
length and raw content bytes. Binary or non-UTF-8 content contributes only its
path, byte size, and SHA-256 metadata unless the user explicitly selects its raw
content. The Review source limit is 512 KiB per file and 4 MiB in aggregate;
when either limit is exceeded, report it and do not silently truncate. These
source limits preserve the 8 MiB worker-frame compatibility established by Goal
05.

## In scope

- A Review command for the whole active document and the current selection.
- An explicit artifact lens, with a sensible inferred default and a visible way
  to correct it. Support the lens set approved by `PRODUCT.md`: Prompt,
  Specification/Plan, Agent Instructions, and Agent Skill. An Agent Skill is a
  `SKILL.md` plus its supporting directory treated as one artifact, not only a
  single Markdown file. It uses the deterministic package rules above and, before
  sending, displays every included file's normalized relative path, byte size,
  and inclusion reason. A file not listed must not be sent; omitting a normally
  in-scope supporting file labels the resulting Review partial.
- Provider-independent Review request and result types that do not introduce
  GPUI, HTTP, or a model SDK into `mt-doc`.
- A structured “What I understand” presentation plus findings and up to five
  ranked clarification questions.
- Present generated prose in the selected interface language, English or
  Simplified Chinese, while preserving quoted source text exactly; do not
  silently couple it to the Translation target language.
- Render model output as inert structured text. It must not execute HTML, MDX,
  script, command links, tool calls, or model-supplied UI actions.
- A stable source anchor for every localized finding: a relative path plus byte
  range or line reference for a package file, or the active document's byte
  range or line reference. Document-wide findings must say that they are
  document-wide rather than inventing a line.
- Explicit labels separating source statements from model inferences.
- Use Goal 05A's credential, endpoint-identity, consent, and outbound-scope
  boundary. Request inspection must prove that the displayed outbound inventory
  exactly equals the user-content bytes provided to the adapter, excluding
  documented protocol framing. Review must show whether it is sending a
  selection, whole document, or Agent Skill package; selection-only analysis
  must identify its missing surrounding document context rather than pretending
  to describe the complete artifact. Do not resolve, display, or send Effective
  Agent Context before Goal 08.
- Treat document content as delimited, untrusted data. Text that asks the reviewer
  to ignore its contract, emit protocol fields, invoke tools, or change endpoints
  cannot alter transport/configuration or bypass structured validation.
- Source-snapshot identity. If the user edits while Review runs, retain the
  result only as stale inspection and never present it as describing the current
  revision.
- Cancellation, request-size limits, timeout behavior, and validated structured
  decoding. Invalid output becomes a diagnostic, never partially trusted UI.
- Useful local-only behavior when no model is configured: existing syntax,
  frontmatter, Skill, and document diagnostics remain available, and the semantic
  Review clearly explains what configuration is missing.
- Recorded or deterministic provider fixtures sufficient to test all UI states
  without a network key.

## Out of scope

- Rewriting text, answering questions inside the app, generating patches, showing
  a diff, or applying edits. Goal 07 owns all mutation.
- Resolving, displaying, or including inherited agent instructions or Effective
  Agent Context. Goal 08 alone owns that input, disclosure, and request scope.
- Running the reviewed prompt, comparing model outputs, or acting as an agent.
- A generic chat transcript, prompt marketplace, template library, numerical
  quality score, or hidden model memory.
- Adding a second transport architecture if Goal 05 was correctly skipped.

## Evaluation standard

Run Review over every artifact in immutable corpus version `goal-01-v1` under
the fixed [Review Evaluation Contract](../../PRODUCT.md#review-evaluation-contract).
Before sending a corpus artifact or scoring a result as threshold evidence,
verify `evaluation/goal-01/MANIFEST.sha256`. Record the corpus version and
manifest digest with every evaluation result. A hash mismatch disqualifies the
artifact and result from threshold evidence. The contract is the sole authority
for the reference configuration and acceptance thresholds; this goal must not
weaken or duplicate them.

For each artifact, record:

- whether the structured result decoded completely;
- which annotated high-impact findings or questions were surfaced;
- unsupported claims or false source anchors;
- low-value boilerplate questions;
- project-owner usefulness judgment.

Do not weaken the contract after seeing results. Exact prose is not compared;
grounded meaning and question usefulness are.

## Completion evidence

1. Document and selection Review both work from a clean profile after explicit
   provider configuration through Goal 05A's secure path.
2. No success, error, cancellation, timeout, stale result, or malformed response
   changes editor text or dirty state.
3. Every localized finding navigates to the source range it claims; package
   findings identify a relative path plus byte range or line reference, and
   selection Review visibly states that surrounding document context was omitted.
4. No result displays more than five clarification questions, and their priority
   is represented explicitly.
5. All 12 corpus artifacts have recorded model/configuration metadata, corpus
   version, and verified manifest digest, and meet the fixed Review Evaluation
   Contract. No result with a manifest mismatch is counted as threshold evidence.
6. Keyless tests cover success, no provider, local endpoint label, remote
   disclosure, cancellation, timeout, malformed fields, oversized payload, stale
   source, CJK, empty selection, and prompt-like instructions embedded in the
   reviewed document.
7. `cargo fmt --all -- --check`, `cargo clippy --workspace --all-targets`, focused
   Review tests, and `cargo test --release --workspace` pass; the pass count is
   recorded. Focused tests include an exact request-inspection proof for an Agent
   Skill with a supporting file and an omitted-supporting-file partial result.

## Stop and ask

Stop if Goal 05A has not established the approved credential/request path, or if
no artifact lens can be selected without changing the intended first user. Do
not send corpus or user content to an endpoint merely to make a test pass.

## Boundary for the next goal

This goal ends with a useful, entirely non-mutating Review. Goal 07 alone adds
answers, generated revisions, diff acceptance, and editor mutation.
