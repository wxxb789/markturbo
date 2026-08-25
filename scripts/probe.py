#!/usr/bin/env -S uv run --script
# /// script
# requires-python = ">=3.11"
# dependencies = ["pillow>=10"]
# ///
"""Measure and inspect a running markturbo window.

The harness behind this repository's performance work. Everything it reports is
read from a real process on a real window — no estimates.

    ./scripts/probe.py memory                 # startup RSS, empty workspace
    ./scripts/probe.py memory --open doc.md   # with a document open
    ./scripts/probe.py cpu --open big.md      # idle CPU over time
    ./scripts/probe.py windows --open x.html  # child windows and hit testing
    ./scripts/probe.py shot --open x.md -o out.png

Why Python rather than PowerShell: this needs Win32 for process counters, child
window enumeration and hit testing, and `ctypes` reaches all of it without the
assembly-loading differences between PowerShell 5 and 7. `uv run` makes the one
dependency (Pillow, for `shot`) implicit.

Windows only. Every subcommand except `shot` works headless.
"""

from __future__ import annotations

import argparse
import ctypes
import ctypes.wintypes as wt
import os
import subprocess
import sys
import time
from pathlib import Path

REPO = Path(__file__).resolve().parent.parent
DEFAULT_EXE = REPO / "target" / "release" / "markturbo.exe"

if sys.platform != "win32":
    sys.exit("probe.py needs Win32 process and window APIs; it is Windows-only.")

psapi = ctypes.WinDLL("psapi", use_last_error=True)
kernel32 = ctypes.WinDLL("kernel32", use_last_error=True)
user32 = ctypes.WinDLL("user32", use_last_error=True)


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


class RECT(ctypes.Structure):
    _fields_ = [("left", ctypes.c_long), ("top", ctypes.c_long),
                ("right", ctypes.c_long), ("bottom", ctypes.c_long)]


class POINT(ctypes.Structure):
    _fields_ = [("x", ctypes.c_long), ("y", ctypes.c_long)]


PROCESS_QUERY_INFORMATION = 0x0400
PROCESS_VM_READ = 0x0010
MB = 1024 * 1024


def _open(pid: int) -> int:
    h = kernel32.OpenProcess(PROCESS_QUERY_INFORMATION | PROCESS_VM_READ, False, pid)
    if not h:
        raise OSError(ctypes.get_last_error(), f"OpenProcess({pid})")
    return h


def memory(pid: int) -> tuple[float, float, float]:
    """Working set, private bytes and peak working set, in MB."""
    h = _open(pid)
    try:
        c = PROCESS_MEMORY_COUNTERS()
        c.cb = ctypes.sizeof(c)
        if not psapi.GetProcessMemoryInfo(h, ctypes.byref(c), c.cb):
            raise OSError(ctypes.get_last_error(), "GetProcessMemoryInfo")
        return c.WorkingSetSize / MB, c.PagefileUsage / MB, c.PeakWorkingSetSize / MB
    finally:
        kernel32.CloseHandle(h)


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
    """The process's main top-level window, or None if it has none yet.

    Largest by area rather than first found: a GPUI process also owns small
    helper windows (tooltip and popup hosts) that are visible and titled, and
    `EnumWindows` reaches them in Z-order, so taking the first gives a 103x22
    tooltip host rather than the window.
    """
    best: list[tuple[int, int]] = []

    @ctypes.WINFUNCTYPE(wt.BOOL, wt.HWND, wt.LPARAM)
    def cb(hwnd, _):
        owner = wt.DWORD()
        user32.GetWindowThreadProcessId(hwnd, ctypes.byref(owner))
        if owner.value == pid and user32.IsWindowVisible(hwnd):
            r = window_rect(hwnd)
            area = (r.right - r.left) * (r.bottom - r.top)
            if area > 0:
                best.append((area, hwnd))
        return True

    user32.EnumWindows(cb, 0)
    return max(best)[1] if best else None


def window_rect(hwnd: int) -> RECT:
    r = RECT()
    user32.GetWindowRect(hwnd, ctypes.byref(r))
    return r


def class_name(hwnd: int) -> str:
    buf = ctypes.create_unicode_buffer(256)
    user32.GetClassNameW(hwnd, buf, 256)
    return buf.value


