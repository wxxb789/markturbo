"""Measure and inspect a running markturbo window.

The harness behind this repository's performance work. Everything it reports is
read from a real process on a real window — no estimates.

    python -m scripts.markturbo_tools.probe memory
    python -m scripts.markturbo_tools.probe startup

Why Python rather than PowerShell: this needs Win32 for process counters, child
window enumeration and hit testing, and `ctypes` reaches all of it without the
assembly-loading differences between PowerShell 5 and 7.

Windows only. Milestone startup measurement and `shot` require an active,
unlocked input desktop; the remaining subcommands work headless.
"""

from __future__ import annotations

import argparse
import ctypes
import ctypes.wintypes as wt
import os
import re
import shutil
import subprocess
import sys
import tempfile
import time
import uuid
from collections import deque
from contextlib import ExitStack, nullcontext
from dataclasses import dataclass
from datetime import UTC, datetime
from pathlib import Path
from statistics import median

from .metrics import inclusive_p95, measure_abba, nearest_rank_percentile
from .goal04 import (
    EVIDENCE_VARIANT_LABELS,
    GOAL04_BUILD_VARIANTS,
    GOAL04_TARGET,
    MODEL_FIRST_USE_EVIDENCE_SCHEMA,
    MODEL_TRANSPORT_DECISIONS,
    MODEL_TRANSPORT_DECISION_SCHEMA,
    QUIET_EVIDENCE_MAX_AGE,
    STARTUP_BUILD_SCHEMA,
    STARTUP_EVIDENCE_SCHEMA,
    STARTUP_QUIET_SCHEMA,
    STARTUP_THRESHOLD_SCHEMA,
    STARTUP_TRACE_EVENTS,
    STARTUP_TRACE_SCHEMA,
    StartupSample,
    StartupTraceReader,
    canonical_threshold_evidence,
    cmd_build_goal04,
    cmd_decide_goal04,
    evaluate_model_threshold,
    goal04_behavior,
    goal04_behavior_verification_command,
    goal04_bloat_command,
    goal04_build_command,
    goal04_host_context,
    goal04_platform_setup,
    goal04_release_profile,
    goal04_tokio_disposition,
    goal04_tree_command,
    load_build_evidence,
    load_threshold_evidence,
    measure_startup_abba,
    milestone_comparison,
    model_first_use_cache_state,
    normalize_goal04_bloat_crates,
    normalized_quiet_evidence,
    parse_goal04_dependency_graph,
    parse_startup_trace,
    quiet_gate_failures,
    read_evidence_object,
    require_distinct_output_path,
    safe_command,
    source_state,
    startup_summary,
    summarize_startup_milestones,
    trace_milestones,
    validate_build_evidence,
    validate_model_first_use_evidence,
    validate_model_transport_decision_evidence,
    validate_quiet_evidence,
    validate_startup_evidence,
    validate_startup_quiet_evidence,
    validate_threshold_evidence,
    write_model_first_use_evidence,
    write_startup_evidence,
)

REPO = Path(__file__).resolve().parents[2]
DEFAULT_EXE = REPO / "target" / "release" / "markturbo.exe"
MAIN_WINDOW_TITLE = "markturbo"
DEFAULT_FORBIDDEN_LOG_SUBSTRINGS = ("RefCell already borrowed",)
WM_CLOSE = 0x0010
SW_RESTORE = 9
SWP_NOZORDER = 0x0004
SWP_NOACTIVATE = 0x0010
VK_F24 = 0x87

if sys.platform == "win32":
    psapi = ctypes.WinDLL("psapi", use_last_error=True)
    kernel32 = ctypes.WinDLL("kernel32", use_last_error=True)
    user32 = ctypes.WinDLL("user32", use_last_error=True)
    pdh = ctypes.WinDLL("pdh", use_last_error=True)
else:
    # Keep geometry and parsing helpers importable for cross-platform tests.
    psapi = kernel32 = user32 = pdh = None


class PROCESS_MEMORY_COUNTERS(ctypes.Structure):
    _fields_ = [
        ("cb", wt.DWORD),
        ("PageFaultCount", wt.DWORD),
        ("PeakWorkingSetSize", ctypes.c_size_t),
        ("WorkingSetSize", ctypes.c_size_t),
        ("QuotaPeakPagedPoolUsage", ctypes.c_size_t),
        ("QuotaPagedPoolUsage", ctypes.c_size_t),
        ("QuotaPeakNonPagedPoolUsage", ctypes.c_size_t),
        ("QuotaNonPagedPoolUsage", ctypes.c_size_t),
        ("PagefileUsage", ctypes.c_size_t),
        ("PeakPagefileUsage", ctypes.c_size_t),
    ]


class FILETIME(ctypes.Structure):
    _fields_ = [("dwLowDateTime", wt.DWORD), ("dwHighDateTime", wt.DWORD)]


class PDH_FMT_COUNTERVALUE_UNION(ctypes.Union):
    _fields_ = [("longValue", ctypes.c_long), ("doubleValue", ctypes.c_double)]


class PDH_FMT_COUNTERVALUE(ctypes.Structure):
    _anonymous_ = ("value",)
    _fields_ = [("CStatus", wt.DWORD), ("value", PDH_FMT_COUNTERVALUE_UNION)]


class RECT(ctypes.Structure):
    _fields_ = [("left", ctypes.c_long), ("top", ctypes.c_long),
                ("right", ctypes.c_long), ("bottom", ctypes.c_long)]


class POINT(ctypes.Structure):
    _fields_ = [("x", ctypes.c_long), ("y", ctypes.c_long)]


@dataclass(frozen=True)
class ProcessMemory:
    working_set_mb: float
    private_mb: float
    peak_working_set_mb: float
    page_faults: int


if sys.platform == "win32":
    user32.GetClientRect.argtypes = [wt.HWND, ctypes.POINTER(RECT)]
    user32.GetClientRect.restype = wt.BOOL
    user32.ClientToScreen.argtypes = [wt.HWND, ctypes.POINTER(POINT)]
    user32.ClientToScreen.restype = wt.BOOL
    user32.SetWindowPos.argtypes = [
        wt.HWND, wt.HWND, ctypes.c_int, ctypes.c_int,
        ctypes.c_int, ctypes.c_int, wt.UINT,
    ]
    user32.SetWindowPos.restype = wt.BOOL
    kernel32.GetSystemTimes.argtypes = [
        ctypes.POINTER(FILETIME), ctypes.POINTER(FILETIME), ctypes.POINTER(FILETIME)
    ]
    kernel32.GetSystemTimes.restype = wt.BOOL
    kernel32.QueryPerformanceCounter.argtypes = [ctypes.POINTER(ctypes.c_longlong)]
    kernel32.QueryPerformanceCounter.restype = wt.BOOL
    kernel32.QueryPerformanceFrequency.argtypes = [ctypes.POINTER(ctypes.c_longlong)]
    kernel32.QueryPerformanceFrequency.restype = wt.BOOL
    pdh.PdhOpenQueryW.argtypes = [wt.LPCWSTR, ctypes.c_size_t, ctypes.POINTER(wt.HANDLE)]
    pdh.PdhOpenQueryW.restype = ctypes.c_long
    pdh.PdhAddEnglishCounterW.argtypes = [
        wt.HANDLE, wt.LPCWSTR, ctypes.c_size_t, ctypes.POINTER(wt.HANDLE)
    ]
    pdh.PdhAddEnglishCounterW.restype = ctypes.c_long
    pdh.PdhCollectQueryData.argtypes = [wt.HANDLE]
    pdh.PdhCollectQueryData.restype = ctypes.c_long
    pdh.PdhGetFormattedCounterValue.argtypes = [
        wt.HANDLE, wt.DWORD, ctypes.POINTER(wt.DWORD),
        ctypes.POINTER(PDH_FMT_COUNTERVALUE),
    ]
    pdh.PdhGetFormattedCounterValue.restype = ctypes.c_long
    pdh.PdhCloseQuery.argtypes = [wt.HANDLE]
    pdh.PdhCloseQuery.restype = ctypes.c_long
    user32.PostMessageW.argtypes = [wt.HWND, wt.UINT, wt.WPARAM, wt.LPARAM]
    user32.PostMessageW.restype = wt.BOOL
    user32.ShowWindow.argtypes = [wt.HWND, ctypes.c_int]
    user32.ShowWindow.restype = wt.BOOL

PDH_FMT_DOUBLE = 0x00000200


PROCESS_QUERY_INFORMATION = 0x0400
PROCESS_VM_READ = 0x0010
MB = 1024 * 1024


def _open(pid: int) -> int:
    h = kernel32.OpenProcess(PROCESS_QUERY_INFORMATION | PROCESS_VM_READ, False, pid)
    if not h:
        raise OSError(ctypes.get_last_error(), f"OpenProcess({pid})")
    return h


def process_memory(pid: int) -> ProcessMemory:
    """Working set, private bytes, peak working set and page faults."""
    h = _open(pid)
    try:
        c = PROCESS_MEMORY_COUNTERS()
        c.cb = ctypes.sizeof(c)
        if not psapi.GetProcessMemoryInfo(h, ctypes.byref(c), c.cb):
            raise OSError(ctypes.get_last_error(), "GetProcessMemoryInfo")
        return ProcessMemory(
            c.WorkingSetSize / MB,
            c.PagefileUsage / MB,
            c.PeakWorkingSetSize / MB,
            int(c.PageFaultCount),
        )
    finally:
        kernel32.CloseHandle(h)


