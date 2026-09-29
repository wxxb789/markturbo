"""Goal 07 deterministic provider protocol and fixture tests."""

from __future__ import annotations

import hashlib
import json
import unittest
import urllib.error
import urllib.request

from scripts.markturbo_tools.native import goal07 as HARNESS
from scripts.markturbo_tools.native import goal07_provider as PROVIDER
from scripts.markturbo_tools.native import runtime


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
            "answers": [{"answer": PROVIDER.ANSWER_TEXT}],
        }
    return {
        "stream": operation == "revision",
        "input": json.dumps(payload, separators=(",", ":")),
    }


class LoopbackProviderTests(unittest.TestCase):
    def test_server_is_loopback_deterministic_and_content_free(self) -> None:
        with PROVIDER.LoopbackRevisionServer(HARNESS.EDITOR_SOURCE_TEXT) as server:
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
                with urllib.request.urlopen(request) as response:
                    body = response.read()
                    self.assertEqual(
                        response.headers["X-MarkTurbo-Goal07-Response-Sentinel"],
                        PROVIDER.RAW_RESPONSE_SENTINEL,
                    )
                if operation == "review":
                    decoded = json.loads(body)
                    self.assertEqual(decoded["status"], "completed")
                    self.assertEqual(
                        decoded["output"][0]["content"][0]["text"],
                        PROVIDER.review_response(),
                    )
                    self.assertNotIn(b"response.output_text.delta", body)
                else:
                    self.assertIn(b"response.output_text.delta", body)
                    self.assertIn(PROVIDER.RAW_RESPONSE_SENTINEL.encode("utf-8"), body)
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
        with PROVIDER.LoopbackRevisionServer(HARNESS.EDITOR_SOURCE_TEXT) as server:
            server.grant_review_consent()
            request = urllib.request.Request(
                server.base_url + "responses",
                data=json.dumps(loopback_request("review")).encode("utf-8"),
                headers={"Content-Type": "application/json"},
            )
            with self.assertRaisesRegex(urllib.error.HTTPError, "409") as raised:
                urllib.request.urlopen(request)
            raised.exception.close()
            self.assertEqual(
                server.request_snapshot(), {"request_count": 0, "invalid_request": True}
            )

    def test_server_rejects_the_wrong_stream_mode_for_each_operation(self) -> None:
        for operation in ("review", "revision"):
            with self.subTest(operation=operation):
                with PROVIDER.LoopbackRevisionServer(HARNESS.EDITOR_SOURCE_TEXT) as server:
                    server.grant_review_consent()
                    server.begin_consent_click(0)
                    if operation == "revision":
                        review = urllib.request.Request(
                            server.base_url + "responses",
                            data=json.dumps(loopback_request("review")).encode("utf-8"),
                            headers={"Content-Type": "application/json"},
                        )
                        with urllib.request.urlopen(review) as response:
                            response.read()
                        server.grant_revision_consent()
                        server.begin_consent_click(1)
                    payload = loopback_request(operation)
                    payload["stream"] = not payload["stream"]
                    request = urllib.request.Request(
                        server.base_url + "responses",
                        data=json.dumps(payload).encode("utf-8"),
                        headers={"Content-Type": "application/json"},
                    )

                    with self.assertRaisesRegex(urllib.error.HTTPError, "409") as raised:
                        urllib.request.urlopen(request)
                    raised.exception.close()
                    self.assertTrue(server.request_snapshot()["invalid_request"])

    def test_request_arrival_sequence_cannot_be_reordered_by_slow_body_validation(self) -> None:
        with PROVIDER.LoopbackRevisionServer(HARNESS.EDITOR_SOURCE_TEXT) as server:
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
        with PROVIDER.LoopbackRevisionServer(HARNESS.EDITOR_SOURCE_TEXT) as server:
            body = json.dumps(loopback_request("review")).encode("utf-8")
            request = urllib.request.Request(
                server.base_url + "wrong",
                data=body,
                headers={"Content-Type": "application/json"},
            )
            with self.assertRaises(urllib.error.HTTPError) as raised:
                urllib.request.urlopen(request)
            raised.exception.close()

        with PROVIDER.LoopbackRevisionServer(HARNESS.EDITOR_SOURCE_TEXT) as server:
            request = urllib.request.Request(
                server.base_url + "responses",
                data=body,
                headers={"Content-Type": "application/json"},
            )
            with self.assertRaises(urllib.error.HTTPError) as raised:
                urllib.request.urlopen(request)
            raised.exception.close()
            self.assertEqual(
                server.request_snapshot(), {"request_count": 0, "invalid_request": True}
            )

    def test_server_rejects_wrong_fixture_binding_and_missing_answer(self) -> None:
        with PROVIDER.LoopbackRevisionServer(HARNESS.EDITOR_SOURCE_TEXT) as server:
            server.grant_review_consent()
            server.begin_consent_click(0)
            review = urllib.request.Request(
                server.base_url + "responses",
                data=json.dumps(loopback_request("review")).encode("utf-8"),
                headers={"Content-Type": "application/json"},
            )
            with urllib.request.urlopen(review) as response:
                response.read()
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
            with self.assertRaises(urllib.error.HTTPError) as raised:
                urllib.request.urlopen(request)
            raised.exception.close()
            with self.assertRaisesRegex(runtime.HarnessFailure, "SOURCE_SHA256"):
                server.contract_evidence()

    def test_revision_response_uses_utf8_byte_ranges_and_complete_coverage(self) -> None:
        decoded = json.loads(PROVIDER.revision_response(HARNESS.EDITOR_SOURCE_TEXT))
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
            HARNESS.SOURCE_BYTES, PROVIDER.fixture_edits(HARNESS.SOURCE_TEXT)
        )
        self.assertIn("计划 🚀".encode("utf-8"), edited)
        self.assertIn(b"\x60\x60\x60rust\r\n", edited)
        self.assertIn(b"[link](https://example.invalid)", edited)
        self.assertIn(b"title: new\r\nowner: team\r\n", edited)
        self.assertEqual(edited.count(b"\r\n"), HARNESS.SOURCE_BYTES.count(b"\r\n"))
