# Goal 01 — Establish the current product contract

## Objective

Create one owner-approved product contract for markturbo that names the first
recurring user, their primary job, the North Star user outcome, the first
artifact types and platform in scope, credential/recovery/remote-model data
policies, explicit non-goals, and observable success measures; support it with
at least 12 rights-cleared real or anonymized evaluation artifacts covering
prompts, specifications or plans, agent instructions, and Agent Skills, without
changing runtime behavior.

## Why this goal comes first

The existing product thesis explains the medium—Markdown as the interface
between humans and agents—but not yet the one recurring outcome that should
order future work. Performance, visual design, model integration, and context
resolution cannot be judged coherently until that outcome and first user are
explicit.

## Required product decision

The contract should test and either adopt or replace this candidate promise:

> markturbo helps people turn rough intent into clear, testable,
> context-aware Markdown by showing what an agent will understand, asking what
> remains unclear, and applying only changes the user approves.

The candidate first user is a developer who uses coding agents and maintains
prompts, specifications, `AGENTS.md`, `CLAUDE.md`, scoped instructions, or
`SKILL.md`. Do not silently treat this candidate as approved if project-owner
feedback points elsewhere.

## In scope

- Name one first recurring user and one primary job-to-be-done.
- Define one North Star user outcome and a small set of leading and guardrail
  measures.
- Decide whether near-term success means retained users, open-source adoption,
  commercial validation, or another explicitly named outcome.
- Select the primary platform for the next public-quality release based on
  actual support, not nominal portability.
- State whether semantic Review may send explicitly selected content to a
  configured endpoint and what meaningful behavior remains local-only.
- Define how API credentials may persist and how local recovery content is
  protected, bounded by count/bytes/age, and deleted. Include the maximum
  acknowledged recovery-loss window after interruption.
- Choose the reference model/configuration and a pre-registered usefulness
  threshold for Review evaluation, plus the minimum first-time-user evidence
  required for later visual acceptance.
- Pre-register the post-release observation window, representative-user count,
  activation/retention/usefulness/trust thresholds, and consensual evidence method
  that Goal 11 will use; do not leave “successful” measurable only by downloads.
- Define the relationship among Review, Agent Context, Markdown editing,
  rendering, diagrams, MDX, Skills, and Translation: core, supporting, or
  compatibility capability.
- Preserve the existing local-first rule: ordinary files remain authoritative
  and no import/export step becomes required.
- Reconcile the current long-form `GOAL.md` with the new contract so only one
  document is presented as current product authority. Preserve useful
  implementation history rather than silently deleting it.
- Re-read every later file in `docs/goals/` after approval and revise, replace, or
  explicitly retire any objective that no longer serves the selected user and
  promise; numeric order must not turn a rejected product hypothesis into work.
- Assemble at least 12 rights-cleared real or anonymized artifacts, with at least
  two examples from each of these groups:
  - task prompts;
  - specifications or plans;
  - agent instruction documents;
  - Agent Skills.
- Annotate every evaluation artifact with useful questions or findings, changes
  that would violate its intent, and the observable outcome a good review
  should support. Exact model wording is not an expected result.

## Out of scope

- Implementing Review, Effective Agent Context, model transport, or UI changes.
- Selecting a dynamic-linking or worker-process architecture.
- Adding telemetry, accounts, cloud storage, or a proprietary workspace format.
- Expanding the renderer, theme, language, or harness catalogs.
- Producing a delivery plan, decision ledger, or execution journal.

## Completion evidence

This goal is complete only when all of the following are true:

1. One concise product contract is identified as the current source of truth.
2. It answers, without placeholders: who, recurring job, promise, North Star
   outcome, first platform, credential/recovery/model-data policies, core
   capabilities, non-goals, evaluation threshold, usability evidence, and
   success measures.
3. At least 12 evaluation artifacts and their human-authored annotations are
   present, contain no secrets or private data, and record provenance plus the
   right to retain and, where intended, redistribute them. Owner-local cases are
   clearly excluded from commits and release artifacts.
4. Every later goal in `docs/goals/` has been re-reviewed and either aligns with the
   approved contract or is explicitly revised, replaced, or retired before work
   continues.
5. The project owner explicitly approves the contract and evaluation set.
6. Documentation links affected by superseding or relocating the old goal are
   valid, and `git diff --check` reports no whitespace errors.

## Stop and ask

Stop and ask the project owner one concise question if any of these remain
ambiguous after reviewing repository evidence:

- the first recurring user or primary artifact;
- the meaning of project success;
- the primary platform;
- whether selected document content may leave the machine;
- how credentials and local recovery content may persist;
- the Review evaluation or first-time-user acceptance threshold.

Do not compensate for a missing product decision by adding broader scope.

## Boundary for the next goal

This goal defines what success means. It must not implement the safety behavior,
first-use flow, Review experience, context semantics, or performance architecture
covered by later goals.
