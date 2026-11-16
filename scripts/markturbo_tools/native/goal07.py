"""Goal 07 native acceptance harness.

This harness drives the real Windows UI with a deterministic OpenAI Responses
loopback server. Each case has isolated data/config/workspace roots. The
server and evidence boundary never persist document text, request bodies, or
credentials; evidence contains only hashes, byte counts, booleans, and safe
reason codes.

On the nsh UI a Review starts with Ctrl+Enter from the editor, the question is
answered in `review-answer-input`, `Prepare revision` asks for the Revision,
and both sends need a real pointer hold on `consent-send`. The Revision field
decides one docked hunk at a time (`Accept` / `Reject`, counted by `n of m
decided`) and `Done` performs the one write. Editor text is read with the
editor's Select All + Copy; Save runs from the title menu.

Ctrl+Enter is state-dependent: it starts Review from the title row
(`ui/titlebar.rs`), sends from the consent card (`ui/consent.rs`), and is Done
on the Revision field (`ui/revision.rs`). It is sent only to start a Review and,
in the stale case, to prove Done stays unbound; consent is always a pointer
hold. Trust scripts' Run scripts confirmation is an nsh system alert outside
the UIA tree, answered with Enter (`runtime.py` `confirm_system_alert`).
"""

from __future__ import annotations

import argparse
import hashlib
import json
import math
import uuid
from pathlib import Path
from typing import Any
from urllib.parse import urlsplit

from . import goal07_provider as provider_fixtures
from .goal03 import Goal03Harness as ClipboardNativeHarness
from .source_contract import (
    alert_enter_confirms,
    production_markers_present,
    production_source,
    rust_source_views,
)
from .runtime import (
    IMAGE_FILE_MACHINE_AMD64,
    INTEGRITY_NAMES,
    PE32_PLUS_MAGIC,
    REASON_CODE_RE,
    SAFE_FAILURE_TYPES,
    SHA256_RE,
    CONFLICT_HEADING,
    MENU_EDIT_AUTOMATION_ID,
    MENU_WEB_AUTOMATION_ID,
    VK_RETURN,
    VK_Z,
    Fingerprint,
    HarnessBlocked,
    HarnessFailure,
    NativeRunPlan,
    complete_evidence as complete_evidence_envelope,
    finite_nonnegative,
    fingerprint_bytes,
    fingerprint_text,
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
    artifact_contains,
)


REPO = Path(__file__).resolve().parents[3]
DEFAULT_EXE = REPO / "target" / "release" / "markturbo.exe"
DEFAULT_EVIDENCE = REPO / "target" / "native-acceptance" / "goal-07-native-acceptance-v1.json"
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

