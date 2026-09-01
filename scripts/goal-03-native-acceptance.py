#!/usr/bin/env -S uv run --script
# /// script
# requires-python = ">=3.11"
# dependencies = ["pywinauto==0.6.9"]
# ///
"""Exercise Goal 03's first-use document workflow through a real Windows UI.

The harness is fail-closed. A PASS binds every observation to the supplied
x64 executable SHA-256 and uses isolated MARKTURBO_DATA_DIR and
MARKTURBO_CONFIG_DIR roots. Evidence contains fingerprints and booleans, never
document text or filesystem paths.
"""

from __future__ import annotations

import argparse
import ctypes
import ctypes.wintypes as wt
import hashlib
import json
import math
import os
import runpy
import shutil
import subprocess
import sys
import tempfile
import time
from pathlib import Path
from typing import Any, Callable


REPO = Path(__file__).resolve().parent.parent
DEFAULT_EXE = REPO / "target" / "release" / "markturbo.exe"
DEFAULT_EVIDENCE = REPO / ".scratch" / "goal-03-native-acceptance-v1.json"
GOAL_02 = Path(__file__).with_name("goal-02-native-acceptance.py")
BASE = runpy.run_path(GOAL_02)

SCHEMA = "markturbo.goal-03-native-acceptance"
SCHEMA_VERSION = 1
SHA256_RE = BASE["SHA256_RE"]
HarnessBlocked = BASE["HarnessBlocked"]
HarnessFailure = BASE["HarnessFailure"]
Fingerprint = BASE["Fingerprint"]
RunningApp = BASE["RunningApp"]
LaunchSpec = BASE["LaunchSpec"]
Win32 = BASE["Win32"]
BaseNativeHarness = BASE["NativeHarness"]
fingerprint_text = BASE["fingerprint_text"]
sha256_file = BASE["sha256_file"]
normalize_expected_hash = BASE["normalize_expected_hash"]
executable_hash_failure = BASE["executable_hash_failure"]
inspect_pe = BASE["inspect_pe"]
platform_preflight_failure = BASE["platform_preflight_failure"]
security_context_failure = BASE["security_context_failure"]
safe_exception_name = BASE["safe_exception_name"]
safe_failure_type = BASE["safe_failure_type"]
wait_until = BASE["wait_until"]
load_pywinauto = BASE["load_pywinauto"]
preflight_goal_02 = BASE["preflight"]
utc_now = BASE["utc_now"]
finite_nonnegative = BASE["finite_nonnegative"]
validate_fingerprint = BASE["validate_fingerprint"]
validate_process_context = BASE["validate_process_context"]
require_true = BASE["require_true"]
mark_remaining_cases = BASE["mark_remaining_cases"]
IMAGE_FILE_MACHINE_AMD64 = BASE["IMAGE_FILE_MACHINE_AMD64"]
PE32_PLUS_MAGIC = BASE["PE32_PLUS_MAGIC"]
VK_A = BASE["VK_A"]
VK_CONTROL = BASE["VK_CONTROL"]
VK_SHIFT = 0x10
VK_S = BASE["VK_S"]
VK_RETURN = 0x0D
VK_ESCAPE = 0x1B
VK_V = 0x56
VK_W = BASE["VK_W"]
key_input = BASE["key_input"]
CF_UNICODETEXT = 13
GMEM_MOVEABLE = 0x0002

DOCUMENT_SAVE_AS_AUTOMATION_ID = "markturbo-document-save-as"
WELCOME_NEW_AUTOMATION_ID = "markturbo-welcome-new"
WELCOME_PASTE_AUTOMATION_ID = "markturbo-welcome-paste"
WELCOME_OPEN_FILE_AUTOMATION_ID = "markturbo-welcome-open-file"
WELCOME_OPEN_FOLDER_AUTOMATION_ID = "markturbo-welcome-open-folder"
WELCOME_OPEN_SAMPLE_AUTOMATION_ID = "markturbo-welcome-open-sample"
WELCOME_DONT_SHOW_AUTOMATION_ID = "markturbo-welcome-dont-show-again"

CASE_WELCOME = "welcome_no_argument"
CASE_NEW_PASTE = "new_and_paste_unicode"
CASE_SAVE_CREATE = "save_as_create_and_reopen"
CASE_SAVE_CANCEL_OVERWRITE = "save_as_cancel_and_overwrite"
CASE_SAMPLE = "bundled_sample"
CASE_RECENTS = "recent_bound_restart_stale"
CASE_CLI = "explicit_cli_targets"
REQUIRED_CASE_IDS = (
    CASE_WELCOME,
    CASE_NEW_PASTE,
    CASE_SAVE_CREATE,
    CASE_SAVE_CANCEL_OVERWRITE,
    CASE_SAMPLE,
    CASE_RECENTS,
    CASE_CLI,
)
CASE_FLOWS = {
    CASE_WELCOME: "no-argument welcome -> dont-show -> restart memory buffer",
    CASE_NEW_PASTE: "welcome New and Paste -> editable Unicode buffers",
    CASE_SAVE_CREATE: "new buffer -> Save As create -> direct reopen exact text",
    CASE_SAVE_CANCEL_OVERWRITE: "Save As cancel -> Replace cancel -> Replace confirm",
    CASE_SAMPLE: "welcome bundled sample -> editable source",
    CASE_RECENTS: "load eleven persisted targets -> restart ten recents -> stale disabled",
    CASE_CLI: "explicit file and directory arguments bypass welcome",
}

DOCUMENT_SENTINEL = "MTG03-NATIVE-SENTINEL-\u4fdd\u5b58-\U0001f680"
NEW_TEXT = f"# {DOCUMENT_SENTINEL}: New\n\n\u4e2d\u6587 and emoji \U0001f680\n"
PASTE_TEXT = f"# {DOCUMENT_SENTINEL}: Paste\n\n\u65e5\u672c\u8a9e and emoji \U0001f9ea\n"
SAVE_TEXT = f"# {DOCUMENT_SENTINEL}: Save As\n\nExact CJK \u4f60\u597d and emoji \U0001f680\n"
OVERWRITE_TEXT = f"# {DOCUMENT_SENTINEL}: Replace\n\nExact CJK \u4fdd\u5b58 and emoji \U0001f9ea\n"
EXISTING_DESTINATION = b"goal-03 original destination\n"
CANCELLED_DESTINATION = b"goal-03 cancelled destination\n"

