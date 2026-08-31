#!/usr/bin/env -S uv run --script
# /// script
# requires-python = ">=3.11"
# dependencies = ["pywinauto==0.6.9"]
# ///
"""Exercise Goal 02 destructive paths through a real Windows UI.

The harness is intentionally fail-closed. It emits PASS only after every
required case runs against the expected x64 release executable in an active,
unlocked Windows 11 session. Document text is never written to stdout or JSON;
observations contain only UTF-8 byte counts, SHA-256 digests, and timings.
"""

from __future__ import annotations

import argparse
import ctypes
import ctypes.wintypes as wt
import datetime as dt
import hashlib
import json
import math
import os
import platform
import re
import shutil
import struct
import subprocess
import sys
import tempfile
import time
from dataclasses import dataclass
from pathlib import Path
from typing import Any, Callable


REPO = Path(__file__).resolve().parent.parent
DEFAULT_EXE = REPO / "target" / "release" / "markturbo.exe"
DEFAULT_EVIDENCE = REPO / ".scratch" / "goal-02-native-acceptance-v1.json"

SCHEMA = "markturbo.goal-02-native-acceptance"
SCHEMA_VERSION = 1
SHA256_RE = re.compile(r"^[0-9a-f]{64}$")
OUTCOME_RE = re.compile(r"^(PASS|FAIL|BLOCKED): ([A-Z0-9_]+)$")
REASON_CODE_RE = re.compile(r"^[A-Z0-9_]+$")
FAILURE_TYPE_RE = re.compile(r"^[A-Za-z_][A-Za-z0-9_]{0,127}$")
SAFE_FAILURE_TYPES = frozenset(
    {
        "AttributeError",
        "COMError",
        "FileExistsError",
        "FileNotFoundError",
        "HarnessFailure",
        "ImportError",
        "IsADirectoryError",
        "KeyError",
        "NotADirectoryError",
        "OSError",
        "PermissionError",
        "RuntimeError",
        "TimeoutError",
        "TimeoutExpired",
        "TypeError",
        "UnknownError",
        "ValueError",
    }
)
SAFE_OBSERVATION_STRINGS = {
    "pointer tab-close -> Cancel -> SendInput Ctrl+S",
    "SendInput Ctrl+W -> Discard",
    "WM_CLOSE -> Save",
    "watcher conflict -> explicit Overwrite",
    "edit -> checkpoint log -> TerminateProcess -> restart -> Discard -> restart after startup log",
    "TerminateProcess",
    "content-free checkpoint success log",
    "content-free recovery startup finished log",
    "untrusted",
    "low",
    "medium",
    "medium-plus",
    "high",
    "system",
    "protected",
}
INTEGRITY_NAMES = {
    "untrusted",
    "low",
    "medium",
    "medium-plus",
    "high",
    "system",
    "protected",
}
ALLOWED_OBSERVATION_KEYS = {
    "app_logs_scanned",
    "byte_count",
    "checkpoint_log_seen",
    "checkpoint_signal",
    "canonical_record_count",
    "canonical_records",
    "canonical_recovery_records_scanned",
    "cancel_kept_tab",
    "dirty_editor",
    "discard_restart_absent_ms",
    "discard_closed_tab",
    "discarded_recovery_absent",
    "discarded_editor",
    "edit_to_signal_ms",
    "edited_text",
    "editor",
    "external_source",
    "explicit_overwrite_used",
    "files_scanned",
    "foreground_verified",
    "flow",
    "integrity",
    "integrity_rid",
    "loaded_source",
    "live_recovery_scan",
    "live_runtime_scan",
    "process_context",
    "process_contexts",
    "recovery_restored_exact",
    "recovery_artifacts_scanned",
    "recovery_leases_scanned",
    "refcell_absent",
    "restart_editor",
    "restart_count",
    "restored_editor",
    "restore_ms",
    "runtime_scan",
    "same_length",
    "same_mtime_ns",
    "saved_after_explicit_overwrite",
    "saved_source",
    "session_id",
    "sha256",
    "source_after_cancel_save",
    "source_after_discard",
    "source_after_recovery_discard",
    "source_before",
    "termination",
    "panic_absent",
    "utf16le_sentinel_absent",
    "utf8_sentinel_absent",
    "startup_finished_log_seen",
    "startup_signal",
    "startup_observed_before_restart_editor",
    "watcher_conflict_before_save",
    "window_exited",
}
CASE_POINTER_CANCEL = "pointer-dirty-tab-close-cancel"
CASE_KEYBOARD_DISCARD = "sendinput-ctrl-w-discard"
CASE_WINDOW_SAVE = "wm-close-save-exact-bytes"
CASE_EXTERNAL_CONFLICT = "same-length-mtime-external-conflict"
CASE_RECOVERY = "cjk-emoji-interruption-recovery-retirement"
REQUIRED_CASE_IDS = (
    CASE_POINTER_CANCEL,
    CASE_KEYBOARD_DISCARD,
    CASE_WINDOW_SAVE,
    CASE_EXTERNAL_CONFLICT,
    CASE_RECOVERY,
)
REQUIRED_OBSERVATION_KEYS = {
    CASE_POINTER_CANCEL: {
        "editor",
        "source_before",
        "source_after_cancel_save",
        "cancel_kept_tab",
        "flow",
        "process_context",
        "foreground_verified",
        "runtime_scan",
    },
    CASE_KEYBOARD_DISCARD: {
        "discarded_editor",
        "source_before",
        "source_after_discard",
        "discard_closed_tab",
        "flow",
        "process_context",
        "foreground_verified",
        "runtime_scan",
    },
    CASE_WINDOW_SAVE: {
        "editor",
        "saved_source",
        "flow",
        "window_exited",
        "process_context",
        "foreground_verified",
        "runtime_scan",
    },
    CASE_EXTERNAL_CONFLICT: {
        "loaded_source",
        "external_source",
        "dirty_editor",
        "saved_after_explicit_overwrite",
        "same_length",
        "same_mtime_ns",
        "watcher_conflict_before_save",
        "explicit_overwrite_used",
        "flow",
        "process_context",
        "foreground_verified",
        "runtime_scan",
    },
    CASE_RECOVERY: {
        "edited_text",
        "checkpoint_log_seen",
        "checkpoint_signal",
        "live_recovery_scan",
        "live_runtime_scan",
        "edit_to_signal_ms",
        "restored_editor",
        "recovery_restored_exact",
        "restore_ms",
        "discard_restart_absent_ms",
        "source_after_recovery_discard",
        "restart_editor",
        "discarded_recovery_absent",
        "termination",
        "restart_count",
        "startup_finished_log_seen",
        "startup_signal",
        "startup_observed_before_restart_editor",
        "flow",
        "process_contexts",
        "foreground_verified",
        "runtime_scan",
    },
}

# Stable GPUI/UIA contract. Lifecycle prompts are native process-owned dialogs;
# their exact app-provided button names are the contract instead of AutomationId.
LAYOUT_SOURCE_AUTOMATION_ID = "markturbo-layout-source"
SOURCE_EDITOR_AUTOMATION_ID = "markturbo-document-source-editor"
TAB_CLOSE_AUTOMATION_ID = "markturbo-document-tab-close"
CONFLICT_OVERWRITE_AUTOMATION_ID = "markturbo-conflict-overwrite"
LIFECYCLE_BUTTON_CONTRACTS = {
    "Save": ("CommandButton_1", "Save"),
    "Discard": ("CommandButton_-2", "Discard"),
    "Cancel": ("CommandButton_2", "Cancel"),
}
LIFECYCLE_CLICK_FAILURE_CODES = {
    "Save": "LIFECYCLE_SAVE_CLICK_FAILED",
    "Discard": "LIFECYCLE_DISCARD_CLICK_FAILED",
    "Cancel": "LIFECYCLE_CANCEL_CLICK_FAILED",
}
TASK_DIALOG_CLASS = "#32770"
TASK_DIALOG_BUTTON_CLASS = "CCPushButton"

CASE_FLOWS = {
    CASE_POINTER_CANCEL: "pointer tab-close -> Cancel -> SendInput Ctrl+S",
    CASE_KEYBOARD_DISCARD: "SendInput Ctrl+W -> Discard",
    CASE_WINDOW_SAVE: "WM_CLOSE -> Save",
    CASE_EXTERNAL_CONFLICT: "watcher conflict -> explicit Overwrite",
    CASE_RECOVERY: (
        "edit -> checkpoint log -> TerminateProcess -> restart -> Discard -> "
        "restart after startup log"
    ),
}

DOCUMENT_SENTINEL = "MTG02-NATIVE-SENTINEL-\u4fdd\u5b58-\U0001f680"
RECOVERY_CHECKPOINT_LOG_RE = re.compile(
    rb"(?mi)^.*\brecovery checkpoint written\s*$"
)
RECOVERY_STARTUP_FINISHED_LOG_RE = re.compile(
    rb"(?mi)^.*\brecovery startup finished\s*$"
)
CANONICAL_RECOVERY_RECORD_RE = re.compile(r"^[0-9a-f]{64}\.mtrecovery$")

ORIGINAL_BYTES = b"native acceptance original"
EXTERNAL_BYTES = b"native acceptance external"
POINTER_EDIT = f"{DOCUMENT_SENTINEL}:pointer"
KEYBOARD_EDIT = f"{DOCUMENT_SENTINEL}:keyboard"
WINDOW_EDIT = f"{DOCUMENT_SENTINEL}:window"
CONFLICT_EDIT = f"{DOCUMENT_SENTINEL}:conflict"
RECOVERY_EDIT = f"{DOCUMENT_SENTINEL}:recovery"

WM_CLOSE = 0x0010
GW_OWNER = 4
SW_RESTORE = 9
VK_CONTROL = 0x11
VK_A = 0x41
VK_BACK = 0x08
VK_S = 0x53
VK_W = 0x57
KEYEVENTF_KEYUP = 0x0002
KEYEVENTF_UNICODE = 0x0004
INPUT_KEYBOARD = 1
PROCESS_QUERY_LIMITED_INFORMATION = 0x1000
PROCESS_TERMINATE = 0x0001
TOKEN_QUERY = 0x0008
TOKEN_INTEGRITY_LEVEL = 25
DESKTOP_READOBJECTS = 0x0001
DESKTOP_SWITCHDESKTOP = 0x0100
UOI_NAME = 2
WTS_CONNECT_STATE = 8
WTS_ACTIVE = 0
IMAGE_FILE_MACHINE_AMD64 = 0x8664
PE32_PLUS_MAGIC = 0x20B


class HarnessBlocked(RuntimeError):
    def __init__(self, code: str, detail: str = "") -> None:
        super().__init__(code)
        self.code = code
        self.detail = detail


class HarnessFailure(RuntimeError):
    def __init__(self, code: str, detail: str = "") -> None:
        super().__init__(code)
        self.code = code
        self.detail = detail


@dataclass(frozen=True)
class Fingerprint:
    byte_count: int
    sha256: str

    def evidence(self) -> dict[str, int | str]:
        return {"byte_count": self.byte_count, "sha256": self.sha256}


@dataclass(frozen=True)
class SecurityContext:
    session_id: int
    integrity_rid: int
    integrity_name: str

    def evidence(self) -> dict[str, int | str]:
        return {
            "session_id": self.session_id,
            "integrity_rid": self.integrity_rid,
            "integrity": self.integrity_name,
        }


@dataclass(frozen=True)
class LaunchSpec:
    args: tuple[str, ...]
    cwd: str
    env: dict[str, str]
    stderr_path: Path


@dataclass
class RunningApp:
    process: subprocess.Popen[bytes]
    window: Any
    hwnd: int
    spec: LaunchSpec
    security_context: SecurityContext
    app_log_path: Path


