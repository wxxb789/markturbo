"""Shared Windows/UIA implementation for hash-bound native acceptance runs."""

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
from collections.abc import Callable
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
INTEGRITY_NAMES = {
    "untrusted",
    "low",
    "medium",
    "medium-plus",
    "high",
    "system",
    "protected",
}

LAYOUT_SOURCE_AUTOMATION_ID = "markturbo-layout-source"
SOURCE_EDITOR_AUTOMATION_ID = "markturbo-document-source-editor"
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
WINDOWS_LAUNCH_ERROR_CODES = {
    2: "PROCESS_LAUNCH_FILE_NOT_FOUND",
    3: "PROCESS_LAUNCH_PATH_NOT_FOUND",
    5: "PROCESS_LAUNCH_ACCESS_DENIED",
    193: "PROCESS_LAUNCH_BAD_EXE_FORMAT",
    740: "PROCESS_LAUNCH_ELEVATION_REQUIRED",
}

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
    source_contract: Callable[[], str | None] | None = None


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
    return LaunchSpec(
        args=(str(copied_exe),) if target is None else (str(copied_exe), str(target)),
        cwd=str(workspace_root),
        env=env,
        stderr_path=stderr_path,
    )


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
        show_window = bool(self.user32.ShowWindow(hwnd, SW_RESTORE))
        bring_to_top = bool(self.user32.BringWindowToTop(hwnd))
        set_foreground = bool(self.user32.SetForegroundWindow(hwnd))
        deadline = time.perf_counter() + timeout
        attempts = 0
        foreground = 0
        while time.perf_counter() < deadline:
            attempts += 1
            foreground = int(self.user32.GetForegroundWindow() or 0)
            if foreground == hwnd:
                return
            time.sleep(0.025)
        raise HarnessBlocked(
            "FOREGROUND_PERMISSION_DENIED",
            diagnostics={
                "requested_hwnd": hwnd,
                "foreground_hwnd": foreground,
                "show_window_return": show_window,
                "bring_to_top_return": bring_to_top,
                "set_foreground_return": set_foreground,
                "foreground_attempts": attempts,
            },
        )

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
        raise HarnessBlocked("WTS_SESSION_NOT_ACTIVE")
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

    def launch_app(
        self,
        target: Path | None,
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
            raise HarnessFailure(launch_failure_code(error), safe_exception_name(error)) from None
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
        self.win32.require_foreground(hwnd, self.ui_timeout)
        return RunningApp(
            process,
            window,
            hwnd,
            spec,
            child_context,
            data_root / "logs" / f"markturbo-{process.pid}.log",
        )
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


def _completion_status(returncode: int) -> str:
    return {0: "PASS", 1: "FAIL", 2: "BLOCKED"}[returncode]


def _complete_early(
    evidence: dict[str, Any],
    required_case_ids: tuple[str, ...],
    returncode: int,
    code: str,
    *,
    blocked: bool,
) -> tuple[int, dict[str, Any], str]:
    mark_remaining_cases(
        evidence,
        0,
        "BLOCKED" if blocked else "NOT_RUN",
        code,
    )
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


def run_native_acceptance(
    args: argparse.Namespace, plan: NativeRunPlan
) -> tuple[int, dict[str, Any], str]:
    """Run one goal's native cases with one fail-closed lifecycle."""
    evidence = plan.new_evidence(args.expect_exe_sha256)
    args.debug_workdir = None

    if plan.source_contract is not None:
        try:
            if code := plan.source_contract():
                return _complete_early(evidence, plan.required_case_ids, 1, code, blocked=False)
        except Exception as error:
            return _complete_early(
                evidence,
                plan.required_case_ids,
                1,
                f"PREFLIGHT_{safe_exception_name(error).upper()}",
                blocked=False,
            )

    try:
        exe = args.exe.resolve(strict=False)
        win32, parent_context = plan.preflight(exe, args.expect_exe_sha256, evidence)
        ui_types = plan.ui_types_loader()
    except HarnessBlocked as error:
        return _complete_early(evidence, plan.required_case_ids, 2, error.code, blocked=True)
    except HarnessFailure as error:
        return _complete_early(evidence, plan.required_case_ids, 1, error.code, blocked=False)
    except Exception as error:
        return _complete_early(
            evidence,
            plan.required_case_ids,
            1,
            f"PREFLIGHT_{safe_exception_name(error).upper()}",
            blocked=False,
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
        )

    selected_index = (
        plan.required_case_ids.index(args.case) if getattr(args, "case", None) else 0
    )
    current_index = selected_index
    harness: NativeHarness | None = None
    returncode, code = 1, "INTERNAL_NO_RESULT"
    try:
        try:
            bin_root = root / "bin"
            bin_root.mkdir()
            copied_exe = Path(shutil.copy2(exe, bin_root / exe.name)).resolve()
            copied = sha256_file(copied_exe)
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
                    returncode, code = 2, error.code
                    break
                except HarnessFailure as error:
                    case.update(
                        status="FAIL",
                        reason_code=error.code,
                        failure_type=safe_failure_type(error),
                        duration_ms=round((time.perf_counter() - started) * 1000, 3),
                    )
                    mark_remaining_cases(
                        evidence, current_index + 1, "NOT_RUN", "SKIPPED_AFTER_FAILURE"
                    )
                    returncode, code = 1, error.code
                    break
                except Exception as error:
                    case.update(
                        status="FAIL",
                        reason_code="INTERNAL_FAILURE",
                        failure_type=safe_exception_name(error),
                        duration_ms=round((time.perf_counter() - started) * 1000, 3),
                    )
                    mark_remaining_cases(
                        evidence, current_index + 1, "NOT_RUN", "SKIPPED_AFTER_FAILURE"
                    )
                    returncode, code = 1, "INTERNAL_FAILURE"
                    break
                case.update(status="PASS", duration_ms=round((time.perf_counter() - started) * 1000, 3))
            else:
                returncode, code = (
                    (1, "PARTIAL_CASE_RUN")
                    if getattr(args, "case", None)
                    else (0, "ALL_REQUIRED_CASES")
                )
    finally:
        try:
            if harness is not None:
                harness.cleanup()
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

        if returncode != 0 and getattr(args, "keep_workdir_on_failure", False):
            args.debug_workdir = root
        else:
            try:
                shutil.rmtree(root)
            except OSError as error:
                returncode, code = 1, "ISOLATION_CLEANUP_FAILED"
                _record_cleanup_failure(
                    evidence,
                    plan.required_case_ids,
                    current_index,
                    code,
                    safe_exception_name(error),
                )
                if root.exists():
                    args.debug_workdir = root

    complete_evidence(evidence, _completion_status(returncode), plan.required_case_ids)
    return returncode, evidence, code


def main_native_acceptance(args: argparse.Namespace, plan: NativeRunPlan) -> int:
    """Run, atomically persist, and report a native acceptance result."""
    returncode, evidence, code = run_native_acceptance(args, plan)
    try:
        write_evidence(args.evidence, evidence, plan.validate_evidence)
    except (OSError, ValueError):
        print("FAIL: EVIDENCE_WRITE_FAILED", file=sys.stderr)
        return 1
    status = _completion_status(returncode)
    stream = sys.stdout if returncode == 0 else sys.stderr
    print(f"{status}: {code}", file=stream)
    print(f"evidence: {args.evidence.resolve()}", file=stream)
    if args.debug_workdir is not None:
        print(f"debug workdir: {args.debug_workdir}", file=stream)
    return returncode

__all__ = [
    "IMAGE_FILE_MACHINE_AMD64",
    "INTEGRITY_NAMES",
    "PE32_PLUS_MAGIC",
    "REASON_CODE_RE",
    "SAFE_FAILURE_TYPES",
    "SHA256_RE",
    "SOURCE_EDITOR_AUTOMATION_ID",
    "VK_A",
    "VK_BACK",
    "VK_CONTROL",
    "VK_S",
    "VK_W",
    "Fingerprint",
    "HarnessBlocked",
    "HarnessFailure",
    "LaunchSpec",
    "NativeHarness",
    "NativeRunPlan",
    "RunningApp",
    "SecurityContext",
    "Win32",
    "build_launch_spec",
    "complete_evidence",
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
]
