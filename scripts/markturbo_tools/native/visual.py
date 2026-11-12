"""Capture the Goal 09 Windows visual-acceptance matrix from a release build.

The matrix is eleven product states at the configured minimum, default and wide
DIP viewports in the two effective color modes and both interface languages.
Each cell launches the packaged app on an isolated profile, drives the real
controls, refuses an obstructed or off-screen window, verifies the effective
background against the shipped theme, and writes one PNG plus a content-free
JSON receipt.

Run from the repository root, on an active Windows 11 x64 desktop, with a
packaged `MarkTurbo.exe` built from the same source revision::

    uv run --locked --project scripts scripts/mt.py visual \\
        --exe crates/mt-app/.nsh/out/MarkTurbo/MarkTurbo.exe \\
        --out docs/evidence/goal-09/matrix

This is capture tooling, not owner approval or first-time-user evidence.
"""

from __future__ import annotations

import argparse
import base64
import ctypes
import ctypes.wintypes as wt
import datetime as dt
import hashlib
import io
import json
import os
import re
import socket
import struct
import subprocess
import sys
import tempfile
import threading
import time
import urllib.parse
import urllib.request
from collections.abc import Iterable, Sequence
from dataclasses import asdict, dataclass
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path
from typing import Any

from .runtime import HarnessBlocked, HarnessFailure, Win32, safe_exception_name, sha256_file

SCHEMA = "markturbo.goal-09-visual-matrix"
RECEIPT_SCHEMA = "markturbo.goal-09-visual-cell"
STATES = (
    "clean-profile-welcome",
    "new-unsaved-document",
    "ordinary-edit",
    "native-preview-or-split",
    "web-compatibility-preview",
    "review-loading",
    "review-result-questions",
    "review-stale",
    "effective-agent-context",
    "external-change-conflict",
    "settings",
)
VIEWPORTS = {
    "minimum": (720, 480),
    "default": (1400, 900),
    "wide": (1920, 1080),
}
MODES = ("light", "dark")
LANGUAGES = ("en-us", "zh-cn")
REVIEW_STATES = frozenset(
    {"review-loading", "review-result-questions", "review-stale"}
)
WAIT = 20.0
FRAME = 0.2
CELL_TIMEOUT = 90.0
DEBUG = False
ACTIONABLE_ROLES = frozenset(
    {
        "Button",
        "CheckBox",
        "ComboBox",
        "Edit",
        "Hyperlink",
        "ListItem",
        "MenuItem",
        "RadioButton",
        "Slider",
        "Spinner",
        "TabItem",
        "TreeItem",
    }
)

# The model request is the only non-loopback-shaped operation in this matrix.
# It reaches a process-local server that returns a fixed review; no credential
# or document content is written to a receipt.
REVIEW_ANSWER = (
    "Which risk should the plan prioritize?"
)
REVIEW_JSON = json.dumps(
    {
        "schema_version": "review-v1",
        "scope": {"kind": "document"},
        "understood_intent": {
            "stated_goal": "Plan the release.",
            "relevant_context": [],
            "constraints": [],
            "non_goals": [],
            "expected_deliverable": "A release plan.",
            "success_evidence": [],
            "inferred_assumptions": [],
            "unresolved_decisions": [],
        },
        "findings": [],
        "clarification_questions": [
            {
                "question": REVIEW_ANSWER,
                "priority": "high",
                "impact": "It changes the risk section.",
            }
        ],
    },
    ensure_ascii=True,
    separators=(",", ":"),
)

LABELS = {
    "en-us": {
        "new": "New",
        "edit": "Edit",
        "preview": "Preview",
        "more": "More layouts",
        "web": "Compatibility Preview",
        "context": "Agent Context",
        "settings": "Settings",
        "review": "Review",
        "send": "Send",
        "save": "Save",
        "welcome": "Start a Markdown document",
        "untitled": "Untitled",
        "web_restricted": "Restricted: scripts disabled",
        "reviewing": "Reviewing…",
        "ready": "Review ready",
        "stale": "This review describes an older source revision and is stale.",
        "question": "Clarification questions",
        "question_text": REVIEW_ANSWER,
        "context_ready": "Effective Agent Context",
        "conflict": "Save conflict",
    },
    "zh-cn": {
        "new": "新建",
        "edit": "编辑",
        "preview": "预览",
        "more": "更多布局",
        "web": "兼容预览",
        "context": "代理上下文",
        "settings": "设置",
        "review": "审查",
        "send": "发送",
        "save": "保存",
        "welcome": "开始编写 Markdown 文档",
        "untitled": "未命名",
        "web_restricted": "受限：脚本已禁用",
        "reviewing": "正在审查…",
        "ready": "审查已就绪",
        "stale": "此审查描述的是较旧的源版本，已过期。",
        "question": "澄清问题",
        "question_text": REVIEW_ANSWER,
        "context_ready": "有效代理上下文",
        "conflict": "保存冲突",
    },
}

THEME_BACKGROUNDS = {
    "light": (255, 255, 255),
    "dark": (13, 17, 23),
}


class CdpError(RuntimeError):
    pass


