"""Unit tests for the Goal 02 native harness without launching a UI."""

from __future__ import annotations

import copy
import contextlib
import hashlib
import io
import json
import shutil
import struct
import tempfile
import unittest
from pathlib import Path
from unittest import mock

from scripts.markturbo_tools.native import goal02 as HARNESS
from scripts.markturbo_tools.native import runtime

BUILD_CLI_COMMAND = HARNESS.build_cli_command
BUILD_LAUNCH_SPEC = runtime.build_launch_spec
COMPLETE_EVIDENCE = HARNESS.complete_evidence
DOCUMENT_SENTINEL = HARNESS.DOCUMENT_SENTINEL
EXECUTABLE_HASH_FAILURE = runtime.executable_hash_failure
FINGERPRINT_TEXT = runtime.fingerprint_text
HARNESS_BLOCKED = runtime.HarnessBlocked
HARNESS_FAILURE = runtime.HarnessFailure
INSPECT_PE_BYTES = runtime.inspect_pe_bytes
INSPECT_PE_SECTIONS = runtime.inspect_pe_sections
NATIVE_HARNESS = HARNESS.Goal02Harness
NEW_EVIDENCE = HARNESS.new_evidence
NORMALIZE_EXPECTED_HASH = runtime.normalize_expected_hash
PARSE_ARGS = HARNESS.parse_args
PARSE_OUTCOME_LINE = runtime.parse_outcome_line
PLATFORM_PREFLIGHT_FAILURE = runtime.platform_preflight_failure
REQUIRED_CASE_IDS = HARNESS.REQUIRED_CASE_IDS
RUNTIME_ARTIFACT_SCAN = HARNESS.scan_runtime_artifacts
LIVE_RECOVERY_SCAN = HARNESS.scan_live_recovery_records
LAUNCH_FAILURE_CODE = runtime.launch_failure_code
SECURITY_CONTEXT = runtime.SecurityContext
SECURITY_CONTEXT_FAILURE = runtime.security_context_failure
SAFE_EXCEPTION_NAME = runtime.safe_exception_name
SAFE_FAILURE_TYPE = runtime.safe_failure_type
VALIDATE_EVIDENCE = HARNESS.validate_evidence
WAIT_UNTIL = runtime.wait_until
EDITOR_NAME = runtime.EDITOR_NAME
TAB_CLOSE_AUTOMATION_ID = HARNESS.TAB_CLOSE_AUTOMATION_ID
VK_A = runtime.VK_A
VK_BACK = runtime.VK_BACK
VK_C = runtime.VK_C

HASH = "a" * 64
SAME_AS_RAW = object()