def memory(pid: int) -> tuple[float, float, float]:
    """Working set, private bytes and peak working set, in MB."""
    sample = process_memory(pid)
    return sample.working_set_mb, sample.private_mb, sample.peak_working_set_mb


def performance_counter() -> int:
    value = ctypes.c_longlong()
    if not kernel32.QueryPerformanceCounter(ctypes.byref(value)):
        raise OSError(ctypes.get_last_error(), "QueryPerformanceCounter")
    return int(value.value)


def performance_frequency() -> int:
    value = ctypes.c_longlong()
    if not kernel32.QueryPerformanceFrequency(ctypes.byref(value)):
        raise OSError(ctypes.get_last_error(), "QueryPerformanceFrequency")
    return int(value.value)


def cpu_seconds(pid: int) -> float:
    """Total kernel + user CPU time consumed, in seconds."""
    h = _open(pid)
    try:
        creation, exit_, kernel, user = FILETIME(), FILETIME(), FILETIME(), FILETIME()
        if not kernel32.GetProcessTimes(
            h, ctypes.byref(creation), ctypes.byref(exit_),
            ctypes.byref(kernel), ctypes.byref(user)
        ):
            raise OSError(ctypes.get_last_error(), "GetProcessTimes")
        ticks = ((kernel.dwHighDateTime << 32) | kernel.dwLowDateTime) + \
                ((user.dwHighDateTime << 32) | user.dwLowDateTime)
        return ticks / 1e7  # 100ns units
    finally:
        kernel32.CloseHandle(h)


def _filetime_ticks(value: FILETIME) -> int:
    return (value.dwHighDateTime << 32) | value.dwLowDateTime


def system_cpu_times() -> tuple[int, int, int]:
    """System idle, kernel, and user time in 100ns ticks."""
    idle, kernel, user = FILETIME(), FILETIME(), FILETIME()
    if not kernel32.GetSystemTimes(
        ctypes.byref(idle), ctypes.byref(kernel), ctypes.byref(user)
    ):
        raise OSError(ctypes.get_last_error(), "GetSystemTimes")
    return tuple(map(_filetime_ticks, (idle, kernel, user)))


class DiskBusyCounter:
    """Low-overhead PDH reader for total physical-disk busy time."""

    def __init__(self) -> None:
        self.query = wt.HANDLE()
        self.counter = wt.HANDLE()
        status = pdh.PdhOpenQueryW(None, 0, ctypes.byref(self.query))
        if status:
            raise OSError(status, "PdhOpenQueryW")
        status = pdh.PdhAddEnglishCounterW(
            self.query,
            r"\PhysicalDisk(_Total)\% Disk Time",
            0,
            ctypes.byref(self.counter),
        )
        if status:
            pdh.PdhCloseQuery(self.query)
            raise OSError(status, "PdhAddEnglishCounterW")
        status = pdh.PdhCollectQueryData(self.query)
        if status:
            self.close()
            raise OSError(status, "PdhCollectQueryData")

    def sample(self) -> float:
        status = pdh.PdhCollectQueryData(self.query)
        if status:
            raise OSError(status, "PdhCollectQueryData")
        value = PDH_FMT_COUNTERVALUE()
        status = pdh.PdhGetFormattedCounterValue(
            self.counter, PDH_FMT_DOUBLE, None, ctypes.byref(value)
        )
        if status:
            raise OSError(status, "PdhGetFormattedCounterValue")
        return max(0.0, value.doubleValue)

    def close(self) -> None:
        if self.query:
            pdh.PdhCloseQuery(self.query)
            self.query = wt.HANDLE()

    def __enter__(self) -> "DiskBusyCounter":
        return self

    def __exit__(self, *_: object) -> None:
        self.close()


def thread_count(pid: int) -> int:
    """Threads in the process, via a toolhelp snapshot."""
    TH32CS_SNAPTHREAD = 0x00000004

    class THREADENTRY32(ctypes.Structure):
        _fields_ = [("dwSize", wt.DWORD), ("cntUsage", wt.DWORD),
                    ("th32ThreadID", wt.DWORD), ("th32OwnerProcessID", wt.DWORD),
                    ("tpBasePri", ctypes.c_long), ("tpDeltaPri", ctypes.c_long),
                    ("dwFlags", wt.DWORD)]

    snap = kernel32.CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0)
    if snap == -1:
        return -1
    try:
        e = THREADENTRY32()
        e.dwSize = ctypes.sizeof(e)
        n = 0
        ok = kernel32.Thread32First(snap, ctypes.byref(e))
        while ok:
            if e.th32OwnerProcessID == pid:
                n += 1
            ok = kernel32.Thread32Next(snap, ctypes.byref(e))
        return n
    finally:
        kernel32.CloseHandle(snap)


def main_window(pid: int) -> int | None:
    """The process's titled main window, or None if it has none yet.

    GPUI also owns visible helper windows, and `EnumWindows` reaches them in
    Z-order. The product title is the stable discriminator; area is only a
    tie-breaker if a broken build creates two windows with the same title.
    """
    best: list[tuple[int, int]] = []

    @ctypes.WINFUNCTYPE(wt.BOOL, wt.HWND, wt.LPARAM)
    def cb(hwnd, _):
        owner = wt.DWORD()
        user32.GetWindowThreadProcessId(hwnd, ctypes.byref(owner))
        if (owner.value == pid and user32.IsWindowVisible(hwnd)
                and window_text(hwnd) == MAIN_WINDOW_TITLE):
            r = window_rect(hwnd)
            area = (r.right - r.left) * (r.bottom - r.top)
            if area > 0:
                best.append((area, hwnd))
        return True

    user32.EnumWindows(cb, 0)
    return max(best)[1] if best else None


def top_windows(pid: int) -> list[int]:
    """Every non-message top-level window owned by the process.

    Hidden and zero-sized windows count: a companion window that happens not to
    be showing is still a second application window and fails single-window
    acceptance.
    """
    found: list[int] = []

    @ctypes.WINFUNCTYPE(wt.BOOL, wt.HWND, wt.LPARAM)
    def cb(hwnd, _):
        owner = wt.DWORD()
        user32.GetWindowThreadProcessId(hwnd, ctypes.byref(owner))
        if owner.value == pid:
            found.append(hwnd)
        return True

    user32.EnumWindows(cb, 0)
    main = main_window(pid)

    def sort_key(hwnd: int) -> tuple[bool, int]:
        width, height = rect_size(window_rect(hwnd))
        return hwnd == main, max(width, 0) * max(height, 0)

    return sorted(
        found,
        key=sort_key,
        reverse=True,
    )


def is_system_input_helper(hwnd: int) -> bool:
    """Whether Windows created a non-product IME helper for this process.

    Text Services Framework creates hidden, zero-sized `IME` and `MSCTFIME UI`
    top-level HWNDs on behalf of GUI processes. They are not application
    windows; a hidden companion preview still has a product class or non-zero
    geometry and is deliberately not exempted.
    """
    width, height = rect_size(window_rect(hwnd))
    return (
        class_name(hwnd) in {"IME", "MSCTFIME UI"}
        and not user32.IsWindowVisible(hwnd)
        and width == 0
        and height == 0
    )


def window_rect(hwnd: int) -> RECT:
    r = RECT()
    user32.GetWindowRect(hwnd, ctypes.byref(r))
    return r


def client_rect(hwnd: int) -> RECT:
    """The client rectangle in screen coordinates."""
    r = RECT()
    if not user32.GetClientRect(hwnd, ctypes.byref(r)):
        raise OSError(ctypes.get_last_error(), "GetClientRect")
    top_left = POINT(r.left, r.top)
    bottom_right = POINT(r.right, r.bottom)
    if not user32.ClientToScreen(hwnd, ctypes.byref(top_left)):
        raise OSError(ctypes.get_last_error(), "ClientToScreen(top-left)")
    if not user32.ClientToScreen(hwnd, ctypes.byref(bottom_right)):
        raise OSError(ctypes.get_last_error(), "ClientToScreen(bottom-right)")
    return RECT(top_left.x, top_left.y, bottom_right.x, bottom_right.y)


def class_name(hwnd: int) -> str:
    buf = ctypes.create_unicode_buffer(256)
    user32.GetClassNameW(hwnd, buf, 256)
    return buf.value


def window_text(hwnd: int) -> str:
    length = user32.GetWindowTextLengthW(hwnd)
    buf = ctypes.create_unicode_buffer(length + 1)
    user32.GetWindowTextW(hwnd, buf, len(buf))
    return buf.value


def child_windows(hwnd: int) -> list[tuple[int, str, RECT, bool]]:
    """Every child window, with its class, screen rect and visibility.

    The WebView is an OS child window rather than a GPUI element, so this is how
    you see where it actually is — and, with `hit_test`, what it covers.
    """
    out: list[tuple[int, str, RECT, bool]] = []

    @ctypes.WINFUNCTYPE(wt.BOOL, wt.HWND, wt.LPARAM)
    def cb(child, _):
        out.append((child, class_name(child), window_rect(child),
                    bool(user32.IsWindowVisible(child))))
        return True

    user32.EnumChildWindows(hwnd, cb, 0)
    return out


