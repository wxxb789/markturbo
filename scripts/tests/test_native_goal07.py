"""Unit tests for the Goal 07 native harness without launching a UI."""

from __future__ import annotations

import argparse
import hashlib
import inspect
import json
import tempfile
import unittest
import urllib.request
import urllib.error
from pathlib import Path
from unittest import mock

from scripts.markturbo_tools.native import goal07 as HARNESS
from scripts.markturbo_tools.native import runtime


HASH = "a" * 64


def fingerprint(value: bytes) -> dict[str, int | str]:
    return {"byte_count": len(value), "sha256": hashlib.sha256(value).hexdigest()}


def process_context() -> dict[str, int | str]:
    return {"session_id": 7, "integrity_rid": 0x2000, "integrity": "medium"}


def runtime_scan() -> dict[str, int | bool]:
    return {
        "files_scanned": 3,
        "app_logs_scanned": 1,
        "config_files_scanned": 1,
        "utf8_sentinel_absent": True,
        "utf16le_sentinel_absent": True,
        "ephemeral_credential_utf8_absent": True,
        "ephemeral_credential_utf16le_absent": True,
        "answer_sentinel_utf8_absent": True,
        "answer_sentinel_utf16le_absent": True,
        "raw_response_sentinel_utf8_absent": True,
        "raw_response_sentinel_utf16le_absent": True,
    }


def environment() -> dict[str, int | str]:
    return {
        "platform": "Windows 11",
        "windows_major": 10,
        "windows_minor": 0,
        "windows_build": 22631,
        "architecture": "x86_64",
        "native_machine_code": 0x8664,
        "python_pointer_bits": 64,
        "wts_state": "WTSActive",
        "active_console_session_id": 7,
        "harness_is_console_session": True,
        "input_desktop": "Default",
        "thread_desktop": "Default",
        "harness_process": process_context(),
    }


def common(case_id: str) -> dict:
    editor_same = fingerprint(HARNESS.EDITOR_SOURCE_BYTES)
    disk_same = fingerprint(HARNESS.SOURCE_BYTES)
    observations = {
        "flow": HARNESS.CASE_FLOWS[case_id],
        "process_context": process_context(),
        "foreground_verified": True,
        "loopback_provider": True,
        "server_loopback": True,
        "server_deterministic": True,
        "provider_request_count": 2,
        "provider_review_count": 1,
        "provider_revision_count": 1,
        "provider_paths_exact": True,
        "provider_no_request_before_consent_click": True,
        "provider_review_before_revision": True,
        "provider_review_source_sha256_match": True,
        "provider_revision_source_sha256_match": True,
        "provider_revision_snapshot_match": True,
        "provider_revision_answer_sentinel_present": True,
        "runtime_scan": runtime_scan(),
    }
    if case_id == HARNESS.CASE_REJECT_ALL:
        observations.update(
            proposal_received=True,
            reviewed_source_match=True,
            editor_before=editor_same,
            editor_after=editor_same,
            source_before=disk_same,
            source_after=disk_same,
            reject_all_byte_identity=True,
            dirty_before=False,
            dirty_after=False,
            reject_all_dirty_unchanged=True,
        )
    elif case_id == HARNESS.CASE_SELECTIVE_UNDO:
        selected = fingerprint(
            HARNESS.apply_edits(
                HARNESS.EDITOR_SOURCE_BYTES,
                HARNESS.fixture_edits(HARNESS.EDITOR_SOURCE_TEXT)[:1],
            )
        )
        observations.update(
            editor_before=editor_same,
            selective_preview=selected,
            editor_after_apply=selected,
            editor_after_undo=editor_same,
            source_before=disk_same,
            source_after=disk_same,
            selective_matches_editor=True,
            selective_matches_expected=True,
            one_undo_transaction=True,
            undo_count=1,
        )
    elif case_id == HARNESS.CASE_ACCEPT_ALL_PREVIEW:
        final = fingerprint(
            HARNESS.apply_edits(
                HARNESS.EDITOR_SOURCE_BYTES,
                HARNESS.fixture_edits(HARNESS.EDITOR_SOURCE_TEXT),
            )
        )
        observations.update(
            editor_before_apply=editor_same,
            editor_after_preview=editor_same,
            preview_fingerprint=final,
            final_preview=final,
            accept_all_matches_expected=True,
            copy_preview_fingerprint=final,
            copy_preview_exact=True,
            copy_editor_before=editor_same,
            copy_editor_after=editor_same,
            copy_editor_unchanged=True,
            copy_dirty_before=False,
            copy_dirty_after=False,
            copy_dirty_unchanged=True,
            copy_source_before=disk_same,
            copy_source_after=disk_same,
            copy_source_unchanged=True,
        )
    elif case_id == HARNESS.CASE_STALE:
        edited = fingerprint(HARNESS.STALE_EDIT_TEXT.encode("utf-8"))
        observations.update(
            editor_before=editor_same,
            editor_after_edit=edited,
            source_before=disk_same,
            source_after=disk_same,
            stale_proposal_received=True,
            stale_visible=True,
            apply_activation_attempted=True,
            apply_disabled_source_contract=True,
            stale_no_mutation=True,
            apply_control_observed=True,
            apply_accessibility_reported_enabled=True,
            editor_after_activation_attempt=edited,
        )
    elif case_id == HARNESS.CASE_SAVE_CONFLICT:
        external = fingerprint(HARNESS.EXTERNAL_SOURCE_BYTES)
        observations.update(
            editor_after_apply=fingerprint(
                HARNESS.apply_edits(
                    HARNESS.EDITOR_SOURCE_BYTES,
                    HARNESS.fixture_edits(HARNESS.EDITOR_SOURCE_TEXT),
                )
            ),
            external_source_before=external,
            external_source_after=external,
            save_shortcut_sent=True,
            conflict_visible_before_save=False,
            safe_save_conflict_visible=True,
            safe_save_no_overwrite=True,
        )
    elif case_id == HARNESS.CASE_TRUST_REVOKE:
        observations.update(
            trust_before=True,
            editor_before_apply=fingerprint(HARNESS.HTML_EDITOR_SOURCE_BYTES),
            editor_after_apply=fingerprint(
                HARNESS.apply_edits(
                    HARNESS.HTML_EDITOR_SOURCE_BYTES,
                    HARNESS.fixture_edits(HARNESS.HTML_EDITOR_SOURCE_TEXT),
                )
            ),
            executable_expected=fingerprint(
                HARNESS.apply_edits(
                    HARNESS.HTML_EDITOR_SOURCE_BYTES,
                    HARNESS.fixture_edits(HARNESS.HTML_EDITOR_SOURCE_TEXT),
                )
            ),
            executable_matches_expected=True,
            restricted_after_apply=True,
            executable_change=True,
            trust_revocation_order_source_contract=True,
            preview_inert_source_contract=True,
            preview_fingerprint=fingerprint(
                HARNESS.apply_edits(
                    HARNESS.HTML_EDITOR_SOURCE_BYTES,
                    HARNESS.fixture_edits(HARNESS.HTML_EDITOR_SOURCE_TEXT),
                )
            ),
            preview_matches_expected=True,
            preview_contains_executable_text=True,
        )
    return observations