SAFE_STRINGS = frozenset(CASE_FLOWS.values()) | BASE["INTEGRITY_NAMES"]
ALLOWED_OBSERVATION_KEYS = {
    "app_logs_scanned",
    "byte_count",
    "config_files_scanned",
    "direct_directory_bypassed_welcome",
    "direct_file_bypassed_welcome",
    "dont_show_memory_buffer",
    "dont_show_visible",
    "editor_after_overwrite_cancel",
    "editor_after_save_as_cancel",
    "editor_before_cancellation",
    "files_scanned",
    "flow",
    "foreground_verified",
    "integrity",
    "integrity_rid",
    "new_buffer_created",
    "new_unicode_editor",
    "overwrite_cancelled",
    "overwrite_confirmed",
    "paste_buffer_created",
    "paste_unicode_editor",
    "process_context",
    "process_contexts",
    "recent_count",
    "recent_restart_visible",
    "reopened_editor",
    "runtime_scan",
    "sample_workspace_opened",
    "save_as_cancelled",
    "save_as_cancel_destination_after",
    "save_as_cancel_destination_before",
    "save_as_cancel_focus_preserved",
    "save_as_created",
    "saved_destination",
    "session_id",
    "sha256",
    "source_after_cancel",
    "source_before",
    "stale_recent_disabled",
    "overwrite_cancel_focus_preserved",
    "utf16le_sentinel_absent",
    "utf8_sentinel_absent",
    "welcome_visible",
}


def has_unicode_clipboard_text(formats: set[int]) -> bool:
    return CF_UNICODETEXT in formats


def recent_settings_document(documents: list[Path]) -> bytes:
    lines = ["show-welcome-on-startup = true", ""]
    for path in documents:
        lines.extend(
            [
                "[[recent-targets]]",
                f"path = {json.dumps(str(path))}",
                'kind = "file"',
                f"display-name = {json.dumps(path.name)}",
                "",
            ]
        )
    return "\n".join(lines).encode("utf-8")


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
        "summary": {},
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


def validate_observations(value: Any, key: str | None = None) -> None:
    if isinstance(value, dict):
        for nested_key, nested in value.items():
            if nested_key not in ALLOWED_OBSERVATION_KEYS:
                raise ValueError("unknown observation field")
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
    for key in ("files_scanned", "app_logs_scanned", "config_files_scanned"):
        if (
            not isinstance(value.get(key), int)
            or isinstance(value[key], bool)
            or value[key] < 0
        ):
            raise ValueError(f"invalid {key}")
    if value["files_scanned"] <= 0 or value["app_logs_scanned"] <= 0:
        raise ValueError("runtime scan must cover an application log")
    require_true(value, "utf8_sentinel_absent", "utf16le_sentinel_absent")


def validate_environment(value: Any) -> dict[str, Any]:
    if not isinstance(value, dict):
        raise ValueError("invalid environment evidence")
    BASE["validate_environment"](value)
    return validate_process_context(value.get("harness_process"))


def required_observations(case_id: str) -> set[str]:
    common = {"flow", "process_context", "foreground_verified", "runtime_scan"}
    specifics = {
        CASE_WELCOME: {
            "welcome_visible",
            "dont_show_visible",
            "dont_show_memory_buffer",
        },
        CASE_NEW_PASTE: {
            "new_buffer_created",
            "paste_buffer_created",
            "new_unicode_editor",
            "paste_unicode_editor",
        },
        CASE_SAVE_CREATE: {"save_as_created", "saved_destination", "reopened_editor"},
        CASE_SAVE_CANCEL_OVERWRITE: {
            "editor_before_cancellation",
            "editor_after_save_as_cancel",
            "editor_after_overwrite_cancel",
            "source_before",
            "source_after_cancel",
            "save_as_cancel_destination_before",
            "save_as_cancel_destination_after",
            "saved_destination",
            "save_as_cancelled",
            "save_as_cancel_focus_preserved",
            "overwrite_cancelled",
            "overwrite_cancel_focus_preserved",
            "overwrite_confirmed",
        },
        CASE_SAMPLE: {"sample_workspace_opened"},
        CASE_RECENTS: {
            "recent_restart_visible",
            "recent_count",
            "stale_recent_disabled",
        },
        CASE_CLI: {"direct_file_bypassed_welcome", "direct_directory_bypassed_welcome"},
    }
    return common | specifics[case_id]


def validate_passed_case(case_id: str, observations: dict[str, Any], parent: dict[str, Any]) -> None:
    if observations.get("flow") != CASE_FLOWS[case_id]:
        raise ValueError("case flow mechanics do not match the required scenario")
    if validate_process_context(observations.get("process_context")) != parent:
        raise ValueError("case process context differs from harness context")
    require_true(observations, "foreground_verified")
    validate_runtime_scan(observations.get("runtime_scan"))
    if case_id == CASE_WELCOME:
        require_true(observations, "welcome_visible", "dont_show_visible", "dont_show_memory_buffer")
    elif case_id == CASE_NEW_PASTE:
        require_true(observations, "new_buffer_created", "paste_buffer_created")
        validate_fingerprint(observations.get("new_unicode_editor"))
        validate_fingerprint(observations.get("paste_unicode_editor"))
    elif case_id == CASE_SAVE_CREATE:
        saved = validate_fingerprint(observations.get("saved_destination"))
        reopened = validate_fingerprint(observations.get("reopened_editor"))
        require_true(observations, "save_as_created")
        if saved != reopened:
            raise ValueError("direct reopen did not preserve exact Save As text")
    elif case_id == CASE_SAVE_CANCEL_OVERWRITE:
        editor_before = validate_fingerprint(observations.get("editor_before_cancellation"))
        editor_after_save_as_cancel = validate_fingerprint(
            observations.get("editor_after_save_as_cancel")
        )
        editor_after_overwrite_cancel = validate_fingerprint(
            observations.get("editor_after_overwrite_cancel")
        )
        before = validate_fingerprint(observations.get("source_before"))
        after_cancel = validate_fingerprint(observations.get("source_after_cancel"))
        save_as_cancel_before = validate_fingerprint(
            observations.get("save_as_cancel_destination_before")
        )
        save_as_cancel_after = validate_fingerprint(
            observations.get("save_as_cancel_destination_after")
        )
        saved = validate_fingerprint(observations.get("saved_destination"))
        require_true(
            observations,
            "save_as_cancelled",
            "save_as_cancel_focus_preserved",
            "overwrite_cancelled",
            "overwrite_cancel_focus_preserved",
            "overwrite_confirmed",
        )
        if save_as_cancel_before != save_as_cancel_after:
            raise ValueError("Save As picker cancellation changed the named destination")
        if before != after_cancel:
            raise ValueError("overwrite cancellation changed the destination")
        if not (
            editor_before == editor_after_save_as_cancel == editor_after_overwrite_cancel
        ):
            raise ValueError("cancellation changed the unsaved editor")
        if saved != editor_before or saved == before:
            raise ValueError("confirmed overwrite did not replace the destination")
    elif case_id == CASE_SAMPLE:
        require_true(observations, "sample_workspace_opened")
    elif case_id == CASE_RECENTS:
        require_true(observations, "recent_restart_visible", "stale_recent_disabled")
        if observations.get("recent_count") != 10:
            raise ValueError("recent target bound was not exactly ten")
    elif case_id == CASE_CLI:
        require_true(observations, "direct_file_bypassed_welcome", "direct_directory_bypassed_welcome")


