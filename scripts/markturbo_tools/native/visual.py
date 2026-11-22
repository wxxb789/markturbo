"""Capture the Goal 09 Windows visual-acceptance matrix from a release build.

The matrix is eleven product states at the configured minimum, default and wide
DIP viewports in the two effective color modes and both interface languages.
Each cell launches the packaged app on an isolated profile under the system
temporary directory and drives the Afterglow UI with the native acceptance
harness's own UIA steps (`runtime.NativeHarness`): the title menu's
`title-menu-*` rows, controls found by AutomationId, typed input, and a real
pointer hold on `consent-send`. Every cell proves it
reached its named state by the state's own controls before it captures; a cell
that cannot fails with a reason code prefixed by its state (for example
`ORDINARY_EDIT_DOCUMENT_NOT_OPEN`) and keeps a `failure.png` of what it reached.
It then refuses an obstructed or off-screen window, verifies the effective
background against the shipped theme, and writes one PNG plus a content-free
JSON receipt.

The profile - settings, logs, recovery payloads, the fixture workspace and an
empty `CODEX_HOME`, so no instructions of the user reach Context or a request -
is deleted with the cell, so an output tree never carries the local paths it
recorded. The Review cells talk to a process-local OpenAI Responses loopback
that refuses a request sent before the consent hold and returns a fixed review;
the child gets one synthetic key in its environment only. A typed edit is
proven by the UIA tree showing it (the editor's accessible readout or the
preview), never through the clipboard, which the run leaves untouched.

Run from the repository root, on an active Windows 11 x64 desktop, with a
packaged `MarkTurbo.exe` built from the same source revision::

    uv run --locked --project scripts scripts/mt.py visual -- \\
        --exe target/release/markturbo.exe \\
        --out target/native-acceptance/goal-09

`--out` names a scratch directory: the capture rewrites the cells it writes and
has no existing-output guard, so it never points at retained evidence.

This is capture tooling, not owner approval or first-time-user evidence.
"""

from __future__ import annotations

import argparse
import base64
import contextlib
import ctypes
import ctypes.wintypes as wt
import datetime as dt
import hashlib
import io
import json
import os
import shutil
import socket
import struct
import subprocess
import sys
import tempfile
import threading
import time
import urllib.parse
import urllib.request
import uuid
from collections.abc import Callable, Iterable, Iterator, Sequence
from dataclasses import asdict, dataclass
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path
from typing import Any

from .evidence import credential_target
from .goal07 import (
    CONSENT_HOLD_SECONDS,
    CONSENT_SEND_AUTOMATION_ID,
    REQUEST_SENT_TIMEOUT_SECONDS,
    REVIEW_ANSWER_INPUT_AUTOMATION_ID,
)
from .runtime import (
    MENU_EDIT_AUTOMATION_ID,
    MENU_SAVE_AUTOMATION_ID,
    MENU_WEB_AUTOMATION_ID,
    HarnessBlocked,
    HarnessFailure,
    NativeHarness,
    RunningApp,
    Win32,
    load_pywinauto,
    phase,
    safe_exception_name,
    sha256_file,
    wait_until,
)

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
# The states that launch on a clean profile with no document.
EMPTY_STATES = frozenset({"clean-profile-welcome", "new-unsaved-document", "settings"})
UI_TIMEOUT = 20.0
# What a state may take to settle once it is proven: a card's glide, a model
# item's ignition and stagger, the hover the pointer left behind.
SETTLE_SECONDS = 1.2
# How long a cell retries removing its profile before it ends the profile's
# WebView2 browser (and again after).
PROFILE_REMOVAL_SECONDS = 3.0
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