class WebSocket:
    """The RFC 6455 subset CDP needs, so capture adds no dependency."""

    def __init__(self, url: str, timeout: float = 10.0) -> None:
        parsed = urllib.parse.urlparse(url)
        if parsed.scheme != "ws" or not parsed.hostname or not parsed.port:
            raise CdpError("unsupported websocket URL")
        self.socket = socket.create_connection(
            (parsed.hostname, parsed.port), timeout=timeout
        )
        self.socket.settimeout(timeout)
        key = base64.b64encode(os.urandom(16)).decode("ascii")
        path = parsed.path or "/"
        request = (
            f"GET {path} HTTP/1.1\r\n"
            f"Host: {parsed.hostname}:{parsed.port}\r\n"
            "Upgrade: websocket\r\n"
            "Connection: Upgrade\r\n"
            f"Sec-WebSocket-Key: {key}\r\n"
            "Sec-WebSocket-Version: 13\r\n"
            "Origin: http://127.0.0.1\r\n\r\n"
        )
        self.socket.sendall(request.encode("ascii"))
        response = b""
        while b"\r\n\r\n" not in response:
            chunk = self.socket.recv(4096)
            if not chunk:
                raise CdpError("websocket handshake closed")
            response += chunk
        if b" 101 " not in response.split(b"\r\n", 1)[0]:
            raise CdpError("websocket handshake refused")

    def send(self, payload: dict[str, Any]) -> None:
        data = json.dumps(payload, separators=(",", ":")).encode("utf-8")
        header = bytearray([0x81])
        masked = 0x80
        if len(data) < 126:
            header.append(masked | len(data))
        elif len(data) < 65536:
            header.append(masked | 126)
            header.extend(struct.pack("!H", len(data)))
        else:
            header.append(masked | 127)
            header.extend(struct.pack("!Q", len(data)))
        mask = os.urandom(4)
        header.extend(mask)
        masked_data = bytes(byte ^ mask[index % 4] for index, byte in enumerate(data))
        self.socket.sendall(bytes(header) + masked_data)

    def receive(self) -> dict[str, Any]:
        first = self.socket.recv(2)
        if len(first) < 2:
            raise CdpError("websocket closed")
        opcode = first[0] & 0x0F
        length = first[1] & 0x7F
        if length == 126:
            length = struct.unpack("!H", self.socket.recv(2))[0]
        elif length == 127:
            length = struct.unpack("!Q", self.socket.recv(8))[0]
        if first[1] & 0x80:
            mask = self.socket.recv(4)
        else:
            mask = None
        payload = b""
        while len(payload) < length:
            chunk = self.socket.recv(length - len(payload))
            if not chunk:
                raise CdpError("websocket closed")
            payload += chunk
        if mask:
            payload = bytes(byte ^ mask[index % 4] for index, byte in enumerate(payload))
        if opcode == 0x9:
            self.send({"type": "pong"})
            return self.receive()
        if opcode == 0x8:
            raise CdpError("websocket closed")
        if opcode != 0x1:
            return self.receive()
        return json.loads(payload.decode("utf-8"))

    def call(self, identifier: int, method: str, params: dict[str, Any] | None = None) -> dict[str, Any]:
        self.send({"id": identifier, "method": method, "params": params or {}})
        while True:
            message = self.receive()
            if message.get("id") == identifier:
                if "error" in message:
                    raise CdpError("CDP command failed")
                return message.get("result", {})

    def close(self) -> None:
        try:
            self.socket.close()
        except OSError:
            pass


def free_port() -> int:
    with socket.socket(socket.AF_INET, socket.SOCK_STREAM) as probe:
        probe.bind(("127.0.0.1", 0))
        return int(probe.getsockname()[1])


def cdp_page_target(port: int, timeout: float = WAIT) -> str:
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        try:
            with urllib.request.urlopen(
                f"http://127.0.0.1:{port}/json/list", timeout=2
            ) as response:
                targets = json.loads(response.read().decode("utf-8"))
            pages = [target for target in targets if target.get("type") == "page"]
            if pages:
                return str(pages[0]["webSocketDebuggerUrl"])
        except (OSError, ValueError, KeyError):
            pass
        time.sleep(0.25)
    raise VisualFailure("WEBVIEW_CDP_TARGET_MISSING")


def cdp_page_png(port: int) -> bytes:
    websocket = WebSocket(cdp_page_target(port))
    try:
        websocket.call(1, "Page.enable")
        result = websocket.call(
            2,
            "Page.captureScreenshot",
            {"format": "png", "fromSurface": True, "captureBeyondViewport": False},
        )
        encoded = result.get("data")
        if not isinstance(encoded, str):
            raise CdpError("CDP screenshot has no data")
        return base64.b64decode(encoded)
    finally:
        websocket.close()


class VisualFailure(RuntimeError):
    """A cell could not be prepared, observed, or captured."""

    def __init__(self, code: str, detail: str = "") -> None:
        super().__init__(code)
        self.code = code
        self.detail = detail


@dataclass(frozen=True)
class Cell:
    state: str
    language: str
    viewport: str
    mode: str

    @property
    def cell_id(self) -> str:
        return f"{self.language}-{self.state}-{self.viewport}-{self.mode}"


@dataclass(frozen=True)
class SourceBinding:
    head: str
    tree: str
    dirty: bool
    worktree_tree: str | None


def utc_now() -> str:
    return dt.datetime.now(dt.timezone.utc).isoformat(timespec="milliseconds").replace(
        "+00:00", "Z"
    )


def matrix_cells(
    states: Iterable[str] = STATES,
    languages: Iterable[str] = LANGUAGES,
    viewports: Iterable[str] = tuple(VIEWPORTS),
    modes: Iterable[str] = MODES,
) -> list[Cell]:
    """Every requested cell in a stable order."""
    return [
        Cell(state, language, viewport, mode)
        for state in states
        for language in languages
        for viewport in viewports
        for mode in modes
    ]


def git(*args: str) -> str:
    result = subprocess.run(
        ["git", *args], capture_output=True, text=True, check=False
    )
    if result.returncode:
        raise VisualFailure("SOURCE_BINDING_FAILED")
    return result.stdout.strip()


def source_binding(require_clean: bool) -> SourceBinding:
    head = git("rev-parse", "HEAD")
    tree = git("rev-parse", "HEAD^{tree}")
    dirty = bool(git("status", "--porcelain", "--untracked-files=normal"))
    worktree_tree = None
    if dirty and require_clean:
        raise VisualFailure("SOURCE_TREE_DIRTY")
    if dirty:
        with tempfile.TemporaryDirectory(prefix="markturbo-tree-") as temporary:
            index = Path(temporary) / "index"
            environment = os.environ.copy()
            environment["GIT_INDEX_FILE"] = str(index)
            for command in (["read-tree", "HEAD"], ["add", "-A"], ["write-tree"]):
                result = subprocess.run(
                    ["git", *command],
                    capture_output=True,
                    text=True,
                    check=False,
                    env=environment,
                )
                if result.returncode:
                    raise VisualFailure("SOURCE_TREE_HASH_FAILED")
            worktree_tree = result.stdout.strip()
    return SourceBinding(head, tree, dirty, worktree_tree)