def validate_evidence(evidence: dict[str, Any]) -> None:
    if evidence.get("schema") != SCHEMA or evidence.get("schema_version") != SCHEMA_VERSION:
        raise ValueError("unsupported evidence schema")
    if evidence.get("status") not in {"PASS", "FAIL", "BLOCKED"}:
        raise ValueError("invalid evidence status")
    executable = evidence.get("executable")
    if not isinstance(executable, dict):
        raise ValueError("missing executable evidence")
    expected = executable.get("expected_sha256")
    if not isinstance(expected, str) or not SHA256_RE.fullmatch(expected):
        raise ValueError("invalid expected executable SHA-256")
    actual = executable.get("sha256")
    if actual is not None and (not isinstance(actual, str) or not SHA256_RE.fullmatch(actual)):
        raise ValueError("invalid executable SHA-256")
    copied = executable.get("copied_sha256")
    if copied is not None and (not isinstance(copied, str) or not SHA256_RE.fullmatch(copied)):
        raise ValueError("invalid copied executable SHA-256")
    if executable.get("hash_verified") is True and actual != expected:
        raise ValueError("verified executable hash does not match expected hash")
    if executable.get("copy_hash_verified") is True and copied != expected:
        raise ValueError("verified copied executable hash does not match expected hash")
    if evidence["status"] == "PASS":
        if not executable.get("hash_verified") or not executable.get("copy_hash_verified"):
            raise ValueError("PASS requires verified executable hashes")
        if executable.get("format") != "PE32+" or executable.get("machine") != "x86_64":
            raise ValueError("PASS requires an AMD64 PE32+ executable")
        if executable.get("machine_code") != IMAGE_FILE_MACHINE_AMD64 or executable.get("optional_magic") != PE32_PLUS_MAGIC:
            raise ValueError("PASS requires an AMD64 PE32+ executable")
        if (
            not isinstance(executable.get("byte_count"), int)
            or isinstance(executable["byte_count"], bool)
            or executable["byte_count"] <= 0
        ):
            raise ValueError("PASS requires a nonempty executable")
    parent = None

    cases = evidence.get("cases")
    if not isinstance(cases, list) or len(cases) != len(REQUIRED_CASE_IDS):
        raise ValueError("required case set is incomplete")
    ids = [case.get("id") for case in cases if isinstance(case, dict)]
    if len(set(ids)) != len(ids) or set(ids) != set(REQUIRED_CASE_IDS):
        raise ValueError("required case set is incomplete")
    if evidence["status"] == "PASS" or any(
        isinstance(case, dict) and case.get("status") == "PASS" for case in cases
    ):
        parent = validate_environment(evidence.get("environment"))
    for case in cases:
        status = case.get("status")
        if status not in {"PASS", "FAIL", "BLOCKED", "NOT_RUN"}:
            raise ValueError("invalid case status")
        reason = case.get("reason_code")
        if reason is not None and (
            not isinstance(reason, str) or not BASE["REASON_CODE_RE"].fullmatch(reason)
        ):
            raise ValueError("invalid case reason code")
        failure_type = case.get("failure_type")
        if failure_type is not None and (
            not isinstance(failure_type, str) or failure_type not in BASE["SAFE_FAILURE_TYPES"]
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
    expected_summary = {
        "required_case_count": len(REQUIRED_CASE_IDS),
        "passed_case_count": sum(case["status"] == "PASS" for case in cases),
        "blocked_case_count": sum(case["status"] == "BLOCKED" for case in cases),
        "failed_case_count": sum(case["status"] == "FAIL" for case in cases),
        "not_run_case_count": sum(case["status"] == "NOT_RUN" for case in cases),
    }
    if summary != expected_summary:
        raise ValueError("case summary does not match case evidence")


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
    target: Path | None,
    data_root: Path,
    config_root: Path,
    workspace_root: Path,
    stderr_path: Path,
    base_env: dict[str, str] | None = None,
) -> Any:
    paths = [copied_exe, data_root, config_root, workspace_root, stderr_path]
    if target is not None:
        paths.append(target)
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
    args = (str(copied_exe),) if target is None else (str(copied_exe), str(target))
    return LaunchSpec(args=args, cwd=str(workspace_root), env=env, stderr_path=stderr_path)


def production_source(path: Path) -> str:
    return path.read_text(encoding="utf-8").split("\n#[cfg(test)]", 1)[0]


def source_contract_failure() -> str | None:
    workspace = production_source(
        REPO / "crates" / "mt-app" / "src" / "views" / "workspace.rs"
    )
    document = production_source(
        REPO / "crates" / "mt-app" / "src" / "views" / "document.rs"
    )
    for value in (
        WELCOME_NEW_AUTOMATION_ID,
        WELCOME_PASTE_AUTOMATION_ID,
        WELCOME_OPEN_FILE_AUTOMATION_ID,
        WELCOME_OPEN_FOLDER_AUTOMATION_ID,
        WELCOME_OPEN_SAMPLE_AUTOMATION_ID,
        WELCOME_DONT_SHOW_AUTOMATION_ID,
    ):
        if value not in workspace:
            return "WELCOME_UIA_CONTRACT_MISSING"
    if DOCUMENT_SAVE_AS_AUTOMATION_ID not in document:
        return "SAVE_AS_UIA_CONTRACT_MISSING"
    if "initial.is_none() && show_welcome_on_startup" not in workspace:
        return "NO_ARGUMENT_WELCOME_CONTRACT_MISSING"
    if "fn dont_show_welcome_again" not in workspace:
        return "DONT_SHOW_WELCOME_CONTRACT_MISSING"
    for contract in (
        "fn on_paste_into_new",
        "cx.read_from_clipboard()",
        "fn open_bundled_sample",
        "fn record_recent_target",
        "fn prompt_save_as_overwrite",
        "PromptButton::ok(i18n::t(i18n::Key::Replace, cx))",
    ):
        if contract not in workspace:
            return "FIRST_USE_SOURCE_CONTRACT_MISSING"
    if "DocumentEvent::SaveAsRequested" not in document:
        return "SAVE_AS_SOURCE_CONTRACT_MISSING"
    return None


def scan_case_artifacts(case_root: Path) -> dict[str, Any]:
    try:
        roots = (case_root / "data", case_root / "config")
        paths = [
            path
            for root in roots
            if root.exists()
            for path in root.rglob("*")
            if path.is_file()
        ]
        stderr = case_root / "stderr.log"
        if stderr.exists():
            paths.append(stderr)
        paths = sorted(set(paths))
    except OSError as error:
        raise HarnessFailure("RUNTIME_ARTIFACT_ENUM_FAILED", safe_exception_name(error)) from None
    if not paths:
        raise HarnessFailure("RUNTIME_ARTIFACTS_MISSING")
    utf8 = DOCUMENT_SENTINEL.encode("utf-8")
    utf16 = DOCUMENT_SENTINEL.encode("utf-16-le")
    logs_root = case_root / "data" / "logs"
    app_logs = [path for path in paths if logs_root in path.parents]
    if not app_logs:
        raise HarnessFailure("APP_LOG_MISSING")
    for path in paths:
        try:
            value = path.read_bytes()
        except OSError as error:
            raise HarnessFailure("RUNTIME_ARTIFACT_SCAN_FAILED", safe_exception_name(error)) from None
        if utf8 in value:
            raise HarnessFailure("UTF8_DOCUMENT_SENTINEL_LEAKED")
        if utf16 in value:
            raise HarnessFailure("UTF16LE_DOCUMENT_SENTINEL_LEAKED")
    config_root = case_root / "config"
    return {
        "files_scanned": len(paths),
        "app_logs_scanned": len(app_logs),
        "config_files_scanned": sum(config_root in path.parents for path in paths),
        "utf8_sentinel_absent": True,
        "utf16le_sentinel_absent": True,
    }


