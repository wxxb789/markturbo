## Summary

MarkTurbo now keeps model credentials out of plaintext settings and requires one-use, scope-bound consent before sending document or Agent Skill content. This secures the in-process provider boundary retained by Goal 05 so Goal 06 can build Review without creating a second credential or consent path.

Completes [Goal 05A](https://github.com/wxxb789/markturbo/blob/codex/goal-05a-privacy/docs/goals/05a-protect-model-credentials-and-request-privacy.md).

## Security Design

- Persistent credentials use Windows Credential Manager with verified writes, pending markers, and cross-process coordination. Stale settings writers cannot restore migrated plaintext or revoked environment authorization.
- Credential and consent identities bind the provider wire format, normalized endpoint, operation, and frozen outbound scope. Vendor environment keys cannot move to a custom endpoint without explicit authorization.
- Remote endpoints require validated HTTPS. Verified loopback HTTP is disclosed as unencrypted and bypasses proxies. Redirects are disabled, and pooled clients remain separated by proxy policy.
- Translation sends exactly the disclosed frozen content. Agent Skill requests enumerate every included path, byte size, and inclusion reason; omitted supporting files produce a partial result.
- Diagnostics remain content-free, dependency response-body tracing is blocked from application logs, and repository plus release-binary scans reject credential or request-body sentinels.

## Validation

- `py -m uv run --project scripts scripts/mt.py check full`: PASS with random credential and request-body sentinels.
  - 223 tooling tests passed.
  - Formatting and Clippy passed; Clippy reported one non-failing test-style warning.
  - 931 release Rust tests passed, 0 failed, 12 ignored.
  - The release binary build and repository/binary privacy scan passed.
- Windows Credential Manager lifecycle acceptance: PASS, 4/4.
- DirectX privacy screenshot acceptance: PASS, 1/1; 1422x735, 149,971 bytes; SHA-256 `b92011c739cbe64e4fac932f3b18dbc07ad7e49797a020665849ae68fbdf828d`.
- No-transport ablation test: PASS, 1/1.
- Code review: harness-native fallback; all security, privacy, correctness, and proof findings were fixed, followed by a final review with no remaining actionable findings.
- Native acceptance is not claimed because Goal 05A has no native acceptance command. The explicit DirectX and Windows Credential Manager runs cover its platform-specific proof cases.

Detailed commands and limitations are recorded in the [Goal 05A completion report](https://github.com/wxxb789/markturbo/blob/codex/goal-05a-privacy/docs/goal-05a-completion.md).

## Boundaries

- Persistent credential storage remains Windows-only; other platforms use explicit environment or session-memory behavior.
- Model transport remains in-process under the Goal 05 decision.
- Semantic Review findings, questions, and revision behavior remain Goal 06 work.

## Post-Deploy Monitoring & Validation

- Re-run the random-sentinel full gate and DirectX privacy screenshot against the final release executable before publication.
- On a clean Windows 11 x64 profile, revalidate credential create, replace, test, delete, migration, cancellation, and secure-store failure behavior.
- Confirm custom-endpoint environment authorization remains identity-bound and does not return after revocation, restart, or concurrent settings writes.
- Inspect only content-free credential and transport diagnostics; do not enable payload-bearing logging or telemetry.