def valid_evidence() -> dict:
    evidence = HARNESS.new_evidence(HASH)
    evidence["transport"]["request_count"] = len(HARNESS.REQUIRED_CASE_IDS) * 2
    evidence["executable"].update(
        {
            "sha256": HASH,
            "byte_count": 1234,
            "hash_verified": True,
            "copied_sha256": HASH,
            "copy_hash_verified": True,
            "format": "PE32+",
            "machine": "x86_64",
            "machine_code": 0x8664,
            "optional_magic": 0x20B,
        }
    )
    evidence["environment"] = environment()
    for case in evidence["cases"]:
        case.update(status="PASS", duration_ms=1.0, observations=common(case["id"]))
    HARNESS.complete_evidence(evidence, "PASS")
    return evidence


def loopback_request(operation: str) -> dict[str, object]:
    source = HARNESS.EDITOR_SOURCE_TEXT
    source_bytes = source.encode("utf-8")
    if operation == "review":
        payload = {
            "operation": "read_only_review",
            "schema_version": "review-v1",
            "canonical_source_bytes": len(source_bytes),
            "canonical_source_sha256": hashlib.sha256(source_bytes).hexdigest(),
            "scope": "document",
            "frames": [{"content_bytes": len(source_bytes), "content": source}],
        }
    else:
        payload = {
            "operation": "revision",
            "schema_version": "revision-v1",
            "source_snapshot": {"revision": 0, "source_generation": 0},
            "source_sha256": hashlib.sha256(source_bytes).hexdigest(),
            "source": source,
            "answers": [{"answer": HARNESS.ANSWER_TEXT}],
        }
    return {
        "stream": operation == "revision",
        "input": json.dumps(payload, separators=(",", ":")),
    }