def executable_binding(path: Path) -> dict[str, Any]:
    if not path.is_file():
        raise VisualFailure("EXECUTABLE_MISSING")
    fingerprint = sha256_file(path)
    return {
        "name": path.name,
        "byte_count": fingerprint.byte_count,
        "sha256": fingerprint.sha256,
    }


def dpi_awareness() -> tuple[int, int, tuple[int, int, int, int]]:
    try:
        ctypes.windll.user32.SetProcessDpiAwarenessContext(ctypes.c_void_p(-4))
    except (AttributeError, OSError):
        ctypes.windll.user32.SetProcessDPIAware()
    user32 = ctypes.windll.user32
    width = int(user32.GetSystemMetrics(0))
    height = int(user32.GetSystemMetrics(1))
    rect = wt.RECT()
    if not user32.SystemParametersInfoW(0x0030, 0, ctypes.byref(rect), 0):
        rect = wt.RECT(0, 0, width, height)
    return width, height, (rect.left, rect.top, rect.right, rect.bottom)


def physical_viewport(name: str, dpi: int) -> tuple[int, int]:
    dip_width, dip_height = VIEWPORTS[name]
    return round(dip_width * dpi / 96.0), round(dip_height * dpi / 96.0)


class _BitmapInfoHeader(ctypes.Structure):
    _fields_ = [
        ("biSize", wt.DWORD),
        ("biWidth", ctypes.c_long),
        ("biHeight", ctypes.c_long),
        ("biPlanes", wt.WORD),
        ("biBitCount", wt.WORD),
        ("biCompression", wt.DWORD),
        ("biSizeImage", wt.DWORD),
        ("biXPelsPerMeter", ctypes.c_long),
        ("biYPelsPerMeter", ctypes.c_long),
        ("biClrUsed", wt.DWORD),
        ("biClrImportant", wt.DWORD),
    ]


class _BitmapInfo(ctypes.Structure):
    _fields_ = [("bmiHeader", _BitmapInfoHeader), ("bmiColors", wt.DWORD * 3)]


def capture_window(hwnd: int) -> Any:
    """The window's client area through PrintWindow, without screen focus."""
    from PIL import Image

    user32 = ctypes.windll.user32
    gdi32 = ctypes.windll.gdi32
    rect = wt.RECT()
    if not user32.GetClientRect(hwnd, ctypes.byref(rect)):
        raise VisualFailure("CAPTURE_CLIENT_RECT_FAILED")
    width, height = rect.right, rect.bottom
    if width <= 0 or height <= 0:
        raise VisualFailure("CAPTURE_EMPTY_WINDOW")
    screen = gdi32.CreateCompatibleDC(None)
    dc = gdi32.CreateCompatibleDC(screen)
    bitmap = None
    try:
        info = _BitmapInfo()
        info.bmiHeader.biSize = ctypes.sizeof(_BitmapInfoHeader)
        info.bmiHeader.biWidth = width
        # Top-down rows.
        info.bmiHeader.biHeight = -height
        info.bmiHeader.biPlanes = 1
        info.bmiHeader.biBitCount = 32
        info.bmiHeader.biCompression = 0
        bits = ctypes.c_void_p()
        bitmap = gdi32.CreateDIBSection(
            dc, ctypes.byref(info), 0, ctypes.byref(bits), None, 0
        )
        if not bitmap or not bits:
            raise VisualFailure("CAPTURE_BITMAP_FAILED")
        old = gdi32.SelectObject(dc, bitmap)
        try:
            if not user32.PrintWindow(hwnd, dc, 3):
                raise VisualFailure("CAPTURE_PRINTWINDOW_FAILED")
            raw = ctypes.string_at(bits, width * height * 4)
        finally:
            gdi32.SelectObject(dc, old)
        return Image.frombuffer(
            "RGBA", (width, height), raw, "raw", "BGRA", 0, 1
        ).convert("RGB")
    finally:
        if bitmap:
            gdi32.DeleteObject(bitmap)
        gdi32.DeleteDC(dc)
        gdi32.DeleteDC(screen)


def wait_until(
    probe: Any, timeout: float, code: str, interval: float = 0.05
) -> Any:
    deadline = time.monotonic() + timeout
    last: Exception | None = None
    while time.monotonic() < deadline:
        try:
            result = probe()
        except Exception as error:  # noqa: BLE001 - UIA raises COM variants
            last = error
            result = None
        if result:
            return result
        time.sleep(interval)
    raise VisualFailure(code, safe_exception_name(last) if last else "")


