"""Exercise Goal 02 destructive paths through a real Windows UI.

The harness is intentionally fail-closed. It emits PASS only after every
required case runs against the expected x64 release executable in an active,
unlocked Windows 11 session. Document text is never written to stdout or JSON;
observations contain only UTF-8 byte counts, SHA-256 digests, and timings.
"""

from __future__ import annotations

import argparse
import math
import os
import re
import sys
import time
from pathlib import Path
from typing import Any, Callable

from .runtime import (
    IMAGE_FILE_MACHINE_AMD64,
    INTEGRITY_NAMES,
    PE32_PLUS_MAGIC,
    REASON_CODE_RE,
    SAFE_FAILURE_TYPES,
    SHA256_RE,
    VK_S,
    VK_W,
    HarnessFailure,
    NativeHarness as BaseNativeHarness,
    NativeRunPlan,
    complete_evidence as complete_evidence_envelope,
    finite_nonnegative,
    fingerprint_bytes,
    load_pywinauto,
    main_native_acceptance,
    normalize_expected_hash,
    preflight,
    require_true,
    safe_exception_name,
    sha256_file,
    utc_now,
    validate_environment,
    validate_fingerprint,
    validate_process_context,
    wait_until,
    write_durable,
    run_native_acceptance,
)

REPO = Path(__file__).resolve().parents[3]
DEFAULT_EXE = REPO / "target" / "release" / "markturbo.exe"
DEFAULT_EVIDENCE = REPO / ".scratch" / "goal-02-native-acceptance-v1.json"

SCHEMA = "markturbo.goal-02-native-acceptance"
SCHEMA_VERSION = 1
SAFE_OBSERVATION_STRINGS = {
    "pointer tab-close -> Cancel -> SendInput Ctrl+S",
    "SendInput Ctrl+W -> Discard",
    "WM_CLOSE -> Save",
    "watcher conflict -> explicit Overwrite",
    "edit -> checkpoint log -> TerminateProcess -> restart -> Discard -> restart after startup log",
    "TerminateProcess",
    "content-free checkpoint success log",
    "content-free recovery startup finished log",
} | INTEGRITY_NAMES
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
    "foreground_diagnostics",
    "foreground_hwnd",
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
    "requested_hwnd",
    "show_window_return",
    "bring_to_top_return",
    "set_foreground_return",
    "foreground_attempts",
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

TAB_CLOSE_AUTOMATION_ID = "markturbo-document-tab-close"
CONFLICT_OVERWRITE_AUTOMATION_ID = "markturbo-conflict-overwrite"

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
    complete_evidence_envelope(evidence, status, REQUIRED_CASE_IDS)


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
    byte_count = executable.get("byte_count")
    if byte_count is not None and (
        not isinstance(byte_count, int) or isinstance(byte_count, bool) or byte_count < 0
    ):
        raise ValueError("invalid executable byte count")
    if evidence["status"] == "PASS" and (not isinstance(byte_count, int) or byte_count <= 0):
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


def build_cli_command(exe: Path, expected_hash: str, evidence: Path) -> list[str]:
    return [
        sys.executable,
        "-m",
        "scripts.markturbo_tools.native.goal02",
        "--exe",
        str(exe),
        "--expect-exe-sha256",
        expected_hash,
        "--evidence",
        str(evidence),
    ]


def checkpoint_success_present(value: bytes, offset: int = 0) -> bool:
    return RECOVERY_CHECKPOINT_LOG_RE.search(value[max(0, offset) :]) is not None


def recovery_startup_finished_present(value: bytes, offset: int = 0) -> bool:
    return RECOVERY_STARTUP_FINISHED_LOG_RE.search(value[max(0, offset) :]) is not None


def wait_for_log_marker(
    path: Path,
    offset: int,
    predicate: Callable[[bytes], bool],
    timeout: float,
    code: str,
) -> None:
    cursor = max(0, offset)
    trailing_line = b""

    def present() -> bool:
        nonlocal cursor, trailing_line
        try:
            with path.open("rb") as handle:
                handle.seek(cursor)
                appended = handle.read()
        except OSError:
            return False
        if not appended:
            return False
        cursor += len(appended)
        observed = trailing_line + appended
        if predicate(observed):
            return True
        trailing_line = observed.rsplit(b"\n", 1)[-1][-512:]
        return False

    wait_until(present, timeout, code, interval=0.025)


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


class Goal02Harness(BaseNativeHarness):
    def wait_checkpoint_log(self, app: RunningApp, offset: int, started: float) -> float:
        remaining = 10.0 - (time.perf_counter() - started)
        if remaining <= 0:
            raise HarnessFailure("RECOVERY_SUCCESS_LOG_EXCEEDED_10_SECONDS")

        wait_for_log_marker(
            app.app_log_path,
            offset,
            checkpoint_success_present,
            remaining,
            "RECOVERY_SUCCESS_LOG_EXCEEDED_10_SECONDS",
        )
        elapsed_ms = (time.perf_counter() - started) * 1000
        if elapsed_ms > 10_000:
            raise HarnessFailure("RECOVERY_SUCCESS_LOG_EXCEEDED_10_SECONDS")
        return elapsed_ms

    def wait_recovery_startup_finished(self, app: RunningApp, timeout: float) -> None:
        wait_for_log_marker(
            app.app_log_path,
            0,
            recovery_startup_finished_present,
            timeout,
            "RECOVERY_STARTUP_FINISHED_TIMEOUT",
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

def native_run_plan() -> NativeRunPlan:
    return NativeRunPlan(
        required_case_ids=REQUIRED_CASE_IDS,
        workdir_prefix="markturbo-goal-02-native-",
        new_evidence=new_evidence,
        validate_evidence=validate_evidence,
        preflight=preflight,
        ui_types_loader=load_pywinauto,
        harness_factory=Goal02Harness,
        scenarios=lambda harness: (
            harness.scenario_pointer_cancel,
            harness.scenario_keyboard_discard,
            harness.scenario_window_save,
            harness.scenario_external_conflict,
            harness.scenario_recovery,
        ),
    )


def run(args: argparse.Namespace) -> tuple[int, dict[str, Any], str]:
    return run_native_acceptance(args, native_run_plan())


def parse_args(argv: list[str] | None = None) -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument(
        "--exe",
        type=Path,
        default=DEFAULT_EXE,
        help="release executable to copy and test (default: target/release/markturbo.exe)",
    )
    parser.add_argument(
        "--case",
        choices=REQUIRED_CASE_IDS,
        help="run one case for debugging; it never produces acceptance PASS",
    )
    parser.add_argument(
        "--keep-workdir-on-failure",
        action="store_true",
        help="preserve the isolated data, config, logs, and stderr directory after a non-PASS run",
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
    return main_native_acceptance(args, native_run_plan())


if __name__ == "__main__":
    raise SystemExit(main())
