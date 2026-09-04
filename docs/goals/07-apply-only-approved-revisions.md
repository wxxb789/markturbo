# Goal 07 — Apply only approved revisions

## Objective

Extend the accepted read-only Review from Goal 06 so a user can answer its
clarification questions, request a revision against the reviewed source
snapshot, inspect a local diff with a reason for every proposed change, accept or
reject individual hunks, and copy or save only the approved result; prove with
keyless tests that rejection is byte-identical, stale proposals cannot apply,
and one undo operation restores the pre-apply editor state.

## Product contract alignment

**Disposition:** Retained and revised on 2026-08-29.

Goal 07 is a document-only intermediate capability. It does not by itself
fulfill the public-quality context-aware promise: Goal 10 requires the
integrated Goal 07 revision workflow and Goal 08's explicitly selected,
disclosed Effective Agent Context in Review before release.

## Product invariant

The model may propose; only the user edits. No generated response is authority to
mutate a file, and no answer that materially affects the agent should remain
trapped in hidden Review state.

## Revision evaluation standard

Revision evaluation is deterministic and local to this goal. It does not use an
acceptance-rate target or reuse the Review usefulness threshold in `PRODUCT.md`.
For every revision-eligible case in immutable corpus version `goal-01-v1`, the
evaluation record must identify the corpus version and manifest digest, the
reviewed source snapshot, answered material questions, the displayed proposed
diff, each owner accept/reject decision, and an owner intent-preservation
judgment for the accepted output.

The revision workflow passes only when all of the following hold:

1. Every revision-eligible `goal-01-v1` case has the complete owner decision and
   intent-preservation record above.
2. Every accepted output is judged by the owner to preserve the artifact's
   intent.
3. No accepted output contains an intent-violating change defined by that
   case's annotation in `OWNER-ANNOTATIONS.md`.
4. Every answered material question is represented in the proposed artifact or
   visibly marked as intentionally omitted.
5. In every reject-all evaluation, the resulting editor bytes are identical to
   the reviewed source snapshot.

The report may describe the number of accepted hunks, but that number is not a
success measure.

## In scope

- Structured answers to clarification questions from the active Review.
- A clear distinction between unanswered, intentionally unspecified, and
  answered questions.
- Revision generation against the exact reviewed snapshot and selected artifact
  lens.
- A locally computed diff. Do not trust model-supplied line numbers or a claim
  that text is unchanged.
- A rationale tied to each proposed hunk or to an explicitly grouped set of
  inseparable hunks.
- Accept/reject controls per hunk plus accept-all and reject-all actions that do
  not hide the individual changes.
- A final preview assembled from the original snapshot and accepted hunks.
  Proposed source and rationale render as inert text: no HTML, MDX, script,
  command link, tool call, or WebView content executes from Review state.
- One explicit Apply action; before it, editor text and dirty state remain
  unchanged. Applying an executable-content change to a trusted MDX/HTML document
  resets it to Restricted and requires a new explicit trust decision for the new
  source revision.
- One editor undo transaction for one Apply action, even when several hunks were
  accepted.
- Copy approved text without saving, and Save/Save As through the established
  safe filesystem paths.
- Source-revision validation immediately before Apply. A stale proposal remains
  inspectable but cannot alter the newer editor; rerun or deliberate rebase is
  explicit.
- Validation of UTF-8 boundaries, non-overlapping edits, bounded output size, and
  malformed or incomplete proposals.
- Ensure each answered material question is either represented in the proposed
  artifact or visibly marked as intentionally not written into it.
- Treat user-authored answers as recoverable dirty state under Goal 02 until they
  are incorporated, explicitly discarded, or exported; closing or interruption
  must not silently lose a clarification the user has typed.
- Evaluation against every artifact in `evaluation/goal-01/CORPUS.md` whose
  annotation calls for a revision.

## Out of scope

- Automatically running the revised prompt or judging downstream model output.
- Autonomous multi-step editing, background agents, batch rewrite of a folder,
  or applying changes without the active user's confirmation.
- Effective Agent Context resolution, source selection, or context inclusion in
  Review; Goal 08 owns those capabilities and their disclosed integration.
- Git staging, commits, version-control history, collaborative review, or cloud
  document history.
- Rich-text editing, a generic chat conversation, prompt scores, or templates.
- Visual redesign outside the Review and diff states; Goal 09 owns hierarchy and
  final presentation polish.

## Required proof cases

Automated tests must cover at least:

1. Reject all leaves editor bytes and dirty state unchanged.
2. Accepting one of several hunks applies exactly that hunk.
3. Accept all produces exactly the displayed final preview.
4. Apply creates one undo step that restores the exact original text.
5. Editing before Apply marks the proposal stale and blocks mutation.
6. Overlapping, out-of-range, non-UTF-8-boundary, oversized, or malformed edits
   are rejected as a complete proposal.
7. CJK, emoji, CRLF-loaded files, frontmatter, fenced code, and links display and
   apply without offset corruption.
8. Save writes through conflict protection; an external write between proposal
   and save is not overwritten silently.
9. Copy exports only the approved result and does not mark the document saved.
10. A model failure after questions are answered preserves both the editor and
    the user's answers long enough to retry or dismiss deliberately.
11. Closing, crashing, or restarting with user-authored answers follows Goal 02's
    recovery/discard contract; restored answers remain bound to their reviewed
    source revision and cannot apply to different text.
12. Generated HTML/MDX remains inert in Review, and applying an executable change
    to a Trusted document revokes trust until the user approves that new revision.

## Completion evidence

- Every revision-eligible artifact in `evaluation/goal-01/CORPUS.md` has a
  recorded proposed diff, human accept/reject decisions, and an
  intent-preservation judgment that passes the Revision evaluation standard.
- Project-owner review confirms that at least one real artifact became clearer
  because a question changed or exposed a decision, not merely because prose was
  expanded.
- The UI never mutates source before Apply and visibly identifies stale results.
- Focused diff/property tests include randomized valid Unicode edits or an
  equivalent exhaustive boundary check.
- `cargo fmt --all -- --check`, `cargo clippy --workspace --all-targets`, focused
  tests, and `cargo test --release --workspace` pass; the pass count is recorded.

## Stop and ask

Stop if the editor cannot represent a multi-hunk Apply as one reliable undo
transaction, if the provider cannot return a proposal that can be validated
without trusting opaque model instructions, or if Goal 06 did not meet its
evaluation gate. Do not substitute unreviewed whole-document replacement or
multiple uncoordinated writes for the approved-diff contract.

## Boundary for the next goal

This goal owns revision review and application for the active artifact only.
Goal 08 next makes selected Effective Agent Context visible, disclosed, and
available to Review. Goal 10 cannot ship the public-quality context-aware promise
until both workflows operate together.
