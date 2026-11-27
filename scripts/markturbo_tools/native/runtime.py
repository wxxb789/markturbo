"""Shared Windows/UIA implementation for hash-bound native acceptance runs."""

from __future__ import annotations

import argparse
import contextlib
import contextvars
import copy
import ctypes
import ctypes.wintypes as wt
import datetime as dt
import hashlib
import json
import math
import ntpath
import os
import platform
import re
import shutil
import struct
import subprocess
import sys
import tempfile
import time
from collections.abc import Callable, Iterator
from dataclasses import dataclass
from pathlib import Path
from typing import Any



SHA256_RE = re.compile(r"^[0-9a-f]{64}$")
OUTCOME_RE = re.compile(r"^(PASS|FAIL|BLOCKED): ([A-Z0-9_]+)$")
REASON_CODE_RE = re.compile(r"^[A-Z0-9_]+$")
FAILURE_TYPE_RE = re.compile(r"^[A-Za-z_][A-Za-z0-9_]{0,127}$")
SAFE_FAILURE_TYPES = frozenset(
    {
        "AttributeError",
        "COMError",
        # Content-free byte-length relations of an editor read that never
        # matched (wait_editor_fingerprint).
        "EditorLonger",
        "EditorSameLength",
        "EditorShorter",
        "FileExistsError",
        # The same relations for a file that never held the expected bytes
        # (wait_file), or that could not be read at all.
        "FileLonger",
        "FileSameLength",
        "FileShorter",
        "FileUnreadable",
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
INTEGRITY_NAMES = {
    "untrusted",
    "low",
    "medium",
    "medium-plus",
    "high",
    "system",
    "protected",
}

# How the nsh 0.7.1 UI appears in Windows UI Automation. Verified against the
# nsh-platform-windows 0.7.1 crate source:
#
# * AutomationId is the view's `.test_id(..)` (access/uia.rs), but only on a
#   view that is itself a UIA element. Plain wrappers are looked through
#   (access.rs `control_for` and `children`). `prims::quiet_action`/
#   `primary_action` and the guard card's
#   actions are the pressable Button view itself, so each carries its own
#   AutomationId (for example `guard-save`, `welcome-new-file`) and is found by
#   it; anonymous name lookups do not see them.
# * Name is the accessibility label, else a text view's visible text, else the
#   folded content of a grouping control such as a Button (access.rs `name_of`).
# * ControlType follows the role, else Text for text views, Edit for inputs,
#   Image for labelled images, Button for pressable views, Group for labelled
#   or explicitly accessible views, and Pane for scrolls (access.rs).
# * Invoke is offered by views with a Press handler or an `activate` action,
#   never by toggles (access/uia.rs); it delivers one whole press, so it cannot
#   charge the consent card's hold (`ui/consent.rs` `send_button`). Value is
#   offered by inputs only; the source editor's input is its IME sink, not
#   the document (nsh-base's editor view and controller), so it is read through the
#   editor's own Select All + Copy, which writes the exact text
#   (system.rs `set_clipboard_text`). SetFocus supports inputs and focusable
#   views (access/uia.rs and access.rs).
EDITOR_NAME = "Markdown source"
MENU_BUTTON_AUTOMATION_ID = "title-settings"
MENU_SAVE_AUTOMATION_ID = "title-menu-save"
MENU_SAVE_AS_AUTOMATION_ID = "title-menu-save-as"
MENU_WEB_AUTOMATION_ID = "title-menu-web"
MENU_QUIT_AUTOMATION_ID = "title-menu-quit"
VIEW_BUTTON_AUTOMATION_ID = "title-view"
SIDEBAR_PANEL_AUTOMATION_ID = "sidebar-panel"
SIDEBAR_ROW_AUTOMATION_ID = "sidebar-file"
SIDEBAR_OUTSIDE_ROW_AUTOMATION_ID = "sidebar-outside-row"
RAIL_FILE_ROW_AUTOMATION_ID = "rail-file-row"
TAB_CLOSE_AUTOMATION_ID = "rail-close"
# The visual matrix drives both interface languages. Keep callers' view names
# independent of the window's locale, but require the full accessible label.
_VIEW_BUTTON_NAMES = {
    "View: Edit — switch to Split": "Edit",
    "View: Split — switch to Preview": "Split",
    "View: Preview — switch to Edit": "Preview",
    "视图：编辑 — 切换到分栏": "Edit",
    "视图：分栏 — 切换到预览": "Split",
    "视图：预览 — 切换到编辑": "Preview",
}
GUARD_HEADING = "Close without saving?"
GUARD_BUTTON_NAMES = {"Save": "Save", "Discard": "Don't save", "Cancel": "Cancel"}
GUARD_BUTTON_AUTOMATION_IDS = {
    "Save": "guard-save",
    "Discard": "guard-dont-save",
    "Cancel": "guard-cancel",
}
# The window-close and quit guard (`GuardKind::QuitApp`): the tab's card with
# the quit heading and Save label; the buttons keep their automation ids.
QUIT_GUARD_HEADING = "Quit without saving?"
QUIT_GUARD_BUTTON_NAMES = {"Save": "Save all", "Discard": "Don't save", "Cancel": "Cancel"}
QUIT_GUARD_BUTTON_AUTOMATION_IDS = dict(GUARD_BUTTON_AUTOMATION_IDS)
CONFLICT_HEADING = "Changed on disk"
LIFECYCLE_CLICK_FAILURE_CODES = {
    "Save": "LIFECYCLE_SAVE_CLICK_FAILED",
    "Discard": "LIFECYCLE_DISCARD_CLICK_FAILED",
    "Cancel": "LIFECYCLE_CANCEL_CLICK_FAILED",
}
# The app's own class (`nsh-platform-windows` `window.rs`), the WebView2 host
# popup pair, and the Windows dialog class. `window_facts` maps them to fixed
# categories: a class name is a free-form string and an image is a local path.
NSH_WINDOW_CLASS = "NshWindow"
WEBVIEW2_POPUP_CLASS = "Chrome_WidgetWin_1"
WEBVIEW2_POPUP_PROCESS_IMAGE = "msedgewebview2.exe"
MARKTURBO_PROCESS_IMAGE = "markturbo.exe"
WINDOW_FACT_CATEGORIES = frozenset({"webview2_popup", "dialog", "app_window", "unknown"})
PROCESS_IMAGE_FACT_CATEGORIES = frozenset({"markturbo", "msedgewebview2", "other"})
TASK_DIALOG_CLASS = "#32770"
TASK_DIALOG_BUTTON_CLASS = "CCPushButton"
CF_UNICODETEXT = 13
GMEM_MOVEABLE = 0x0002

WM_CLOSE = 0x0010
GW_OWNER = 4
# `GetClassNameW`'s documented maximum class name, and a capacity above every
# executable path's length.
WINDOW_CLASS_CAPACITY = 256
PROCESS_IMAGE_CAPACITY = 32768
SW_RESTORE = 9
VK_CONTROL = 0x11
VK_MENU = 0x12
VK_A = 0x41
VK_B = 0x42
VK_BACK = 0x08
VK_C = 0x43
VK_ESCAPE = 0x1B
VK_RETURN = 0x0D
VK_Z = 0x5A
VK_S = 0x53
VK_W = 0x57
KEYEVENTF_KEYUP = 0x0002
KEYEVENTF_UNICODE = 0x0004
INPUT_MOUSE = 0
INPUT_KEYBOARD = 1
MOUSEEVENTF_LEFTDOWN = 0x0002
MOUSEEVENTF_LEFTUP = 0x0004
PROCESS_QUERY_LIMITED_INFORMATION = 0x1000
PROCESS_TERMINATE = 0x0001
JOB_OBJECT_BASIC_PROCESS_ID_LIST = 3
ERROR_MORE_DATA = 234
TOKEN_QUERY = 0x0008
TOKEN_INTEGRITY_LEVEL = 25
DESKTOP_READOBJECTS = 0x0001
DESKTOP_SWITCHDESKTOP = 0x0100
UOI_NAME = 2
WTS_CONNECT_STATE = 8
WTS_ACTIVE = 0
IMAGE_FILE_MACHINE_AMD64 = 0x8664
PE32_PLUS_MAGIC = 0x20B
WINDOWS_LAUNCH_ERROR_CODES = {
    2: "PROCESS_LAUNCH_FILE_NOT_FOUND",
    3: "PROCESS_LAUNCH_PATH_NOT_FOUND",
    5: "PROCESS_LAUNCH_ACCESS_DENIED",
    193: "PROCESS_LAUNCH_BAD_EXE_FORMAT",
    740: "PROCESS_LAUNCH_ELEVATION_REQUIRED",
}
ISOLATION_CLEANUP_TIMEOUT = 5.0
ISOLATION_CLEANUP_RETRY_INTERVAL = 0.05
PROCESS_CLEANUP_TIMEOUT = 5.0
CREATE_SUSPENDED = 0x00000004
JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE = 0x00002000
JOB_OBJECT_MSG_ACTIVE_PROCESS_ZERO = 4
JOB_OBJECT_COMPLETION_KEY = 1
WAIT_TIMEOUT = 258
ES_CONTINUOUS = 0x80000000
ES_SYSTEM_REQUIRED = 0x00000001
ES_DISPLAY_REQUIRED = 0x00000002
# The one wait for the OS to hand the main window the foreground. Longer waits
# do not help: a refused activation stays refused until the session changes.
FOREGROUND_TIMEOUT_SECONDS = 3.0
# Frames a focus change takes to reach the window's keymap (`focus_editor`).
FOCUS_SETTLE_FRAMES = 2
# How long a run waits for the operator to give the session a foreground
# window back (`Win32.require_foreground`), and what it asks them.
FOREGROUND_RECOVERY_SECONDS = 30.0
FOREGROUND_RECOVERY_PROMPT = (
    "No window has the foreground: restore the Remote Desktop window to continue "
    f"(waiting up to {FOREGROUND_RECOVERY_SECONDS:.0f} s)."
)
# Where a launch clicks when Windows refuses it the foreground: in the title
# row's left padding (`ui/titlebar.rs`, `RAIL_W + COLUMN_GAP` = 84 px wide,
# `TITLE_ROW_H` = 32 px high), a drag region at any scale of at least 100 %,
# which holds no control. Physical pixels from the window's top-left corner.
TITLE_ROW_CLICK_POINT = (24, 16)
# The longest a step nested inside an outer wait (one editor copy inside an
# exact-bytes wait) may take, and never past the outer wait's deadline.
INNER_WAIT_SECONDS = 2.0
# How often a wait for a file's exact bytes hashes it.
FILE_POLL_SECONDS = 0.1


class GUITHREADINFO(ctypes.Structure):
    _fields_ = [
        ("cbSize", wt.DWORD), ("flags", wt.DWORD),
        ("hwndActive", wt.HWND), ("hwndFocus", wt.HWND),
        ("hwndCapture", wt.HWND), ("hwndMenuOwner", wt.HWND),
        ("hwndMoveSize", wt.HWND), ("hwndCaret", wt.HWND),
        ("rcCaret", wt.RECT),
    ]


class JOBOBJECT_BASIC_LIMIT_INFORMATION(ctypes.Structure):
    _fields_ = [
        ("PerProcessUserTimeLimit", ctypes.c_int64),
        ("PerJobUserTimeLimit", ctypes.c_int64),
        ("LimitFlags", wt.DWORD),
        ("MinimumWorkingSetSize", ctypes.c_size_t),
        ("MaximumWorkingSetSize", ctypes.c_size_t),
        ("ActiveProcessLimit", wt.DWORD),
        ("Affinity", ctypes.c_size_t),
        ("PriorityClass", wt.DWORD),
        ("SchedulingClass", wt.DWORD),
    ]


class IO_COUNTERS(ctypes.Structure):
    _fields_ = [(name, ctypes.c_uint64) for name in (
        "ReadOperationCount", "WriteOperationCount", "OtherOperationCount",
        "ReadTransferCount", "WriteTransferCount", "OtherTransferCount",
    )]


class JOBOBJECT_EXTENDED_LIMIT_INFORMATION(ctypes.Structure):
    _fields_ = [
        ("BasicLimitInformation", JOBOBJECT_BASIC_LIMIT_INFORMATION),
        ("IoInfo", IO_COUNTERS),
        ("ProcessMemoryLimit", ctypes.c_size_t),
        ("JobMemoryLimit", ctypes.c_size_t),
        ("PeakProcessMemoryUsed", ctypes.c_size_t),
        ("PeakJobMemoryUsed", ctypes.c_size_t),
    ]


class JOBOBJECT_ASSOCIATE_COMPLETION_PORT(ctypes.Structure):
    _fields_ = [("CompletionKey", wt.LPVOID), ("CompletionPort", wt.HANDLE)]


class JOBOBJECT_BASIC_PROCESS_ID_LIST(ctypes.Structure):
    _fields_ = [
        ("NumberOfAssignedProcesses", wt.DWORD),
        ("NumberOfProcessIdsInList", wt.DWORD),
        ("ProcessIdList", ctypes.c_size_t * 1),
    ]


class HarnessBlocked(RuntimeError):
    def __init__(
        self,
        code: str,
        detail: str = "",
        diagnostics: dict[str, int | bool] | None = None,
    ) -> None:
        super().__init__(code)
        self.code = code
        self.detail = detail
        self.diagnostics = diagnostics or {}


class HarnessFailure(RuntimeError):
    def __init__(
        self,
        code: str,
        detail: str = "",
        observations: dict[str, Any] | None = None,
    ) -> None:
        super().__init__(code)
        self.code = code
        self.detail = detail
        # Content-free facts for the failed case's evidence; the goal's own
        # contract validates their shape like any other observation.
        self.observations = observations or {}


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
    process: OwnedProcess
    window: Any
    hwnd: int
    spec: LaunchSpec
    security_context: SecurityContext
    app_log_path: Path


@dataclass(frozen=True)
class NativeRunPlan:
    """Goal-specific inputs for the shared native acceptance lifecycle."""

    required_case_ids: tuple[str, ...]
    workdir_prefix: str
    new_evidence: Callable[[str], dict[str, Any]]
    validate_evidence: Callable[[dict[str, Any]], None]
    preflight: Callable[[Path, str, dict[str, Any]], tuple["Win32", SecurityContext]]
    ui_types_loader: Callable[[], tuple[Any, ...]]
    harness_factory: Callable[..., "NativeHarness"]
    scenarios: Callable[["NativeHarness"], tuple[Callable[[], dict[str, Any]], ...]]
    finalize_evidence: Callable[[dict[str, Any]], None] | None = None


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


def elapsed_ms(started: float) -> float:
    """Milliseconds since `started` (a `time.perf_counter()` reading), rounded to microseconds."""
    return round((time.perf_counter() - started) * 1000, 3)


def utc_now() -> str:
    return dt.datetime.now(dt.timezone.utc).isoformat(timespec="milliseconds").replace(
        "+00:00", "Z"
    )


def complete_evidence(
    evidence: dict[str, Any], status: str, required_case_ids: tuple[str, ...]
) -> None:
    evidence["status"] = status
    evidence["completed_at_utc"] = utc_now()
    cases = evidence["cases"]
    evidence["summary"] = {
        "required_case_count": len(required_case_ids),
        "passed_case_count": sum(case["status"] == "PASS" for case in cases),
        "blocked_case_count": sum(case["status"] == "BLOCKED" for case in cases),
        "failed_case_count": sum(case["status"] == "FAIL" for case in cases),
        "not_run_case_count": sum(case["status"] == "NOT_RUN" for case in cases),
    }


def write_evidence(
    path: Path,
    evidence: dict[str, Any],
    validator: Callable[[dict[str, Any]], None],
) -> None:
    validator(evidence)
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


def fingerprint_bytes(value: bytes) -> Fingerprint:
    return Fingerprint(len(value), hashlib.sha256(value).hexdigest())


def fingerprint_text(value: str) -> Fingerprint:
    return fingerprint_bytes(value.encode("utf-8"))


GENERIC_READ = 0x80000000
FILE_SHARE_READ = 0x00000001
FILE_SHARE_WRITE = 0x00000002
FILE_SHARE_DELETE = 0x00000004
OPEN_EXISTING = 3
FILE_ATTRIBUTE_NORMAL = 0x00000080
INVALID_HANDLE_VALUE = ctypes.c_void_p(-1).value


def open_shared_for_read(path: Path) -> Any:
    """A binary reader of `path` that never blocks the app's own writes.

    Python's `open` withholds FILE_SHARE_DELETE, so while the harness reads a
    document the app's atomic save (`ReplaceFileW`, which renames over the
    path) fails with a sharing violation. This handle grants read, write and
    delete sharing; elsewhere than Windows `open` already shares them."""
    if sys.platform != "win32":
        return path.open("rb")
    import msvcrt

    kernel32 = ctypes.WinDLL("kernel32", use_last_error=True)
    kernel32.CreateFileW.argtypes = [
        wt.LPCWSTR, wt.DWORD, wt.DWORD, wt.LPVOID, wt.DWORD, wt.DWORD, wt.HANDLE,
    ]
    kernel32.CreateFileW.restype = wt.HANDLE
    kernel32.CloseHandle.argtypes = [wt.HANDLE]
    kernel32.CloseHandle.restype = wt.BOOL
    handle = kernel32.CreateFileW(
        str(path),
        GENERIC_READ,
        FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
        None,
        OPEN_EXISTING,
        FILE_ATTRIBUTE_NORMAL,
        None,
    )
    if handle is None or handle == INVALID_HANDLE_VALUE:
        error = ctypes.get_last_error()
        raise OSError(None, ctypes.FormatError(error).strip(), str(path), error)
    try:
        descriptor = msvcrt.open_osfhandle(handle, os.O_RDONLY | os.O_BINARY)
    except OSError:
        kernel32.CloseHandle(handle)
        raise
    return os.fdopen(descriptor, "rb")


def sha256_file(path: Path) -> Fingerprint:
    digest = hashlib.sha256()
    byte_count = 0
    with open_shared_for_read(path) as handle:
        while chunk := handle.read(1024 * 1024):
            byte_count += len(chunk)
            digest.update(chunk)
    return Fingerprint(byte_count, digest.hexdigest())


def artifact_contains(path: Path, patterns: tuple[bytes, ...]) -> bytes | None:
    """Find a sensitive byte sequence while keeping only a chunk boundary tail."""
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


def inspect_pe_sections(path: Path) -> list[dict[str, int | str]]:
    with path.open("rb") as handle:
        data = handle.read(1024 * 1024)
    inspect_pe_bytes(data)
    pe_offset = struct.unpack_from("<I", data, 0x3C)[0]
    section_count = struct.unpack_from("<H", data, pe_offset + 6)[0]
    optional_size = struct.unpack_from("<H", data, pe_offset + 20)[0]
    table = pe_offset + 24 + optional_size
    if section_count == 0 or table + section_count * 40 > len(data):
        raise ValueError("truncated PE section table")
    sections: list[dict[str, int | str]] = []
    for index in range(section_count):
        offset = table + index * 40
        name = data[offset : offset + 8].split(b"\0", 1)[0].decode("ascii")
        sections.append(
            {
                "name": name,
                "virtual_size": struct.unpack_from("<I", data, offset + 8)[0],
                "raw_size": struct.unpack_from("<I", data, offset + 16)[0],
                "characteristics": struct.unpack_from("<I", data, offset + 36)[0],
            }
        )
    return sections


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


def launch_failure_code(error: OSError) -> str:
    """Map Win32 launch errors without exposing exception text or paths."""
    for value in (getattr(error, "winerror", None), error.errno):
        if isinstance(value, int):
            if code := WINDOWS_LAUNCH_ERROR_CODES.get(value & 0xFFFF):
                return code
    return "PROCESS_LAUNCH_FAILED"


def build_launch_spec(
    copied_exe: Path,
    target: Path | None,
    data_root: Path,
    config_root: Path,
    workspace_root: Path,
    stderr_path: Path,
    base_env: dict[str, str] | None = None,
    ephemeral_openai_api_key: str | None = None,
) -> LaunchSpec:
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
    if ephemeral_openai_api_key is not None:
        if not ephemeral_openai_api_key:
            raise ValueError("ephemeral OpenAI API key must not be empty")
        env["OPENAI_API_KEY"] = ephemeral_openai_api_key
    return LaunchSpec(
        args=(str(copied_exe),) if target is None else (str(copied_exe), str(target)),
        cwd=str(workspace_root),
        env=env,
        stderr_path=stderr_path,
    )


def has_unicode_clipboard_text(formats: set[int]) -> bool:
    return CF_UNICODETEXT in formats


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


class OwnedProcess:
    """One suspended launch and its non-breakaway descendants, never a PID tree scan."""

    def __init__(self, win32: Win32, spec: LaunchSpec) -> None:
        self.win32 = win32
        self.spec = spec
        self.pid = 0
        self.returncode: int | None = None
        self._job = self._port = self._process = self._thread = None
        self._assigned = False
        self._closed = False
        self._cleanup_failure: HarnessFailure | None = None

    def start(self) -> None:
        kernel = self.win32.kernel32
        try:
            self._job = kernel.CreateJobObjectW(None, None)
            if not self._job:
                raise HarnessFailure("PROCESS_JOB_CREATE_FAILED")
            limits = JOBOBJECT_EXTENDED_LIMIT_INFORMATION()
            # Neither BREAKAWAY_OK nor SILENT_BREAKAWAY_OK is enabled.
            limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE
            if not kernel.SetInformationJobObject(
                self._job, 9, ctypes.byref(limits), ctypes.sizeof(limits)
            ):
                raise HarnessFailure("PROCESS_JOB_LIMIT_FAILED")
            self._port = kernel.CreateIoCompletionPort(wt.HANDLE(-1), None, 0, 1)
            if not self._port:
                raise HarnessFailure("PROCESS_JOB_PORT_CREATE_FAILED")
            association = JOBOBJECT_ASSOCIATE_COMPLETION_PORT(
                JOB_OBJECT_COMPLETION_KEY, self._port
            )
            if not kernel.SetInformationJobObject(
                self._job, 7, ctypes.byref(association), ctypes.sizeof(association)
            ):
                raise HarnessFailure("PROCESS_JOB_PORT_ASSOCIATE_FAILED")
            self._process, self._thread, self.pid = self.win32.create_suspended_process(
                self.spec
            )
            if not kernel.AssignProcessToJobObject(self._job, self._process):
                raise HarnessFailure("PROCESS_JOB_ASSIGN_FAILED")
            self._assigned = True
            if kernel.ResumeThread(self._thread) == 0xFFFFFFFF:
                raise HarnessFailure("PROCESS_RESUME_FAILED")
            if not kernel.CloseHandle(self._thread):
                raise HarnessFailure("PROCESS_HANDLE_CLOSE_FAILED")
            self._thread = None
        except BaseException:
            self.cleanup()
            raise

    def active_process_ids(self) -> tuple[int, ...]:
        if self._closed or not self._job:
            raise HarnessFailure("PROCESS_JOB_UNAVAILABLE")
        capacity = 16
        while True:
            buffer_size = (
                ctypes.sizeof(wt.DWORD) * 2 + ctypes.sizeof(ctypes.c_size_t) * capacity
            )
            buffer = ctypes.create_string_buffer(buffer_size)
            returned = wt.DWORD()
            succeeded = self.win32.kernel32.QueryInformationJobObject(
                self._job,
                JOB_OBJECT_BASIC_PROCESS_ID_LIST,
                buffer,
                buffer_size,
                ctypes.byref(returned),
            )
            info = ctypes.cast(
                buffer, ctypes.POINTER(JOBOBJECT_BASIC_PROCESS_ID_LIST)
            ).contents
            assigned = int(info.NumberOfAssignedProcesses)
            count = int(info.NumberOfProcessIdsInList)
            if succeeded:
                if assigned > capacity or count > capacity:
                    capacity = max(capacity * 2, assigned, count)
                    continue
                if count == 0:
                    raise HarnessFailure("PROCESS_JOB_EMPTY")
                process_ids = (ctypes.c_size_t * count).from_buffer(
                    buffer, ctypes.sizeof(wt.DWORD) * 2
                )
                return tuple(int(process_ids[index]) for index in range(count))
            if ctypes.get_last_error() != ERROR_MORE_DATA:
                raise HarnessFailure("PROCESS_JOB_QUERY_FAILED")
            capacity = max(capacity * 2, assigned, count)

    def poll(self) -> int | None:
        try:
            return self.wait(0)
        except subprocess.TimeoutExpired:
            return None

    def wait(self, timeout: float) -> int:
        if self.returncode is None:
            if self._cleanup_failure is not None:
                raise self._cleanup_failure
            result = self.win32.kernel32.WaitForSingleObject(
                self._process, math.ceil(timeout * 1000)
            )
            if result == WAIT_TIMEOUT:
                raise subprocess.TimeoutExpired(self.spec.args, timeout)
            if result != 0:
                raise HarnessFailure("PROCESS_WAIT_FAILED")
            code = wt.DWORD()
            if not self.win32.kernel32.GetExitCodeProcess(self._process, ctypes.byref(code)):
                raise HarnessFailure("PROCESS_EXIT_CODE_FAILED")
            self.returncode = int(code.value)
        return self.returncode

    def _wait_for_empty_job(self) -> None:
        # A job handle is NOT signaled on ordinary last-process exit. Subscribe
        # before assignment, then consume the private port's active-zero message.
        # Job notifications are not guaranteed delivery: absence is a failure,
        # never permission to scan/remove artifacts or infer quiescence from a PID.
        deadline = time.monotonic() + PROCESS_CLEANUP_TIMEOUT
        while True:
            remaining = deadline - time.monotonic()
            if remaining <= 0:
                raise HarnessFailure("PROCESS_JOB_QUIESCENCE_TIMEOUT")
            message = wt.DWORD()
            key = ctypes.c_size_t()
            overlapped = wt.LPVOID()
            if not self.win32.kernel32.GetQueuedCompletionStatus(
                self._port, ctypes.byref(message), ctypes.byref(key),
                ctypes.byref(overlapped), math.ceil(remaining * 1000),
            ):
                code = (
                    "PROCESS_JOB_QUIESCENCE_TIMEOUT"
                    if ctypes.get_last_error() == WAIT_TIMEOUT
                    else "PROCESS_JOB_QUIESCENCE_WAIT_FAILED"
                )
                raise HarnessFailure(code)
            if (
                key.value == JOB_OBJECT_COMPLETION_KEY
                and message.value == JOB_OBJECT_MSG_ACTIVE_PROCESS_ZERO
            ):
                return

    def cleanup(self) -> None:
        if not self._closed:
            kernel = self.win32.kernel32
            try:
                if self._assigned:
                    # Stop only this launch's job, even if its parent already exited.
                    if not kernel.TerminateJobObject(self._job, 1):
                        raise HarnessFailure("PROCESS_JOB_TERMINATE_FAILED")
                    self._wait_for_empty_job()
                elif self._process:
                    # Assignment failed: the unowned primary thread is still suspended.
                    if not kernel.TerminateProcess(self._process, 1):
                        raise HarnessFailure("CLEANUP_REAP_FAILED")
                if self._process:
                    self.wait(PROCESS_CLEANUP_TIMEOUT)
            except HarnessFailure as error:
                self._cleanup_failure = error
            except (OSError, subprocess.TimeoutExpired) as error:
                self._cleanup_failure = HarnessFailure(
                    "CLEANUP_REAP_FAILED", safe_exception_name(error)
                )
            except BaseException as error:
                self._cleanup_failure = HarnessFailure(
                    "CLEANUP_REAP_FAILED", safe_exception_name(error)
                )
                raise
            finally:
                # Closing the job is a kill-on-close backstop, not proof of exit.
                for name in ("_job", "_thread", "_process", "_port"):
                    handle = getattr(self, name)
                    if handle and not kernel.CloseHandle(handle):
                        self._cleanup_failure = self._cleanup_failure or HarnessFailure(
                            "PROCESS_HANDLE_CLOSE_FAILED"
                        )
                    setattr(self, name, None)
                self._closed = True
        if self._cleanup_failure is not None:
            raise self._cleanup_failure


def window_fact_category(class_name: str | None, process_image: str | None) -> str:
    """The fixed category of one extra top-level window.

    The WebView2 popup is the class and the process image together; a dialog
    or the app's own class stands alone; any other class string -- the OS
    accepts arbitrary ones -- is unknown.
    """
    if class_name == WEBVIEW2_POPUP_CLASS and process_image == WEBVIEW2_POPUP_PROCESS_IMAGE:
        return "webview2_popup"
    if class_name == TASK_DIALOG_CLASS:
        return "dialog"
    if class_name == NSH_WINDOW_CLASS:
        return "app_window"
    return "unknown"


def process_image_category(process_image: str | None) -> str:
    """The application family of one process image's executable base name."""
    if process_image == MARKTURBO_PROCESS_IMAGE:
        return "markturbo"
    if process_image == WEBVIEW2_POPUP_PROCESS_IMAGE:
        return "msedgewebview2"
    return "other"


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
        self.kernel32.QueryFullProcessImageNameW.argtypes = [
            wt.HANDLE,
            wt.DWORD,
            wt.LPWSTR,
            ctypes.POINTER(wt.DWORD),
        ]
        self.kernel32.QueryFullProcessImageNameW.restype = wt.BOOL
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
        self.kernel32.CreateJobObjectW.argtypes = [wt.LPVOID, wt.LPCWSTR]
        self.kernel32.CreateJobObjectW.restype = wt.HANDLE
        self.kernel32.SetInformationJobObject.argtypes = [wt.HANDLE, ctypes.c_int, wt.LPVOID, wt.DWORD]
        self.kernel32.SetInformationJobObject.restype = wt.BOOL
        self.kernel32.QueryInformationJobObject.argtypes = [
            wt.HANDLE,
            ctypes.c_int,
            wt.LPVOID,
            wt.DWORD,
            ctypes.POINTER(wt.DWORD),
        ]
        self.kernel32.QueryInformationJobObject.restype = wt.BOOL
        self.kernel32.AssignProcessToJobObject.argtypes = [wt.HANDLE, wt.HANDLE]
        self.kernel32.AssignProcessToJobObject.restype = wt.BOOL
        self.kernel32.TerminateJobObject.argtypes = [wt.HANDLE, wt.UINT]
        self.kernel32.TerminateJobObject.restype = wt.BOOL
        self.kernel32.CreateIoCompletionPort.argtypes = [wt.HANDLE, wt.HANDLE, ctypes.c_size_t, wt.DWORD]
        self.kernel32.CreateIoCompletionPort.restype = wt.HANDLE
        self.kernel32.GetQueuedCompletionStatus.argtypes = [
            wt.HANDLE, ctypes.POINTER(wt.DWORD), ctypes.POINTER(ctypes.c_size_t),
            ctypes.POINTER(wt.LPVOID), wt.DWORD,
        ]
        self.kernel32.GetQueuedCompletionStatus.restype = wt.BOOL
        self.kernel32.ResumeThread.argtypes = [wt.HANDLE]
        self.kernel32.ResumeThread.restype = wt.DWORD
        self.kernel32.WaitForSingleObject.argtypes = [wt.HANDLE, wt.DWORD]
        self.kernel32.WaitForSingleObject.restype = wt.DWORD
        self.kernel32.GetExitCodeProcess.argtypes = [wt.HANDLE, ctypes.POINTER(wt.DWORD)]
        self.kernel32.GetExitCodeProcess.restype = wt.BOOL
        self.kernel32.SetThreadExecutionState.argtypes = [wt.DWORD]
        self.kernel32.SetThreadExecutionState.restype = wt.DWORD

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
        self.user32.IsIconic.argtypes = [wt.HWND]
        self.user32.IsIconic.restype = wt.BOOL
        self.user32.SetForegroundWindow.argtypes = [wt.HWND]
        self.user32.SetForegroundWindow.restype = wt.BOOL
        self.user32.BringWindowToTop.argtypes = [wt.HWND]
        self.user32.BringWindowToTop.restype = wt.BOOL
        self.user32.GetForegroundWindow.restype = wt.HWND
        self.user32.GetGUIThreadInfo.argtypes = [wt.DWORD, ctypes.POINTER(GUITHREADINFO)]
        self.user32.GetGUIThreadInfo.restype = wt.BOOL
        self.user32.IsChild.argtypes = [wt.HWND, wt.HWND]
        self.user32.IsChild.restype = wt.BOOL
        self.user32.IsWindow.argtypes = [wt.HWND]
        self.user32.IsWindow.restype = wt.BOOL
        self.user32.SetCursorPos.argtypes = [ctypes.c_int, ctypes.c_int]
        self.user32.SetCursorPos.restype = wt.BOOL
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

    def create_suspended_process(self, spec: LaunchSpec) -> tuple[int, int, int]:
        # CPython's Windows primitive retains the primary thread handle (Popen
        # closes it). Its STARTUPINFO handle_list avoids leaking other handles.
        import _winapi
        import msvcrt

        handles: list[int] = []
        try:
            with open(os.devnull, "r+b") as null, spec.stderr_path.open("ab") as stderr:
                current = _winapi.GetCurrentProcess()
                for stream in (null, stderr):
                    handles.append(_winapi.DuplicateHandle(
                        current, msvcrt.get_osfhandle(stream.fileno()), current,
                        0, True, _winapi.DUPLICATE_SAME_ACCESS,
                    ))
                startup = subprocess.STARTUPINFO()
                startup.dwFlags = subprocess.STARTF_USESTDHANDLES
                startup.hStdInput = startup.hStdOutput = handles[0]
                startup.hStdError = handles[1]
                startup.lpAttributeList = {"handle_list": handles}
                process, thread, pid, _tid = _winapi.CreateProcess(
                    spec.args[0], subprocess.list2cmdline(spec.args), None, None,
                    True, CREATE_SUSPENDED, spec.env, spec.cwd, startup,
                )
                return process, thread, pid
        finally:
            for handle in handles:
                _winapi.CloseHandle(handle)

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

    def keep_display_awake(self) -> None:
        """Keep the display and system on for this thread until `allow_display_sleep`."""
        if not self.kernel32.SetThreadExecutionState(
            ES_CONTINUOUS | ES_DISPLAY_REQUIRED | ES_SYSTEM_REQUIRED
        ):
            raise HarnessBlocked("EXECUTION_STATE_REQUEST_FAILED")

    def allow_display_sleep(self) -> None:
        self.kernel32.SetThreadExecutionState(ES_CONTINUOUS)

    def interactive_session_blocker(self) -> HarnessBlocked | None:
        """Re-sample what the preflight proved: an active session on the Default desktop.

        A lost foreground is the symptom; a disconnected or locked session is
        the cause worth reporting.
        """
        try:
            if self.wts_state(self.process_session_id(os.getpid())) != WTS_ACTIVE:
                return HarnessBlocked("SESSION_NOT_ACTIVE")
            self.input_desktop()
        except HarnessBlocked as error:
            return error
        except (HarnessFailure, OSError):
            return None
        return None

    # The foreground waits read these; tests replace them on an instance.
    def clock(self) -> float:
        return time.perf_counter()

    def sleep(self, seconds: float) -> None:
        time.sleep(seconds)

    def require_foreground(self, hwnd: int, timeout: float = FOREGROUND_TIMEOUT_SECONDS) -> None:
        """Make `hwnd` the foreground window within at most `FOREGROUND_TIMEOUT_SECONDS`.

        A caller's longer `timeout` is capped: the foreground has one timeout.
        A window already in the foreground is left as it is. When no window
        has the foreground at all (a minimized Remote Desktop client stops
        the remote session's input), the operator is asked once on the
        console to restore it; once any window has the foreground again,
        within `FOREGROUND_RECOVERY_SECONDS`, the activation is retried once
        and counted in `foreground_recoveries`. Another window holding the
        foreground blocks at once.
        """
        if int(self.user32.GetForegroundWindow() or 0) == hwnd:
            return
        try:
            self._activate(hwnd, min(timeout, FOREGROUND_TIMEOUT_SECONDS))
        except HarnessBlocked as error:
            if (
                error.code != "FOREGROUND_PERMISSION_DENIED"
                or error.diagnostics.get("foreground_hwnd") != 0
            ):
                raise
            print(FOREGROUND_RECOVERY_PROMPT, file=sys.stderr, flush=True)
            try:
                wait_until(
                    lambda: int(self.user32.GetForegroundWindow() or 0) != 0,
                    FOREGROUND_RECOVERY_SECONDS,
                    "FOREGROUND_RECOVERY_TIMEOUT",
                    interval=0.1,
                    clock=self.clock,
                    sleep=self.sleep,
                )
            except HarnessFailure:
                raise error from None
            count("foreground_recoveries")
            self._activate(hwnd, min(timeout, FOREGROUND_TIMEOUT_SECONDS))

    def _restore_if_minimized(self, hwnd: int) -> bool:
        """Restore a minimized window only: `SW_RESTORE` also un-maximizes a
        maximized one, which moves every control."""
        if not self.user32.IsIconic(hwnd):
            return False
        return bool(self.user32.ShowWindow(hwnd, SW_RESTORE))

    def _activate(self, hwnd: int, timeout: float) -> None:
        show_window = self._restore_if_minimized(hwnd)
        bring_to_top = bool(self.user32.BringWindowToTop(hwnd))
        set_foreground = bool(self.user32.SetForegroundWindow(hwnd))
        foreground = int(self.user32.GetForegroundWindow() or 0)
        if foreground == hwnd:
            return
        try:
            self.send_inputs([key_input(VK_MENU, False), key_input(VK_MENU, True)])
        except HarnessFailure:
            try:
                self.send_inputs([key_input(VK_MENU, True)])
            except HarnessFailure:
                pass
        show_window = self._restore_if_minimized(hwnd)
        bring_to_top = bool(self.user32.BringWindowToTop(hwnd))
        set_foreground = bool(self.user32.SetForegroundWindow(hwnd))
        foreground = int(self.user32.GetForegroundWindow() or 0)
        if foreground == hwnd:
            return
        deadline = self.clock() + timeout
        attempts = 0
        while self.clock() < deadline:
            attempts += 1
            foreground = int(self.user32.GetForegroundWindow() or 0)
            if foreground == hwnd:
                return
            show_window = self._restore_if_minimized(hwnd)
            bring_to_top = bool(self.user32.BringWindowToTop(hwnd))
            set_foreground = bool(self.user32.SetForegroundWindow(hwnd))
            self.sleep(0.025)
        diagnostics: dict[str, int | bool] = {
            "requested_hwnd": hwnd,
            "foreground_hwnd": foreground,
            "show_window_return": show_window,
            "bring_to_top_return": bring_to_top,
            "set_foreground_return": set_foreground,
            "foreground_attempts": attempts,
        }
        cause = self.interactive_session_blocker()
        code = "FOREGROUND_PERMISSION_DENIED" if cause is None else cause.code
        raise HarnessBlocked(code, diagnostics=diagnostics)

    def foreground_focus(self, hwnd: int) -> int:
        """Observe OS keyboard ownership, not UIA's logical control focus."""
        if self.user32.GetForegroundWindow() != hwnd:
            return 0
        info = GUITHREADINFO(cbSize=ctypes.sizeof(GUITHREADINFO))
        if not self.user32.GetGUIThreadInfo(0, ctypes.byref(info)):
            raise HarnessFailure("NATIVE_FOCUS_QUERY_FAILED")
        return int(info.hwndFocus or 0)

    def foreground_window(self) -> int:
        return int(self.user32.GetForegroundWindow() or 0)

    def task_dialogs(self, process_id: int, owner_hwnd: int | None = None) -> list[int]:
        """Visible task dialogs (`#32770`) of `process_id`.

        nsh 0.7.1 gives the IFileSaveDialog an owner of the hidden surface
        window, not the visible main window, so a picker is found by its
        process. `owner_hwnd` narrows the search to one owner, for the
        nested dialog the picker itself owns (its overwrite confirmation).
        """
        dialogs: list[int] = []
        failure_code: str | None = None

        @self._enum_windows_proc
        def collect(hwnd: int, _lparam: int) -> bool:
            nonlocal failure_code
            if (
                owner_hwnd is not None
                and int(self.user32.GetWindow(hwnd, GW_OWNER) or 0) != owner_hwnd
            ):
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

    def visible_top_level_windows(self, process_ids: tuple[int, ...]) -> tuple[int, ...]:
        owned_processes = frozenset(process_id for process_id in process_ids if process_id > 0)
        if not owned_processes:
            raise HarnessFailure("PROCESS_JOB_EMPTY")
        handles: list[int] = []
        failure_code: str | None = None

        @self._enum_windows_proc
        def collect(hwnd: int, _lparam: int) -> bool:
            nonlocal failure_code
            if not self.user32.IsWindowVisible(hwnd):
                return True
            process = wt.DWORD()
            if not self.user32.GetWindowThreadProcessId(hwnd, ctypes.byref(process)):
                failure_code = "APP_WINDOW_PROCESS_QUERY_FAILED"
                return False
            if process.value in owned_processes:
                handles.append(int(hwnd))
            return True

        if not self.user32.EnumWindows(collect, 0) and failure_code is None:
            raise HarnessFailure("APP_WINDOW_ENUM_FAILED")
        if failure_code is not None:
            raise HarnessFailure(failure_code)
        return tuple(handles)

    def window_facts(self, hwnd: int, main_hwnd: int) -> dict[str, Any]:
        """Content-free identity of one extra top-level window.

        Its class and process image are mapped to fixed categories; the raw
        class name and image path never leave this method. Titles are never
        read: they can contain document names.
        """
        process = wt.DWORD()
        pid = (
            int(process.value)
            if self.user32.GetWindowThreadProcessId(hwnd, ctypes.byref(process))
            else None
        )
        class_buffer = ctypes.create_unicode_buffer(WINDOW_CLASS_CAPACITY)
        class_name = (
            class_buffer.value
            if self.user32.GetClassNameW(hwnd, class_buffer, len(class_buffer))
            else None
        )
        owner = int(self.user32.GetWindow(hwnd, GW_OWNER) or 0)
        image = self.process_image_name(pid) if pid is not None else None
        return {
            "category": window_fact_category(class_name, image),
            "owned_by_main": owner == main_hwnd,
            "process_image": process_image_category(image),
        }

    def process_image_name(self, pid: int) -> str | None:
        """A process's executable file name alone; None when it is unreadable."""
        process = self.kernel32.OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, False, pid)
        if not process:
            return None
        try:
            capacity = wt.DWORD(PROCESS_IMAGE_CAPACITY)
            buffer = ctypes.create_unicode_buffer(capacity.value)
            if not self.kernel32.QueryFullProcessImageNameW(
                process, 0, buffer, ctypes.byref(capacity)
            ):
                return None
            return ntpath.basename(buffer.value)
        finally:
            self.kernel32.CloseHandle(process)

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
        self.send_shortcuts(hwnd, (key,))

    def wait_frames(self, frames: int = FOCUS_SETTLE_FRAMES) -> None:
        """Wait for `frames` display frames: nsh hands the key bindings of a
        focus change to the window with the next frame's batch."""
        for _ in range(frames):
            try:
                failed = ctypes.WinDLL("dwmapi").DwmFlush() != 0
            except OSError:
                failed = True
            if failed:
                # Composition is off (a locked or remote session): one
                # frame's time, as nsh's own test agent waits.
                time.sleep(1 / 60)

    def send_shortcuts(self, hwnd: int, keys: tuple[int, ...]) -> None:
        """Press each of `keys` in turn with Ctrl held, as one SendInput batch:
        no other input can land between them."""
        self.require_foreground(hwnd)
        inputs = [key_input(VK_CONTROL, False)]
        for key in keys:
            inputs.extend((key_input(key, False), key_input(key, True)))
        inputs.append(key_input(VK_CONTROL, True))
        self.send_inputs(inputs)

    def send_key(self, hwnd: int, key: int) -> None:
        self.require_foreground(hwnd)
        self.send_inputs([key_input(key, False), key_input(key, True)])

    def move_cursor(self, x: int, y: int) -> None:
        if not self.user32.SetCursorPos(x, y):
            raise HarnessFailure("SET_CURSOR_POS_FAILED")

    def press_and_hold(self, hwnd: int, x: int, y: int, seconds: float) -> None:
        """Hold the primary button at a screen point: a real pointer hold."""
        self.require_foreground(hwnd)
        self.move_cursor(x, y)
        self.send_inputs([mouse_input(MOUSEEVENTF_LEFTDOWN)])
        try:
            time.sleep(seconds)
        finally:
            self.send_inputs([mouse_input(MOUSEEVENTF_LEFTUP)])

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


CLIPBOARD_OPEN_TIMEOUT_SECONDS = 3.0
CLIPBOARD_OPEN_RETRY_SECONDS = 0.02
# An editor read resends Ctrl+C once when the clipboard has not changed this
# long after the copy, and polls its sequence number this often.
COPY_RESEND_SECONDS = 0.3
CLIPBOARD_SEQUENCE_POLL_SECONDS = 0.01


def open_clipboard_with_retry(
    open_clipboard: Callable[[], bool],
    owner_window_present: Callable[[], bool],
    timeout: float = CLIPBOARD_OPEN_TIMEOUT_SECONDS,
    *,
    clock: Callable[[], float] = time.perf_counter,
    sleep: Callable[[float], None] = time.sleep,
) -> int:
    """Open the clipboard, retrying while another process holds it; the attempt count.

    Windows lets one process open the clipboard at a time, and clipboard
    listeners (the RDP clipboard redirector, clipboard managers) open it
    briefly after every change: a single failed attempt is contention, not
    an unavailable clipboard. A clipboard still held at the deadline blocks
    the run with content-free diagnostics: the attempt count and whether a
    window (the holder) has it open."""
    deadline = clock() + timeout
    attempts = 0
    while True:
        attempts += 1
        if open_clipboard():
            return attempts
        if clock() >= deadline:
            raise HarnessBlocked(
                "CLIPBOARD_UNAVAILABLE",
                diagnostics={
                    "clipboard_open_attempts": attempts,
                    "clipboard_owner_window_present": owner_window_present(),
                },
            )
        sleep(CLIPBOARD_OPEN_RETRY_SECONDS)


class _ClipboardApi:
    """The Win32 text clipboard, declared once."""

    _instance: _ClipboardApi | None = None

    def __init__(self) -> None:
        user32 = ctypes.WinDLL("user32", use_last_error=True)
        kernel32 = ctypes.WinDLL("kernel32", use_last_error=True)
        user32.OpenClipboard.argtypes = [wt.HWND]
        user32.OpenClipboard.restype = wt.BOOL
        user32.CloseClipboard.restype = wt.BOOL
        user32.GetOpenClipboardWindow.restype = wt.HWND
        user32.EnumClipboardFormats.argtypes = [wt.UINT]
        user32.EnumClipboardFormats.restype = wt.UINT
        user32.IsClipboardFormatAvailable.argtypes = [wt.UINT]
        user32.IsClipboardFormatAvailable.restype = wt.BOOL
        user32.GetClipboardData.argtypes = [wt.UINT]
        user32.GetClipboardData.restype = wt.HANDLE
        user32.EmptyClipboard.restype = wt.BOOL
        user32.SetClipboardData.argtypes = [wt.UINT, wt.HANDLE]
        user32.SetClipboardData.restype = wt.HANDLE
        user32.GetClipboardSequenceNumber.restype = wt.DWORD
        kernel32.GlobalAlloc.argtypes = [wt.UINT, ctypes.c_size_t]
        kernel32.GlobalAlloc.restype = wt.HGLOBAL
        kernel32.GlobalFree.argtypes = [wt.HGLOBAL]
        kernel32.GlobalFree.restype = wt.HGLOBAL
        kernel32.GlobalLock.argtypes = [wt.HGLOBAL]
        kernel32.GlobalLock.restype = wt.LPVOID
        kernel32.GlobalUnlock.argtypes = [wt.HGLOBAL]
        kernel32.GlobalUnlock.restype = wt.BOOL
        self.user32 = user32
        self.kernel32 = kernel32

    @classmethod
    def get(cls) -> _ClipboardApi:
        if cls._instance is None:
            cls._instance = cls()
        return cls._instance

    def open(self) -> None:
        open_clipboard_with_retry(
            lambda: bool(self.user32.OpenClipboard(None)),
            lambda: bool(self.user32.GetOpenClipboardWindow()),
        )


def clipboard_sequence_number() -> int:
    """The clipboard's change count; reading it does not open the clipboard."""
    return int(_ClipboardApi.get().user32.GetClipboardSequenceNumber())


def read_clipboard_text() -> str | None:
    """The clipboard's Unicode text, None when it holds none.

    A clipboard with other formats but no text blocks the run: the harness
    could neither use nor restore it faithfully."""
    api = _ClipboardApi.get()
    user32, kernel32 = api.user32, api.kernel32
    api.open()
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


def write_clipboard_text(value: str | None) -> None:
    """Replace the clipboard with `value`, or empty it for None."""
    api = _ClipboardApi.get()
    user32, kernel32 = api.user32, api.kernel32
    api.open()
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


def key_input(key: int, key_up: bool) -> INPUT:
    flags = KEYEVENTF_KEYUP if key_up else 0
    return INPUT(type=INPUT_KEYBOARD, ki=KEYBDINPUT(key, 0, flags, 0, 0))


def mouse_input(flags: int) -> INPUT:
    return INPUT(type=INPUT_MOUSE, mi=MOUSEINPUT(0, 0, 0, flags, 0, 0))


def unicode_input(unit: int, key_up: bool) -> INPUT:
    flags = KEYEVENTF_UNICODE | (KEYEVENTF_KEYUP if key_up else 0)
    return INPUT(type=INPUT_KEYBOARD, ki=KEYBDINPUT(0, unit, flags, 0, 0))


def remove_tree_with_retry(root: Path, timeout: float = ISOLATION_CLEANUP_TIMEOUT) -> None:
    deadline = time.perf_counter() + timeout
    while True:
        try:
            shutil.rmtree(root)
            return
        except FileNotFoundError:
            return
        except PermissionError:
            if time.perf_counter() >= deadline:
                raise
            time.sleep(ISOLATION_CLEANUP_RETRY_INTERVAL)


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
    predicate: Callable[[], Any],
    timeout: float,
    timeout_code: str,
    interval: float = 0.05,
    *,
    clock: Callable[[], float] | None = None,
    sleep: Callable[[float], None] | None = None,
) -> Any:
    clock = clock or time.perf_counter
    sleep = sleep or time.sleep
    deadline = clock() + timeout
    while clock() < deadline:
        try:
            result = predicate()
        except (HarnessBlocked, HarnessFailure):
            raise
        except Exception:
            result = None
        if result:
            return result
        sleep(interval)
    raise HarnessFailure(timeout_code)


