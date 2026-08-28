# Goal 11 — Validate recurring user value

## Objective

Within the observation window, representative-user count, and thresholds approved
in Goal 01, determine whether the primary-platform release creates recurring
value: collect privacy-compliant activation, Review usefulness, approved-revision,
trust, and repeat-use evidence without collecting document or model content;
produce one owner-approved continue, revise, or stop decision whose cited evidence
meets the pre-registered standard, and add no new product capability merely to
improve the measurement.

## Why shipping is not the final outcome

A successful build and installer prove delivery. They do not prove that Review
clarifies thinking, that users return, or that the product is more useful than an
ordinary editor or a one-shot model rewrite. This goal closes that gap before the
roadmap expands again.

## In scope

- Use the target user, observation window, cohort/evidence count, and acceptance
  thresholds fixed in Goal 01. Do not choose them after seeing results.
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
- Measure repeat use in the separate time interval defined by Goal 01 rather than
  counting several actions in one session as retention.
- Record safety and trust outcomes: abandoned setup, provider/credential failure,
  stale-result blocks, recovery use, accidental-loss reports, privacy concerns,
  misleading findings, reverted accepted edits, and crashes.
- Where users can assess it honestly, record whether the revised artifact improved
  the subsequent agent interaction; keep “unknown” rather than forcing a rating.
- Segment evidence by artifact lens and endpoint type only when the sample supports
  the distinction; do not draw subgroup conclusions from isolated examples.
- Use the evidence mechanism approved in Goal 01:
  - moderated/recorded-with-consent sessions;
  - interviews and user-provided outcome reports;
  - opt-in, content-free event counters;
  - local metrics exported deliberately by the user;
  - or an approved combination.
- If minimal measurement support was approved but not yet present, limit it to the
  events and retention policy required here, document it, and subject it to the
  same privacy review as model requests.
- Include negative and abandoned cases in the result; downloads, stars, and
  positive anecdotes alone are not proof of recurring value.

## Data boundary

Never collect or transmit:

- document, selection, clarification-answer, proposed-revision, or model-response
  content;
- API keys, endpoint credentials, full local paths, recovery contents, or file
  names that may reveal project identity;
- Effective Agent Context source text;
- stable cross-product identity not explicitly approved in Goal 01.

Use coarse event names, durations, counts, lens categories, explicit user ratings,
and separately consented interview material. Provide a way to inspect and disable
any in-product collection, and honor deletion/retention policy.

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

1. The full pre-registered observation window and evidence count are complete, or
   the report honestly records why recruitment failed and does not claim product
   validation.
2. Activation, useful-question, accepted/rejected-revision, repeat-use, trust, and
   downstream-outcome evidence are reported against Goal 01's thresholds.
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

Stop if Goal 01 lacks a pre-registered observation window, representative-user
count, success thresholds, or lawful/consensual evidence method. Ask for those
inputs before collecting data; do not infer success from repository popularity or
from the project owner's own sessions alone.

## Final boundary

Only the decision from this goal should authorize a next wave such as more
harness profiles, optional renderer workers, downstream prompt execution,
commercial packaging, or additional platforms. Each requires a new bounded goal
rather than reopening this sequence implicitly.