# The AutomationIds (the views' `test_id`s) that drive and prove each state.
# They are language-independent, so one plan drives both interface languages.
WELCOME_NEW_FILE_AUTOMATION_ID = "welcome-new-file"
TITLE_FILE_AUTOMATION_ID = "title-file"
TITLE_REVIEW_AUTOMATION_ID = "title-review"
MENU_SPLIT_AUTOMATION_ID = "title-menu-split"
MENU_CONTEXT_AUTOMATION_ID = "title-menu-context"
MENU_SETTINGS_AUTOMATION_ID = "title-menu-settings"
WEB_VIEW_AUTOMATION_ID = "web-view"
CONTEXT_CLOSE_AUTOMATION_ID = "context-close"
CONFLICT_KEEP_MINE_AUTOMATION_ID = "conflict-keep-mine"
# The page's primary action. (Its `settings-close` is off screen in UIA and
# unpainted at rest, so it cannot prove the page.)
SETTINGS_SAVE_AUTOMATION_ID = "settings-save"
REVIEW_CANCEL_AUTOMATION_ID = "review-cancel"
REVIEW_AGAIN_AUTOMATION_ID = "review-again"
# `state.rs` names a new document's tab with `Key::Untitled`.
UNTITLED = {"en-us": "Untitled", "zh-cn": "未命名"}
# `i18n::effective_context_choice_label(language, false)`: a source's include
# toggle. Its `context-include-<n>` id sits on a view UIA does not expose, so
# the checkbox is found by its accessible label.
CONTEXT_SOURCE_TOGGLE = {
    "en-us": "Include context source (unchecked)",
    "zh-cn": "包含 Context 来源（未选择）",
}
# Ctrl+End: the caret to the document's end, collapsing a Select All.
VK_END = 0x23

# Fixture text only: nothing a user wrote reaches a capture or a receipt.
PLAN_NAME = "plan.md"
PLAN_TEXT = (
    "# Release plan\n\nDescribe the next release and list **one** risk.\n\n"
    "- [ ] verify install\n- [ ] verify uninstall\n"
)
PLAN_HEADING = "Release plan"
AGENTS_TEXT = "# Project instructions\n\nKeep answers concise.\n"
NEW_DOCUMENT_TEXT = "# Launch notes"
ORDINARY_EDIT_TEXT = "Ship after the install check passes."
CONFLICT_EDIT_TEXT = "Hold the release until the rollback is rehearsed."
STALE_EDIT_TEXT = "Name the release owner."
EXTERNAL_TEXT = "# Release plan\n\nChanged by another editor.\n"

# The model request is the only non-loopback-shaped operation in this matrix.
# It reaches a process-local server that returns a fixed review; no credential
# or document content is written to a receipt.
REVIEW_QUESTIONS = (
    ("Which risk should the plan prioritize?", "high", "It changes the risk section."),
    ("Who signs off on the release?", "medium", "It names the owner of the go decision."),
    ("Is uninstall in scope for this release?", "low", "It decides the second check."),
)
REVIEW_FIRST_QUESTION = REVIEW_QUESTIONS[0][0]
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
            {"question": question, "priority": priority, "impact": impact}
            for question, priority, impact in REVIEW_QUESTIONS
        ],
    },
    ensure_ascii=True,
    separators=(",", ":"),
)
REVIEW_PROVIDER = "openai-responses"
REVIEW_MODEL = "goal09-review"
# A held Review answers when its cell ends, or after this long.
REVIEW_HOLD_SECONDS = 120.0
REVIEW_ANSWER_TIMEOUT_SECONDS = 30.0