def valid_evidence() -> dict:
    evidence = NEW_EVIDENCE(HASH)
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
    evidence["environment"] = {
        "platform": "Windows 11",
        "windows_major": 10,
        "windows_build": 22631,
        "architecture": "x86_64",
        "native_machine_code": 0x8664,
        "python_pointer_bits": 64,
        "wts_state": "WTSActive",
        "input_desktop": "Default",
        "thread_desktop": "Default",
        "harness_process": {
            "session_id": 1,
            "integrity_rid": 0x2000,
            "integrity": "medium",
        },
    }
    process_context = {"session_id": 1, "integrity_rid": 0x2000, "integrity": "medium"}
    runtime_scan = {
        "files_scanned": 4,
        "app_logs_scanned": 1,
        "recovery_artifacts_scanned": 2,
        "canonical_recovery_records_scanned": 1,
        "recovery_leases_scanned": 1,
        "utf8_sentinel_absent": True,
        "utf16le_sentinel_absent": True,
        "panic_absent": True,
        "refcell_absent": True,
    }

    def fp(value: bytes) -> dict[str, int | str]:
        return {"byte_count": len(value), "sha256": hashlib.sha256(value).hexdigest()}

    original = fp(b"orig")
    edited = fp(b"edit")
    external = fp(b"swap")
    canonical_record = fp(b"ciphertext")
    live_recovery_scan = {
        "canonical_record_count": 1,
        "canonical_records": [canonical_record],
        "utf8_sentinel_absent": True,
        "utf16le_sentinel_absent": True,
    }
    observations = {
        REQUIRED_CASE_IDS[0]: {
            "editor": edited,
            "source_before": original,
            "source_after_cancel_save": edited,
            "cancel_kept_tab": True,
            "flow": HARNESS.CASE_FLOWS[REQUIRED_CASE_IDS[0]],
            "process_context": process_context,
            "foreground_verified": True,
            "runtime_scan": runtime_scan,
        },
        REQUIRED_CASE_IDS[1]: {
            "discarded_editor": edited,
            "source_before": original,
            "source_after_discard": original,
            "discard_closed_tab": True,
            "flow": HARNESS.CASE_FLOWS[REQUIRED_CASE_IDS[1]],
            "process_context": process_context,
            "foreground_verified": True,
            "runtime_scan": runtime_scan,
        },
        REQUIRED_CASE_IDS[2]: {
            "editor": edited,
            "alt_f4_cancel_kept_window": True,
            "editor_after_alt_f4_cancel": edited,
            "source_after_alt_f4_cancel": fp(HARNESS.ORIGINAL_BYTES),
            "saved_source": edited,
            "flow": HARNESS.CASE_FLOWS[REQUIRED_CASE_IDS[2]],
            "window_exited": True,
            "process_context": process_context,
            "foreground_verified": True,
            "runtime_scan": runtime_scan,
        },
        REQUIRED_CASE_IDS[3]: {
            "loaded_source": original,
            "external_source": external,
            "dirty_editor": edited,
            "source_after_refused_save": external,
            "editor_after_conflict": edited,
            "saved_after_explicit_overwrite": edited,
            "same_length": True,
            "same_mtime_ns": True,
            "save_refused_with_conflict": True,
            "explicit_overwrite_used": True,
            "flow": HARNESS.CASE_FLOWS[REQUIRED_CASE_IDS[3]],
            "process_context": process_context,
            "foreground_verified": True,
            "runtime_scan": runtime_scan,
        },
        REQUIRED_CASE_IDS[4]: {
            "edited_text": edited,
            "checkpoint_record_seen": True,
            "checkpoint_signal": HARNESS.CHECKPOINT_SIGNAL,
            "live_runtime_scan": runtime_scan,
            "live_recovery_scan": live_recovery_scan,
            "edit_to_signal_ms": 9999.0,
            "recovery_offer_seen": True,
            "restored_editor": edited,
            "recovery_restored_exact": True,
            "restore_ms": 3.0,
            "source_after_recovery_discard": original,
            "restart_editor": original,
            "discarded_recovery_absent": True,
            "discard_restart_absent_ms": 4.0,
            "termination": "TerminateProcess",
            "restart_count": 2,
            "startup_signal": HARNESS.STARTUP_SIGNAL,
            "startup_observed_before_restart_editor": True,
            "flow": HARNESS.CASE_FLOWS[REQUIRED_CASE_IDS[4]],
            "process_contexts": [process_context, process_context, process_context],
            "foreground_verified": True,
            "runtime_scan": runtime_scan,
        },
    }
    for case in evidence["cases"]:
        case["status"] = "PASS"
        case["duration_ms"] = 1.25
        case["observations"] = observations[case["id"]]
    COMPLETE_EVIDENCE(evidence, "PASS")
    return evidence


def minimal_pe(machine: int = 0x8664, magic: int = 0x20B) -> bytes:
    data = bytearray(512)
    data[:2] = b"MZ"
    struct.pack_into("<I", data, 0x3C, 0x80)
    data[0x80:0x84] = b"PE\0\0"
    struct.pack_into("<H", data, 0x84, machine)
    struct.pack_into("<H", data, 0x86, 1)
    struct.pack_into("<H", data, 0x94, 0xF0)
    struct.pack_into("<H", data, 0x98, magic)
    data[0x188:0x190] = b".text\0\0\0"
    struct.pack_into("<I", data, 0x190, 123)
    struct.pack_into("<I", data, 0x198, 512)
    struct.pack_into("<I", data, 0x1AC, 0x60000020)
    return bytes(data)
