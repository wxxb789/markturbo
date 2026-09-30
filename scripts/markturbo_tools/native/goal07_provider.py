"""Goal-specific deterministic provider fixtures for Goal 07 native acceptance."""

from __future__ import annotations

import hashlib
import http.server
import json
import threading
from typing import Any

from .runtime import HarnessFailure

ANSWER_SENTINEL = "MTG07-NATIVE-ANSWER-SENTINEL"
RAW_RESPONSE_SENTINEL = "MTG07-NATIVE-RAW-RESPONSE-SENTINEL"
ANSWER_TEXT = f"Use the new title for the revised plan. {ANSWER_SENTINEL}"

QUESTION_0 = "Which title should the revised plan use?"
QUESTION_1 = "Should the owner field remain explicit?"
QUESTION_2 = "Which rollout detail is intentionally left open?"
QUESTION_IMPACT_0 = "A title choice changes the artifact's audience and identity."
QUESTION_IMPACT_1 = "An owner choice changes accountability for the first step."
QUESTION_IMPACT_2 = "A rollout choice changes the success evidence."


def _question_id(index: int, question: str, priority: int, impact: str) -> str:
    digest = hashlib.sha256()
    digest.update(b"markturbo-revision-question-v1\0")
    digest.update(index.to_bytes(8, "big"))
    digest.update(question.encode("utf-8"))
    digest.update(bytes([priority]))
    digest.update(b"\x01")
    digest.update(impact.encode("utf-8"))
    return digest.hexdigest()


QUESTION_IDS = (
    _question_id(0, QUESTION_0, 1, QUESTION_IMPACT_0),
    _question_id(1, QUESTION_1, 2, QUESTION_IMPACT_1),
    _question_id(2, QUESTION_2, 3, QUESTION_IMPACT_2),
)


def _source_edit(source: str, old: str, new: str) -> dict[str, Any]:
    source_bytes = source.encode("utf-8")
    old_bytes = old.encode("utf-8")
    start = source_bytes.find(old_bytes)
    if start < 0:
        raise ValueError("native fixture edit source is missing")
    return {
        "range": {"start": start, "end": start + len(old_bytes)},
        "expected_source": old,
        "replacement": new,
    }


def fixture_edits(source: str) -> list[dict[str, Any]]:
    if "window.goal07" in source:
        return [_source_edit(source, 'goal07 = "old"', 'goal07 = "new"')]
    return [
        _source_edit(source, "title: old", "title: new"),
        _source_edit(source, "owner: TBD", "owner: team"),
    ]


def revision_response(source: str) -> str:
    groups = (
        [
            {
                "rationale": "The executable fixture changes only the requested script value.",
                "edits": [_source_edit(source, 'goal07 = "old"', 'goal07 = "new"')],
            }
        ]
        if "window.goal07" in source
        else [
            {
                "rationale": "The answered title decision changes only the title line.",
                "edits": [_source_edit(source, "title: old", "title: new")],
            },
            {
                "rationale": "The answered owner decision changes only the owner line.",
                "edits": [_source_edit(source, "owner: TBD", "owner: team")],
            },
        ]
    )
    coverage = [
        {
            "question_index": 0,
            "question_id": QUESTION_IDS[0],
            "status": {"kind": "represented", "change_ids": [0]},
        },
        {
            "question_index": 1,
            "question_id": QUESTION_IDS[1],
            "status": {
                "kind": "intentionally_omitted",
                "reason": "The owner deliberately left this decision unchanged.",
            },
        },
        {
            "question_index": 2,
            "question_id": QUESTION_IDS[2],
            "status": {"kind": "not_addressed"},
        },
    ]
    return json.dumps(
        {"schema_version": "revision-v1", "groups": groups, "question_coverage": coverage},
        separators=(",", ":"),
    )


