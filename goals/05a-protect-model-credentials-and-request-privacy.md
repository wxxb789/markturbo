# Goal 05A — Protect model credentials and request privacy

## Objective

Before semantic Review is enabled, stop persisting model API keys as plaintext
application settings by using Windows Credential Manager on Windows 11 x64 or
an explicit environment/session-only fallback, separate reusable non-secret
model configuration from Translation-specific settings, require an informed
user action before any document or context content is sent, and verify that real
credentials and private user request bodies never appear in settings files,
recovery data, logs, process arguments, errors, screenshots, metrics, or
committed evaluation fixtures; synthetic sentinel fixtures remain allowed for
testing.

## Why this goal was added during final review

The initial ordered goals protected text integrity and worker IPC but missed the
existing plain-text `translate_api_key` setting. Prompt and instruction documents
may contain sensitive product or repository information, so a public Review
feature needs both credential protection and a precise outbound-data contract
regardless of whether Goal 05 chose a worker or retained in-process transport.

## Product contract alignment

**Disposition:** Retained and revised on 2026-08-29.

This goal implements `PRODUCT.md`'s local-first model-data policy on Windows 11
x64: configured credentials are confidential, model traffic is always explicit,
and local editing, rendering, search, Skill discovery, context resolution, and
recovery send no document or context content. It preserves configured endpoint
and Translation support without adding providers or background telemetry.

## In scope

- Inventory every current path by which model credentials enter, persist, move
  through, or leave the application.
- On Windows 11 x64, store persistent credentials in Windows Credential Manager.
  The credential identity contains the application, provider wire format,
  scheme, normalized host, effective port, and normalized API base path, so a
  secret for one identity cannot silently be reused by another.
- Restrict vendor environment variables such as `OPENAI_API_KEY` and
  `ANTHROPIC_API_KEY` to their vendor-default endpoint identities unless the user
  explicitly authorizes that credential for a custom host; changing `base_url`
  must never redirect an ambient vendor key silently.
- Define cross-host redirect behavior so authentication cannot follow a response
  to a different endpoint identity.
- Parse every API base URL before use and reject userinfo, query, and fragment
  components. A non-loopback endpoint must use HTTPS with certificate
  validation. HTTP is allowed only for a verified loopback endpoint and is
  disclosed as unencrypted with no proxy. Never automatically follow a redirect;
  any redirect response is a diagnostic.
- When a supported secure store is unavailable, offer only explicitly described
  environment or session-memory behavior; never silently fall back to plaintext
  persistence.
- Migrate an existing plaintext key only after explicit user approval and a
  verified secure write. Remove the plaintext value only after that succeeds;
  preserve it and report the failure if migration cannot complete.
- Let the user replace, test, and delete a stored credential without displaying
  its value after storage.
- Generalize reusable non-secret configuration—wire format, model, base URL, and
  endpoint identity—so Review and Translation do not maintain divergent provider
  settings. Keep operation-specific settings, such as translation target
  language, separate.
- Define consent by endpoint identity and operation. Before sending, the UI must
  disclose whether the endpoint is local or remote, the provider wire format and
  normalized endpoint identity, whether the scope is a selection, document, or
  document plus named Effective Agent Context sources, and that protocol framing
  plus the disclosed source content cross the boundary. Consent applies only to
  that operation, endpoint identity, and displayed content scope.
- Treat an Agent Skill package as a first-class outbound scope. Before sending,
  enumerate every included package file by normalized relative path, byte size,
  and inclusion reason. A file absent from that list must not be sent. If a
  normally in-scope supporting file is omitted, label the Review result partial.
- Re-confirm when the endpoint identity or request scope materially changes; do
  not treat consent for one provider or one selected document as consent for an
  entire workspace.
- Make every model request user-initiated. Opening, previewing, indexing,
  discovering, or resolving context must not send content.
- Do not make background model requests, upload content for training, emit
  content-bearing telemetry, or perform hidden workspace scans. Content may not
  be sent for crash reporting, evaluation, or product improvement.
- Treat document text as untrusted request data: it cannot alter endpoint or
  credential configuration, request tools, start another operation, or bypass
  response validation merely by containing instructions.
- Redact secrets and request bodies from transport errors and diagnostic logs
  while retaining enough endpoint/provider information to troubleshoot safely.
- Use fake credential-store and transport implementations for keyless tests.

## Out of scope

- Accounts, cloud secret synchronization, organization policy, billing, or a
  hosted markturbo proxy.
- Bundling a local model or downloading model weights.
- Choosing worker versus in-process transport; Goal 05 owns that measured
  architecture decision.
- Defining Review findings, questions, or revision behavior; Goals 06 and 07 own
  semantic product behavior.
- Sending filesystem contents for telemetry, evaluation, crash reporting, or
  background quality improvement.
- Permitting HTTP except to a verified loopback endpoint, claiming that it is
  encrypted, or following a redirect automatically.

## Required proof cases

Automated tests must cover at least:

1. A newly entered persistent key is absent from serialized settings.
2. Environment and session-only keys never become persistent implicitly.
3. Successful legacy-key migration writes secure storage before removing
   plaintext; failed migration leaves the original recoverable and reports why.
4. Replacing and deleting a key affect only the selected provider/endpoint.
5. Switching endpoint host cannot silently reuse a host-bound credential; a
   vendor environment key is not sent to a custom host without explicit consent,
   authentication does not follow any redirect, base URLs with userinfo, query,
   or fragments are rejected, non-loopback HTTP is rejected, and a verified
   loopback HTTP disclosure says unencrypted with no proxy.
6. Opening a document, scanning Skills, and resolving context send zero requests.
7. Selection consent sends only the displayed selection and required protocol
   framing, not the whole document or workspace.
8. Changing to document or Effective Context scope requires the corresponding
   explicit disclosure.
9. Agent Skill package disclosure lists each normalized relative path, byte size,
   and inclusion reason; request inspection proves that exactly and only those
   listed files are supplied to the provider adapter, and an omitted supporting
   file makes the result partial.
10. Logs, errors, process arguments, recovery records, and screenshots contain
   neither a sentinel API key nor a sentinel request body.
11. Document content that imitates system instructions, tool requests, protocol
    fields, or endpoint URLs remains inert data and cannot alter transport state.

## Completion evidence

- The Windows 11 x64 Credential Manager behavior and environment/session-only
  fallback policy are documented and approved by the project owner.
- A settings file written after configuration contains all required non-secret
  fields and no credential value.
- A migration acceptance run covers success, user cancellation, secure-store
  failure, replacement, and deletion without losing the only copy silently.
- A request-inspection test proves the displayed outbound scope equals exactly
  the bytes supplied as user content to the provider adapter, excluding
  documented protocol framing. For an Agent Skill package, it proves the
  displayed path, byte-size, and inclusion-reason inventory exactly matches the
  included files and that no unlisted file crosses the boundary.
- Repository and packaged-file scans using sentinel secrets find zero unintended
  occurrences.
- `cargo fmt --all -- --check`, `cargo clippy --workspace --all-targets`, focused
  credential/privacy tests, and `cargo test --release --workspace` pass; the pass
  count is recorded.

## Stop and ask

Stop if Windows Credential Manager cannot bind a credential to the approved
application/provider/endpoint identity, or if provider routing cannot preserve
that identity across requests and redirects. Use only the approved environment
or session-memory fallback; never preserve the existing plaintext setting.

## Boundary for the next goal

This goal establishes secure model configuration and informed request transport.
Goal 06 consumes that boundary to implement non-mutating Review; it must not add
another credential store, consent model, or provider configuration path.