def rect_size(rect: RECT) -> tuple[int, int]:
    return rect.right - rect.left, rect.bottom - rect.top


def rect_tuple(rect: RECT) -> tuple[int, int, int, int]:
    return rect.left, rect.top, rect.right, rect.bottom


def rect_contains(outer: RECT, inner: RECT) -> bool:
    return (
        inner.left >= outer.left
        and inner.top >= outer.top
        and inner.right <= outer.right
        and inner.bottom <= outer.bottom
    )


def usable_child_rects(
    children: list[tuple[int, str, RECT, bool]],
    expected_class: str,
    parent_client: RECT,
) -> list[tuple[int, int, int, int]]:
    """Visible, non-empty expected children fully inside the client area."""
    usable = []
    for _, cls, rect, visible in children:
        width, height = rect_size(rect)
        if (
            cls == expected_class
            and visible
            and width > 0
            and height > 0
            and rect_contains(parent_client, rect)
        ):
            usable.append(rect_tuple(rect))
    return sorted(usable)


def expected_child_failures(
    children: list[tuple[int, str, RECT, bool]],
    expected_classes: list[str],
    parent_client: RECT,
    require_native_chrome_insets: bool = False,
) -> list[str]:
    failures = []
    for expected in expected_classes:
        matching = [item for item in children if item[1] == expected]
        usable = usable_child_rects(children, expected, parent_client)
        if usable and require_native_chrome_insets:
            inset_children = [
                rect
                for rect in usable
                if rect[1] > parent_client.top and rect[3] < parent_client.bottom
            ]
            if not inset_children:
                observed = ", ".join(
                    f"top={top - parent_client.top},"
                    f"bottom={parent_client.bottom - bottom}"
                    for _, top, _, bottom in usable
                )
                failures.append(
                    f"child class {expected!r} leaves no positive top and bottom "
                    f"native-chrome insets ({observed})"
                )
                continue
        if usable:
            continue
        if not matching:
            failures.append(f"expected child class {expected!r}, not found")
            continue
        failures.append(
            f"child class {expected!r} exists but none are visible, non-zero, "
            "and contained by the main client rectangle"
        )
    return failures


def hit_test(x: int, y: int) -> str:
    """Which window owns the pixel at (x, y) — i.e. who gets the click."""
    p = POINT(x, y)
    return class_name(user32.WindowFromPoint(p))


def launch(exe: Path, target: str | None, settle: float,
           log: Path | None = None,
           env: dict[str, str] | None = None) -> subprocess.Popen:
    args = [str(exe)] + ([target] if target else [])
    err = open(log, "wb") if log else None
    try:
        p = subprocess.Popen(
            args,
            stdout=subprocess.DEVNULL,
            stderr=err if err is not None else subprocess.DEVNULL,
            env=env,
        )
    finally:
        if err is not None:
            err.close()
    time.sleep(settle)
    if p.poll() is not None:
        sys.exit(f"markturbo exited with {p.returncode} before settling")
    return p


def kill_and_wait(p: subprocess.Popen, timeout: float = 5.0) -> int:
    """Terminate a probe process and always reap its handle."""
    if p.poll() is None:
        p.kill()
    try:
        return p.wait(timeout=timeout)
    except subprocess.TimeoutExpired:
        raise RuntimeError("probe process did not exit after termination") from None


def graceful_close(
    p: subprocess.Popen,
    hwnd: int,
    timeout: float,
) -> str | None:
    """Close through the real window lifecycle; return a failure description."""
    if p.poll() is not None:
        return f"process exited with {p.returncode} before WM_CLOSE"
    if not user32.PostMessageW(hwnd, WM_CLOSE, 0, 0):
        return f"PostMessageW(WM_CLOSE) failed: {ctypes.get_last_error()}"
    try:
        returncode = p.wait(timeout=timeout)
    except subprocess.TimeoutExpired:
        kill_and_wait(p)
        return f"process did not exit within {timeout:g}s after WM_CLOSE"
    if returncode != 0:
        return f"process exited with {returncode} after WM_CLOSE"
    return None


def log_content_failures(
    path: Path,
    forbidden: tuple[str, ...],
    *,
    required: tuple[str, ...] = (),
    label: str,
) -> list[str]:
    try:
        contents = path.read_text(encoding="utf-8", errors="replace")
    except OSError as error:
        return [f"could not read {label} {path}: {error}"]
    failures = [
        f"{label} contains forbidden substring {substring!r} "
        f"({contents.count(substring)} occurrence(s))"
        for substring in forbidden
        if substring in contents
    ]
    failures.extend(
        f"{label} does not contain required substring {substring!r}"
        for substring in required
        if substring not in contents
    )
    return failures


def child_geometry(
    children: list[tuple[int, str, RECT, bool]],
    expected_classes: list[str],
    parent_client: RECT,
) -> dict[str, list[tuple[int, int, int, int]]]:
    return {
        expected: usable_child_rects(children, expected, parent_client)
        for expected in expected_classes
    }


def wait_for_resized_layout(
    hwnd: int,
    target_size: tuple[int, int],
    expected_classes: list[str],
    previous_geometry: dict[str, list[tuple[int, int, int, int]]],
    timeout: float,
    require_native_chrome_insets: bool,
) -> tuple[
    list[tuple[int, str, RECT, bool]] | None,
    dict[str, list[tuple[int, int, int, int]]],
    str | None,
]:
    """Wait until both the host and at least one expected child have resized."""
    deadline = time.monotonic() + timeout
    last_size = rect_size(window_rect(hwnd))
    last_child_failures: list[str] = []
    last_geometry = previous_geometry
    while time.monotonic() < deadline:
        last_size = rect_size(window_rect(hwnd))
        parent_client = client_rect(hwnd)
        children = child_windows(hwnd)
        last_child_failures = expected_child_failures(
            children,
            expected_classes,
            parent_client,
            require_native_chrome_insets,
        )
        last_geometry = child_geometry(children, expected_classes, parent_client)
        child_changed = not expected_classes or any(
            last_geometry.get(expected) != previous_geometry.get(expected)
            for expected in expected_classes
        )
        if last_size == target_size and not last_child_failures and child_changed:
            return children, last_geometry, None
        time.sleep(0.02)
    detail = "; ".join(last_child_failures) if last_child_failures else (
        "expected child bounds did not change"
    )
    return None, last_geometry, (
        f"timed out waiting for resize {target_size[0]}x{target_size[1]}; "
        f"main is {last_size[0]}x{last_size[1]}; {detail}"
    )


def run_resize_cycles(
    hwnd: int,
    expected_classes: list[str],
    cycles: int,
    timeout: float,
    require_native_chrome_insets: bool,
) -> list[str]:
    """Exercise host-to-WebView Bounds updates and verify usable child geometry."""
    if cycles == 0:
        return []

    user32.ShowWindow(hwnd, SW_RESTORE)
    time.sleep(0.1)
    original = window_rect(hwnd)
    original_size = rect_size(original)
    parent_client = client_rect(hwnd)
    children = child_windows(hwnd)
    failures = expected_child_failures(
        children,
        expected_classes,
        parent_client,
        require_native_chrome_insets,
    )
    if failures:
        return failures
    geometry = child_geometry(children, expected_classes, parent_client)

    width, height = original_size
    alternate = (
        width - 160 if width > 800 else width + 160,
        height - 120 if height > 600 else height + 120,
    )
    for cycle in range(1, cycles + 1):
        for phase, target_size in (("alternate", alternate), ("restore", original_size)):
            if not user32.SetWindowPos(
                hwnd, 0, original.left, original.top,
                target_size[0], target_size[1],
                SWP_NOZORDER | SWP_NOACTIVATE,
            ):
                failures.append(
                    f"resize cycle {cycle} {phase}: SetWindowPos failed: "
                    f"{ctypes.get_last_error()}"
                )
                return failures
            _, next_geometry, failure = wait_for_resized_layout(
                hwnd,
                target_size,
                expected_classes,
                geometry,
                timeout,
                require_native_chrome_insets,
            )
            if failure is not None:
                failures.append(f"resize cycle {cycle} {phase}: {failure}")
                return failures
            geometry = next_geometry
            print(
                f"resize cycle {cycle} {phase}: "
                f"{target_size[0]}x{target_size[1]} PASS"
            )
    return failures