class MOUSEINPUT(ctypes.Structure):
    _fields_ = [
        ("dx", wt.LONG),
        ("dy", wt.LONG),
        ("mouseData", wt.DWORD),
        ("dwFlags", wt.DWORD),
        ("time", wt.DWORD),
        ("dwExtraInfo", ctypes.c_size_t),
    ]


class KEYBDINPUT(ctypes.Structure):
    _fields_ = [
        ("wVk", wt.WORD),
        ("wScan", wt.WORD),
        ("dwFlags", wt.DWORD),
        ("time", wt.DWORD),
        ("dwExtraInfo", ctypes.c_size_t),
    ]


class HARDWAREINPUT(ctypes.Structure):
    _fields_ = [("uMsg", wt.DWORD), ("wParamL", wt.WORD), ("wParamH", wt.WORD)]


class INPUT_UNION(ctypes.Union):
    _fields_ = [("mi", MOUSEINPUT), ("ki", KEYBDINPUT), ("hi", HARDWAREINPUT)]


class INPUT(ctypes.Structure):
    _anonymous_ = ("value",)
    _fields_ = [("type", wt.DWORD), ("value", INPUT_UNION)]


class SID_AND_ATTRIBUTES(ctypes.Structure):
    _fields_ = [("Sid", wt.LPVOID), ("Attributes", wt.DWORD)]


class TOKEN_MANDATORY_LABEL_STRUCT(ctypes.Structure):
    _fields_ = [("Label", SID_AND_ATTRIBUTES)]


def utc_now() -> str:
    return dt.datetime.now(dt.timezone.utc).isoformat(timespec="milliseconds").replace(
        "+00:00", "Z"
    )


def fingerprint_bytes(value: bytes) -> Fingerprint:
    return Fingerprint(len(value), hashlib.sha256(value).hexdigest())


def fingerprint_text(value: str) -> Fingerprint:
    return fingerprint_bytes(value.encode("utf-8"))


def sha256_file(path: Path) -> Fingerprint:
    digest = hashlib.sha256()
    byte_count = 0
    with path.open("rb") as handle:
        while chunk := handle.read(1024 * 1024):
            byte_count += len(chunk)
            digest.update(chunk)
    return Fingerprint(byte_count, digest.hexdigest())


def normalize_expected_hash(value: str) -> str:
    normalized = value.strip().lower()
    if not SHA256_RE.fullmatch(normalized):
        raise argparse.ArgumentTypeError("expected SHA-256 must be exactly 64 hex digits")
    return normalized


def executable_hash_failure(actual: str, expected: str) -> str | None:
    if not SHA256_RE.fullmatch(actual):
        return "EXECUTABLE_HASH_INVALID"
    if not SHA256_RE.fullmatch(expected):
        return "EXPECTED_HASH_INVALID"
    return None if actual == expected else "EXECUTABLE_HASH_MISMATCH"


def inspect_pe_bytes(data: bytes) -> dict[str, int | str]:
    if len(data) < 0x40 or data[:2] != b"MZ":
        raise ValueError("missing DOS header")
    pe_offset = struct.unpack_from("<I", data, 0x3C)[0]
    if pe_offset > len(data) - 26 or data[pe_offset : pe_offset + 4] != b"PE\0\0":
        raise ValueError("missing PE header")
    machine = struct.unpack_from("<H", data, pe_offset + 4)[0]
    optional_size = struct.unpack_from("<H", data, pe_offset + 20)[0]
    if optional_size < 2 or pe_offset + 24 + optional_size > len(data):
        raise ValueError("truncated optional header")
    optional_magic = struct.unpack_from("<H", data, pe_offset + 24)[0]
    if machine != IMAGE_FILE_MACHINE_AMD64 or optional_magic != PE32_PLUS_MAGIC:
        raise ValueError("executable is not AMD64 PE32+")
    return {
        "format": "PE32+",
        "machine": "x86_64",
        "machine_code": machine,
        "optional_magic": optional_magic,
    }


def inspect_pe(path: Path) -> dict[str, int | str]:
    with path.open("rb") as handle:
        header = handle.read(1024 * 1024)
    return inspect_pe_bytes(header)


def platform_preflight_failure(
    system: str,
    windows_major: int,
    windows_build: int,
    native_machine: str,
    pointer_bits: int,
) -> str | None:
    if system != "Windows":
        return "WINDOWS_REQUIRED"
    if windows_major != 10 or windows_build < 22000:
        return "WINDOWS_11_REQUIRED"
    if native_machine.lower() not in {"amd64", "x86_64"} or pointer_bits != 64:
        return "WINDOWS_X64_REQUIRED"
    return None


def security_context_failure(parent: SecurityContext, child: SecurityContext) -> str | None:
    if parent.session_id != child.session_id:
        return "PROCESS_SESSION_MISMATCH"
    if parent.integrity_rid != child.integrity_rid:
        return "PROCESS_INTEGRITY_MISMATCH"
    return None


def parse_outcome_line(line: str) -> tuple[str, str]:
    match = OUTCOME_RE.fullmatch(line.strip())
    if match is None:
        raise ValueError("invalid harness outcome line")
    return match.group(1), match.group(2)


def safe_exception_name(error: BaseException) -> str:
    """Return only a type name; exception strings can contain UI document text."""
    name = type(error).__name__
    return name if FAILURE_TYPE_RE.fullmatch(name) else "UnknownError"


def safe_failure_type(error: HarnessFailure) -> str:
    candidate = error.detail or safe_exception_name(error)
    return candidate if candidate in SAFE_FAILURE_TYPES else safe_exception_name(error)