class LoopbackProviderTests(unittest.TestCase):
    def test_server_is_loopback_deterministic_and_content_free(self) -> None:
        with HARNESS.LoopbackRevisionServer(HARNESS.EDITOR_SOURCE_TEXT) as server:
            server.grant_review_consent()
            server.begin_consent_click(0)
            for operation in ("review", "revision"):
                if operation == "revision":
                    server.grant_revision_consent()
                    server.begin_consent_click(1)
                payload = loopback_request(operation)
                request = urllib.request.Request(
                    server.base_url + "responses",
                    data=json.dumps(payload).encode("utf-8"),
                    headers={"Content-Type": "application/json"},
                )
                response = urllib.request.urlopen(request)
                body = response.read()
                self.assertEqual(
                    response.headers["X-MarkTurbo-Goal07-Response-Sentinel"],
                    HARNESS.RAW_RESPONSE_SENTINEL,
                )
                if operation == "review":
                    decoded = json.loads(body)
                    self.assertEqual(decoded["status"], "completed")
                    self.assertEqual(
                        decoded["output"][0]["content"][0]["text"],
                        HARNESS.review_response(),
                    )
                    self.assertNotIn(b"response.output_text.delta", body)
                else:
                    self.assertIn(b"response.output_text.delta", body)
                    self.assertIn(HARNESS.RAW_RESPONSE_SENTINEL.encode("utf-8"), body)
            self.assertEqual(len(server.requests), 2)
            self.assertTrue(
                all(
                    set(record)
                    == {
                        "request_index",
                        "byte_count",
                        "path_exact",
                        "stream",
                        "review",
                        "revision",
                        "consent_gate_open",
                        "source_sha256_match",
                        "source_snapshot_match",
                        "answer_sentinel_present",
                        "request_arrival_sequence",
                        "consent_click_sequence",
                        "request_after_consent_click",
                    }
                    for record in server.requests
                )
            )
            self.assertNotIn(HARNESS.DOCUMENT_SENTINEL, json.dumps(server.requests))
            self.assertTrue(server.requests[0]["review"])
            self.assertTrue(server.requests[1]["revision"])
            self.assertFalse(server.requests[0]["stream"])
            self.assertTrue(server.requests[1]["stream"])
            self.assertEqual(server.request_count_snapshot(), 2)
            self.assertEqual(server.contract_evidence()["provider_request_count"], 2)
            self.assertTrue(
                all(record["request_after_consent_click"] for record in server.requests)
            )

    def test_server_rejects_request_after_gate_but_before_click_marker(self) -> None:
        with HARNESS.LoopbackRevisionServer(HARNESS.EDITOR_SOURCE_TEXT) as server:
            server.grant_review_consent()
            request = urllib.request.Request(
                server.base_url + "responses",
                data=json.dumps(loopback_request("review")).encode("utf-8"),
                headers={"Content-Type": "application/json"},
            )
            with self.assertRaisesRegex(urllib.error.HTTPError, "409"):
                urllib.request.urlopen(request)
            self.assertEqual(
                server.request_snapshot(), {"request_count": 0, "invalid_request": True}
            )

    def test_server_rejects_the_wrong_stream_mode_for_each_operation(self) -> None:
        for operation in ("review", "revision"):
            with self.subTest(operation=operation):
                with HARNESS.LoopbackRevisionServer(HARNESS.EDITOR_SOURCE_TEXT) as server:
                    server.grant_review_consent()
                    server.begin_consent_click(0)
                    if operation == "revision":
                        review = urllib.request.Request(
                            server.base_url + "responses",
                            data=json.dumps(loopback_request("review")).encode("utf-8"),
                            headers={"Content-Type": "application/json"},
                        )
                        urllib.request.urlopen(review).read()
                        server.grant_revision_consent()
                        server.begin_consent_click(1)
                    payload = loopback_request(operation)
                    payload["stream"] = not payload["stream"]
                    request = urllib.request.Request(
                        server.base_url + "responses",
                        data=json.dumps(payload).encode("utf-8"),
                        headers={"Content-Type": "application/json"},
                    )

                    with self.assertRaisesRegex(urllib.error.HTTPError, "409"):
                        urllib.request.urlopen(request)
                    self.assertTrue(server.request_snapshot()["invalid_request"])

    def test_request_arrival_sequence_cannot_be_reordered_by_slow_body_validation(self) -> None:
        with HARNESS.LoopbackRevisionServer(HARNESS.EDITOR_SOURCE_TEXT) as server:
            server.grant_review_consent()
            payload = loopback_request("review")
            arrival = server._next_event_sequence()
            server.begin_consent_click(0)
            record, failure = server._validate_request(
                "/v1/responses", payload, request_arrival_sequence=arrival
            )
            self.assertIsNone(record)
            self.assertEqual(failure, "LOOPBACK_REQUEST_BEFORE_CONSENT_CLICK")

    def test_server_rejects_wrong_path_and_request_before_consent(self) -> None:
        with HARNESS.LoopbackRevisionServer(HARNESS.EDITOR_SOURCE_TEXT) as server:
            body = json.dumps(loopback_request("review")).encode("utf-8")
            request = urllib.request.Request(
                server.base_url + "wrong",
                data=body,
                headers={"Content-Type": "application/json"},
            )
            with self.assertRaises(urllib.error.HTTPError):
                urllib.request.urlopen(request)

        with HARNESS.LoopbackRevisionServer(HARNESS.EDITOR_SOURCE_TEXT) as server:
            request = urllib.request.Request(
                server.base_url + "responses",
                data=body,
                headers={"Content-Type": "application/json"},
            )
            with self.assertRaises(urllib.error.HTTPError):
                urllib.request.urlopen(request)
            self.assertEqual(
                server.request_snapshot(), {"request_count": 0, "invalid_request": True}
            )

    def test_server_rejects_wrong_fixture_binding_and_missing_answer(self) -> None:
        with HARNESS.LoopbackRevisionServer(HARNESS.EDITOR_SOURCE_TEXT) as server:
            server.grant_review_consent()
            server.begin_consent_click(0)
            review = urllib.request.Request(
                server.base_url + "responses",
                data=json.dumps(loopback_request("review")).encode("utf-8"),
                headers={"Content-Type": "application/json"},
            )
            urllib.request.urlopen(review).read()
            server.grant_revision_consent()
            server.begin_consent_click(1)
            invalid = loopback_request("revision")
            invalid["input"] = invalid["input"].replace(
                hashlib.sha256(HARNESS.EDITOR_SOURCE_BYTES).hexdigest(), "0" * 64
            )
            request = urllib.request.Request(
                server.base_url + "responses",
                data=json.dumps(invalid).encode("utf-8"),
                headers={"Content-Type": "application/json"},
            )
            with self.assertRaises(urllib.error.HTTPError):
                urllib.request.urlopen(request)
            with self.assertRaisesRegex(runtime.HarnessFailure, "SOURCE_SHA256"):
                server.contract_evidence()

    def test_revision_response_uses_utf8_byte_ranges_and_complete_coverage(self) -> None:
        decoded = json.loads(HARNESS.revision_response(HARNESS.EDITOR_SOURCE_TEXT))
        self.assertEqual(decoded["schema_version"], "revision-v1")
        self.assertEqual(len(decoded["groups"]), 2)
        self.assertEqual(len(decoded["question_coverage"]), 3)
        for group in decoded["groups"]:
            for edit in group["edits"]:
                start = edit["range"]["start"]
                end = edit["range"]["end"]
                self.assertEqual(
                    HARNESS.EDITOR_SOURCE_BYTES[start:end],
                    edit["expected_source"].encode("utf-8"),
                )

    def test_apply_edits_preserves_crlf_cjk_emoji_fences_and_links(self) -> None:
        edited = HARNESS.apply_edits(
            HARNESS.SOURCE_BYTES, HARNESS.fixture_edits(HARNESS.SOURCE_TEXT)
        )
        self.assertIn("计划 🚀".encode("utf-8"), edited)
        self.assertIn(b"\x60\x60\x60rust\r\n", edited)
        self.assertIn(b"[link](https://example.invalid)", edited)
        self.assertIn(b"title: new\r\nowner: team\r\n", edited)
        self.assertEqual(edited.count(b"\r\n"), HARNESS.SOURCE_BYTES.count(b"\r\n"))