# `ui/review.rs`: the structured result's first line, the answer input, and
# the anonymous Buttons named by their labels.
REVIEW_RESULT_MARKER = "Understood:"
REVIEW_ANSWER_INPUT_AUTOMATION_ID = "review-answer-input"
REVIEW_ANSWER_NAME = "Answer"
PREPARE_REVISION_NAME = "Prepare revision"
# `ui/consent.rs` `send_button`: only a pointer hold past `HOLD_MS` sends.
CONSENT_SEND_AUTOMATION_ID = "consent-send"
CONSENT_HOLD_SECONDS = 1.8
# `ui/revision.rs`: the docked hunk's decisions, the count and Done.
REVISION_ACCEPT_NAME = "Accept"
REVISION_REJECT_NAME = "Reject"
REVISION_DONE_NAME = "Done"
REVISION_STALE_TEXT = "Stale \u2014 edited after review"
# Not in the nsh UI yet: the Copy of the approved result, and an exposed group
# around the docked hunk's proposed lines.
REVISION_COPY_AUTOMATION_ID = "revision-copy"
REVISION_PROPOSAL_AFTER_AUTOMATION_ID = "revision-proposal-after"
# `ui/preview.rs`. `state.rs` `request_web_trust` confirms with an nsh system
# alert outside the UIA tree, answered with Enter (`confirm_system_alert`).
TRUST_SCRIPTS_NAME = "Trust scripts"
TRUSTED_BANNER = "Trusted: scripts may run"
RESTRICTED_BANNER = "Restricted: scripts disabled"
UI_SOURCE_REQUIREMENTS = {
    "crates/mt-app/src/ui/titlebar.rs": (
        '"mod+enter"',
        '"title-settings"',
        '"title-menu-save"',
        '"title-menu-edit"',
        '"title-menu-web"',
        '"title-menu-quit"',
        '"title-dirty"',
    ),
    "crates/mt-app/src/ui/review.rs": (
        '"review-answer-input"',
        '"Answer"',
        '"Prepare revision"',
        '"Understood:"',
    ),
    "crates/mt-app/src/ui/consent.rs": ('"consent-send"', ".long_press_delay(HOLD_MS)"),
    "crates/mt-app/src/ui/revision.rs": (
        '"Accept"',
        '"Reject"',
        '"Done"',
        '"Stale \u2014 edited after review"',
        '"{n} of {m} decided"',
    ),
    "crates/mt-app/src/ui/notices.rs": (
        '"Changed on disk"',
        '"Close without saving?"',
        '"Don\'t save"',
    ),
    "crates/mt-app/src/i18n.rs": (
        '"Trust scripts"',
        '"Trusted: scripts may run"',
        '"Restricted: scripts disabled"',
    ),
    "crates/mt-app/src/state.rs": ('.destructive("trust", "Run scripts")',),
    "crates/mt-app/src/ui/document.rs": ('"Markdown source"',),
}

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
    CASE_REJECT_ALL: "Review -> Revision -> Reject every hunk -> Done -> exact editor and source identity",
    CASE_SELECTIVE_UNDO: "Review -> Revision -> Accept one hunk, Reject one -> Done -> one undo",
    CASE_ACCEPT_ALL_PREVIEW: (
        "Review -> Revision -> Accept every hunk -> Copy approved result -> Done exact"
    ),
    CASE_STALE: "Revision -> edit source -> stale visible -> Accept withheld and Done refused",
    CASE_SAVE_CONFLICT: "Revision -> Done -> external write -> menu Save conflict",
    CASE_TRUST_REVOKE: "trusted HTML -> executable Done -> Restricted trust",
}

DOCUMENT_SENTINEL = "MTG07-NATIVE-REVISION-SENTINEL"
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

