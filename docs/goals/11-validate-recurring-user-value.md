# Goal 11 — Validate recurring user value

## Objective

Run the [Post-Release Validation](../../PRODUCT.md#post-release-validation) program
for the first public-quality Windows 11 x64 release: collect privacy-compliant
activation, Review usefulness, approved-revision, trust, and repeat-use evidence
without collecting document or model content; produce one owner-approved
continue, revise, or stop decision against that fixed contract, and add no new
product capability merely to improve the measurement.

## Product contract alignment

**Disposition:** Retained and revised on 2026-08-29.

This goal executes the target-user cohort, observation window, success criteria,
guardrails, evidence mechanisms, and retention policy fixed in
[Post-Release Validation](../../PRODUCT.md#post-release-validation). It owns the
collection, analysis, and continue/revise/stop workflow, not a second set of
product thresholds.

## Why shipping is not the final outcome

A successful build and installer prove delivery. They do not prove that Review
clarifies thinking, that users return, or that the product is more useful than an
ordinary editor or a one-shot model rewrite. This goal closes that gap before the
roadmap expands again.

## In scope

- Use the target user, observation window, cohort, and fixed criteria in
  [Post-Release Validation](../../PRODUCT.md#post-release-validation). Do not choose
  or weaken a criterion after seeing results.
- Observe the complete activation path:
  - install and first launch;
  - open or paste a real artifact;
  - start the first Review;
  - resolve, intentionally defer, or reject a clarification;
  - accept/reject a proposed revision;
  - copy or save the approved artifact.
- Evaluate whether Review changed thought rather than only prose. Ask whether a
  question exposed an assumption, changed a decision, narrowed scope, clarified
  evidence, or correctly confirmed that no revision was needed.
- Measure retention only under the separate-day repeat-use definition in
  [Post-Release Validation](../../PRODUCT.md#post-release-validation); several
  actions in one session are not retention.
- Record safety and trust outcomes: abandoned setup, provider/credential failure,
  stale-result blocks, recovery use, accidental-loss reports, privacy concerns,
  misleading findings, reverted accepted edits, and crashes.
- Where users can assess it honestly, record whether the revised artifact improved
  the subsequent agent interaction; keep “unknown” rather than forcing a rating.
- Segment evidence by artifact lens and endpoint type only when the sample supports
  the distinction; do not draw subgroup conclusions from isolated examples.
- Use only the approved evidence mechanisms:
  - moderated/recorded-with-consent sessions;
  - interviews and user-provided outcome reports;
  - optional content-free local metrics exported deliberately by the user;
  - or an approved combination.
- If minimal measurement support was approved but not yet present, limit it to the
  local content-free events and retention policy required here, document it,
  require deliberate user export, and subject it to the same privacy review as
  model requests.
- Include negative and abandoned cases in the result; downloads, stars, and
  positive anecdotes alone are not proof of recurring value.

## Data boundary

Never collect or transmit:

- document, selection, clarification-answer, proposed-revision, or model-response
  content;
- API keys, endpoint credentials, full local paths, recovery contents, or file
  names that may reveal project identity;
- Effective Agent Context source text;
- stable cross-product identity.

Use coarse event names, durations, counts, lens categories, explicit user ratings,
and separately consented interview material. Provide a way to inspect and disable
any local collection. Recordings require separate consent and are deleted within
30 days; de-identified notes and exported event summaries are deleted within 90
days after the final decision.

## Out of scope

- Adding another artifact lens, harness profile, renderer, theme, model provider,
  chat interface, template system, or autonomous agent.
- Changing evaluation thresholds, excluding failed users, or extending the window
  after seeing an unfavorable result.
- Treating GitHub stars, raw downloads, or launch speed as the North Star outcome.
- Collecting hidden telemetry, uploading content for “quality,” or requiring an
  account solely for measurement.
- Solving every issue discovered during observation. Record release-blocking
  safety defects immediately; turn other validated needs into later goals only
  after this decision.

## Completion evidence

1. The full pre-registered observation window and cohort are complete, or the
   report honestly records why recruitment was inconclusive and does not claim
   product validation.
2. Results report every success criterion and guardrail in
   [Post-Release Validation](../../PRODUCT.md#post-release-validation). A missing
   criterion prevents `continue`.
3. Every quantitative result includes its denominator, missing/unknown count, and
   collection method.
4. At least one analysis covers users who abandoned or rejected Review, not only
   successful sessions.
5. A privacy audit confirms that collected/exported records contain none of the
   prohibited content or identifiers above and follow the approved retention
   policy.
6. The project owner chooses and records exactly one outcome:
   - **continue** — thresholds support investing further in this direction;
   - **revise** — evidence identifies a bounded product-contract change and the
     existing later goals are rewritten before more feature work;
   - **stop** — recurring value is not established and expansion pauses.
7. Any code added solely for approved measurement passes focused privacy tests,
   `cargo fmt --all -- --check`, `cargo clippy --workspace --all-targets`, and
   `cargo test --release --workspace`; otherwise this remains an evidence-only
   goal and no code change is required.

## Stop and ask

Stop if the observation window, cohort, fixed criteria, or consented evidence
method in [Post-Release Validation](../../PRODUCT.md#post-release-validation) cannot
be followed. Do not infer success from repository popularity, an inadequate
cohort, or the project owner's own sessions alone.

## Final boundary

Only the decision from this goal should authorize a next wave such as more
harness profiles, optional renderer workers, downstream prompt execution,
commercial packaging, or additional platforms. Each requires a new bounded goal
rather than reopening this sequence implicitly.
