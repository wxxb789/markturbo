"""Goal 07 native acceptance harness.

This harness drives the real Windows UI with a deterministic OpenAI Responses
loopback server. Each case has isolated data/config/workspace roots. The
server and evidence boundary never persist document text, request bodies, or
credentials; evidence contains only hashes, byte counts, booleans, and safe
reason codes.
"""

from __future__ import annotations

import argparse
import hashlib
import http.server
import json
import math
import threading
import time
import uuid
from pathlib import Path
from typing import Any
from urllib.parse import urlsplit

from .goal03 import Goal03Harness as ClipboardNativeHarness
from .runtime import (
    IMAGE_FILE_MACHINE_AMD64,
    INTEGRITY_NAMES,
    PE32_PLUS_MAGIC,
    REASON_CODE_RE,
    SAFE_FAILURE_TYPES,
    SHA256_RE,
    SOURCE_EDITOR_AUTOMATION_ID,
    TASK_DIALOG_BUTTON_CLASS,
    VK_A,
    VK_CONTROL,
    VK_S,
    Fingerprint,
    HarnessBlocked,
    HarnessFailure,
    NativeRunPlan,
    complete_evidence as complete_evidence_envelope,
    finite_nonnegative,
    fingerprint_bytes,
    fingerprint_text,
    key_input,
    load_pywinauto,
    main_native_acceptance,
    normalize_expected_hash,
    preflight,
    require_true,
    safe_exception_name,
    sha256_file,
    utc_now,
    validate_environment as validate_runtime_environment,
    validate_fingerprint,
    validate_process_context,
    wait_until,
    write_durable,
    run_native_acceptance,
)


REPO = Path(__file__).resolve().parents[3]
DEFAULT_EXE = REPO / "target" / "release" / "markturbo.exe"
DEFAULT_EVIDENCE = REPO / ".scratch" / "goal-07-native-acceptance-v1.json"
SCHEMA = "markturbo.goal-07-native-acceptance"
SCHEMA_VERSION = 1
MAX_EDIT_OUTPUT_BYTES = 4 * 1024 * 1024

EVIDENCE_KEYS = frozenset(
    {
        "schema",
        "schema_version",
        "status",
        "started_at_utc",
        "completed_at_utc",
        "transport",
        "executable",
        "environment",
        "cases",
        "summary",
    }
)
TRANSPORT_KEYS = frozenset({"mode", "provider", "deterministic", "request_count"})
EXECUTABLE_KEYS = frozenset(
    {
        "expected_sha256",
        "sha256",
        "byte_count",
        "hash_verified",
        "copied_sha256",
        "copy_hash_verified",
        "format",
        "machine",
        "machine_code",
        "optional_magic",
    }
)
ENVIRONMENT_KEYS = frozenset(
    {
        "platform",
        "windows_major",
        "windows_minor",
        "windows_build",
        "architecture",
        "native_machine_code",
        "python_pointer_bits",
        "wts_state",
        "active_console_session_id",
        "harness_is_console_session",
        "input_desktop",
        "thread_desktop",
        "harness_process",
    }
)
PROCESS_CONTEXT_KEYS = frozenset({"session_id", "integrity_rid", "integrity"})
CASE_KEYS = frozenset(
    {"id", "status", "duration_ms", "reason_code", "failure_type", "observations"}
)
SUMMARY_KEYS = frozenset(
    {
        "required_case_count",
        "passed_case_count",
        "blocked_case_count",
        "failed_case_count",
        "not_run_case_count",
    }
)
RUNTIME_SCAN_KEYS = frozenset(
    {
        "files_scanned",
        "app_logs_scanned",
        "config_files_scanned",
        "utf8_sentinel_absent",
        "utf16le_sentinel_absent",
        "ephemeral_credential_utf8_absent",
        "ephemeral_credential_utf16le_absent",
        "answer_sentinel_utf8_absent",
        "answer_sentinel_utf16le_absent",
        "raw_response_sentinel_utf8_absent",
        "raw_response_sentinel_utf16le_absent",
    }
)

REVIEW_RUN_ACCESSIBILITY_ID = "markturbo-review-run"
REVIEW_RESULT_ACCESSIBILITY_ID = "markturbo-review-result"
REVISION_RUN_ACCESSIBILITY_ID = "markturbo-revision-run"
REVISION_CONTROL_BOTTOM_INSET = 32
# Keep the request alias for callers that used the pre-Goal-07 name. The
# shipped UI exposes the revision action as `markturbo-revision-run`.
REVISION_REQUEST_ACCESSIBILITY_ID = REVISION_RUN_ACCESSIBILITY_ID
REVISION_RESULT_ACCESSIBILITY_ID = "markturbo-revision-result"
REVISION_PREVIEW_ACCESSIBILITY_ID = "markturbo-revision-preview"
REVISION_PREVIEW_SOURCE_ACCESSIBILITY_ID = "markturbo-revision-preview-source"
REVISION_PREVIEW_SOURCE_LABELS = frozenset(
    {"Final approved Revision source", "最终批准的修订源文本"}
)
REVISION_STALE_ACCESSIBILITY_ID = "markturbo-revision-stale"
REVISION_ACCEPT_ALL_ACCESSIBILITY_ID = "markturbo-revision-accept-all"
REVISION_REJECT_ALL_ACCESSIBILITY_ID = "markturbo-revision-reject-all"
REVISION_APPLY_ACCESSIBILITY_ID = "markturbo-revision-apply"
REVISION_COPY_ACCESSIBILITY_ID = "markturbo-revision-copy"
REVISION_QUESTION_PREFIX = "markturbo-revision-question-"
REVISION_CHANGE_PREFIX = "markturbo-revision-change-"
# Compatibility aliases for code that imports the old harness constants.
REVISION_ANSWER_PREFIX = REVISION_QUESTION_PREFIX
REVISION_ANSWER_UNSPECIFIED_PREFIX = REVISION_QUESTION_PREFIX
REVISION_HUNK_PREFIX = REVISION_CHANGE_PREFIX
CONFLICT_OVERWRITE_ACCESSIBILITY_ID = "markturbo-conflict-overwrite"
TRUST_AUTOMATION_ID = "markturbo-document-trust"
REVISION_CONSENT_AUTOMATION_ID = "CommandButton_1"
VK_SHIFT = 0x10
VK_R = 0x52

CASE_REJECT_ALL = "reject_all_byte_identity"
CASE_SELECTIVE_UNDO = "selective_apply_one_undo"
CASE_ACCEPT_ALL_PREVIEW = "accept_all_preview"
CASE_STALE = "stale_proposal_blocks_apply"
CASE_SAVE_CONFLICT = "safe_save_conflict"
CASE_TRUST_REVOKE = "trusted_executable_apply_revokes_trust"
REQUIRED_CASE_IDS = (
    CASE_REJECT_ALL,
    CASE_SELECTIVE_UNDO,
    CASE_ACCEPT_ALL_PREVIEW,
    CASE_STALE,
    CASE_SAVE_CONFLICT,
    CASE_TRUST_REVOKE,
)
CASE_FLOWS = {
    CASE_REJECT_ALL: "Review -> Revision -> reject all -> exact editor and source identity",
    CASE_SELECTIVE_UNDO: "Review -> Revision -> one hunk -> Apply -> one undo",
    CASE_ACCEPT_ALL_PREVIEW: "Review -> Revision -> accept all -> exact final preview before Apply",
    CASE_STALE: "Revision -> edit source -> stale visible -> Apply disabled",
    CASE_SAVE_CONFLICT: "Revision -> Apply -> external write -> safe Save conflict",
    CASE_TRUST_REVOKE: "trusted HTML -> executable Apply -> Restricted trust",
}

DOCUMENT_SENTINEL = "MTG07-NATIVE-REVISION-SENTINEL"
ANSWER_SENTINEL = "MTG07-NATIVE-ANSWER-SENTINEL"
RAW_RESPONSE_SENTINEL = "MTG07-NATIVE-RAW-RESPONSE-SENTINEL"
SOURCE_TEXT = (
    "---\r\n"
    "title: old\r\n"
    "owner: TBD\r\n"
    "---\r\n"
    "\r\n"
    "# Plan 计划 🚀\r\n"
    "\r\n"
    "\x60\x60\x60rust\r\n"
    "fn main() {}\r\n"
    "\x60\x60\x60\r\n"
    "\r\n"
    "[link](https://example.invalid)\r\n"
    f"\r\n<!-- {DOCUMENT_SENTINEL} -->\r\n"
)
SOURCE_BYTES = SOURCE_TEXT.encode("utf-8")
EDITOR_SOURCE_TEXT = SOURCE_TEXT.replace("\r\n", "\n").replace("\r", "\n")
EDITOR_SOURCE_BYTES = EDITOR_SOURCE_TEXT.encode("utf-8")
HTML_SOURCE_TEXT = (
    "<!doctype html>\r\n"
    "<script>window.goal07 = \"old\";</script>\r\n"
    "<p>trusted 文档 🚀</p>\r\n"
    f"<!-- {DOCUMENT_SENTINEL} -->\r\n"
)
HTML_SOURCE_BYTES = HTML_SOURCE_TEXT.encode("utf-8")
HTML_EDITOR_SOURCE_TEXT = HTML_SOURCE_TEXT.replace("\r\n", "\n").replace("\r", "\n")
HTML_EDITOR_SOURCE_BYTES = HTML_EDITOR_SOURCE_TEXT.encode("utf-8")
STALE_EDIT_TEXT = "---\ntitle: locally edited\nowner: TBD\n---\n"
EXTERNAL_SOURCE_BYTES = b"external writer won\r\n"
ANSWER_TEXT = f"Use the new title for the revised plan. {ANSWER_SENTINEL}"

QUESTION_0 = "Which title should the revised plan use?"
QUESTION_1 = "Should the owner field remain explicit?"
QUESTION_2 = "Which rollout detail is intentionally left open?"
QUESTION_IMPACT_0 = "A title choice changes the artifact's audience and identity."
QUESTION_IMPACT_1 = "An owner choice changes accountability for the first step."
QUESTION_IMPACT_2 = "A rollout choice changes the success evidence."