def child_windows(hwnd: int) -> list[tuple[str, RECT, bool]]:
    """Every child window, with its class, screen rect and visibility.

    The WebView is an OS child window rather than a GPUI element, so this is how
    you see where it actually is — and, with `hit_test`, what it covers.
    """
    out: list[tuple[str, RECT, bool]] = []

    @ctypes.WINFUNCTYPE(wt.BOOL, wt.HWND, wt.LPARAM)
    def cb(child, _):
        out.append((class_name(child), window_rect(child),
                    bool(user32.IsWindowVisible(child))))
        return True

    user32.EnumChildWindows(hwnd, cb, 0)
    return out


def hit_test(x: int, y: int) -> str:
    """Which window owns the pixel at (x, y) — i.e. who gets the click."""
    p = POINT(x, y)
    return class_name(user32.WindowFromPoint(p))


def launch(exe: Path, target: str | None, settle: float,
           log: Path | None = None) -> subprocess.Popen:
    args = [str(exe)] + ([target] if target else [])
    err = open(log, "wb") if log else subprocess.DEVNULL
    p = subprocess.Popen(args, stdout=subprocess.DEVNULL, stderr=err)
    time.sleep(settle)
    if p.poll() is not None:
        sys.exit(f"markturbo exited with {p.returncode} before settling")
    return p


def cmd_memory(a: argparse.Namespace) -> None:
    p = launch(a.exe, a.open, a.settle, a.log)
    try:
        ws, priv, peak = memory(p.pid)
        print(f"working set {ws:8.1f} MB")
        print(f"private     {priv:8.1f} MB")
        print(f"peak        {peak:8.1f} MB")
        print(f"threads     {thread_count(p.pid):8d}")
    finally:
        p.kill()


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
        p.kill()


def cmd_windows(a: argparse.Namespace) -> None:
    p = launch(a.exe, a.open, a.settle, a.log)
    try:
        hwnd = main_window(p.pid)
        if hwnd is None:
            sys.exit("no visible top-level window")
        r = window_rect(hwnd)
        print(f"main window  {r.left},{r.top} to {r.right},{r.bottom}  "
              f"({class_name(hwnd)})")
        children = child_windows(hwnd)
        print(f"child windows: {len(children)}")
        for cls, cr, vis in children:
            print(f"  {cls:<34} {cr.left:>5},{cr.top:<5} "
                  f"{cr.right - cr.left:>5}x{cr.bottom - cr.top:<5} visible={vis}")

        # Walk down one column from just under the title bar. Where the answer
        # stops being the GPUI window is where a GPUI overlay stops being
        # clickable — the WebView is an OS child window and sits above
        # everything GPUI draws.
        x = r.left + int((r.right - r.left) * 0.2) + 40
        print(f"\nhit test down x={x}:")
        for dy in (50, 60, 70, 75, 80, 85, 90, 100, 140, 220):
            print(f"  y+{dy:<4} -> {hit_test(x, r.top + dy)}")
    finally:
        p.kill()


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
        p.kill()


def main() -> None:
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
                       help="file or folder to open; omit for the current directory")
        p.add_argument("--settle", type=float, default=18.0,
                       help="seconds to wait before measuring (default: 18)")
        p.add_argument("--log", type=Path,
                       help="capture the process's stderr to this file")

    common(ap)
    sub = ap.add_subparsers(dest="cmd", required=True)

    common(sub.add_parser("memory", help="working set, private bytes, threads"))

    cpu = sub.add_parser("cpu", help="CPU over a series of windows")
    common(cpu)
    cpu.add_argument("--window", type=float, default=5.0, help="seconds per sample")
    cpu.add_argument("--samples", type=int, default=12, help="number of samples")

    common(sub.add_parser("windows", help="child windows and hit testing"))

    shot = sub.add_parser("shot", help="screenshot the window")
    common(shot)
    shot.add_argument("-o", "--out", default="shot.png")

    a = ap.parse_args()
    if not a.exe.is_file():
        sys.exit(f"{a.exe} not found — cargo build --release first")
    {"memory": cmd_memory, "cpu": cmd_cpu,
     "windows": cmd_windows, "shot": cmd_shot}[a.cmd](a)


if __name__ == "__main__":
    main()