class ReviewServer:
    """A loopback OpenAI-compatible endpoint for the three Review cells."""

    def __init__(self, hold: bool) -> None:
        self.hold = hold
        self.seen = threading.Event()
        self.release = threading.Event()
        handler = self._handler()
        self.server = ThreadingHTTPServer(("127.0.0.1", 0), handler)
        self.server.daemon_threads = True
        self.thread = threading.Thread(
            target=self.server.serve_forever, name="goal09-review", daemon=True
        )

    @property
    def port(self) -> int:
        return int(self.server.server_address[1])

    @property
    def base_url(self) -> str:
        return f"http://127.0.0.1:{self.port}/v1/"

    def start(self) -> None:
        self.thread.start()

    def close(self) -> None:
        self.release.set()
        self.server.shutdown()
        self.server.server_close()

    def _handler(self) -> type[BaseHTTPRequestHandler]:
        outer = self

        class Handler(BaseHTTPRequestHandler):
            def log_message(self, *_args: Any) -> None:
                return

            def do_POST(self) -> None:  # noqa: N802 - stdlib handler name
                length = int(self.headers.get("content-length", "0"))
                self.rfile.read(length)
                outer.seen.set()
                if outer.hold and not outer.release.wait(WAIT):
                    self.send_error(504)
                    return
                response = json.dumps(
                    {
                        "id": "chatcmpl-goal09",
                        "object": "chat.completion",
                        "model": "goal09-review",
                        "choices": [
                            {
                                "index": 0,
                                "message": {"role": "assistant", "content": REVIEW_JSON},
                                "finish_reason": "stop",
                            }
                        ],
                    },
                    ensure_ascii=True,
                    separators=(",", ":"),
                ).encode("utf-8")
                try:
                    self.send_response(200)
                    self.send_header("content-type", "application/json")
                    self.send_header("content-length", str(len(response)))
                    self.send_header("connection", "close")
                    self.end_headers()
                    self.wfile.write(response)
                except (BrokenPipeError, ConnectionResetError):
                    # The cell closed its app after the capture; the response
                    # has no reader, and that is not a matrix failure.
                    return

        return Handler


def credential_identity(port: int) -> str:
    identity = hashlib.sha256()
    for component in (
        "io.github.wxxb789.markturbo",
        "openai-chat",
        "http",
        "127.0.0.1",
        str(port),
        "/v1/",
    ):
        identity.update(component.encode("ascii"))
        identity.update(b"\0")
    return (
        "io.github.wxxb789.markturbo:model-credential:v2"
        f"|wire=openai-chat|host=127.0.0.1|identity-sha256={identity.hexdigest()}"
    )


