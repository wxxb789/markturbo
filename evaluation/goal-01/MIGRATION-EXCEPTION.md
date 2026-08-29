# Goal 01 v1 Migration Exception

**Corpus version:** `goal-01-v1`

## Owner Approval

On 2026-08-29, the project owner explicitly approved one in-place migration
exception for `goal-01-v1`. The exception addresses the corpus snapshot and
fixed high-impact denominator review feedback without creating a v2 corpus.

The original owner-approved corpus commit was
`03d5b9e3ae99e10db28bf8ce93c572d19b492ece`. The SHA-256 digest of its original
`evaluation/goal-01/MANIFEST.sha256` file was
`c3deed707528c84107c55061ac069830b489e481811aac86afa9c10490bc6a8c`.

## Authorized Changes

This one-time exception authorizes only these changes to the approved v1 corpus:

1. replace mutable canonical goal, instruction, Skill, and license references
   with complete version-local snapshots under `snapshots/`, preserving their
   exact bytes with version-local Git attributes;
2. add `SCORING.md`, which atomizes the owner annotations into a fixed
   high-impact denominator and required hit count;
3. regenerate `MANIFEST.sha256` to cover the migrated, self-contained corpus;
4. update corpus metadata and the product contract to record this exception.

The source artifact bytes copied into `snapshots/` are the bytes approved for
v1 at the time of this migration. No task prompt, owner annotation meaning,
evaluation configuration, rights record, or evaluation threshold was otherwise
changed.

## Resumed Immutability

Immutability resumes after this migration. Any subsequent artifact, annotation,
notice, provenance, scoring item, manifest coverage, or other corpus metadata
change requires a new owner-approved corpus version. `goal-01-v1` must not be
updated in place again.
