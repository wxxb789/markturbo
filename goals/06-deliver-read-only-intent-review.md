# Goal 06 — Deliver read-only Intent Review

## Objective

Let a user review the active document or selection without changing one document
source byte: present a validated structured account of understood intent,
source-grounded findings, assumptions, and no more than five prioritized
clarification questions through the artifact lenses approved in Goal 01; process
every Goal 01 evaluation artifact under a recorded model, model-version, Review
prompt-version, and sampling configuration, meet the owner-approved usefulness
threshold, and degrade explicitly when no model is configured or a response is
stale, malformed, cancelled, or unavailable.

## Product behavior

Review is an inspection before it is an editor. It should help the user compare
what they meant with what another reasoner understood.

A successful result distinguishes:

- stated goal;
- relevant context and inputs;
- constraints and non-goals;
- expected deliverable;
- success evidence;
- inferred assumptions;
- unresolved or contradictory decisions.

It asks only questions whose answers could materially change the requested
outcome. It does not reward length, inject generic prompt boilerplate, or claim a
universal prompt score.

## In scope

- A Review command for the whole active document and the current selection.
- An explicit artifact lens, with a sensible inferred default and a visible way
  to correct it. Support the lens set approved by Goal 01; the expected initial
  set is Prompt, Specification/Plan, Agent Instructions, and Agent Skill.
- Provider-independent Review request and result types that do not introduce
  GPUI, HTTP, or a model SDK into `mt-doc`.
- A structured “What I understand” presentation plus findings and up to five
  ranked clarification questions.
- Present generated prose in the selected interface language or another explicit
  Review-language setting approved in Goal 01 while preserving quoted source
  text exactly; do not silently couple it to the Translation target language.
- Render model output as inert structured text. It must not execute HTML, MDX,
  script, command links, tool calls, or model-supplied UI actions.
- A stable source anchor for every localized finding. Document-wide findings
  must say that they are document-wide rather than inventing a line.
- Explicit labels separating source statements from model inferences.
- Use Goal 05A's credential, endpoint-identity, consent, and outbound-scope
  boundary. Review must show whether it is sending a selection or whole document;
  selection-only analysis must identify its missing surrounding context rather
  than pretending to describe the complete artifact.
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
- Resolving inherited agent instructions or Effective Agent Context. Goal 08 owns
  that input.
- Running the reviewed prompt, comparing model outputs, or acting as an agent.
- A generic chat transcript, prompt marketplace, template library, numerical
  quality score, or hidden model memory.
- Adding a second transport architecture if Goal 05 was correctly skipped.

## Evaluation standard

Run Review over every artifact in Goal 01's corpus using the approved reference
configuration. For each artifact, record:

- whether the structured result decoded completely;
- which annotated high-impact findings or questions were surfaced;
- unsupported claims or false source anchors;
- low-value boilerplate questions;
- project-owner usefulness judgment.

The quantitative acceptance threshold is the one approved in Goal 01. Do not
weaken it after seeing results. Exact prose is not compared; grounded meaning and
question usefulness are.

## Completion evidence

1. Document and selection Review both work from a clean profile after explicit
   provider configuration through Goal 05A's secure path.
2. No success, error, cancellation, timeout, stale result, or malformed response
   changes editor text or dirty state.
3. Every localized finding navigates to the source range it claims; selection
   Review visibly states that surrounding document context was omitted.
4. No result displays more than five clarification questions, and their priority
   is represented explicitly.
5. All corpus artifacts have recorded model/configuration metadata and evaluation
   results meeting Goal 01's threshold and receive project-owner approval.
6. Keyless tests cover success, no provider, local endpoint label, remote
   disclosure, cancellation, timeout, malformed fields, oversized payload, stale
   source, CJK, empty selection, and prompt-like instructions embedded in the
   reviewed document.
7. `cargo fmt --all -- --check`, `cargo clippy --workspace --all-targets`, focused
   Review tests, and `cargo test --release --workspace` pass; the pass count is
   recorded.

## Stop and ask

Stop if Goal 01 has no approved model-data policy or evaluation threshold, Goal
05A has not established a safe credential/request path, or no artifact lens can
be selected without changing the intended first user. Do not send corpus or user
content to an endpoint merely to make a test pass.

## Boundary for the next goal

This goal ends with a useful, entirely non-mutating Review. Goal 07 alone adds
answers, generated revisions, diff acceptance, and editor mutation.