def startup_once(exe: Path, target: str | None, timeout: float) -> float:
    """Milliseconds from process creation until the titled window is visible."""
    args = [str(exe)] + ([target] if target else [])
    start = time.perf_counter()
    p = subprocess.Popen(args, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    try:
        deadline = start + timeout
        while time.perf_counter() < deadline:
            if p.poll() is not None:
                sys.exit(f"{exe} exited with {p.returncode} before opening a window")
            hwnd = main_window(p.pid)
            if hwnd is not None:
                return (time.perf_counter() - start) * 1000
            time.sleep(0.005)
        sys.exit(f"{exe} did not open a titled window within {timeout:g}s")
    finally:
        kill_and_wait(p)


def startup_milestones_once(
    exe: Path,
    target: str | None,
    timeout: float,
    *,
    win32: object,
    parent_context: object,
    idle_settle: float,
    profile_root: Path | None = None,
) -> StartupSample:
    """Measure named startup milestones with app-side QPC acknowledgements."""
    from .native.runtime import security_context_failure

    frequency = performance_frequency()
    nonce = uuid.uuid4().hex
    root_context = (
        tempfile.TemporaryDirectory(prefix="markturbo-startup-")
        if profile_root is None
        else nullcontext(profile_root)
    )
    with root_context as directory:
        root = Path(directory).resolve()
        trace_path = root / f"startup-{nonce}.jsonl"
        data_root = root / "data"
        config_root = root / "config"
        data_root.mkdir(exist_ok=True)
        config_root.mkdir(exist_ok=True)
        env = os.environ.copy()
        for secret in ("ANTHROPIC_API_KEY", "OPENAI_API_KEY", "MARKTURBO_TRANSLATE_MODEL"):
            env.pop(secret, None)
        env.update(
            {
                "MARKTURBO_DATA_DIR": str(data_root),
                "MARKTURBO_CONFIG_DIR": str(config_root),
                "MARKTURBO_STARTUP_TRACE": str(trace_path),
                "MARKTURBO_STARTUP_NONCE": nonce,
                "RUST_LOG": "warn",
            }
        )
        args = [str(exe)] + ([target] if target else [])
        trace = StartupTraceReader(
            trace_path,
            nonce=nonce,
            pid=0,
            frequency=frequency,
        )
        start_counter = performance_counter()
        p = subprocess.Popen(args, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, env=env)
        trace.pid = p.pid
        process_created = performance_counter()
        try:
            child_context = win32.security_context(p.pid)
            failure = security_context_failure(parent_context, child_context)
            if failure is not None:
                raise RuntimeError(failure)

            deadline = time.perf_counter() + timeout
            hwnd = None
            visible_counter = None
            input_sent = False
            events: dict[str, StartupTraceEvent] = {}
            while time.perf_counter() < deadline:
                if p.poll() is not None:
                    raise RuntimeError(
                        f"{exe} exited with {p.returncode} before completing startup milestones"
                    )
                if hwnd is None:
                    hwnd = main_window(p.pid)
                    if hwnd is not None:
                        visible_counter = performance_counter()
                events = trace.read()
                if (
                    not input_sent
                    and hwnd is not None
                    and "first_frame_painted" in events
                ):
                    win32.send_key(hwnd, VK_F24)
                    input_sent = True
                if input_sent and "first_input_handled" in events:
                    break
                time.sleep(0.002)
            else:
                missing = [name for name in STARTUP_TRACE_EVENTS if name not in events]
                if hwnd is None:
                    missing.append("window_visible")
                raise RuntimeError(
                    f"{exe} did not complete startup milestones within {timeout:g}s; "
                    f"missing {', '.join(missing) or 'input acknowledgement'}"
                )

            if visible_counter is None:
                raise RuntimeError(f"{exe} never exposed its titled main window")
            milestones = trace_milestones(
                events,
                start_counter=start_counter,
                frequency=frequency,
            )
            initial_state = events["initial_state_ready"].detail
            if initial_state not in {"welcome", "workspace", "bare"}:
                raise RuntimeError(f"invalid initial startup state: {initial_state!r}")
            time.sleep(idle_settle)
            idle = process_memory(p.pid)
            return StartupSample(
                process_created_ms=(process_created - start_counter) / frequency * 1000,
                process_started_ms=milestones["process_started_ms"],
                initial_state_ready_ms=milestones["initial_state_ready_ms"],
                window_visible_ms=(visible_counter - start_counter) / frequency * 1000,
                first_frame_painted_ms=milestones["first_frame_painted_ms"],
                first_input_handled_ms=milestones["first_input_handled_ms"],
                initial_state=initial_state,
                idle_working_set_mb=idle.working_set_mb,
                idle_private_mb=idle.private_mb,
                peak_working_set_mb=idle.peak_working_set_mb,
                page_faults=idle.page_faults,
                threads=thread_count(p.pid),
            )
        finally:
            trace.close()
            measurement_error = sys.exception()
            try:
                kill_and_wait(p)
            except RuntimeError as cleanup_error:
                if measurement_error is None:
                    raise
                measurement_error.add_note(f"startup probe cleanup failed: {cleanup_error}")
            trace_path.unlink(missing_ok=True)


def summarize_startup(label: str, samples: list[float]) -> None:
    p95 = inclusive_p95(samples)
    print(f"{label}: {len(samples)} samples")
    print("  " + "  ".join(f"{sample:7.1f}" for sample in samples))
    print(f"  median {median(samples):.1f} ms  p95 {p95:.1f} ms")


def summarize_startup_comparison(
    a_label: str,
    b_label: str,
    comparison: AbbaComparison,
) -> None:
    """Report paired round means and B-minus-A deltas."""
    print(f"paired comparison: B - A ({len(comparison.deltas)} A-B-B-A rounds)")
    print(f"  A = {a_label}")
    print(f"  B = {b_label}")
    for index, (a, b, delta, percent) in enumerate(
        zip(
            comparison.paired_a,
            comparison.paired_b,
            comparison.deltas,
            comparison.percentages,
            strict=True,
        ),
        start=1,
    ):
        print(f"  round {index:>2}: A {a:7.1f}  B {b:7.1f}  "
              f"B-A {delta:+7.1f} ms {percent:+6.2f}%")
    print(f"  median paired A {median(comparison.paired_a):.1f} ms  "
          f"B {median(comparison.paired_b):.1f} ms")
    print(f"  median B-A {median(comparison.deltas):+.1f} ms  "
          f"{median(comparison.percentages):+.2f}%")


def cmd_startup_milestones(a: argparse.Namespace, requested_exes: list[Path]) -> None:
    from .native.runtime import HarnessBlocked, HarnessFailure, preflight, sha256_file

    formal_labels = a.label in EVIDENCE_VARIANT_LABELS and (
        not a.compare or a.compare_label in EVIDENCE_VARIANT_LABELS
    )
    source_snapshot = (
        source_state()
        if formal_labels or a.evidence or a.quiet_evidence is not None
        else None
    )
    host_context: dict[str, object] | None = None
    if a.evidence or a.quiet_evidence is not None:
        try:
            host_context = goal04_host_context()
        except HarnessBlocked as error:
            sys.exit(f"startup milestone preflight BLOCKED: {error.code}")
        except HarnessFailure as error:
            sys.exit(f"startup milestone preflight failed: {error.code}")
    measurement_started_at = datetime.now(UTC)

    labels = [a.label or str(a.exe)]
    if a.compare:
        labels.append(a.compare_label or str(a.compare))

    builds: list[dict[str, object]] = []
    threshold: dict[str, object] | None = None
    if formal_labels:
        assert source_snapshot is not None
        build_paths = [a.build_evidence]
        if a.compare:
            build_paths.append(a.compare_build_evidence)
        try:
            builds = [
                load_build_evidence(
                    path,
                    variant_name=label,
                    source=source_snapshot,
                    executable=exe,
                )
                for path, label, exe in zip(
                    build_paths, labels, requested_exes, strict=True
                )
            ]
        except ValueError as error:
            sys.exit(f"startup build evidence failed: {error}")
        if len(builds) == 2 and builds[0]["toolchain"] != builds[1]["toolchain"]:
            sys.exit("startup comparison build manifests use different Rust toolchains")

        if labels == ["full", "no-model"]:
            try:
                threshold = load_threshold_evidence(
                    a.threshold_evidence,
                    source=source_snapshot,
                    checked_at=measurement_started_at,
                )
            except ValueError as error:
                sys.exit(f"startup threshold evidence failed: {error}")

    quiet_gate: dict[str, object] = {}
    if a.quiet_evidence is not None:
        try:
            quiet_gate = read_evidence_object(a.quiet_evidence)
            validate_startup_quiet_evidence(
                quiet_gate,
                source=source_snapshot or source_state(),
                host=host_context or goal04_host_context(),
                checked_at=measurement_started_at,
            )
            quiet_gate = normalized_quiet_evidence(quiet_gate)
        except ValueError as error:
            sys.exit(str(error))

    with ExitStack() as resources:
        exes = requested_exes
        if builds:
            directory = Path(
                resources.enter_context(
                    tempfile.TemporaryDirectory(prefix="markturbo-startup-binaries-")
                )
            ).resolve()
            copied: list[Path] = []
            for index, (source_exe, build) in enumerate(
                zip(requested_exes, builds, strict=True)
            ):
                destination = directory / f"variant-{index}.exe"
                shutil.copy2(source_exe, destination)
                if sha256_file(destination).evidence() != build["executable"]:
                    sys.exit("startup executable changed while creating the private measurement copy")
                copied.append(destination)
            exes = copied

        preflights: list[dict[str, object]] = []
        win32 = parent_context = None
        try:
            for exe in exes:
                evidence: dict[str, object] = {"executable": {}}
                fingerprint = sha256_file(exe)
                current_win32, current_parent = preflight(
                    exe,
                    fingerprint.sha256,
                    evidence,
                    fingerprint=fingerprint,
                )
                preflights.append(evidence)
                if win32 is None:
                    win32, parent_context = current_win32, current_parent
        except HarnessBlocked as error:
            sys.exit(f"startup milestone preflight BLOCKED: {error.code}")
        except HarnessFailure as error:
            sys.exit(f"startup milestone preflight failed: {error.code}")

        assert win32 is not None and parent_context is not None
        profile_roots: list[Path | None] = []
        for _ in exes:
            if a.cache_state == "warm":
                directory = resources.enter_context(
                    tempfile.TemporaryDirectory(prefix="markturbo-startup-warm-")
                )
                profile_roots.append(Path(directory).resolve())
            else:
                profile_roots.append(None)

        def measure(index: int) -> StartupSample:
            exe = exes[index]
            try:
                return startup_milestones_once(
                    exe,
                    a.open,
                    a.timeout,
                    win32=win32,
                    parent_context=parent_context,
                    idle_settle=a.idle_settle,
                    profile_root=profile_roots[index],
                )
            except (HarnessBlocked, HarnessFailure) as error:
                sys.exit(f"startup milestone input failed: {error.code}")
            except (OSError, RuntimeError, ValueError) as error:
                sys.exit(f"startup milestone measurement failed: {error}")

        for _ in range(a.warmup):
            for index in range(len(exes)):
                measure(index)

        if a.compare:
            samples_a, samples_b = measure_startup_abba(
                a.rounds,
                lambda: measure(0),
                lambda: measure(1),
            )
            summarize_startup_milestones(labels[0], samples_a)
            summarize_startup_milestones(labels[1], samples_b)
            print(f"paired comparison: B - A ({a.rounds} A-B-B-A rounds)")
            for field, summary in milestone_comparison(samples_a, samples_b).items():
                print(
                    f"  {field.removesuffix('_ms'):<27} "
                    f"{summary['median_b_minus_a']:+7.1f}  "
                    f"{summary['median_b_minus_a_percent']:+6.2f}%"
                )
        else:
            samples_a = tuple(measure(0) for _ in range(a.rounds))
            samples_b = ()
            summarize_startup_milestones(labels[0], samples_a)

        if a.evidence:
            assert source_snapshot is not None and host_context is not None
            if source_state() != source_snapshot:
                sys.exit("source state changed during startup measurement")
            if any(
                sha256_file(exe).evidence() != build["executable"]
                for exe, build in zip(exes, builds, strict=True)
            ):
                sys.exit("startup executable changed during measurement")
            write_startup_evidence(
                a.evidence,
                label_a=labels[0],
                samples_a=samples_a,
                cache_state=a.cache_state,
                rounds=a.rounds,
                warmup=a.warmup,
                idle_settle=a.idle_settle,
                measurement_started_at=measurement_started_at,
                source=source_snapshot,
                host=host_context,
                build_a=builds[0],
                preflight_a=preflights[0],
                quiet_gate=quiet_gate,
                threshold=threshold,
                label_b=labels[1] if a.compare else None,
                samples_b=samples_b,
                build_b=builds[1] if a.compare else None,
                preflight_b=preflights[1] if a.compare else None,
            )


def cmd_startup(a: argparse.Namespace) -> None:
    """Repeated startup measurement, optionally interleaving two binaries."""
    formal_labels = a.label in EVIDENCE_VARIANT_LABELS and (
        not a.compare or a.compare_label in EVIDENCE_VARIANT_LABELS
    )
    model_pair = bool(a.compare and "no-model" in {a.label, a.compare_label})
    if model_pair and [a.label, a.compare_label] != ["full", "no-model"]:
        sys.exit("no-model comparison requires --label full --compare-label no-model")
    decision_pair = bool(a.compare and [a.label, a.compare_label] == ["full", "no-model"])
    if a.compare and not a.milestones:
        sys.exit("paired startup comparisons require --milestones and build provenance")
    if a.compare and not formal_labels:
        sys.exit("paired milestone comparisons require recognized variant labels")
    if a.rounds < 1:
        sys.exit("--rounds must be at least 1")
    if a.warmup < 0:
        sys.exit("--warmup cannot be negative")
    if a.milestones and formal_labels and a.cache_state == "warm" and a.warmup < 1:
        sys.exit("formal warm startup measurement requires at least one warmup")
    if a.timeout <= 0:
        sys.exit("--timeout must be greater than zero")
    if a.milestones and a.idle_settle < 0:
        sys.exit("--idle-settle cannot be negative")
    if a.evidence and not a.milestones:
        sys.exit("--evidence requires --milestones")
    if a.evidence and a.quiet_evidence is None:
        sys.exit("--evidence requires --quiet-evidence from a passing quiet gate")
    if a.milestones and formal_labels and a.build_evidence is None:
        sys.exit("formal milestone measurement requires --build-evidence for --exe")
    if a.evidence and a.rounds < 10:
        sys.exit("--evidence requires at least 10 rounds")
    if a.evidence and a.label not in EVIDENCE_VARIANT_LABELS:
        sys.exit(
            "--evidence requires --label as one of: "
            + ", ".join(sorted(EVIDENCE_VARIANT_LABELS))
        )
    if a.evidence and a.compare:
        if a.compare_label not in EVIDENCE_VARIANT_LABELS:
            sys.exit(
                "--evidence with --compare requires --compare-label as one of: "
                + ", ".join(sorted(EVIDENCE_VARIANT_LABELS))
            )
        if a.compare_label == a.label:
            sys.exit("startup evidence variant labels must be distinct")
    if a.milestones and formal_labels and a.compare and a.compare_build_evidence is None:
        sys.exit("formal comparison requires --compare-build-evidence")
    if decision_pair and a.threshold_evidence is None:
        sys.exit("full/no-model measurement requires owner-approved --threshold-evidence")
    if a.threshold_evidence is not None and not decision_pair:
        sys.exit("--threshold-evidence is only valid for full/no-model comparison")

    if a.evidence is not None:
        try:
            require_distinct_output_path(
                a.evidence,
                a.exe,
                a.compare,
                a.build_evidence,
                a.compare_build_evidence,
                a.quiet_evidence,
                a.threshold_evidence,
                Path(a.open) if a.open is not None else None,
            )
        except ValueError as error:
            sys.exit(str(error))

    exes = [a.exe] + ([a.compare] if a.compare else [])
    for exe in exes:
        if not exe.is_file():
            sys.exit(f"{exe} not found")

    if a.milestones:
        cmd_startup_milestones(a, exes)
        return

    for _ in range(a.warmup):
        for exe in exes:
            startup_once(exe, a.open, a.timeout)

    if a.compare:
        comparison = measure_abba(
            a.rounds,
            lambda: startup_once(a.exe, a.open, a.timeout),
            lambda: startup_once(a.compare, a.open, a.timeout),
        )
        summarize_startup(f"A {a.exe}", list(comparison.samples_a))
        summarize_startup(f"B {a.compare}", list(comparison.samples_b))
        summarize_startup_comparison(str(a.exe), str(a.compare), comparison)
    else:
        samples = []
        for _ in range(a.rounds):
            samples.append(startup_once(a.exe, a.open, a.timeout))
        summarize_startup(str(a.exe), samples)


def duration_us(text: str) -> float:
    """Parse Rust's compact `Duration` debug form into microseconds."""
    match = re.search(r"first\s+([0-9.]+)(ns|[µμu]s|ms|s)\s+subsequent", text)
    if not match:
        raise ValueError(f"cannot parse first/subsequent duration from: {text}")
    value = float(match.group(1))
    return value * {"ns": 0.001, "µs": 1.0, "μs": 1.0, "us": 1.0,
                    "ms": 1000.0, "s": 1_000_000.0}[match.group(2)]


def ignored_cost_once(
    exe: Path,
    test_name: str,
    timeout: float,
    *,
    label: str,
    env: dict[str, str] | None = None,
) -> float:
    result = subprocess.run(
        [str(exe), test_name, "--ignored", "--nocapture", "--exact"],
        capture_output=True,
        text=True,
        encoding="utf-8",
        errors="replace",
        timeout=timeout,
        env=env,
    )
    output = result.stdout + result.stderr
    if result.returncode:
        sys.exit(f"{exe} {label} probe failed with {result.returncode}:\n{output}")
    try:
        return duration_us(output)
    except ValueError as error:
        sys.exit(str(error))


def formula_once(exe: Path, font_dir: Path | None, timeout: float) -> float:
    env = os.environ.copy()
    env.pop("MT_MATH_FONT_DIR", None)
    if font_dir is not None:
        env["MT_MATH_FONT_DIR"] = str(font_dir)
    return ignored_cost_once(
        exe,
        "first_formula_costs_little_more_than_the_rest",
        timeout,
        label="formula",
        env=env,
    )


def model_first_use_once(exe: Path, timeout: float) -> float:
    return ignored_cost_once(
        exe,
        "first_model_transport_use_cost",
        timeout,
        label="model first-use",
    )


def summarize_formula(label: str, samples: list[float]) -> None:
    print(f"{label}: {len(samples)} samples")
    print("  " + "  ".join(f"{sample:7.1f}" for sample in samples))
    print(
        f"  median {median(samples):.1f} us  "
        f"p95 {nearest_rank_percentile(samples, 0.95):.1f} us"
    )


def summarize_formula_comparison(comparison: AbbaComparison) -> None:
    print(f"paired comparison: B - A ({len(comparison.deltas)} A-B-B-A rounds)")
    for index, (a_value, b_value, delta, percent) in enumerate(
        zip(
            comparison.paired_a,
            comparison.paired_b,
            comparison.deltas,
            comparison.percentages,
            strict=True,
        ),
        start=1,
    ):
        print(
            f"  round {index:>2}: A {a_value:7.1f}  B {b_value:7.1f}  "
            f"B-A {delta:+7.1f} us {percent:+6.2f}%"
        )
    print(
        f"  median paired A {median(comparison.paired_a):.1f} us  "
        f"B {median(comparison.paired_b):.1f} us"
    )
    print(
        f"  median B-A {median(comparison.deltas):+.1f} us  "
        f"{median(comparison.percentages):+.2f}%"
    )


def cmd_formula(a: argparse.Namespace) -> None:
    """First-formula A-B-B-A comparison using prebuilt test executables."""
    if a.rounds < 1:
        sys.exit("--rounds must be at least 1")
    if a.warmup < 0:
        sys.exit("--warmup cannot be negative")
    if a.timeout <= 0:
        sys.exit("--timeout must be greater than zero")
    if a.font_dir is not None and not a.font_dir.is_dir():
        sys.exit(f"{a.font_dir} not found")
    exes = [a.exe] + ([a.compare] if a.compare else [])
    for exe in exes:
        if not exe.is_file():
            sys.exit(f"{exe} not found")

    for _ in range(a.warmup):
        for exe in exes:
            formula_once(exe, a.font_dir, a.timeout)

    if not a.compare:
        samples = [
            formula_once(a.exe, a.font_dir, a.timeout) for _ in range(a.rounds)
        ]
        summarize_formula(str(a.exe), samples)
        return

    comparison = measure_abba(
        a.rounds,
        lambda: formula_once(a.exe, a.font_dir, a.timeout),
        lambda: formula_once(a.compare, a.font_dir, a.timeout),
    )
    summarize_formula(f"A {a.exe}", list(comparison.samples_a))
    summarize_formula(f"B {a.compare}", list(comparison.samples_b))
    summarize_formula_comparison(comparison)


def cmd_model_first_use(a: argparse.Namespace) -> None:
    if a.rounds < 1:
        sys.exit("--rounds must be at least 1")
    if a.warmup < 0:
        sys.exit("--warmup cannot be negative")
    if a.timeout <= 0:
        sys.exit("--timeout must be greater than zero")
    if not a.exe.is_file():
        sys.exit(f"{a.exe} not found")

    if a.evidence is None:
        for _ in range(a.warmup):
            model_first_use_once(a.exe, a.timeout)
        samples = [model_first_use_once(a.exe, a.timeout) for _ in range(a.rounds)]
        summarize_formula(str(a.exe), samples)
        return

    if a.rounds < 10:
        sys.exit("model first-use evidence requires at least 10 rounds")
    if a.quiet_evidence is None:
        sys.exit("model first-use evidence requires --quiet-evidence")
    if a.build_evidence is None:
        sys.exit("model first-use evidence requires --build-evidence")
    if a.app_exe is None or not a.app_exe.is_file():
        sys.exit("model first-use evidence requires an existing --app-exe")
    if a.app_build_evidence is None:
        sys.exit("model first-use evidence requires --app-build-evidence")
    try:
        require_distinct_output_path(
            a.evidence,
            a.exe,
            a.app_exe,
            a.build_evidence,
            a.app_build_evidence,
            a.quiet_evidence,
        )
    except ValueError as error:
        sys.exit(str(error))

    from .native.runtime import HarnessBlocked, HarnessFailure, sha256_file

    source_snapshot = source_state()
    try:
        host_context = goal04_host_context()
    except HarnessBlocked as error:
        sys.exit(f"model first-use preflight BLOCKED: {error.code}")
    except HarnessFailure as error:
        sys.exit(f"model first-use preflight failed: {error.code}")
    measurement_started_at = datetime.now(UTC)
    try:
        quiet_gate = read_evidence_object(a.quiet_evidence)
        validate_startup_quiet_evidence(
            quiet_gate,
            source=source_snapshot,
            host=host_context,
            checked_at=measurement_started_at,
        )
        quiet_gate = normalized_quiet_evidence(quiet_gate)
        test_build = load_build_evidence(
            a.build_evidence,
            variant_name="model-first-use",
            source=source_snapshot,
            executable=a.exe,
        )
        full_build = load_build_evidence(
            a.app_build_evidence,
            variant_name="full",
            source=source_snapshot,
            executable=a.app_exe,
        )
    except ValueError as error:
        sys.exit(f"model first-use evidence failed: {error}")
    if test_build["toolchain"] != full_build["toolchain"]:
        sys.exit("model first-use build manifests use different Rust toolchains")

    with tempfile.TemporaryDirectory(prefix="markturbo-model-first-use-") as directory:
        executable = Path(directory).resolve() / "model-first-use.exe"
        shutil.copy2(a.exe, executable)
        if sha256_file(executable).evidence() != test_build["executable"]:
            sys.exit("model first-use executable changed while creating its private copy")
        for _ in range(a.warmup):
            model_first_use_once(executable, a.timeout)
        samples = [
            model_first_use_once(executable, a.timeout) for _ in range(a.rounds)
        ]
        summarize_formula("model-first-use", samples)
        if sha256_file(executable).evidence() != test_build["executable"]:
            sys.exit("model first-use executable changed during measurement")

    if source_state() != source_snapshot:
        sys.exit("source state changed during model first-use measurement")
    if sha256_file(a.app_exe).evidence() != full_build["executable"]:
        sys.exit("full application executable changed during model first-use measurement")
    write_model_first_use_evidence(
        a.evidence,
        measurement_started_at=measurement_started_at,
        source=source_snapshot,
        host=host_context,
        quiet_gate=quiet_gate,
        full_build=full_build,
        test_build=test_build,
        samples=samples,
        rounds=a.rounds,
        warmup=a.warmup,
    )


def cmd_memory(a: argparse.Namespace) -> None:
    p = launch(a.exe, a.open, a.settle, a.log)
    try:
        sample = process_memory(p.pid)
        print(f"working set {sample.working_set_mb:8.1f} MB")
        print(f"private     {sample.private_mb:8.1f} MB")
        print(f"peak        {sample.peak_working_set_mb:8.1f} MB")
        print(f"page faults {sample.page_faults:8d}")
        print(f"threads     {thread_count(p.pid):8d}")
    finally:
        kill_and_wait(p)


def cmd_cpu(a: argparse.Namespace) -> None:
    """CPU over a series of windows.

    A single sample lands on startup transients — the harness scan, the first
    frame — and reads as a runaway loop. Only a series distinguishes "busy while
    starting" from "never converges", which is exactly the distinction the
    infinite-reparse defect turned on.
    """
    p = launch(a.exe, a.open, a.settle, a.log)
    try:
        prev, prev_wall = cpu_seconds(p.pid), time.monotonic()
        samples = []
        for _ in range(a.samples):
            time.sleep(a.window)
            now, wall = cpu_seconds(p.pid), time.monotonic()
            pct = (now - prev) / (wall - prev_wall) * 100
            samples.append(pct)
            prev, prev_wall = now, wall
        print(f"{a.window:.0f}s windows, per cent of one core:")
        print("  " + "  ".join(f"{s:5.1f}" for s in samples))
        tail = samples[len(samples) // 2:]
        print(f"steady state (second half): {sum(tail) / len(tail):.1f}%")
        ws, _, _ = memory(p.pid)
        print(f"working set: {ws:.1f} MB")
    finally:
        kill_and_wait(p)


def cmd_quiet(a: argparse.Namespace) -> None:
    """Fail unless the host is quiet enough for a small A/B decision."""
    if a.samples < 2:
        sys.exit("--samples must be at least 2")
    if a.interval <= 0:
        sys.exit("--interval must be greater than zero")

    host_context: dict[str, object] | None = None
    if a.evidence is not None:
        from .native.runtime import HarnessBlocked, HarnessFailure

        try:
            host_context = goal04_host_context()
        except HarnessBlocked as error:
            sys.exit(f"quiet evidence preflight BLOCKED: {error.code}")
        except HarnessFailure as error:
            sys.exit(f"quiet evidence preflight failed: {error.code}")

    idle, kernel, user = system_cpu_times()
    cpu_samples: deque[float] = deque(maxlen=a.samples)
    disk_samples: deque[float] = deque(maxlen=a.samples)
    max_intervals = a.samples
    if a.wait_seconds:
        max_intervals = max(a.samples, int(a.wait_seconds / a.interval))
    waited = 0.0
    last_failures: list[str] = []
    with DiskBusyCounter() as disk:
        for _ in range(max_intervals):
            time.sleep(a.interval)
            waited += a.interval
            next_idle, next_kernel, next_user = system_cpu_times()
            total = (next_kernel - kernel) + (next_user - user)
            busy = total - (next_idle - idle)
            cpu_samples.append(max(0.0, min(100.0, busy / total * 100)))
            disk_samples.append(disk.sample())
            idle, kernel, user = next_idle, next_kernel, next_user
            if len(cpu_samples) < a.samples:
                continue
            last_failures = quiet_gate_failures(
                list(cpu_samples),
                list(disk_samples),
                a.max_cpu_median,
                a.max_cpu_p95,
                a.max_disk_median,
                a.max_disk_p95,
            )
            if not last_failures:
                break

    cpu = list(cpu_samples)
    disk_values = list(disk_samples)
    cpu_median = median(cpu)
    cpu_p95 = nearest_rank_percentile(cpu, 0.95)
    disk_median = median(disk_values)
    disk_p95 = nearest_rank_percentile(disk_values, 0.95)
    print(f"quiet gate: {a.samples} x {a.interval:g}s rolling window")
    print(f"  waited {waited:.0f}s")
    print(f"  CPU   median {cpu_median:.2f}%  p95 {cpu_p95:.2f}%")
    print(f"  disk  median {disk_median:.2f}%  p95 {disk_p95:.2f}%")
    status = "FAIL" if last_failures else "PASS"
    if a.evidence is not None:
        evidence = {
            "schema": STARTUP_QUIET_SCHEMA,
            "created_at": datetime.now(UTC).isoformat(),
            "status": status,
            "command": safe_command(),
            "source": source_state(),
            "host": host_context,
            "window": {"samples": a.samples, "interval_seconds": a.interval},
            "waited_seconds": waited,
            "thresholds": {
                "max_cpu_median_percent": a.max_cpu_median,
                "max_cpu_p95_percent": a.max_cpu_p95,
                "max_disk_median_percent": a.max_disk_median,
                "max_disk_p95_percent": a.max_disk_p95,
            },
            "samples": {"cpu_percent": cpu, "disk_percent": disk_values},
            "summary": {
                "cpu_median_percent": cpu_median,
                "cpu_p95_percent": cpu_p95,
                "disk_median_percent": disk_median,
                "disk_p95_percent": disk_p95,
            },
            "failures": last_failures,
        }
        from .native.runtime import write_evidence

        write_evidence(a.evidence, evidence, validate_quiet_evidence)
    if last_failures:
        for failure in last_failures:
            print(f"  FAIL  {failure}")
        raise SystemExit(1)
    print("  PASS")


def cmd_windows(a: argparse.Namespace) -> None:
    if a.expect_top_level is not None and a.expect_top_level < 0:
        sys.exit("--expect-top-level cannot be negative")
    if a.expect_native_chrome_insets and not a.expect_child_class:
        sys.exit("--expect-native-chrome-insets requires --expect-child-class")
    if a.resize_cycles < 0:
        sys.exit("--resize-cycles cannot be negative")
    if a.lifecycle_timeout <= 0:
        sys.exit("--lifecycle-timeout must be greater than zero")

    temporary_log = a.log is None
    if temporary_log:
        handle = tempfile.NamedTemporaryFile(
            prefix="markturbo-probe-", suffix=".log", delete=False
        )
        log_path = Path(handle.name)
        handle.close()
    else:
        log_path = a.log

    probe_root = tempfile.TemporaryDirectory(
        prefix="markturbo-probe-data-", ignore_cleanup_errors=True
    )
    probe_root_dir = Path(probe_root.name)
    runtime_data_dir = probe_root_dir / "data"
    executable_dir = probe_root_dir / "bin"
    executable_dir.mkdir()
    probe_exe = Path(shutil.copy2(a.exe, executable_dir / a.exe.name))
    env = os.environ.copy()
    env["MARKTURBO_DATA_DIR"] = str(runtime_data_dir)
    env["RUST_LOG"] = "info"
    executable_webview_data = probe_exe.with_name(f"{probe_exe.name}.WebView2")

    failures: list[str] = []
    p: subprocess.Popen | None = None
    app_log_path: Path | None = None
    try:
        p = launch(probe_exe, a.open, a.settle, log_path, env)
        app_log_path = runtime_data_dir / "logs" / f"markturbo-{p.pid}.log"
        hwnd = main_window(p.pid)
        if hwnd is None:
            failures.append(
                f"no visible top-level window titled {MAIN_WINDOW_TITLE!r}"
            )
        else:
            r = window_rect(hwnd)
            client = client_rect(hwnd)
            print(f"main window  {r.left},{r.top} to {r.right},{r.bottom}  "
                  f"({class_name(hwnd)}) title={window_text(hwnd)!r}")
            print(
                f"main client  {client.left},{client.top} to "
                f"{client.right},{client.bottom}"
            )
            tops = top_windows(p.pid)
            print(f"process-owned top-level windows: {len(tops)}")
            for top in tops:
                tr = window_rect(top)
                print(f"  {class_name(top):<34} {tr.left:>5},{tr.top:<5} "
                      f"{tr.right - tr.left:>5}x{tr.bottom - tr.top:<5} "
                      f"visible={bool(user32.IsWindowVisible(top))} "
                      f"title={window_text(top)!r}")

            application_tops = [
                top for top in tops if not is_system_input_helper(top)
            ]

            if a.expect_top_level == 1:
                if len(application_tops) != 1 or application_tops[0] != hwnd:
                    failures.append(
                        "expected the titled main window to be the process's "
                        "only application top-level window, found "
                        f"{len(application_tops)}"
                    )
            elif (
                a.expect_top_level is not None
                and len(application_tops) != a.expect_top_level
            ):
                failures.append(
                    f"expected {a.expect_top_level} application top-level "
                    f"window(s), found {len(application_tops)}"
                )

            user32.SetForegroundWindow(hwnd)
            time.sleep(0.25)
            children = child_windows(hwnd)
            print(f"child windows: {len(children)}")
            for _, cls, cr, visible in children:
                print(f"  {cls:<34} {cr.left:>5},{cr.top:<5} "
                      f"{cr.right - cr.left:>5}x{cr.bottom - cr.top:<5} "
                      f"visible={visible}")

            child_failures = expected_child_failures(
                children,
                a.expect_child_class,
                client,
                a.expect_native_chrome_insets,
            )
            failures.extend(child_failures)

            # Walk down one column from just under the title bar. Where the
            # answer stops being GPUI is where an overlay stops being clickable.
            x = r.left + int((r.right - r.left) * 0.2) + 40
            print(f"\nhit test down x={x}:")
            for dy in (50, 60, 70, 75, 80, 85, 90, 100, 140, 220):
                print(f"  y+{dy:<4} -> {hit_test(x, r.top + dy)}")

            if not child_failures:
                failures.extend(run_resize_cycles(
                    hwnd,
                    a.expect_child_class,
                    a.resize_cycles,
                    a.lifecycle_timeout,
                    a.expect_native_chrome_insets,
                ))

            shutdown_failure = graceful_close(p, hwnd, a.lifecycle_timeout)
            if shutdown_failure is not None:
                failures.append(shutdown_failure)
            else:
                print("shutdown: WM_CLOSE -> exit 0 PASS")
    finally:
        if p is not None:
            kill_and_wait(p)
        forbidden = tuple(dict.fromkeys(
            DEFAULT_FORBIDDEN_LOG_SUBSTRINGS
            + tuple(a.forbid_log_substring)
        ))
        stderr_failures = log_content_failures(
            log_path, forbidden, label="stderr log"
        )
        failures.extend(stderr_failures)
        if not stderr_failures:
            print(
                f"stderr scan: {len(forbidden)} forbidden substring(s) absent PASS"
            )

        if app_log_path is None or p is None:
            failures.append("process did not launch, so no application log was available")
        else:
            app_log_failures = log_content_failures(
                app_log_path,
                forbidden,
                required=(f"pid={p.pid};",),
                label="application log",
            )
            failures.extend(app_log_failures)
            if not app_log_failures:
                print(
                    "application log scan: startup record present and "
                    f"{len(forbidden)} forbidden substring(s) absent PASS"
                )
        webview_data_dir = runtime_data_dir / "webview2"
        if webview_data_dir.is_dir() and any(webview_data_dir.iterdir()):
            print(f"WebView data: {webview_data_dir} PASS")
        else:
            failures.append(
                f"WebView did not populate its data directory: {webview_data_dir}"
            )
        if executable_webview_data.exists():
            failures.append(
                "WebView data was created beside the executable: "
                f"{executable_webview_data}"
            )
        if temporary_log:
            try:
                log_path.unlink(missing_ok=True)
            except OSError as error:
                failures.append(f"could not remove temporary stderr log: {error}")
        probe_root.cleanup()

    if failures:
        print("\nacceptance: FAIL")
        for failure in failures:
            print(f"  {failure}")
        raise SystemExit(1)
    print("\nacceptance: PASS")


def cmd_shot(a: argparse.Namespace) -> None:
    """Screenshot the window.

    `ImageGrab` reads the *screen*, not the window's own surface, so anything
    overlapping markturbo lands in the capture. There is a window-owned path
    (`PrintWindow` with `PW_RENDERFULLCONTENT`), but GPUI draws through a
    DirectX swap chain that it does not reach — it returns the frame, blank.
    So this needs an unobstructed foreground window, which makes it a tool for
    someone sitting at the machine rather than part of an automated run.
    """
    from PIL import ImageGrab

    p = launch(a.exe, a.open, a.settle, a.log)
    try:
        hwnd = main_window(p.pid)
        if hwnd is None:
            sys.exit("no visible top-level window")
        user32.SetForegroundWindow(hwnd)
        time.sleep(1.5)
        r = window_rect(hwnd)
        # Confirm we really are on top: if the pixel at our own centre belongs
        # to someone else's window, the capture would be of their content.
        cx, cy = (r.left + r.right) // 2, (r.top + r.bottom) // 2
        owner = user32.WindowFromPoint(POINT(cx, cy))
        root = user32.GetAncestor(owner, 2)  # GA_ROOT
        if root != hwnd:
            sys.exit(
                f"another window ({class_name(owner)}) covers the centre of "
                f"markturbo's window; a screen capture would show it instead. "
                f"Bring markturbo to the front, or use `windows` / `memory`, "
                f"which do not need an unobstructed view."
            )
        img = ImageGrab.grab(bbox=(r.left, r.top, r.right, r.bottom))
        out = Path(a.out)
        out.parent.mkdir(parents=True, exist_ok=True)
        img.save(out)
        print(f"captured {out}  ({img.width}x{img.height})")
    finally:
        kill_and_wait(p)


def main(argv: list[str] | None = None) -> int:
    if sys.platform != "win32":
        print(
            "probe requires Win32 process and window APIs; it is Windows-only.",
            file=sys.stderr,
        )
        return 2

    ap = argparse.ArgumentParser(
        description=__doc__.split("\n")[0],
        formatter_class=argparse.RawDescriptionHelpFormatter,
        epilog=__doc__,
    )
    # On the parent *and* on every subparser, so `probe.py memory --open x` and
    # `probe.py --open x memory` both work. argparse only accepts a parent's
    # optionals before the subcommand, which is not how anyone types it.
    def common(p: argparse.ArgumentParser) -> None:
        p.add_argument("--exe", type=Path, default=DEFAULT_EXE,
                       help="binary under test (default: target/release)")
        p.add_argument("--open", metavar="PATH",
                       help="file or folder to open; omit for the welcome screen (use . for cwd)")
        p.add_argument("--settle", type=float, default=18.0,
                       help="seconds to wait before measuring (default: 18)")
        p.add_argument("--log", type=Path,
                       help="capture the process's stderr to this file")

    common(ap)
    sub = ap.add_subparsers(dest="cmd", required=True)

    common(sub.add_parser("memory", help="working set, private bytes, threads"))

    startup = sub.add_parser("startup", help="time to a visible titled main window")
    common(startup)
    startup.add_argument("--compare", type=Path, help="second binary; measured A-B-B-A")
    startup.add_argument("--rounds", type=int, default=10,
                         help="rounds (10 means 20 samples per binary with --compare)")
    startup.add_argument("--warmup", type=int, default=2,
                         help="discarded launches per binary")
    startup.add_argument("--timeout", type=float, default=30.0,
                         help="seconds allowed for one launch")
    startup.add_argument(
        "--milestones",
        action="store_true",
        help="measure app-acknowledged ready, paint and input milestones",
    )
    startup.add_argument("--label", help="content-free label for --exe evidence")
    startup.add_argument("--compare-label", help="content-free label for --compare evidence")
    startup.add_argument(
        "--cache-state",
        choices=("warm", "fresh-profile"),
        default="warm",
        help="warm reuses isolated profiles; fresh-profile resets profiles without claiming OS cold",
    )
    startup.add_argument("--evidence", type=Path, help="write milestone evidence JSON")
    startup.add_argument(
        "--build-evidence",
        type=Path,
        help="build manifest for --exe from build-goal04",
    )
    startup.add_argument(
        "--compare-build-evidence",
        type=Path,
        help="build manifest for --compare from build-goal04",
    )
    startup.add_argument(
        "--threshold-evidence",
        type=Path,
        help="owner-approved materiality threshold for full/no-model evidence",
    )
    startup.add_argument(
        "--quiet-evidence",
        type=Path,
        help="passing JSON emitted by the quiet subcommand",
    )
    startup.add_argument(
        "--idle-settle",
        type=float,
        default=1.0,
        help="seconds after input acknowledgement before idle counters",
    )

    build = sub.add_parser(
        "build-goal04",
        help="fresh-build one source-bound Goal 04 measurement artifact",
    )
    build.add_argument("--variant", choices=tuple(GOAL04_BUILD_VARIANTS), required=True)
    build.add_argument("--target-dir", type=Path, required=True)
    build.add_argument("--evidence", type=Path, required=True)

    decision = sub.add_parser(
        "decide-goal04",
        help="bind the owner-approved model-transport decision to both cache-mode runs",
    )
    decision.add_argument("--warm-evidence", type=Path, required=True)
    decision.add_argument("--fresh-profile-evidence", type=Path, required=True)
    decision.add_argument("--decision", choices=MODEL_TRANSPORT_DECISIONS, required=True)
    decision.add_argument("--owner-approved", action="store_true")
    decision.add_argument("--evidence", type=Path, required=True)

    formula = sub.add_parser("formula", help="first-formula A-B-B-A comparison")
    formula.add_argument("--exe", type=Path, required=True,
                         help="prebuilt open_document_cost test executable")
    formula.add_argument("--compare", type=Path,
                         help="second test executable; measured A-B-B-A")
    formula.add_argument(
        "--font-dir",
        type=Path,
        help="optional complete KaTeX override; omit to measure embedded fonts",
    )
    formula.add_argument("--rounds", type=int, default=10)
    formula.add_argument("--warmup", type=int, default=2)
    formula.add_argument("--timeout", type=float, default=30.0)

    model = sub.add_parser(
        "model-first-use",
        help="full-build model transport initialization plus first loopback request",
    )
    model.add_argument("--exe", type=Path, required=True,
                       help="prebuilt model_first_use_cost test executable")
    model.add_argument("--app-exe", type=Path,
                       help="full application executable from the same source build")
    model.add_argument("--build-evidence", type=Path,
                       help="model-first-use test build manifest")
    model.add_argument("--app-build-evidence", type=Path,
                       help="full application build manifest")
    model.add_argument("--quiet-evidence", type=Path,
                       help="fresh passing Goal 04 quiet evidence")
    model.add_argument("--evidence", type=Path,
                       help="write hash-bound model first-use evidence")
    model.add_argument("--rounds", type=int, default=10)
    model.add_argument("--warmup", type=int, default=2)
    model.add_argument("--timeout", type=float, default=30.0)

    cpu = sub.add_parser("cpu", help="CPU over a series of windows")
    common(cpu)
    cpu.add_argument("--window", type=float, default=5.0, help="seconds per sample")
    cpu.add_argument("--samples", type=int, default=12, help="number of samples")

    quiet = sub.add_parser("quiet", help="host CPU/disk gate for A/B measurements")
    quiet.add_argument("--samples", type=int, default=60, help="number of samples")
    quiet.add_argument("--interval", type=float, default=1.0,
                       help="seconds per sample")
    quiet.add_argument("--wait-seconds", type=float, default=0.0,
                       help="wait up to this long for a passing rolling window")
    quiet.add_argument("--max-cpu-median", type=float, default=5.0)
    quiet.add_argument("--max-cpu-p95", type=float, default=10.0)
    quiet.add_argument("--max-disk-median", type=float, default=2.0)
    quiet.add_argument("--max-disk-p95", type=float, default=10.0)
    quiet.add_argument("--evidence", type=Path, help="write PASS or FAIL evidence JSON")

    windows = sub.add_parser("windows", help="child windows and hit testing")
    common(windows)
    windows.add_argument(
        "--expect-top-level", type=int, metavar="COUNT",
        help="fail unless COUNT top-level windows belong to the process, hidden included",
    )
    windows.add_argument(
        "--expect-child-class", action="append", default=[], metavar="CLASS",
        help=(
            "require a visible, non-zero child of CLASS inside the main client; "
            "may be repeated"
        ),
    )
    windows.add_argument(
        "--expect-native-chrome-insets", action="store_true",
        help=(
            "require every expected child to leave positive top and bottom "
            "insets for native chrome"
        ),
    )
    windows.add_argument(
        "--forbid-log-substring", action="append", default=[], metavar="TEXT",
        help=(
            "additional stderr text that fails acceptance; "
            "'RefCell already borrowed' is always forbidden"
        ),
    )
    windows.add_argument(
        "--resize-cycles", type=int, default=2, metavar="COUNT",
        help="alternate and restore the main-window size COUNT times (default: 2)",
    )
    windows.add_argument(
        "--lifecycle-timeout", type=float, default=5.0, metavar="SECONDS",
        help="timeout for each resize and graceful WM_CLOSE (default: 5)",
    )

    shot = sub.add_parser("shot", help="screenshot the window")
    common(shot)
    shot.add_argument("-o", "--out", default="shot.png")

    a = ap.parse_args(argv)
    if hasattr(a, "exe") and not a.exe.is_file():
        sys.exit(f"{a.exe} not found — cargo build --release first")
    {"startup": cmd_startup, "build-goal04": cmd_build_goal04,
     "decide-goal04": cmd_decide_goal04, "formula": cmd_formula,
     "model-first-use": cmd_model_first_use, "memory": cmd_memory,
     "cpu": cmd_cpu, "quiet": cmd_quiet, "windows": cmd_windows,
     "shot": cmd_shot}[a.cmd](a)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
