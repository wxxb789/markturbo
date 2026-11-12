# Core Skill leaf test ablation

## Baseline

- Production file: `crates/mt-core/src/agent_artifacts/skill.rs`.
- Original SHA-256: `e80ab00373492c996cddd82668502a7edb1731304d950dafdf2273d837fdc2bd`.
- Candidate: `fixtures::discovery_does_not_descend_into_a_skill`.
- Surviving detector: `discovery::a_skill_directory_is_a_leaf`.
- The fixture candidate has no nested `SKILL.md`; the survivor deliberately
  creates `skills/outer/inner/SKILL.md`.
- Executed the existing CI-profile `fixtures` and `discovery` test binaries:
  17/17 and 10/10 passed, exits 0. No source mutation was active.

## Experiment status

The next temporary mutation removes only the leaf `return` after
`out.insert(skill)` in `walk`. Compare the full 27 tests with the same selection
minus the candidate. Require the same surviving leaf detector to fail while
the candidate remains green. Restore the exact production SHA-256 before
deleting the ineffective candidate or running final validation.

The temporary leaf-return mutation has been restored. Verify the original
production SHA-256 and green tests before integrating the test-only ablation.

## Measured mutant with the original test selection

Command:
`cargo test --locked --profile ci -p mt-core --all-features --test fixtures --test discovery --no-fail-fast -- --nocapture`

- Exit 101, as expected for the deliberate fault.
- Discovery: 9 passed, 1 failed. The only failure was
  `a_skill_directory_is_a_leaf`, with `nested skill must be shadowed`.
- Fixtures: 17 passed, including `discovery_does_not_descend_into_a_skill`.
- The candidate did not detect the very behavior named in its title.

## Measured mutant with the candidate excluded

Ran the same two mutant executables with
`--skip discovery_does_not_descend_into_a_skill` (no recompilation or other change).

- Discovery: exit 101; 9 passed and the same single leaf detector failed.
- Fixtures: exit 0; 16 passed, 1 filtered out.
- Removing the candidate did not change the failure set. The survivor has a
  nested Skill input that the candidate lacks, so the latter supplies no
  independent protection for leaf traversal.
- Decision: remove only the ineffective fixture test after restoration proof;
  retain the nested-Skill regression and all other fixture/integration tests.

## Restoration and integration

- The restored production SHA-256 exactly matches the recorded original.
- Re-ran the identical Cargo command/configuration after restoration: exit 0,
  discovery 10/10 and fixtures 17/17 passed.
- Removed only the ineffective fixture test. The final check
  `cargo test --locked --profile ci -p mt-core --all-features --test fixtures --test discovery`
  exited 0: discovery 10/10 and fixtures 16/16 passed. No production mutation
  remains; the only lasting Rust change from this experiment is the deletion
  of the ineffective fixture test.
