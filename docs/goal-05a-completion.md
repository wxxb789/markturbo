# Goal 05A Completion Report

**Date:** 2026-09-06
**Goal:** [Goal 05A](goals/archive/05a-protect-model-credentials-and-request-privacy.md) -
Protect model credentials and request privacy.
**Status:** COMPLETE.

## Delivered boundary

- Reusable model configuration now owns provider wire format, model name, parsed
  endpoint identity, transport disclosure, and credential identity. Translation
  consumes that shared boundary without defining a second provider configuration.
- Persistent credentials use Windows Credential Manager on Windows. Session values
  remain in memory, environment values are eligible only for the exact authorized
  endpoint, and no newly entered credential is serialized into `settings.toml`.
- A verified non-secret pending marker surrounds each persistent replacement. If
  the write or its compensation cannot be verified, that endpoint remains
  quarantined across application restarts until an explicit delete or a verified
  replacement clears the marker. A global named Windows mutex serializes these
  operations across processes and logon sessions with a bounded wait that fails
  closed rather than freezing the UI.
- Credential targets use a lowercase SHA-256 digest of the complete exact endpoint
  identity. This preserves case-sensitive API-path distinctions even though Windows
  Credential Manager compares target names case-insensitively.
- Security-sensitive settings are persisted before their UI state changes. Settings
  writers coordinate across processes and reconcile the legacy credential field with
  the latest disk snapshot, so a stale process cannot restore migrated plaintext.
- Legacy plaintext migration is explicit. The secure write is read back before the
  latest settings snapshot removes the plaintext field; cancellation or any failure
  preserves the only copy. A pre-existing credential-shaped `model-base-url` is also
  preserved on disk to avoid silent data loss, but it is inactive, hidden from the
  editable field, rejected by endpoint parsing, and replaced only by an explicit Save.
- Every Translation request freezes its provider inputs before disclosure. Consent is
  one-use and bound to the Translation operation, exact endpoint identity, and exact
  selection, block, or document scope. A changed document invalidates pending consent.
- Remote endpoints require HTTPS. Only literal loopback IP addresses qualify as
  verified loopback; their HTTP transport is disclosed as unencrypted and bypasses
  proxies. Hostnames such as `localhost` remain remote, require HTTPS, and may use the
  configured system proxy. Redirects are disabled, so credentials and content cannot
  follow a redirect to another identity.
- Errors and debug output are content-free. Credential entry stays masked and is never
  prefilled after storage. The credential test sends only a fixed synthetic payload.
  A no-op tracing subscriber blocks dependency response-body traces from falling back
  into the application logger on Linux.
- Agent Skill outbound requests are represented by an immutable payload and matching
  normalized path, byte-size, inclusion-reason inventory, including partial-package
  state. The production `AgentSkillProviderAdapter` seam exposes those frozen entries
  only after its endpoint and a consumed Review authorization match the disclosure.
  Partial state is derived from explicit omitted-file records rather than a caller
  boolean. Review semantics remain deferred to Goal 06.
- The privacy gate scans Git index blobs by object ID, tracked worktree entries,
  non-ignored untracked entries, secret-bearing paths, and the packaged executable in
  UTF-8, UTF-16LE, and UTF-16BE forms. Tracked `.scratch` evidence is included;
  symlink text is scanned without following a target outside the repository,
  gitlink paths remain covered, overlapping candidate ranges are fully redacted,
  and candidate path text never enters `git cat-file` process arguments.

## Required proof cases