class EvidenceTests(unittest.TestCase):
    def test_complete_pass_is_hash_bound_and_requires_all_cases(self) -> None:
        evidence = valid_evidence()
        HARNESS.validate_evidence(evidence)
        self.assertEqual(evidence["status"], "PASS")
        self.assertEqual(
            evidence["summary"]["passed_case_count"], len(HARNESS.REQUIRED_CASE_IDS)
        )

    def test_reject_all_requires_exact_byte_identity(self) -> None:
        evidence = valid_evidence()
        evidence["cases"][0]["observations"]["editor_after"] = fingerprint(b"changed")
        with self.assertRaisesRegex(ValueError, "reject-all changed"):
            HARNESS.validate_evidence(evidence)

    def test_selective_case_requires_one_undo(self) -> None:
        evidence = valid_evidence()
        evidence["cases"][1]["observations"]["undo_count"] = 2
        with self.assertRaisesRegex(ValueError, "exactly one undo"):
            HARNESS.validate_evidence(evidence)

    def test_transport_count_must_match_case_observations(self) -> None:
        evidence = valid_evidence()
        evidence["transport"]["request_count"] -= 1
        with self.assertRaisesRegex(ValueError, "transport request count"):
            HARNESS.validate_evidence(evidence)

    def test_copy_and_stale_proofs_are_required(self) -> None:
        evidence = valid_evidence()
        evidence["cases"][2]["observations"]["copy_preview_exact"] = False
        with self.assertRaisesRegex(ValueError, "requires true copy_preview_exact"):
            HARNESS.validate_evidence(evidence)

        evidence = valid_evidence()
        evidence["cases"][3]["observations"]["apply_activation_attempted"] = False
        with self.assertRaisesRegex(ValueError, "requires true apply_activation_attempted"):
            HARNESS.validate_evidence(evidence)

    def test_trust_proof_must_bind_expected_change_and_source_contract(self) -> None:
        evidence = valid_evidence()
        evidence["cases"][5]["observations"]["trust_revocation_order_source_contract"] = False
        with self.assertRaisesRegex(ValueError, "requires true trust_revocation_order_source_contract"):
            HARNESS.validate_evidence(evidence)

    def test_pass_rejects_hash_mismatch_and_private_content(self) -> None:
        evidence = valid_evidence()
        evidence["executable"]["copied_sha256"] = "b" * 64
        with self.assertRaisesRegex(ValueError, "verified copied executable hash"):
            HARNESS.validate_evidence(evidence)

        evidence = valid_evidence()
        evidence["cases"][0]["observations"]["flow"] = HARNESS.DOCUMENT_SENTINEL
        with self.assertRaisesRegex(ValueError, "free-form observation strings"):
            HARNESS.validate_evidence(evidence)

    def test_blocked_evidence_is_valid_without_windows_environment(self) -> None:
        evidence = HARNESS.new_evidence(HASH)
        evidence["cases"][0].update(
            status="BLOCKED",
            reason_code="WINDOWS_REQUIRED",
            duration_ms=0.0,
            observations={},
        )
        for case in evidence["cases"][1:]:
            case.update(
                status="BLOCKED",
                reason_code="SKIPPED_AFTER_BLOCKED",
                duration_ms=0.0,
            )
        HARNESS.complete_evidence(evidence, "BLOCKED")
        HARNESS.validate_evidence(evidence)
        self.assertEqual(
            evidence["summary"]["blocked_case_count"], len(HARNESS.REQUIRED_CASE_IDS)
        )

    def test_evidence_objects_have_exact_keys_and_private_content_is_recursive(self) -> None:
        for location in ("top-level", "executable", "environment", "case"):
            evidence = valid_evidence()
            if location == "top-level":
                evidence["unexpected"] = True
            elif location == "executable":
                evidence["executable"]["unexpected"] = True
            elif location == "environment":
                evidence["environment"]["unexpected"] = True
            else:
                evidence["cases"][0]["unexpected"] = True
            with self.subTest(location=location), self.assertRaisesRegex(ValueError, "keys"):
                HARNESS.validate_evidence(evidence)

        for sentinel in (
            HARNESS.DOCUMENT_SENTINEL,
            HARNESS.ANSWER_SENTINEL,
            HARNESS.RAW_RESPONSE_SENTINEL,
        ):
            evidence = valid_evidence()
            evidence["cases"][0]["observations"]["flow"] = sentinel
            with self.subTest(sentinel=sentinel), self.assertRaisesRegex(
                ValueError, "free-form observation strings"
            ):
                HARNESS.validate_evidence(evidence)

        evidence = valid_evidence()
        evidence["cases"][0]["observations"]["process_context"] = {
            "request_body": True,
        }
        with self.assertRaisesRegex(ValueError, "private content"):
            HARNESS.validate_evidence(evidence)

        for key in ("process_context", "runtime_scan", "editor_before"):
            evidence = valid_evidence()
            evidence["cases"][0]["observations"][key]["unexpected"] = True
            with self.subTest(nested_key=key), self.assertRaisesRegex(ValueError, "keys"):
                HARNESS.validate_evidence(evidence)

    def test_runtime_scan_requires_positive_and_consistent_config_counts(self) -> None:
        evidence = valid_evidence()
        evidence["cases"][0]["observations"]["runtime_scan"]["config_files_scanned"] = 0
        with self.assertRaisesRegex(ValueError, "application log"):
            HARNESS.validate_evidence(evidence)

        evidence = valid_evidence()
        evidence["cases"][0]["observations"]["runtime_scan"].update(
            files_scanned=1,
            app_logs_scanned=1,
            config_files_scanned=1,
        )
        with self.assertRaisesRegex(ValueError, "counts are inconsistent"):
            HARNESS.validate_evidence(evidence)

        evidence = valid_evidence()
        evidence["cases"][0]["observations"]["runtime_scan"].update(
            files_scanned=2,
            app_logs_scanned=1,
            config_files_scanned=1,
        )
        with self.assertRaisesRegex(ValueError, "file count is incomplete"):
            HARNESS.validate_evidence(evidence)


