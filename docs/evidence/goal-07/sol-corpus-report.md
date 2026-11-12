# Goal 07 Corpus Capture Report

Date: 2026-09-20

## Scope

This run generated private `markturbo-goal-07-revision-capture-v2` inputs for
all 12 artifacts in immutable corpus `goal-01-v1` and validated them with the
existing release evaluator. The work was performed inline in the current Sol
task after the parent reported the Luna transport as terminal. Terra was not
used.

This report records machine-validation facts only. It does not declare corpus
eligibility, make owner accept/reject decisions, judge intent preservation, or
claim that any proposed artifact is clearer. No owner composition files were
provided to the evaluator.

## Corpus Identity

- Manifest: `evaluation/goal-01/MANIFEST.sha256`
- Manifest SHA-256: `290a0421753974dabb0cef1552f158cba4ddf2cafd3d91f0ea256d406f561f7c`
- Manifest entries verified against repository bytes: 45
- Manifest mismatches: 0
- Owner annotations SHA-256: `502a905db10be764d49344f3528eff238d5813d47a330bee628b61060ccec0ae`

`sha256sum` was unavailable in this PowerShell environment. The same manifest
check was performed with `Get-FileHash -Algorithm SHA256` over every parsed
manifest entry.

## Capture Inventory

`artifact_sha256` is the evaluator's deterministic aggregate over the sorted
artifact file metadata. Each capture also carries the manifest-listed SHA-256
and exact byte count for every individual corpus file.

| Capture | ID | Lens | Files | Artifact bytes | artifact_sha256 |
|---|---|---|---:|---:|---|
| `captures/ai-01.json` | AI-01 | Agent instructions | 1 | 3291 | `d2cfc86c0e5cd695b5bc11e1fa0d0ea336a28f57567815988dfbee22ae4ef725` |
| `captures/ai-02.json` | AI-02 | Agent instructions | 1 | 1082 | `d862bf86c7e9e8e6793e0406c488ea37b182f08f075c8847144da9646bc96484` |
| `captures/as-01.json` | AS-01 | Agent Skill | 23 | 201410 | `7f4da57edf4797c9a965efdfd9fe687350c28d462dd728f92dc141aa7146b2b1` |
| `captures/as-02.json` | AS-02 | Agent Skill | 5 | 116555 | `a8bf153ef556cf514e9255476acfdc9364164d3425224480b96e7bdd46793fbc` |
| `captures/sp-01.json` | SP-01 | Specification / plan | 1 | 7714 | `bd94f5b4e3f1d77163e4b103918fb0e75d9004a783d469edd5806ead46b6e2c1` |
| `captures/sp-02.json` | SP-02 | Specification / plan | 1 | 4813 | `c4ad80b8240cdc7986fa46c54b627bb4ac43dced9934e5e8c98d28bfb53b8608` |
| `captures/sp-03.json` | SP-03 | Specification / plan | 1 | 9331 | `d5a2e6ac96ebbb7d31130ca014b6bbdd5364ae910c423f318277632d33e6d88b` |
| `captures/sp-04.json` | SP-04 | Specification / plan | 1 | 9014 | `6dc46d0821d27572dd4fbd857525c5e6437b446820b2885dfdfab19ee170615d` |
| `captures/tp-01.json` | TP-01 | Task prompt | 1 | 626 | `302d3e24d3004953a7b4dabd9e8b916585c574f98c752044cee5aa06d24f4f08` |
| `captures/tp-02.json` | TP-02 | Task prompt | 1 | 476 | `0a2aef34caa32d502d602fe0c0269088053616d7313c51a1793026a3721d8c41` |
| `captures/tp-03.json` | TP-03 | Task prompt | 1 | 573 | `bc19915c16461dcdee484a4cffc76c183b5fe9bedec214901b08e7c2f7a9ae58` |
| `captures/tp-04.json` | TP-04 | Task prompt | 1 | 441 | `40e155b251959f1840d23c030625383b8167b42d9534d790c9b0ad7c68bc670a` |

Every capture contains:

- a production-valid `ReviewRequest` bound to the exact document or complete
  Agent Skill package;
- a production-valid `review-v1` output;
- one explicit `answered` clarification state;
- one `revision-v1` change group whose coverage marks that answer as
  `represented` by local change ID 0;
- an append-only candidate clarification grounded in the artifact and its
  owner annotation.

One concrete answered-question-driven candidate is TP-03: its answer selects
the exact Cargo source normalization, and the proposed text distinguishes
`git = "URL"` from `git = "URL", rev = "..."`, requires one shared source
selector, and leaves the resolved revision to `Cargo.lock`. This is a proposal
for later owner evaluation, not an owner-approved change.

## Evaluator Run

Runner:

- Path: `target/release/markturbo-goal07-evaluate.exe`
- SHA-256: `c77587986a2d14861c8026da883249c3a4ca3ee5a709477b48de7a3dee5e4768`

Command form used:

```powershell
$runner = 'target\release\markturbo-goal07-evaluate.exe'
$runnerSha = (Get-FileHash -Algorithm SHA256 -LiteralPath $runner).Hash.ToLowerInvariant()
$inputs = Get-ChildItem '.scratch\goal-07\captures\*.json' | Sort-Object Name
$args = @()
foreach ($input in $inputs) { $args += '--input'; $args += $input.FullName }
$args += @(
  '--receipt', '.scratch\goal-07\luna-machine-receipt.json',
  '--runner-sha256', $runnerSha,
  '--expect-runner-sha256', $runnerSha
)
& $runner @args
```

Result:

- Exit code: 0
- Evaluator output: `GOAL07_MACHINE_RECEIPT_SHA256=726a366ebd876ded19de9cd5a7dcb390e4ccad1859a138bbbc8251e7d5591b65`
- Receipt file SHA-256: `726a366ebd876ded19de9cd5a7dcb390e4ccad1859a138bbbc8251e7d5591b65`
- Receipt schema: `markturbo-goal-07-machine-receipt-v2`
- Cases decoded and validated: 12
- Cases with one answered material question represented by a change: 12
- Cases whose reject-all result matches source SHA-256 and byte count: 12
- Cases with owner composition: 0
- Cases with `approved_output.status = not_composed`: 12

## Blockers

No blocker remains for capture generation or machine validation. Goal 07's
owner evaluation remains intentionally incomplete until the project owner
supplies eligibility, per-change accept/reject decisions, and intent-preservation
judgments. Those judgments were not inferred or fabricated in this run.