SAFE_STRINGS = frozenset(CASE_FLOWS.values()) | INTEGRITY_NAMES | {
    "keyless_loopback",
    "openai-responses",
    "Trusted",
    "Restricted",
}
ALLOWED_OBSERVATION_KEYS = {
    "accept_all_matches_expected",
    "copy_dirty_after",
    "copy_dirty_before",
    "copy_dirty_unchanged",
    "copy_source_after",
    "copy_source_before",
    "copy_source_unchanged",
    "copy_editor_after",
    "copy_editor_before",
    "copy_editor_unchanged",
    "copy_preview_fingerprint",
    "copy_preview_exact",
    "dirty_after",
    "dirty_before",
    "reject_all_dirty_unchanged",
    "apply_activation_attempted",
    "apply_disabled_source_contract",
    "editor_after",
    "editor_after_apply",
    "editor_after_edit",
    "editor_after_preview",
    "editor_before",
    "editor_before_apply",
    "external_source_after",
    "external_source_before",
    "final_preview",
    "flow",
    "foreground_verified",
    "loopback_provider",
    "one_undo_transaction",
    "preview_fingerprint",
    "proposal_received",
    "reviewed_source_match",
    "provider_request_count",
    "provider_review_count",
    "provider_revision_count",
    "provider_paths_exact",
    "provider_no_request_before_consent_click",
    "provider_review_before_revision",
    "provider_review_source_sha256_match",
    "provider_revision_source_sha256_match",
    "provider_revision_snapshot_match",
    "provider_revision_answer_sentinel_present",
    "process_context",
    "reject_all_byte_identity",
    "runtime_scan",
    "safe_save_conflict_visible",
    "safe_save_no_overwrite",
    "save_shortcut_sent",
    "conflict_visible_before_save",
    "selective_preview",
    "selective_matches_editor",
    "selective_matches_expected",
    "server_deterministic",
    "server_loopback",
    "source_after",
    "source_before",
    "stale_no_mutation",
    "stale_proposal_received",
    "status_text_observed",
    "trust_before",
    "restricted_after_apply",
    "executable_change",
    "undo_count",
    "utf16le_sentinel_absent",
    "utf8_sentinel_absent",
    "files_scanned",
    "app_logs_scanned",
    "config_files_scanned",
    "ephemeral_credential_utf8_absent",
    "ephemeral_credential_utf16le_absent",
    "byte_count",
    "sha256",
    "session_id",
    "integrity_rid",
    "integrity",
    "editor_after_undo",
    "stale_visible",
    "preview_inert_source_contract",
    "apply_control_observed",
    "apply_accessibility_reported_enabled",
    "editor_after_activation_attempt",
    "trust_revocation_order_source_contract",
    "executable_expected",
    "executable_matches_expected",
    "preview_matches_expected",
    "preview_contains_executable_text",
    "answer_sentinel_utf8_absent",
    "answer_sentinel_utf16le_absent",
    "raw_response_sentinel_utf8_absent",
    "raw_response_sentinel_utf16le_absent",
    "requested_hwnd",
    "foreground_hwnd",
    "show_window_return",
    "bring_to_top_return",
    "set_foreground_return",
    "foreground_attempts",
}


_ACTIVE_EVIDENCE: dict[str, Any] | None = None


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


def revision_question_accessibility_id(index: int) -> str:
    try:
        question_id = QUESTION_IDS[index]
    except IndexError as error:
        raise ValueError("native fixture question index is out of range") from error
    return f"{REVISION_QUESTION_PREFIX}{question_id}"


def revision_answer_accessibility_id(index: int, state: str) -> str:
    if state not in {"unanswered", "unspecified", "answered", "input"}:
        raise ValueError("native fixture answer state is invalid")
    return f"{revision_question_accessibility_id(index)}-{state}"


def revision_change_accessibility_id(change_id: int, decision: str) -> str:
    if decision not in {"accept", "reject"}:
        raise ValueError("native fixture change decision is invalid")
    return f"{REVISION_CHANGE_PREFIX}{change_id}-{decision}"