def new_evidence(expected_hash: str) -> dict[str, Any]:
    return {
        "schema": SCHEMA,
        "schema_version": SCHEMA_VERSION,
        "status": "BLOCKED",
        "started_at_utc": utc_now(),
        "completed_at_utc": None,
        "executable": {
            "expected_sha256": expected_hash,
            "sha256": None,
            "byte_count": None,
            "hash_verified": False,
            "copied_sha256": None,
            "copy_hash_verified": False,
            "format": None,
            "machine": None,
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
        "summary": {
            "required_case_count": len(REQUIRED_CASE_IDS),
            "passed_case_count": 0,
            "blocked_case_count": 0,
            "failed_case_count": 0,
            "not_run_case_count": len(REQUIRED_CASE_IDS),
        },
    }


def complete_evidence(evidence: dict[str, Any], status: str) -> None:
    evidence["status"] = status
    evidence["completed_at_utc"] = utc_now()
    cases = evidence["cases"]
    evidence["summary"] = {
        "required_case_count": len(REQUIRED_CASE_IDS),
        "passed_case_count": sum(case["status"] == "PASS" for case in cases),
        "blocked_case_count": sum(case["status"] == "BLOCKED" for case in cases),
        "failed_case_count": sum(case["status"] == "FAIL" for case in cases),
        "not_run_case_count": sum(case["status"] == "NOT_RUN" for case in cases),
    }


def validate_evidence(evidence: dict[str, Any]) -> None:
    if evidence.get("schema") != SCHEMA or evidence.get("schema_version") != SCHEMA_VERSION:
        raise ValueError("unsupported evidence schema")
    if evidence.get("status") not in {"PASS", "FAIL", "BLOCKED"}:
        raise ValueError("invalid evidence status")

    executable = evidence.get("executable")
    if not isinstance(executable, dict):
        raise ValueError("missing executable evidence")
    expected_hash = executable.get("expected_sha256")
    actual_hash = executable.get("sha256")
    if not isinstance(expected_hash, str) or not SHA256_RE.fullmatch(expected_hash):
        raise ValueError("invalid expected executable SHA-256")
    if actual_hash is not None and (
        not isinstance(actual_hash, str) or not SHA256_RE.fullmatch(actual_hash)
    ):
        raise ValueError("invalid executable SHA-256")
    if executable.get("hash_verified") is True and actual_hash != expected_hash:
        raise ValueError("verified executable hash does not match expected hash")
    if evidence["status"] == "PASS" and executable.get("hash_verified") is not True:
        raise ValueError("PASS requires a verified executable hash")
    copied_hash = executable.get("copied_sha256")
    if copied_hash is not None and (
        not isinstance(copied_hash, str) or not SHA256_RE.fullmatch(copied_hash)
    ):
        raise ValueError("invalid copied executable SHA-256")
    if executable.get("copy_hash_verified") is True and copied_hash != expected_hash:
        raise ValueError("verified copied executable hash does not match expected hash")
    if evidence["status"] == "PASS" and executable.get("copy_hash_verified") is not True:
        raise ValueError("PASS requires a verified copied executable hash")
    if evidence["status"] == "PASS" and (
        not isinstance(executable.get("byte_count"), int) or executable["byte_count"] <= 0
    ):
        raise ValueError("PASS requires a nonempty executable")
    if evidence["status"] == "PASS" and (
        executable.get("format") != "PE32+"
        or executable.get("machine") != "x86_64"
        or executable.get("machine_code") != IMAGE_FILE_MACHINE_AMD64
        or executable.get("optional_magic") != PE32_PLUS_MAGIC
    ):
        raise ValueError("PASS requires an AMD64 PE32+ executable")

    environment = evidence.get("environment")
    if not isinstance(environment, dict):
        raise ValueError("invalid environment evidence")
    parent_context = environment.get("harness_process")
    if evidence["status"] == "PASS":
        validate_environment(environment)
        validate_process_context(parent_context)

    cases = evidence.get("cases")
    if not isinstance(cases, list):
        raise ValueError("missing case evidence")
    ids = [case.get("id") for case in cases if isinstance(case, dict)]
    duplicates = sorted({case_id for case_id in ids if ids.count(case_id) > 1})
    if duplicates:
        raise ValueError("duplicate required case ids")
    missing = sorted(set(REQUIRED_CASE_IDS) - set(ids))
    extra = sorted(set(ids) - set(REQUIRED_CASE_IDS))
    if missing or extra or len(cases) != len(REQUIRED_CASE_IDS):
        raise ValueError("required case set is incomplete")
    for case in cases:
        if case.get("status") not in {"PASS", "FAIL", "BLOCKED", "NOT_RUN"}:
            raise ValueError("invalid case status")
        reason_code = case.get("reason_code")
        if reason_code is not None and (
            not isinstance(reason_code, str) or not REASON_CODE_RE.fullmatch(reason_code)
        ):
            raise ValueError("invalid case reason code")
        failure_type = case.get("failure_type")
        if failure_type is not None and (
            not isinstance(failure_type, str) or failure_type not in SAFE_FAILURE_TYPES
        ):
            raise ValueError("invalid case failure type")
        if case["status"] != "FAIL" and failure_type is not None:
            raise ValueError("only failed cases may contain a failure type")
        observations = case.get("observations")
        if not isinstance(observations, dict):
            raise ValueError("invalid case observations")
        if case["status"] == "PASS" and not REQUIRED_OBSERVATION_KEYS[case["id"]].issubset(
            observations
        ):
            raise ValueError("passed case evidence is incomplete")
        validate_observations(observations)
        duration_ms = case.get("duration_ms")
        if case["status"] == "NOT_RUN":
            if duration_ms is not None:
                raise ValueError("NOT_RUN case cannot have a duration")
        elif not finite_nonnegative(duration_ms):
            raise ValueError("completed case duration must be nonnegative")
        if case["status"] == "PASS":
            validate_passed_case(case["id"], observations, parent_context)
    if evidence["status"] == "PASS" and any(case["status"] != "PASS" for case in cases):
        raise ValueError("PASS requires every required case to pass")

    summary = evidence.get("summary")
    if not isinstance(summary, dict) or summary.get("required_case_count") != len(
        REQUIRED_CASE_IDS
    ):
        raise ValueError("invalid evidence summary")
    expected_counts = {
        "passed_case_count": sum(case["status"] == "PASS" for case in cases),
        "blocked_case_count": sum(case["status"] == "BLOCKED" for case in cases),
        "failed_case_count": sum(case["status"] == "FAIL" for case in cases),
        "not_run_case_count": sum(case["status"] == "NOT_RUN" for case in cases),
    }
    if any(summary.get(key) != value for key, value in expected_counts.items()):
        raise ValueError("case summary does not match case evidence")


def validate_observations(value: Any, key: str | None = None) -> None:
    if isinstance(value, dict):
        for key, nested in value.items():
            if key not in ALLOWED_OBSERVATION_KEYS:
                raise ValueError("unknown observation field")
            if key == "sha256" and (
                not isinstance(nested, str) or not SHA256_RE.fullmatch(nested)
            ):
                raise ValueError("invalid observation SHA-256")
            validate_observations(nested, key)
    elif isinstance(value, list):
        for nested in value:
            validate_observations(nested, key)
    elif isinstance(value, str):
        if key == "sha256":
            return
        if value not in SAFE_OBSERVATION_STRINGS:
            raise ValueError("free-form observation strings are forbidden")
    elif value is not None and not isinstance(value, (bool, int, float)):
        raise ValueError("invalid observation value")


def validate_fingerprint(value: Any) -> dict[str, Any]:
    if not isinstance(value, dict):
        raise ValueError("missing fingerprint evidence")
    byte_count = value.get("byte_count")
    digest = value.get("sha256")
    if not isinstance(byte_count, int) or isinstance(byte_count, bool) or byte_count < 0:
        raise ValueError("invalid fingerprint byte count")
    if not isinstance(digest, str) or not SHA256_RE.fullmatch(digest):
        raise ValueError("invalid fingerprint SHA-256")
    return value


def finite_nonnegative(value: Any) -> bool:
    return (
        isinstance(value, (int, float))
        and not isinstance(value, bool)
        and math.isfinite(value)
        and value >= 0
    )


def validate_process_context(value: Any) -> dict[str, Any]:
    if not isinstance(value, dict):
        raise ValueError("missing process context evidence")
    if not isinstance(value.get("session_id"), int) or value["session_id"] < 0:
        raise ValueError("invalid process session evidence")
    if not isinstance(value.get("integrity_rid"), int) or value["integrity_rid"] <= 0:
        raise ValueError("invalid process integrity evidence")
    if value.get("integrity") not in INTEGRITY_NAMES:
        raise ValueError("invalid process integrity name")
    return value


def validate_environment(value: dict[str, Any]) -> None:
    if not value:
        raise ValueError("PASS requires nonempty environment evidence")
    if (
        value.get("platform") != "Windows 11"
        or value.get("windows_major") != 10
        or not isinstance(value.get("windows_build"), int)
        or value["windows_build"] < 22000
    ):
        raise ValueError("PASS requires Windows 11 build 22000 or newer")
    if (
        value.get("architecture") != "x86_64"
        or value.get("native_machine_code") != IMAGE_FILE_MACHINE_AMD64
        or value.get("python_pointer_bits") != 64
    ):
        raise ValueError("PASS requires x64 OS and Python process evidence")
    if value.get("wts_state") != "WTSActive":
        raise ValueError("PASS requires WTSActive evidence")
    for key in ("input_desktop", "thread_desktop"):
        if not isinstance(value.get(key), str) or not value[key].strip():
            raise ValueError("PASS requires nonempty desktop evidence")
    if (
        value["input_desktop"].casefold() != "default"
        or value["thread_desktop"].casefold() != value["input_desktop"].casefold()
    ):
        raise ValueError("PASS requires the unlocked Default input desktop")


def require_true(observations: dict[str, Any], *keys: str) -> None:
    for key in keys:
        if observations.get(key) is not True:
            raise ValueError(f"passed case requires true {key}")


def validate_runtime_scan(value: Any) -> None:
    if not isinstance(value, dict):
        raise ValueError("missing runtime scan evidence")
    if not isinstance(value.get("files_scanned"), int) or value["files_scanned"] <= 0:
        raise ValueError("runtime scan must cover at least one file")
    if not isinstance(value.get("app_logs_scanned"), int) or value["app_logs_scanned"] <= 0:
        raise ValueError("runtime scan must cover an application log")
    if (
        not isinstance(value.get("recovery_artifacts_scanned"), int)
        or value["recovery_artifacts_scanned"] < 0
    ):
        raise ValueError("invalid recovery artifact scan count")
    for key in ("canonical_recovery_records_scanned", "recovery_leases_scanned"):
        if not isinstance(value.get(key), int) or value[key] < 0:
            raise ValueError(f"invalid {key}")
    if value["canonical_recovery_records_scanned"] + value["recovery_leases_scanned"] > value[
        "recovery_artifacts_scanned"
    ]:
        raise ValueError("runtime scan recovery artifact counts are inconsistent")
    if value["files_scanned"] < (
        value["app_logs_scanned"] + value["recovery_artifacts_scanned"] + 1
    ):
        raise ValueError("runtime scan file count is incomplete")
    require_true(
        value,
        "utf8_sentinel_absent",
        "utf16le_sentinel_absent",
        "panic_absent",
        "refcell_absent",
    )


def validate_live_recovery_scan(value: Any) -> None:
    if not isinstance(value, dict):
        raise ValueError("missing live recovery scan evidence")
    count = value.get("canonical_record_count")
    records = value.get("canonical_records")
    if not isinstance(count, int) or count <= 0:
        raise ValueError("live recovery scan requires a canonical record")
    if not isinstance(records, list) or len(records) != count:
        raise ValueError("live recovery record count does not match fingerprints")
    for record in records:
        fingerprint = validate_fingerprint(record)
        if fingerprint["byte_count"] <= 0:
            raise ValueError("canonical recovery record must be nonempty")
    require_true(value, "utf8_sentinel_absent", "utf16le_sentinel_absent")


def validate_passed_case(
    case_id: str, observations: dict[str, Any], parent_context: Any
) -> None:
    parent = validate_process_context(parent_context)
    contexts = observations.get("process_contexts")
    if contexts is None:
        contexts = [observations.get("process_context")]
    if not isinstance(contexts, list) or not contexts:
        raise ValueError("passed case requires process context")
    for context in contexts:
        if validate_process_context(context) != parent:
            raise ValueError("case process context differs from harness context")
    if observations.get("flow") != CASE_FLOWS[case_id]:
        raise ValueError("case flow mechanics do not match the required scenario")
    require_true(observations, "foreground_verified")
    validate_runtime_scan(observations.get("runtime_scan"))

    if case_id == CASE_POINTER_CANCEL:
        editor = validate_fingerprint(observations.get("editor"))
        saved = validate_fingerprint(observations.get("source_after_cancel_save"))
        validate_fingerprint(observations.get("source_before"))
        require_true(observations, "cancel_kept_tab")
        if editor != saved:
            raise ValueError("Cancel did not preserve exact editor bytes")
    elif case_id == CASE_KEYBOARD_DISCARD:
        before = validate_fingerprint(observations.get("source_before"))
        after = validate_fingerprint(observations.get("source_after_discard"))
        validate_fingerprint(observations.get("discarded_editor"))
        require_true(observations, "discard_closed_tab")
        if before != after:
            raise ValueError("Discard changed source bytes")
    elif case_id == CASE_WINDOW_SAVE:
        editor = validate_fingerprint(observations.get("editor"))
        saved = validate_fingerprint(observations.get("saved_source"))
        require_true(observations, "window_exited")
        if editor != saved:
            raise ValueError("window Save did not preserve exact editor bytes")
    elif case_id == CASE_EXTERNAL_CONFLICT:
        loaded = validate_fingerprint(observations.get("loaded_source"))
        external = validate_fingerprint(observations.get("external_source"))
        dirty = validate_fingerprint(observations.get("dirty_editor"))
        saved = validate_fingerprint(observations.get("saved_after_explicit_overwrite"))
        require_true(
            observations,
            "same_length",
            "same_mtime_ns",
            "watcher_conflict_before_save",
            "explicit_overwrite_used",
        )
        if loaded["byte_count"] != external["byte_count"] or loaded["sha256"] == external["sha256"]:
            raise ValueError("external rewrite evidence is not same-length and different")
        if dirty != saved or external == saved:
            raise ValueError("explicit overwrite did not preserve exact dirty editor bytes")
    elif case_id == CASE_RECOVERY:
        edited = validate_fingerprint(observations.get("edited_text"))
        restored = validate_fingerprint(observations.get("restored_editor"))
        after_discard = validate_fingerprint(observations.get("source_after_recovery_discard"))
        restarted = validate_fingerprint(observations.get("restart_editor"))
        require_true(
            observations,
            "checkpoint_log_seen",
            "recovery_restored_exact",
            "discarded_recovery_absent",
            "startup_finished_log_seen",
            "startup_observed_before_restart_editor",
        )
        if observations.get("checkpoint_signal") != "content-free checkpoint success log":
            raise ValueError("recovery checkpoint signal is not the required log")
        if observations.get("startup_signal") != "content-free recovery startup finished log":
            raise ValueError("recovery startup signal is not the required log")
        if observations.get("termination") != "TerminateProcess":
            raise ValueError("recovery interruption did not use TerminateProcess")
        if observations.get("restart_count") != 2 or len(contexts) != 3:
            raise ValueError("recovery scenario requires two restarts")
        validate_live_recovery_scan(observations.get("live_recovery_scan"))
        validate_runtime_scan(observations.get("live_runtime_scan"))
        if observations["live_runtime_scan"]["canonical_recovery_records_scanned"] < observations[
            "live_recovery_scan"
        ]["canonical_record_count"]:
            raise ValueError("live runtime scan omitted canonical recovery records")
        if observations["live_runtime_scan"]["recovery_leases_scanned"] < 1:
            raise ValueError("live runtime scan omitted recovery lease")
        signal_ms = observations.get("edit_to_signal_ms")
        if not finite_nonnegative(signal_ms) or signal_ms > 10_000:
            raise ValueError("recovery success signal exceeds 10000ms")
        for key in ("restore_ms", "discard_restart_absent_ms"):
            value = observations.get(key)
            if not finite_nonnegative(value):
                raise ValueError("recovery timing must be nonnegative")
        if edited != restored:
            raise ValueError("recovery did not restore exact editor bytes")
        if after_discard != restarted or restarted == edited:
            raise ValueError("discarded recovery was present after restart")


def write_evidence(path: Path, evidence: dict[str, Any]) -> None:
    validate_evidence(evidence)
    path = path.resolve()
    path.parent.mkdir(parents=True, exist_ok=True)
    temporary = path.with_name(f".{path.name}.{os.getpid()}.tmp")
    try:
        with temporary.open("w", encoding="utf-8", newline="\n") as handle:
            json.dump(evidence, handle, indent=2, sort_keys=True, ensure_ascii=True)
            handle.write("\n")
            handle.flush()
            os.fsync(handle.fileno())
        os.replace(temporary, path)
    finally:
        temporary.unlink(missing_ok=True)


def build_launch_spec(
    copied_exe: Path,
    target: Path,
    data_root: Path,
    config_root: Path,
    workspace_root: Path,
    stderr_path: Path,
    base_env: dict[str, str] | None = None,
) -> LaunchSpec:
    paths = [copied_exe, target, data_root, config_root, workspace_root, stderr_path]
    if any(not path.is_absolute() for path in paths):
        raise ValueError("launch paths must be absolute")
    env = dict(os.environ if base_env is None else base_env)
    for secret in ("ANTHROPIC_API_KEY", "OPENAI_API_KEY", "MARKTURBO_TRANSLATE_MODEL"):
        env.pop(secret, None)
    env.update(
        {
            "MARKTURBO_DATA_DIR": str(data_root),
            "MARKTURBO_CONFIG_DIR": str(config_root),
            "RUST_LOG": "debug",
        }
    )
    return LaunchSpec(
        args=(str(copied_exe), str(target)),
        cwd=str(workspace_root),
        env=env,
        stderr_path=stderr_path,
    )


def build_cli_command(exe: Path, expected_hash: str, evidence: Path) -> list[str]:
    return [
        sys.executable,
        str(Path(__file__).resolve()),
        "--exe",
        str(exe),
        "--expect-exe-sha256",
        expected_hash,
        "--evidence",
        str(evidence),
    ]


def load_pywinauto() -> Any:
    try:
        from pywinauto import Application
        from pywinauto.controls.uiawrapper import UIAWrapper
        from pywinauto.uia_defines import IUIA, NoPatternInterfaceError
        from pywinauto.uia_element_info import UIAElementInfo
    except (ImportError, OSError) as error:
        raise HarnessBlocked(
            "PYWINAUTO_UNAVAILABLE", f"pywinauto import failed ({safe_exception_name(error)})"
        ) from None
    return Application, UIAElementInfo, UIAWrapper, IUIA, NoPatternInterfaceError


class Win32:
    def __init__(self) -> None:
        self.kernel32 = ctypes.WinDLL("kernel32", use_last_error=True)
        self.user32 = ctypes.WinDLL("user32", use_last_error=True)
        self.ntdll = ctypes.WinDLL("ntdll", use_last_error=True)
        self.wtsapi32 = ctypes.WinDLL("wtsapi32", use_last_error=True)
        self.advapi32 = ctypes.WinDLL("advapi32", use_last_error=True)
        self._declare()

    def _declare(self) -> None:
        self.kernel32.GetCurrentProcess.restype = wt.HANDLE
        self.kernel32.GetCurrentThreadId.restype = wt.DWORD
        self.kernel32.ProcessIdToSessionId.argtypes = [wt.DWORD, ctypes.POINTER(wt.DWORD)]
        self.kernel32.ProcessIdToSessionId.restype = wt.BOOL
        self.kernel32.OpenProcess.argtypes = [wt.DWORD, wt.BOOL, wt.DWORD]
        self.kernel32.OpenProcess.restype = wt.HANDLE
        self.kernel32.CloseHandle.argtypes = [wt.HANDLE]
        self.kernel32.CloseHandle.restype = wt.BOOL
        self.kernel32.WTSGetActiveConsoleSessionId.restype = wt.DWORD
        self.kernel32.IsWow64Process2.argtypes = [
            wt.HANDLE,
            ctypes.POINTER(wt.WORD),
            ctypes.POINTER(wt.WORD),
        ]
        self.kernel32.IsWow64Process2.restype = wt.BOOL
        self.kernel32.TerminateProcess.argtypes = [wt.HANDLE, wt.UINT]
        self.kernel32.TerminateProcess.restype = wt.BOOL

        self.ntdll.RtlGetVersion.argtypes = [wt.LPVOID]
        self.ntdll.RtlGetVersion.restype = wt.LONG

        self.advapi32.OpenProcessToken.argtypes = [wt.HANDLE, wt.DWORD, ctypes.POINTER(wt.HANDLE)]
        self.advapi32.OpenProcessToken.restype = wt.BOOL
        self.advapi32.GetTokenInformation.argtypes = [
            wt.HANDLE,
            ctypes.c_int,
            wt.LPVOID,
            wt.DWORD,
            ctypes.POINTER(wt.DWORD),
        ]
        self.advapi32.GetTokenInformation.restype = wt.BOOL
        self.advapi32.GetSidSubAuthorityCount.argtypes = [wt.LPVOID]
        self.advapi32.GetSidSubAuthorityCount.restype = ctypes.POINTER(ctypes.c_ubyte)
        self.advapi32.GetSidSubAuthority.argtypes = [wt.LPVOID, wt.DWORD]
        self.advapi32.GetSidSubAuthority.restype = ctypes.POINTER(wt.DWORD)

        self.wtsapi32.WTSQuerySessionInformationW.argtypes = [
            wt.HANDLE,
            wt.DWORD,
            ctypes.c_int,
            ctypes.POINTER(wt.LPWSTR),
            ctypes.POINTER(wt.DWORD),
        ]
        self.wtsapi32.WTSQuerySessionInformationW.restype = wt.BOOL
        self.wtsapi32.WTSFreeMemory.argtypes = [wt.LPVOID]

        self.user32.OpenInputDesktop.argtypes = [wt.DWORD, wt.BOOL, wt.DWORD]
        self.user32.OpenInputDesktop.restype = wt.HANDLE
        self.user32.GetThreadDesktop.argtypes = [wt.DWORD]
        self.user32.GetThreadDesktop.restype = wt.HANDLE
        self.user32.GetUserObjectInformationW.argtypes = [
            wt.HANDLE,
            ctypes.c_int,
            wt.LPVOID,
            wt.DWORD,
            ctypes.POINTER(wt.DWORD),
        ]
        self.user32.GetUserObjectInformationW.restype = wt.BOOL
        self.user32.SwitchDesktop.argtypes = [wt.HANDLE]
        self.user32.SwitchDesktop.restype = wt.BOOL
        self.user32.CloseDesktop.argtypes = [wt.HANDLE]
        self.user32.CloseDesktop.restype = wt.BOOL
        self.user32.ShowWindow.argtypes = [wt.HWND, ctypes.c_int]
        self.user32.ShowWindow.restype = wt.BOOL
        self.user32.SetForegroundWindow.argtypes = [wt.HWND]
        self.user32.SetForegroundWindow.restype = wt.BOOL
        self.user32.BringWindowToTop.argtypes = [wt.HWND]
        self.user32.BringWindowToTop.restype = wt.BOOL
        self.user32.GetForegroundWindow.restype = wt.HWND
        self._enum_windows_proc = ctypes.WINFUNCTYPE(wt.BOOL, wt.HWND, wt.LPARAM)
        self.user32.EnumWindows.argtypes = [self._enum_windows_proc, wt.LPARAM]
        self.user32.EnumWindows.restype = wt.BOOL
        self.user32.GetWindow.argtypes = [wt.HWND, wt.UINT]
        self.user32.GetWindow.restype = wt.HWND
        self.user32.GetWindowThreadProcessId.argtypes = [wt.HWND, ctypes.POINTER(wt.DWORD)]
        self.user32.GetWindowThreadProcessId.restype = wt.DWORD
        self.user32.IsWindowVisible.argtypes = [wt.HWND]
        self.user32.IsWindowVisible.restype = wt.BOOL
        self.user32.GetClassNameW.argtypes = [wt.HWND, wt.LPWSTR, ctypes.c_int]
        self.user32.GetClassNameW.restype = ctypes.c_int
        self.user32.PostMessageW.argtypes = [wt.HWND, wt.UINT, wt.WPARAM, wt.LPARAM]
        self.user32.PostMessageW.restype = wt.BOOL
        self.user32.SendInput.argtypes = [wt.UINT, ctypes.POINTER(INPUT), ctypes.c_int]
        self.user32.SendInput.restype = wt.UINT

    def windows_version(self) -> tuple[int, int, int]:
        class RTL_OSVERSIONINFOEXW(ctypes.Structure):
            _fields_ = [
                ("dwOSVersionInfoSize", wt.DWORD),
                ("dwMajorVersion", wt.DWORD),
                ("dwMinorVersion", wt.DWORD),
                ("dwBuildNumber", wt.DWORD),
                ("dwPlatformId", wt.DWORD),
                ("szCSDVersion", wt.WCHAR * 128),
                ("wServicePackMajor", wt.WORD),
                ("wServicePackMinor", wt.WORD),
                ("wSuiteMask", wt.WORD),
                ("wProductType", ctypes.c_ubyte),
                ("wReserved", ctypes.c_ubyte),
            ]

        version = RTL_OSVERSIONINFOEXW()
        version.dwOSVersionInfoSize = ctypes.sizeof(version)
        if self.ntdll.RtlGetVersion(ctypes.byref(version)) != 0:
            raise HarnessFailure("WINDOWS_VERSION_QUERY_FAILED")
        return version.dwMajorVersion, version.dwMinorVersion, version.dwBuildNumber

    def native_machine(self) -> int:
        process_machine = wt.WORD()
        native_machine = wt.WORD()
        if not self.kernel32.IsWow64Process2(
            self.kernel32.GetCurrentProcess(),
            ctypes.byref(process_machine),
            ctypes.byref(native_machine),
        ):
            raise HarnessFailure("NATIVE_ARCHITECTURE_QUERY_FAILED")
        return int(native_machine.value)

    def process_session_id(self, pid: int) -> int:
        session = wt.DWORD()
        if not self.kernel32.ProcessIdToSessionId(pid, ctypes.byref(session)):
            raise HarnessBlocked("PROCESS_SESSION_QUERY_FAILED")
        return int(session.value)

    def process_integrity_rid(self, pid: int) -> int:
        process = self.kernel32.OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, False, pid)
        if not process:
            raise HarnessBlocked("PROCESS_TOKEN_OPEN_FAILED")
        token = wt.HANDLE()
        try:
            if not self.advapi32.OpenProcessToken(process, TOKEN_QUERY, ctypes.byref(token)):
                raise HarnessBlocked("PROCESS_TOKEN_OPEN_FAILED")
            needed = wt.DWORD()
            self.advapi32.GetTokenInformation(
                token, TOKEN_INTEGRITY_LEVEL, None, 0, ctypes.byref(needed)
            )
            if needed.value == 0:
                raise HarnessBlocked("PROCESS_INTEGRITY_QUERY_FAILED")
            buffer = ctypes.create_string_buffer(needed.value)
            if not self.advapi32.GetTokenInformation(
                token,
                TOKEN_INTEGRITY_LEVEL,
                buffer,
                needed,
                ctypes.byref(needed),
            ):
                raise HarnessBlocked("PROCESS_INTEGRITY_QUERY_FAILED")
            label = ctypes.cast(buffer, ctypes.POINTER(TOKEN_MANDATORY_LABEL_STRUCT)).contents
            count = self.advapi32.GetSidSubAuthorityCount(label.Label.Sid)
            if not count or count.contents.value == 0:
                raise HarnessBlocked("PROCESS_INTEGRITY_QUERY_FAILED")
            rid = self.advapi32.GetSidSubAuthority(
                label.Label.Sid, count.contents.value - 1
            )
            if not rid:
                raise HarnessBlocked("PROCESS_INTEGRITY_QUERY_FAILED")
            return int(rid.contents.value)
        finally:
            if token:
                self.kernel32.CloseHandle(token)
            self.kernel32.CloseHandle(process)

    def security_context(self, pid: int) -> SecurityContext:
        rid = self.process_integrity_rid(pid)
        return SecurityContext(self.process_session_id(pid), rid, integrity_name(rid))

    def wts_state(self, session_id: int) -> int:
        buffer = wt.LPWSTR()
        size = wt.DWORD()
        if not self.wtsapi32.WTSQuerySessionInformationW(
            None,
            session_id,
            WTS_CONNECT_STATE,
            ctypes.byref(buffer),
            ctypes.byref(size),
        ):
            raise HarnessBlocked("WTS_STATE_QUERY_FAILED")
        try:
            if size.value < ctypes.sizeof(wt.DWORD):
                raise HarnessBlocked("WTS_STATE_QUERY_FAILED")
            return ctypes.cast(buffer, ctypes.POINTER(wt.DWORD)).contents.value
        finally:
            self.wtsapi32.WTSFreeMemory(buffer)

    def desktop_name(self, desktop: int) -> str:
        needed = wt.DWORD()
        self.user32.GetUserObjectInformationW(desktop, UOI_NAME, None, 0, ctypes.byref(needed))
        if needed.value == 0:
            raise HarnessBlocked("INPUT_DESKTOP_NAME_FAILED")
        buffer = ctypes.create_unicode_buffer(needed.value // ctypes.sizeof(wt.WCHAR))
        if not self.user32.GetUserObjectInformationW(
            desktop, UOI_NAME, buffer, needed, ctypes.byref(needed)
        ):
            raise HarnessBlocked("INPUT_DESKTOP_NAME_FAILED")
        return buffer.value

    def input_desktop(self) -> tuple[str, str]:
        desktop = self.user32.OpenInputDesktop(
            0, False, DESKTOP_READOBJECTS | DESKTOP_SWITCHDESKTOP
        )
        if not desktop:
            raise HarnessBlocked("INPUT_DESKTOP_UNAVAILABLE")
        try:
            input_name = self.desktop_name(desktop)
            if not self.user32.SwitchDesktop(desktop):
                raise HarnessBlocked("INPUT_DESKTOP_LOCKED")
        finally:
            self.user32.CloseDesktop(desktop)
        thread_desktop = self.user32.GetThreadDesktop(self.kernel32.GetCurrentThreadId())
        if not thread_desktop:
            raise HarnessBlocked("THREAD_DESKTOP_UNAVAILABLE")
        thread_name = self.desktop_name(thread_desktop)
        if input_name.casefold() != "default" or thread_name.casefold() != input_name.casefold():
            raise HarnessBlocked("INPUT_DESKTOP_MISMATCH")
        return input_name, thread_name

    def require_foreground(self, hwnd: int, timeout: float = 2.0) -> None:
        self.user32.ShowWindow(hwnd, SW_RESTORE)
        self.user32.BringWindowToTop(hwnd)
        self.user32.SetForegroundWindow(hwnd)
        deadline = time.perf_counter() + timeout
        while time.perf_counter() < deadline:
            if int(self.user32.GetForegroundWindow() or 0) == hwnd:
                return
            time.sleep(0.025)
        raise HarnessBlocked("FOREGROUND_PERMISSION_DENIED")

    def owned_task_dialogs(self, process_id: int, owner_hwnd: int) -> list[int]:
        dialogs: list[int] = []
        failure_code: str | None = None

        @self._enum_windows_proc
        def collect(hwnd: int, _lparam: int) -> bool:
            nonlocal failure_code
            if int(self.user32.GetWindow(hwnd, GW_OWNER) or 0) != owner_hwnd:
                return True
            process = wt.DWORD()
            if not self.user32.GetWindowThreadProcessId(hwnd, ctypes.byref(process)):
                failure_code = "TASK_DIALOG_PROCESS_QUERY_FAILED"
                return False
            if process.value != process_id or not self.user32.IsWindowVisible(hwnd):
                return True
            class_name = ctypes.create_unicode_buffer(256)
            if not self.user32.GetClassNameW(hwnd, class_name, len(class_name)):
                failure_code = "TASK_DIALOG_CLASS_QUERY_FAILED"
                return False
            if class_name.value == TASK_DIALOG_CLASS:
                dialogs.append(int(hwnd))
            return True

        if not self.user32.EnumWindows(collect, 0) and failure_code is None:
            raise HarnessFailure("TASK_DIALOG_ENUM_FAILED")
        if failure_code is not None:
            raise HarnessFailure(failure_code)
        return dialogs

    def post_close(self, hwnd: int) -> None:
        if not self.user32.PostMessageW(hwnd, WM_CLOSE, 0, 0):
            raise HarnessFailure("WM_CLOSE_POST_FAILED")

    def send_inputs(self, inputs: list[INPUT]) -> None:
        if not inputs:
            return
        array = (INPUT * len(inputs))(*inputs)
        sent = self.user32.SendInput(len(array), array, ctypes.sizeof(INPUT))
        if sent != len(array):
            raise HarnessFailure("SENDINPUT_INCOMPLETE")

    def send_shortcut(self, hwnd: int, key: int) -> None:
        self.require_foreground(hwnd)
        self.send_inputs(
            [
                key_input(VK_CONTROL, False),
                key_input(key, False),
                key_input(key, True),
                key_input(VK_CONTROL, True),
            ]
        )

    def send_key(self, hwnd: int, key: int) -> None:
        self.require_foreground(hwnd)
        self.send_inputs([key_input(key, False), key_input(key, True)])

    def send_unicode(self, hwnd: int, text: str) -> None:
        self.require_foreground(hwnd)
        units = struct.unpack(f"<{len(text.encode('utf-16-le')) // 2}H", text.encode("utf-16-le"))
        inputs: list[INPUT] = []
        for unit in units:
            inputs.extend((unicode_input(unit, False), unicode_input(unit, True)))
        self.send_inputs(inputs)

    def terminate_process(self, pid: int) -> None:
        process = self.kernel32.OpenProcess(PROCESS_TERMINATE, False, pid)
        if not process:
            raise HarnessFailure("TERMINATE_PROCESS_OPEN_FAILED")
        try:
            if not self.kernel32.TerminateProcess(process, 0xDEAD):
                raise HarnessFailure("TERMINATE_PROCESS_FAILED")
        finally:
            self.kernel32.CloseHandle(process)


def key_input(key: int, key_up: bool) -> INPUT:
    flags = KEYEVENTF_KEYUP if key_up else 0
    return INPUT(type=INPUT_KEYBOARD, ki=KEYBDINPUT(key, 0, flags, 0, 0))


def unicode_input(unit: int, key_up: bool) -> INPUT:
    flags = KEYEVENTF_UNICODE | (KEYEVENTF_KEYUP if key_up else 0)
    return INPUT(type=INPUT_KEYBOARD, ki=KEYBDINPUT(0, unit, flags, 0, 0))


def integrity_name(rid: int) -> str:
    if rid < 0x1000:
        return "untrusted"
    if rid < 0x2000:
        return "low"
    if rid < 0x2100:
        return "medium"
    if rid < 0x3000:
        return "medium-plus"
    if rid < 0x4000:
        return "high"
    if rid < 0x5000:
        return "system"
    return "protected"


def write_durable(path: Path, value: bytes) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    with path.open("wb") as handle:
        handle.write(value)
        handle.flush()
        os.fsync(handle.fileno())


def wait_until(
    predicate: Callable[[], Any], timeout: float, timeout_code: str, interval: float = 0.05
) -> Any:
    deadline = time.perf_counter() + timeout
    while time.perf_counter() < deadline:
        try:
            result = predicate()
        except (HarnessBlocked, HarnessFailure):
            raise
        except Exception:
            result = None
        if result:
            return result
        time.sleep(interval)
    raise HarnessFailure(timeout_code)


def checkpoint_success_present(value: bytes, offset: int = 0) -> bool:
    return RECOVERY_CHECKPOINT_LOG_RE.search(value[max(0, offset) :]) is not None


def recovery_startup_finished_present(value: bytes, offset: int = 0) -> bool:
    return RECOVERY_STARTUP_FINISHED_LOG_RE.search(value[max(0, offset) :]) is not None


def scan_live_recovery_records(data_root: Path) -> dict[str, Any]:
    recovery_root = data_root / "recovery"
    records = sorted(
        path
        for path in recovery_root.glob("*.mtrecovery")
        if path.is_file() and CANONICAL_RECOVERY_RECORD_RE.fullmatch(path.name)
    )
    if not records:
        raise HarnessFailure("CANONICAL_RECOVERY_RECORD_MISSING")
    utf8 = DOCUMENT_SENTINEL.encode("utf-8")
    utf16 = DOCUMENT_SENTINEL.encode("utf-16-le")
    fingerprints = []
    for path in records:
        try:
            value = path.read_bytes()
        except OSError as error:
            raise HarnessFailure(
                "LIVE_RECOVERY_RECORD_SCAN_FAILED", safe_exception_name(error)
            ) from None
        if not value:
            raise HarnessFailure("CANONICAL_RECOVERY_RECORD_EMPTY")
        if utf8 in value:
            raise HarnessFailure("UTF8_DOCUMENT_SENTINEL_LEAKED")
        if utf16 in value:
            raise HarnessFailure("UTF16LE_DOCUMENT_SENTINEL_LEAKED")
        fingerprints.append(fingerprint_bytes(value).evidence())
    return {
        "canonical_record_count": len(records),
        "canonical_records": fingerprints,
        "utf8_sentinel_absent": True,
        "utf16le_sentinel_absent": True,
    }


def runtime_artifact_paths(data_root: Path, stderr_path: Path) -> list[Path]:
    paths = [stderr_path]
    for root in (data_root / "logs", data_root / "recovery"):
        if root.exists():
            paths.extend(path for path in root.rglob("*") if path.is_file())
    return sorted(set(path.resolve() for path in paths))


def scan_runtime_artifacts(data_root: Path, stderr_path: Path) -> dict[str, Any]:
    utf8 = DOCUMENT_SENTINEL.encode("utf-8")
    utf16 = DOCUMENT_SENTINEL.encode("utf-16-le")
    paths = runtime_artifact_paths(data_root, stderr_path)
    app_logs = [path for path in paths if path.parent == (data_root / "logs").resolve()]
    recovery_root = (data_root / "recovery").resolve()
    recovery_artifacts = [path for path in paths if recovery_root in path.parents]
    canonical_records = [
        path
        for path in recovery_artifacts
        if CANONICAL_RECOVERY_RECORD_RE.fullmatch(path.name)
    ]
    recovery_leases = [
        path for path in recovery_artifacts if path.name == ".markturbo-recovery.lock"
    ]
    if not app_logs:
        raise HarnessFailure("APP_LOG_MISSING")
    for path in paths:
        try:
            value = path.read_bytes()
        except OSError as error:
            raise HarnessFailure(
                "RUNTIME_ARTIFACT_SCAN_FAILED", safe_exception_name(error)
            ) from None
        lowered = value.lower()
        if utf8 in value:
            raise HarnessFailure("UTF8_DOCUMENT_SENTINEL_LEAKED")
        if utf16 in value:
            raise HarnessFailure("UTF16LE_DOCUMENT_SENTINEL_LEAKED")
        if b"refcell already borrowed" in lowered:
            raise HarnessFailure("REFCELL_BORROW_PANIC_LOGGED")
        if re.search(rb"\bpanic(?:ked)?\b", lowered):
            raise HarnessFailure("PANIC_LOGGED")
    return {
        "files_scanned": len(paths),
        "app_logs_scanned": len(app_logs),
        "recovery_artifacts_scanned": len(recovery_artifacts),
        "canonical_recovery_records_scanned": len(canonical_records),
        "recovery_leases_scanned": len(recovery_leases),
        "utf8_sentinel_absent": True,
        "utf16le_sentinel_absent": True,
        "panic_absent": True,
        "refcell_absent": True,
    }


class NativeHarness:
    def __init__(
        self,
        copied_exe: Path,
        root: Path,
        ui_timeout: float,
        win32: Win32,
        application_class: Any,
        uia_element_info_class: Any,
        uia_wrapper_class: Any,
        iuia_class: Any,
        no_pattern_error_class: Any,
        parent_context: SecurityContext,
    ) -> None:
        self.copied_exe = copied_exe
        self.root = root
        self.ui_timeout = ui_timeout
        self.win32 = win32
        self.application_class = application_class
        self.uia_element_info_class = uia_element_info_class
        self.uia_wrapper_class = uia_wrapper_class
        self.iuia_class = iuia_class
        self.no_pattern_error_class = no_pattern_error_class
        self.parent_context = parent_context
        self.processes: list[subprocess.Popen[bytes]] = []

    def case_roots(self, case_id: str) -> tuple[Path, Path, Path, Path]:
        case_root = (self.root / "cases" / case_id).resolve()
        data_root = (case_root / "data").resolve()
        config_root = (case_root / "config").resolve()
        workspace_root = (case_root / "workspace").resolve()
        stderr_path = (case_root / "stderr.log").resolve()
        for directory in (data_root, config_root, workspace_root):
            directory.mkdir(parents=True, exist_ok=True)
        return data_root, config_root, workspace_root, stderr_path

    def launch(
        self,
        target: Path,
        data_root: Path,
        config_root: Path,
        workspace_root: Path,
        stderr_path: Path,
    ) -> RunningApp:
        spec = build_launch_spec(
            self.copied_exe, target, data_root, config_root, workspace_root, stderr_path
        )
        try:
            with stderr_path.open("ab") as stderr:
                process = subprocess.Popen(
                    spec.args,
                    cwd=spec.cwd,
                    env=spec.env,
                    stdin=subprocess.DEVNULL,
                    stdout=subprocess.DEVNULL,
                    stderr=stderr,
                )
        except OSError as error:
            raise HarnessFailure("PROCESS_LAUNCH_FAILED", safe_exception_name(error)) from None
        self.processes.append(process)
        if process.poll() is not None:
            raise HarnessFailure("PROCESS_EXITED_BEFORE_UI")
        child_context = self.win32.security_context(process.pid)
        if failure := security_context_failure(self.parent_context, child_context):
            raise HarnessBlocked(failure)
        try:
            app = self.application_class(backend="uia").connect(
                process=process.pid, timeout=self.ui_timeout
            )
            window = app.top_window()
            window.wait("exists visible enabled ready", timeout=self.ui_timeout)
            hwnd = int(window.handle)
        except Exception as error:
            if process.poll() is not None:
                raise HarnessFailure("PROCESS_EXITED_BEFORE_UI") from None
            raise HarnessFailure("MAIN_WINDOW_UIA_TIMEOUT", safe_exception_name(error)) from None
        if not hwnd:
            raise HarnessBlocked("UIA_MAIN_WINDOW_HANDLE_MISSING")
        self.win32.require_foreground(hwnd)
        running = RunningApp(
            process,
            window,
            hwnd,
            spec,
            child_context,
            data_root / "logs" / f"markturbo-{process.pid}.log",
        )
        self.activate_source_layout(running)
        return running

    def fresh_uia_root(self, hwnd: int) -> Any:
        return self.uia_element_info_class(hwnd)

    def control_by_id(
        self,
        hwnd: int,
        automation_id: str,
        control_type: str,
        mismatch_code: str,
        expected_name: str | None = None,
        expected_class_name: str | None = None,
    ) -> Any | None:
        uia = self.iuia_class()
        condition = uia.iuia.CreatePropertyCondition(
            uia.UIA_dll.UIA_AutomationIdPropertyId, automation_id
        )
        elements = self.fresh_uia_root(hwnd).element.FindAll(
            uia.tree_scope["descendants"], condition
        )
        if elements.Length == 0:
            return None
        if elements.Length != 1:
            raise HarnessFailure(mismatch_code)
        element = elements.GetElement(0)
        expected_control_type = uia.known_control_types[control_type]
        if (
            element.CurrentAutomationId != automation_id
            or element.CurrentControlType != expected_control_type
            or (expected_name is not None and element.CurrentName != expected_name)
            or (
                expected_class_name is not None
                and element.CurrentClassName != expected_class_name
            )
        ):
            raise HarnessFailure(mismatch_code)
        control = self.uia_wrapper_class(self.uia_element_info_class(element))
        info = control.element_info
        if info.automation_id is None or info.control_type is None:
            raise RuntimeError("UIA_CONTROL_PROPERTIES_UNAVAILABLE")
        if info.automation_id != automation_id or info.control_type != control_type:
            raise HarnessFailure(mismatch_code)
        return control

    def find_control(
        self,
        app: RunningApp,
        automation_id: str,
        control_type: str,
        timeout_code: str,
        mismatch_code: str,
        timeout: float | None = None,
    ) -> Any:
        last_failure_type: str | None = None

        def locate() -> Any | None:
            nonlocal last_failure_type
            try:
                control = self.control_by_id(
                    app.hwnd, automation_id, control_type, mismatch_code
                )
                if control is None:
                    return None
                if not control.is_visible() or not control.is_enabled():
                    return None
            except HarnessFailure:
                raise
            except Exception as error:
                last_failure_type = safe_exception_name(error)
                return None
            return control

        try:
            return wait_until(
                locate,
                self.ui_timeout if timeout is None else timeout,
                timeout_code,
                interval=0.025,
            )
        except HarnessFailure as error:
            if error.code == timeout_code and last_failure_type is not None:
                raise HarnessFailure(timeout_code, last_failure_type) from None
            raise

    def click_control(self, control: Any, failure_code: str) -> None:
        try:
            control.click_input()
        except Exception as error:
            raise HarnessFailure(failure_code, safe_exception_name(error)) from None

    def activate_source_layout(self, app: RunningApp) -> None:
        source_layout = self.find_control(
            app,
            LAYOUT_SOURCE_AUTOMATION_ID,
            "TabItem",
            "SOURCE_LAYOUT_UIA_TIMEOUT",
            "SOURCE_LAYOUT_UIA_CONTRACT_MISMATCH",
        )
        self.click_control(source_layout, "SOURCE_LAYOUT_CLICK_FAILED")
        self.find_control(
            app,
            SOURCE_EDITOR_AUTOMATION_ID,
            "Edit",
            "SOURCE_EDITOR_UIA_TIMEOUT",
            "SOURCE_EDITOR_UIA_CONTRACT_MISMATCH",
        )

    def control_absent(
        self,
        app: RunningApp,
        automation_id: str,
        control_type: str,
        query_failure_code: str,
        mismatch_code: str,
    ) -> bool:
        try:
            control = self.control_by_id(
                app.hwnd, automation_id, control_type, mismatch_code
            )
            return control is None
        except Exception as error:
            if isinstance(error, HarnessFailure):
                raise
            raise HarnessFailure(query_failure_code, safe_exception_name(error)) from None

    def editor_absent_while_running(self, app: RunningApp) -> bool:
        if app.process.poll() is not None:
            raise HarnessFailure("PROCESS_EXITED_DURING_TAB_CLOSE")
        return self.control_absent(
            app,
            SOURCE_EDITOR_AUTOMATION_ID,
            "Edit",
            "DISCARD_EDITOR_ABSENCE_UIA_QUERY_FAILED",
            "DISCARD_EDITOR_ABSENCE_UIA_CONTRACT_MISMATCH",
        )

    def focus_editor(self, app: RunningApp) -> None:
        editor = self.find_control(
            app,
            SOURCE_EDITOR_AUTOMATION_ID,
            "Edit",
            "SOURCE_EDITOR_UIA_TIMEOUT",
            "SOURCE_EDITOR_UIA_CONTRACT_MISMATCH",
        )
        self.click_control(editor, "EDITOR_POINTER_FOCUS_FAILED")

    def read_editor_fingerprint(self, app: RunningApp) -> Fingerprint | None:
        editor = self.control_by_id(
            app.hwnd,
            SOURCE_EDITOR_AUTOMATION_ID,
            "Edit",
            "SOURCE_EDITOR_UIA_CONTRACT_MISMATCH",
        )
        if editor is None:
            return None
        try:
            pattern = editor.iface_value
        except self.no_pattern_error_class:
            raise HarnessFailure("EDITOR_UIA_VALUE_CONTRACT_MISMATCH") from None
        if pattern is None:
            raise HarnessFailure("EDITOR_UIA_VALUE_CONTRACT_MISMATCH")
        value = pattern.CurrentValue
        if not isinstance(value, str):
            raise HarnessFailure("EDITOR_UIA_VALUE_CONTRACT_MISMATCH")
        return fingerprint_text(value)

    def editor_fingerprint(self, app: RunningApp) -> Fingerprint:
        self.focus_editor(app)
        fingerprint, _ = self.wait_editor_fingerprint(
            app, None, self.ui_timeout, already_focused=True
        )
        return fingerprint

    def replace_editor(self, app: RunningApp, value: str) -> Fingerprint:
        self.focus_editor(app)
        self.win32.send_shortcut(app.hwnd, VK_A)
        if value:
            self.win32.send_unicode(app.hwnd, value)
        else:
            self.win32.send_key(app.hwnd, VK_BACK)
        expected = fingerprint_text(value)
        actual, _ = self.wait_editor_fingerprint(
            app, expected, self.ui_timeout, already_focused=True
        )
        return actual

    def lifecycle_dialog(self, app: RunningApp, timeout: float | None = None) -> dict[str, Any]:
        last_failure_type: str | None = None

        def locate() -> dict[str, Any] | None:
            nonlocal last_failure_type
            try:
                dialogs = self.win32.owned_task_dialogs(app.process.pid, app.hwnd)
                if not dialogs:
                    return None
                if len(dialogs) != 1:
                    raise HarnessFailure("MULTIPLE_LIFECYCLE_TASK_DIALOGS")
                dialog_hwnd = dialogs[0]
                buttons = {}
                for name, (automation_id, expected_name) in LIFECYCLE_BUTTON_CONTRACTS.items():
                    button = self.control_by_id(
                        dialog_hwnd,
                        automation_id,
                        "Button",
                        "LIFECYCLE_BUTTON_CONTRACT_MISMATCH",
                        expected_name,
                        TASK_DIALOG_BUTTON_CLASS,
                    )
                    if button is None:
                        return None
                    buttons[name] = button
                return buttons
            except HarnessFailure:
                raise
            except Exception as error:
                last_failure_type = safe_exception_name(error)
                return None

        try:
            return wait_until(
                locate,
                self.ui_timeout if timeout is None else timeout,
                "LIFECYCLE_TASK_DIALOG_TIMEOUT",
            )
        except HarnessFailure as error:
            if error.code == "LIFECYCLE_TASK_DIALOG_TIMEOUT" and last_failure_type is not None:
                raise HarnessFailure(error.code, last_failure_type) from None
            raise

    def click_lifecycle_decision(self, app: RunningApp, name: str) -> None:
        button = self.lifecycle_dialog(app)[name]
        self.click_control(button, LIFECYCLE_CLICK_FAILURE_CODES[name])

    def wait_process_exit(self, app: RunningApp, timeout: float | None = None) -> None:
        try:
            returncode = app.process.wait(timeout=timeout or self.ui_timeout)
        except subprocess.TimeoutExpired:
            raise HarnessFailure("PROCESS_EXIT_TIMEOUT") from None
        if returncode != 0:
            raise HarnessFailure("PROCESS_EXIT_NONZERO")

    def reap(self, app: RunningApp) -> None:
        if app.process.poll() is None:
            try:
                self.win32.post_close(app.hwnd)
                app.process.wait(timeout=2.0)
            except (HarnessBlocked, HarnessFailure, subprocess.TimeoutExpired):
                pass
        if app.process.poll() is None:
            try:
                app.process.kill()
                app.process.wait(timeout=5.0)
            except (OSError, subprocess.TimeoutExpired):
                raise HarnessFailure("CLEANUP_REAP_FAILED") from None
        if app.process.poll() is None:
            raise HarnessFailure("CLEANUP_REAP_FAILED")

    def terminate(self, app: RunningApp) -> None:
        self.win32.terminate_process(app.process.pid)
        try:
            app.process.wait(timeout=5.0)
        except subprocess.TimeoutExpired:
            raise HarnessFailure("TERMINATE_PROCESS_WAIT_FAILED") from None

    def cleanup(self) -> None:
        for process in self.processes:
            if process.poll() is not None:
                continue
            try:
                process.kill()
                process.wait(timeout=5.0)
            except (OSError, subprocess.TimeoutExpired):
                raise HarnessFailure("CLEANUP_REAP_FAILED") from None

    def wait_file(self, path: Path, expected: Fingerprint, code: str) -> Fingerprint:
        def matches() -> Fingerprint | None:
            try:
                actual = sha256_file(path)
            except OSError:
                return None
            return actual if actual == expected else None

        return wait_until(matches, self.ui_timeout, code)

    def wait_editor_fingerprint(
        self,
        app: RunningApp,
        expected: Fingerprint | None,
        timeout: float,
        *,
        already_focused: bool = False,
    ) -> tuple[Fingerprint, float]:
        started = time.perf_counter()
        if not already_focused:
            self.focus_editor(app)
        saw_readable_value = False
        last_failure_type: str | None = None

        def matches() -> Fingerprint | None:
            nonlocal saw_readable_value, last_failure_type
            try:
                actual = self.read_editor_fingerprint(app)
            except HarnessFailure:
                raise
            except Exception as error:
                last_failure_type = safe_exception_name(error)
                return None
            if actual is None:
                return None
            saw_readable_value = True
            return actual if expected is None or actual == expected else None

        timeout_code = "EDITOR_UIA_VALUE_TIMEOUT" if expected is None else "EDITOR_EXACT_BYTES_TIMEOUT"
        try:
            actual = wait_until(matches, timeout, timeout_code, interval=0.1)
        except HarnessFailure as error:
            if error.code != timeout_code:
                raise
            if expected is not None and saw_readable_value:
                raise
            raise HarnessFailure("EDITOR_UIA_VALUE_TIMEOUT", last_failure_type or "") from None
        return actual, (time.perf_counter() - started) * 1000

    def wait_checkpoint_log(self, app: RunningApp, offset: int, started: float) -> float:
        remaining = 10.0 - (time.perf_counter() - started)
        if remaining <= 0:
            raise HarnessFailure("RECOVERY_SUCCESS_LOG_EXCEEDED_10_SECONDS")

        def present() -> bool:
            try:
                return checkpoint_success_present(app.app_log_path.read_bytes(), offset)
            except OSError:
                return False

        wait_until(
            present,
            remaining,
            "RECOVERY_SUCCESS_LOG_EXCEEDED_10_SECONDS",
            interval=0.025,
        )
        elapsed_ms = (time.perf_counter() - started) * 1000
        if elapsed_ms > 10_000:
            raise HarnessFailure("RECOVERY_SUCCESS_LOG_EXCEEDED_10_SECONDS")
        return elapsed_ms

    def wait_recovery_startup_finished(self, app: RunningApp, timeout: float) -> None:
        def present() -> bool:
            try:
                return recovery_startup_finished_present(app.app_log_path.read_bytes())
            except OSError:
                return False

        wait_until(
            present,
            timeout,
            "RECOVERY_STARTUP_FINISHED_TIMEOUT",
            interval=0.025,
        )

    def scenario_pointer_cancel(self) -> dict[str, Any]:
        data, config, workspace, stderr = self.case_roots(CASE_POINTER_CANCEL)
        document = (workspace / "pointer.md").resolve()
        write_durable(document, ORIGINAL_BYTES)
        source_before = sha256_file(document)
        app = self.launch(document, data, config, workspace, stderr)
        try:
            editor = self.replace_editor(app, POINTER_EDIT)
            close = self.find_control(
                app,
                TAB_CLOSE_AUTOMATION_ID,
                "Button",
                "TAB_CLOSE_UIA_TIMEOUT",
                "TAB_CLOSE_UIA_CONTRACT_MISMATCH",
            )
            self.click_control(close, "TAB_CLOSE_POINTER_CLICK_FAILED")
            self.click_lifecycle_decision(app, "Cancel")
            if app.process.poll() is not None:
                raise HarnessFailure("CANCEL_CLOSED_PROCESS")
            if self.editor_fingerprint(app) != editor:
                raise HarnessFailure("CANCEL_CHANGED_EDITOR_BYTES")
            if sha256_file(document) != source_before:
                raise HarnessFailure("CANCEL_CHANGED_SOURCE")
            self.win32.send_shortcut(app.hwnd, VK_S)
            saved = self.wait_file(document, editor, "CANCEL_FOLLOWUP_SAVE_TIMEOUT")
            observations = {
                "editor": editor.evidence(),
                "source_before": source_before.evidence(),
                "source_after_cancel_save": saved.evidence(),
                "cancel_kept_tab": True,
                "flow": CASE_FLOWS[CASE_POINTER_CANCEL],
                "process_context": app.security_context.evidence(),
                "foreground_verified": True,
            }
        finally:
            self.reap(app)
        observations["runtime_scan"] = scan_runtime_artifacts(data, stderr)
        return observations

    def scenario_keyboard_discard(self) -> dict[str, Any]:
        data, config, workspace, stderr = self.case_roots(CASE_KEYBOARD_DISCARD)
        document = (workspace / "keyboard.md").resolve()
        write_durable(document, ORIGINAL_BYTES)
        source_before = sha256_file(document)
        app = self.launch(document, data, config, workspace, stderr)
        try:
            discarded = self.replace_editor(app, KEYBOARD_EDIT)
            self.win32.send_shortcut(app.hwnd, VK_W)
            self.click_lifecycle_decision(app, "Discard")
            wait_until(
                lambda: self.editor_absent_while_running(app),
                self.ui_timeout,
                "DISCARD_TAB_CLOSE_TIMEOUT",
            )
            source_after = sha256_file(document)
            if source_after != source_before:
                raise HarnessFailure("DISCARD_CHANGED_SOURCE")
            self.win32.post_close(app.hwnd)
            self.wait_process_exit(app)
            observations = {
                "discarded_editor": discarded.evidence(),
                "source_before": source_before.evidence(),
                "source_after_discard": source_after.evidence(),
                "discard_closed_tab": True,
                "flow": CASE_FLOWS[CASE_KEYBOARD_DISCARD],
                "process_context": app.security_context.evidence(),
                "foreground_verified": True,
            }
        finally:
            self.reap(app)
        observations["runtime_scan"] = scan_runtime_artifacts(data, stderr)
        return observations

    def scenario_window_save(self) -> dict[str, Any]:
        data, config, workspace, stderr = self.case_roots(CASE_WINDOW_SAVE)
        document = (workspace / "window.md").resolve()
        write_durable(document, ORIGINAL_BYTES)
        app = self.launch(document, data, config, workspace, stderr)
        try:
            editor = self.replace_editor(app, WINDOW_EDIT)
            self.win32.post_close(app.hwnd)
            self.click_lifecycle_decision(app, "Save")
            self.wait_process_exit(app, max(self.ui_timeout, 20.0))
            saved = sha256_file(document)
            if saved != editor:
                raise HarnessFailure("WINDOW_SAVE_BYTES_MISMATCH")
            observations = {
                "editor": editor.evidence(),
                "saved_source": saved.evidence(),
                "flow": CASE_FLOWS[CASE_WINDOW_SAVE],
                "window_exited": True,
                "process_context": app.security_context.evidence(),
                "foreground_verified": True,
            }
        finally:
            self.reap(app)
        observations["runtime_scan"] = scan_runtime_artifacts(data, stderr)
        return observations

    def scenario_external_conflict(self) -> dict[str, Any]:
        data, config, workspace, stderr = self.case_roots(CASE_EXTERNAL_CONFLICT)
        document = (workspace / "conflict.md").resolve()
        write_durable(document, ORIGINAL_BYTES)
        loaded = sha256_file(document)
        app = self.launch(document, data, config, workspace, stderr)
        try:
            dirty = self.replace_editor(app, CONFLICT_EDIT)
            before = document.stat()
            if len(ORIGINAL_BYTES) != len(EXTERNAL_BYTES):
                raise HarnessFailure("FIXTURE_LENGTH_MISMATCH")
            write_durable(document, EXTERNAL_BYTES)
            os.utime(document, ns=(before.st_atime_ns, before.st_mtime_ns))
            after = document.stat()
            if after.st_size != before.st_size or after.st_mtime_ns != before.st_mtime_ns:
                raise HarnessFailure("EXACT_MTIME_RESTORE_UNAVAILABLE")

            # The watcher must expose the conflict before any Save input.
            overwrite = self.find_control(
                app,
                CONFLICT_OVERWRITE_AUTOMATION_ID,
                "Button",
                "CONFLICT_OVERWRITE_UIA_TIMEOUT",
                "CONFLICT_OVERWRITE_UIA_CONTRACT_MISMATCH",
                self.ui_timeout,
            )
            external = sha256_file(document)
            if external != fingerprint_bytes(EXTERNAL_BYTES):
                raise HarnessFailure("WATCHER_CONFLICT_CHANGED_EXTERNAL_SOURCE")
            if self.editor_fingerprint(app) != dirty:
                raise HarnessFailure("WATCHER_CONFLICT_CHANGED_EDITOR")
            self.click_control(overwrite, "CONFLICT_OVERWRITE_CLICK_FAILED")
            saved = self.wait_file(document, dirty, "EXPLICIT_OVERWRITE_SAVE_TIMEOUT")
            self.win32.post_close(app.hwnd)
            self.wait_process_exit(app)
            observations = {
                "loaded_source": loaded.evidence(),
                "external_source": external.evidence(),
                "dirty_editor": dirty.evidence(),
                "saved_after_explicit_overwrite": saved.evidence(),
                "same_length": True,
                "same_mtime_ns": True,
                "watcher_conflict_before_save": True,
                "explicit_overwrite_used": True,
                "flow": CASE_FLOWS[CASE_EXTERNAL_CONFLICT],
                "process_context": app.security_context.evidence(),
                "foreground_verified": True,
            }
        finally:
            self.reap(app)
        observations["runtime_scan"] = scan_runtime_artifacts(data, stderr)
        return observations

    def scenario_recovery(self) -> dict[str, Any]:
        data, config, workspace, stderr = self.case_roots(CASE_RECOVERY)
        document = (workspace / "recovery.md").resolve()
        write_durable(document, ORIGINAL_BYTES)
        source = sha256_file(document)

        first = self.launch(document, data, config, workspace, stderr)
        try:
            wait_until(first.app_log_path.exists, self.ui_timeout, "APP_LOG_STARTUP_TIMEOUT")
            log_offset = first.app_log_path.stat().st_size
            input_started = time.perf_counter()
            edited = self.replace_editor(first, RECOVERY_EDIT)
            signal_ms = self.wait_checkpoint_log(first, log_offset, input_started)
            live_recovery_scan = scan_live_recovery_records(data)
            self.terminate(first)
        finally:
            self.reap(first)

        live_runtime_scan = scan_runtime_artifacts(data, stderr)

        second = self.launch(document, data, config, workspace, stderr)
        try:
            restored, restore_ms = self.wait_editor_fingerprint(
                second, edited, max(self.ui_timeout, 20.0)
            )
            self.win32.send_shortcut(second.hwnd, VK_W)
            self.click_lifecycle_decision(second, "Discard")
            wait_until(
                lambda: self.editor_absent_while_running(second),
                max(self.ui_timeout, 20.0),
                "RECOVERY_DISCARD_CLOSE_TIMEOUT",
            )
            if sha256_file(document) != source:
                raise HarnessFailure("RECOVERY_DISCARD_CHANGED_SOURCE")
            self.win32.post_close(second.hwnd)
            self.wait_process_exit(second, max(self.ui_timeout, 20.0))
        finally:
            self.reap(second)

        third = self.launch(document, data, config, workspace, stderr)
        try:
            restart_started = time.perf_counter()
            self.wait_recovery_startup_finished(third, max(self.ui_timeout, 20.0))
            restarted, _ = self.wait_editor_fingerprint(
                third, source, max(self.ui_timeout, 20.0)
            )
            absent_ms = (time.perf_counter() - restart_started) * 1000
            observations = {
                "edited_text": edited.evidence(),
                "checkpoint_log_seen": True,
                "checkpoint_signal": "content-free checkpoint success log",
                "live_runtime_scan": live_runtime_scan,
                "live_recovery_scan": live_recovery_scan,
                "edit_to_signal_ms": round(signal_ms, 3),
                "restored_editor": restored.evidence(),
                "recovery_restored_exact": restored == edited,
                "restore_ms": round(restore_ms, 3),
                "source_after_recovery_discard": sha256_file(document).evidence(),
                "restart_editor": restarted.evidence(),
                "discarded_recovery_absent": restarted == source,
                "discard_restart_absent_ms": round(absent_ms, 3),
                "termination": "TerminateProcess",
                "restart_count": 2,
                "startup_finished_log_seen": True,
                "startup_signal": "content-free recovery startup finished log",
                "startup_observed_before_restart_editor": True,
                "flow": CASE_FLOWS[CASE_RECOVERY],
                "process_contexts": [
                    first.security_context.evidence(),
                    second.security_context.evidence(),
                    third.security_context.evidence(),
                ],
                "foreground_verified": True,
            }
        finally:
            self.reap(third)
        observations["runtime_scan"] = scan_runtime_artifacts(data, stderr)
        return observations


def preflight(
    exe: Path, expected_hash: str, evidence: dict[str, Any]
) -> tuple[Win32, SecurityContext]:
    if not exe.is_file():
        raise HarnessFailure("EXECUTABLE_MISSING")
    actual = sha256_file(exe)
    evidence["executable"].update(actual.evidence())
    if failure := executable_hash_failure(actual.sha256, expected_hash):
        raise HarnessFailure(failure)
    try:
        pe = inspect_pe(exe)
    except (OSError, ValueError):
        raise HarnessFailure("EXECUTABLE_NOT_X64_PE") from None
    evidence["executable"].update(pe)
    evidence["executable"]["hash_verified"] = True

    if sys.platform != "win32":
        raise HarnessFailure("WINDOWS_REQUIRED")
    win32 = Win32()
    major, minor, build = win32.windows_version()
    native_machine = win32.native_machine()
    machine = "AMD64" if native_machine == IMAGE_FILE_MACHINE_AMD64 else f"0x{native_machine:04x}"
    if failure := platform_preflight_failure(
        platform.system(), major, build, machine, ctypes.sizeof(ctypes.c_void_p) * 8
    ):
        raise HarnessFailure(failure)
    parent = win32.security_context(os.getpid())
    if win32.wts_state(parent.session_id) != WTS_ACTIVE:
        raise HarnessBlocked("WTS_SESSION_NOT_ACTIVE")
    active_console = int(win32.kernel32.WTSGetActiveConsoleSessionId())
    input_name, thread_name = win32.input_desktop()
    evidence["environment"] = {
        "platform": "Windows 11",
        "windows_major": major,
        "windows_minor": minor,
        "windows_build": build,
        "architecture": "x86_64",
        "native_machine_code": native_machine,
        "python_pointer_bits": ctypes.sizeof(ctypes.c_void_p) * 8,
        "wts_state": "WTSActive",
        "active_console_session_id": None if active_console == 0xFFFFFFFF else active_console,
        "harness_is_console_session": active_console == parent.session_id,
        "input_desktop": input_name,
        "thread_desktop": thread_name,
        "harness_process": parent.evidence(),
    }
    return win32, parent


def mark_remaining_cases(
    evidence: dict[str, Any], start_index: int, status: str, reason_code: str
) -> None:
    for case in evidence["cases"][start_index:]:
        if case["status"] == "NOT_RUN":
            case["status"] = status
            case["reason_code"] = reason_code
            if status != "NOT_RUN":
                case["duration_ms"] = 0.0


def run(args: argparse.Namespace) -> tuple[int, dict[str, Any], str]:
    evidence = new_evidence(args.expect_exe_sha256)
    try:
        exe = args.exe.resolve(strict=False)
        win32, parent_context = preflight(exe, args.expect_exe_sha256, evidence)
        (
            application_class,
            uia_element_info_class,
            uia_wrapper_class,
            iuia_class,
            no_pattern_error_class,
        ) = load_pywinauto()
    except HarnessBlocked as error:
        mark_remaining_cases(evidence, 0, "BLOCKED", error.code)
        complete_evidence(evidence, "BLOCKED")
        return 2, evidence, error.code
    except HarnessFailure as error:
        mark_remaining_cases(evidence, 0, "NOT_RUN", error.code)
        complete_evidence(evidence, "FAIL")
        return 1, evidence, error.code
    except Exception as error:
        code = f"PREFLIGHT_{safe_exception_name(error).upper()}"
        mark_remaining_cases(evidence, 0, "NOT_RUN", code)
        complete_evidence(evidence, "FAIL")
        return 1, evidence, code

    try:
        temporary_context = tempfile.TemporaryDirectory(
            prefix="markturbo-goal-02-native-", ignore_cleanup_errors=True
        )
        temporary = temporary_context.__enter__()
        root = Path(temporary).resolve()
        bin_root = (root / "bin").resolve()
        bin_root.mkdir()
        copied_exe = Path(shutil.copy2(exe, bin_root / exe.name)).resolve()
        copied_hash = sha256_file(copied_exe)
    except Exception as error:
        code = f"ISOLATION_{safe_exception_name(error).upper()}"
        mark_remaining_cases(evidence, 0, "NOT_RUN", code)
        complete_evidence(evidence, "FAIL")
        return 1, evidence, code

    harness: NativeHarness | None = None
    current_index = 0
    returncode = 1
    code = "INTERNAL_NO_RESULT"
    try:
        try:
            evidence["executable"]["copied_sha256"] = copied_hash.sha256
            evidence["executable"]["copy_hash_verified"] = (
                copied_hash.sha256 == args.expect_exe_sha256
            )
            if not evidence["executable"]["copy_hash_verified"]:
                raise HarnessFailure("COPIED_EXECUTABLE_HASH_MISMATCH")

            harness = NativeHarness(
                copied_exe,
                root,
                args.ui_timeout,
                win32,
                application_class,
                uia_element_info_class,
                uia_wrapper_class,
                iuia_class,
                no_pattern_error_class,
                parent_context,
            )
            scenarios: tuple[Callable[[], dict[str, Any]], ...] = (
                harness.scenario_pointer_cancel,
                harness.scenario_keyboard_discard,
                harness.scenario_window_save,
                harness.scenario_external_conflict,
                harness.scenario_recovery,
            )
            for index, scenario in enumerate(scenarios):
                current_index = index
                case = evidence["cases"][index]
                started = time.perf_counter()
                try:
                    case["observations"] = scenario()
                except HarnessBlocked as error:
                    case["status"] = "BLOCKED"
                    case["reason_code"] = error.code
                    case["duration_ms"] = round((time.perf_counter() - started) * 1000, 3)
                    mark_remaining_cases(evidence, index + 1, "BLOCKED", "SKIPPED_AFTER_BLOCKED")
                    returncode, code = 2, error.code
                    break
                except HarnessFailure as error:
                    case["status"] = "FAIL"
                    case["reason_code"] = error.code
                    case["failure_type"] = safe_failure_type(error)
                    case["duration_ms"] = round((time.perf_counter() - started) * 1000, 3)
                    mark_remaining_cases(evidence, index + 1, "NOT_RUN", "SKIPPED_AFTER_FAILURE")
                    returncode, code = 1, error.code
                    break
                except Exception as error:
                    case["status"] = "FAIL"
                    case["reason_code"] = "INTERNAL_FAILURE"
                    case["failure_type"] = safe_exception_name(error)
                    case["duration_ms"] = round((time.perf_counter() - started) * 1000, 3)
                    mark_remaining_cases(evidence, index + 1, "NOT_RUN", "SKIPPED_AFTER_FAILURE")
                    returncode, code = 1, case["reason_code"]
                    break
                case["status"] = "PASS"
                case["duration_ms"] = round((time.perf_counter() - started) * 1000, 3)
            else:
                returncode, code = 0, "ALL_REQUIRED_CASES"
        except HarnessBlocked as error:
            mark_remaining_cases(evidence, current_index, "BLOCKED", error.code)
            returncode, code = 2, error.code
        except HarnessFailure as error:
            mark_remaining_cases(evidence, current_index, "NOT_RUN", error.code)
            returncode, code = 1, error.code
    finally:
        try:
            if harness is not None:
                harness.cleanup()
        except HarnessFailure as error:
            returncode, code = 1, error.code
            failed = evidence["cases"][min(current_index, len(REQUIRED_CASE_IDS) - 1)]
            failed["status"] = "FAIL"
            failed["reason_code"] = error.code
            failed["failure_type"] = safe_failure_type(error)
            failed["duration_ms"] = failed["duration_ms"] or 0.0
            for case in evidence["cases"][current_index + 1 :]:
                if case["status"] != "PASS":
                    case["status"] = "NOT_RUN"
                    case["reason_code"] = "SKIPPED_AFTER_CLEANUP_FAILURE"
        try:
            temporary_context.__exit__(None, None, None)
        except Exception as error:
            returncode, code = 1, "ISOLATION_CLEANUP_FAILED"
            failed = evidence["cases"][min(current_index, len(REQUIRED_CASE_IDS) - 1)]
            failed["status"] = "FAIL"
            failed["reason_code"] = code
            failed["failure_type"] = safe_exception_name(error)
            failed["duration_ms"] = failed["duration_ms"] or 0.0

    complete_evidence(evidence, {0: "PASS", 1: "FAIL", 2: "BLOCKED"}[returncode])
    return returncode, evidence, code


def parse_args(argv: list[str] | None = None) -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument(
        "--exe",
        type=Path,
        default=DEFAULT_EXE,
        help="release executable to copy and test (default: target/release/markturbo.exe)",
    )
    parser.add_argument(
        "--expect-exe-sha256",
        type=normalize_expected_hash,
        required=True,
        help="required SHA-256 binding for the release executable",
    )
    parser.add_argument(
        "--evidence",
        type=Path,
        default=DEFAULT_EVIDENCE,
        help="versioned JSON evidence path (default: .scratch)",
    )
    parser.add_argument(
        "--ui-timeout",
        type=float,
        default=15.0,
        help="seconds allowed for ordinary UI state transitions (default: 15)",
    )
    args = parser.parse_args(argv)
    if not math.isfinite(args.ui_timeout) or args.ui_timeout <= 0:
        parser.error("--ui-timeout must be greater than zero")
    return args


def main(argv: list[str] | None = None) -> int:
    args = parse_args(argv)
    returncode, evidence, code = run(args)
    try:
        write_evidence(args.evidence, evidence)
    except (OSError, ValueError):
        print("FAIL: EVIDENCE_WRITE_FAILED", file=sys.stderr)
        return 1
    status = {0: "PASS", 1: "FAIL", 2: "BLOCKED"}[returncode]
    stream = sys.stdout if returncode == 0 else sys.stderr
    print(f"{status}: {code}", file=stream)
    print(f"evidence: {args.evidence.resolve()}", file=stream)
    return returncode


if __name__ == "__main__":
    raise SystemExit(main())