class PrivacyAndRuntimeTests(unittest.TestCase):
    def test_runtime_scan_detects_split_boundary_private_bytes(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / "data" / "logs").mkdir(parents=True)
            (root / "config").mkdir()
            split = 1024 * 1024 - 2
            (root / "data" / "logs" / "app.log").write_bytes(
                b"x" * split + HARNESS.DOCUMENT_SENTINEL.encode("utf-8")
            )
            with self.assertRaisesRegex(
                runtime.HarnessFailure, "UTF8_DOCUMENT_SENTINEL_LEAKED"
            ):
                HARNESS.scan_case_artifacts(root, "synthetic-key")

    def test_runtime_scan_rejects_credential_leak_without_recording_value(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / "data" / "logs").mkdir(parents=True)
            (root / "config").mkdir()
            (root / "data" / "logs" / "app.log").write_text(
                "synthetic-key", encoding="utf-8"
            )
            with self.assertRaisesRegex(
                runtime.HarnessFailure, "UTF8_EPHEMERAL_CREDENTIAL_LEAKED"
            ):
                HARNESS.scan_case_artifacts(root, "synthetic-key")

    def test_runtime_scan_rejects_answer_and_raw_response_sentinels(self) -> None:
        for sentinel, code in (
            (HARNESS.ANSWER_SENTINEL, "UTF8_ANSWER_SENTINEL_LEAKED"),
            (HARNESS.RAW_RESPONSE_SENTINEL, "UTF8_RAW_RESPONSE_SENTINEL_LEAKED"),
        ):
            with self.subTest(code=code), tempfile.TemporaryDirectory() as directory:
                root = Path(directory)
                (root / "data" / "logs").mkdir(parents=True)
                (root / "config").mkdir()
                (root / "data" / "logs" / "app.log").write_text(
                    sentinel, encoding="utf-8"
                )
                with self.assertRaisesRegex(runtime.HarnessFailure, code):
                    HARNESS.scan_case_artifacts(root, "synthetic-key")

    def test_evidence_does_not_allow_request_body_or_credentials(self) -> None:
        evidence = HARNESS.new_evidence(HASH)
        evidence["transport"]["request_body"] = "forbidden"
        with self.assertRaisesRegex(ValueError, "invalid loopback transport"):
            HARNESS.validate_evidence(evidence)

    def test_privacy_scan_runs_after_app_reap(self) -> None:
        harness = object.__new__(HARNESS.Goal07Harness)
        harness._credential = "synthetic-credential"
        events: list[str] = []
        harness.reap = lambda _app: events.append("reap")
        provider = mock.Mock()
        provider.contract_evidence.return_value = {
            "provider_request_count": 2,
            "provider_review_count": 1,
            "provider_revision_count": 1,
            "provider_paths_exact": True,
            "provider_no_request_before_consent_click": True,
            "provider_review_before_revision": True,
            "provider_review_source_sha256_match": True,
            "provider_revision_source_sha256_match": True,
            "provider_revision_snapshot_match": True,
            "provider_revision_answer_sentinel_present": True,
        }
        with mock.patch.object(
            HARNESS,
            "scan_case_artifacts",
            side_effect=lambda *_args: events.append("scan") or runtime_scan(),
        ):
            result = harness._finalize_observations(
                provider, Path("unused"), object(), {}
            )
        self.assertEqual(events, ["reap", "scan"])
        self.assertEqual(result["runtime_scan"]["files_scanned"], 3)

    def test_failed_privacy_scan_does_not_advance_transport_count(self) -> None:
        harness = object.__new__(HARNESS.Goal07Harness)
        harness._credential = "synthetic-credential"
        harness.reap = mock.Mock()
        provider = mock.Mock()
        provider.contract_evidence.return_value = {
            "provider_request_count": 2,
            "provider_review_count": 1,
            "provider_revision_count": 1,
            "provider_paths_exact": True,
            "provider_no_request_before_consent_click": True,
            "provider_review_before_revision": True,
            "provider_review_source_sha256_match": True,
            "provider_revision_source_sha256_match": True,
            "provider_revision_snapshot_match": True,
            "provider_revision_answer_sentinel_present": True,
        }
        evidence = HARNESS.new_evidence(HASH)

        with (
            mock.patch.object(HARNESS, "_ACTIVE_EVIDENCE", evidence),
            mock.patch.object(
                HARNESS,
                "scan_case_artifacts",
                side_effect=runtime.HarnessFailure("UTF8_DOCUMENT_SENTINEL_LEAKED"),
            ),
            self.assertRaisesRegex(
                runtime.HarnessFailure, "UTF8_DOCUMENT_SENTINEL_LEAKED"
            ),
        ):
            harness._finalize_observations(provider, Path("unused"), object(), {})

        self.assertEqual(evidence["transport"]["request_count"], 0)
        HARNESS.complete_evidence(evidence, "BLOCKED")
        HARNESS.validate_evidence(evidence)


