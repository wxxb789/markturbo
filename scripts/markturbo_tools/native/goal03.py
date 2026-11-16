"""Exercise Goal 03's first-use document workflow through a real Windows UI.

The harness is fail-closed. A PASS binds every observation to the supplied
x64 executable SHA-256 and uses isolated MARKTURBO_DATA_DIR and
MARKTURBO_CONFIG_DIR roots. Evidence contains fingerprints and booleans, never
document text or filesystem paths.

It drives the nsh welcome (`ui/welcome.rs`): Open folder carries its own
AutomationId; New file, Paste and Open sample are anonymous Buttons named by
their labels; the recent rows share the `welcome-recent` id. Save As runs from
the title menu or Ctrl+Shift+S through the native IFileSaveDialog, whose own
overwrite confirmation is the only overwrite decision. Editor text is read with
the editor's Select All + Copy (see `runtime.EDITOR_NAME`).
"""

from __future__ import annotations

import argparse
import hashlib
import json
import math
import re
import tomllib
from pathlib import Path
from typing import Any

from .source_contract import production_markers_present
from .runtime import (
    IMAGE_FILE_MACHINE_AMD64,
    INTEGRITY_NAMES,
    PE32_PLUS_MAGIC,
    REASON_CODE_RE,
    SAFE_FAILURE_TYPES,
    SHA256_RE,
    EDITOR_NAME,
    MENU_SAVE_AS_AUTOMATION_ID,
    VK_A,
    VK_CONTROL,
    VK_RETURN,
    VK_S,
    VK_V,
    VK_W,
    Fingerprint,
    HarnessBlocked,
    HarnessFailure,
    NativeHarness as BaseNativeHarness,
    NativeRunPlan,
    artifact_contains,
    complete_evidence as complete_evidence_envelope,
    finite_nonnegative,
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
    validate_fingerprint,
    validate_environment as validate_runtime_environment,
    validate_process_context,
    wait_until,
    write_durable,
)


REPO = Path(__file__).resolve().parents[3]
DEFAULT_EXE = REPO / "target" / "release" / "markturbo.exe"
DEFAULT_EVIDENCE = REPO / "target" / "native-acceptance" / "goal-03-native-acceptance-v1.json"

SCHEMA = "markturbo.goal-03-native-acceptance"
SCHEMA_VERSION = 1
VK_SHIFT = 0x10
VK_ESCAPE = 0x1B

WELCOME_OPEN_FOLDER_AUTOMATION_ID = "welcome-open-folder"
WELCOME_NEW_NAME = "New file"
WELCOME_PASTE_NAME = "Paste"
WELCOME_SAMPLE_NAME = "Open sample"
WELCOME_RECENT_AUTOMATION_ID = "welcome-recent"
# What a stale recent row must say when it is not simply disabled.
STALE_RECENT_MARKER = "Missing"
UI_SOURCE_REQUIREMENTS = {
    "crates/mt-app/src/ui/welcome.rs": (
        '"welcome-open-folder"',
        '"New file"',
        '"Paste"',
        '"Open sample"',
        '"welcome-recent"',
    ),
    "crates/mt-app/src/ui/titlebar.rs": (
        '"title-menu-save-as"',
        'on_key("shift+mod+s"',
    ),
    "crates/mt-app/src/ui/rail.rs": ('on_key("mod+w"',),
    "crates/mt-app/src/ui/notices.rs": ('"Close without saving?"', '"Don\'t save"'),
    "crates/mt-app/src/ui/document.rs": ('"Markdown source"',),
    "crates/mt-app/src/state.rs": ("pick_save_path(",),
}

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
    CASE_WELCOME: "no-argument welcome -> four entry points -> restart welcome",
    CASE_NEW_PASTE: "welcome New file and Paste -> editable Unicode buffers",
    CASE_SAVE_CREATE: "new buffer -> menu Save As create -> direct reopen exact text",
    CASE_SAVE_CANCEL_OVERWRITE: (
        "Save As cancel -> native overwrite No -> native overwrite Yes"
    ),
    CASE_SAMPLE: "welcome Open sample -> sample workspace replaces welcome",
    CASE_RECENTS: (
        "eleven persisted targets -> reopen records bound of ten -> restart stale degraded"
    ),
    CASE_CLI: "explicit file and directory arguments bypass welcome",
}