def review_response() -> str:
    return json.dumps(
        {
            "schema_version": "review-v1",
            "scope": {"kind": "document"},
            "understood_intent": {
                "stated_goal": "Preserve the reviewed artifact while making approved decisions explicit.",
                "relevant_context": [],
                "constraints": [],
                "non_goals": [],
                "expected_deliverable": "A locally approved revision.",
                "success_evidence": [],
                "inferred_assumptions": [],
                "unresolved_decisions": [],
            },
            "findings": [],
            "clarification_questions": [
                {"question": QUESTION_0, "priority": "high", "impact": QUESTION_IMPACT_0},
                {"question": QUESTION_1, "priority": "medium", "impact": QUESTION_IMPACT_1},
                {"question": QUESTION_2, "priority": "low", "impact": QUESTION_IMPACT_2},
            ],
        },
        separators=(",", ":"),
    )


class _LoopbackHttpServer(http.server.ThreadingHTTPServer):
    daemon_threads = True
    allow_reuse_address = True


class LoopbackRevisionServer:
    """A deterministic OpenAI Responses-compatible loopback provider."""

    def __init__(self, source: str) -> None:
        self.source = source
        self._source_sha256 = hashlib.sha256(source.encode("utf-8")).hexdigest()
        self._source_snapshot = {"revision": 0, "source_generation": 0}
        self.requests: list[dict[str, Any]] = []
        self._server: _LoopbackHttpServer | None = None
        self._thread: threading.Thread | None = None
        self._lock = threading.RLock()
        self._review_consent = False
        self._revision_consent = False
        self._event_sequence = 0
        self._consent_click_sequence: dict[str, int | None] = {
            "read_only_review": None,
            "revision": None,
        }
        self._invalid_request = False
        self._failure_code: str | None = None

    @staticmethod
    def _nested_values(value: Any, key: str) -> list[Any]:
        values: list[Any] = []
        if isinstance(value, dict):
            if key in value:
                values.append(value[key])
            for nested in value.values():
                values.extend(LoopbackRevisionServer._nested_values(nested, key))
        elif isinstance(value, list):
            for nested in value:
                values.extend(LoopbackRevisionServer._nested_values(nested, key))
        elif isinstance(value, str) and value[:1] in {"{", "["}:
            try:
                decoded = json.loads(value)
            except (json.JSONDecodeError, RecursionError):
                return values
            values.extend(LoopbackRevisionServer._nested_values(decoded, key))
        return values

    @staticmethod
    def _contains_exact_string(value: Any, expected: str) -> bool:
        if isinstance(value, str):
            if value == expected:
                return True
            if value[:1] in {"{", "["}:
                try:
                    return LoopbackRevisionServer._contains_exact_string(
                        json.loads(value), expected
                    )
                except (json.JSONDecodeError, RecursionError):
                    return False
            return False
        if isinstance(value, dict):
            return any(
                LoopbackRevisionServer._contains_exact_string(nested, expected)
                for nested in value.values()
            )
        if isinstance(value, list):
            return any(
                LoopbackRevisionServer._contains_exact_string(nested, expected)
                for nested in value
            )
        return False

    @staticmethod
    def _contains_text(value: Any, expected: str) -> bool:
        if isinstance(value, str):
            if expected in value:
                return True
            if value[:1] in {"{", "["}:
                try:
                    return LoopbackRevisionServer._contains_text(json.loads(value), expected)
                except (json.JSONDecodeError, RecursionError):
                    return False
            return False
        if isinstance(value, dict):
            return any(
                LoopbackRevisionServer._contains_text(nested, expected)
                for nested in value.values()
            )
        if isinstance(value, list):
            return any(
                LoopbackRevisionServer._contains_text(nested, expected)
                for nested in value
            )
        return False

    @staticmethod
    def _operations(value: Any) -> list[str]:
        return [
            operation
            for operation in LoopbackRevisionServer._nested_values(value, "operation")
            if isinstance(operation, str)
        ]

    def grant_review_consent(self) -> None:
        with self._lock:
            self._review_consent = True

    def grant_revision_consent(self) -> None:
        with self._lock:
            self._revision_consent = True

    def begin_consent_click(self, expected_request_count: int) -> None:
        """Record the click-dispatch linearization point before UIA Invoke."""
        operation = {
            0: "read_only_review",
            1: "revision",
        }.get(expected_request_count)
        if operation is None:
            raise HarnessFailure("CONSENT_GATE_CONFIGURATION_INVALID")
        with self._lock:
            self._begin_consent_click_locked(operation, expected_request_count)

    def _begin_consent_click_locked(self, operation: str, expected_request_count: int) -> None:
        if self._invalid_request or len(self.requests) != expected_request_count:
            raise HarnessFailure("CONSENT_REQUEST_SEQUENCE_INVALID")
        if not (
            self._review_consent if operation == "read_only_review" else self._revision_consent
        ):
            raise HarnessFailure("CONSENT_GATE_NOT_OPEN")
        if self._consent_click_sequence[operation] is not None:
            raise HarnessFailure("CONSENT_CLICK_ALREADY_RECORDED")
        self._event_sequence += 1
        self._consent_click_sequence[operation] = self._event_sequence

    def dispatch_consent_click(self, expected_request_count: int, click: Any) -> None:
        """Linearize UIA click dispatch and provider request arrival."""
        operation = {
            0: "read_only_review",
            1: "revision",
        }.get(expected_request_count)
        if operation is None:
            raise HarnessFailure("CONSENT_GATE_CONFIGURATION_INVALID")
        with self._lock:
            self._begin_consent_click_locked(operation, expected_request_count)
            click()

    def request_count_snapshot(self) -> int:
        """Return only the accepted request count, without exposing request data."""
        with self._lock:
            return len(self.requests)

    def _next_event_sequence(self) -> int:
        with self._lock:
            self._event_sequence += 1
            return self._event_sequence

    def request_snapshot(self) -> dict[str, int | bool]:
        """Return the safe count/invalid-attempt state used before consent clicks."""
        with self._lock:
            return {
                "request_count": len(self.requests),
                "invalid_request": self._invalid_request,
            }

    def _reject(self, handler: http.server.BaseHTTPRequestHandler, code: str, status: int) -> None:
        with self._lock:
            self._invalid_request = True
            self._failure_code = code
        try:
            handler.send_response(status)
            handler.send_header("Content-Length", "0")
            handler.send_header("Connection", "close")
            handler.end_headers()
        except (BrokenPipeError, ConnectionResetError):
            return

    def _validate_request(
        self,
        path: str,
        parsed: Any,
        body_length: int | None = None,
        request_arrival_sequence: int | None = None,
    ) -> tuple[dict[str, Any], str] | tuple[None, str]:
        if request_arrival_sequence is None:
            request_arrival_sequence = self._next_event_sequence()
        if path != "/v1/responses":
            return None, "LOOPBACK_PATH_MISMATCH"
        if not isinstance(parsed, dict):
            return None, "LOOPBACK_REQUEST_NOT_OBJECT"
        operations = self._operations(parsed)
        if len(operations) != 1:
            return None, "LOOPBACK_OPERATION_MISSING_OR_DUPLICATE"
        operation = operations[0]
        if operation not in {"read_only_review", "revision"}:
            return None, "LOOPBACK_OPERATION_UNEXPECTED"
        stream = parsed.get("stream")
        expected_stream = operation == "revision"
        if stream is not expected_stream:
            return None, "LOOPBACK_STREAM_MODE_MISMATCH"

        canonical_hashes = self._nested_values(parsed, "canonical_source_sha256")
        canonical_sizes = self._nested_values(parsed, "canonical_source_bytes")
        source_hashes = self._nested_values(parsed, "source_sha256")
        snapshots = self._nested_values(parsed, "source_snapshot")
        source_present = self._contains_exact_string(parsed, self.source)
        expected_size = len(self.source.encode("utf-8"))
        if operation == "read_only_review":
            source_hash_match = canonical_hashes == [self._source_sha256]
            source_snapshot_match = canonical_sizes == [expected_size] and source_present
            answer_sentinel_present = False
        else:
            source_hash_match = source_hashes == [self._source_sha256]
            source_snapshot_match = snapshots == [self._source_snapshot] and source_present
            answer_sentinel_present = self._contains_text(parsed, ANSWER_SENTINEL)
        with self._lock:
            review_count = sum(record["review"] for record in self.requests)
            revision_count = sum(record["revision"] for record in self.requests)
            consent = self._review_consent if operation == "read_only_review" else self._revision_consent
            consent_click_sequence = self._consent_click_sequence[operation]
            if not consent:
                return None, "LOOPBACK_REQUEST_BEFORE_CONSENT"
            if (
                consent_click_sequence is None
                or request_arrival_sequence <= consent_click_sequence
            ):
                return None, "LOOPBACK_REQUEST_BEFORE_CONSENT_CLICK"
            if operation == "read_only_review" and (review_count != 0 or revision_count != 0):
                return None, "LOOPBACK_REVIEW_ORDER_INVALID"
            if operation == "revision" and (review_count != 1 or revision_count != 0):
                return None, "LOOPBACK_REVISION_ORDER_INVALID"
            if not source_hash_match:
                return None, "LOOPBACK_SOURCE_SHA256_MISMATCH"
            if not source_snapshot_match:
                return None, "LOOPBACK_SOURCE_SNAPSHOT_MISMATCH"
            if operation == "revision" and not answer_sentinel_present:
                return None, "LOOPBACK_ANSWER_SENTINEL_MISSING"
            record = {
                "request_index": len(self.requests) + 1,
                "byte_count": (
                    body_length
                    if body_length is not None
                    else len(
                        json.dumps(parsed, separators=(",", ":"), ensure_ascii=False).encode(
                            "utf-8"
                        )
                    )
                ),
                "path_exact": True,
                "stream": stream,
                "review": operation == "read_only_review",
                "revision": operation == "revision",
                "consent_gate_open": True,
                "source_sha256_match": source_hash_match,
                "source_snapshot_match": source_snapshot_match,
                "answer_sentinel_present": answer_sentinel_present,
                "request_arrival_sequence": request_arrival_sequence,
                "consent_click_sequence": consent_click_sequence,
                "request_after_consent_click": request_arrival_sequence > consent_click_sequence,
            }
            return record, operation

    def contract_evidence(self) -> dict[str, Any]:
        with self._lock:
            records = tuple(self.requests)
            review_records = tuple(record for record in records if record["review"])
            revision_records = tuple(record for record in records if record["revision"])
            if self._invalid_request:
                raise HarnessFailure(self._failure_code or "LOOPBACK_REQUEST_INVALID")
            if len(records) != 2 or len(review_records) != 1 or len(revision_records) != 1:
                raise HarnessFailure("LOOPBACK_REQUEST_SEQUENCE_INCOMPLETE")
            if records[0]["review"] is not True or records[1]["revision"] is not True:
                raise HarnessFailure("LOOPBACK_REQUEST_SEQUENCE_INVALID")
            if not all(
                record["request_after_consent_click"]
                and record["request_arrival_sequence"] > record["consent_click_sequence"]
                for record in records
            ):
                raise HarnessFailure("LOOPBACK_REQUEST_BEFORE_CONSENT_CLICK")
            return {
                "provider_request_count": len(records),
                "provider_review_count": len(review_records),
                "provider_revision_count": len(revision_records),
                "provider_paths_exact": all(record["path_exact"] for record in records),
                "provider_no_request_before_consent_click": all(
                    record["request_after_consent_click"] for record in records
                ),
                "provider_review_before_revision": records[0]["review"]
                and records[1]["revision"],
                "provider_review_source_sha256_match": review_records[0][
                    "source_sha256_match"
                ],
                "provider_revision_source_sha256_match": revision_records[0][
                    "source_sha256_match"
                ],
                "provider_revision_snapshot_match": revision_records[0][
                    "source_snapshot_match"
                ],
                "provider_revision_answer_sentinel_present": revision_records[0][
                    "answer_sentinel_present"
                ],
            }

    def start(self) -> "LoopbackRevisionServer":
        owner = self

        class Handler(http.server.BaseHTTPRequestHandler):
            def log_message(self, _format: str, *_args: Any) -> None:
                return

            def do_POST(self) -> None:  # noqa: N802
                request_arrival_sequence = owner._next_event_sequence()
                try:
                    if self.path != "/v1/responses":
                        owner._reject(self, "LOOPBACK_PATH_MISMATCH", 404)
                        return
                    try:
                        length = int(self.headers.get("Content-Length", "0"))
                    except (TypeError, ValueError):
                        owner._reject(self, "LOOPBACK_CONTENT_LENGTH_INVALID", 400)
                        return
                    if length < 0 or length > 8 * 1024 * 1024:
                        owner._reject(self, "LOOPBACK_REQUEST_TOO_LARGE", 413)
                        return
                    body = self.rfile.read(length)
                    try:
                        parsed = json.loads(body.decode("utf-8"))
                    except (UnicodeDecodeError, json.JSONDecodeError, RecursionError):
                        owner._reject(self, "LOOPBACK_REQUEST_JSON_INVALID", 400)
                        return
                    try:
                        record, operation = owner._validate_request(
                            self.path,
                            parsed,
                            len(body),
                            request_arrival_sequence,
                        )
                    except (RecursionError, ValueError):
                        owner._reject(self, "LOOPBACK_REQUEST_INVALID", 400)
                        return
                    if record is None:
                        owner._reject(self, operation, 409)
                        return
                    with owner._lock:
                        owner.requests.append(record)
                    response = (
                        revision_response(owner.source)
                        if operation == "revision"
                        else review_response()
                    )
                    streaming = operation == "revision"
                    if streaming:
                        response_body = owner._sse(response)
                        content_type = "text/event-stream"
                    else:
                        response_body = json.dumps(
                            {
                                "id": "resp-goal07-review",
                                "object": "response",
                                "status": "completed",
                                "model": "goal07-loopback-model",
                                "output": [
                                    {
                                        "type": "message",
                                        "id": "msg-goal07-review",
                                        "role": "assistant",
                                        "content": [
                                            {
                                                "type": "output_text",
                                                "text": response,
                                                "annotations": [],
                                            }
                                        ],
                                    }
                                ],
                            },
                            separators=(",", ":"),
                        ).encode("utf-8")
                        content_type = "application/json"
                    self.send_response(200)
                    self.send_header("Content-Type", content_type)
                    self.send_header("Content-Length", str(len(response_body)))
                    self.send_header("X-MarkTurbo-Goal07-Response-Sentinel", RAW_RESPONSE_SENTINEL)
                    self.send_header("Connection", "close")
                    self.end_headers()
                    self.wfile.write(response_body)
                    self.wfile.flush()
                except (BrokenPipeError, ConnectionResetError):
                    return

        self._server = _LoopbackHttpServer(("127.0.0.1", 0), Handler)
        self._thread = threading.Thread(target=self._server.serve_forever, daemon=True)
        self._thread.start()
        return self

    @property
    def base_url(self) -> str:
        if self._server is None:
            raise RuntimeError("loopback server is not started")
        host, port = self._server.server_address
        return f"http://{host}:{port}/v1/"

    @staticmethod
    def _sse(response: str) -> bytes:
        raw = response.encode("utf-8")
        split = len(raw) // 2
        chunks = (
            raw[:split].decode("utf-8", "ignore"),
            raw[split:].decode("utf-8", "ignore"),
        )
        # An SSE comment is ignored by the parser but gives the privacy scan a
        # raw-response sentinel to catch if transport diagnostics are leaked.
        events = [f": {RAW_RESPONSE_SENTINEL}\n\n"]
        for chunk in chunks:
            events.append(
                "event: response.output_text.delta\n"
                + "data: "
                + json.dumps(
                    {
                        "type": "response.output_text.delta",
                        "delta": chunk,
                        "output_index": 0,
                        "content_index": 0,
                    },
                    separators=(",", ":"),
                )
                + "\n\n"
            )
        events.append(
            "event: response.completed\n"
            + "data: "
            + json.dumps(
                {
                    "type": "response.completed",
                    "response": {
                        "id": "resp-goal07",
                        "object": "response",
                        "status": "completed",
                        "model": "goal07-loopback-model",
                        "output": [],
                    },
                },
                separators=(",", ":"),
            )
            + "\n\n"
        )
        return "".join(events).encode("utf-8")

    def close(self) -> None:
        if self._server is not None:
            self._server.shutdown()
            self._server.server_close()
            self._server = None
        if self._thread is not None:
            self._thread.join(timeout=2.0)
            self._thread = None

    def __enter__(self) -> "LoopbackRevisionServer":
        return self.start()

    def __exit__(self, _type: Any, _value: Any, _traceback: Any) -> None:
        self.close()