# The integer counters a goal run's `metrics` evidence carries.
METRIC_COUNTERS = ("editor_reads", "copy_resends", "foreground_recoveries", "uia_queries")


class RunMetrics:
    """Content-free cost of one goal run: each phase's wall time (in the order
    the phases ended, a nested phase before its parent) and the counters of
    the harness's costly or recovering steps."""

    def __init__(self) -> None:
        self.phase_timings: list[dict[str, Any]] = []
        self.counters = dict.fromkeys(METRIC_COUNTERS, 0)

    def evidence(self) -> dict[str, Any]:
        return {
            "phase_timings": [dict(timing) for timing in self.phase_timings],
            "counters": dict(self.counters),
        }


_ACTIVE_METRICS: contextvars.ContextVar[RunMetrics | None] = contextvars.ContextVar(
    "markturbo_native_metrics", default=None
)


def count(counter: str) -> None:
    """Add one to the running goal's `counter`; nothing outside a goal run."""
    metrics = _ACTIVE_METRICS.get()
    if metrics is not None:
        metrics.counters[counter] += 1


@contextlib.contextmanager
def phase(prefix: str) -> Iterator[None]:
    """Prefix every failure raised inside one phase of a merged session with
    the phase's name, so a shared helper's code stays locatable. A code that
    already carries the prefix is left alone; BLOCKED is the environment's,
    not the phase's, and passes through unchanged. The running goal records
    the phase's wall time whatever its outcome."""
    started = time.perf_counter()
    try:
        yield
    except HarnessFailure as error:
        if error.code.startswith(f"{prefix}_"):
            raise
        raise HarnessFailure(
            f"{prefix}_{error.code}", error.detail, error.observations
        ) from None
    finally:
        metrics = _ACTIVE_METRICS.get()
        if metrics is not None:
            metrics.phase_timings.append({"phase": prefix, "ms": elapsed_ms(started)})