DOCUMENT_SENTINEL = "MTG03-NATIVE-SENTINEL-\u4fdd\u5b58-\U0001f680"
NEW_TEXT = f"# {DOCUMENT_SENTINEL}: New\n\n\u4e2d\u6587 and emoji \U0001f680\n"
PASTE_TEXT = f"# {DOCUMENT_SENTINEL}: Paste\n\n\u65e5\u672c\u8a9e and emoji \U0001f9ea\n"
SAVE_TEXT = f"# {DOCUMENT_SENTINEL}: Save As\n\nExact CJK \u4f60\u597d and emoji \U0001f680\n"
OVERWRITE_TEXT = f"# {DOCUMENT_SENTINEL}: Replace\n\nExact CJK \u4fdd\u5b58 and emoji \U0001f9ea\n"
EXISTING_DESTINATION = b"goal-03 original destination\n"
CANCELLED_DESTINATION = b"goal-03 cancelled destination\n"

SAFE_STRINGS = frozenset(CASE_FLOWS.values()) | INTEGRITY_NAMES
SAMPLE_VERSION_RE = re.compile(r"^[0-9a-f]{24}$")
ALLOWED_OBSERVATION_KEYS = {
    "app_logs_scanned",
    "byte_count",
    "config_files_scanned",
    "direct_directory_bypassed_welcome",
    "direct_file_bypassed_welcome",
    "editor_after_overwrite_cancel",
    "editor_after_save_as_cancel",
    "editor_before_cancellation",
    "entry_points_visible",
    "files_scanned",
    "flow",
    "foreground_attempts",
    "foreground_diagnostics",
    "foreground_hwnd",
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
    "recent_visible_count",
    "requested_hwnd",
    "reopened_editor",
    "runtime_scan",
    "sample_workspace_opened",
    "sample_content",
    "sample_file_count",
    "sample_manifest",
    "sample_version",
    "save_as_cancelled",
    "save_as_cancel_destination_after",
    "save_as_cancel_destination_before",
    "save_as_cancel_focus_preserved",
    "save_as_created",
    "saved_destination",
    "session_id",
    "set_foreground_return",
    "show_window_return",
    "bring_to_top_return",
    "sha256",
    "source_after_cancel",
    "source_before",
    "stale_recent_disabled",
    "overwrite_cancel_focus_preserved",
    "utf16le_sentinel_absent",
    "utf8_sentinel_absent",
    "welcome_restart_visible",
    "welcome_visible",
}


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
    complete_evidence_envelope(evidence, status, REQUIRED_CASE_IDS)


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
        if key == "sample_version" and SAMPLE_VERSION_RE.fullmatch(value):
            return
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
    validate_runtime_environment(value)
    return validate_process_context(value.get("harness_process"))