class AppWindow:
    def __init__(self, win32: Win32, win: Any) -> None:
        self.win32 = win32
        self.win = win
        self.hwnd = int(win.handle)

    def wait_text(self, text: str, timeout: float = WAIT) -> None:
        def probe() -> bool:
            try:
                for control in self.win.descendants():
                    try:
                        if control.window_text() == text:
                            return True
                    except Exception:  # noqa: BLE001 - UIA element can go stale
                        continue
            except Exception:  # noqa: BLE001 - the tree can be rebuilding
                return False
            return False

        try:
            wait_until(probe, timeout, "STATE_MARKER_MISSING")
        except VisualFailure:
            if DEBUG:
                print(
                    "  texts=" + repr([c.window_text() for c in self.win.descendants()]),
                    file=sys.stderr,
                    flush=True,
                )
            raise

    def wait_contains(self, text: str, timeout: float = WAIT) -> None:
        def probe() -> bool:
            for control in self.win.descendants():
                try:
                    if text in control.window_text():
                        return True
                except Exception:  # noqa: BLE001 - UIA element can go stale
                    continue
            return False

        try:
            wait_until(probe, timeout, "STATE_MARKER_MISSING")
        except VisualFailure:
            if DEBUG:
                print(
                    "  texts=" + repr([c.window_text() for c in self.win.descendants()]),
                    file=sys.stderr,
                    flush=True,
                )
            raise

    def has_text(self, text: str) -> bool:
        try:
            for control in self.win.descendants():
                try:
                    if control.window_text() == text:
                        return True
                except Exception:  # noqa: BLE001 - UIA element can go stale
                    continue
        except Exception:  # noqa: BLE001 - the tree can be rebuilding
            return False
        return False

    def scroll_until_text(self, text: str, timeout: float = WAIT) -> None:
        deadline = time.monotonic() + timeout
        user32 = self.win32.user32
        left, top, right, bottom = self.client_rect()
        x = right - max(120, (right - left) // 6)
        y = (top + bottom) // 2
        user32.SetCursorPos(x, y)
        while time.monotonic() < deadline:
            if self.has_text(text):
                return
            user32.mouse_event(0x0800, 0, 0, -120, 0)
            time.sleep(0.2)
        if not self.has_text(text):
            raise VisualFailure("STATE_MARKER_MISSING")

    def button(self, label: str) -> Any:
        def probe() -> Any | None:
            for control in self.win.descendants():
                try:
                    if (
                        control.element_info.control_type == "Button"
                        and control.window_text() == label
                    ):
                        return control
                except Exception:  # noqa: BLE001 - UIA element can go stale
                    continue
            return None

        return wait_until(probe, WAIT, "CONTROL_MISSING")

    def click(self, label: str) -> None:
        control = self.button(label)
        try:
            control.invoke()
        except Exception:
            try:
                control.click_input()
            except Exception as error:  # noqa: BLE001
                raise VisualFailure(
                    "CONTROL_CLICK_FAILED", safe_exception_name(error)
                ) from None
        time.sleep(FRAME)

    def editor_text(self) -> str:
        def probe() -> str | None:
            for control in self.win.descendants():
                try:
                    if control.window_text() != "Markdown source":
                        continue
                    pattern = control.iface_value
                    value = pattern.CurrentValue
                    return value if isinstance(value, str) else None
                except Exception:  # noqa: BLE001 - no ValuePattern yet
                    continue
            return None

        value = wait_until(probe, WAIT, "EDITOR_MISSING")
        return value

    def set_editor_text(self, text: str) -> None:
        editor = None
        for control in self.win.descendants():
            try:
                if control.window_text() == "Markdown source":
                    editor = control
                    break
            except Exception:  # noqa: BLE001
                continue
        if editor is None:
            raise VisualFailure("EDITOR_MISSING")
        try:
            editor.set_edit_text(text)
        except Exception as error:  # noqa: BLE001
            raise VisualFailure("EDITOR_UPDATE_FAILED", safe_exception_name(error)) from None
        wait_until(
            lambda: self.editor_text() == text,
            WAIT,
            "EDITOR_UPDATE_NOT_OBSERVED",
        )

    def resize_client(self, width: int, height: int, work: tuple[int, int, int, int]) -> None:
        user32 = self.win32.user32
        for _ in range(3):
            outer = wt.RECT()
            client = wt.RECT()
            if not user32.GetWindowRect(self.hwnd, ctypes.byref(outer)):
                raise VisualFailure("WINDOW_RECT_MISSING")
            if not user32.GetClientRect(self.hwnd, ctypes.byref(client)):
                raise VisualFailure("CLIENT_RECT_MISSING")
            outer_width = (outer.right - outer.left) + (width - client.right)
            outer_height = (outer.bottom - outer.top) + (height - client.bottom)
            x = max(0, work[0] + (work[2] - work[0] - outer_width) // 2)
            y = max(0, work[1] + (work[3] - work[1] - outer_height) // 2)
            placed = bool(
                user32.SetWindowPos(
                    self.hwnd,
                    0,
                    x,
                    y,
                    outer_width,
                    outer_height,
                    # The app clamps WM_WINDOWPOSCHANGING to the current
                    # monitor; the harness needs off-screen portrait
                    # sessions to exercise the landscape acceptance
                    # viewports, so skip that notification.
                    0x0004 | 0x0010 | 0x0400,
                )
            )
            if not placed:
                raise VisualFailure("WINDOW_RESIZE_FAILED")
            time.sleep(0.35)
            user32.GetClientRect(self.hwnd, ctypes.byref(client))
            if DEBUG:
                error = ctypes.get_last_error()
                class_name = ctypes.create_unicode_buffer(128)
                user32.GetClassNameW(self.hwnd, class_name, 128)
                print(
                    f"  resize {width}x{height}: hwnd={self.hwnd} class={class_name.value} "
                    f"placed={placed} err={error} "
                    f"outer={outer_width}x{outer_height} "
                    f"client={client.right}x{client.bottom}",
                    file=sys.stderr,
                    flush=True,
                )
            if client.right == width and client.bottom == height:
                return
        raise VisualFailure("WINDOW_VIEWPORT_MISMATCH")

    def foreground(self) -> bool:
        foreground = int(self.win32.user32.GetForegroundWindow() or 0)
        return foreground == self.hwnd

    def accessibility(self) -> dict[str, Any]:
        """Names and roles of the actionable controls in the current state.

        A screen reader needs a non-empty UIA name for every button, field,
        link and menu item. The capture fails instead of recording an unnamed
        control. Pane-sized pressable containers (the editor gutter and its
        viewport, which the text engine exposes as buttons) and the WebView
        document are structural; names are required on compact controls.
        """
        roles: dict[str, int] = {}
        unnamed: list[dict[str, Any]] = []
        document_checkboxes = 0
        for control in self.win.descendants():
            try:
                info = control.element_info
                role = (info.control_type or "").strip()
                if role not in ACTIONABLE_ROLES:
                    continue
                name = (info.name or "").strip()
                automation_id = (info.automation_id or "").strip()
                rectangle = control.rectangle()
                focusable = control.is_keyboard_focusable()
                enabled = control.is_enabled()
            except Exception:  # noqa: BLE001 - UIA element can go stale
                continue
            if role == "Button" and rectangle.bottom - rectangle.top > 200:
                continue
            # The nsh text engine renders Markdown task items as unlabeled
            # checkboxes beside their text; the app cannot name those nodes.
            # They remain in the reading order as document content.
            if role == "CheckBox" and not name:
                document_checkboxes += 1
                continue
            roles[role] = roles.get(role, 0) + 1
            if not name:
                unnamed.append(
                    {
                        "role": role,
                        "automation_id": automation_id,
                        "focusable": focusable,
                        "enabled": enabled,
                        "bounds": [
                            rectangle.left,
                            rectangle.top,
                            rectangle.right,
                            rectangle.bottom,
                        ],
                    }
                )
        return {
            "roles": dict(sorted(roles.items())),
            "control_count": sum(roles.values()),
            "document_checkboxes": document_checkboxes,
            "unnamed": unnamed,
        }

    def webview_rect(
        self, timeout: float = WAIT
    ) -> tuple[int, int, int, int]:
        deadline = time.monotonic() + timeout
        while True:
            left, top, right, bottom = self.client_rect()
            best: tuple[int, int, int, int] | None = None
            for control in self.win.descendants():
                try:
                    if not control.window_text().startswith("data:text/html"):
                        continue
                    rectangle = control.rectangle()
                    candidate = (
                        rectangle.left - left,
                        rectangle.top - top,
                        rectangle.right - left,
                        rectangle.bottom - top,
                    )
                    if candidate[2] > candidate[0] and candidate[3] > candidate[1]:
                        if best is None or (candidate[2] - candidate[0]) * (
                            candidate[3] - candidate[1]
                        ) > (best[2] - best[0]) * (best[3] - best[1]):
                            best = candidate
                except Exception:  # noqa: BLE001 - UIA element can go stale
                    continue
            if best is not None:
                return best
            if time.monotonic() >= deadline:
                raise VisualFailure("WEBVIEW_RECT_MISSING")
            time.sleep(0.25)

    def client_rect(self) -> tuple[int, int, int, int]:
        client = wt.RECT()
        if not self.win32.user32.GetClientRect(self.hwnd, ctypes.byref(client)):
            raise VisualFailure("CLIENT_RECT_MISSING")
        origin = wt.POINT(0, 0)
        if not self.win32.user32.ClientToScreen(self.hwnd, ctypes.byref(origin)):
            raise VisualFailure("CLIENT_ORIGIN_MISSING")
        return origin.x, origin.y, origin.x + client.right, origin.y + client.bottom

    def window_size(self) -> tuple[int, int]:
        outer = wt.RECT()
        if not self.win32.user32.GetWindowRect(self.hwnd, ctypes.byref(outer)):
            raise VisualFailure("WINDOW_RECT_MISSING")
        return outer.right - outer.left, outer.bottom - outer.top

    def make_topmost(self) -> bool:
        user32 = self.win32.user32
        placed = bool(
            user32.SetWindowPos(self.hwnd, -1, 0, 0, 0, 0, 0x0001 | 0x0002 | 0x0010)
        )
        try:
            ctypes.windll.dwmapi.DwmFlush()
        except (AttributeError, OSError):
            pass
        time.sleep(FRAME)
        return placed

    def obstruction(self) -> dict[str, Any]:
        left, top, right, bottom = self.client_rect()
        user32 = self.win32.user32
        foreground = int(user32.GetForegroundWindow() or 0)
        points = {
            "center": ((left + right) // 2, (top + bottom) // 2),
            "top_left": (left + 8, top + 8),
            "top_right": (right - 8, top + 8),
            "bottom_left": (left + 8, bottom - 8),
            "bottom_right": (right - 8, bottom - 8),
        }
        hits: dict[str, bool] = {}
        for name, (x, y) in points.items():
            hit = int(user32.WindowFromPoint(wt.POINT(x, y)) or 0)
            hits[name] = bool(hit) and (
                hit == self.hwnd or bool(user32.IsChild(self.hwnd, hit))
            )
        return {
            "foreground_window_matches": foreground == self.hwnd,
            "point_hits_app": hits,
            "visible": bool(user32.IsWindowVisible(self.hwnd)),
        }

    def close(self) -> None:
        try:
            self.win32.post_close(self.hwnd)
            self.win.close()
        except Exception:  # noqa: BLE001 - best effort
            pass


def launch(
    win32: Win32,
    exe: Path,
    environment: dict[str, str],
    cwd: Path,
) -> tuple[subprocess.Popen[bytes], AppWindow]:
    from pywinauto import Application

    process = subprocess.Popen(
        [str(exe)],
        env=environment,
        cwd=str(cwd),
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
    )
    try:
        application = Application(backend="uia").connect(
            process=process.pid, timeout=WAIT
        )
        window = application.top_window()
        window.wait("exists visible enabled ready", timeout=WAIT)
    except Exception as error:  # noqa: BLE001 - pywinauto raises several types
        process.terminate()
        process.wait(timeout=5)
        raise VisualFailure("WINDOW_CONNECT_FAILED", safe_exception_name(error)) from None
    return process, AppWindow(win32, window)


def write_settings(path: Path, mode: str, language: str, server: ReviewServer | None) -> None:
    lines = [
        f'theme = "{mode}"',
        f'language = "{language}"',
        "show-welcome-on-startup = true",
    ]
    if server is not None:
        lines.extend(
            [
                'model-provider = "openai-chat"',
                'model-name = "goal09-review"',
                f'model-base-url = "{server.base_url}"',
                f'model-environment-key-identity = "{credential_identity(server.port)}"',
            ]
        )
    path.write_text("\n".join(lines) + "\n", encoding="utf-8")


def fixture(workspace: Path) -> Path:
    workspace.mkdir(parents=True, exist_ok=True)
    (workspace / "AGENTS.md").write_text(
        "# Project instructions\n\nKeep answers concise.\n", encoding="utf-8"
    )
    plan = workspace / "plan.md"
    plan.write_text(
        "# Release plan\n\nDescribe the next release and list **one** risk.\n\n"
        "- [ ] verify install\n- [ ] verify uninstall\n",
        encoding="utf-8",
    )
    (workspace / "other.md").write_text("# Other\n", encoding="utf-8")
    return plan


def prepare_state(window: AppWindow, state: str, language: str, plan: Path) -> None:
    labels = LABELS[language]
    if state == "clean-profile-welcome":
        window.wait_text(labels["welcome"])
        return
    if state == "new-unsaved-document":
        window.wait_text(labels["welcome"])
        window.click(labels["new"])
        window.wait_contains(labels["untitled"])
        return
    if state == "settings":
        window.click(labels["settings"])
        window.wait_text(labels["settings"])
        return
    if state == "effective-agent-context":
        window.click(labels["context"])
        window.wait_text(labels["context_ready"])
        return
    if state == "ordinary-edit":
        window.click(labels["edit"])
        window.wait_text("Markdown source")
        return
    if state == "native-preview-or-split":
        window.click(labels["preview"])
        window.wait_text("Release plan")
        return
    if state == "web-compatibility-preview":
        window.click(labels["more"])
        window.click(labels["web"])
        window.wait_text(labels["web_restricted"])
        return
    if state == "external-change-conflict":
        window.click(labels["edit"])
        window.set_editor_text(
            "# Release plan\n\nLocal edit.\n\n- [ ] verify install\n"
        )
        plan.write_text("# External change\n", encoding="utf-8")
        window.click(labels["save"])
        window.wait_text(labels["conflict"])
        return
    if state in REVIEW_STATES:
        if DEBUG:
            print("  review: Review", file=sys.stderr, flush=True)
        for _attempt in range(2):
            window.click(labels["review"])
            try:
                window.wait_text(labels["send"], timeout=5.0)
                break
            except VisualFailure:
                continue
        else:
            raise VisualFailure("REVIEW_CONSENT_MISSING")
        if DEBUG:
            print("  review: Send", file=sys.stderr, flush=True)
        window.click(labels["send"])
        if state == "review-loading":
            window.wait_text(labels["reviewing"])
            return
        if DEBUG:
            print("  review: ready", file=sys.stderr, flush=True)
        window.wait_text(labels["ready"], timeout=30.0)
        if state == "review-result-questions":
            window.scroll_until_text(labels["question"])
            window.wait_contains(labels["question_text"])
            return
        window.set_editor_text("# Release plan\n\nLocal edit.\n")
        window.wait_text(labels["stale"], timeout=20.0)
        return
    raise VisualFailure("STATE_UNKNOWN")


def composite_web(image: Any, page_png: bytes, rect: tuple[int, int, int, int]) -> Any:
    from PIL import Image

    page = Image.open(io.BytesIO(page_png)).convert("RGB")
    left = max(0, rect[0])
    top = max(0, rect[1])
    right = min(image.width, rect[2])
    bottom = min(image.height, rect[3])
    if right <= left or bottom <= top:
        raise VisualFailure("WEBVIEW_RECT_OUTSIDE_WINDOW")
    page = page.resize((right - left, bottom - top), Image.LANCZOS)
    composed = image.copy()
    composed.paste(page, (left, top))
    return composed


def expected_background(mode: str) -> tuple[int, int, int]:
    return THEME_BACKGROUNDS[mode]


def color_distance(left: tuple[int, int, int], right: tuple[int, int, int]) -> int:
    return max(abs(a - b) for a, b in zip(left, right))


def web_content_present(
    image: Any, mode: str, rect: tuple[int, int, int, int]
) -> bool:
    """Whether the compatibility pane has more than its empty background.

    The page is composited into the pane rectangle, so the check only looks
    there. A page with text has many pixels away from the theme background; an
    empty pane has almost none. Sampling every four pixels and requiring a
    tenth of a percent keeps a short release plan above the floor at the wide
    viewport, where the text occupies little of the pane, while an empty pane
    stays well below it.
    """
    background = expected_background(mode)
    left = max(0, rect[0])
    top = max(0, rect[1])
    right = min(image.width, rect[2])
    bottom = min(image.height, rect[3])
    changed = 0
    total = 0
    for y in range(top, bottom, 4):
        for x in range(left, right, 4):
            total += 1
            pixel = image.getpixel((x, y))
            if color_distance(pixel, background) > 24:
                changed += 1
    return total > 0 and changed >= max(64, total // 1000)


def capture_cell(
    win32: Win32,
    exe: Path,
    out: Path,
    cell: Cell,
    source: SourceBinding,
    executable: dict[str, Any],
) -> dict[str, Any]:
    started = utc_now()
    case_root = out / "cells" / cell.cell_id
    case_root.mkdir(parents=True, exist_ok=True)
    workspace = case_root / "workspace"
    config = case_root / "config"
    data = case_root / "data"
    for directory in (workspace, config, data):
        directory.mkdir(exist_ok=True)
    plan = fixture(workspace)
    server = ReviewServer(hold=cell.state == "review-loading") if cell.state in REVIEW_STATES else None
    if server is not None:
        server.start()
    write_settings(config / "settings.toml", cell.mode, cell.language, server)
    environment = dict(os.environ)
    environment.update(
        {
            "MARKTURBO_CONFIG_DIR": str(config),
            "MARKTURBO_DATA_DIR": str(data),
            "RUST_LOG": "error",
        }
    )
    if cell.state not in {"clean-profile-welcome", "new-unsaved-document", "settings"}:
        environment["MARKTURBO_OPEN"] = str(plan)
    if server is not None:
        environment["OPENAI_API_KEY"] = "goal09-loopback-test-key"
    web_port = free_port() if cell.state == "web-compatibility-preview" else None
    if web_port is not None:
        environment["WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS"] = (
            f"--remote-debugging-port={web_port} --remote-allow-origins=*"
        )

    process: subprocess.Popen[bytes] | None = None
    window: AppWindow | None = None
    receipt: dict[str, Any] = {
        "schema": RECEIPT_SCHEMA,
        "cell": asdict(cell),
        "cell_id": cell.cell_id,
        "status": "FAILED",
        "started_at": started,
        "source": asdict(source),
        "executable": executable,
    }
    try:
        process, window = launch(win32, exe, environment, workspace)
        focused = window.foreground()
        dpi = int(win32.user32.GetDpiForWindow(window.hwnd))
        width, height = physical_viewport(cell.viewport, dpi)
        window.resize_client(width, height, dpi_awareness()[2])
        prepare_state(window, cell.state, cell.language, plan)
        topmost = window.make_topmost()
        accessibility = window.accessibility()
        receipt["accessibility"] = accessibility
        if DEBUG:
            print(f"  accessibility={accessibility}", file=sys.stderr, flush=True)
        if accessibility["unnamed"]:
            raise VisualFailure("ACCESSIBILITY_UNNAMED_CONTROL")
        obstruction = window.obstruction()
        if not obstruction["visible"]:
            raise VisualFailure("CAPTURE_WINDOW_NOT_VISIBLE")
        image = capture_window(window.hwnd)
        if image.size != (width, height):
            raise VisualFailure("CAPTURE_SIZE_MISMATCH")
        web_evidence = None
        web_rect = (0, 0, image.width, image.height)
        if web_port is not None:
            time.sleep(0.4)
            page = cdp_page_png(web_port)
            rect = window.webview_rect()
            if DEBUG:
                debug_page = Path(tempfile.gettempdir()) / f"{cell.cell_id}-page.png"
                debug_raw = Path(tempfile.gettempdir()) / f"{cell.cell_id}-raw.png"
                debug_page.write_bytes(page)
                image.save(debug_raw)
                print(
                    f"  web page={len(page)} bytes rect={rect} raw={image.size} "
                    f"files={debug_page} {debug_raw}",
                    file=sys.stderr,
                    flush=True,
                )
            image = composite_web(image, page, rect)
            web_rect = rect
            web_evidence = {
                "method": "WebView2 CDP Page.captureScreenshot",
                "page_byte_count": len(page),
                "page_sha256": hashlib.sha256(page).hexdigest(),
                "rect_client_px": list(rect),
            }
        observed = image.getpixel((2, 2))
        expected = expected_background(cell.mode)
        distance = color_distance(observed, expected)
        if distance > 10:
            raise VisualFailure("CAPTURE_EFFECTIVE_MODE_MISMATCH")
        if cell.state == "web-compatibility-preview" and not web_content_present(
            image, cell.mode, web_rect
        ):
            raise VisualFailure("WEBVIEW_CONTENT_MISSING")
        destination = case_root / "capture.png"
        image.save(destination, format="PNG")
        fingerprint = sha256_file(destination)
        receipt.update(
            {
                "status": "CAPTURED",
                "completed_at": utc_now(),
                "viewport": {
                    "name": cell.viewport,
                    "dip": list(VIEWPORTS[cell.viewport]),
                    "dpi": dpi,
                    "client_px": [width, height],
                    "window_px": list(window.window_size()),
                },
                "image": {
                    "width": image.width,
                    "height": image.height,
                    "byte_count": fingerprint.byte_count,
                    "sha256": fingerprint.sha256,
                },
                "capture_method": "PrintWindow+CDP" if web_evidence else "PrintWindow",
                "web_capture": web_evidence,
                "obstruction": {
                    **obstruction,
                    "session_foreground": focused,
                    "topmost_requested": topmost,
                },
                "accessibility": accessibility,
                "effective_mode": {
                    "requested": cell.mode,
                    "background_rgb": list(observed),
                    "expected_rgb": list(expected),
                    "max_channel_distance": distance,
                },
                "state_markers": [cell.state],
            }
        )
    except VisualFailure as error:
        receipt["reason_code"] = error.code
        if error.detail:
            receipt["failure_type"] = error.detail
        if DEBUG:
            print(f"  {error.code}: {error.detail}", file=sys.stderr, flush=True)
        if window is not None:
            try:
                capture_window(window.hwnd).save(case_root / "failure.png", format="PNG")
            except Exception:  # noqa: BLE001
                pass
    except (HarnessFailure, HarnessBlocked) as error:
        receipt["reason_code"] = error.code
        if error.detail:
            receipt["failure_type"] = error.detail
    except Exception as error:  # noqa: BLE001 - a cell must not abort the matrix
        receipt["reason_code"] = "CAPTURE_FAILED"
        receipt["failure_type"] = safe_exception_name(error)
        if DEBUG:
            print(f"  {type(error).__name__}: {error}", file=sys.stderr, flush=True)
    finally:
        if window is not None:
            window.close()
        if process is not None:
            try:
                process.terminate()
                process.wait(timeout=5)
            except Exception:  # noqa: BLE001
                pass
        if server is not None:
            server.close()
        receipt.setdefault("completed_at", utc_now())
        (case_root / "receipt.json").write_text(
            json.dumps(receipt, indent=2, sort_keys=True, ensure_ascii=True) + "\n",
            encoding="utf-8",
        )
    return receipt


def parse_csv(value: str, allowed: Sequence[str]) -> list[str]:
    selected = [item.strip() for item in value.split(",") if item.strip()]
    unknown = [item for item in selected if item not in allowed]
    if unknown:
        raise argparse.ArgumentTypeError(
            f"unknown value(s): {', '.join(unknown)}"
        )
    return selected


def parser() -> argparse.ArgumentParser:
    result = argparse.ArgumentParser(
        prog="mt.py visual",
        description="Capture the Goal 09 visual-acceptance matrix.",
    )
    result.add_argument("--exe", required=True, type=Path)
    result.add_argument("--out", required=True, type=Path)
    result.add_argument("--states", default=",".join(STATES))
    result.add_argument("--languages", default=",".join(LANGUAGES))
    result.add_argument("--viewports", default=",".join(VIEWPORTS))
    result.add_argument("--modes", default=",".join(MODES))
    result.add_argument(
        "--allow-dirty",
        action="store_true",
        help="capture from a dirty worktree and bind the generated tree hash",
    )
    result.add_argument("--debug", action="store_true", help=argparse.SUPPRESS)
    return result


def main(argv: Sequence[str] | None = None) -> int:
    if sys.platform != "win32":
        print("error: Goal 09 visual capture requires Windows", file=sys.stderr)
        return 2
    arguments = parser().parse_args(argv)
    global DEBUG
    DEBUG = bool(arguments.debug)
    try:
        states = parse_csv(arguments.states, STATES)
        languages = parse_csv(arguments.languages, LANGUAGES)
        viewports = parse_csv(arguments.viewports, tuple(VIEWPORTS))
        modes = parse_csv(arguments.modes, MODES)
        source = source_binding(require_clean=not arguments.allow_dirty)
        executable = executable_binding(arguments.exe)
    except (VisualFailure, argparse.ArgumentTypeError) as error:
        print(f"error: {error}", file=sys.stderr)
        return 1
    if not states or not languages or not viewports or not modes:
        print("error: each matrix axis needs at least one value", file=sys.stderr)
        return 1
    out = arguments.out.resolve()
    out.mkdir(parents=True, exist_ok=True)
    try:
        win32 = Win32()
    except Exception as error:  # noqa: BLE001
        print(f"error: Windows APIs unavailable: {safe_exception_name(error)}", file=sys.stderr)
        return 2
    cells = matrix_cells(states, languages, viewports, modes)
    summary: dict[str, Any] = {
        "schema": SCHEMA,
        "started_at": utc_now(),
        "source": asdict(source),
        "executable": executable,
        "cells": [],
    }
    for index, cell in enumerate(cells, 1):
        print(f"[{index}/{len(cells)}] {cell.cell_id}", flush=True)
        receipt = capture_cell(win32, arguments.exe.resolve(), out, cell, source, executable)
        summary["cells"].append(
            {
                "cell_id": cell.cell_id,
                "status": receipt["status"],
                "reason_code": receipt.get("reason_code"),
            }
        )
    captured = sum(item["status"] == "CAPTURED" for item in summary["cells"])
    summary["status"] = "PASS" if captured == len(cells) else "FAIL"
    summary["completed_at"] = utc_now()
    summary["summary"] = {
        "cell_count": len(cells),
        "captured_count": captured,
        "failed_count": len(cells) - captured,
    }
    (out / "matrix.json").write_text(
        json.dumps(summary, indent=2, sort_keys=True, ensure_ascii=True) + "\n",
        encoding="utf-8",
    )
    print(f"{summary['status']}: {captured}/{len(cells)} captured", flush=True)
    return 0 if summary["status"] == "PASS" else 1


if __name__ == "__main__":
    raise SystemExit(main())
