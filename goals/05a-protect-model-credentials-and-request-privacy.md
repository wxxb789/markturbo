# Goal 05A — Protect model credentials and request privacy

## Objective

Before semantic Review is enabled, stop persisting model API keys as plaintext
application settings by using the primary platform's approved credential store
or an explicit environment/session-only fallback, separate reusable non-secret
model configuration from Translation-specific settings, require an informed
user action before any document or context content is sent, and verify that real
credentials and private user request bodies never appear in settings files,
recovery data, logs, process arguments, errors, screenshots, or committed
evaluation fixtures; synthetic sentinel fixtures remain allowed for testing.

## Why this goal was added during final review

The initial ordered goals protected text integrity and worker IPC but missed the
existing plain-text `translate_api_key` setting. Prompt and instruction documents
may contain sensitive product or repository information, so a public Review
feature needs both credential protection and a precise outbound-data contract
regardless of whether Goal 05 chose a worker or retained in-process transport.

## In scope

- Inventory every current path by which model credentials enter, persist, move
  through, or leave the application.
- Store persistent credentials in the supported platform credential facility,
  keyed by an application/provider/endpoint identity that cannot silently reuse a
  secret for a different host.
- Restrict vendor environment variables such as `OPENAI_API_KEY` and
  `ANTHROPIC_API_KEY` to their vendor-default endpoint identities unless the user
  explicitly authorizes that credential for a custom host; changing `base_url`
  must never redirect an ambient vendor key silently.
- Define cross-host redirect behavior so authentication cannot follow a response
  to a different endpoint identity.
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
- Define consent by endpoint identity and operation. The UI must show whether the
  endpoint is local or remote and exactly which selection, document, or resolved
  context sources are included before sending content outside the core process.
- Re-confirm when the endpoint identity or request scope materially changes; do
  not treat consent for one provider or one selected document as consent for an
  entire workspace.
- Make every model request user-initiated. Opening, previewing, indexing,
  discovering, or resolving context must not send content.
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
- Claiming that an HTTP local endpoint is encrypted; label its actual scheme and
  boundary honestly.

## Required proof cases

Automated tests must cover at least:

1. A newly entered persistent key is absent from serialized settings.
2. Environment and session-only keys never become persistent implicitly.
3. Successful legacy-key migration writes secure storage before removing
   plaintext; failed migration leaves the original recoverable and reports why.
4. Replacing and deleting a key affect only the selected provider/endpoint.
5. Switching endpoint host cannot silently reuse a host-bound credential; a
   vendor environment key is not sent to a custom host without explicit consent,
   and authentication does not follow a cross-host redirect.
6. Opening a document, scanning Skills, and resolving context send zero requests.
7. Selection consent sends only the displayed selection and required protocol
   framing, not the whole document or workspace.
8. Changing to document or Effective Context scope requires the corresponding
   explicit disclosure.
9. Logs, errors, process arguments, recovery records, and screenshots contain
   neither a sentinel API key nor a sentinel request body.
10. Document content that imitates system instructions, tool requests, protocol
    fields, or endpoint URLs remains inert data and cannot alter transport state.

## Completion evidence

- The primary-platform secure-store behavior and fallback policy are documented
  and approved by the project owner.
- A settings file written after configuration contains all required non-secret
  fields and no credential value.
- A migration acceptance run covers success, user cancellation, secure-store
  failure, replacement, and deletion without losing the only copy silently.
- A request-inspection test proves the displayed outbound scope equals the bytes
  supplied as user content to the provider adapter, excluding documented protocol
  framing.
- Repository and packaged-file scans using sentinel secrets find zero unintended
  occurrences.
- `cargo fmt --all -- --check`, `cargo clippy --workspace --all-targets`, focused
  credential/privacy tests, and `cargo test --release --workspace` pass; the pass
  count is recorded.

## Stop and ask

Stop if Goal 01 does not settle recovery and model-data policy, if the primary
platform has no acceptable secure credential facility, or if provider routing
cannot bind a credential to a stable endpoint identity. Ask whether to choose
environment-only credentials rather than preserving the existing plaintext
setting.

## Boundary for the next goal

This goal establishes secure model configuration and informed request transport.
Goal 06 consumes that boundary to implement non-mutating Review; it must not add
another credential store, consent model, or provider configuration path.