| Case | Evidence |
|---|---|
| New, session, and environment credentials do not enter settings | `settings::tests::current_settings_never_serialize_an_api_credential_field`; `credentials::tests::environment_and_session_credentials_never_write_the_secure_store_implicitly`; `resolved_credentials_never_enter_the_settings_file` |
| Migration ordering, cancellation/failure preservation, and concurrent settings | `credentials::tests::legacy_migration_clears_plaintext_only_after_verified_secure_write`; `goal_05a_credential_lifecycle_failed_migration_preserves_the_only_plaintext_copy`; `legacy_migration_preserves_settings_changed_while_secure_storage_runs`; `stale_process_cannot_restore_a_migrated_plaintext_credential`; `settings::tests::settings_writes_fail_closed_while_the_global_lock_is_busy`; `stale_security_state_is_not_written_when_the_settings_file_is_missing`; `malformed_current_settings_block_a_stale_security_write`; `views::settings_page::tests::goal_05a_credential_lifecycle_cancel_preserves_the_legacy_copy`; the explicit 4/4 lifecycle run below |
| Replace/delete affect only one identity | `credentials::tests::replacing_one_identity_leaves_every_other_identity_untouched`; `deleting_one_identity_leaves_every_other_identity_untouched` |
| Failed write compensation remains fail-closed | `credentials::tests::persistent_success_requires_a_verified_read_back`; `failed_compensation_quarantines_an_unverified_value_across_restart`; `replacing_a_quarantined_target_never_clears_its_existing_marker_early`; `failed_write_to_a_quarantined_target_preserves_the_target_and_marker`; `failed_verification_of_a_quarantined_target_preserves_the_quarantine`; `a_pending_marker_double_check_blocks_a_cross_process_write_race`; `failed_persistent_delete_preserves_the_session_override`; the explicit bounded global-lock proof below |
| Exact endpoint binding and WinCred case safety | `model::tests::port_path_and_wire_format_are_identity_boundaries`; `credential_targets_do_not_collide_under_windows_case_insensitive_matching`; `settings::tests::failed_persisted_update_keeps_the_published_security_state`; the explicit WinCred acceptance below |
| Endpoint, proxy, TLS, and redirect policy | `model::tests::invalid_endpoint_matrix_fails_closed_without_echoing_input`; `loopback_http_is_local_unencrypted_and_proxy_free`; `localhost_requires_https_and_remains_proxy_eligible`; `i18n::tests::local_https_status_discloses_encryption_and_disabled_proxy_in_both_languages`; `translate::tests::transport_builder_disables_redirects_and_keeps_certificate_validation`; `transport_clients_are_reused_within_but_not_across_proxy_policies`; `redirect_is_not_followed_to_a_second_server` |
| Local-only operations send zero requests | `views::workspace::tests::opening_and_scanning_with_model_configured_sends_no_request`; existing local document, Skill discovery, and Effective Agent Context paths have no request authorization or transport entry point |
| Displayed scope equals provider content | `translate::tests::bound_selection_disclosure_matches_the_exact_loopback_request_body` runs `TranslationRequest::prepare -> bind_request -> disclosure -> authorize -> execute` and compares the selection kind, disclosed byte count, and captured provider JSON while proving surrounding document text is absent |
| Consent cancellation and material changes | `views::workspace::tests::cancelling_translation_consent_sends_no_request`; `changing_the_document_invalidates_pending_translation_consent`; `approving_translation_consent_sends_one_frozen_request`; model authorization mismatch tests |
| Agent Skill inventory and partial state | `model::tests::agent_skill_request_keeps_disclosure_and_payload_exactly_aligned` drives the production `AgentSkillProviderAdapter` seam with a recording fake, proves mismatched authorization or adapter endpoint sends nothing, and compares every adapter path, byte size, reason, and byte payload with the displayed inventory; explicit omission records derive partial state; `agent_skill_request_rejects_unsafe_or_ambiguous_paths`; bilingual disclosure tests |
| Content remains inert and diagnostics remain content-free | the loopback request inspection includes instruction-like text and no tools; `translate::tests::errors_and_debug_output_exclude_credentials_and_request_or_response_bodies`; `main::tests::tracing_cannot_bridge_provider_response_bodies_into_application_logs`; credential redaction tests |
| Repository and packaged-file sentinel scan | `scripts.tests.test_privacy` covers index-only and worktree content, UTF-8/UTF-16, tracked `target` and `.scratch`, symlinks and gitlinks, fully overlapping values, secret-bearing paths without argv disclosure, and release-binary cases; the final `check full` ran with random `MARKTURBO_PRIVACY_SENTINEL` and `MARKTURBO_PRIVATE_REQUEST_SENTINEL` values |

The explicit DirectX screenshot proof renders the production Translation settings
page, its real masked credential controls, and an actual GPUI model-consent prompt.
Two equal-length but different credential and request-body sentinels produce
pixel-identical, nonblank images; the encoded PNG contains none of the four values.
The local evidence is `.scratch/goal-05a-settings-privacy.png`: 1422x735, 149,971
bytes, SHA-256
`b92011c739cbe64e4fac932f3b18dbc07ad7e49797a020665849ae68fbdf828d`.
The transport is in-process, so credentials and request bodies are never placed in
model-request process arguments; the privacy scanner separately proves that
secret-bearing repository paths are not placed in its own child-process arguments.

## Validation

- `uv run --project scripts scripts/mt.py check fast`: PASS; 223 tooling tests.
- `uv run --project scripts scripts/mt.py check full`: PASS with random synthetic
  credential and request-body sentinels; 223 tooling tests; formatting and Clippy;
  931 release Rust tests passed, 0 failed, 12 ignored; release binary build and
  repository/binary privacy scan passed.
- `cargo test -p mt-app goal_05a_credential_lifecycle -- --include-ignored --test-threads=1`:
  PASS; 4/4. One real Windows Credential Manager test covers migration success,
  case-distinct targets, replacement, session precedence, and deletion; a second
  Windows test proves bounded global-lock contention; the same run covers UI
  cancellation and fake secure-store failure without losing the only plaintext copy.
- `MARKTURBO_PRIVACY_SCREENSHOT=<path> cargo test -p mt-app views::settings_page::tests::rendered_privacy_surfaces_are_secret_invariant -- --ignored --exact --test-threads=1`:
  PASS; 1/1 DirectX-rendered screenshot proof, with the artifact described above.
- `cargo test -p mt-app --no-default-features translate::ablation_tests::measurement_build_reports_the_removed_transport_after_preparation -- --exact`:
  PASS; 1/1.
- Focused Goal 05A suites: model 13/13, credentials 20/20 plus 2 explicit ignored
  integration tests, transport 21/21, settings 27/27, settings UI 11/11 plus the
  explicit screenshot proof, i18n 14/14, consent 3/3, document translation 22/22,
  and privacy tooling 20/20.

An additional debug-profile `cargo test --workspace` run passed all functional tests
but exceeded the existing 120-second threshold in
`parses_a_100k_line_document_in_bounded_time` twice (`124.711s` and `120.249s`) on
this noisy machine. The required locked release-profile run passed that performance
test and the complete canonical gate.

No hash-bound native executable acceptance command is defined for Goal 05A, and none
is claimed here. Source tests, GPUI state tests, the explicit DirectX rendering proof,
loopback request inspection, the real Windows Credential Manager integration test,
and the packaged-binary privacy scan are the applicable evidence.