class SourceAndCliTests(unittest.TestCase):
    def test_scenario_contract_failure_is_raised_before_shared_pass_status(self) -> None:
        harness = object.__new__(HARNESS.Goal07Harness)
        observations = common(HARNESS.CASE_REJECT_ALL)
        observations["reject_all_byte_identity"] = False
        harness.parent_context = mock.Mock()
        harness.parent_context.evidence.return_value = observations["process_context"]
        harness.scenario_reject_all = lambda: observations
        evidence = HARNESS.new_evidence(HASH)

        with (
            mock.patch.object(HARNESS, "_ACTIVE_EVIDENCE", evidence),
            self.assertRaisesRegex(runtime.HarnessFailure, "CASE_CONTRACT_FAILED"),
        ):
            harness.scenario(HARNESS.CASE_REJECT_ALL)

        self.assertEqual(evidence["transport"]["request_count"], 0)
        HARNESS.complete_evidence(evidence, "BLOCKED")
        HARNESS.validate_evidence(evidence)

    def test_preview_source_accepts_name_only_text_but_not_the_static_label(self) -> None:
        class NoValuePattern(Exception):
            pass

        class Control:
            def __init__(self, name: str) -> None:
                self.element_info = mock.Mock()
                self.element_info.name = name

            @property
            def iface_value(self) -> object:
                raise NoValuePattern

            def children(self) -> list[object]:
                return []

        harness = object.__new__(HARNESS.Goal07Harness)
        harness.no_pattern_error_class = NoValuePattern
        harness.find_control = lambda *_args, **_kwargs: Control("approved\npreview")

        self.assertEqual(harness._preview_source_text(mock.Mock()), "approved\npreview")

        harness.find_control = lambda *_args, **_kwargs: Control(
            "Final approved Revision source"
        )
        with self.assertRaisesRegex(
            HARNESS.HarnessFailure, "REVISION_PREVIEW_SOURCE_TEXT_CONTRACT_MISMATCH"
        ):
            harness._preview_source_text(mock.Mock())

    def test_answer_input_uses_the_focused_unicode_keyboard_path(self) -> None:
        harness = object.__new__(HARNESS.Goal07Harness)
        harness.ui_timeout = 0.1
        events: list[tuple[str, object]] = []

        class Control:
            def is_enabled(self) -> bool:
                return True

        control = Control()
        app = mock.Mock(hwnd=41)
        harness._click_id = lambda _app, automation_id: events.append(
            ("answer-state", automation_id)
        )
        harness.control_by_id = lambda *_args, **_kwargs: control
        harness.win32 = mock.Mock()
        harness.win32.send_unicode.side_effect = lambda hwnd, text: events.append(
            ("send-unicode", (hwnd, text))
        )
        harness._text_control = lambda *_args, **_kwargs: "kept intent"

        harness._set_answer(app, 0, "kept intent")

        self.assertEqual(events[0][0], "answer-state")
        self.assertEqual(events[1], ("send-unicode", (41, "kept intent")))

    def test_bottom_bar_covered_revision_control_scrolls_from_visible_review_content(self) -> None:
        harness = object.__new__(HARNESS.Goal07Harness)
        harness.ui_timeout = 0.1
        window_rect = mock.Mock(top=50, bottom=1031)
        offscreen = mock.Mock(top=972, bottom=1000)
        onscreen = mock.Mock(top=864, bottom=892)
        control = mock.Mock()
        control.rectangle.side_effect = [offscreen, onscreen]
        anchor = mock.Mock()
        anchor.rectangle.return_value = mock.Mock(top=100, bottom=120)
        app = mock.Mock(hwnd=41)
        app.window.rectangle.return_value = window_rect
        harness.control_by_id = mock.Mock(
            side_effect=lambda _hwnd, automation_id, *_args: (
                control if automation_id == "target" else anchor
            )
        )

        result = harness._scroll_revision_control_into_view(
            app, "target", "Button", control
        )

        self.assertIs(result, control)
        anchor.wheel_mouse_input.assert_called_once_with(wheel_dist=-3)

    def test_stale_control_scrolls_up_until_accessible(self) -> None:
        harness = object.__new__(HARNESS.Goal07Harness)
        harness.ui_timeout = 0.1
        stale = mock.Mock()
        stale.rectangle.return_value = mock.Mock(top=120, bottom=140)
        anchor = mock.Mock()
        anchor.rectangle.return_value = mock.Mock(top=700, bottom=760)
        app = mock.Mock(hwnd=41)
        app.window.rectangle.return_value = mock.Mock(top=50, bottom=900)
        stale_calls = 0

        def control_by_id(_hwnd, automation_id, *_args, **_kwargs):
            nonlocal stale_calls
            if automation_id == HARNESS.REVISION_STALE_ACCESSIBILITY_ID:
                stale_calls += 1
                return stale if stale_calls > 1 else None
            if automation_id == HARNESS.REVISION_PREVIEW_ACCESSIBILITY_ID:
                return anchor
            return None

        harness.control_by_id = control_by_id

        result = harness._find_stale_control(app)

        self.assertIs(result, stale)
        anchor.wheel_mouse_input.assert_called_once_with(wheel_dist=3)

    def test_apply_state_scrolls_above_the_bottom_bar_before_querying(self) -> None:
        harness = object.__new__(HARNESS.Goal07Harness)
        button = mock.Mock()
        button.is_enabled.return_value = True
        app = mock.Mock(hwnd=41)
        harness.control_by_id = mock.Mock(return_value=button)
        harness._scroll_revision_control_into_view = mock.Mock(return_value=button)

        result, reported_enabled = harness._apply_button_state(app)

        self.assertIs(result, button)
        self.assertTrue(reported_enabled)
        harness._scroll_revision_control_into_view.assert_called_once_with(
            app,
            HARNESS.REVISION_APPLY_ACCESSIBILITY_ID,
            "Button",
            button,
        )

    def test_trust_case_reactivates_source_before_review(self) -> None:
        body = inspect.getsource(HARNESS.Goal07Harness.scenario_trust_revoke)
        trusted = body.index("trusted_label = wait_until")
        reactivate = body.index("self.activate_source_layout(app)", trusted)
        review = body.index("self.run_review(app, provider)", trusted)

        self.assertLess(trusted, reactivate)
        self.assertLess(reactivate, review)

    def test_revision_result_uses_the_preview_group_contract(self) -> None:
        harness = object.__new__(HARNESS.Goal07Harness)
        harness._set_answer = mock.Mock()
        harness._mark_intentionally_unspecified = mock.Mock()
        harness._click_id = mock.Mock()
        harness._approve_consent = mock.Mock()
        harness.find_control = mock.Mock(return_value=object())

        harness.request_revision(mock.Mock(), answer=False)

        self.assertEqual(
            harness.find_control.call_args.args[2],
            "Group",
        )

    def test_consent_gate_rejects_any_pre_click_request_snapshot(self) -> None:
        harness = object.__new__(HARNESS.Goal07Harness)
        provider = mock.Mock()
        provider.request_snapshot.return_value = {
            "request_count": 1,
            "invalid_request": False,
        }
        opened: list[bool] = []
        with self.assertRaisesRegex(
            runtime.HarnessFailure, "REVIEW_REQUEST_BEFORE_CONSENT_CLICK"
        ):
            harness._open_consent_gate(provider, 0, lambda: opened.append(True))
        provider.request_snapshot.assert_called_once_with()
        self.assertEqual(opened, [])

    def test_consent_gate_records_rejected_request_before_click(self) -> None:
        harness = object.__new__(HARNESS.Goal07Harness)
        provider = mock.Mock()
        provider.request_snapshot.return_value = {
            "request_count": 0,
            "invalid_request": True,
        }
        with self.assertRaisesRegex(
            runtime.HarnessFailure, "REVISION_REQUEST_BEFORE_CONSENT_CLICK"
        ):
            harness._open_consent_gate(provider, 1, lambda: None)

    def test_consent_gate_opens_before_the_consent_button_click(self) -> None:
        harness = object.__new__(HARNESS.Goal07Harness)
        harness.ui_timeout = 0.1
        events: list[str] = []
        harness.win32 = mock.Mock()
        harness.win32.owned_task_dialogs.return_value = [99]
        harness.control_by_id = lambda *_args, **_kwargs: object()
        harness.click_control = lambda _control, _failure: events.append("click")
        provider = mock.Mock()
        provider.request_snapshot.return_value = {
            "request_count": 0,
            "invalid_request": False,
        }
        provider.begin_consent_click.side_effect = lambda count: events.append(
            f"marker-{count}"
        )

        harness._approve_consent(
            mock.Mock(process=mock.Mock(pid=7), hwnd=8),
            "REVIEW_CONSENT_CLICK_FAILED",
            provider=provider,
            expected_request_count=0,
            open_gate=lambda: events.append("gate"),
        )

        self.assertEqual(events, ["gate", "marker-0", "click"])

    def test_app_owned_click_requires_foreground_first(self) -> None:
        harness = object.__new__(HARNESS.Goal07Harness)
        harness.ui_timeout = 0.1
        events: list[str] = []
        harness.win32 = mock.Mock()
        harness.win32.require_foreground.side_effect = lambda *_args: events.append(
            "foreground"
        )
        harness.find_control = lambda *_args, **_kwargs: object()
        harness._scroll_revision_control_into_view = (
            lambda _app, _automation_id, _control_type, control: (
                events.append("scroll") or control
            )
        )
        harness.click_control = lambda _control, _failure: events.append("click")

        harness._click_id(mock.Mock(hwnd=41), "markturbo-revision-accept-all")

        self.assertEqual(events, ["foreground", "scroll", "click"])

    def test_revision_run_waits_until_enabled_before_click(self) -> None:
        harness = object.__new__(HARNESS.Goal07Harness)
        harness.ui_timeout = 0.1
        states = iter((False, True))
        events: list[str] = []

        class Control:
            def is_enabled(self) -> bool:
                events.append("enabled")
                return next(states)

        control = Control()
        harness.win32 = mock.Mock()
        harness.find_control = lambda *_args, **_kwargs: control
        harness._scroll_revision_control_into_view = (
            lambda _app, _automation_id, _control_type, candidate: candidate
        )
        harness.control_by_id = lambda *_args, **_kwargs: control
        harness.click_control = lambda _control, _failure: events.append("click")

        harness._click_id(mock.Mock(hwnd=41), HARNESS.REVISION_RUN_ACCESSIBILITY_ID)

        self.assertEqual(events, ["enabled", "enabled", "click"])

    def test_loopback_profile_is_keyless_and_uses_only_a_process_synthetic_key(self) -> None:
        settings = HARNESS.review_settings_document("http://127.0.0.1:4141/v1/").decode(
            "utf-8"
        )
        self.assertIn('model-provider = "openai-responses"', settings)
        self.assertNotIn("api-key", settings.casefold())
        self.assertIn("model-environment-key-identity", settings)
        self.assertIn(
            HARNESS.endpoint_environment_key_identity("http://127.0.0.1:4141/v1/"),
            settings,
        )
        harness = object.__new__(HARNESS.Goal07Harness)
        harness._credential = "markturbo-goal07-loopback-key"
        self.assertEqual(
            harness.openai_api_key_for_child(), "markturbo-goal07-loopback-key"
        )

    def test_harness_credentials_are_fresh_uuid_values(self) -> None:
        with mock.patch.object(HARNESS.ClipboardNativeHarness, "__init__", return_value=None):
            first = HARNESS.Goal07Harness()
            second = HARNESS.Goal07Harness()
        self.assertRegex(first.openai_api_key_for_child(), r"^markturbo-goal07-[0-9a-f]{32}$")
        self.assertRegex(second.openai_api_key_for_child(), r"^markturbo-goal07-[0-9a-f]{32}$")
        self.assertNotEqual(first.openai_api_key_for_child(), second.openai_api_key_for_child())

    def test_profile_blocks_existing_persistent_credential_after_dynamic_endpoint(self) -> None:
        harness = object.__new__(HARNESS.Goal07Harness)
        harness.win32 = mock.Mock()
        harness.win32.persistent_credential_target_exists.return_value = True
        with tempfile.TemporaryDirectory() as directory:
            harness.root = Path(directory)
            with self.assertRaisesRegex(runtime.HarnessBlocked, "PERSISTENT_CREDENTIAL_PRESENT"):
                harness.profile("synthetic", "http://127.0.0.1:43123/v1/")
        harness.win32.persistent_credential_target_exists.assert_called_once_with(
            HARNESS.endpoint_environment_key_identity("http://127.0.0.1:43123/v1/")
        )

    def test_dynamic_endpoint_identity_includes_port_and_path(self) -> None:
        first = HARNESS.endpoint_environment_key_identity("http://127.0.0.1:4141/v1/")
        second = HARNESS.endpoint_environment_key_identity("http://127.0.0.1:4141/other/")
        third = HARNESS.endpoint_environment_key_identity("http://127.0.0.1:4142/v1/")
        self.assertNotEqual(first, second)
        self.assertNotEqual(first, third)
        self.assertEqual(
            first,
            "io.github.wxxb789.markturbo:model-credential:v2|wire=openai-responses|"
            "host=127.0.0.1|identity-sha256=76d1c8cb04c37287006720b64f66be02bb85723be756cc668cbc0acc13bb1eb4",
        )

    def test_question_and_change_ids_match_production_shape(self) -> None:
        self.assertRegex(
            HARNESS.revision_question_accessibility_id(0),
            r"^markturbo-revision-question-[0-9a-f]{64}$",
        )
        self.assertEqual(
            HARNESS.revision_change_accessibility_id(7, "accept"),
            "markturbo-revision-change-7-accept",
        )

    def test_parser_requires_hash_and_positive_timeout(self) -> None:
        args = HARNESS.parse_args(["--expect-exe-sha256", HASH, "--ui-timeout", "1.5"])
        self.assertEqual(args.expect_exe_sha256, HASH)
        self.assertEqual(args.ui_timeout, 1.5)
        with self.assertRaises(argparse.ArgumentTypeError):
            HARNESS.normalize_expected_hash("not-a-hash")

    def test_single_case_is_not_a_full_acceptance_run(self) -> None:
        args = HARNESS.parse_args(
            ["--expect-exe-sha256", HASH, "--case", HARNESS.CASE_STALE]
        )
        self.assertEqual(args.case, HARNESS.CASE_STALE)

    def test_source_contract_reads_production_only(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            workspace = root / "crates" / "mt-app" / "src" / "views"
            workspace.mkdir(parents=True)
            production = "\n".join(
                [
                    HARNESS.REVIEW_RUN_ACCESSIBILITY_ID,
                    HARNESS.REVIEW_RESULT_ACCESSIBILITY_ID,
                    HARNESS.REVISION_REQUEST_ACCESSIBILITY_ID,
                    HARNESS.REVISION_PREVIEW_ACCESSIBILITY_ID,
                    HARNESS.REVISION_PREVIEW_SOURCE_ACCESSIBILITY_ID,
                    HARNESS.REVISION_STALE_ACCESSIBILITY_ID,
                    HARNESS.REVISION_ACCEPT_ALL_ACCESSIBILITY_ID,
                    HARNESS.REVISION_REJECT_ALL_ACCESSIBILITY_ID,
                    HARNESS.REVISION_APPLY_ACCESSIBILITY_ID,
                    HARNESS.REVISION_COPY_ACCESSIBILITY_ID,
                    HARNESS.REVISION_CHANGE_PREFIX,
                    "accessibility_id(REVISION_RUN_ACCESSIBILITY_ID)",
                    'accessibility_id("markturbo-revision-preview")',
                    'accessibility_id("markturbo-revision-preview-source")',
                    ".aria_value(preview.clone())",
                    '.id("revision-stale")',
                    "accessibility_id(REVISION_STALE_ACCESSIBILITY_ID)",
                    ".role(gpui::Role::Label)",
                    ".aria_label(i18n::t(i18n::Key::RevisionStaleInspection, cx))",
                    ".into_any_element()",
                    'Button::new("revision-apply")',
                    "accessibility_id(REVISION_APPLY_ACCESSIBILITY_ID)",
                    ".disabled(revision_stale)",
                    "this.apply_revision(window, cx)",
                    ".into_any_element()",
                    "accessibility_id(REVISION_ACCEPT_ALL_ACCESSIBILITY_ID)",
                    "accessibility_id(REVISION_REJECT_ALL_ACCESSIBILITY_ID)",
                    "accessibility_id(REVISION_APPLY_ACCESSIBILITY_ID)",
                    "accessibility_id(REVISION_COPY_ACCESSIBILITY_ID)",
                    "revision_question_binding_id(index, question)",
                    '"{question_id}-answered"',
                    '"{question_id}-input"',
                    '"markturbo-revision-change-{}"',
                    "apply_approved_revision Revision",
                    "let preview: SharedString = self.revision_preview().unwrap_or_default().into();",
                    'accessibility_id("markturbo-revision-preview")',
                    'accessibility_id("markturbo-revision-preview-source")',
                    ".role(gpui::Role::Label)",
                    ".aria_value(preview.clone())",
                    ".child(preview)",
                    "for coverage in revision.result.question_coverage()",
                ]
            )
            (workspace / "workspace.rs").write_text(production, encoding="utf-8")
            (workspace / "document.rs").write_text(
                "\n".join(
                    [
                        'DocumentEvent::Conflict Button::new("trust") Trust::Trusted',
                        "accessibility_id(DOCUMENT_TRUST_ACCESSIBILITY_ID)",
                        '"markturbo-document-trust"',
                        "CONFLICT_OVERWRITE_ACCESSIBILITY_ID",
                        '"markturbo-conflict-overwrite"',
                        "pub fn apply_approved_revision() {",
                        "let revoke_trust = self.trust == Trust::Trusted && matches!(self.document.doc_type(), DocType::Html | DocType::Mdx);",
                        "if revoke_trust {",
                        "self.trust = Trust::Restricted;",
                        "self.rebuild_web(cx);",
                        "}",
                        "self.replace_text(final_text, window, cx);",
                        "}",
                    ]
                ),
                encoding="utf-8",
            )
            with mock.patch.object(HARNESS, "REPO", root):
                self.assertIsNone(HARNESS.source_contract_failure())
                self.assertTrue(HARNESS.trust_apply_source_contract_ok())
                self.assertTrue(HARNESS.preview_inert_source_contract_ok())
            (workspace / "workspace.rs").write_text(
                "\n#[cfg(test)]\n" + production,
                encoding="utf-8",
            )
            with mock.patch.object(HARNESS, "REPO", root):
                self.assertEqual(
                    HARNESS.source_contract_failure(), "REVISION_UIA_CONTRACT_MISSING"
                )

    def test_apply_edits_rejects_invalid_ranges_duplicate_offsets_and_oversized_output(self) -> None:
        invalid = [
            {"range": {"start": -1, "end": 0}, "expected_source": "", "replacement": "x"},
            {"range": {"start": 0, "end": 4}, "expected_source": "abc", "replacement": "x"},
        ]
        for edit in invalid:
            with self.subTest(edit=edit), self.assertRaisesRegex(ValueError, "invalid native fixture edit"):
                HARNESS.apply_edits(b"abc", [edit])

        split_character = {
            "range": {"start": 1, "end": 1},
            "expected_source": "",
            "replacement": "x",
        }
        with self.assertRaisesRegex(ValueError, "invalid native fixture edit"):
            HARNESS.apply_edits("é".encode("utf-8"), [split_character])

        duplicate_insertions = [
            {"range": {"start": 1, "end": 1}, "expected_source": "", "replacement": "x"},
            {"range": {"start": 1, "end": 1}, "expected_source": "", "replacement": "y"},
        ]
        with self.assertRaisesRegex(ValueError, "invalid native fixture edit"):
            HARNESS.apply_edits(b"abc", duplicate_insertions)

        overlapping = [
            {"range": {"start": 0, "end": 2}, "expected_source": "ab", "replacement": "x"},
            {"range": {"start": 1, "end": 3}, "expected_source": "bc", "replacement": "y"},
        ]
        with self.assertRaisesRegex(ValueError, "invalid native fixture edit"):
            HARNESS.apply_edits(b"abc", overlapping)

        with mock.patch.object(HARNESS, "MAX_EDIT_OUTPUT_BYTES", 3):
            oversized = {
                "range": {"start": 0, "end": 1},
                "expected_source": "a",
                "replacement": "xxxx",
            }
            with self.assertRaisesRegex(ValueError, "invalid native fixture edit"):
                HARNESS.apply_edits(b"abc", [oversized])

        no_op = {
            "range": {"start": 0, "end": 1},
            "expected_source": "a",
            "replacement": "a",
        }
        with self.assertRaisesRegex(ValueError, "invalid native fixture edit"):
            HARNESS.apply_edits(b"abc", [no_op])

        with mock.patch.object(HARNESS, "MAX_EDIT_OUTPUT_BYTES", 4):
            subset_only_oversized = [
                {
                    "range": {"start": 0, "end": 3},
                    "expected_source": "abc",
                    "replacement": "",
                },
                {
                    "range": {"start": 4, "end": 4},
                    "expected_source": "",
                    "replacement": "xyz",
                },
            ]
            with self.assertRaisesRegex(ValueError, "invalid native fixture edit"):
                HARNESS.apply_edits(b"abcd", subset_only_oversized)


if __name__ == "__main__":
    unittest.main()