class Goal03Harness(BaseNativeHarness):
    def profile(self, case_id: str) -> tuple[Path, Path, Path, Path, Path]:
        data_root, config_root, workspace_root, stderr_path = self.case_roots(case_id)
        case_root = data_root.parent
        return case_root, data_root, config_root, workspace_root, stderr_path

    def launch_target(
        self,
        target: Path | None,
        data_root: Path,
        config_root: Path,
        workspace_root: Path,
        stderr_path: Path,
    ) -> Any:
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
            app = self.application_class(backend="uia").connect(process=process.pid, timeout=self.ui_timeout)
            window = app.top_window()
            window.wait("exists visible enabled ready", timeout=self.ui_timeout)
            hwnd = int(window.handle)
        except Exception as error:
            if process.poll() is not None:
                raise HarnessFailure("PROCESS_EXITED_BEFORE_UI") from None
            raise HarnessFailure("MAIN_WINDOW_UIA_TIMEOUT", safe_exception_name(error)) from None
        if not hwnd:
            raise HarnessBlocked("UIA_MAIN_WINDOW_HANDLE_MISSING")
        self.win32.require_foreground(hwnd, self.ui_timeout)
        return RunningApp(
            process,
            window,
            hwnd,
            spec,
            child_context,
            data_root / "logs" / f"markturbo-{process.pid}.log",
        )

    def welcome_control(self, app: Any, automation_id: str) -> Any:
        return self.find_control(
            app,
            automation_id,
            "Button",
            "WELCOME_UIA_TIMEOUT",
            "WELCOME_UIA_CONTRACT_MISMATCH",
        )

    def wait_welcome_absent(self, app: Any) -> None:
        wait_until(
            lambda: self.control_absent(
                app,
                WELCOME_NEW_AUTOMATION_ID,
                "Button",
                "WELCOME_UIA_QUERY_FAILED",
                "WELCOME_UIA_CONTRACT_MISMATCH",
            ),
            self.ui_timeout,
            "WELCOME_DID_NOT_CLOSE",
        )

    def source_editor_ready(self, app: Any) -> None:
        self.activate_source_layout(app)

    def source_editor_has_focus(self, app: Any) -> bool:
        try:
            editor = self.control_by_id(
                app.hwnd,
                BASE["SOURCE_EDITOR_AUTOMATION_ID"],
                "Edit",
                "SOURCE_EDITOR_UIA_CONTRACT_MISMATCH",
            )
            return editor is not None and bool(editor.has_keyboard_focus())
        except HarnessFailure:
            raise
        except Exception as error:
            raise HarnessFailure(
                "SOURCE_EDITOR_FOCUS_QUERY_FAILED", safe_exception_name(error)
            ) from None

    def require_source_editor_focus(self, app: Any, failure_code: str) -> None:
        wait_until(
            lambda: self.source_editor_has_focus(app),
            self.ui_timeout,
            failure_code,
            interval=0.025,
        )

    def replace_editor(self, app: Any, value: str) -> Fingerprint:
        clipboard_before = self.read_text_clipboard()
        try:
            self.write_text_clipboard(value)
            self.focus_editor(app)
            self.win32.send_shortcut(app.hwnd, VK_A)
            self.win32.send_shortcut(app.hwnd, VK_V)
            expected = fingerprint_text(value)
            actual, _ = self.wait_editor_fingerprint(
                app, expected, self.ui_timeout, already_focused=True
            )
            return actual
        finally:
            self.write_text_clipboard(clipboard_before)

    def discard_memory_document(self, app: Any) -> None:
        self.win32.send_shortcut(app.hwnd, VK_W)
        self.click_lifecycle_decision(app, "Discard")
        wait_until(
            lambda: self.editor_absent_while_running(app),
            self.ui_timeout,
            "MEMORY_DISCARD_TIMEOUT",
        )
        self.close_app(app)

    def close_app(self, app: Any) -> None:
        self.win32.post_close(app.hwnd)
        self.wait_process_exit(app)

    def read_text_clipboard(self) -> str | None:
        user32 = ctypes.WinDLL("user32", use_last_error=True)
        kernel32 = ctypes.WinDLL("kernel32", use_last_error=True)
        user32.OpenClipboard.argtypes = [wt.HWND]
        user32.OpenClipboard.restype = wt.BOOL
        user32.CloseClipboard.restype = wt.BOOL
        user32.EnumClipboardFormats.argtypes = [wt.UINT]
        user32.EnumClipboardFormats.restype = wt.UINT
        user32.IsClipboardFormatAvailable.argtypes = [wt.UINT]
        user32.IsClipboardFormatAvailable.restype = wt.BOOL
        user32.GetClipboardData.argtypes = [wt.UINT]
        user32.GetClipboardData.restype = wt.HANDLE
        kernel32.GlobalLock.argtypes = [wt.HGLOBAL]
        kernel32.GlobalLock.restype = wt.LPVOID
        kernel32.GlobalUnlock.argtypes = [wt.HGLOBAL]
        kernel32.GlobalUnlock.restype = wt.BOOL
        if not user32.OpenClipboard(None):
            raise HarnessBlocked("CLIPBOARD_PRESERVATION_UNAVAILABLE")
        try:
            formats: set[int] = set()
            format_id = 0
            while format_id := int(user32.EnumClipboardFormats(format_id)):
                formats.add(format_id)
            if formats and not has_unicode_clipboard_text(formats):
                raise HarnessBlocked("CLIPBOARD_CONTAINS_NON_TEXT")
            if not user32.IsClipboardFormatAvailable(CF_UNICODETEXT):
                return None
            handle = user32.GetClipboardData(CF_UNICODETEXT)
            if not handle:
                raise HarnessBlocked("CLIPBOARD_TEXT_READ_FAILED")
            pointer = kernel32.GlobalLock(handle)
            if not pointer:
                raise HarnessBlocked("CLIPBOARD_TEXT_READ_FAILED")
            try:
                return ctypes.wstring_at(pointer)
            finally:
                kernel32.GlobalUnlock(handle)
        finally:
            user32.CloseClipboard()

    def write_text_clipboard(self, value: str | None) -> None:
        user32 = ctypes.WinDLL("user32", use_last_error=True)
        kernel32 = ctypes.WinDLL("kernel32", use_last_error=True)
        user32.OpenClipboard.argtypes = [wt.HWND]
        user32.OpenClipboard.restype = wt.BOOL
        user32.CloseClipboard.restype = wt.BOOL
        user32.EmptyClipboard.restype = wt.BOOL
        user32.SetClipboardData.argtypes = [wt.UINT, wt.HANDLE]
        user32.SetClipboardData.restype = wt.HANDLE
        kernel32.GlobalAlloc.argtypes = [wt.UINT, ctypes.c_size_t]
        kernel32.GlobalAlloc.restype = wt.HGLOBAL
        kernel32.GlobalFree.argtypes = [wt.HGLOBAL]
        kernel32.GlobalFree.restype = wt.HGLOBAL
        kernel32.GlobalLock.argtypes = [wt.HGLOBAL]
        kernel32.GlobalLock.restype = wt.LPVOID
        kernel32.GlobalUnlock.argtypes = [wt.HGLOBAL]
        kernel32.GlobalUnlock.restype = wt.BOOL
        if not user32.OpenClipboard(None):
            raise HarnessBlocked("CLIPBOARD_PRESERVATION_UNAVAILABLE")
        try:
            if not user32.EmptyClipboard():
                raise HarnessBlocked("CLIPBOARD_WRITE_FAILED")
            if value is None:
                return
            payload = value.encode("utf-16-le") + b"\0\0"
            handle = kernel32.GlobalAlloc(GMEM_MOVEABLE, len(payload))
            if not handle:
                raise HarnessBlocked("CLIPBOARD_WRITE_FAILED")
            pointer = kernel32.GlobalLock(handle)
            if not pointer:
                kernel32.GlobalFree(handle)
                raise HarnessBlocked("CLIPBOARD_WRITE_FAILED")
            ctypes.memmove(pointer, payload, len(payload))
            kernel32.GlobalUnlock(handle)
            if not user32.SetClipboardData(CF_UNICODETEXT, handle):
                kernel32.GlobalFree(handle)
                raise HarnessBlocked("CLIPBOARD_WRITE_FAILED")
        finally:
            user32.CloseClipboard()

    def common_file_dialog(self, app: Any) -> tuple[int, Any]:
        last_error: str | None = None

        def locate() -> tuple[int, Any] | None:
            nonlocal last_error
            try:
                dialogs = self.win32.owned_task_dialogs(app.process.pid, app.hwnd)
                if len(dialogs) > 1:
                    raise HarnessFailure("MULTIPLE_SAVE_AS_FILE_DIALOGS")
                if not dialogs:
                    return None
                hwnd = dialogs[0]
                combo = self.control_by_id(
                    hwnd,
                    "FileNameControlHost",
                    "ComboBox",
                    "SAVE_AS_FILE_DIALOG_UIA_CONTRACT_MISMATCH",
                )
                if combo is None:
                    return None
                edits = [
                    control
                    for control in combo.descendants()
                    if control.element_info.automation_id == "1001"
                    and control.element_info.control_type == "Edit"
                ]
                if len(edits) != 1:
                    raise HarnessFailure("SAVE_AS_FILE_DIALOG_UIA_CONTRACT_MISMATCH")
                edit = edits[0]
                if not edit.is_visible() or not edit.is_enabled():
                    return None
                return hwnd, edit
            except HarnessFailure:
                raise
            except Exception as error:
                last_error = safe_exception_name(error)
                return None

        try:
            return wait_until(locate, self.ui_timeout, "SAVE_AS_FILE_DIALOG_TIMEOUT", interval=0.05)
        except HarnessFailure as error:
            if error.code == "SAVE_AS_FILE_DIALOG_TIMEOUT" and last_error:
                raise HarnessFailure(error.code, last_error) from None
            raise

    def accept_native_overwrite_confirmation(self, app: Any, file_dialog_hwnd: int) -> None:
        def locate() -> tuple[int, Any] | bool | None:
            nested = self.win32.owned_task_dialogs(app.process.pid, file_dialog_hwnd)
            if len(nested) > 1:
                raise HarnessFailure("MULTIPLE_NATIVE_OVERWRITE_DIALOGS")
            if nested:
                hwnd = nested[0]
                confirm = self.control_by_id(
                    hwnd,
                    "CommandButton_6",
                    "Button",
                    "NATIVE_OVERWRITE_DIALOG_UIA_CONTRACT_MISMATCH",
                    expected_class_name="CCPushButton",
                )
                return (hwnd, confirm) if confirm is not None else None
            if file_dialog_hwnd not in self.win32.owned_task_dialogs(
                app.process.pid, app.hwnd
            ):
                return True
            return None

        result = wait_until(
            locate,
            self.ui_timeout,
            "NATIVE_OVERWRITE_DIALOG_TIMEOUT",
            interval=0.05,
        )
        if result is True:
            return
        _, confirm = result
        self.click_control(confirm, "NATIVE_OVERWRITE_CONFIRM_CLICK_FAILED")

    def select_save_path(self, app: Any, path: Path) -> None:
        hwnd, edit = self.common_file_dialog(app)
        self.click_control(edit, "SAVE_AS_FILENAME_FOCUS_FAILED")
        self.win32.send_shortcut(hwnd, VK_A)
        self.win32.send_unicode(hwnd, str(path))
        self.win32.send_key(hwnd, VK_RETURN)
        if path.exists():
            self.accept_native_overwrite_confirmation(app, hwnd)
        wait_until(
            lambda: hwnd
            not in self.win32.owned_task_dialogs(app.process.pid, app.hwnd),
            self.ui_timeout,
            "SAVE_AS_FILE_DIALOG_CLOSE_TIMEOUT",
            interval=0.05,
        )
        self.win32.require_foreground(app.hwnd, self.ui_timeout)

    def cancel_save_picker(self, app: Any, path: Path) -> None:
        hwnd, edit = self.common_file_dialog(app)
        self.click_control(edit, "SAVE_AS_FILENAME_FOCUS_FAILED")
        self.win32.send_shortcut(hwnd, VK_A)
        self.win32.send_unicode(hwnd, str(path))
        self.win32.send_key(hwnd, VK_ESCAPE)
        wait_until(
            lambda: not self.win32.owned_task_dialogs(app.process.pid, app.hwnd),
            self.ui_timeout,
            "SAVE_AS_PICKER_CANCEL_TIMEOUT",
        )
        self.win32.require_foreground(app.hwnd, self.ui_timeout)

    def request_save_as(self, app: Any) -> None:
        button = self.find_control(
            app,
            DOCUMENT_SAVE_AS_AUTOMATION_ID,
            "Button",
            "SAVE_AS_BUTTON_UIA_TIMEOUT",
            "SAVE_AS_BUTTON_UIA_CONTRACT_MISMATCH",
        )
        self.click_control(button, "SAVE_AS_BUTTON_CLICK_FAILED")

    def request_save_as_shortcut(self, app: Any) -> None:
        self.require_source_editor_focus(app, "SAVE_AS_SHORTCUT_EDITOR_NOT_FOCUSED")
        self.win32.require_foreground(app.hwnd, self.ui_timeout)
        self.win32.send_inputs(
            [
                key_input(VK_CONTROL, False),
                key_input(VK_SHIFT, False),
                key_input(VK_S, False),
                key_input(VK_S, True),
                key_input(VK_SHIFT, True),
                key_input(VK_CONTROL, True),
            ]
        )

    def overwrite_buttons(self, app: Any) -> dict[str, Any]:
        def locate() -> dict[str, Any] | None:
            dialogs = self.win32.owned_task_dialogs(app.process.pid, app.hwnd)
            if len(dialogs) > 1:
                raise HarnessFailure("MULTIPLE_OVERWRITE_DIALOGS")
            if not dialogs:
                return None
            hwnd = dialogs[0]
            replace = self.control_by_id(
                hwnd,
                "CommandButton_1",
                "Button",
                "OVERWRITE_DIALOG_UIA_CONTRACT_MISMATCH",
                "Replace",
                "CCPushButton",
            )
            cancel = self.control_by_id(
                hwnd,
                "CommandButton_2",
                "Button",
                "OVERWRITE_DIALOG_UIA_CONTRACT_MISMATCH",
                "Cancel",
                "CCPushButton",
            )
            if replace is None or cancel is None:
                return None
            return {"Replace": replace, "Cancel": cancel}

        return wait_until(locate, self.ui_timeout, "OVERWRITE_DIALOG_TIMEOUT", interval=0.05)

    def choose_overwrite(self, app: Any, choice: str) -> None:
        self.click_control(self.overwrite_buttons(app)[choice], f"OVERWRITE_{choice.upper()}_CLICK_FAILED")

    def recent_controls(self, app: Any) -> list[Any]:
        try:
            uia = self.iuia_class()
            condition = uia.iuia.CreateTrueCondition()
            elements = self.fresh_uia_root(app.hwnd).element.FindAll(
                uia.tree_scope["descendants"], condition
            )
            controls = []
            seen_ids: set[str] = set()
            expected_control_type = uia.known_control_types["Button"]
            for index in range(elements.Length):
                element = elements.GetElement(index)
                automation_id = element.CurrentAutomationId or ""
                if automation_id.startswith(
                    "markturbo-welcome-recent-"
                ) and not automation_id.startswith(
                    (
                        "markturbo-welcome-recent-remove-",
                        "markturbo-welcome-recent-status-",
                    )
                ):
                    if (
                        automation_id in seen_ids
                        or element.CurrentControlType != expected_control_type
                    ):
                        raise HarnessFailure("RECENT_UIA_CONTRACT_MISMATCH")
                    control = self.uia_wrapper_class(self.uia_element_info_class(element))
                    info = control.element_info
                    if (
                        info.automation_id != automation_id
                        or info.control_type != "Button"
                        or not control.is_visible()
                    ):
                        raise HarnessFailure("RECENT_UIA_CONTRACT_MISMATCH")
                    seen_ids.add(automation_id)
                    controls.append(control)
            return controls
        except HarnessFailure:
            raise
        except Exception as error:
            raise HarnessFailure("RECENT_UIA_QUERY_FAILED", safe_exception_name(error)) from None

    def require_recent_status(self, app: Any, expected: str, failure_code: str) -> None:
        try:
            uia = self.iuia_class()
            elements = self.fresh_uia_root(app.hwnd).element.FindAll(
                uia.tree_scope["descendants"], uia.iuia.CreateTrueCondition()
            )
            expected_control_type = uia.known_control_types["Text"]
            matches = []
            for index in range(elements.Length):
                element = elements.GetElement(index)
                if (
                    (element.CurrentAutomationId or "").startswith(
                        "markturbo-welcome-recent-status-"
                    )
                    and
                    element.CurrentName == expected
                    and element.CurrentControlType == expected_control_type
                ):
                    control = self.uia_wrapper_class(self.uia_element_info_class(element))
                    if control.is_visible():
                        matches.append(control)
            if len(matches) != 1:
                raise HarnessFailure(failure_code)
        except HarnessFailure:
            raise
        except Exception as error:
            raise HarnessFailure(failure_code, safe_exception_name(error)) from None

    def scenario_welcome(self) -> dict[str, Any]:
        case_root, data, config, workspace, stderr = self.profile(CASE_WELCOME)
        first = self.launch_target(None, data, config, workspace, stderr)
        try:
            self.welcome_control(first, WELCOME_NEW_AUTOMATION_ID)
            self.welcome_control(first, WELCOME_PASTE_AUTOMATION_ID)
            dont_show = self.welcome_control(first, WELCOME_DONT_SHOW_AUTOMATION_ID)
            self.click_control(dont_show, "DONT_SHOW_WELCOME_CLICK_FAILED")
            self.wait_welcome_absent(first)
            self.source_editor_ready(first)
            self.close_app(first)
            second = self.launch_target(None, data, config, workspace, stderr)
            try:
                self.wait_welcome_absent(second)
                self.source_editor_ready(second)
                self.close_app(second)
            finally:
                self.reap(second)
            observations = {
                "welcome_visible": True,
                "dont_show_visible": True,
                "dont_show_memory_buffer": True,
                "flow": CASE_FLOWS[CASE_WELCOME],
                "process_context": first.security_context.evidence(),
                "foreground_verified": True,
            }
        finally:
            self.reap(first)
        observations["runtime_scan"] = scan_case_artifacts(case_root)
        return observations

    def scenario_new_paste(self) -> dict[str, Any]:
        case_root, data, config, workspace, stderr = self.profile(CASE_NEW_PASTE)
        new = self.launch_target(None, data, config, workspace, stderr)
        try:
            self.click_control(self.welcome_control(new, WELCOME_NEW_AUTOMATION_ID), "WELCOME_NEW_CLICK_FAILED")
            self.wait_welcome_absent(new)
            self.source_editor_ready(new)
            new_text = self.replace_editor(new, NEW_TEXT)
            self.discard_memory_document(new)
            paste = self.launch_target(None, data, config, workspace, stderr)
            try:
                clipboard_before = self.read_text_clipboard()
                try:
                    self.write_text_clipboard(PASTE_TEXT)
                    clipboard_text = self.read_text_clipboard()
                    if clipboard_text is None:
                        raise HarnessBlocked("CLIPBOARD_TEXT_READ_FAILED")
                    clipboard_fingerprint = fingerprint_text(clipboard_text)
                    self.click_control(self.welcome_control(paste, WELCOME_PASTE_AUTOMATION_ID), "WELCOME_PASTE_CLICK_FAILED")
                    self.wait_welcome_absent(paste)
                    self.source_editor_ready(paste)
                    paste_text = self.editor_fingerprint(paste)
                    if paste_text != clipboard_fingerprint:
                        raise HarnessFailure("WELCOME_PASTE_EXACT_TEXT_MISMATCH")
                finally:
                    self.write_text_clipboard(clipboard_before)
                self.discard_memory_document(paste)
            finally:
                self.reap(paste)
            observations = {
                "new_buffer_created": True,
                "paste_buffer_created": True,
                "new_unicode_editor": new_text.evidence(),
                "paste_unicode_editor": paste_text.evidence(),
                "flow": CASE_FLOWS[CASE_NEW_PASTE],
                "process_context": new.security_context.evidence(),
                "foreground_verified": True,
            }
        finally:
            self.reap(new)
        observations["runtime_scan"] = scan_case_artifacts(case_root)
        return observations

    def scenario_save_create(self) -> dict[str, Any]:
        case_root, data, config, workspace, stderr = self.profile(CASE_SAVE_CREATE)
        destination = (workspace / "created.md").resolve()
        first = self.launch_target(None, data, config, workspace, stderr)
        try:
            self.click_control(self.welcome_control(first, WELCOME_NEW_AUTOMATION_ID), "WELCOME_NEW_CLICK_FAILED")
            self.source_editor_ready(first)
            editor = self.replace_editor(first, SAVE_TEXT)
            self.request_save_as(first)
            self.select_save_path(first, destination)
            saved = self.wait_file(destination, editor, "SAVE_AS_CREATE_TIMEOUT")
            self.close_app(first)
            reopened = self.launch_target(destination, data, config, workspace, stderr)
            try:
                self.wait_welcome_absent(reopened)
                self.source_editor_ready(reopened)
                exact = self.editor_fingerprint(reopened)
                if exact != editor:
                    raise HarnessFailure("SAVE_AS_REOPEN_EXACT_TEXT_MISMATCH")
                self.close_app(reopened)
            finally:
                self.reap(reopened)
            observations = {
                "save_as_created": True,
                "saved_destination": saved.evidence(),
                "reopened_editor": exact.evidence(),
                "flow": CASE_FLOWS[CASE_SAVE_CREATE],
                "process_context": first.security_context.evidence(),
                "foreground_verified": True,
            }
        finally:
            self.reap(first)
        observations["runtime_scan"] = scan_case_artifacts(case_root)
        return observations

    def scenario_save_cancel_overwrite(self) -> dict[str, Any]:
        case_root, data, config, workspace, stderr = self.profile(CASE_SAVE_CANCEL_OVERWRITE)
        cancelled_destination = (workspace / "cancelled.md").resolve()
        destination = (workspace / "replace.md").resolve()
        BASE["write_durable"](cancelled_destination, CANCELLED_DESTINATION)
        BASE["write_durable"](destination, EXISTING_DESTINATION)
        cancelled_before = sha256_file(cancelled_destination)
        before = sha256_file(destination)
        app = self.launch_target(None, data, config, workspace, stderr)
        try:
            self.click_control(self.welcome_control(app, WELCOME_NEW_AUTOMATION_ID), "WELCOME_NEW_CLICK_FAILED")
            self.source_editor_ready(app)
            editor = self.replace_editor(app, OVERWRITE_TEXT)
            self.request_save_as_shortcut(app)
            self.cancel_save_picker(app, cancelled_destination)
            self.require_source_editor_focus(app, "SAVE_AS_PICKER_CANCEL_FOCUS_CHANGED")
            cancelled_after = sha256_file(cancelled_destination)
            editor_after_save_as_cancel, _ = self.wait_editor_fingerprint(
                app, editor, self.ui_timeout, already_focused=True
            )
            if cancelled_after != cancelled_before or editor_after_save_as_cancel != editor:
                raise HarnessFailure("SAVE_AS_PICKER_CANCEL_CHANGED_STATE")
            self.request_save_as_shortcut(app)
            self.select_save_path(app, destination)
            self.choose_overwrite(app, "Cancel")
            self.require_source_editor_focus(app, "OVERWRITE_CANCEL_FOCUS_CHANGED")
            after_cancel = sha256_file(destination)
            editor_after_overwrite_cancel, _ = self.wait_editor_fingerprint(
                app, editor, self.ui_timeout, already_focused=True
            )
            if after_cancel != before or editor_after_overwrite_cancel != editor:
                raise HarnessFailure("OVERWRITE_CANCEL_CHANGED_STATE")
            self.request_save_as_shortcut(app)
            self.select_save_path(app, destination)
            self.choose_overwrite(app, "Replace")
            saved = self.wait_file(destination, editor, "OVERWRITE_CONFIRM_TIMEOUT")
            self.close_app(app)
            observations = {
                "editor_before_cancellation": editor.evidence(),
                "editor_after_save_as_cancel": editor_after_save_as_cancel.evidence(),
                "editor_after_overwrite_cancel": editor_after_overwrite_cancel.evidence(),
                "source_before": before.evidence(),
                "source_after_cancel": after_cancel.evidence(),
                "save_as_cancel_destination_before": cancelled_before.evidence(),
                "save_as_cancel_destination_after": cancelled_after.evidence(),
                "saved_destination": saved.evidence(),
                "save_as_cancelled": True,
                "save_as_cancel_focus_preserved": True,
                "overwrite_cancelled": True,
                "overwrite_cancel_focus_preserved": True,
                "overwrite_confirmed": True,
                "flow": CASE_FLOWS[CASE_SAVE_CANCEL_OVERWRITE],
                "process_context": app.security_context.evidence(),
                "foreground_verified": True,
            }
        finally:
            self.reap(app)
        observations["runtime_scan"] = scan_case_artifacts(case_root)
        return observations

    def scenario_sample(self) -> dict[str, Any]:
        case_root, data, config, workspace, stderr = self.profile(CASE_SAMPLE)
        app = self.launch_target(None, data, config, workspace, stderr)
        try:
            sample = self.welcome_control(app, WELCOME_OPEN_SAMPLE_AUTOMATION_ID)
            self.click_control(sample, "WELCOME_SAMPLE_CLICK_FAILED")
            self.wait_welcome_absent(app)
            self.close_app(app)
            observations = {
                "sample_workspace_opened": True,
                "flow": CASE_FLOWS[CASE_SAMPLE],
                "process_context": app.security_context.evidence(),
                "foreground_verified": True,
            }
        finally:
            self.reap(app)
        observations["runtime_scan"] = scan_case_artifacts(case_root)
        return observations

    def scenario_recents(self) -> dict[str, Any]:
        case_root, data, config, workspace, stderr = self.profile(CASE_RECENTS)
        documents = []
        for index in range(11):
            path = (workspace / f"recent-{index:02}.md").resolve()
            BASE["write_durable"](path, f"recent {index}\n".encode("utf-8"))
            documents.append(path)
        BASE["write_durable"](
            config / "settings.toml", recent_settings_document(documents)
        )
        app = self.launch_target(None, data, config, workspace, stderr)
        try:
            self.welcome_control(app, WELCOME_NEW_AUTOMATION_ID)

            def ten_initial_recent_controls() -> list[Any] | None:
                controls = self.recent_controls(app)
                return controls if len(controls) == 10 else None

            wait_until(
                ten_initial_recent_controls,
                self.ui_timeout,
                "INITIAL_RECENT_TARGET_COUNT_TIMEOUT",
            )
            first_context = app.security_context
            self.close_app(app)
        finally:
            self.reap(app)
        documents[1].unlink()
        restarted = self.launch_target(None, data, config, workspace, stderr)
        try:
            self.welcome_control(restarted, WELCOME_NEW_AUTOMATION_ID)
            def ten_recent_controls() -> list[Any] | None:
                controls = self.recent_controls(restarted)
                return controls if len(controls) == 10 else None

            controls = wait_until(
                ten_recent_controls,
                self.ui_timeout,
                "RECENT_TARGET_COUNT_TIMEOUT",
            )
            stale_name = documents[1].name
            stale_matches = [
                control
                for control in controls
                if stale_name in (control.element_info.name or "")
            ]
            if len(stale_matches) != 1:
                raise HarnessFailure("STALE_RECENT_UIA_CONTRACT_MISMATCH")
            stale = stale_matches[0]
            self.require_recent_status(restarted, "Missing", "STALE_RECENT_STATUS_MISSING")
            if stale.is_enabled():
                raise HarnessFailure("STALE_RECENT_ENABLED")
            self.welcome_control(restarted, WELCOME_NEW_AUTOMATION_ID)
            if not self.editor_absent_while_running(restarted):
                raise HarnessFailure("STALE_RECENT_OPENED_DOCUMENT")
            self.close_app(restarted)
            observations = {
                "recent_restart_visible": True,
                "recent_count": len(controls),
                "stale_recent_disabled": True,
                "flow": CASE_FLOWS[CASE_RECENTS],
                "process_context": first_context.evidence(),
                "foreground_verified": True,
            }
        finally:
            self.reap(restarted)
        observations["runtime_scan"] = scan_case_artifacts(case_root)
        return observations

    def scenario_cli(self) -> dict[str, Any]:
        case_root, data, config, workspace, stderr = self.profile(CASE_CLI)
        document = (workspace / "explicit.md").resolve()
        BASE["write_durable"](document, SAVE_TEXT.encode("utf-8"))
        file_app = self.launch_target(document, data, config, workspace, stderr)
        try:
            self.wait_welcome_absent(file_app)
            self.source_editor_ready(file_app)
            if self.editor_fingerprint(file_app) != fingerprint_text(SAVE_TEXT):
                raise HarnessFailure("EXPLICIT_FILE_TEXT_MISMATCH")
            self.close_app(file_app)
            directory = (workspace / "folder").resolve()
            directory.mkdir()
            BASE["write_durable"](directory / "readme.md", b"folder\n")
            directory_app = self.launch_target(directory, data, config, workspace, stderr)
            try:
                self.wait_welcome_absent(directory_app)
                self.close_app(directory_app)
            finally:
                self.reap(directory_app)
            observations = {
                "direct_file_bypassed_welcome": True,
                "direct_directory_bypassed_welcome": True,
                "flow": CASE_FLOWS[CASE_CLI],
                "process_context": file_app.security_context.evidence(),
                "foreground_verified": True,
            }
        finally:
            self.reap(file_app)
        observations["runtime_scan"] = scan_case_artifacts(case_root)
        return observations