def required_observations(case_id: str) -> set[str]:
    common = {"flow", "process_context", "foreground_verified", "runtime_scan"}
    specifics = {
        CASE_WELCOME: {
            "welcome_visible",
            "entry_points_visible",
            "welcome_restart_visible",
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
        CASE_SAMPLE: {
            "sample_workspace_opened",
            "sample_file_count",
            "sample_manifest",
            "sample_content",
            "sample_version",
        },
        CASE_RECENTS: {
            "recent_restart_visible",
            "recent_count",
            "recent_visible_count",
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
        require_true(
            observations, "welcome_visible", "entry_points_visible", "welcome_restart_visible"
        )
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
        file_count = observations.get("sample_file_count")
        if not isinstance(file_count, int) or isinstance(file_count, bool) or file_count <= 0:
            raise ValueError("sample file inventory must be nonempty")
        manifest = validate_fingerprint(observations.get("sample_manifest"))
        content = validate_fingerprint(observations.get("sample_content"))
        if manifest["byte_count"] <= 0 or content["byte_count"] <= 0:
            raise ValueError("sample fingerprints must be nonempty")
        version = observations.get("sample_version")
        if not isinstance(version, str) or not SAMPLE_VERSION_RE.fullmatch(version):
            raise ValueError("sample version must be a 24-character SHA-256 prefix")
        if content["sha256"][:24] != version:
            raise ValueError("sample version does not match the materialized content")
    elif case_id == CASE_RECENTS:
        require_true(observations, "recent_restart_visible", "stale_recent_disabled")
        if observations.get("recent_count") != 10:
            raise ValueError("recent target bound was not exactly ten")
        visible = observations.get("recent_visible_count")
        if not isinstance(visible, int) or isinstance(visible, bool) or not 1 <= visible <= 10:
            raise ValueError("recent rows must show between one and ten targets")
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
    expected_summary = {
        "required_case_count": len(REQUIRED_CASE_IDS),
        "passed_case_count": sum(case["status"] == "PASS" for case in cases),
        "blocked_case_count": sum(case["status"] == "BLOCKED" for case in cases),
        "failed_case_count": sum(case["status"] == "FAIL" for case in cases),
        "not_run_case_count": sum(case["status"] == "NOT_RUN" for case in cases),
    }
    if summary != expected_summary:
        raise ValueError("case summary does not match case evidence")


def source_contract_failure() -> str | None:
    """The nsh selectors this harness drives must exist in production source."""
    if not production_markers_present(REPO, UI_SOURCE_REQUIREMENTS):
        return "FIRST_USE_SOURCE_CONTRACT_MISSING"
    return None


def persisted_recent_count(config_root: Path) -> int:
    """How many recent targets the settings file holds (paths are not kept)."""
    try:
        settings = tomllib.loads((config_root / "settings.toml").read_text(encoding="utf-8"))
    except (OSError, tomllib.TOMLDecodeError) as error:
        raise HarnessFailure("RECENT_SETTINGS_UNREADABLE", safe_exception_name(error)) from None
    targets = settings.get("recent-targets", [])
    if not isinstance(targets, list):
        raise HarnessFailure("RECENT_SETTINGS_UNREADABLE")
    return len(targets)


def sample_inventory(root: Path) -> tuple[int, Fingerprint, Fingerprint]:
    """Fingerprint a sample tree without retaining paths or file contents."""
    files = sorted(path for path in root.rglob("*") if path.is_file())
    manifest = hashlib.sha256()
    content = hashlib.sha256()
    content_byte_count = 0
    for path in files:
        relative = path.relative_to(root).as_posix().encode("utf-8")
        manifest.update(relative)
        manifest.update(b"\0")
        content.update(relative)
        content.update(b"\0")
        with path.open("rb") as handle:
            while chunk := handle.read(1024 * 1024):
                content_byte_count += len(chunk)
                content.update(chunk)
        content.update(b"\0")
    manifest_byte_count = sum(
        len(path.relative_to(root).as_posix().encode("utf-8")) + 1 for path in files
    )
    return (
        len(files),
        Fingerprint(manifest_byte_count, manifest.hexdigest()),
        Fingerprint(content_byte_count, content.hexdigest()),
    )


def materialized_sample_inventory(data_root: Path) -> dict[str, Any]:
    try:
        versions = sorted(path for path in (data_root / "sample").iterdir() if path.is_dir())
    except OSError as error:
        raise HarnessFailure("SAMPLE_MATERIALIZATION_MISSING", safe_exception_name(error)) from None
    if len(versions) != 1:
        raise HarnessFailure("SAMPLE_MATERIALIZATION_INCOMPLETE")
    try:
        file_count, manifest, content = sample_inventory(versions[0])
    except OSError as error:
        raise HarnessFailure("SAMPLE_MATERIALIZATION_SCAN_FAILED", safe_exception_name(error)) from None
    if (
        file_count <= 0
        or manifest.byte_count <= 0
        or content.byte_count <= 0
        or versions[0].name != content.sha256[:24]
    ):
        raise HarnessFailure("SAMPLE_MATERIALIZATION_INCOMPLETE")
    return {
        "sample_file_count": file_count,
        "sample_manifest": manifest.evidence(),
        "sample_content": content.evidence(),
        "sample_version": versions[0].name,
    }


def scan_case_artifacts(case_root: Path) -> dict[str, Any]:
    try:
        roots = (case_root / "data", case_root / "config")
        paths = [
            path
            for root in roots
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
            leaked = artifact_contains(path, (utf8, utf16))
        except OSError as error:
            raise HarnessFailure("RUNTIME_ARTIFACT_SCAN_FAILED", safe_exception_name(error)) from None
        if leaked == utf8:
            raise HarnessFailure("UTF8_DOCUMENT_SENTINEL_LEAKED")
        if leaked == utf16:
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

    def welcome_open_folder(self, app: Any) -> Any:
        return self.find_control(
            app,
            WELCOME_OPEN_FOLDER_AUTOMATION_ID,
            "Button",
            "WELCOME_UIA_TIMEOUT",
            "WELCOME_UIA_CONTRACT_MISMATCH",
        )

    def welcome_action(self, app: Any, name: str) -> Any:
        return self.find_named(
            app, name, "Button", "WELCOME_UIA_TIMEOUT", "WELCOME_UIA_CONTRACT_MISMATCH"
        )

    def welcome_entry_points(self, app: Any) -> None:
        self.welcome_open_folder(app)
        for name in (WELCOME_NEW_NAME, WELCOME_PASTE_NAME, WELCOME_SAMPLE_NAME):
            self.welcome_action(app, name)

    def wait_welcome_absent(self, app: Any) -> None:
        wait_until(
            lambda: self.control_absent(
                app,
                WELCOME_OPEN_FOLDER_AUTOMATION_ID,
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
            editor = self.control_by_name(
                app.hwnd, EDITOR_NAME, "Edit", "SOURCE_EDITOR_UIA_CONTRACT_MISMATCH"
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
        finally:
            self.write_text_clipboard(clipboard_before)
        expected = fingerprint_text(value)
        actual, _ = self.wait_editor_fingerprint(app, expected, self.ui_timeout)
        return actual

    def discard_memory_document(self, app: Any) -> None:
        self.focus_editor(app)
        self.win32.send_shortcut(app.hwnd, VK_W)
        self.click_lifecycle_decision(app, "Discard")
        wait_until(
            lambda: self.editor_absent_while_running(app),
            self.ui_timeout,
            "MEMORY_DISCARD_TIMEOUT",
        )
        self.close_app(app)

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

    def native_overwrite_confirmation(self, app: Any, file_dialog_hwnd: int) -> dict[str, Any]:
        """The IFileSaveDialog's own overwrite prompt: Yes and No."""

        def locate() -> dict[str, Any] | None:
            nested = self.win32.owned_task_dialogs(app.process.pid, file_dialog_hwnd)
            if len(nested) > 1:
                raise HarnessFailure("MULTIPLE_NATIVE_OVERWRITE_DIALOGS")
            if not nested:
                if file_dialog_hwnd not in self.win32.owned_task_dialogs(
                    app.process.pid, app.hwnd
                ):
                    raise HarnessFailure("NATIVE_OVERWRITE_CONFIRMATION_MISSING")
                return None
            buttons = {}
            for choice, automation_id in (("Yes", "CommandButton_6"), ("No", "CommandButton_7")):
                button = self.control_by_id(
                    nested[0],
                    automation_id,
                    "Button",
                    "NATIVE_OVERWRITE_DIALOG_UIA_CONTRACT_MISMATCH",
                    expected_class_name="CCPushButton",
                )
                if button is None:
                    return None
                buttons[choice] = button
            return buttons

        return wait_until(
            locate, self.ui_timeout, "NATIVE_OVERWRITE_DIALOG_TIMEOUT", interval=0.05
        )

    def type_save_path(self, app: Any, path: Path) -> int:
        hwnd, edit = self.common_file_dialog(app)
        self.click_control(edit, "SAVE_AS_FILENAME_FOCUS_FAILED")
        self.win32.send_shortcut(hwnd, VK_A)
        self.win32.send_unicode(hwnd, str(path))
        self.win32.send_key(hwnd, VK_RETURN)
        return hwnd

    def wait_file_dialog_closed(self, app: Any, hwnd: int, code: str) -> None:
        wait_until(
            lambda: hwnd not in self.win32.owned_task_dialogs(app.process.pid, app.hwnd),
            self.ui_timeout,
            code,
            interval=0.05,
        )
        self.win32.require_foreground(app.hwnd, self.ui_timeout)

    def select_save_path(self, app: Any, path: Path) -> None:
        """Save As to a path that does not exist yet."""
        hwnd = self.type_save_path(app, path)
        self.wait_file_dialog_closed(app, hwnd, "SAVE_AS_FILE_DIALOG_CLOSE_TIMEOUT")

    def select_existing_save_path(self, app: Any, path: Path, *, replace: bool) -> None:
        """Save As onto an existing file: answer the native overwrite prompt."""
        hwnd = self.type_save_path(app, path)
        choice = "Yes" if replace else "No"
        button = self.native_overwrite_confirmation(app, hwnd)[choice]
        self.click_control(button, f"NATIVE_OVERWRITE_{choice.upper()}_CLICK_FAILED")
        if not replace:
            # No returns to the picker; leaving it is the user's cancel.
            self.common_file_dialog(app)
            self.win32.send_key(hwnd, VK_ESCAPE)
        self.wait_file_dialog_closed(app, hwnd, "SAVE_AS_FILE_DIALOG_CLOSE_TIMEOUT")

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
        self.menu_command(app, MENU_SAVE_AS_AUTOMATION_ID)

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

    def recent_controls(self, app: Any) -> list[Any]:
        try:
            uia = self.iuia_class()
            condition = uia.iuia.CreatePropertyCondition(
                uia.UIA_dll.UIA_AutomationIdPropertyId, WELCOME_RECENT_AUTOMATION_ID
            )
            elements = self.fresh_uia_root(app.hwnd).element.FindAll(
                uia.tree_scope["descendants"], condition
            )
            controls = []
            expected_control_type = uia.known_control_types["Button"]
            for index in range(elements.Length):
                element = elements.GetElement(index)
                if (
                    element.CurrentAutomationId != WELCOME_RECENT_AUTOMATION_ID
                    or element.CurrentControlType != expected_control_type
                ):
                    raise HarnessFailure("RECENT_UIA_CONTRACT_MISMATCH")
                control = self.uia_wrapper_class(self.uia_element_info_class(element))
                info = control.element_info
                if (
                    info.automation_id != WELCOME_RECENT_AUTOMATION_ID
                    or info.control_type != "Button"
                    or not control.is_visible()
                ):
                    raise HarnessFailure("RECENT_UIA_CONTRACT_MISMATCH")
                controls.append(control)
            return controls
        except HarnessFailure:
            raise
        except Exception as error:
            raise HarnessFailure("RECENT_UIA_QUERY_FAILED", safe_exception_name(error)) from None

    def scenario_welcome(self) -> dict[str, Any]:
        case_root, data, config, workspace, stderr = self.profile(CASE_WELCOME)
        first = self.launch_app(None, data, config, workspace, stderr)
        try:
            self.welcome_entry_points(first)
            self.close_app(first)
            second = self.launch_app(None, data, config, workspace, stderr)
            try:
                self.welcome_entry_points(second)
                self.close_app(second)
            finally:
                self.reap(second)
            observations = {
                "welcome_visible": True,
                "entry_points_visible": True,
                "welcome_restart_visible": True,
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
        new = self.launch_app(None, data, config, workspace, stderr)
        try:
            self.click_control(self.welcome_action(new, WELCOME_NEW_NAME), "WELCOME_NEW_CLICK_FAILED")
            self.wait_welcome_absent(new)
            self.source_editor_ready(new)
            new_text = self.replace_editor(new, NEW_TEXT)
            self.discard_memory_document(new)
            paste = self.launch_app(None, data, config, workspace, stderr)
            try:
                clipboard_before = self.read_text_clipboard()
                try:
                    self.write_text_clipboard(PASTE_TEXT)
                    clipboard_text = self.read_text_clipboard()
                    if clipboard_text is None:
                        raise HarnessBlocked("CLIPBOARD_TEXT_READ_FAILED")
                    clipboard_fingerprint = fingerprint_text(clipboard_text)
                    self.click_control(
                        self.welcome_action(paste, WELCOME_PASTE_NAME), "WELCOME_PASTE_CLICK_FAILED"
                    )
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
        first = self.launch_app(None, data, config, workspace, stderr)
        try:
            self.click_control(self.welcome_action(first, WELCOME_NEW_NAME), "WELCOME_NEW_CLICK_FAILED")
            self.source_editor_ready(first)
            editor = self.replace_editor(first, SAVE_TEXT)
            self.request_save_as(first)
            self.select_save_path(first, destination)
            saved = self.wait_file(destination, editor, "SAVE_AS_CREATE_TIMEOUT")
            self.close_app(first)
            reopened = self.launch_app(destination, data, config, workspace, stderr)
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
        write_durable(cancelled_destination, CANCELLED_DESTINATION)
        write_durable(destination, EXISTING_DESTINATION)
        cancelled_before = sha256_file(cancelled_destination)
        before = sha256_file(destination)
        app = self.launch_app(None, data, config, workspace, stderr)
        try:
            self.click_control(self.welcome_action(app, WELCOME_NEW_NAME), "WELCOME_NEW_CLICK_FAILED")
            self.source_editor_ready(app)
            editor = self.replace_editor(app, OVERWRITE_TEXT)
            self.focus_editor(app)
            self.request_save_as_shortcut(app)
            self.cancel_save_picker(app, cancelled_destination)
            self.require_source_editor_focus(app, "SAVE_AS_PICKER_CANCEL_FOCUS_CHANGED")
            cancelled_after = sha256_file(cancelled_destination)
            editor_after_save_as_cancel = self.editor_fingerprint(app)
            if cancelled_after != cancelled_before or editor_after_save_as_cancel != editor:
                raise HarnessFailure("SAVE_AS_PICKER_CANCEL_CHANGED_STATE")
            self.request_save_as_shortcut(app)
            self.select_existing_save_path(app, destination, replace=False)
            self.require_source_editor_focus(app, "OVERWRITE_CANCEL_FOCUS_CHANGED")
            after_cancel = sha256_file(destination)
            editor_after_overwrite_cancel = self.editor_fingerprint(app)
            if after_cancel != before or editor_after_overwrite_cancel != editor:
                raise HarnessFailure("OVERWRITE_CANCEL_CHANGED_STATE")
            self.request_save_as_shortcut(app)
            self.select_existing_save_path(app, destination, replace=True)
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
        app = self.launch_app(None, data, config, workspace, stderr)
        try:
            sample = self.welcome_action(app, WELCOME_SAMPLE_NAME)
            self.click_control(sample, "WELCOME_SAMPLE_CLICK_FAILED")
            self.wait_welcome_absent(app)
            materialized = materialized_sample_inventory(data)
            self.close_app(app)
            observations = {
                "sample_workspace_opened": True,
                **materialized,
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
            write_durable(path, f"recent {index}\n".encode("utf-8"))
            documents.append(path)
        write_durable(
            config / "settings.toml", recent_settings_document(documents)
        )
        app = self.launch_app(None, data, config, workspace, stderr)
        try:
            self.welcome_open_folder(app)
            controls = wait_until(
                lambda: self.recent_controls(app),
                self.ui_timeout,
                "INITIAL_RECENT_TARGETS_TIMEOUT",
            )
            # Reopening a target records it: the persisted history is bounded.
            self.click_control(controls[0], "RECENT_TARGET_CLICK_FAILED")
            self.source_editor_ready(app)
            first_context = app.security_context
            self.close_app(app)
        finally:
            self.reap(app)
        recent_count = persisted_recent_count(config)
        if recent_count != 10:
            raise HarnessFailure("RECENT_TARGET_BOUND_NOT_TEN")
        documents[1].unlink()
        restarted = self.launch_app(None, data, config, workspace, stderr)
        try:
            self.welcome_open_folder(restarted)
            controls = wait_until(
                lambda: self.recent_controls(restarted),
                self.ui_timeout,
                "RECENT_TARGETS_TIMEOUT",
            )
            if not 1 <= len(controls) <= 10:
                raise HarnessFailure("RECENT_TARGET_ROWS_OUT_OF_BOUND")
            stale_name = documents[1].name
            stale_matches = [
                control
                for control in controls
                if stale_name in (control.element_info.name or "")
            ]
            if len(stale_matches) != 1:
                raise HarnessFailure("STALE_RECENT_UIA_CONTRACT_MISMATCH")
            stale = stale_matches[0]
            if stale.is_enabled() and STALE_RECENT_MARKER not in (stale.element_info.name or ""):
                raise HarnessFailure("STALE_RECENT_NOT_DEGRADED")
            self.welcome_open_folder(restarted)
            if not self.editor_absent_while_running(restarted):
                raise HarnessFailure("STALE_RECENT_OPENED_DOCUMENT")
            self.close_app(restarted)
            observations = {
                "recent_restart_visible": True,
                "recent_count": recent_count,
                "recent_visible_count": len(controls),
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
        write_durable(document, SAVE_TEXT.encode("utf-8"))
        file_app = self.launch_app(document, data, config, workspace, stderr)
        try:
            self.wait_welcome_absent(file_app)
            self.source_editor_ready(file_app)
            if self.editor_fingerprint(file_app) != fingerprint_text(SAVE_TEXT):
                raise HarnessFailure("EXPLICIT_FILE_TEXT_MISMATCH")
            self.close_app(file_app)
            directory = (workspace / "folder").resolve()
            directory.mkdir()
            write_durable(directory / "readme.md", b"folder\n")
            directory_app = self.launch_app(directory, data, config, workspace, stderr)
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


def native_run_plan() -> NativeRunPlan:
    return NativeRunPlan(
        required_case_ids=REQUIRED_CASE_IDS,
        workdir_prefix="markturbo-goal-03-native-",
        new_evidence=new_evidence,
        validate_evidence=validate_evidence,
        source_contract=source_contract_failure,
        preflight=preflight,
        ui_types_loader=load_pywinauto,
        harness_factory=Goal03Harness,
        scenarios=lambda harness: (
            harness.scenario_welcome,
            harness.scenario_new_paste,
            harness.scenario_save_create,
            harness.scenario_save_cancel_overwrite,
            harness.scenario_sample,
            harness.scenario_recents,
            harness.scenario_cli,
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
        help="preserve the isolated data, config, logs, and stderr directory after a non-PASS run",
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