def endpoint_environment_key_identity(endpoint: str) -> str:
    """Mirror EndpointIdentity::credential_target without exposing a secret."""
    parsed = urlsplit(endpoint)
    scheme = parsed.scheme.casefold()
    host = parsed.hostname
    if (
        scheme not in {"http", "https"}
        or not host
        or parsed.username is not None
        or parsed.password is not None
        or parsed.query
        or parsed.fragment
    ):
        raise ValueError("loopback endpoint identity requires an HTTP(S) host")
    try:
        port = parsed.port or (80 if scheme == "http" else 443)
    except ValueError as error:
        raise ValueError("loopback endpoint identity has an invalid port") from error
    path = parsed.path or "/"
    if not path.endswith("/"):
        path += "/"
    digest = hashlib.sha256()
    for component in (
        "io.github.wxxb789.markturbo",
        "openai-responses",
        scheme,
        host.casefold(),
        str(port),
        path,
    ):
        digest.update(component.encode("utf-8"))
        digest.update(b"\0")
    return (
        "io.github.wxxb789.markturbo:model-credential:v2|wire=openai-responses"
        f"|host={host.casefold()}|identity-sha256={digest.hexdigest()}"
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
        values: list[str] = []
        if isinstance(value, dict):
            operation = value.get("operation")
            if isinstance(operation, str):
                values.append(operation)
            for nested in value.values():
                values.extend(LoopbackRevisionServer._operations(nested))
        elif isinstance(value, list):
            for nested in value:
                values.extend(LoopbackRevisionServer._operations(nested))
        elif isinstance(value, str) and value[:1] in {"{", "["}:
            try:
                values.extend(LoopbackRevisionServer._operations(json.loads(value)))
            except (json.JSONDecodeError, RecursionError):
                return values
        return values

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


def apply_edits(source: bytes, edits: list[dict[str, Any]]) -> bytes:
    """Apply validated non-overlapping byte edits for expected-preview checks."""
    if not isinstance(source, bytes) or len(source) > MAX_EDIT_OUTPUT_BYTES:
        raise ValueError("invalid native fixture edit")
    if not isinstance(edits, list):
        raise ValueError("invalid native fixture edit")
    try:
        decoded_source = source.decode("utf-8")
    except UnicodeDecodeError:
        raise ValueError("invalid native fixture edit") from None
    utf8_boundaries = {0}
    offset = 0
    for character in decoded_source:
        offset += len(character.encode("utf-8"))
        utf8_boundaries.add(offset)
    for edit in edits:
        if (
            not isinstance(edit, dict)
            or set(edit) != {"range", "expected_source", "replacement"}
            or not isinstance(edit["range"], dict)
            or set(edit["range"]) != {"start", "end"}
            or not isinstance(edit["range"].get("start"), int)
            or isinstance(edit["range"].get("start"), bool)
            or not isinstance(edit["range"].get("end"), int)
            or isinstance(edit["range"].get("end"), bool)
            or not isinstance(edit.get("expected_source"), str)
            or not isinstance(edit.get("replacement"), str)
        ):
            raise ValueError("invalid native fixture edit")
    ordered = sorted(edits, key=lambda edit: edit["range"]["start"])
    output = bytearray()
    cursor = 0
    seen_offsets: set[int] = set()
    for edit in ordered:
        if (
            not isinstance(edit, dict)
            or set(edit) != {"range", "expected_source", "replacement"}
            or not isinstance(edit["range"], dict)
            or set(edit["range"]) != {"start", "end"}
            or not isinstance(edit["range"].get("start"), int)
            or isinstance(edit["range"].get("start"), bool)
            or not isinstance(edit["range"].get("end"), int)
            or isinstance(edit["range"].get("end"), bool)
            or not isinstance(edit.get("expected_source"), str)
            or not isinstance(edit.get("replacement"), str)
        ):
            raise ValueError("invalid native fixture edit")
        start = edit["range"]["start"]
        end = edit["range"]["end"]
        if (
            start < 0
            or end < start
            or end > len(source)
            or start not in utf8_boundaries
            or end not in utf8_boundaries
            or start in seen_offsets
            or start < cursor
        ):
            raise ValueError("invalid native fixture edit")
        seen_offsets.add(start)
        expected = edit["expected_source"].encode("utf-8")
        replacement = edit["replacement"].encode("utf-8")
        if len(replacement) > MAX_EDIT_OUTPUT_BYTES:
            raise ValueError("invalid native fixture edit")
        if source[start:end] != expected:
            raise ValueError("invalid native fixture edit")
        if source[start:end] == replacement:
            raise ValueError("invalid native fixture edit")
        output.extend(source[cursor:start])
        output.extend(replacement)
        if len(output) + len(source) - end > MAX_EDIT_OUTPUT_BYTES:
            raise ValueError("invalid native fixture edit")
        cursor = end
    maximum_subset_size = len(source)
    for edit in ordered:
        start = edit["range"]["start"]
        end = edit["range"]["end"]
        replacement_size = len(edit["replacement"].encode("utf-8"))
        removed_size = end - start
        if replacement_size > removed_size:
            maximum_subset_size += replacement_size - removed_size
            if maximum_subset_size > MAX_EDIT_OUTPUT_BYTES:
                raise ValueError("invalid native fixture edit")
    output.extend(source[cursor:])
    if len(output) > MAX_EDIT_OUTPUT_BYTES:
        raise ValueError("invalid native fixture edit")
    return bytes(output)


def new_evidence(expected_hash: str) -> dict[str, Any]:
    global _ACTIVE_EVIDENCE
    evidence = {
        "schema": SCHEMA,
        "schema_version": SCHEMA_VERSION,
        "status": "BLOCKED",
        "started_at_utc": utc_now(),
        "completed_at_utc": None,
        "transport": {
            "mode": "keyless_loopback",
            "provider": "openai-responses",
            "deterministic": True,
            "request_count": 0,
        },
        "executable": {
            "expected_sha256": expected_hash,
            "sha256": None,
            "byte_count": None,
            "hash_verified": False,
            "copied_sha256": None,
            "copy_hash_verified": False,
            "format": None,
            "machine": None,
            "machine_code": None,
            "optional_magic": None,
        },
        "environment": {},
        "cases": [
            {
                "id": case_id,
                "status": "NOT_RUN",
                "duration_ms": None,
                "reason_code": None,
                "failure_type": None,
                "observations": {},
            }
            for case_id in REQUIRED_CASE_IDS
        ],
        "summary": {},
    }
    _ACTIVE_EVIDENCE = evidence
    return evidence


def complete_evidence(evidence: dict[str, Any], status: str) -> None:
    complete_evidence_envelope(evidence, status, REQUIRED_CASE_IDS)


PRIVATE_SENTINELS = (DOCUMENT_SENTINEL, ANSWER_SENTINEL, RAW_RESPONSE_SENTINEL)
PRIVATE_KEYS = frozenset({"request_body", "credential", "api_key", "api-key"})

FINGERPRINT_KEYS = frozenset({"byte_count", "sha256"})
FINGERPRINT_OBSERVATION_KEYS = frozenset(
    {
        "editor_after",
        "editor_after_apply",
        "editor_after_activation_attempt",
        "editor_after_edit",
        "editor_after_preview",
        "editor_after_undo",
        "editor_before",
        "editor_before_apply",
        "executable_expected",
        "external_source_after",
        "external_source_before",
        "final_preview",
        "preview_fingerprint",
        "selective_preview",
        "source_after",
        "source_before",
        "copy_editor_after",
        "copy_editor_before",
        "copy_preview_fingerprint",
        "copy_source_after",
        "copy_source_before",
    }
)
STRUCTURED_OBSERVATION_KEYS = FINGERPRINT_OBSERVATION_KEYS | {
    "process_context",
    "runtime_scan",
}


def reject_private_content(value: Any) -> None:
    """Reject content-bearing or credential-shaped values at every nesting level."""
    if isinstance(value, dict):
        for key, nested in value.items():
            if not isinstance(key, str) or key.casefold() in PRIVATE_KEYS:
                raise ValueError("evidence contains private content")
            folded_key = key.casefold()
            if any(sentinel.casefold() in folded_key for sentinel in PRIVATE_SENTINELS):
                raise ValueError("free-form observation strings are forbidden")
            reject_private_content(nested)
    elif isinstance(value, list):
        for nested in value:
            reject_private_content(nested)
    elif isinstance(value, str):
        folded_value = value.casefold()
        if any(sentinel.casefold() in folded_value for sentinel in PRIVATE_SENTINELS):
            raise ValueError("free-form observation strings are forbidden")


def require_exact_keys(value: Any, expected: frozenset[str], label: str) -> None:
    if not isinstance(value, dict) or set(value) != expected:
        raise ValueError(f"invalid {label} keys")


def validate_observations(value: Any, key: str | None = None) -> None:
    if isinstance(value, dict):
        if key == "process_context":
            require_exact_keys(value, PROCESS_CONTEXT_KEYS, "process context")
            validate_process_context(value)
            return
        if key == "runtime_scan":
            validate_runtime_scan(value)
            return
        if key in FINGERPRINT_OBSERVATION_KEYS:
            require_exact_keys(value, FINGERPRINT_KEYS, f"{key} fingerprint")
            validate_fingerprint(value)
            return
        for nested_key, nested in value.items():
            if not isinstance(nested_key, str):
                raise ValueError("unknown observation field")
            if nested_key.casefold() in PRIVATE_KEYS:
                raise ValueError("evidence contains private content")
            if nested_key not in ALLOWED_OBSERVATION_KEYS:
                raise ValueError("unknown observation field")
            if isinstance(nested, dict) and nested_key not in STRUCTURED_OBSERVATION_KEYS:
                raise ValueError("invalid nested observation structure")
            if nested_key == "sha256" and (
                not isinstance(nested, str) or not SHA256_RE.fullmatch(nested)
            ):
                raise ValueError("invalid observation SHA-256")
            validate_observations(nested, nested_key)
    elif isinstance(value, list):
        for nested in value:
            validate_observations(nested, key)
    elif isinstance(value, str):
        if key != "sha256" and value not in SAFE_STRINGS:
            raise ValueError("free-form observation strings are forbidden")
    elif value is not None and not isinstance(value, (bool, int, float)):
        raise ValueError("invalid observation value")


def validate_runtime_scan(value: Any) -> None:
    if not isinstance(value, dict):
        raise ValueError("missing runtime scan evidence")
    require_exact_keys(value, RUNTIME_SCAN_KEYS, "runtime scan")
    for key in ("files_scanned", "app_logs_scanned", "config_files_scanned"):
        if not isinstance(value.get(key), int) or isinstance(value[key], bool) or value[key] < 0:
            raise ValueError(f"invalid {key}")
    if (
        value["files_scanned"] <= 0
        or value["app_logs_scanned"] <= 0
        or value["config_files_scanned"] <= 0
    ):
        raise ValueError("runtime scan must cover an application log")
    if (
        value["app_logs_scanned"] > value["files_scanned"]
        or value["config_files_scanned"] > value["files_scanned"]
        or value["app_logs_scanned"] + value["config_files_scanned"]
        > value["files_scanned"]
    ):
        raise ValueError("runtime scan counts are inconsistent")
    if value["files_scanned"] < value["app_logs_scanned"] + value["config_files_scanned"] + 1:
        raise ValueError("runtime scan file count is incomplete")
    require_true(value, "utf8_sentinel_absent", "utf16le_sentinel_absent")
    require_true(
        value,
        "ephemeral_credential_utf8_absent",
        "ephemeral_credential_utf16le_absent",
        "answer_sentinel_utf8_absent",
        "answer_sentinel_utf16le_absent",
        "raw_response_sentinel_utf8_absent",
        "raw_response_sentinel_utf16le_absent",
    )


def validate_environment(value: Any) -> dict[str, Any]:
    if not isinstance(value, dict):
        raise ValueError("invalid environment evidence")
    require_exact_keys(value, ENVIRONMENT_KEYS, "environment")
    require_exact_keys(value["harness_process"], PROCESS_CONTEXT_KEYS, "process context")
    validate_runtime_environment(value)
    return validate_process_context(value.get("harness_process"))


def required_observations(case_id: str) -> set[str]:
    common = {
        "flow",
        "process_context",
        "foreground_verified",
        "runtime_scan",
        "loopback_provider",
        "server_loopback",
        "server_deterministic",
        "provider_request_count",
        "provider_review_count",
        "provider_revision_count",
        "provider_paths_exact",
        "provider_no_request_before_consent_click",
        "provider_review_before_revision",
        "provider_review_source_sha256_match",
        "provider_revision_source_sha256_match",
        "provider_revision_snapshot_match",
        "provider_revision_answer_sentinel_present",
    }
    specifics = {
        CASE_REJECT_ALL: {
            "proposal_received",
            "reviewed_source_match",
            "editor_before",
            "editor_after",
            "source_before",
            "source_after",
            "reject_all_byte_identity",
            "dirty_before",
            "dirty_after",
            "reject_all_dirty_unchanged",
        },
        CASE_SELECTIVE_UNDO: {
            "editor_before",
            "selective_preview",
            "editor_after_apply",
            "editor_after_undo",
            "source_before",
            "source_after",
            "selective_matches_editor",
            "selective_matches_expected",
            "one_undo_transaction",
            "undo_count",
        },
        CASE_ACCEPT_ALL_PREVIEW: {
            "editor_before_apply",
            "editor_after_preview",
            "preview_fingerprint",
            "final_preview",
            "accept_all_matches_expected",
            "copy_preview_fingerprint",
            "copy_preview_exact",
            "copy_editor_before",
            "copy_editor_after",
            "copy_editor_unchanged",
            "copy_dirty_before",
            "copy_dirty_after",
            "copy_dirty_unchanged",
            "copy_source_before",
            "copy_source_after",
            "copy_source_unchanged",
        },
        CASE_STALE: {
            "editor_before",
            "editor_after_edit",
            "source_before",
            "source_after",
            "stale_proposal_received",
            "stale_visible",
            "apply_activation_attempted",
            "apply_disabled_source_contract",
            "stale_no_mutation",
            "apply_control_observed",
            "apply_accessibility_reported_enabled",
            "editor_after_activation_attempt",
        },
        CASE_SAVE_CONFLICT: {
            "editor_after_apply",
            "external_source_before",
            "external_source_after",
            "save_shortcut_sent",
            "conflict_visible_before_save",
            "safe_save_conflict_visible",
            "safe_save_no_overwrite",
        },
        CASE_TRUST_REVOKE: {
            "trust_before",
            "editor_after_apply",
            "restricted_after_apply",
            "executable_change",
            "executable_expected",
            "executable_matches_expected",
            "trust_revocation_order_source_contract",
            "preview_inert_source_contract",
            "preview_fingerprint",
            "preview_matches_expected",
            "preview_contains_executable_text",
        },
    }
    return common | specifics[case_id]


def validate_passed_case(case_id: str, observations: dict[str, Any], parent: dict[str, Any]) -> None:
    if observations.get("flow") != CASE_FLOWS[case_id]:
        raise ValueError("case flow mechanics do not match the required scenario")
    require_exact_keys(observations.get("process_context"), PROCESS_CONTEXT_KEYS, "process context")
    if validate_process_context(observations.get("process_context")) != parent:
        raise ValueError("case process context differs from harness context")
    require_true(
        observations,
        "foreground_verified",
        "loopback_provider",
        "server_loopback",
        "server_deterministic",
        "provider_paths_exact",
        "provider_no_request_before_consent_click",
        "provider_review_before_revision",
        "provider_review_source_sha256_match",
        "provider_revision_source_sha256_match",
        "provider_revision_snapshot_match",
        "provider_revision_answer_sentinel_present",
    )
    validate_runtime_scan(observations.get("runtime_scan"))
    if (
        not isinstance(observations.get("provider_request_count"), int)
        or observations["provider_request_count"] != 2
        or observations.get("provider_review_count") != 1
        or observations.get("provider_revision_count") != 1
    ):
        raise ValueError("loopback provider did not receive exactly one Review and Revision request")

    if case_id == CASE_REJECT_ALL:
        before = validate_fingerprint(observations["editor_before"])
        after = validate_fingerprint(observations["editor_after"])
        source_before = validate_fingerprint(observations["source_before"])
        source_after = validate_fingerprint(observations["source_after"])
        require_true(
            observations,
            "proposal_received",
            "reviewed_source_match",
            "reject_all_byte_identity",
            "reject_all_dirty_unchanged",
        )
        if (
            before != after
            or source_before != source_after
            or observations.get("dirty_before") is not observations.get("dirty_after")
        ):
            raise ValueError("reject-all changed editor or source bytes")
    elif case_id == CASE_SELECTIVE_UNDO:
        before = validate_fingerprint(observations["editor_before"])
        selected = validate_fingerprint(observations["selective_preview"])
        applied = validate_fingerprint(observations["editor_after_apply"])
        undone = validate_fingerprint(observations["editor_after_undo"])
        source_before = validate_fingerprint(observations["source_before"])
        source_after = validate_fingerprint(observations["source_after"])
        require_true(
            observations,
            "selective_matches_editor",
            "selective_matches_expected",
            "one_undo_transaction",
        )
        if applied != selected or undone != before or source_before != source_after:
            raise ValueError("selective Apply or one undo did not preserve exact bytes")
        if observations.get("undo_count") != 1:
            raise ValueError("selective Apply must be restored by exactly one undo")
    elif case_id == CASE_ACCEPT_ALL_PREVIEW:
        before = validate_fingerprint(observations["editor_before_apply"])
        after = validate_fingerprint(observations["editor_after_preview"])
        preview = validate_fingerprint(observations["preview_fingerprint"])
        final = validate_fingerprint(observations["final_preview"])
        copied = validate_fingerprint(observations["copy_preview_fingerprint"])
        copy_before = validate_fingerprint(observations["copy_editor_before"])
        copy_after = validate_fingerprint(observations["copy_editor_after"])
        copy_source_before = validate_fingerprint(observations["copy_source_before"])
        copy_source_after = validate_fingerprint(observations["copy_source_after"])
        require_true(observations, "accept_all_matches_expected")
        require_true(
            observations,
            "copy_preview_exact",
            "copy_editor_unchanged",
            "copy_dirty_unchanged",
            "copy_source_unchanged",
        )
        if (
            before != after
            or preview != final
            or copied != final
            or copy_before != copy_after
            or copy_source_before != copy_source_after
            or observations.get("copy_dirty_before")
            is not observations.get("copy_dirty_after")
        ):
            raise ValueError("accept-all preview was not stable before Apply")
    elif case_id == CASE_STALE:
        before = validate_fingerprint(observations["editor_before"])
        edited = validate_fingerprint(observations["editor_after_edit"])
        after_observation = validate_fingerprint(observations["editor_after_activation_attempt"])
        source_before = validate_fingerprint(observations["source_before"])
        source_after = validate_fingerprint(observations["source_after"])
        require_true(
            observations,
            "stale_proposal_received",
            "stale_visible",
            "apply_activation_attempted",
            "apply_disabled_source_contract",
            "stale_no_mutation",
            "apply_control_observed",
        )
        if (
            before == edited
            or edited != after_observation
            or source_before != source_after
        ):
            raise ValueError("stale proposal did not remain fail-closed")
    elif case_id == CASE_SAVE_CONFLICT:
        before = validate_fingerprint(observations["external_source_before"])
        after = validate_fingerprint(observations["external_source_after"])
        require_true(
            observations,
            "save_shortcut_sent",
            "safe_save_conflict_visible",
            "safe_save_no_overwrite",
        )
        if before != after:
            raise ValueError("safe-save conflict overwrote the external source")
        validate_fingerprint(observations["editor_after_apply"])
    elif case_id == CASE_TRUST_REVOKE:
        before = validate_fingerprint(observations["editor_before_apply"])
        actual = validate_fingerprint(observations["editor_after_apply"])
        expected = validate_fingerprint(observations["executable_expected"])
        preview = validate_fingerprint(observations["preview_fingerprint"])
        require_true(
            observations,
            "trust_before",
            "restricted_after_apply",
            "executable_change",
            "executable_matches_expected",
            "trust_revocation_order_source_contract",
            "preview_inert_source_contract",
            "preview_matches_expected",
            "preview_contains_executable_text",
        )
        if preview != expected or actual != expected or before == actual:
            raise ValueError("trusted executable Apply did not produce the expected restricted change")



def validate_evidence(evidence: dict[str, Any]) -> None:
    if not isinstance(evidence, dict):
        raise ValueError("invalid top-level evidence")
    require_exact_keys(evidence, EVIDENCE_KEYS, "top-level evidence")
    if evidence.get("schema") != SCHEMA or evidence.get("schema_version") != SCHEMA_VERSION:
        raise ValueError("unsupported evidence schema")
    if evidence.get("status") not in {"PASS", "FAIL", "BLOCKED"}:
        raise ValueError("invalid evidence status")
    transport = evidence.get("transport")
    if (
        not isinstance(transport, dict)
        or set(transport) != TRANSPORT_KEYS
        or transport.get("mode") != "keyless_loopback"
        or transport.get("provider") != "openai-responses"
        or transport.get("deterministic") is not True
        or not isinstance(transport.get("request_count"), int)
        or isinstance(transport.get("request_count"), bool)
        or transport["request_count"] < 0
    ):
        raise ValueError("invalid loopback transport evidence")
    reject_private_content(evidence)
    cases_for_transport = evidence.get("cases")
    if isinstance(cases_for_transport, list):
        observed_request_count = sum(
            int(case.get("observations", {}).get("provider_request_count", 0))
            for case in cases_for_transport
            if isinstance(case, dict)
            and isinstance(case.get("observations"), dict)
            and isinstance(case["observations"].get("provider_request_count"), int)
            and not isinstance(case["observations"].get("provider_request_count"), bool)
        )
        if transport["request_count"] != observed_request_count:
            raise ValueError("top-level transport request count is not synchronized")

    executable = evidence.get("executable")
    if not isinstance(executable, dict):
        raise ValueError("missing executable evidence")
    require_exact_keys(executable, EXECUTABLE_KEYS, "executable")
    expected = executable.get("expected_sha256")
    if not isinstance(expected, str) or not SHA256_RE.fullmatch(expected):
        raise ValueError("invalid expected executable SHA-256")
    for key in ("sha256", "copied_sha256"):
        value = executable.get(key)
        if value is not None and (
            not isinstance(value, str) or not SHA256_RE.fullmatch(value)
        ):
            raise ValueError(f"invalid executable {key}")
    if executable.get("hash_verified") is True and executable.get("sha256") != expected:
        raise ValueError("verified executable hash does not match expected hash")
    if executable.get("copy_hash_verified") is True and executable.get("copied_sha256") != expected:
        raise ValueError("verified copied executable hash does not match expected hash")
    if evidence["status"] == "PASS":
        if not executable.get("hash_verified") or not executable.get("copy_hash_verified"):
            raise ValueError("PASS requires verified executable hashes")
        if executable.get("format") != "PE32+" or executable.get("machine") != "x86_64":
            raise ValueError("PASS requires an AMD64 PE32+ executable")
        if (
            executable.get("machine_code") != IMAGE_FILE_MACHINE_AMD64
            or executable.get("optional_magic") != PE32_PLUS_MAGIC
        ):
            raise ValueError("PASS requires an AMD64 PE32+ executable")
        if (
            not isinstance(executable.get("byte_count"), int)
            or isinstance(executable["byte_count"], bool)
            or executable["byte_count"] <= 0
        ):
            raise ValueError("PASS requires a nonempty executable")

    cases = evidence.get("cases")
    if not isinstance(cases, list) or len(cases) != len(REQUIRED_CASE_IDS):
        raise ValueError("required case set is incomplete")
    ids = [case.get("id") for case in cases if isinstance(case, dict)]
    if len(set(ids)) != len(ids) or set(ids) != set(REQUIRED_CASE_IDS):
        raise ValueError("required case set is incomplete")
    parent = None
    if evidence["status"] == "PASS" or any(
        case.get("status") == "PASS" for case in cases if isinstance(case, dict)
    ):
        parent = validate_environment(evidence.get("environment"))
    for case in cases:
        require_exact_keys(case, CASE_KEYS, "case")
        status = case.get("status")
        if status not in {"PASS", "FAIL", "BLOCKED", "NOT_RUN"}:
            raise ValueError("invalid case status")
        reason = case.get("reason_code")
        if reason is not None and (
            not isinstance(reason, str) or not REASON_CODE_RE.fullmatch(reason)
        ):
            raise ValueError("invalid case reason code")
        failure_type = case.get("failure_type")
        if failure_type is not None and (
            not isinstance(failure_type, str) or failure_type not in SAFE_FAILURE_TYPES
        ):
            raise ValueError("invalid case failure type")
        if status != "FAIL" and failure_type is not None:
            raise ValueError("only failed cases may contain a failure type")
        duration = case.get("duration_ms")
        if status == "NOT_RUN":
            if duration is not None:
                raise ValueError("NOT_RUN case cannot have a duration")
        elif not finite_nonnegative(duration):
            raise ValueError("completed case duration must be nonnegative")
        observations = case.get("observations")
        if not isinstance(observations, dict):
            raise ValueError("invalid case observations")
        validate_observations(observations)
        if status == "PASS":
            if not required_observations(case["id"]).issubset(observations):
                raise ValueError("passed case evidence is incomplete")
            if parent is None:
                raise ValueError("passed case requires validated environment evidence")
            validate_passed_case(case["id"], observations, parent)
    if evidence["status"] == "PASS" and any(case["status"] != "PASS" for case in cases):
        raise ValueError("PASS requires every required case to pass")
    summary = evidence.get("summary")
    if not isinstance(summary, dict):
        raise ValueError("invalid case summary")
    require_exact_keys(summary, SUMMARY_KEYS, "case summary")
    expected_summary = {
        "required_case_count": len(REQUIRED_CASE_IDS),
        "passed_case_count": sum(case["status"] == "PASS" for case in cases),
        "blocked_case_count": sum(case["status"] == "BLOCKED" for case in cases),
        "failed_case_count": sum(case["status"] == "FAIL" for case in cases),
        "not_run_case_count": sum(case["status"] == "NOT_RUN" for case in cases),
    }
    if evidence.get("summary") != expected_summary:
        raise ValueError("case summary does not match case evidence")
    if evidence.get("environment") != {}:
        require_exact_keys(evidence.get("environment"), ENVIRONMENT_KEYS, "environment")


def production_source(path: Path) -> str:
    return path.read_text(encoding="utf-8").split("\n#[cfg(test)]", 1)[0]


def trust_apply_source_contract_ok() -> bool:
    """Check the production ordering that makes trusted Apply fail closed."""
    document = production_source(REPO / "crates" / "mt-app" / "src" / "views" / "document.rs")
    start = document.find("pub fn apply_approved_revision")
    if start < 0:
        return False
    end = document.find("\n    ///", start + 1)
    body = document[start:] if end < 0 else document[start:end]
    markers = (
        "let revoke_trust =",
        "self.trust == Trust::Trusted",
        "matches!(self.document.doc_type(),",
        "DocType::Html | DocType::Mdx",
        "if revoke_trust {",
        "self.trust = Trust::Restricted;",
        "self.rebuild_web(cx);",
        "self.replace_text(final_text, window, cx);",
    )
    if any(marker not in body for marker in markers):
        return False
    revoke = body.find("let revoke_trust =")
    branch = body.find("if revoke_trust {")
    trust = body.find("self.trust = Trust::Restricted;")
    rebuild = body.find("self.rebuild_web(cx);", trust)
    replace = body.find("self.replace_text(final_text, window, cx);", rebuild)
    return (
        revoke >= 0
        and branch > revoke
        and trust > branch
        and rebuild > trust
        and replace > rebuild
    )


def preview_inert_source_contract_ok() -> bool:
    """Check that the approved preview is rendered as inert GPUI text."""
    workspace = production_source(REPO / "crates" / "mt-app" / "src" / "views" / "workspace.rs")
    start = workspace.find("self.revision_preview().unwrap_or_default()")
    if start < 0:
        return False
    end = workspace.find("for coverage in revision.result.question_coverage()", start)
    body = workspace[start:] if end < 0 else workspace[start:end]
    required = (
        'accessibility_id("markturbo-revision-preview")',
        'accessibility_id("markturbo-revision-preview-source")',
        ".role(gpui::Role::Label)",
        ".aria_value(preview.clone())",
        ".child(preview)",
    )
    forbidden = ("WebSurface", "WebView", "rebuild_web", "render_html", "set_html")
    return all(marker in body for marker in required) and not any(
        marker in body for marker in forbidden
    )


def stale_accessibility_source_contract_ok() -> bool:
    """Check that the stale warning is exposed as an inert UIA text node."""
    workspace = production_source(REPO / "crates" / "mt-app" / "src" / "views" / "workspace.rs")
    start = workspace.find('.id("revision-stale")')
    if start < 0:
        return False
    end = workspace.find(".into_any_element()", start)
    if end < 0:
        return False
    body = workspace[start:end]
    return all(
        marker in body
        for marker in (
            "accessibility_id(REVISION_STALE_ACCESSIBILITY_ID)",
            ".role(gpui::Role::Label)",
            ".aria_label(i18n::t(i18n::Key::RevisionStaleInspection, cx))",
        )
    )


def stale_apply_source_contract_ok() -> bool:
    """Check that the stale state disables the rendered Apply command."""
    workspace = production_source(REPO / "crates" / "mt-app" / "src" / "views" / "workspace.rs")
    start = workspace.find('Button::new("revision-apply")')
    if start < 0:
        return False
    end = workspace.find(".into_any_element()", start)
    if end < 0:
        return False
    body = workspace[start:end]
    return all(
        marker in body
        for marker in (
            "accessibility_id(REVISION_APPLY_ACCESSIBILITY_ID)",
            ".disabled(revision_stale)",
            "this.apply_revision(window, cx)",
        )
    )


def source_contract_failure() -> str | None:
    workspace = production_source(REPO / "crates" / "mt-app" / "src" / "views" / "workspace.rs")
    document = production_source(REPO / "crates" / "mt-app" / "src" / "views" / "document.rs")
    for value in (
        REVIEW_RUN_ACCESSIBILITY_ID,
        REVIEW_RESULT_ACCESSIBILITY_ID,
        REVISION_REQUEST_ACCESSIBILITY_ID,
        REVISION_PREVIEW_ACCESSIBILITY_ID,
        REVISION_PREVIEW_SOURCE_ACCESSIBILITY_ID,
        REVISION_STALE_ACCESSIBILITY_ID,
        REVISION_ACCEPT_ALL_ACCESSIBILITY_ID,
        REVISION_REJECT_ALL_ACCESSIBILITY_ID,
        REVISION_APPLY_ACCESSIBILITY_ID,
        REVISION_COPY_ACCESSIBILITY_ID,
        REVISION_CHANGE_PREFIX,
    ):
        if value not in workspace:
            return "REVISION_UIA_CONTRACT_MISSING"
    for contract in (
        "accessibility_id(REVISION_RUN_ACCESSIBILITY_ID)",
        'accessibility_id("markturbo-revision-preview")',
        'accessibility_id("markturbo-revision-preview-source")',
        "accessibility_id(REVISION_STALE_ACCESSIBILITY_ID)",
        "accessibility_id(REVISION_ACCEPT_ALL_ACCESSIBILITY_ID)",
        "accessibility_id(REVISION_REJECT_ALL_ACCESSIBILITY_ID)",
        "accessibility_id(REVISION_APPLY_ACCESSIBILITY_ID)",
        "accessibility_id(REVISION_COPY_ACCESSIBILITY_ID)",
        "revision_question_binding_id(index, question)",
        '"{question_id}-answered"',
        '"{question_id}-input"',
        '"markturbo-revision-change-{}"',
        "apply_approved_revision",
        "Revision",
    ):
        if contract not in workspace:
            return "REVISION_SOURCE_CONTRACT_MISSING"
    for contract in (
        "DocumentEvent::Conflict",
        'Button::new("trust")',
        "accessibility_id(DOCUMENT_TRUST_ACCESSIBILITY_ID)",
        '"markturbo-document-trust"',
        "Trust::Trusted",
        "CONFLICT_OVERWRITE_ACCESSIBILITY_ID",
        '"markturbo-conflict-overwrite"',
    ):
        if contract not in document and contract not in workspace:
            return "REVISION_BOUNDARY_CONTRACT_MISSING"
    if not trust_apply_source_contract_ok():
        return "REVISION_TRUST_SOURCE_CONTRACT_MISSING"
    if not preview_inert_source_contract_ok():
        return "REVISION_PREVIEW_INERT_CONTRACT_MISSING"
    if not stale_accessibility_source_contract_ok():
        return "REVISION_STALE_ACCESSIBILITY_CONTRACT_MISSING"
    if not stale_apply_source_contract_ok():
        return "REVISION_STALE_APPLY_CONTRACT_MISSING"
    return None


def artifact_contains(path: Path, patterns: tuple[bytes, ...]) -> bytes | None:
    overlap = max(len(pattern) for pattern in patterns) - 1
    tail = b""
    with path.open("rb") as handle:
        while chunk := handle.read(1024 * 1024):
            value = tail + chunk
            for pattern in patterns:
                if pattern in value:
                    return pattern
            tail = value[-overlap:] if overlap else b""
    return None


def scan_case_artifacts(case_root: Path, ephemeral_credential: str) -> dict[str, Any]:
    try:
        roots = (case_root / "data", case_root / "config")
        paths = [path for root in roots for path in root.rglob("*") if path.is_file()]
        stderr = case_root / "stderr.log"
        if stderr.exists():
            paths.append(stderr)
        paths = sorted(set(paths))
    except OSError as error:
        raise HarnessFailure("RUNTIME_ARTIFACT_ENUM_FAILED", safe_exception_name(error)) from None
    if not paths:
        raise HarnessFailure("RUNTIME_ARTIFACTS_MISSING")
    logs_root = case_root / "data" / "logs"
    app_logs = [path for path in paths if logs_root in path.parents]
    if not app_logs:
        raise HarnessFailure("APP_LOG_MISSING")
    config_root = case_root / "config"
    config_files = [path for path in paths if config_root in path.parents]
    patterns = (
        DOCUMENT_SENTINEL.encode("utf-8"),
        DOCUMENT_SENTINEL.encode("utf-16-le"),
        ephemeral_credential.encode("utf-8"),
        ephemeral_credential.encode("utf-16-le"),
        ANSWER_SENTINEL.encode("utf-8"),
        ANSWER_SENTINEL.encode("utf-16-le"),
        RAW_RESPONSE_SENTINEL.encode("utf-8"),
        RAW_RESPONSE_SENTINEL.encode("utf-16-le"),
    )
    for path in paths:
        try:
            leaked = artifact_contains(path, patterns)
        except OSError as error:
            raise HarnessFailure("RUNTIME_ARTIFACT_SCAN_FAILED", safe_exception_name(error)) from None
        if leaked == patterns[0]:
            raise HarnessFailure("UTF8_DOCUMENT_SENTINEL_LEAKED")
        if leaked == patterns[1]:
            raise HarnessFailure("UTF16LE_DOCUMENT_SENTINEL_LEAKED")
        if leaked == patterns[2]:
            raise HarnessFailure("UTF8_EPHEMERAL_CREDENTIAL_LEAKED")
        if leaked == patterns[3]:
            raise HarnessFailure("UTF16LE_EPHEMERAL_CREDENTIAL_LEAKED")
        if leaked == patterns[4]:
            raise HarnessFailure("UTF8_ANSWER_SENTINEL_LEAKED")
        if leaked == patterns[5]:
            raise HarnessFailure("UTF16LE_ANSWER_SENTINEL_LEAKED")
        if leaked == patterns[6]:
            raise HarnessFailure("UTF8_RAW_RESPONSE_SENTINEL_LEAKED")
        if leaked == patterns[7]:
            raise HarnessFailure("UTF16LE_RAW_RESPONSE_SENTINEL_LEAKED")
    if not config_files:
        raise HarnessFailure("CONFIG_FILE_MISSING")
    if len(paths) < len(app_logs) + len(config_files) + 1:
        raise HarnessFailure("RUNTIME_FILE_COUNT_INCOMPLETE")
    return {
        "files_scanned": len(paths),
        "app_logs_scanned": len(app_logs),
        "config_files_scanned": len(config_files),
        "utf8_sentinel_absent": True,
        "utf16le_sentinel_absent": True,
        "ephemeral_credential_utf8_absent": True,
        "ephemeral_credential_utf16le_absent": True,
        "answer_sentinel_utf8_absent": True,
        "answer_sentinel_utf16le_absent": True,
        "raw_response_sentinel_utf8_absent": True,
        "raw_response_sentinel_utf16le_absent": True,
    }


def review_settings_document(endpoint: str) -> bytes:
    return (
        "show-welcome-on-startup = false\n"
        'model-provider = "openai-responses"\n'
        'model-name = "goal07-loopback-model"\n'
        f"model-base-url = {json.dumps(endpoint)}\n"
        f"model-environment-key-identity = {json.dumps(endpoint_environment_key_identity(endpoint))}\n"
    ).encode("utf-8")


class Goal07Harness(ClipboardNativeHarness):
    def __init__(self, *args: Any) -> None:
        super().__init__(*args)
        self._credential = f"markturbo-goal07-{uuid.uuid4().hex}"

    def openai_api_key_for_child(self) -> str | None:
        return self._credential

    def profile(
        self, case_id: str, endpoint: str, *, html: bool = False
    ) -> tuple[Path, Path, Path, Path, Path, Path]:
        target = endpoint_environment_key_identity(endpoint)
        if self.win32.persistent_credential_target_exists(target):
            raise HarnessBlocked("PERSISTENT_CREDENTIAL_PRESENT")
        data_root, config_root, workspace_root, stderr_path = self.case_roots(case_id)
        case_root = data_root.parent
        source = (workspace_root / ("trusted.html" if html else "revision.md")).resolve()
        write_durable(source, HTML_SOURCE_BYTES if html else SOURCE_BYTES)
        write_durable(config_root / "settings.toml", review_settings_document(endpoint))
        return case_root, data_root, config_root, workspace_root, stderr_path, source

    def _text_control(self, app: Any, automation_id: str, control_type: str = "Text") -> str:
        control = self.find_control(
            app,
            automation_id,
            control_type,
            f"{automation_id.upper().replace('-', '_')}_UIA_TIMEOUT",
            f"{automation_id.upper().replace('-', '_')}_UIA_CONTRACT_MISMATCH",
        )
        try:
            if control_type == "Edit":
                return str(control.iface_value.CurrentValue)
            value = control.element_info.name
            if isinstance(value, str) and value:
                return value
            pattern = control.iface_value
            return str(pattern.CurrentValue)
        except Exception as error:
            raise HarnessFailure("REVISION_TEXT_UIA_QUERY_FAILED", safe_exception_name(error)) from None

    def _fingerprint_control(self, app: Any, automation_id: str) -> Fingerprint:
        return fingerprint_text(self._text_control(app, automation_id))

    def _preview_source_text(self, app: Any) -> str:
        """Read the labelled preview's child text rather than its aria label."""
        control = self.find_control(
            app,
            REVISION_PREVIEW_SOURCE_ACCESSIBILITY_ID,
            "Text",
            "REVISION_PREVIEW_SOURCE_UIA_TIMEOUT",
            "REVISION_PREVIEW_SOURCE_UIA_CONTRACT_MISMATCH",
        )
        name = self._control_name(control)
        try:
            pattern = control.iface_value
            value = pattern.CurrentValue
        except self.no_pattern_error_class:
            value = ""
        except Exception as error:
            raise HarnessFailure(
                "REVISION_PREVIEW_SOURCE_UIA_QUERY_FAILED", safe_exception_name(error)
            ) from None
        if isinstance(value, str) and value and value != name:
            return value
        try:
            children = control.children()
            child_names = [
                child.element_info.name
                for child in children
                if isinstance(child.element_info.name, str)
                and child.element_info.name
                and child.element_info.name != name
            ]
        except Exception as error:
            raise HarnessFailure(
                "REVISION_PREVIEW_SOURCE_UIA_QUERY_FAILED", safe_exception_name(error)
            ) from None
        if child_names:
            return "\n".join(child_names)
        if isinstance(value, str) and value:
            return value
        if name and name not in REVISION_PREVIEW_SOURCE_LABELS:
            return name
        if name:
            raise HarnessFailure("REVISION_PREVIEW_SOURCE_TEXT_CONTRACT_MISMATCH")
        raise HarnessFailure("REVISION_PREVIEW_SOURCE_TEXT_UNAVAILABLE")

    @staticmethod
    def _control_name(control: Any) -> str:
        try:
            name = control.element_info.name
        except Exception as error:
            raise HarnessFailure("UIA_CONTROL_NAME_QUERY_FAILED", safe_exception_name(error)) from None
        return name if isinstance(name, str) else ""

    def _document_dirty(self, app: Any, filename: str) -> bool:
        """Read the active tab's real UIA aria label, including its dirty marker."""
        expected = filename.casefold()

        def locate() -> dict[str, bool] | None:
            try:
                uia = self.iuia_class()
                elements = self.fresh_uia_root(app.hwnd).element.FindAll(
                    uia.tree_scope["descendants"], uia.iuia.CreateTrueCondition()
                )
                tab_item_type = uia.known_control_types["TabItem"]
                for index in range(elements.Length):
                    element = elements.GetElement(index)
                    if element.CurrentControlType != tab_item_type:
                        continue
                    name = str(element.CurrentName or "")
                    if expected not in name.casefold():
                        continue
                    lowered = name.casefold()
                    return {
                        "dirty": "unsaved changes" in lowered or "未保存" in name,
                    }
                return None
            except HarnessFailure:
                raise
            except Exception as error:
                raise HarnessFailure("DIRTY_STATE_UIA_QUERY_FAILED", safe_exception_name(error)) from None

        result = wait_until(locate, self.ui_timeout, "DIRTY_STATE_UIA_TIMEOUT", interval=0.05)
        return bool(result["dirty"])

    def _trust_label(self, app: Any) -> str:
        control = self.find_control(
            app,
            TRUST_AUTOMATION_ID,
            "Button",
            "TRUST_BUTTON_UIA_TIMEOUT",
            "TRUST_BUTTON_UIA_CONTRACT_MISMATCH",
        )
        return self._control_name(control)

    @staticmethod
    def _is_trusted_label(label: str) -> bool:
        lowered = label.casefold()
        return "trusted" in lowered or "已信任" in label

    @staticmethod
    def _is_restricted_label(label: str) -> bool:
        lowered = label.casefold()
        return (
            "trust this document" in lowered
            or "restricted" in lowered
            or "受限" in label
        )

    def _require_foreground(self, app: Any) -> bool:
        self.win32.require_foreground(app.hwnd, self.ui_timeout)
        return True

    def _click_id(self, app: Any, automation_id: str, control_type: str = "Button") -> None:
        self._require_foreground(app)
        control = self.find_control(
            app,
            automation_id,
            control_type,
            f"{automation_id.upper().replace('-', '_')}_UIA_TIMEOUT",
            f"{automation_id.upper().replace('-', '_')}_UIA_CONTRACT_MISMATCH",
        )
        if automation_id.startswith("markturbo-revision-"):
            control = self._scroll_revision_control_into_view(
                app, automation_id, control_type, control
            )
        if automation_id == REVISION_RUN_ACCESSIBILITY_ID:
            def enabled_revision_run() -> Any | None:
                candidate = self.control_by_id(
                    app.hwnd,
                    automation_id,
                    control_type,
                    "REVISION_RUN_UIA_CONTRACT_MISMATCH",
                )
                if candidate is None:
                    return None
                try:
                    return candidate if candidate.is_enabled() else None
                except Exception as error:
                    raise HarnessFailure(
                        "REVISION_RUN_UIA_QUERY_FAILED", safe_exception_name(error)
                    ) from None

            control = wait_until(
                enabled_revision_run,
                self.ui_timeout,
                "REVISION_RUN_ENABLED_TIMEOUT",
                interval=0.025,
            )
            control = self._scroll_revision_control_into_view(
                app, automation_id, control_type, control
            )
            self._require_foreground(app)
        self.click_control(control, f"{automation_id.upper().replace('-', '_')}_CLICK_FAILED")

    def _scroll_revision_control_into_view(
        self, app: Any, automation_id: str, control_type: str, control: Any
    ) -> Any:
        window_rect = app.window.rectangle()
        anchors = [(REVIEW_RESULT_ACCESSIBILITY_ID, "Text")]
        anchors.extend(
            (revision_answer_accessibility_id(index, "input"), "Edit")
            for index in range(len(QUESTION_IDS))
        )
        anchors.append((REVISION_PREVIEW_ACCESSIBILITY_ID, "Group"))
        for _ in range(8):
            control_rect = control.rectangle()
            window_rect = app.window.rectangle()
            safe_bottom = window_rect.bottom - REVISION_CONTROL_BOTTOM_INSET
            if (
                control_rect.top >= window_rect.top
                and control_rect.bottom <= safe_bottom
            ):
                return control
            anchor = next(
                (
                    candidate
                    for candidate_id, candidate_type in anchors
                    if (candidate := self.control_by_id(
                        app.hwnd,
                        candidate_id,
                        candidate_type,
                        "REVISION_SCROLL_ANCHOR_UIA_CONTRACT_MISMATCH",
                    ))
                    is not None
                    and (candidate_rect := candidate.rectangle()).top >= window_rect.top
                    and candidate_rect.bottom <= window_rect.bottom
                ),
                None,
            )
            if anchor is None:
                break
            anchor.wheel_mouse_input(
                wheel_dist=-3 if control_rect.bottom > safe_bottom else 3
            )
            time.sleep(0.05)
            next_control = self.control_by_id(
                app.hwnd,
                automation_id,
                control_type,
                f"{automation_id.upper().replace('-', '_')}_UIA_CONTRACT_MISMATCH",
            )
            if next_control is None:
                break
            control = next_control
        raise HarnessFailure(
            f"{automation_id.upper().replace('-', '_')}_SCROLL_TIMEOUT"
        )

    def _find_stale_control(self, app: Any) -> Any:
        def locate() -> Any | None:
            control = self.control_by_id(
                app.hwnd,
                REVISION_STALE_ACCESSIBILITY_ID,
                "Text",
                "REVISION_STALE_UIA_CONTRACT_MISMATCH",
            )
            if control is not None:
                return control
            window_rect = app.window.rectangle()
            for automation_id, control_type in (
                (REVISION_PREVIEW_ACCESSIBILITY_ID, "Group"),
                (REVISION_RESULT_ACCESSIBILITY_ID, "Group"),
                *(
                    (revision_answer_accessibility_id(index, "input"), "Edit")
                    for index in range(len(QUESTION_IDS))
                ),
            ):
                anchor = self.control_by_id(
                    app.hwnd,
                    automation_id,
                    control_type,
                    "REVISION_SCROLL_ANCHOR_UIA_CONTRACT_MISMATCH",
                )
                if anchor is None:
                    continue
                anchor_rect = anchor.rectangle()
                if anchor_rect.top >= window_rect.top and anchor_rect.bottom <= window_rect.bottom:
                    anchor.wheel_mouse_input(wheel_dist=3)
                    break
            return None

        control = wait_until(
            locate,
            self.ui_timeout,
            "REVISION_STALE_UIA_TIMEOUT",
            interval=0.05,
        )
        return self._scroll_revision_control_into_view(
            app, REVISION_STALE_ACCESSIBILITY_ID, "Text", control
        )

    def _set_answer(self, app: Any, index: int, value: str) -> None:
        # The input is disabled until the explicit Answered state is selected.
        # Type through the focused editor so the native harness exercises the
        # same input path as a user instead of relying on unsupported UIA SetValue.
        self._click_id(app, revision_answer_accessibility_id(index, "answered"))
        automation_id = revision_answer_accessibility_id(index, "input")
        def enabled_input() -> Any | None:
            control = self.control_by_id(
                app.hwnd,
                automation_id,
                "Edit",
                "REVISION_ANSWER_UIA_CONTRACT_MISMATCH",
            )
            if control is None:
                return None
            try:
                return control if control.is_enabled() else None
            except Exception as error:
                raise HarnessFailure(
                    "REVISION_ANSWER_UIA_QUERY_FAILED", safe_exception_name(error)
                ) from None

        wait_until(
            enabled_input,
            self.ui_timeout,
            "REVISION_ANSWER_UIA_TIMEOUT",
            interval=0.025,
        )
        self.win32.send_unicode(app.hwnd, value)
        wait_until(
            lambda: value if self._text_control(app, automation_id, "Edit") == value else None,
            self.ui_timeout,
            "REVISION_ANSWER_INPUT_TIMEOUT",
            interval=0.025,
        )

    def _mark_intentionally_unspecified(self, app: Any, index: int) -> None:
        self._click_id(app, revision_answer_accessibility_id(index, "unspecified"))

    def _open_consent_gate(
        self,
        provider: LoopbackRevisionServer,
        expected_request_count: int,
        open_gate: Any,
    ) -> None:
        snapshot = provider.request_snapshot()
        if (
            set(snapshot) != {"request_count", "invalid_request"}
            or snapshot["request_count"] != expected_request_count
            or snapshot["invalid_request"] is not False
        ):
            operation = "REVIEW" if expected_request_count == 0 else "REVISION"
            raise HarnessFailure(f"{operation}_REQUEST_BEFORE_CONSENT_CLICK")
        open_gate()

    def _approve_consent(
        self,
        app: Any,
        failure_code: str,
        *,
        provider: LoopbackRevisionServer | None = None,
        expected_request_count: int | None = None,
        open_gate: Any = None,
    ) -> None:
        def locate() -> Any | None:
            dialogs = self.win32.owned_task_dialogs(app.process.pid, app.hwnd)
            if len(dialogs) > 1:
                raise HarnessFailure("MULTIPLE_REVISION_CONSENT_DIALOGS")
            if not dialogs:
                return None
            return self.control_by_id(
                dialogs[0],
                REVISION_CONSENT_AUTOMATION_ID,
                "Button",
                "REVISION_CONSENT_UIA_CONTRACT_MISMATCH",
                expected_class_name=TASK_DIALOG_BUTTON_CLASS,
            )

        button = wait_until(locate, self.ui_timeout, "REVISION_CONSENT_UIA_TIMEOUT", interval=0.025)
        if provider is not None:
            if expected_request_count is None or open_gate is None:
                raise HarnessFailure("CONSENT_GATE_CONFIGURATION_INVALID")
            self._open_consent_gate(provider, expected_request_count, open_gate)
            click = lambda: self.click_control(button, failure_code)
            if isinstance(provider, LoopbackRevisionServer):
                provider.dispatch_consent_click(expected_request_count, click)
                return
            begin_click = getattr(provider, "begin_consent_click", None)
            if begin_click is None:
                raise HarnessFailure("CONSENT_GATE_CONFIGURATION_INVALID")
            begin_click(expected_request_count)
            click()
            return
        self.click_control(button, failure_code)

    def run_review(
        self, app: Any, provider: LoopbackRevisionServer | None = None
    ) -> None:
        self.focus_editor(app)
        self._require_foreground(app)
        self.win32.send_inputs(
            [
                key_input(VK_CONTROL, False),
                key_input(VK_SHIFT, False),
                key_input(VK_R, False),
                key_input(VK_R, True),
                key_input(VK_SHIFT, True),
                key_input(VK_CONTROL, True),
            ]
        )
        self._click_id(app, REVIEW_RUN_ACCESSIBILITY_ID)
        self._approve_consent(
            app,
            "REVIEW_CONSENT_CLICK_FAILED",
            provider=provider,
            expected_request_count=0,
            open_gate=provider.grant_review_consent if provider is not None else None,
        )
        self.find_control(
            app,
            REVIEW_RESULT_ACCESSIBILITY_ID,
            "Text",
            "REVIEW_RESULT_UIA_TIMEOUT",
            "REVIEW_RESULT_UIA_CONTRACT_MISMATCH",
        )

    def request_revision(
        self,
        app: Any,
        *,
        answer: bool = True,
        provider: LoopbackRevisionServer | None = None,
    ) -> None:
        if answer:
            self._set_answer(app, 0, ANSWER_TEXT)
            self._mark_intentionally_unspecified(app, 1)
        self._click_id(app, REVISION_RUN_ACCESSIBILITY_ID)
        self._approve_consent(
            app,
            "REVISION_CONSENT_CLICK_FAILED",
            provider=provider,
            expected_request_count=1,
            open_gate=provider.grant_revision_consent if provider is not None else None,
        )
        self.find_control(
            app,
            REVISION_PREVIEW_ACCESSIBILITY_ID,
            "Group",
            "REVISION_PREVIEW_UIA_TIMEOUT",
            "REVISION_PREVIEW_UIA_CONTRACT_MISMATCH",
        )

    def _apply_button_state(self, app: Any) -> tuple[Any, bool]:
        button = self.control_by_id(
            app.hwnd,
            REVISION_APPLY_ACCESSIBILITY_ID,
            "Button",
            "REVISION_APPLY_UIA_CONTRACT_MISMATCH",
        )
        if button is None:
            raise HarnessFailure("REVISION_APPLY_UIA_TIMEOUT")
        button = self._scroll_revision_control_into_view(
            app, REVISION_APPLY_ACCESSIBILITY_ID, "Button", button
        )
        try:
            return button, bool(button.is_enabled())
        except Exception as error:
            raise HarnessFailure("REVISION_APPLY_UIA_QUERY_FAILED", safe_exception_name(error)) from None

    def _apply_button_enabled(self, app: Any) -> bool:
        return self._apply_button_state(app)[1]

    def _status_contains_conflict(self, app: Any) -> bool:
        def locate() -> bool | None:
            try:
                control = self.control_by_id(
                    app.hwnd,
                    CONFLICT_OVERWRITE_ACCESSIBILITY_ID,
                    "Button",
                    "CONFLICT_OVERWRITE_UIA_CONTRACT_MISMATCH",
                )
                return control is not None
            except Exception as error:
                raise HarnessFailure(
                    "CONFLICT_OVERWRITE_UIA_QUERY_FAILED", safe_exception_name(error)
                ) from None

        return bool(wait_until(locate, self.ui_timeout, "SAVE_CONFLICT_UIA_TIMEOUT", interval=0.05))

    def _common_observations(self, provider: LoopbackRevisionServer) -> dict[str, Any]:
        contract = provider.contract_evidence()
        return {
            "loopback_provider": True,
            "server_loopback": True,
            "server_deterministic": True,
            **contract,
        }

    def _finalize_observations(
        self,
        provider: LoopbackRevisionServer,
        case_root: Path,
        app: Any,
        observations: dict[str, Any],
    ) -> dict[str, Any]:
        """Reap the app before the final privacy scan and provider close."""
        self.reap(app)
        observations.update(self._common_observations(provider))
        observations["runtime_scan"] = scan_case_artifacts(case_root, self._credential)
        return observations

    def scenario_reject_all(self) -> dict[str, Any]:
        provider = LoopbackRevisionServer(EDITOR_SOURCE_TEXT).start()
        app = None
        try:
            case_root, data, config, workspace, stderr, source = self.profile(
                CASE_REJECT_ALL, provider.base_url
            )
            app = self.launch_app(source, data, config, workspace, stderr)
            self.activate_source_layout(app)
            editor_before = self.editor_fingerprint(app)
            source_before = sha256_file(source)
            dirty_before = self._document_dirty(app, source.name)
            self.run_review(app, provider)
            self.request_revision(app, provider=provider)
            preview_text = self._preview_source_text(app)
            self._click_id(app, REVISION_REJECT_ALL_ACCESSIBILITY_ID)
            editor_after = self.editor_fingerprint(app)
            source_after = sha256_file(source)
            dirty_after = self._document_dirty(app, source.name)
            observations = {
                "flow": CASE_FLOWS[CASE_REJECT_ALL],
                "process_context": app.security_context.evidence(),
                "foreground_verified": self._require_foreground(app),
                "proposal_received": bool(preview_text),
                "reviewed_source_match": editor_before
                == fingerprint_bytes(EDITOR_SOURCE_BYTES),
                "editor_before": editor_before.evidence(),
                "editor_after": editor_after.evidence(),
                "source_before": source_before.evidence(),
                "source_after": source_after.evidence(),
                "reject_all_byte_identity": (
                    editor_before == editor_after and source_before == source_after
                ),
                "dirty_before": dirty_before,
                "dirty_after": dirty_after,
                "reject_all_dirty_unchanged": dirty_before == dirty_after,
            }
            return self._finalize_observations(provider, case_root, app, observations)
        finally:
            if app is not None:
                self.reap(app)
            provider.close()

    def scenario_selective_undo(self) -> dict[str, Any]:
        provider = LoopbackRevisionServer(EDITOR_SOURCE_TEXT).start()
        app = None
        try:
            case_root, data, config, workspace, stderr, source = self.profile(
                CASE_SELECTIVE_UNDO, provider.base_url
            )
            app = self.launch_app(source, data, config, workspace, stderr)
            self.activate_source_layout(app)
            editor_before = self.editor_fingerprint(app)
            source_before = sha256_file(source)
            self.run_review(app, provider)
            self.request_revision(app, provider=provider)
            expected_preview = fingerprint_bytes(
                apply_edits(EDITOR_SOURCE_BYTES, fixture_edits(EDITOR_SOURCE_TEXT)[:1])
            )
            self._click_id(app, revision_change_accessibility_id(0, "accept"))
            self._click_id(app, revision_change_accessibility_id(1, "reject"))
            selected_preview = fingerprint_text(self._preview_source_text(app))
            self._click_id(app, REVISION_APPLY_ACCESSIBILITY_ID)
            editor_after_apply = self.editor_fingerprint(app)
            self.win32.send_shortcut(app.hwnd, 0x5A)
            editor_after_undo = self.editor_fingerprint(app)
            source_after = sha256_file(source)
            observations = {
                "flow": CASE_FLOWS[CASE_SELECTIVE_UNDO],
                "process_context": app.security_context.evidence(),
                "foreground_verified": self._require_foreground(app),
                "editor_before": editor_before.evidence(),
                "selective_preview": selected_preview.evidence(),
                "editor_after_apply": editor_after_apply.evidence(),
                "editor_after_undo": editor_after_undo.evidence(),
                "source_before": source_before.evidence(),
                "source_after": source_after.evidence(),
                "selective_matches_editor": editor_after_apply == selected_preview,
                "selective_matches_expected": selected_preview == expected_preview,
                "one_undo_transaction": editor_after_undo == editor_before,
                "undo_count": 1,
            }
            return self._finalize_observations(provider, case_root, app, observations)
        finally:
            if app is not None:
                self.reap(app)
            provider.close()

    def scenario_accept_all_preview(self) -> dict[str, Any]:
        provider = LoopbackRevisionServer(EDITOR_SOURCE_TEXT).start()
        app = None
        try:
            case_root, data, config, workspace, stderr, source = self.profile(
                CASE_ACCEPT_ALL_PREVIEW, provider.base_url
            )
            app = self.launch_app(source, data, config, workspace, stderr)
            self.activate_source_layout(app)
            self.run_review(app, provider)
            self.request_revision(app, provider=provider)
            editor_before = self.editor_fingerprint(app)
            self._click_id(app, REVISION_ACCEPT_ALL_ACCESSIBILITY_ID)
            preview_text = self._preview_source_text(app)
            final_preview = fingerprint_text(preview_text)
            expected = fingerprint_bytes(
                apply_edits(EDITOR_SOURCE_BYTES, fixture_edits(EDITOR_SOURCE_TEXT))
            )
            editor_after_preview = self.editor_fingerprint(app)
            copy_editor_before = editor_after_preview
            copy_dirty_before = self._document_dirty(app, source.name)
            copy_source_before = sha256_file(source)
            clipboard_before = self.read_text_clipboard()
            try:
                # Clear the clipboard first so an unchanged old value cannot
                # accidentally satisfy the copy assertion.
                self.write_text_clipboard(None)
                self._click_id(app, REVISION_COPY_ACCESSIBILITY_ID)
                copied = wait_until(
                    lambda: self.read_text_clipboard(),
                    self.ui_timeout,
                    "REVISION_COPY_CLIPBOARD_TIMEOUT",
                    interval=0.05,
                )
                copy_editor_after = self.editor_fingerprint(app)
                copy_dirty_after = self._document_dirty(app, source.name)
                copy_source_after = sha256_file(source)
            finally:
                self.write_text_clipboard(clipboard_before)
            observations = {
                "flow": CASE_FLOWS[CASE_ACCEPT_ALL_PREVIEW],
                "process_context": app.security_context.evidence(),
                "foreground_verified": self._require_foreground(app),
                "editor_before_apply": editor_before.evidence(),
                "editor_after_preview": editor_after_preview.evidence(),
                "preview_fingerprint": final_preview.evidence(),
                "final_preview": final_preview.evidence(),
                "accept_all_matches_expected": final_preview == expected,
                "copy_preview_fingerprint": fingerprint_text(str(copied)).evidence(),
                "copy_preview_exact": str(copied) == preview_text,
                "copy_editor_before": copy_editor_before.evidence(),
                "copy_editor_after": copy_editor_after.evidence(),
                "copy_editor_unchanged": copy_editor_before == copy_editor_after,
                "copy_dirty_before": copy_dirty_before,
                "copy_dirty_after": copy_dirty_after,
                "copy_dirty_unchanged": copy_dirty_before == copy_dirty_after,
                "copy_source_before": copy_source_before.evidence(),
                "copy_source_after": copy_source_after.evidence(),
                "copy_source_unchanged": copy_source_before == copy_source_after,
            }
            return self._finalize_observations(provider, case_root, app, observations)
        finally:
            if app is not None:
                self.reap(app)
            provider.close()

    def scenario_stale(self) -> dict[str, Any]:
        provider = LoopbackRevisionServer(EDITOR_SOURCE_TEXT).start()
        app = None
        try:
            case_root, data, config, workspace, stderr, source = self.profile(
                CASE_STALE, provider.base_url
            )
            app = self.launch_app(source, data, config, workspace, stderr)
            self.activate_source_layout(app)
            self.run_review(app, provider)
            self.request_revision(app, provider=provider)
            editor_before = self.editor_fingerprint(app)
            self.replace_editor(app, STALE_EDIT_TEXT)
            editor_after_edit = self.editor_fingerprint(app)
            self._find_stale_control(app)
            apply_button, apply_accessibility_reported_enabled = self._apply_button_state(app)
            source_before = sha256_file(source)
            self.click_control(apply_button, "REVISION_APPLY_CLICK_FAILED")
            editor_after_activation_attempt = self.editor_fingerprint(app)
            source_after = sha256_file(source)
            observations = {
                "flow": CASE_FLOWS[CASE_STALE],
                "process_context": app.security_context.evidence(),
                "foreground_verified": self._require_foreground(app),
                "editor_before": editor_before.evidence(),
                "editor_after_edit": editor_after_edit.evidence(),
                "source_before": source_before.evidence(),
                "source_after": source_after.evidence(),
                "stale_proposal_received": True,
                "stale_visible": True,
                "apply_activation_attempted": True,
                "apply_disabled_source_contract": stale_apply_source_contract_ok(),
                "apply_control_observed": apply_button is not None,
                "apply_accessibility_reported_enabled": apply_accessibility_reported_enabled,
                "editor_after_activation_attempt": editor_after_activation_attempt.evidence(),
                "stale_no_mutation": (
                    source_before == source_after
                    and editor_after_edit == editor_after_activation_attempt
                ),
            }
            return self._finalize_observations(provider, case_root, app, observations)
        finally:
            if app is not None:
                self.reap(app)
            provider.close()

    def scenario_save_conflict(self) -> dict[str, Any]:
        provider = LoopbackRevisionServer(EDITOR_SOURCE_TEXT).start()
        app = None
        try:
            case_root, data, config, workspace, stderr, source = self.profile(
                CASE_SAVE_CONFLICT, provider.base_url
            )
            app = self.launch_app(source, data, config, workspace, stderr)
            self.activate_source_layout(app)
            self.run_review(app, provider)
            self.request_revision(app, provider=provider)
            self._click_id(app, REVISION_ACCEPT_ALL_ACCESSIBILITY_ID)
            self._click_id(app, REVISION_APPLY_ACCESSIBILITY_ID)
            editor_after_apply = self.editor_fingerprint(app)
            write_durable(source, EXTERNAL_SOURCE_BYTES)
            external_source_before = sha256_file(source)
            conflict_visible_before_save = self._status_contains_conflict(app)
            self.focus_editor(app)
            self.win32.send_shortcut(app.hwnd, VK_S)
            conflict = self._status_contains_conflict(app)
            external_source_after = sha256_file(source)
            observations = {
                "flow": CASE_FLOWS[CASE_SAVE_CONFLICT],
                "process_context": app.security_context.evidence(),
                "foreground_verified": self._require_foreground(app),
                "editor_after_apply": editor_after_apply.evidence(),
                "external_source_before": external_source_before.evidence(),
                "external_source_after": external_source_after.evidence(),
                "save_shortcut_sent": True,
                "conflict_visible_before_save": conflict_visible_before_save,
                "safe_save_conflict_visible": conflict,
                "safe_save_no_overwrite": external_source_before == external_source_after,
            }
            return self._finalize_observations(provider, case_root, app, observations)
        finally:
            if app is not None:
                self.reap(app)
            provider.close()

    def scenario_trust_revoke(self) -> dict[str, Any]:
        provider = LoopbackRevisionServer(HTML_EDITOR_SOURCE_TEXT).start()
        app = None
        try:
            case_root, data, config, workspace, stderr, source = self.profile(
                CASE_TRUST_REVOKE, provider.base_url, html=True
            )
            app = self.launch_app(source, data, config, workspace, stderr)
            self.activate_source_layout(app)
            trust = self.find_control(
                app,
                TRUST_AUTOMATION_ID,
                "Button",
                "TRUST_BUTTON_UIA_TIMEOUT",
                "TRUST_BUTTON_UIA_CONTRACT_MISMATCH",
            )
            self.click_control(trust, "TRUST_BUTTON_CLICK_FAILED")

            def trusted_label() -> str | None:
                label = self._trust_label(app)
                return label if self._is_trusted_label(label) else None

            trusted_label = wait_until(
                trusted_label,
                self.ui_timeout,
                "TRUST_ENABLE_STATE_TIMEOUT",
                interval=0.05,
            )
            trust_before = self._is_trusted_label(str(trusted_label))
            self.activate_source_layout(app)
            self.run_review(app, provider)
            self.request_revision(app, provider=provider)
            self._click_id(app, REVISION_ACCEPT_ALL_ACCESSIBILITY_ID)
            editor_before_apply = self.editor_fingerprint(app)
            expected = fingerprint_bytes(
                apply_edits(HTML_EDITOR_SOURCE_BYTES, fixture_edits(HTML_EDITOR_SOURCE_TEXT))
            )
            preview_text = self._preview_source_text(app)
            preview = fingerprint_text(preview_text)
            self._click_id(app, REVISION_APPLY_ACCESSIBILITY_ID)
            editor_after_apply = self.editor_fingerprint(app)

            def restricted_label() -> str | None:
                label = self._trust_label(app)
                return label if self._is_restricted_label(label) else None

            restricted_label = wait_until(
                restricted_label,
                self.ui_timeout,
                "TRUST_RESTRICTED_STATE_TIMEOUT",
                interval=0.05,
            )
            trust_order_contract = trust_apply_source_contract_ok()
            preview_inert_contract = preview_inert_source_contract_ok()
            observations = {
                "flow": CASE_FLOWS[CASE_TRUST_REVOKE],
                "process_context": app.security_context.evidence(),
                "foreground_verified": self._require_foreground(app),
                "trust_before": trust_before,
                "editor_before_apply": editor_before_apply.evidence(),
                "editor_after_apply": editor_after_apply.evidence(),
                "executable_expected": expected.evidence(),
                "executable_matches_expected": editor_after_apply == expected,
                "restricted_after_apply": self._is_restricted_label(str(restricted_label)),
                "executable_change": editor_before_apply != editor_after_apply,
                "trust_revocation_order_source_contract": trust_order_contract,
                "preview_inert_source_contract": preview_inert_contract,
                "preview_fingerprint": preview.evidence(),
                "preview_matches_expected": preview == expected,
                "preview_contains_executable_text": (
                    "<script>" in preview_text and 'goal07 = "new"' in preview_text
                ),
            }
            return self._finalize_observations(provider, case_root, app, observations)
        finally:
            if app is not None:
                self.reap(app)
            provider.close()

    def scenario(self, case_id: str) -> dict[str, Any]:
        observations = {
            CASE_REJECT_ALL: self.scenario_reject_all,
            CASE_SELECTIVE_UNDO: self.scenario_selective_undo,
            CASE_ACCEPT_ALL_PREVIEW: self.scenario_accept_all_preview,
            CASE_STALE: self.scenario_stale,
            CASE_SAVE_CONFLICT: self.scenario_save_conflict,
            CASE_TRUST_REVOKE: self.scenario_trust_revoke,
        }[case_id]()
        try:
            validate_passed_case(case_id, observations, self.parent_context.evidence())
        except (KeyError, TypeError, ValueError) as error:
            raise HarnessFailure("CASE_CONTRACT_FAILED", safe_exception_name(error)) from None
        if _ACTIVE_EVIDENCE is not None:
            _ACTIVE_EVIDENCE["transport"]["request_count"] = sum(
                int(case.get("observations", {}).get("provider_request_count", 0))
                for case in _ACTIVE_EVIDENCE.get("cases", ())
                if isinstance(case, dict)
            ) + int(observations["provider_request_count"])
        return observations


def goal07_preflight(exe: Path, expected_hash: str, evidence: dict[str, Any]) -> tuple[Any, Any]:
    return preflight(exe, expected_hash, evidence)


def native_run_plan() -> NativeRunPlan:
    return NativeRunPlan(
        required_case_ids=REQUIRED_CASE_IDS,
        workdir_prefix="markturbo-goal-07-native-",
        new_evidence=new_evidence,
        validate_evidence=validate_evidence,
        source_contract=source_contract_failure,
        preflight=goal07_preflight,
        ui_types_loader=load_pywinauto,
        harness_factory=Goal07Harness,
        scenarios=lambda harness: tuple(
            lambda case_id=case_id: harness.scenario(case_id)
            for case_id in REQUIRED_CASE_IDS
        ),
    )


def run(args: argparse.Namespace) -> tuple[int, dict[str, Any], str]:
    return run_native_acceptance(args, native_run_plan())


def parse_args(argv: list[str] | None = None) -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--exe", type=Path, default=DEFAULT_EXE)
    parser.add_argument("--expect-exe-sha256", type=normalize_expected_hash, required=True)
    parser.add_argument("--evidence", type=Path, default=DEFAULT_EVIDENCE)
    parser.add_argument("--ui-timeout", type=float, default=15.0)
    parser.add_argument(
        "--case",
        choices=REQUIRED_CASE_IDS,
        help="run one case for debugging; it never produces acceptance PASS",
    )
    parser.add_argument(
        "--keep-workdir-on-failure",
        action="store_true",
        help="preserve isolated roots after a non-PASS run",
    )
    args = parser.parse_args(argv)
    if not math.isfinite(args.ui_timeout) or args.ui_timeout <= 0:
        parser.error("--ui-timeout must be greater than zero")
    return args


def main(argv: list[str] | None = None) -> int:
    args = parse_args(argv)
    return main_native_acceptance(args, native_run_plan())


if __name__ == "__main__":
    raise SystemExit(main())