def run(args: argparse.Namespace) -> tuple[int, dict[str, Any], str]:
    evidence = new_evidence(args.expect_exe_sha256)
    if failure := source_contract_failure():
        mark_remaining_cases(evidence, 0, "NOT_RUN", failure)
        complete_evidence(evidence, "FAIL")
        return 1, evidence, failure
    try:
        exe = args.exe.resolve(strict=False)
        win32, parent_context = preflight_goal_02(exe, args.expect_exe_sha256, evidence)
        application_class, uia_element_info_class, uia_wrapper_class, iuia_class, no_pattern_error_class = load_pywinauto()
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

    with tempfile.TemporaryDirectory(prefix="markturbo-goal-03-native-", ignore_cleanup_errors=True) as temporary:
        root = Path(temporary).resolve()
        try:
            bin_root = root / "bin"
            bin_root.mkdir()
            copied_exe = Path(shutil.copy2(exe, bin_root / exe.name)).resolve()
            copied = sha256_file(copied_exe)
            evidence["executable"]["copied_sha256"] = copied.sha256
            evidence["executable"]["copy_hash_verified"] = copied.sha256 == args.expect_exe_sha256
            if not evidence["executable"]["copy_hash_verified"]:
                raise HarnessFailure("COPIED_EXECUTABLE_HASH_MISMATCH")
            source_sample = REPO / "sample"
            if not source_sample.is_dir():
                raise HarnessFailure("SAMPLE_FIXTURE_MISSING")
            shutil.copytree(source_sample, bin_root / "sample")
        except (HarnessFailure, OSError) as error:
            code = error.code if isinstance(error, HarnessFailure) else "ISOLATION_COPY_FAILED"
            mark_remaining_cases(evidence, 0, "NOT_RUN", code)
            complete_evidence(evidence, "FAIL")
            return 1, evidence, code

        harness = Goal03Harness(
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
            harness.scenario_welcome,
            harness.scenario_new_paste,
            harness.scenario_save_create,
            harness.scenario_save_cancel_overwrite,
            harness.scenario_sample,
            harness.scenario_recents,
            harness.scenario_cli,
        )
        returncode, code = 1, "INTERNAL_NO_RESULT"
        current = 0
        try:
            for current, scenario in enumerate(scenarios):
                case = evidence["cases"][current]
                started = time.perf_counter()
                try:
                    case["observations"] = scenario()
                except HarnessBlocked as error:
                    case.update(status="BLOCKED", reason_code=error.code, duration_ms=round((time.perf_counter() - started) * 1000, 3))
                    mark_remaining_cases(evidence, current + 1, "BLOCKED", "SKIPPED_AFTER_BLOCKED")
                    returncode, code = 2, error.code
                    break
                except HarnessFailure as error:
                    case.update(status="FAIL", reason_code=error.code, failure_type=safe_failure_type(error), duration_ms=round((time.perf_counter() - started) * 1000, 3))
                    mark_remaining_cases(evidence, current + 1, "NOT_RUN", "SKIPPED_AFTER_FAILURE")
                    returncode, code = 1, error.code
                    break
                except Exception as error:
                    case.update(status="FAIL", reason_code="INTERNAL_FAILURE", failure_type=safe_exception_name(error), duration_ms=round((time.perf_counter() - started) * 1000, 3))
                    mark_remaining_cases(evidence, current + 1, "NOT_RUN", "SKIPPED_AFTER_FAILURE")
                    returncode, code = 1, "INTERNAL_FAILURE"
                    break
                case.update(status="PASS", duration_ms=round((time.perf_counter() - started) * 1000, 3))
            else:
                returncode, code = 0, "ALL_REQUIRED_CASES"
        finally:
            try:
                harness.cleanup()
            except HarnessFailure as error:
                returncode, code = 1, error.code
                case = evidence["cases"][current]
                case.update(status="FAIL", reason_code=error.code, failure_type=safe_failure_type(error), duration_ms=case["duration_ms"] or 0.0)
        complete_evidence(evidence, {0: "PASS", 1: "FAIL", 2: "BLOCKED"}[returncode])
        return returncode, evidence, code


def parse_args(argv: list[str] | None = None) -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--exe", type=Path, default=DEFAULT_EXE)
    parser.add_argument("--expect-exe-sha256", type=normalize_expected_hash, required=True)
    parser.add_argument("--evidence", type=Path, default=DEFAULT_EVIDENCE)
    parser.add_argument("--ui-timeout", type=float, default=15.0)
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
