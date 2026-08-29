# Goal 01 Evaluation Corpus

**Corpus version:** `goal-01-v1`

**Status:** Owner-approved on 2026-08-29. The owner approved a one-time,
in-place migration exception on 2026-08-29 to make `goal-01-v1`
self-contained. The exception is recorded in `MIGRATION-EXCEPTION.md`.
Immutability resumed when this migrated manifest was approved. Project-owner
annotations are recorded in `OWNER-ANNOTATIONS.md` and the fixed scoring
registry is `SCORING.md`.

This manifest defines 12 owner-approved, rights-cleared evaluation artifacts for
the four first Review lenses. Owner-local material is intentionally excluded.
Evaluation must use the exact bytes fixed by `MANIFEST.sha256` and record both
the corpus version and manifest digest alongside each Review result.

## Immutability And Migration Record

`goal-01-v1` received one owner-approved in-place migration exception, detailed
in `MIGRATION-EXCEPTION.md`. It replaced references to mutable canonical files
with version-local snapshots and added the pre-registered scoring registry. The
original approved commit and manifest-file digest are retained there.

After that migration, `goal-01-v1` is immutable. `MANIFEST.sha256` fixes the
bytes of this file, `.gitattributes`, `OWNER-ANNOTATIONS.md`,
`THIRD-PARTY-NOTICES.md`, `MIGRATION-EXCEPTION.md`, `SCORING.md`, the Apache-2.0
license snapshot, the four task prompts, the four goal snapshots, the two
instruction snapshots, and every regular file in the two Agent Skill
snapshots. Its entries are all paths inside `evaluation/goal-01/`.

Any later artifact, annotation, notice, provenance, scoring item, or other
corpus metadata change requires a new owner-approved corpus version and
manifest. Never update an approved version in place. Hash-mismatched content is
ineligible as threshold evidence.

From the repository root, `sha256sum -c evaluation/goal-01/MANIFEST.sha256`
verifies the approved bytes without contacting a network service.

## Corpus

| ID | Lens | Artifact | Provenance | Retention and redistribution |
|---|---|---|---|---|
| TP-01 | Task prompt | `task-prompts/01-upgrade-gpui-without-regression.md` | Anonymized from repository commit `a29308f`; no personal metadata retained. | Project-authored derivative; owner-approved Apache-2.0 redistribution. |
| TP-02 | Task prompt | `task-prompts/02-keep-web-preview-in-one-window.md` | Anonymized from repository commit `b56881d`; no personal metadata retained. | Project-authored derivative; owner-approved Apache-2.0 redistribution. |
| TP-03 | Task prompt | `task-prompts/03-diagnose-duplicate-git-crates.md` | Anonymized from repository commit `5ebc43b` and the checked-in solution note. | Project-authored derivative; owner-approved Apache-2.0 redistribution. |
| TP-04 | Task prompt | `task-prompts/04-measure-release-profile-on-quiet-host.md` | Anonymized from repository commit `11d6c67` and `docs/TODO.md`. | Project-authored derivative; owner-approved Apache-2.0 redistribution. |
| SP-01 | Specification / plan | `snapshots/goals/02-guarantee-user-text-safety.md` | Repository-authored product goal introduced in commit `f3eff0f`; snapshot created by the owner-approved migration. | Owner-approved Apache-2.0 redistribution. |
| SP-02 | Specification / plan | `snapshots/goals/03-create-first-use-document-flow.md` | Repository-authored product goal introduced in commit `f3eff0f`; snapshot created by the owner-approved migration. | Owner-approved Apache-2.0 redistribution. |
| SP-03 | Specification / plan | `snapshots/goals/05a-protect-model-credentials-and-request-privacy.md` | Repository-authored product goal introduced in commit `f3eff0f`; snapshot created by the owner-approved migration. | Owner-approved Apache-2.0 redistribution. |
| SP-04 | Specification / plan | `snapshots/goals/06-deliver-read-only-intent-review.md` | Repository-authored product goal introduced in commit `f3eff0f`; snapshot created by the owner-approved migration. | Owner-approved Apache-2.0 redistribution. |
| AI-01 | Agent instructions | `snapshots/agent-instructions/repository-AGENTS.md` | Repository instruction snapshot created by the owner-approved migration. | Owner-approved Apache-2.0 redistribution. |
| AI-02 | Agent instructions | `snapshots/agent-instructions/sample-workspace-AGENTS.md` | Controlled sample-workspace instruction snapshot created by the owner-approved migration. | Owner-approved Apache-2.0 redistribution. |
| AS-01 | Agent Skill | `snapshots/skills/gpui/` | Vendored from `longbridge/gpui-component`, pinned by the `gpui` hash in `skills-lock.json`; complete directory snapshot created by the owner-approved migration. | Upstream Apache-2.0; retain attribution and license notices; owner-approved redistribution. |
| AS-02 | Agent Skill | `snapshots/skills/gpui-component/` | Vendored from `longbridge/gpui-component`, pinned by the `gpui-component` hash in `skills-lock.json`; complete directory snapshot created by the owner-approved migration. | Upstream Apache-2.0; retain attribution and license notices; owner-approved redistribution. |

The upstream attribution retained for AS-01 and AS-02 is recorded in
`THIRD-PARTY-NOTICES.md`; `snapshots/licenses/Apache-2.0.txt` fixes the
Apache-2.0 terms that accompany this corpus version.

## Handling Rules

- Do not add owner-local prompts, private repositories, screenshots, recordings,
  API responses, or evaluation notes to this directory.
- Do not store names, email addresses, full local paths, credentials, endpoint
  secrets, or customer/project identifiers in provenance.
- Run a secret scan before approving the set and before every release that ships
  the corpus.
- Treat the two Agent Skills as directories, including their referenced support
  files, rather than reviewing only `SKILL.md` in isolation.
- When a source artifact or corpus metadata changes, create and obtain owner
  approval for a new corpus version. Do not silently compare results from
  different versions or manifests. The migration exception recorded in this
  version is the sole exception to this rule.
- Model responses and owner judgments remain owner-local unless the owner grants
  a separate redistribution right.

## Approval Record

On 2026-08-29 the project owner confirmed all of the following:

1. each artifact represents a real or appropriately anonymized case;
2. the project may retain every checked-in artifact;
3. the project may redistribute every checked-in artifact under the named terms;
4. the files contain no secret, private, or third-party-confidential data;
5. the project-owner annotations in `OWNER-ANNOTATIONS.md` express the
   evaluation intent used by this contract.