def environment_preflight() -> tuple[Win32, SecurityContext, dict[str, Any]]:
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
        raise HarnessBlocked("SESSION_NOT_ACTIVE")
    active_console = int(win32.kernel32.WTSGetActiveConsoleSessionId())
    input_name, thread_name = win32.input_desktop()
    environment = {
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
    return win32, parent, environment


def preflight(
    exe: Path,
    expected_hash: str,
    evidence: dict[str, Any],
    *,
    fingerprint: Fingerprint | None = None,
) -> tuple[Win32, SecurityContext]:
    """Bind the executable hash and prove the interactive environment.

    Inside `run_native_acceptance` this is the active `NativeSession`'s
    preflight: done once per session, its facts copied into every goal's
    evidence.
    """
    session = _ACTIVE_SESSION.get()
    if session is not None and fingerprint is None:
        return session.preflight(exe, expected_hash, evidence)
    return _preflight(exe, expected_hash, evidence, fingerprint)


def _preflight(
    exe: Path,
    expected_hash: str,
    evidence: dict[str, Any],
    fingerprint: Fingerprint | None = None,
) -> tuple[Win32, SecurityContext]:
    if not exe.is_file():
        raise HarnessFailure("EXECUTABLE_MISSING")
    actual = sha256_file(exe) if fingerprint is None else fingerprint
    evidence["executable"].update(actual.evidence())
    if failure := executable_hash_failure(actual.sha256, expected_hash):
        raise HarnessFailure(failure)
    try:
        pe = inspect_pe(exe)
    except (OSError, ValueError):
        raise HarnessFailure("EXECUTABLE_NOT_X64_PE") from None
    evidence["executable"].update(pe)
    evidence["executable"]["hash_verified"] = True

    win32, parent, environment = environment_preflight()
    evidence["environment"] = environment
    return win32, parent


class NativeSession:
    """What one `mt.py accept` invocation shares across the goals it runs.

    Its `preflight` hashes the executable and samples the environment once,
    asks Windows to keep the display and system on, and saves the user's text
    clipboard once (an empty or text-only clipboard is a precondition); per
    editor read the harness then only copies and reads. `close`
    restores the clipboard and releases the display once, after every app has
    exited. A harness killed mid-run cannot restore the clipboard.

    A `shared` session (`accept all`) also copies the executable once for
    every goal; otherwise the copy lives in the goal's own workdir.
    """

    def __init__(self, *, shared: bool = False) -> None:
        self.shared = shared
        self._binding: tuple[Path, str] | None = None
        self._preflight_evidence: dict[str, Any] = {}
        self._preflight_result: tuple[Win32, SecurityContext] | None = None
        self._preflight_error: BaseException | None = None
        self._awake: Win32 | None = None
        self._clipboard_saved = False
        self._clipboard: str | None = None
        self._root: Path | None = None
        self._copy: tuple[Path, Fingerprint] | None = None

    def preflight(
        self, exe: Path, expected_hash: str, evidence: dict[str, Any]
    ) -> tuple[Win32, SecurityContext]:
        if self._binding is None:
            self._binding = (exe, expected_hash)
            scratch: dict[str, Any] = {"executable": {}, "environment": {}}
            try:
                win32, parent = _preflight(exe, expected_hash, scratch)
                win32.keep_display_awake()
                self._awake = win32
                self._clipboard = read_clipboard_text()
                self._clipboard_saved = True
                self._preflight_result = (win32, parent)
            except Exception as error:
                self._preflight_error = error
            self._preflight_evidence = scratch
        elif self._binding != (exe, expected_hash):
            raise RuntimeError("a native session binds one executable")
        evidence["executable"].update(self._preflight_evidence["executable"])
        evidence["environment"] = copy.deepcopy(self._preflight_evidence["environment"])
        if self._preflight_error is not None:
            raise self._preflight_error
        assert self._preflight_result is not None
        return self._preflight_result

    def copy_executable(self, exe: Path, workdir: Path) -> tuple[Path, Fingerprint]:
        """The isolated executable copy the apps launch from, and its hash."""
        if self._copy is not None:
            return self._copy
        if self.shared:
            if self._root is None:
                self._root = Path(tempfile.mkdtemp(prefix="markturbo-native-session-")).resolve()
            directory = self._root
        else:
            directory = workdir / "bin"
            directory.mkdir()
        copied = Path(shutil.copy2(exe, directory / exe.name)).resolve()
        result = (copied, sha256_file(copied))
        if self.shared:
            self._copy = result
        return result

    def close(self) -> str | None:
        """Undo what the session changed; the first failure's code, if any."""
        failure: str | None = None
        if self._clipboard_saved:
            self._clipboard_saved = False
            try:
                write_clipboard_text(self._clipboard)
            except Exception:
                failure = "CLIPBOARD_RESTORE_FAILED"
            self._clipboard = None
        if self._awake is not None:
            self._awake.allow_display_sleep()
            self._awake = None
        if self._root is not None:
            try:
                remove_tree_with_retry(self._root)
            except OSError:
                failure = failure or "SESSION_CLEANUP_FAILED"
            self._root = None
        return failure


_ACTIVE_SESSION: contextvars.ContextVar[NativeSession | None] = contextvars.ContextVar(
    "markturbo_native_session", default=None
)


def mark_remaining_cases(
    evidence: dict[str, Any], start_index: int, status: str, reason_code: str
) -> None:
    for case in evidence["cases"][start_index:]:
        if case["status"] == "NOT_RUN":
            case["status"] = status
            case["reason_code"] = reason_code
            if status != "NOT_RUN":
                case["duration_ms"] = 0.0


def finite_nonnegative(value: Any) -> bool:
    return (
        isinstance(value, (int, float))
        and not isinstance(value, bool)
        and math.isfinite(value)
        and value >= 0
    )


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


class NativeHarness:
    # The harness's own waits read these; tests replace them on an instance
    # with a fake clock. They resolve `time.*` per call, so patching the
    # module's `time` also reaches them.
    def clock(self) -> float:
        return time.perf_counter()

    def sleep(self, seconds: float) -> None:
        time.sleep(seconds)

    # The deadline of the outermost wait in progress (`bounded_by`).
    _deadline: float | None = None
    # `clock()` when the last editor read asked the app to copy: the instant
    # the read's text was sampled, before any clipboard wait.
    last_editor_sample_at: float | None = None

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
        self.processes: list[OwnedProcess] = []

    @contextlib.contextmanager
    def bounded_by(self, timeout: float) -> Iterator[None]:
        """Run waits nested inside an outer wait that must end within `timeout`."""
        previous = self._deadline
        deadline = self.clock() + timeout
        self._deadline = deadline if previous is None else min(previous, deadline)
        try:
            yield
        finally:
            self._deadline = previous

    def inner_timeout(self) -> float | None:
        """A nested step's wait: at most `INNER_WAIT_SECONDS` and never past the
        outer deadline; None (the step's own default) outside any outer wait."""
        if self._deadline is None:
            return None
        return max(0.0, min(INNER_WAIT_SECONDS, self._deadline - self.clock()))

    def case_roots(self, case_id: str) -> tuple[Path, Path, Path, Path]:
        case_root = (self.root / "cases" / case_id).resolve()
        data_root = (case_root / "data").resolve()
        config_root = (case_root / "config").resolve()
        workspace_root = (case_root / "workspace").resolve()
        stderr_path = (case_root / "stderr.log").resolve()
        for directory in (data_root, config_root, workspace_root):
            directory.mkdir(parents=True, exist_ok=True)
        return data_root, config_root, workspace_root, stderr_path

    def launch_app(
        self,
        target: Path | None,
        data_root: Path,
        config_root: Path,
        workspace_root: Path,
        stderr_path: Path,
    ) -> RunningApp:
        spec = build_launch_spec(
            self.copied_exe,
            target,
            data_root,
            config_root,
            workspace_root,
            stderr_path,
            ephemeral_openai_api_key=self.openai_api_key_for_child(),
        )
        process = OwnedProcess(self.win32, spec)
        self.processes.append(process)
        try:
            process.start()
        except OSError as error:
            raise HarnessFailure(launch_failure_code(error), safe_exception_name(error)) from None
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
        try:
            self.win32.require_foreground(hwnd, FOREGROUND_TIMEOUT_SECONDS)
        except HarnessBlocked as error:
            if error.code != "FOREGROUND_PERMISSION_DENIED":
                raise
            try:
                window.click_input(coords=TITLE_ROW_CLICK_POINT)
            except Exception:
                raise error from None
            self.win32.require_foreground(hwnd, FOREGROUND_TIMEOUT_SECONDS)
        return RunningApp(
            process,
            window,
            hwnd,
            spec,
            child_context,
            data_root / "logs" / f"markturbo-{process.pid}.log",
        )

    def openai_api_key_for_child(self) -> str | None:
        """Return the one process-only credential a native harness may restore."""
        return None
    def launch(
        self,
        target: Path,
        data_root: Path,
        config_root: Path,
        workspace_root: Path,
        stderr_path: Path,
    ) -> RunningApp:
        running = self.launch_app(
            target, data_root, config_root, workspace_root, stderr_path
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
        count("uia_queries")
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

    def control_by_name(
        self,
        hwnd: int,
        name: str,
        control_type: str,
        mismatch_code: str,
        *,
        anonymous: bool = True,
    ) -> Any | None:
        """The one control of `control_type` named `name`.

        `anonymous` keeps only elements without an AutomationId, such as the
        source editor and card headings, so a control with its own id that
        happens to share the label never matches.
        """
        matches = self.controls_by_name(hwnd, name, control_type, anonymous=anonymous)
        if not matches:
            return None
        if len(matches) != 1:
            raise HarnessFailure(mismatch_code)
        return matches[0]

    def controls_by_name(
        self, hwnd: int, name: str, control_type: str, *, anonymous: bool = True
    ) -> list[Any]:
        count("uia_queries")
        uia = self.iuia_class()
        condition = uia.iuia.CreatePropertyCondition(uia.UIA_dll.UIA_NamePropertyId, name)
        elements = self.fresh_uia_root(hwnd).element.FindAll(
            uia.tree_scope["descendants"], condition
        )
        expected_control_type = uia.known_control_types[control_type]
        controls = []
        for index in range(elements.Length):
            element = elements.GetElement(index)
            if element.CurrentName != name or element.CurrentControlType != expected_control_type:
                continue
            if anonymous and element.CurrentAutomationId:
                continue
            control = self.uia_wrapper_class(self.uia_element_info_class(element))
            if control.element_info.control_type is None:
                raise RuntimeError("UIA_CONTROL_PROPERTIES_UNAVAILABLE")
            controls.append(control)
        return controls

    def _wait_ready(
        self,
        lookup: Callable[[], Any | None],
        timeout_code: str,
        timeout: float | None,
    ) -> Any:
        last_failure_type: str | None = None

        def locate() -> Any | None:
            nonlocal last_failure_type
            try:
                control = lookup()
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
                clock=self.clock,
                sleep=self.sleep,
            )
        except HarnessFailure as error:
            if error.code == timeout_code and last_failure_type is not None:
                raise HarnessFailure(timeout_code, last_failure_type) from None
            raise

    def find_control(
        self,
        app: RunningApp,
        automation_id: str,
        control_type: str,
        timeout_code: str,
        mismatch_code: str,
        timeout: float | None = None,
    ) -> Any:
        return self._wait_ready(
            lambda: self.control_by_id(app.hwnd, automation_id, control_type, mismatch_code),
            timeout_code,
            timeout,
        )

    def find_named(
        self,
        app: RunningApp,
        name: str,
        control_type: str,
        timeout_code: str,
        mismatch_code: str,
        timeout: float | None = None,
        *,
        anonymous: bool = True,
    ) -> Any:
        return self._wait_ready(
            lambda: self.control_by_name(
                app.hwnd, name, control_type, mismatch_code, anonymous=anonymous
            ),
            timeout_code,
            timeout,
        )

    def name_present(self, app: RunningApp, name: str, control_type: str) -> bool:
        """Whether any control of `control_type` carries `name` (ids allowed)."""
        try:
            return bool(self.controls_by_name(app.hwnd, name, control_type, anonymous=False))
        except HarnessFailure:
            raise
        except Exception as error:
            raise HarnessFailure("UIA_NAME_QUERY_FAILED", safe_exception_name(error)) from None

    def wait_name(
        self, app: RunningApp, name: str, control_type: str, timeout_code: str,
        timeout: float | None = None,
    ) -> None:
        """Wait for a control named `name`. A query that fails while the UI
        rebuilds (an element gone mid-read) is a poll that saw nothing yet,
        as in `_wait_ready`; the timeout keeps the last failure's type."""
        last_failure_type: str | None = None

        def present() -> bool:
            nonlocal last_failure_type
            try:
                return self.name_present(app, name, control_type)
            except HarnessFailure as error:
                if error.code != "UIA_NAME_QUERY_FAILED":
                    raise
                last_failure_type = error.detail or None
                return False

        try:
            wait_until(
                present,
                self.ui_timeout if timeout is None else timeout,
                timeout_code,
                interval=0.05,
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

    def invoke_control(self, control: Any, failure_code: str) -> None:
        """One whole press through the UIA Invoke pattern (no pointer)."""
        try:
            control.invoke()
        except Exception as error:
            raise HarnessFailure(failure_code, safe_exception_name(error)) from None

    def hold_control(
        self, app: RunningApp, control: Any, seconds: float, failure_code: str
    ) -> None:
        """Press and hold the pointer at the control's centre, then release.

        A card glides in, so the centre is taken only once two reads 50 ms
        apart agree: a hold aimed mid-glide can land beside the control.
        """

        def centre() -> tuple[int, int]:
            try:
                rectangle = control.rectangle()
            except Exception as error:
                raise HarnessFailure(failure_code, safe_exception_name(error)) from None
            return (
                (rectangle.left + rectangle.right) // 2,
                (rectangle.top + rectangle.bottom) // 2,
            )

        previous = centre()
        self.sleep(0.05)

        def settled() -> tuple[int, int] | None:
            nonlocal previous
            current = centre()
            if current == previous:
                return current
            previous = current
            return None

        x, y = wait_until(
            settled,
            self.ui_timeout,
            f"{failure_code}_TARGET_UNSETTLED",
            interval=0.05,
            clock=self.clock,
            sleep=self.sleep,
        )
        self.win32.press_and_hold(app.hwnd, x, y, seconds)

    def _press_menu_anchor(self, app: RunningApp) -> None:
        anchor = self.find_control(
            app,
            MENU_BUTTON_AUTOMATION_ID,
            "Button",
            "MENU_BUTTON_UIA_TIMEOUT",
            "MENU_BUTTON_UIA_CONTRACT_MISMATCH",
        )
        self.invoke_control(anchor, "MENU_BUTTON_INVOKE_FAILED")

    def menu_command(self, app: RunningApp, automation_id: str) -> None:
        """Run one title-menu command: open Settings and invoke the row."""
        code = automation_id.upper().replace("-", "_")
        if self.control_by_id(
            app.hwnd, automation_id, "Button", f"{code}_UIA_CONTRACT_MISMATCH"
        ) is None:
            self._press_menu_anchor(app)
        item = self.find_control(
            app, automation_id, "Button", f"{code}_UIA_TIMEOUT", f"{code}_UIA_CONTRACT_MISMATCH"
        )
        self.invoke_control(item, f"{code}_INVOKE_FAILED")

    def _view_button(self, app: RunningApp, previous: str | None = None) -> tuple[Any, str]:
        name = ""

        def locate() -> Any | None:
            nonlocal name
            button = self.control_by_id(
                app.hwnd, VIEW_BUTTON_AUTOMATION_ID, "Button", "TITLE_VIEW_UIA_CONTRACT_MISMATCH"
            )
            if button is None:
                return None
            name = button.element_info.name or ""
            return button if name != previous else None

        button = self._wait_ready(locate, "TITLE_VIEW_UIA_TIMEOUT", None)
        return button, name

    def view_button_name(self, app: RunningApp) -> str:
        return self._view_button(app)[1]

    def view_command(self, app: RunningApp, view: str) -> None:
        """Reach the requested view in at most one full cycle of title-view."""
        button, name = self._view_button(app)
        for presses in range(4):
            current = _VIEW_BUTTON_NAMES.get(name)
            if current == view:
                return
            if current is None or presses == 3:
                raise HarnessFailure("TITLE_VIEW_UIA_CONTRACT_MISMATCH")
            self.invoke_control(button, "TITLE_VIEW_INVOKE_FAILED")
            button, name = self._view_button(app, previous=name)

    def _compatibility_button(self, app: RunningApp) -> Any | None:
        return self.control_by_id(
            app.hwnd, MENU_WEB_AUTOMATION_ID, "Button", "TITLE_MENU_WEB_UIA_CONTRACT_MISMATCH"
        )

    def _open_compatibility_switch(self, app: RunningApp) -> tuple[Any, bool]:
        button = self._compatibility_button(app)
        # A closing menu keeps its old, disabled rows until the fade ends.
        if button is None or not button.is_enabled():
            self._press_menu_anchor(app)
        button = self._wait_ready(
            lambda: self._compatibility_button(app), "TITLE_MENU_WEB_UIA_TIMEOUT", None
        )
        # nsh maps Switch to a UIA Button with Toggle, not Invoke. Its On/Off
        # text is folded into the control rather than exposed on its own.
        try:
            toggle = button.iface_toggle
            checked = toggle.CurrentToggleState
        except self.no_pattern_error_class:
            raise HarnessFailure("TITLE_MENU_WEB_UIA_CONTRACT_MISMATCH") from None
        except Exception as error:
            raise HarnessFailure("TITLE_MENU_WEB_UIA_QUERY_FAILED", safe_exception_name(error)) from None
        if checked not in (0, 1):
            raise HarnessFailure("TITLE_MENU_WEB_UIA_CONTRACT_MISMATCH")
        return toggle, checked == 1

    def compatibility_state(self, app: RunningApp) -> bool:
        """Read the Web renderer's checked state and dismiss its menu."""
        _toggle, checked = self._open_compatibility_switch(app)
        self.win32.send_key(app.hwnd, VK_ESCAPE)
        return checked

    def set_compatibility(self, app: RunningApp, on: bool) -> None:
        toggle, checked = self._open_compatibility_switch(app)
        if checked == on:
            self.win32.send_key(app.hwnd, VK_ESCAPE)
            return
        try:
            toggle.Toggle()
        except Exception as error:
            raise HarnessFailure("TITLE_MENU_WEB_INVOKE_FAILED", safe_exception_name(error)) from None

        def closed() -> bool:
            button = self._compatibility_button(app)
            return button is None or not button.is_enabled()

        wait_until(
            closed, self.ui_timeout, "TITLE_MENU_WEB_CLOSE_UIA_TIMEOUT",
            interval=0.025, clock=self.clock, sleep=self.sleep,
        )

    def sidebar_collapsed(self, app: RunningApp) -> bool:
        """Whether the expanded sidebar panel is absent from UIA."""
        return self.control_absent(
            app, SIDEBAR_PANEL_AUTOMATION_ID, "Group",
            "SIDEBAR_PANEL_UIA_QUERY_FAILED", "SIDEBAR_PANEL_UIA_CONTRACT_MISMATCH",
        )

    def ensure_sidebar_open(self, app: RunningApp) -> None:
        if not self.sidebar_collapsed(app):
            return
        self.win32.send_shortcuts(app.hwnd, (VK_B,))
        self.find_control(
            app, SIDEBAR_PANEL_AUTOMATION_ID, "Group",
            "SIDEBAR_PANEL_UIA_TIMEOUT", "SIDEBAR_PANEL_UIA_CONTRACT_MISMATCH",
        )

    def save_via_menu(self, app: RunningApp) -> None:
        self.menu_command(app, MENU_SAVE_AUTOMATION_ID)

    def quit_via_menu(self, app: RunningApp) -> None:
        self.menu_command(app, MENU_QUIT_AUTOMATION_ID)

    def editor_control(self, app: RunningApp, timeout: float | None = None) -> Any:
        return self.find_named(
            app,
            EDITOR_NAME,
            "Edit",
            "SOURCE_EDITOR_UIA_TIMEOUT",
            "SOURCE_EDITOR_UIA_CONTRACT_MISMATCH",
            timeout,
        )

    def activate_source_layout(self, app: RunningApp) -> None:
        """Return to Edit and wait until the source editor is ready."""
        self.view_command(app, "Edit")
        self.editor_control(app)

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
        try:
            return self.control_by_name(
                app.hwnd,
                EDITOR_NAME,
                "Edit",
                "DISCARD_EDITOR_ABSENCE_UIA_CONTRACT_MISMATCH",
            ) is None
        except HarnessFailure:
            raise
        except Exception as error:
            raise HarnessFailure(
                "DISCARD_EDITOR_ABSENCE_UIA_QUERY_FAILED", safe_exception_name(error)
            ) from None

    def focus_editor(self, app: RunningApp, timeout: float | None = None) -> None:
        self._focus_editor_control(app, self.editor_control(app, timeout))

    def _focus_editor_control(self, app: RunningApp, editor: Any) -> None:
        """Give `editor` the keyboard: UIA SetFocus only when it lacks the
        focus, then the window the foreground."""
        try:
            focused = bool(editor.has_keyboard_focus())
        except Exception:
            focused = False
        if not focused:
            try:
                editor.set_focus()
            except Exception as error:
                raise HarnessFailure("EDITOR_FOCUS_FAILED", safe_exception_name(error)) from None
        self.win32.require_foreground(app.hwnd, FOREGROUND_TIMEOUT_SECONDS)
        if not focused:
            # The editor binds its own keys (Ctrl+Enter: a new line) as it
            # takes the focus, and the app binds its Ctrl+Enter again after
            # it; both reach the window's keymap with a later frame. A user
            # never presses a key within those frames, so neither does this.
            self.win32.wait_frames()

    def read_text_clipboard(self) -> str | None:
        return read_clipboard_text()

    def clipboard_sequence(self) -> int:
        return clipboard_sequence_number()

    def _clipboard_changed_since(self, sequence: int, timeout: float) -> bool:
        """Whether the clipboard changes after `sequence` within `timeout`
        (checked at least once)."""
        deadline = self.clock() + timeout
        while True:
            if self.clipboard_sequence() != sequence:
                return True
            if self.clock() >= deadline:
                return False
            self.sleep(CLIPBOARD_SEQUENCE_POLL_SECONDS)

    def write_text_clipboard(self, value: str | None) -> None:
        write_clipboard_text(value)

    def forget_clipboard_text(self, expected: Fingerprint) -> None:
        """Empties the clipboard when it already holds `expected`.

        An editor read takes any sequence move as its copy landing, and
        another writer (the Remote Desktop clipboard relay) can move it too.
        A dropped copy would then read the text left from an earlier read,
        which passes only when that text is the one awaited. Emptying the
        clipboard then leaves a dropped copy nothing to pass on; any other
        text only fails the comparison and is read again.
        """
        current = self.read_text_clipboard()
        if current is not None and fingerprint_text(current) == expected:
            self.write_text_clipboard(None)

    def wait_clipboard_text(self, timeout_code: str) -> str:
        """The clipboard's text once something (even an empty text) arrives.

        Inside an outer wait (`bounded_by`) this is a nested step and waits at
        most `inner_timeout()`; on its own it waits `ui_timeout`.
        """

        def copied() -> tuple[str] | None:
            value = self.read_text_clipboard()
            return None if value is None else (value,)

        timeout = self.inner_timeout()
        (value,) = wait_until(
            copied,
            self.ui_timeout if timeout is None else timeout,
            timeout_code,
            interval=0.025,
            clock=self.clock,
            sleep=self.sleep,
        )
        return value

    def read_editor_text(self, app: RunningApp, editor: Any | None = None) -> str:
        """The editor's exact document through its own Select All + Copy.

        Only copies and reads, never writes the clipboard: the session saved
        the user's clipboard once and restores it when the run ends. The copy
        is known to have landed when the clipboard's sequence number moves
        past the one sampled before Ctrl+A Ctrl+C (one input batch); the text
        is then read once. A copy that has not landed after
        `COPY_RESEND_SECONDS` is resent once (`copy_resends`). Another
        clipboard writer (the Remote Desktop clipboard relay) can move the
        sequence too, so callers compare the fingerprint and retry, first
        emptying a clipboard that already holds the text they await
        (`forget_clipboard_text`). Nothing
        read here is written to evidence except its fingerprint. `editor` is
        the editor control a caller already found; without one it is looked
        up here.
        """
        count("editor_reads")
        if editor is None:
            editor = self.editor_control(app, self.inner_timeout())
        self._focus_editor_control(app, editor)
        timeout = self.inner_timeout()
        timeout = self.ui_timeout if timeout is None else timeout
        deadline = self.clock() + timeout
        sequence = self.clipboard_sequence()
        self.win32.send_shortcuts(app.hwnd, (VK_A, VK_C))
        self.last_editor_sample_at = self.clock()
        if not self._clipboard_changed_since(sequence, min(COPY_RESEND_SECONDS, timeout)):
            count("copy_resends")
            self.win32.send_shortcut(app.hwnd, VK_C)
            self.last_editor_sample_at = self.clock()
            if not self._clipboard_changed_since(sequence, max(0.0, deadline - self.clock())):
                raise HarnessFailure("EDITOR_COPY_TIMEOUT")
        return self.wait_clipboard_text("EDITOR_COPY_TIMEOUT")

    def read_editor_fingerprint(self, app: RunningApp) -> Fingerprint | None:
        """One editor read on one UIA query: the editor found here is the one
        focused and copied; None while there is no ready editor."""
        editor = self.control_by_name(
            app.hwnd, EDITOR_NAME, "Edit", "SOURCE_EDITOR_UIA_CONTRACT_MISMATCH"
        )
        if editor is None or not editor.is_visible() or not editor.is_enabled():
            return None
        return fingerprint_text(self.read_editor_text(app, editor))

    def editor_fingerprint(self, app: RunningApp) -> Fingerprint:
        fingerprint, _ = self.wait_editor_fingerprint(app, None, self.ui_timeout)
        return fingerprint

    def replace_editor(self, app: RunningApp, value: str) -> Fingerprint:
        self.focus_editor(app)
        self.win32.send_shortcut(app.hwnd, VK_A)
        if value:
            self.win32.send_unicode(app.hwnd, value)
            actual, _ = self.wait_editor_fingerprint(
                app, fingerprint_text(value), self.ui_timeout
            )
            return actual
        self.win32.send_key(app.hwnd, VK_BACK)
        # An empty document copies line-wise as its one empty line, "\n"
        # (nsh-base 0.7.1 `editor/commands.rs` `copy`). Backspace over Select
        # All leaves no line break behind, so here "\n" can only mean empty.
        self.wait_editor_fingerprint(app, fingerprint_text("\n"), self.ui_timeout)
        return fingerprint_text("")

    def guard_dialog(
        self,
        app: RunningApp,
        heading: str,
        button_names: dict[str, str],
        button_automation_ids: dict[str, str],
        timeout: float | None = None,
    ) -> dict[str, Any]:
        """One in-window guard card (`ui/notices.rs` `guard_card`): its decisions.

        The heading Text names the card's kind. Each decision is a Button with
        its own AutomationId and label; `button_names` and
        `button_automation_ids` share their keys.
        """
        last_failure_type: str | None = None

        def locate() -> dict[str, Any] | None:
            nonlocal last_failure_type
            if app.process.poll() is not None:
                raise HarnessFailure("PROCESS_EXITED_BEFORE_GUARD")
            try:
                if self.control_by_name(
                    app.hwnd, heading, "Text", "GUARD_CARD_UIA_CONTRACT_MISMATCH"
                ) is None:
                    return None
                buttons = {}
                for decision, automation_id in button_automation_ids.items():
                    button = self.control_by_id(
                        app.hwnd,
                        automation_id,
                        "Button",
                        "GUARD_BUTTON_UIA_CONTRACT_MISMATCH",
                        expected_name=button_names[decision],
                    )
                    if button is None:
                        return None
                    buttons[decision] = button
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
                "GUARD_CARD_UIA_TIMEOUT",
            )
        except HarnessFailure as error:
            if error.code == "GUARD_CARD_UIA_TIMEOUT" and last_failure_type is not None:
                raise HarnessFailure(error.code, last_failure_type) from None
            raise

    def lifecycle_dialog(self, app: RunningApp, timeout: float | None = None) -> dict[str, Any]:
        """The dirty tab's close guard card and its decisions."""
        return self.guard_dialog(
            app, GUARD_HEADING, GUARD_BUTTON_NAMES, GUARD_BUTTON_AUTOMATION_IDS, timeout
        )

    def click_lifecycle_decision(self, app: RunningApp, name: str) -> None:
        button = self.lifecycle_dialog(app)[name]
        self.click_control(button, LIFECYCLE_CLICK_FAILURE_CODES[name])

    def quit_dialog(self, app: RunningApp, timeout: float | None = None) -> dict[str, Any]:
        """The quit guard card (`GuardKind::QuitApp`) of every window close and
        Quit; its Save is Save all, which saves every dirty tab."""
        return self.guard_dialog(
            app,
            QUIT_GUARD_HEADING,
            QUIT_GUARD_BUTTON_NAMES,
            QUIT_GUARD_BUTTON_AUTOMATION_IDS,
            timeout,
        )

    def click_quit_decision(self, app: RunningApp, name: str) -> None:
        button = self.quit_dialog(app)[name]
        self.click_control(button, LIFECYCLE_CLICK_FAILURE_CODES[name])

    def wait_process_exit(self, app: RunningApp, timeout: float | None = None) -> None:
        try:
            returncode = app.process.wait(timeout=timeout or self.ui_timeout)
        except subprocess.TimeoutExpired:
            raise HarnessFailure("PROCESS_EXIT_TIMEOUT") from None
        if returncode != 0:
            raise HarnessFailure("PROCESS_EXIT_NONZERO")

    def close_app(self, app: RunningApp) -> None:
        self.win32.post_close(app.hwnd)
        self.wait_process_exit(app)

    def reap(self, app: RunningApp) -> None:
        try:
            if app.process.poll() is None:
                try:
                    self.win32.post_close(app.hwnd)
                    app.process.wait(timeout=2.0)
                except (HarnessBlocked, HarnessFailure, subprocess.TimeoutExpired):
                    pass
        finally:
            app.process.cleanup()

    def terminate(self, app: RunningApp) -> None:
        self.win32.terminate_process(app.process.pid)
        try:
            app.process.wait(timeout=5.0)
        except subprocess.TimeoutExpired:
            raise HarnessFailure("TERMINATE_PROCESS_WAIT_FAILED") from None

    def cleanup(self) -> None:
        failure: BaseException | None = None
        for process in self.processes:
            try:
                process.cleanup()
            except BaseException as error:
                failure = failure or error
        if failure is not None:
            raise failure

    def wait_file(self, path: Path, expected: Fingerprint, code: str) -> Fingerprint:
        """Wait for `path` to hold `expected`, hashing it every 100 ms: each
        read is a window in which the app's own save competes for the file."""
        last: Fingerprint | None = None

        def matches() -> Fingerprint | None:
            nonlocal last
            try:
                last = sha256_file(path)
            except OSError:
                last = None
                return None
            return last if last == expected else None

        try:
            return wait_until(
                matches,
                self.ui_timeout,
                code,
                interval=FILE_POLL_SECONDS,
                clock=self.clock,
                sleep=self.sleep,
            )
        except HarnessFailure as error:
            if error.code != code:
                raise
            # Only the byte-length relation leaves the harness.
            if last is None:
                relation = "FileUnreadable"
            else:
                difference = last.byte_count - expected.byte_count
                relation = (
                    "FileLonger" if difference > 0
                    else "FileShorter" if difference < 0
                    else "FileSameLength"
                )
            raise HarnessFailure(code, relation) from None

    def wait_editor_fingerprint(
        self,
        app: RunningApp,
        expected: Fingerprint | None,
        timeout: float,
    ) -> tuple[Fingerprint, float]:
        """Wait up to `timeout` for the editor's copied text (to equal `expected`).

        Every nested step (finding the editor, the copy wait) is bounded by
        what is left of `timeout`. The returned milliseconds end when the
        matching read asked the app to copy (`last_editor_sample_at`), so
        they exclude the harness's own clipboard waits after that sample.
        """
        started = self.clock()
        saw_readable_value = False
        last_actual: Fingerprint | None = None
        last_failure_type: str | None = None
        last_failure_code: str | None = None
        matched_at = started

        def matches() -> Fingerprint | None:
            nonlocal saw_readable_value, last_actual, last_failure_type, last_failure_code
            nonlocal matched_at
            self.last_editor_sample_at = None
            try:
                actual = self.read_editor_fingerprint(app)
            except (HarnessBlocked, HarnessFailure) as error:
                if isinstance(error, HarnessFailure) and error.code in {
                    "EDITOR_COPY_TIMEOUT",
                    "SOURCE_EDITOR_UIA_TIMEOUT",
                }:
                    last_failure_type = "HarnessFailure"
                    last_failure_code = error.code
                    return None
                raise
            except Exception as error:
                last_failure_type = safe_exception_name(error)
                last_failure_code = None
                return None
            if actual is None:
                return None
            saw_readable_value = True
            last_actual = actual
            if expected is not None and actual != expected:
                return None
            matched_at = (
                self.clock() if self.last_editor_sample_at is None else self.last_editor_sample_at
            )
            return actual

        timeout_code = "EDITOR_TEXT_TIMEOUT" if expected is None else "EDITOR_EXACT_BYTES_TIMEOUT"
        if expected is not None:
            self.forget_clipboard_text(expected)
        try:
            with self.bounded_by(timeout):
                actual = wait_until(
                    matches, timeout, timeout_code, interval=0.1, clock=self.clock, sleep=self.sleep
                )
        except HarnessFailure as error:
            if error.code != timeout_code:
                raise
            if expected is not None and last_actual is not None:
                # Only the byte-length relation leaves the harness.
                difference = last_actual.byte_count - expected.byte_count
                relation = (
                    "EditorLonger" if difference > 0
                    else "EditorShorter" if difference < 0
                    else "EditorSameLength"
                )
                raise HarnessFailure(timeout_code, relation) from None
            if not saw_readable_value and last_failure_code == "SOURCE_EDITOR_UIA_TIMEOUT":
                # The editor control never appeared: keep that locatable code.
                raise HarnessFailure("SOURCE_EDITOR_UIA_TIMEOUT") from None
            raise HarnessFailure("EDITOR_TEXT_TIMEOUT", last_failure_type or "") from None
        return actual, (matched_at - started) * 1000

    def source_editor_has_focus(self, app: RunningApp) -> bool:
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

    def require_source_editor_focus(self, app: RunningApp, failure_code: str) -> None:
        wait_until(
            lambda: self.source_editor_has_focus(app),
            self.ui_timeout,
            failure_code,
            interval=0.025,
        )


def _completion_status(returncode: int) -> str:
    return {0: "PASS", 1: "FAIL", 2: "BLOCKED"}[returncode]


def _complete_early(
    evidence: dict[str, Any],
    required_case_ids: tuple[str, ...],
    returncode: int,
    code: str,
    *,
    blocked: bool,
    finalize_evidence: Callable[[dict[str, Any]], None] | None = None,
) -> tuple[int, dict[str, Any], str]:
    mark_remaining_cases(
        evidence,
        0,
        "BLOCKED" if blocked else "NOT_RUN",
        code,
    )
    if finalize_evidence is not None:
        finalize_evidence(evidence)
    complete_evidence(evidence, _completion_status(returncode), required_case_ids)
    return returncode, evidence, code


def _record_cleanup_failure(
    evidence: dict[str, Any],
    required_case_ids: tuple[str, ...],
    current_index: int,
    code: str,
    failure_type: str,
) -> None:
    failed = evidence["cases"][min(current_index, len(required_case_ids) - 1)]
    failed.update(
        status="FAIL",
        reason_code=code,
        failure_type=failure_type,
        duration_ms=failed["duration_ms"] or 0.0,
    )
    for case in evidence["cases"][current_index + 1 :]:
        if case["status"] != "PASS":
            case.update(
                status="NOT_RUN",
                reason_code="SKIPPED_AFTER_CLEANUP_FAILURE",
                failure_type=None,
                duration_ms=None,
            )


def worst_returncode(*returncodes: int) -> int:
    """The decisive exit status: FAIL (1) over BLOCKED (2) over PASS (0).

    A FAIL is a behavioural verdict; BLOCKED only says the environment kept a
    run from reaching one.
    """
    return max(returncodes, key=lambda code: {0: 0, 2: 1, 1: 2}[code])


def run_native_acceptance(
    args: argparse.Namespace, plan: NativeRunPlan, session: NativeSession | None = None
) -> tuple[int, dict[str, Any], str]:
    """Run one goal's native cases with one fail-closed lifecycle.

    `session` is the invocation's shared `NativeSession` (`accept all`);
    without one the run owns a session and closes it before returning.
    """
    owned = session is None
    active = NativeSession() if session is None else session
    metrics = RunMetrics()
    token = _ACTIVE_SESSION.set(active)
    metrics_token = _ACTIVE_METRICS.set(metrics)
    try:
        returncode, evidence, code = _run_goal(args, plan, active)
        evidence["metrics"] = metrics.evidence()
    finally:
        _ACTIVE_METRICS.reset(metrics_token)
        _ACTIVE_SESSION.reset(token)
        close_failure = active.close() if owned else None
    if close_failure is not None:
        returncode, code = 1, close_failure
        args.cleanup_failed = True
        started = [index for index, case in enumerate(evidence["cases"]) if case["status"] != "NOT_RUN"]
        _record_cleanup_failure(
            evidence,
            plan.required_case_ids,
            started[-1] if started else 0,
            code,
            "HarnessFailure",
        )
        if plan.finalize_evidence is not None:
            plan.finalize_evidence(evidence)
        complete_evidence(evidence, "FAIL", plan.required_case_ids)
    return returncode, evidence, code


def _run_goal(
    args: argparse.Namespace, plan: NativeRunPlan, session: NativeSession
) -> tuple[int, dict[str, Any], str]:
    evidence = plan.new_evidence(args.expect_exe_sha256)
    args.debug_workdir = None
    # Set when the goal's processes or workdir may outlive it, so a suite
    # stops rather than run the next goal beside them.
    args.cleanup_failed = False

    # A missing executable is the first, unambiguous acceptance failure.
    if not args.exe.is_file():
        return _complete_early(
            evidence,
            plan.required_case_ids,
            1,
            "EXECUTABLE_MISSING",
            blocked=False,
            finalize_evidence=plan.finalize_evidence,
        )

    try:
        exe = args.exe.resolve(strict=False)
        win32, parent_context = plan.preflight(exe, args.expect_exe_sha256, evidence)
        ui_types = plan.ui_types_loader()
    except HarnessBlocked as error:
        return _complete_early(
            evidence,
            plan.required_case_ids,
            2,
            error.code,
            blocked=True,
            finalize_evidence=plan.finalize_evidence,
        )
    except HarnessFailure as error:
        return _complete_early(
            evidence,
            plan.required_case_ids,
            1,
            error.code,
            blocked=False,
            finalize_evidence=plan.finalize_evidence,
        )
    except Exception as error:
        return _complete_early(
            evidence,
            plan.required_case_ids,
            1,
            f"PREFLIGHT_{safe_exception_name(error).upper()}",
            blocked=False,
            finalize_evidence=plan.finalize_evidence,
        )

    try:
        root = Path(tempfile.mkdtemp(prefix=plan.workdir_prefix)).resolve()
    except OSError as error:
        return _complete_early(
            evidence,
            plan.required_case_ids,
            1,
            f"ISOLATION_{safe_exception_name(error).upper()}",
            blocked=False,
            finalize_evidence=plan.finalize_evidence,
        )

    selected_index = (
        plan.required_case_ids.index(args.case) if getattr(args, "case", None) else 0
    )
    keep_going = bool(getattr(args, "keep_going", False))
    current_index = selected_index
    harness: NativeHarness | None = None
    returncode, code = 1, "INTERNAL_NO_RESULT"
    try:
        try:
            copied_exe, copied = session.copy_executable(exe, root)
            evidence["executable"]["copied_sha256"] = copied.sha256
            evidence["executable"]["copy_hash_verified"] = (
                copied.sha256 == args.expect_exe_sha256
            )
            if not evidence["executable"]["copy_hash_verified"]:
                raise HarnessFailure("COPIED_EXECUTABLE_HASH_MISMATCH")
            harness = plan.harness_factory(
                copied_exe,
                root,
                args.ui_timeout,
                win32,
                *ui_types,
                parent_context,
            )
            scenarios = plan.scenarios(harness)
            if len(scenarios) != len(plan.required_case_ids):
                raise RuntimeError("scenario count does not match required cases")
        except HarnessBlocked as error:
            mark_remaining_cases(evidence, current_index, "BLOCKED", error.code)
            returncode, code = 2, error.code
        except HarnessFailure as error:
            mark_remaining_cases(evidence, current_index, "NOT_RUN", error.code)
            returncode, code = 1, error.code
        except OSError:
            code = "ISOLATION_COPY_FAILED"
            mark_remaining_cases(evidence, current_index, "NOT_RUN", code)
            returncode = 1
        except Exception as error:
            code = f"ISOLATION_{safe_exception_name(error).upper()}"
            mark_remaining_cases(evidence, current_index, "NOT_RUN", code)
            returncode = 1
        else:
            selected = (
                ((selected_index, scenarios[selected_index]),)
                if getattr(args, "case", None)
                else tuple(enumerate(scenarios))
            )
            # With `keep_going` a FAIL does not stop the later cases; the
            # first FAIL still decides the run. BLOCKED always stops.
            first_failure: str | None = None
            for current_index, scenario in selected:
                case = evidence["cases"][current_index]
                started = time.perf_counter()
                try:
                    case["observations"] = scenario()
                except HarnessBlocked as error:
                    case["observations"].update(error.diagnostics)
                    case.update(
                        status="BLOCKED",
                        reason_code=error.code,
                        duration_ms=round((time.perf_counter() - started) * 1000, 3),
                    )
                    mark_remaining_cases(
                        evidence, current_index + 1, "BLOCKED", "SKIPPED_AFTER_BLOCKED"
                    )
                    returncode, code = (1, first_failure) if first_failure else (2, error.code)
                    break
                except Exception as error:
                    failure_code = (
                        error.code if isinstance(error, HarnessFailure) else "INTERNAL_FAILURE"
                    )
                    if isinstance(error, HarnessFailure):
                        # Content-free failure diagnostics; the goal's contract
                        # validates them on evidence write.
                        case["observations"].update(error.observations)
                    case.update(
                        status="FAIL",
                        reason_code=failure_code,
                        failure_type=(
                            safe_failure_type(error)
                            if isinstance(error, HarnessFailure)
                            else safe_exception_name(error)
                        ),
                        duration_ms=round((time.perf_counter() - started) * 1000, 3),
                    )
                    first_failure = first_failure or failure_code
                    if keep_going:
                        continue
                    mark_remaining_cases(
                        evidence, current_index + 1, "NOT_RUN", "SKIPPED_AFTER_FAILURE"
                    )
                    returncode, code = 1, failure_code
                    break
                case.update(status="PASS", duration_ms=round((time.perf_counter() - started) * 1000, 3))
            else:
                if first_failure is not None:
                    returncode, code = 1, first_failure
                elif getattr(args, "case", None):
                    returncode, code = 1, "PARTIAL_CASE_RUN"
                else:
                    returncode, code = 0, "ALL_REQUIRED_CASES"
    finally:
        cleanup_succeeded = False
        try:
            if harness is not None:
                harness.cleanup()
            cleanup_succeeded = True
        except HarnessFailure as error:
            returncode, code = 1, error.code
            _record_cleanup_failure(
                evidence,
                plan.required_case_ids,
                current_index,
                error.code,
                safe_failure_type(error),
            )
        except Exception as error:
            returncode, code = 1, "CLEANUP_INTERNAL_FAILURE"
            _record_cleanup_failure(
                evidence,
                plan.required_case_ids,
                current_index,
                code,
                safe_exception_name(error),
            )

        args.cleanup_failed = not cleanup_succeeded
        if not cleanup_succeeded or (
            returncode != 0 and getattr(args, "keep_workdir_on_failure", False)
        ):
            args.debug_workdir = root
        else:
            try:
                remove_tree_with_retry(root)
            except OSError as error:
                returncode, code = 1, "ISOLATION_CLEANUP_FAILED"
                args.cleanup_failed = True
                _record_cleanup_failure(
                    evidence,
                    plan.required_case_ids,
                    current_index,
                    code,
                    safe_exception_name(error),
                )
                if root.exists():
                    args.debug_workdir = root

    if plan.finalize_evidence is not None:
        plan.finalize_evidence(evidence)
    complete_evidence(evidence, _completion_status(returncode), plan.required_case_ids)
    return returncode, evidence, code


def _persist_and_report(
    args: argparse.Namespace,
    plan: NativeRunPlan,
    returncode: int,
    evidence: dict[str, Any],
    code: str,
) -> int:
    try:
        write_evidence(args.evidence, evidence, plan.validate_evidence)
    except (OSError, ValueError):
        print("FAIL: EVIDENCE_WRITE_FAILED", file=sys.stderr)
        return 1
    status = _completion_status(returncode)
    stream = sys.stdout if returncode == 0 else sys.stderr
    print(f"{status}: {code}", file=stream)
    print(f"evidence: {args.evidence.resolve()}", file=stream)
    if getattr(args, "debug_workdir", None) is not None:
        print(f"debug workdir: {args.debug_workdir}", file=stream)
    return returncode


def main_native_acceptance(
    args: argparse.Namespace, plan: NativeRunPlan, session: NativeSession | None = None
) -> int:
    """Run, atomically persist, and report a native acceptance result."""
    returncode, evidence, code = run_native_acceptance(args, plan, session)
    return _persist_and_report(args, plan, returncode, evidence, code)


def main_native_acceptance_suite(
    runs: list[tuple[str, argparse.Namespace, NativeRunPlan]], *, keep_going: bool
) -> int:
    """Run several goals in one process and one shared `NativeSession`.

    The executable is hashed, the environment sampled, the clipboard saved and
    the executable copied once; each goal keeps its own workdir and writes its
    own evidence. `keep_going` applies to every goal's cases and across goals:
    a goal BLOCKED stops the later goals, and so does a goal FAIL unless
    `keep_going`; a goal whose cleanup failed (`args.cleanup_failed`) stops
    them regardless; a stopped goal still writes its evidence (every
    case BLOCKED or NOT_RUN with a SKIPPED_AFTER_* reason). The exit status is
    the worst goal status (`worst_returncode`); a session that cannot restore
    the clipboard fails the command without changing any goal's evidence.
    """
    session = NativeSession(shared=True)
    overall = 0
    stopped: tuple[int, str] | None = None
    try:
        for name, args, plan in runs:
            print(f"== {name}", flush=True)
            args.keep_going = keep_going
            if stopped is None:
                returncode = main_native_acceptance(args, plan, session)
                if getattr(args, "cleanup_failed", False):
                    stopped = (1, "SKIPPED_AFTER_CLEANUP_FAILURE")
                elif returncode == 2:
                    stopped = (2, "SKIPPED_AFTER_BLOCKED")
                elif returncode == 1 and not keep_going:
                    stopped = (1, "SKIPPED_AFTER_FAILURE")
            else:
                args.debug_workdir = None
                skipped, evidence, code = _complete_early(
                    plan.new_evidence(args.expect_exe_sha256),
                    plan.required_case_ids,
                    stopped[0],
                    stopped[1],
                    blocked=stopped[0] == 2,
                    finalize_evidence=plan.finalize_evidence,
                )
                returncode = _persist_and_report(args, plan, skipped, evidence, code)
            overall = worst_returncode(overall, returncode)
    finally:
        close_failure = session.close()
    if close_failure is not None:
        print(f"FAIL: {close_failure}", file=sys.stderr)
        overall = worst_returncode(overall, 1)
    print(f"overall: {_completion_status(overall)}", file=sys.stdout if overall == 0 else sys.stderr)
    return overall

__all__ = [
    "IMAGE_FILE_MACHINE_AMD64",
    "INTEGRITY_NAMES",
    "PE32_PLUS_MAGIC",
    "REASON_CODE_RE",
    "SAFE_FAILURE_TYPES",
    "SHA256_RE",
    "EDITOR_NAME",
    "VK_A",
    "VK_BACK",
    "VK_C",
    "VK_CONTROL",
    "VK_RETURN",
    "VK_S",
    "VK_W",
    "VK_Z",
    "Fingerprint",
    "HarnessBlocked",
    "HarnessFailure",
    "LaunchSpec",
    "NativeHarness",
    "NativeRunPlan",
    "NativeSession",
    "FOREGROUND_TIMEOUT_SECONDS",
    "INNER_WAIT_SECONDS",
    "METRIC_COUNTERS",
    "RunMetrics",
    "RunningApp",
    "SecurityContext",
    "Win32",
    "artifact_contains",
    "build_launch_spec",
    "clipboard_sequence_number",
    "complete_evidence",
    "count",
    "elapsed_ms",
    "executable_hash_failure",
    "finite_nonnegative",
    "fingerprint_bytes",
    "fingerprint_text",
    "inspect_pe",
    "inspect_pe_bytes",
    "key_input",
    "launch_failure_code",
    "load_pywinauto",
    "mark_remaining_cases",
    "main_native_acceptance",
    "normalize_expected_hash",
    "parse_outcome_line",
    "phase",
    "platform_preflight_failure",
    "preflight",
    "require_true",
    "safe_exception_name",
    "safe_failure_type",
    "security_context_failure",
    "sha256_file",
    "utc_now",
    "validate_environment",
    "validate_fingerprint",
    "validate_process_context",
    "wait_until",
    "write_durable",
    "write_evidence",
    "run_native_acceptance",
    "main_native_acceptance_suite",
    "read_clipboard_text",
    "write_clipboard_text",
    "worst_returncode",
]