# `ui/tokens.rs` `LIGHT.room` / `DARK.room`: the window ground under the title
# row, where the effective-mode sample is taken.
THEME_BACKGROUNDS = {
    "light": (0xF2, 0xF0, 0xEB),
    "dark": (0x10, 0x11, 0x13),
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


def cdp_page_target(port: int, timeout: float = UI_TIMEOUT) -> str:
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


def state_code(state: str) -> str:
    """The reason-code prefix of a state's own failures."""
    return state.upper().replace("-", "_")


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


class ReviewServer:
    """A loopback OpenAI Responses endpoint for the three Review cells.

    It answers one read-only Review with `REVIEW_JSON`, and refuses (409) any
    request that arrives before `allow` - the moment the harness starts the
    consent hold - so a Review sent without consent fails its cell. With
    `hold`, the answer waits until `close` (the loading cell captures while it
    waits). Only counts and a reason code are kept; no request body is.
    """

    def __init__(self, hold: bool) -> None:
        self.hold = hold
        self.consented = threading.Event()
        self.release = threading.Event()
        self._lock = threading.Lock()
        self._request_count = 0
        self._refusal: str | None = None
        self.server = ThreadingHTTPServer(("127.0.0.1", 0), self._handler())
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

    @property
    def request_count(self) -> int:
        with self._lock:
            return self._request_count

    @property
    def refusal(self) -> str | None:
        with self._lock:
            return self._refusal

    def allow(self) -> None:
        self.consented.set()

    def start(self) -> None:
        self.thread.start()

    def close(self) -> None:
        self.release.set()
        self.server.shutdown()
        self.server.server_close()

    def _refuse(self, code: str) -> None:
        with self._lock:
            self._refusal = self._refusal or code

    def _handler(self) -> type[BaseHTTPRequestHandler]:
        outer = self

        class Handler(BaseHTTPRequestHandler):
            def log_message(self, *_args: Any) -> None:
                return

            def _reply(self, status: int, body: bytes = b"") -> None:
                try:
                    self.send_response(status)
                    self.send_header("content-type", "application/json")
                    self.send_header("content-length", str(len(body)))
                    self.send_header("connection", "close")
                    self.end_headers()
                    self.wfile.write(body)
                except (BrokenPipeError, ConnectionResetError):
                    # The cell closed its app after the capture; the response
                    # has no reader, and that is not a matrix failure.
                    return

            def do_POST(self) -> None:  # noqa: N802 - stdlib handler name
                length = int(self.headers.get("content-length", "0"))
                body = self.rfile.read(length)
                if self.path != "/v1/responses":
                    outer._refuse("REVIEW_REQUEST_PATH_MISMATCH")
                    self._reply(404)
                    return
                if not outer.consented.is_set():
                    outer._refuse("REVIEW_REQUEST_BEFORE_CONSENT")
                    self._reply(409)
                    return
                if b"read_only_review" not in body:
                    outer._refuse("REVIEW_REQUEST_NOT_READ_ONLY")
                    self._reply(409)
                    return
                with outer._lock:
                    outer._request_count += 1
                if outer.hold:
                    outer.release.wait(REVIEW_HOLD_SECONDS)
                self._reply(200, review_response_body())

        return Handler


def review_response_body() -> bytes:
    """One completed Responses object carrying `REVIEW_JSON`."""
    return json.dumps(
        {
            "id": "resp-goal09-review",
            "object": "response",
            "status": "completed",
            "model": REVIEW_MODEL,
            "output": [
                {
                    "type": "message",
                    "id": "msg-goal09-review",
                    "role": "assistant",
                    "content": [
                        {"type": "output_text", "text": REVIEW_JSON, "annotations": []}
                    ],
                }
            ],
        },
        ensure_ascii=True,
        separators=(",", ":"),
    ).encode("utf-8")


def credential_identity(port: int) -> str:
    return credential_target(f"http://127.0.0.1:{port}/v1/", REVIEW_PROVIDER)


class AppWindow:
    """The window geometry, accessibility and obstruction checks of a cell."""

    def __init__(self, win32: Win32, win: Any) -> None:
        self.win32 = win32
        self.win = win
        self.hwnd = int(win.handle)

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
                print(
                    f"  resize {width}x{height}: placed={placed} "
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

    def webview_rect(self, timeout: float = UI_TIMEOUT) -> tuple[int, int, int, int]:
        """The compatibility WebView's client rectangle (`ui/preview.rs`
        `web-view`), as the largest control carrying its AutomationId."""

        def locate() -> tuple[int, int, int, int] | None:
            left, top, _right, _bottom = self.client_rect()
            best: tuple[int, int, int, int] | None = None
            for control in self.win.descendants():
                try:
                    if control.element_info.automation_id != WEB_VIEW_AUTOMATION_ID:
                        continue
                    rectangle = control.rectangle()
                except Exception:  # noqa: BLE001 - UIA element can go stale
                    continue
                candidate = (
                    rectangle.left - left,
                    rectangle.top - top,
                    rectangle.right - left,
                    rectangle.bottom - top,
                )
                if candidate[2] <= candidate[0] or candidate[3] <= candidate[1]:
                    continue
                if best is None or area(candidate) > area(best):
                    best = candidate
            return best

        return wait_until(locate, timeout, "WEBVIEW_RECT_MISSING", interval=0.25)

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

    def park_pointer(self) -> None:
        """Move the pointer off every control, so no hover reaches a capture:
        the bottom-right screen corner, or the window's own corner (its status
        line's end) when the window covers the screen's."""
        user32 = self.win32.user32
        x = int(user32.GetSystemMetrics(0)) - 1
        y = int(user32.GetSystemMetrics(1)) - 1
        left, top, right, bottom = self.client_rect()
        if left <= x < right and top <= y < bottom:
            x, y = right - 2, bottom - 2
        user32.SetCursorPos(x, y)

    def make_topmost(self) -> bool:
        user32 = self.win32.user32
        placed = bool(
            user32.SetWindowPos(self.hwnd, -1, 0, 0, 0, 0, 0x0001 | 0x0002 | 0x0010)
        )
        try:
            ctypes.windll.dwmapi.DwmFlush()
        except (AttributeError, OSError):
            pass
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


def area(rect: tuple[int, int, int, int]) -> int:
    return (rect[2] - rect[0]) * (rect[3] - rect[1])


class VisualHarness(NativeHarness):
    """The native acceptance harness's UIA steps, driving one cell to its state.

    `prepare` runs the state's plan inside `phase(<STATE>)`, so every failure
    carries the state's prefix, and returns the AutomationIds that proved the
    state (the receipt's content-free `state_markers`).
    """

    def __init__(self, *args: Any, credential: str | None = None) -> None:
        super().__init__(*args)
        self._credential = credential
        # Pointer holds on `consent-send` a Review cell needed (0 elsewhere).
        self.consent_holds = 0

    def openai_api_key_for_child(self) -> str | None:
        return self._credential

    # -- generic steps -------------------------------------------------------

    def require(
        self, app: RunningApp, automation_id: str, code: str, control_type: str = "Button",
        timeout: float | None = None,
    ) -> Any:
        """The one ready control with `automation_id`, else `code`."""
        return self.find_control(
            app, automation_id, control_type, code, f"{code}_UIA_CONTRACT_MISMATCH", timeout
        )

    def present(self, app: RunningApp, automation_id: str, control_type: str = "Button") -> bool:
        return (
            self.control_by_id(
                app.hwnd, automation_id, control_type, "STATE_UIA_CONTRACT_MISMATCH"
            )
            is not None
        )

    def press(self, app: RunningApp, automation_id: str, code: str) -> None:
        control = self.require(app, automation_id, f"{code}_MISSING")
        self.invoke_control(control, f"{code}_INVOKE_FAILED")

    def wait_text(
        self, app: RunningApp, name: str, code: str, timeout: float | None = None
    ) -> None:
        self.wait_name(app, name, "Text", code, timeout)

    def wait_title(self, app: RunningApp, title: str, code: str) -> None:
        """The title row names the active tab `title` (`title-file`'s label)."""

        def named() -> bool:
            control = self.control_by_id(
                app.hwnd, TITLE_FILE_AUTOMATION_ID, "Button", f"{code}_UIA_CONTRACT_MISMATCH"
            )
            return control is not None and control.element_info.name == title

        wait_until(named, self.ui_timeout, code, interval=0.05)

    def shows_text(self, app: RunningApp, text: str) -> bool:
        """Whether a control's UIA name holds `text`: the editor's accessible
        readout of its document, or the preview rendering it."""
        for control in app.window.descendants():
            try:
                if text in (control.element_info.name or ""):
                    return True
            except Exception:  # noqa: BLE001 - UIA element can go stale
                continue
        return False

    def type_at_end(self, app: RunningApp, text: str) -> None:
        """Type `text` at the document's end, the way a user does, until the
        document shows it. The clipboard is never touched."""
        self.focus_editor(app)
        # A layout switch just rebuilt the editor: its key bindings reach the
        # window with a later frame, and Ctrl must be up before the text, or
        # the caret stays put and the first character is lost.
        self.win32.wait_frames()
        self.win32.send_shortcut(app.hwnd, VK_END)
        self.win32.wait_frames()
        self.win32.send_unicode(app.hwnd, text)
        wait_until(lambda: self.shows_text(app, text), self.ui_timeout, "TYPED_EDIT_MISSING")

    def open_document(self, app: RunningApp) -> None:
        """The fixture document is the active tab and its editor is ready."""
        self.wait_title(app, PLAN_NAME, "DOCUMENT_NOT_OPEN")
        self.editor_control(app)

    def source_only(self, app: RunningApp) -> None:
        """The Edit layout: the editor, and no native preview beside it."""
        self.menu_command(app, MENU_EDIT_AUTOMATION_ID)
        self.editor_control(app)
        wait_until(
            lambda: not self.name_present(app, PLAN_HEADING, "Text"),
            self.ui_timeout,
            "PREVIEW_STILL_SHOWN",
        )

    # -- the eleven states ---------------------------------------------------

    def prepare(
        self, app: RunningApp, cell: Cell, plan: Path, server: ReviewServer | None
    ) -> list[str]:
        with phase(state_code(cell.state)):
            return self._plans()[cell.state](app, cell, plan, server)

    def _plans(self) -> dict[str, Callable[..., list[str]]]:
        return {
            "clean-profile-welcome": self.welcome,
            "new-unsaved-document": self.new_unsaved_document,
            "ordinary-edit": self.ordinary_edit,
            "native-preview-or-split": self.split,
            "web-compatibility-preview": self.web_preview,
            "review-loading": self.review,
            "review-result-questions": self.review,
            "review-stale": self.review,
            "effective-agent-context": self.context,
            "external-change-conflict": self.conflict,
            "settings": self.settings_page,
        }

    def welcome(self, app: RunningApp, *_: Any) -> list[str]:
        self.require(app, WELCOME_NEW_FILE_AUTOMATION_ID, "WELCOME_MISSING")
        if self.present(app, TITLE_FILE_AUTOMATION_ID):
            raise HarnessFailure("DOCUMENT_OPEN")
        return [WELCOME_NEW_FILE_AUTOMATION_ID]

    def new_unsaved_document(self, app: RunningApp, cell: Cell, *_: Any) -> list[str]:
        self.press(app, WELCOME_NEW_FILE_AUTOMATION_ID, "WELCOME_NEW_FILE")
        self.wait_title(app, UNTITLED[cell.language], "UNTITLED_TAB_MISSING")
        self.type_at_end(app, NEW_DOCUMENT_TEXT)
        return [TITLE_FILE_AUTOMATION_ID, "editor"]

    def ordinary_edit(self, app: RunningApp, *_: Any) -> list[str]:
        self.open_document(app)
        self.source_only(app)
        self.type_at_end(app, ORDINARY_EDIT_TEXT)
        return [TITLE_FILE_AUTOMATION_ID, MENU_EDIT_AUTOMATION_ID, "editor"]

    def split(self, app: RunningApp, *_: Any) -> list[str]:
        self.open_document(app)
        self.menu_command(app, MENU_SPLIT_AUTOMATION_ID)
        self.editor_control(app)
        self.wait_text(app, PLAN_HEADING, "PREVIEW_MISSING")
        return [TITLE_FILE_AUTOMATION_ID, MENU_SPLIT_AUTOMATION_ID, "editor", "preview"]

    def web_preview(self, app: RunningApp, *_: Any) -> list[str]:
        self.open_document(app)
        self.menu_command(app, MENU_WEB_AUTOMATION_ID)
        self.require(app, WEB_VIEW_AUTOMATION_ID, "WEB_VIEW_MISSING")
        return [TITLE_FILE_AUTOMATION_ID, MENU_WEB_AUTOMATION_ID, WEB_VIEW_AUTOMATION_ID]

    def context(self, app: RunningApp, cell: Cell, *_: Any) -> list[str]:
        self.open_document(app)
        self.menu_command(app, MENU_CONTEXT_AUTOMATION_ID)
        self.require(app, CONTEXT_CLOSE_AUTOMATION_ID, "CONTEXT_FIELD_MISSING")
        # The workspace's AGENTS.md is the one source that applies.
        self.wait_name(
            app, CONTEXT_SOURCE_TOGGLE[cell.language], "CheckBox", "CONTEXT_SOURCE_MISSING"
        )
        return [
            TITLE_FILE_AUTOMATION_ID,
            MENU_CONTEXT_AUTOMATION_ID,
            CONTEXT_CLOSE_AUTOMATION_ID,
            "context-source-toggle",
        ]

    def conflict(self, app: RunningApp, _cell: Cell, plan: Path, _server: Any) -> list[str]:
        self.open_document(app)
        self.source_only(app)
        self.type_at_end(app, CONFLICT_EDIT_TEXT)
        plan.write_text(EXTERNAL_TEXT, encoding="utf-8", newline="\n")
        self.menu_command(app, MENU_SAVE_AUTOMATION_ID)
        self.require(app, CONFLICT_KEEP_MINE_AUTOMATION_ID, "CONFLICT_BANNER_MISSING")
        if plan.read_text(encoding="utf-8") != EXTERNAL_TEXT:
            raise HarnessFailure("SAVE_OVERWROTE_EXTERNAL_CHANGE")
        return [TITLE_FILE_AUTOMATION_ID, MENU_SAVE_AUTOMATION_ID, CONFLICT_KEEP_MINE_AUTOMATION_ID]

    def settings_page(self, app: RunningApp, *_: Any) -> list[str]:
        self.menu_command(app, MENU_SETTINGS_AUTOMATION_ID)
        self.require(app, SETTINGS_SAVE_AUTOMATION_ID, "SETTINGS_PAGE_MISSING")
        return [MENU_SETTINGS_AUTOMATION_ID, SETTINGS_SAVE_AUTOMATION_ID]

    def review(
        self, app: RunningApp, cell: Cell, _plan: Path, server: ReviewServer | None
    ) -> list[str]:
        """`Review` -> the consent card -> a pointer hold on `consent-send`
        (the only press that sends) -> the state."""
        if server is None:
            raise HarnessFailure("REVIEW_SERVER_MISSING")
        self.open_document(app)
        self.press(app, TITLE_REVIEW_AUTOMATION_ID, "TITLE_REVIEW")
        send = self.require(app, CONSENT_SEND_AUTOMATION_ID, "CONSENT_CARD_MISSING")
        if server.request_count or server.refusal:
            raise HarnessFailure("REVIEW_REQUEST_BEFORE_CONSENT")
        server.allow()
        # A hold that lands as the card settles can go unseen (about one cell
        # in twenty): while the card is still open and nothing was sent, hold
        # once more. The receipt keeps the count, so a second hold is visible.
        for attempt in (1, 2):
            self.consent_holds = attempt
            self.hold_control(app, send, CONSENT_HOLD_SECONDS, "CONSENT_HOLD")
            try:
                wait_until(
                    lambda: server.request_count > 0 or server.refusal,
                    REQUEST_SENT_TIMEOUT_SECONDS,
                    "REVIEW_REQUEST_NOT_SENT",
                )
                break
            except HarnessFailure as error:
                if attempt == 2 or error.code != "REVIEW_REQUEST_NOT_SENT":
                    raise
                if not self.present(app, CONSENT_SEND_AUTOMATION_ID):
                    raise
        if server.refusal:
            raise HarnessFailure(server.refusal)
        markers = [TITLE_REVIEW_AUTOMATION_ID, CONSENT_SEND_AUTOMATION_ID]
        if cell.state == "review-loading":
            self.require(app, REVIEW_CANCEL_AUTOMATION_ID, "REVIEW_LOADING_MISSING")
            return [*markers, REVIEW_CANCEL_AUTOMATION_ID]
        self.wait_text(
            app, REVIEW_FIRST_QUESTION, "REVIEW_RESULT_MISSING", REVIEW_ANSWER_TIMEOUT_SECONDS
        )
        self.require(app, REVIEW_ANSWER_INPUT_AUTOMATION_ID, "REVIEW_ANSWER_INPUT_MISSING", "Edit")
        markers += ["review-question", REVIEW_ANSWER_INPUT_AUTOMATION_ID]
        if cell.state == "review-result-questions":
            return markers
        self.type_at_end(app, STALE_EDIT_TEXT)
        self.require(app, REVIEW_AGAIN_AUTOMATION_ID, "REVIEW_STALE_MISSING")
        if not self.name_present(app, REVIEW_FIRST_QUESTION, "Text"):
            raise HarnessFailure("REVIEW_STALE_RESULT_GONE")
        return [*markers, REVIEW_AGAIN_AUTOMATION_ID]


def write_settings(path: Path, mode: str, language: str, server: ReviewServer | None) -> None:
    lines = [
        f'theme = "{mode}"',
        f'language = "{language}"',
    ]
    if server is not None:
        lines.extend(
            [
                f'model-provider = "{REVIEW_PROVIDER}"',
                f'model-name = "{REVIEW_MODEL}"',
                f'model-base-url = "{server.base_url}"',
                f'model-environment-key-identity = "{credential_identity(server.port)}"',
            ]
        )
    path.write_text("\n".join(lines) + "\n", encoding="utf-8")


def fixture(workspace: Path) -> Path:
    workspace.mkdir(parents=True, exist_ok=True)
    (workspace / "AGENTS.md").write_text(AGENTS_TEXT, encoding="utf-8", newline="\n")
    plan = workspace / PLAN_NAME
    plan.write_text(PLAN_TEXT, encoding="utf-8", newline="\n")
    (workspace / "other.md").write_text("# Other\n", encoding="utf-8", newline="\n")
    return plan


@contextlib.contextmanager
def child_environment(overrides: dict[str, str]) -> Iterator[None]:
    """`overrides` in this process's environment while a launch copies it
    (`runtime.build_launch_spec`), then the previous values back."""
    previous = {name: os.environ.get(name) for name in overrides}
    os.environ.update(overrides)
    try:
        yield
    finally:
        for name, value in previous.items():
            if value is None:
                os.environ.pop(name, None)
            else:
                os.environ[name] = value


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


def web_content_present(image: Any, rect: tuple[int, int, int, int]) -> bool:
    """Whether the compatibility pane has more than its empty page ground.

    The page is composited into the pane rectangle, so the check only looks
    there, against the page's own ground at the pane's top-left corner. A page
    with text has many pixels away from that ground; an empty pane has almost
    none. Sampling every four pixels and requiring a tenth of a percent keeps
    a short release plan above the floor at the wide viewport, where the text
    occupies little of the pane, while an empty pane stays well below it.
    """
    left = max(0, rect[0])
    top = max(0, rect[1])
    right = min(image.width, rect[2])
    bottom = min(image.height, rect[3])
    if right <= left or bottom <= top:
        return False
    ground = image.getpixel((left, top))
    changed = 0
    total = 0
    for y in range(top, bottom, 4):
        for x in range(left, right, 4):
            total += 1
            if color_distance(image.getpixel((x, y)), ground) > 24:
                changed += 1
    return changed >= max(64, total // 1000)


def stop_profile_browsers(profile: Path) -> None:
    """End the WebView2 browser processes serving `profile`'s WebView2
    folder. The browser escapes the launch's job object and outlives the app
    by minutes, holding the profile's files."""
    script = (
        "Get-CimInstance Win32_Process -Filter \"Name='msedgewebview2.exe'\" | "
        "Where-Object { $_.CommandLine -and $_.CommandLine.IndexOf("
        "$env:MARKTURBO_GOAL09_PROFILE, [StringComparison]::OrdinalIgnoreCase) -ge 0 } | "
        "ForEach-Object { Stop-Process -Id $_.ProcessId -Force -ErrorAction SilentlyContinue }"
    )
    try:
        subprocess.run(
            ["powershell", "-NoProfile", "-NonInteractive", "-Command", script],
            env={**os.environ, "MARKTURBO_GOAL09_PROFILE": str(profile)},
            capture_output=True,
            timeout=30,
            check=False,
        )
    except (OSError, subprocess.TimeoutExpired):
        pass


def remove_profile(profile: Path, timeout: float = PROFILE_REMOVAL_SECONDS) -> None:
    """Delete a cell's profile; if a WebView2 browser still holds its files,
    end that browser and try again."""

    def removed() -> bool:
        shutil.rmtree(profile, ignore_errors=True)
        return not profile.exists()

    for attempt in (1, 2):
        try:
            wait_until(removed, timeout, "PROFILE_NOT_REMOVED", interval=0.25)
            return
        except HarnessFailure:
            if attempt == 1:
                stop_profile_browsers(profile)


def capture_cell(
    win32: Win32,
    exe: Path,
    out: Path,
    cell: Cell,
    source: SourceBinding,
    executable: dict[str, Any],
    ui_types: tuple[Any, ...],
    parent_context: Any,
) -> dict[str, Any]:
    started = utc_now()
    case_root = out / "cells" / cell.cell_id
    case_root.mkdir(parents=True, exist_ok=True)
    for stale in ("capture.png", "failure.png"):
        (case_root / stale).unlink(missing_ok=True)
    profile = Path(tempfile.mkdtemp(prefix="markturbo-goal09-")).resolve()
    config, data, workspace, codex_home, local_app_data = (
        profile / name
        for name in ("config", "data", "workspace", "codex-home", "local-app-data")
    )
    for directory in (config, data, workspace, codex_home, local_app_data):
        directory.mkdir(parents=True, exist_ok=True)
    plan = fixture(workspace)
    server = ReviewServer(hold=cell.state == "review-loading") if cell.state in REVIEW_STATES else None
    if server is not None:
        server.start()
    write_settings(config / "settings.toml", cell.mode, cell.language, server)
    # nsh keeps its WebView2 profile under `%LOCALAPPDATA%\<app id>`. One per
    # cell, so a cell never joins the previous cell's still-exiting browser
    # (which ignores this cell's debugging port) nor the user's own profile.
    overrides = {"CODEX_HOME": str(codex_home), "LOCALAPPDATA": str(local_app_data)}
    web_port = free_port() if cell.state == "web-compatibility-preview" else None
    if web_port is not None:
        overrides["WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS"] = (
            f"--remote-debugging-port={web_port} --remote-allow-origins=*"
        )
    harness = VisualHarness(
        exe,
        profile,
        UI_TIMEOUT,
        win32,
        *ui_types,
        parent_context,
        credential=f"markturbo-goal09-{uuid.uuid4().hex}" if server is not None else None,
    )

    app: RunningApp | None = None
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
        with child_environment(overrides):
            app = harness.launch_app(
                None if cell.state in EMPTY_STATES else plan,
                data,
                config,
                workspace,
                profile / "stderr.log",
            )
        window = AppWindow(win32, app.window)
        focused = window.foreground()
        dpi = int(win32.user32.GetDpiForWindow(window.hwnd))
        width, height = physical_viewport(cell.viewport, dpi)
        window.resize_client(width, height, dpi_awareness()[2])
        markers = harness.prepare(app, cell, plan, server)
        # Kept on a later failure too: the state was reached, and the
        # `failure.png` shows it.
        receipt["state_markers"] = markers
        receipt["consent_hold_count"] = None if server is None else harness.consent_holds
        window.park_pointer()
        time.sleep(SETTLE_SECONDS)
        # The app can grow its client area by the caption height after the
        # first resize (seen as 1162 for 1125 px); place it again.
        window.resize_client(width, height, dpi_awareness()[2])
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
        if web_port is not None:
            page = cdp_page_png(web_port)
            rect = window.webview_rect()
            image = composite_web(image, page, rect)
            if not web_content_present(image, rect):
                raise VisualFailure("WEBVIEW_CONTENT_MISSING")
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
                "review_request_count": None if server is None else server.request_count,
            }
        )
    except (VisualFailure, HarnessFailure, HarnessBlocked) as error:
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
    except Exception as error:  # noqa: BLE001 - a cell must not abort the matrix
        receipt["reason_code"] = "CAPTURE_FAILED"
        receipt["failure_type"] = safe_exception_name(error)
        if DEBUG:
            print(f"  {type(error).__name__}: {error}", file=sys.stderr, flush=True)
    finally:
        try:
            if app is not None:
                harness.reap(app)
            harness.cleanup()
        except Exception:  # noqa: BLE001 - the job object still ends the app
            pass
        if server is not None:
            server.close()
        remove_profile(profile)
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
        parent_context = win32.security_context(os.getpid())
        ui_types = load_pywinauto()
    except HarnessBlocked as error:
        print(f"error: {error.code}", file=sys.stderr)
        return 2
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
    win32.keep_display_awake()
    try:
        for index, cell in enumerate(cells, 1):
            print(f"[{index}/{len(cells)}] {cell.cell_id}", flush=True)
            receipt = capture_cell(
                win32,
                arguments.exe.resolve(),
                out,
                cell,
                source,
                executable,
                ui_types,
                parent_context,
            )
            if receipt["status"] != "CAPTURED":
                print(f"  {receipt.get('reason_code')}", flush=True)
            summary["cells"].append(
                {
                    "cell_id": cell.cell_id,
                    "status": receipt["status"],
                    "reason_code": receipt.get("reason_code"),
                }
            )
    finally:
        win32.allow_display_sleep()
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