SAFE_STRINGS = frozenset(CASE_FLOWS.values()) | INTEGRITY_NAMES | {
    "keyless_loopback",
    "openai-responses",
    "Trusted",
    "Restricted",
}
ALLOWED_OBSERVATION_KEYS = {
    "accept_all_expected",
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
    "editor_after_done",
    "editor_after_edit",
    "editor_after_preview",
    "editor_before",
    "editor_before_apply",
    "external_source_after",
    "external_source_before",
    "flow",
    "foreground_verified",
    "loopback_provider",
    "one_undo_transaction",
    "preview_fingerprint",
    "proposal_expected",
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
    "save_command_sent",
    "conflict_visible_before_save",
    "selective_expected",
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
    "apply_control_observed",
    "apply_accessibility_reported_enabled",
    "editor_after_activation_attempt",
    "preview_inert_source_contract",
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
    return evidence


def finalize_evidence(evidence: dict[str, Any]) -> None:
    request_count = 0
    for case in evidence.get("cases", ()):
        if not isinstance(case, dict):
            continue
        observations = case.get("observations")
        if not isinstance(observations, dict) or "provider_request_count" not in observations:
            continue
        observed = observations["provider_request_count"]
        if not isinstance(observed, int) or isinstance(observed, bool) or observed < 0:
            raise ValueError("invalid provider request count")
        request_count += observed
    transport = evidence.get("transport")
    if not isinstance(transport, dict):
        raise ValueError("invalid loopback transport evidence")
    transport["request_count"] = request_count


def complete_evidence(evidence: dict[str, Any], status: str) -> None:
    finalize_evidence(evidence)
    complete_evidence_envelope(evidence, status, REQUIRED_CASE_IDS)


PRIVATE_SENTINELS = (
    DOCUMENT_SENTINEL,
    provider_fixtures.ANSWER_SENTINEL,
    provider_fixtures.RAW_RESPONSE_SENTINEL,
)
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
        "accept_all_expected",
        "editor_after_done",
        "preview_fingerprint",
        "proposal_expected",
        "selective_expected",
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
            "selective_expected",
            "editor_after_apply",
            "editor_after_undo",
            "source_before",
            "source_after",
            "selective_matches_expected",
            "one_undo_transaction",
            "undo_count",
        },
        CASE_ACCEPT_ALL_PREVIEW: {
            "editor_before_apply",
            "editor_after_preview",
            "accept_all_expected",
            "editor_after_done",
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
            "save_command_sent",
            "conflict_visible_before_save",
            "safe_save_conflict_visible",
            "safe_save_no_overwrite",
        },
        CASE_TRUST_REVOKE: {
            "trust_before",
            "editor_before_apply",
            "editor_after_apply",
            "restricted_after_apply",
            "executable_change",
            "executable_expected",
            "executable_matches_expected",
            "trust_revocation_order_source_contract",
            "preview_inert_source_contract",
            "preview_fingerprint",
            "proposal_expected",
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
        selected = validate_fingerprint(observations["selective_expected"])
        applied = validate_fingerprint(observations["editor_after_apply"])
        undone = validate_fingerprint(observations["editor_after_undo"])
        source_before = validate_fingerprint(observations["source_before"])
        source_after = validate_fingerprint(observations["source_after"])
        require_true(
            observations,
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
        final = validate_fingerprint(observations["accept_all_expected"])
        done = validate_fingerprint(observations["editor_after_done"])
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
            or done != final
            or copied != final
            or copy_before != copy_after
            or copy_source_before != copy_source_after
            or observations.get("copy_dirty_before")
            is not observations.get("copy_dirty_after")
        ):
            raise ValueError("accept-all was not stable before Done or not exact after it")
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
            "save_command_sent",
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
        proposal = validate_fingerprint(observations["proposal_expected"])
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
        if preview != proposal or actual != expected or before == actual:
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
        observed_request_count = 0
        for case in cases_for_transport:
            if not isinstance(case, dict):
                continue
            observations = case.get("observations")
            if not isinstance(observations, dict) or "provider_request_count" not in observations:
                continue
            count = observations["provider_request_count"]
            if not isinstance(count, int) or isinstance(count, bool) or count < 0:
                raise ValueError("invalid provider request count")
            observed_request_count += count
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


def _rust_function_body(source: str | None, signature: str) -> str | None:
    if source is None:
        return None
    views = rust_source_views(source)
    if views is None:
        return None
    code_only, _ = views

    start = code_only.find(signature)
    if start < 0:
        return None
    opening = code_only.find("{", start + len(signature))
    if opening < 0:
        return None

    depth = 0
    for index in range(opening, len(code_only)):
        if code_only[index] == "{":
            depth += 1
        elif code_only[index] == "}":
            depth -= 1
            if depth == 0:
                return code_only[opening + 1 : index]
    return None


def trust_apply_source_contract_ok() -> bool:
    """Applying a revision revokes the tab's script trust (`state.rs`) before
    the approved text replaces the buffer, so no trusted page renders it."""
    state = production_source(REPO / "crates" / "mt-app" / "src" / "state.rs")
    body = _rust_function_body(state, "pub fn apply_revision(")
    if body is None:
        return False
    revoke = body.find("web_trusted.set(false)")
    replace = body.find("tab.text.set(")
    return 0 <= revoke < replace


def preview_inert_source_contract_ok() -> bool:
    """The Revision field shows proposals as text, never through a WebView."""
    revision = production_source(REPO / "crates" / "mt-app" / "src" / "ui" / "revision.rs")
    if revision is None:
        return False
    views = rust_source_views(revision)
    if views is None:
        return False
    _, comment_free = views
    forbidden = ("web_view", "WebView", "web_view_html", "java_script_enabled")
    return f'"{REVISION_PROPOSAL_AFTER_AUTOMATION_ID}"' in comment_free and not any(
        marker in comment_free for marker in forbidden
    )


def stale_apply_source_contract_ok() -> bool:
    """A stale proposal withholds Accept and leaves Done's key unbound."""
    revision = production_source(REPO / "crates" / "mt-app" / "src" / "ui" / "revision.rs")
    state = production_source(REPO / "crates" / "mt-app" / "src" / "state.rs")
    apply = _rust_function_body(state, "pub fn apply_revision(")
    if revision is None or apply is None:
        return False
    return (
        "state.revision_is_stale()" in revision
        and "!done_key_gate.revision_is_stale()" in revision
        and "!snapshot.matches(current_snapshot)" in apply
    )


def source_contract_failure() -> str | None:
    if not production_markers_present(REPO, UI_SOURCE_REQUIREMENTS):
        return "REVISION_SOURCE_CONTRACT_MISSING"
    if not alert_enter_confirms(
        REPO / "crates/mt-app/src/state.rs", "fn request_web_trust(", "trust"
    ):
        return "REVISION_SOURCE_CONTRACT_MISSING"
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
        provider_fixtures.ANSWER_SENTINEL.encode("utf-8"),
        provider_fixtures.ANSWER_SENTINEL.encode("utf-16-le"),
        provider_fixtures.RAW_RESPONSE_SENTINEL.encode("utf-8"),
        provider_fixtures.RAW_RESPONSE_SENTINEL.encode("utf-16-le"),
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

    def _require_foreground(self, app: Any) -> bool:
        self.win32.require_foreground(app.hwnd, self.ui_timeout)
        return True

    def _press_named(self, app: Any, name: str, code: str) -> None:
        self._require_foreground(app)
        control = self.find_named(
            app, name, "Button", f"{code}_UIA_TIMEOUT", f"{code}_UIA_CONTRACT_MISMATCH"
        )
        self.click_control(control, f"{code}_CLICK_FAILED")

    def _named_texts(self, app: Any, automation_id: str, code: str) -> str:
        """The line texts inside one exposed group, joined by newlines."""
        group = self.find_control(
            app, automation_id, "Group", f"{code}_UIA_TIMEOUT", f"{code}_UIA_CONTRACT_MISMATCH"
        )
        try:
            names = [
                child.element_info.name
                for child in group.children()
                if child.element_info.control_type == "Text"
                and isinstance(child.element_info.name, str)
            ]
        except Exception as error:
            raise HarnessFailure(f"{code}_UIA_QUERY_FAILED", safe_exception_name(error)) from None
        if not names:
            raise HarnessFailure(f"{code}_TEXT_UNAVAILABLE")
        return "\n".join(names)

    def _open_consent_gate(
        self,
        provider: provider_fixtures.LoopbackRevisionServer,
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
        provider: provider_fixtures.LoopbackRevisionServer,
        expected_request_count: int | None = None,
        open_gate: Any = None,
    ) -> None:
        send = self.find_control(
            app,
            CONSENT_SEND_AUTOMATION_ID,
            "Button",
            "CONSENT_SEND_UIA_TIMEOUT",
            "CONSENT_SEND_UIA_CONTRACT_MISMATCH",
        )
        if expected_request_count is None or open_gate is None:
            raise HarnessFailure("CONSENT_GATE_CONFIGURATION_INVALID")
        self._open_consent_gate(provider, expected_request_count, open_gate)
        hold = lambda: self.hold_control(app, send, CONSENT_HOLD_SECONDS, failure_code)
        provider.dispatch_consent_click(expected_request_count, hold)

    def run_review(
        self, app: Any, provider: provider_fixtures.LoopbackRevisionServer,
        *, preserve_focus: bool = False,
    ) -> None:
        if preserve_focus:
            if self.win32.foreground_focus(app.hwnd) != app.hwnd:
                raise HarnessFailure("WEB_TO_SOURCE_NATIVE_FOCUS_LOST")
        else:
            self.focus_editor(app)
            self._require_foreground(app)
        self.win32.send_shortcut(app.hwnd, VK_RETURN)
        self._approve_consent(
            app,
            "REVIEW_CONSENT_HOLD_FAILED",
            provider=provider,
            expected_request_count=0,
            open_gate=provider.grant_review_consent,
        )
        self.wait_name(
            app, REVIEW_RESULT_MARKER, "Text", "REVIEW_RESULT_UIA_TIMEOUT", max(self.ui_timeout, 60.0)
        )

    def _set_answer(self, app: Any, value: str) -> None:
        # Type through the focused input, the same path a user takes.
        answer = self.find_control(
            app,
            REVIEW_ANSWER_INPUT_AUTOMATION_ID,
            "Edit",
            "REVIEW_ANSWER_UIA_TIMEOUT",
            "REVIEW_ANSWER_UIA_CONTRACT_MISMATCH",
        )
        try:
            answer.set_focus()
        except Exception as error:
            raise HarnessFailure("REVIEW_ANSWER_FOCUS_FAILED", safe_exception_name(error)) from None
        self.win32.send_unicode(app.hwnd, value)

        def typed() -> bool:
            control = self.control_by_id(
                app.hwnd,
                REVIEW_ANSWER_INPUT_AUTOMATION_ID,
                "Edit",
                "REVIEW_ANSWER_UIA_CONTRACT_MISMATCH",
            )
            return control is not None and control.iface_value.CurrentValue == value

        wait_until(typed, self.ui_timeout, "REVIEW_ANSWER_INPUT_TIMEOUT", interval=0.025)
        self._press_named(app, REVIEW_ANSWER_NAME, "REVIEW_ANSWER_SUBMIT")

    def request_revision(
        self,
        app: Any,
        *,
        provider: provider_fixtures.LoopbackRevisionServer,
        hunks: int,
    ) -> None:
        self._set_answer(app, provider_fixtures.ANSWER_TEXT)
        self._press_named(app, PREPARE_REVISION_NAME, "REVISION_PREPARE")
        self._approve_consent(
            app,
            "REVISION_CONSENT_HOLD_FAILED",
            provider=provider,
            expected_request_count=1,
            open_gate=provider.grant_revision_consent,
        )
        self.wait_name(
            app,
            f"0 of {hunks} decided",
            "Text",
            "REVISION_PROPOSAL_UIA_TIMEOUT",
            max(self.ui_timeout, 60.0),
        )

    def decide_hunks(self, app: Any, decisions: list[bool]) -> None:
        """Decide the docked hunk, in order, waiting for each to be counted."""
        total = len(decisions)
        for index, accepted in enumerate(decisions):
            name = REVISION_ACCEPT_NAME if accepted else REVISION_REJECT_NAME
            self._press_named(app, name, "REVISION_ACCEPT" if accepted else "REVISION_REJECT")
            self.wait_name(
                app, f"{index + 1} of {total} decided", "Text", "REVISION_DECISION_TIMEOUT"
            )

    def done(self, app: Any) -> None:
        self._press_named(app, REVISION_DONE_NAME, "REVISION_DONE")

    def _common_observations(
        self, provider: provider_fixtures.LoopbackRevisionServer
    ) -> dict[str, Any]:
        contract = provider.contract_evidence()
        return {
            "loopback_provider": True,
            "server_loopback": True,
            "server_deterministic": True,
            **contract,
        }

    def _finalize_observations(
        self,
        provider: provider_fixtures.LoopbackRevisionServer,
        case_root: Path,
        app: Any,
        observations: dict[str, Any],
    ) -> dict[str, Any]:
        """Reap the app before the final privacy scan and provider close."""
        self.reap(app)
        observations.update(self._common_observations(provider))
        observations["runtime_scan"] = scan_case_artifacts(case_root, self._credential)
        return observations

    def _expected(self, source: bytes, edits: list[dict[str, Any]]) -> Fingerprint:
        return fingerprint_bytes(apply_edits(source, edits))

    def scenario_reject_all(self) -> dict[str, Any]:
        provider = provider_fixtures.LoopbackRevisionServer(EDITOR_SOURCE_TEXT).start()
        app = None
        try:
            case_root, data, config, workspace, stderr, source = self.profile(
                CASE_REJECT_ALL, provider.base_url
            )
            hunks = len(provider_fixtures.fixture_edits(EDITOR_SOURCE_TEXT))
            app = self.launch_app(source, data, config, workspace, stderr)
            self.activate_source_layout(app)
            editor_before = self.editor_fingerprint(app)
            source_before = sha256_file(source)
            dirty_before = self.document_dirty(app)
            self.run_review(app, provider)
            self.request_revision(app, provider=provider, hunks=hunks)
            self.decide_hunks(app, [False] * hunks)
            self.done(app)
            editor_after = self.editor_fingerprint(app)
            source_after = sha256_file(source)
            dirty_after = self.document_dirty(app)
            observations = {
                "flow": CASE_FLOWS[CASE_REJECT_ALL],
                "process_context": app.security_context.evidence(),
                "foreground_verified": self._require_foreground(app),
                "proposal_received": hunks > 0,
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
            try:
                if app is not None and app.process.poll() is None:
                    self.reap(app)
            finally:
                provider.close()

    def scenario_selective_undo(self) -> dict[str, Any]:
        provider = provider_fixtures.LoopbackRevisionServer(EDITOR_SOURCE_TEXT).start()
        app = None
        try:
            case_root, data, config, workspace, stderr, source = self.profile(
                CASE_SELECTIVE_UNDO, provider.base_url
            )
            edits = provider_fixtures.fixture_edits(EDITOR_SOURCE_TEXT)
            app = self.launch_app(source, data, config, workspace, stderr)
            self.activate_source_layout(app)
            editor_before = self.editor_fingerprint(app)
            source_before = sha256_file(source)
            self.run_review(app, provider)
            self.request_revision(app, provider=provider, hunks=len(edits))
            expected = self._expected(EDITOR_SOURCE_BYTES, edits[:1])
            self.decide_hunks(app, [True] + [False] * (len(edits) - 1))
            self.done(app)
            editor_after_apply = self.editor_fingerprint(app)
            self.focus_editor(app)
            self.win32.send_shortcut(app.hwnd, VK_Z)
            editor_after_undo = self.editor_fingerprint(app)
            source_after = sha256_file(source)
            observations = {
                "flow": CASE_FLOWS[CASE_SELECTIVE_UNDO],
                "process_context": app.security_context.evidence(),
                "foreground_verified": self._require_foreground(app),
                "editor_before": editor_before.evidence(),
                "selective_expected": expected.evidence(),
                "editor_after_apply": editor_after_apply.evidence(),
                "editor_after_undo": editor_after_undo.evidence(),
                "source_before": source_before.evidence(),
                "source_after": source_after.evidence(),
                "selective_matches_expected": editor_after_apply == expected,
                "one_undo_transaction": editor_after_undo == editor_before,
                "undo_count": 1,
            }
            return self._finalize_observations(provider, case_root, app, observations)
        finally:
            try:
                if app is not None and app.process.poll() is None:
                    self.reap(app)
            finally:
                provider.close()

    def scenario_accept_all_preview(self) -> dict[str, Any]:
        provider = provider_fixtures.LoopbackRevisionServer(EDITOR_SOURCE_TEXT).start()
        app = None
        try:
            case_root, data, config, workspace, stderr, source = self.profile(
                CASE_ACCEPT_ALL_PREVIEW, provider.base_url
            )
            edits = provider_fixtures.fixture_edits(EDITOR_SOURCE_TEXT)
            app = self.launch_app(source, data, config, workspace, stderr)
            self.activate_source_layout(app)
            self.run_review(app, provider)
            self.request_revision(app, provider=provider, hunks=len(edits))
            editor_before = self.editor_fingerprint(app)
            expected_bytes = apply_edits(EDITOR_SOURCE_BYTES, edits)
            expected = fingerprint_bytes(expected_bytes)
            self.decide_hunks(app, [True] * len(edits))
            editor_after_preview = self.editor_fingerprint(app)
            copy_editor_before = editor_after_preview
            copy_dirty_before = self.document_dirty(app)
            copy_source_before = sha256_file(source)
            clipboard_before = self.read_text_clipboard()
            try:
                # Clear the clipboard first so an unchanged old value cannot
                # accidentally satisfy the copy assertion.
                self.write_text_clipboard(None)
                copy = self.find_control(
                    app,
                    REVISION_COPY_AUTOMATION_ID,
                    "Button",
                    "REVISION_COPY_UIA_TIMEOUT",
                    "REVISION_COPY_UIA_CONTRACT_MISMATCH",
                )
                self.click_control(copy, "REVISION_COPY_CLICK_FAILED")
                copied = self.wait_clipboard_text("REVISION_COPY_CLIPBOARD_TIMEOUT")
            finally:
                self.write_text_clipboard(clipboard_before)
            copy_editor_after = self.editor_fingerprint(app)
            copy_dirty_after = self.document_dirty(app)
            copy_source_after = sha256_file(source)
            self.done(app)
            editor_after_done = self.editor_fingerprint(app)
            observations = {
                "flow": CASE_FLOWS[CASE_ACCEPT_ALL_PREVIEW],
                "process_context": app.security_context.evidence(),
                "foreground_verified": self._require_foreground(app),
                "editor_before_apply": editor_before.evidence(),
                "editor_after_preview": editor_after_preview.evidence(),
                "accept_all_expected": expected.evidence(),
                "editor_after_done": editor_after_done.evidence(),
                "accept_all_matches_expected": editor_after_done == expected,
                "copy_preview_fingerprint": fingerprint_text(copied).evidence(),
                "copy_preview_exact": copied.encode("utf-8") == expected_bytes,
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
            try:
                if app is not None and app.process.poll() is None:
                    self.reap(app)
            finally:
                provider.close()

    def scenario_stale(self) -> dict[str, Any]:
        provider = provider_fixtures.LoopbackRevisionServer(EDITOR_SOURCE_TEXT).start()
        app = None
        try:
            case_root, data, config, workspace, stderr, source = self.profile(
                CASE_STALE, provider.base_url
            )
            hunks = len(provider_fixtures.fixture_edits(EDITOR_SOURCE_TEXT))
            app = self.launch_app(source, data, config, workspace, stderr)
            self.activate_source_layout(app)
            self.run_review(app, provider)
            self.request_revision(app, provider=provider, hunks=hunks)
            editor_before = self.editor_fingerprint(app)
            self.replace_editor(app, STALE_EDIT_TEXT)
            editor_after_edit = self.editor_fingerprint(app)
            self.wait_name(app, REVISION_STALE_TEXT, "Text", "REVISION_STALE_UIA_TIMEOUT")
            if self.name_present(app, REVISION_ACCEPT_NAME, "Button"):
                raise HarnessFailure("STALE_REVISION_OFFERS_ACCEPT")
            done = self.control_by_name(
                app.hwnd, REVISION_DONE_NAME, "Button", "REVISION_DONE_UIA_CONTRACT_MISMATCH"
            )
            done_enabled = bool(done is not None and done.is_enabled())
            source_before = sha256_file(source)
            if done is not None:
                self.click_control(done, "REVISION_DONE_CLICK_FAILED")
            self.focus_editor(app)
            self.win32.send_shortcut(app.hwnd, VK_RETURN)
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
                "apply_control_observed": done is not None,
                "apply_accessibility_reported_enabled": done_enabled,
                "editor_after_activation_attempt": editor_after_activation_attempt.evidence(),
                "stale_no_mutation": (
                    source_before == source_after
                    and editor_after_edit == editor_after_activation_attempt
                ),
            }
            return self._finalize_observations(provider, case_root, app, observations)
        finally:
            try:
                if app is not None and app.process.poll() is None:
                    self.reap(app)
            finally:
                provider.close()

    def scenario_save_conflict(self) -> dict[str, Any]:
        provider = provider_fixtures.LoopbackRevisionServer(EDITOR_SOURCE_TEXT).start()
        app = None
        try:
            case_root, data, config, workspace, stderr, source = self.profile(
                CASE_SAVE_CONFLICT, provider.base_url
            )
            hunks = len(provider_fixtures.fixture_edits(EDITOR_SOURCE_TEXT))
            app = self.launch_app(source, data, config, workspace, stderr)
            self.activate_source_layout(app)
            self.run_review(app, provider)
            self.request_revision(app, provider=provider, hunks=hunks)
            self.decide_hunks(app, [True] * hunks)
            self.done(app)
            editor_after_apply = self.editor_fingerprint(app)
            write_durable(source, EXTERNAL_SOURCE_BYTES)
            external_source_before = sha256_file(source)
            conflict_visible_before_save = self.name_present(app, CONFLICT_HEADING, "Text")
            self.save_via_menu(app)
            self.wait_name(app, CONFLICT_HEADING, "Text", "SAVE_CONFLICT_UIA_TIMEOUT")
            external_source_after = sha256_file(source)
            observations = {
                "flow": CASE_FLOWS[CASE_SAVE_CONFLICT],
                "process_context": app.security_context.evidence(),
                "foreground_verified": self._require_foreground(app),
                "editor_after_apply": editor_after_apply.evidence(),
                "external_source_before": external_source_before.evidence(),
                "external_source_after": external_source_after.evidence(),
                "save_command_sent": True,
                "conflict_visible_before_save": conflict_visible_before_save,
                "safe_save_conflict_visible": True,
                "safe_save_no_overwrite": external_source_before == external_source_after,
            }
            return self._finalize_observations(provider, case_root, app, observations)
        finally:
            try:
                if app is not None and app.process.poll() is None:
                    self.reap(app)
            finally:
                provider.close()

    def scenario_trust_revoke(self) -> dict[str, Any]:
        provider = provider_fixtures.LoopbackRevisionServer(HTML_EDITOR_SOURCE_TEXT).start()
        app = None
        try:
            case_root, data, config, workspace, stderr, source = self.profile(
                CASE_TRUST_REVOKE, provider.base_url, html=True
            )
            edits = provider_fixtures.fixture_edits(HTML_EDITOR_SOURCE_TEXT)
            app = self.launch_app(source, data, config, workspace, stderr)
            self.menu_command(app, MENU_WEB_AUTOMATION_ID)
            self._press_named(app, TRUST_SCRIPTS_NAME, "TRUST_BUTTON")
            # The Run scripts alert is outside the UIA tree; Enter answers it
            # with Run scripts (`alert_enter_confirms`).
            self.confirm_system_alert(
                app,
                lambda: not self.name_present(app, TRUSTED_BANNER, "Text"),
                "TRUST_ENABLED_BEFORE_CONFIRMATION",
            )
            self.wait_name(app, TRUSTED_BANNER, "Text", "TRUST_ENABLE_STATE_TIMEOUT")
            trust_before = True
            # Leaving the Web pane must hand keyboard ownership back to the
            # main window before the Review shortcut is sent.
            self.menu_command(app, MENU_EDIT_AUTOMATION_ID)
            self.editor_control(app)
            wait_until(
                lambda: self.win32.foreground_focus(app.hwnd) == app.hwnd,
                self.ui_timeout,
                "WEB_TO_SOURCE_NATIVE_FOCUS_TIMEOUT",
            )
            self.run_review(app, provider, preserve_focus=True)
            self.request_revision(app, provider=provider, hunks=len(edits))
            preview_text = self._named_texts(
                app, REVISION_PROPOSAL_AFTER_AUTOMATION_ID, "REVISION_PROPOSAL_AFTER"
            )
            preview = fingerprint_text(preview_text)
            expected_bytes = apply_edits(HTML_EDITOR_SOURCE_BYTES, edits)
            expected = fingerprint_bytes(expected_bytes)
            proposal = fingerprint_text(
                next(
                    line
                    for line in expected_bytes.decode("utf-8").split("\n")
                    if edits[0]["replacement"] in line
                )
            )
            editor_before_apply = self.editor_fingerprint(app)
            self.decide_hunks(app, [True] * len(edits))
            self.done(app)
            editor_after_apply = self.editor_fingerprint(app)
            self.menu_command(app, MENU_WEB_AUTOMATION_ID)
            self.wait_name(app, RESTRICTED_BANNER, "Text", "TRUST_RESTRICTED_STATE_TIMEOUT")
            observations = {
                "flow": CASE_FLOWS[CASE_TRUST_REVOKE],
                "process_context": app.security_context.evidence(),
                "foreground_verified": self._require_foreground(app),
                "trust_before": trust_before,
                "editor_before_apply": editor_before_apply.evidence(),
                "editor_after_apply": editor_after_apply.evidence(),
                "executable_expected": expected.evidence(),
                "executable_matches_expected": editor_after_apply == expected,
                "restricted_after_apply": True,
                "executable_change": editor_before_apply != editor_after_apply,
                "trust_revocation_order_source_contract": trust_apply_source_contract_ok(),
                "preview_inert_source_contract": preview_inert_source_contract_ok(),
                "preview_fingerprint": preview.evidence(),
                "proposal_expected": proposal.evidence(),
                "preview_matches_expected": preview == proposal,
                "preview_contains_executable_text": (
                    "<script>" in preview_text and 'goal07 = "new"' in preview_text
                ),
            }
            # The applied buffer is dirty: quit through the guard, unsaved.
            self.quit_via_menu(app)
            self.click_lifecycle_decision(app, "Discard")
            self.wait_process_exit(app)
            return self._finalize_observations(provider, case_root, app, observations)
        finally:
            try:
                if app is not None and app.process.poll() is None:
                    self.reap(app)
            finally:
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
        return observations


def native_run_plan() -> NativeRunPlan:
    return NativeRunPlan(
        required_case_ids=REQUIRED_CASE_IDS,
        workdir_prefix="markturbo-goal-07-native-",
        new_evidence=new_evidence,
        validate_evidence=validate_evidence,
        source_contract=source_contract_failure,
        finalize_evidence=finalize_evidence,
        preflight=preflight,
        ui_types_loader=load_pywinauto,
        harness_factory=Goal07Harness,
        scenarios=lambda harness: tuple(
            lambda case_id=case_id: harness.scenario(case_id)
            for case_id in REQUIRED_CASE_IDS
        ),
    )


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
