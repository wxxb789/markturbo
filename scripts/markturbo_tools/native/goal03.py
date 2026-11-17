"""Exercise Goal 03's first-use document workflow through a real Windows UI.

The harness is fail-closed. A PASS binds every observation to the supplied
x64 executable SHA-256 and uses isolated MARKTURBO_DATA_DIR and
MARKTURBO_CONFIG_DIR roots. Evidence contains fingerprints and booleans, never
document text or filesystem paths.

It drives the nsh welcome (`ui/welcome.rs`): Open folder
(`welcome-open-folder`), New file (`welcome-new-file`), Paste (`welcome-paste`)
and Open sample (`welcome-open-sample`) are Buttons with their own
AutomationIds. The recent rows are Buttons sharing the `welcome-recent` id,
named `<kind> <name>`; a target that no longer exists stays listed, disabled,
named `<kind> <name> — Missing`, and a press opens nothing. An open workspace
(Open sample, a directory argument, a folder) replaces the welcome with the
workspace home, whose file rows are Buttons sharing `workspace-home-file`.
Save As runs from the title menu or Ctrl+Shift+S through the native
IFileSaveDialog, whose own overwrite confirmation is the only overwrite
decision: Yes replaces the existing file. Editor text is read with the
editor's Select All + Copy (see `runtime.EDITOR_NAME`).
"""

from __future__ import annotations

import argparse
import hashlib
import json
import re
import tomllib
from pathlib import Path
from typing import Any

from . import evidence
from .evidence import leak_codes
from .runtime import (
    INTEGRITY_NAMES,
    MENU_SAVE_AS_AUTOMATION_ID,
    VK_A,
    VK_CONTROL,
    VK_RETURN,
    VK_S,
    VK_W,
    Fingerprint,
    HarnessBlocked,
    HarnessFailure,
    NativeHarness as BaseNativeHarness,
    NativeRunPlan,
    fingerprint_text,
    key_input,
    load_pywinauto,
    main_native_acceptance,
    preflight,
    safe_exception_name,
    sha256_file,
    validate_fingerprint,
    wait_until,
    write_durable,
)


SCHEMA = "markturbo.goal-03-native-acceptance"
VK_SHIFT = 0x10
VK_ESCAPE = 0x1B

WELCOME_OPEN_FOLDER_AUTOMATION_ID = "welcome-open-folder"
WELCOME_NEW_AUTOMATION_ID = "welcome-new-file"
WELCOME_PASTE_AUTOMATION_ID = "welcome-paste"
WELCOME_SAMPLE_AUTOMATION_ID = "welcome-open-sample"
WELCOME_RECENT_AUTOMATION_ID = "welcome-recent"
# A file recent row is named `File <name>`; once its target is gone it is
# disabled and named `File <name> — Missing` (`ui/welcome.rs` `recents`).
STALE_RECENT_NAME = "File {name} \u2014 Missing"
WORKSPACE_HOME_FILE_AUTOMATION_ID = "workspace-home-file"
CASE_WELCOME = "welcome_no_argument"
CASE_NEW_PASTE = "new_and_paste_unicode"
CASE_SAVE_CREATE = "save_as_create_and_reopen"
CASE_SAVE_CANCEL_OVERWRITE = "save_as_cancel_and_overwrite"
CASE_SAMPLE = "bundled_sample"
CASE_RECENTS = "recent_bound_restart_stale"
CASE_CLI = "explicit_cli_targets"
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
REQUIRED_CASE_IDS = tuple(CASE_FLOWS)

DOCUMENT_SENTINEL = "MTG03-NATIVE-SENTINEL-\u4fdd\u5b58-\U0001f680"
NEW_TEXT = f"# {DOCUMENT_SENTINEL}: New\n\n\u4e2d\u6587 and emoji \U0001f680\n"
PASTE_TEXT = f"# {DOCUMENT_SENTINEL}: Paste\n\n\u65e5\u672c\u8a9e and emoji \U0001f9ea\n"
SAVE_TEXT = f"# {DOCUMENT_SENTINEL}: Save As\n\nExact CJK \u4f60\u597d and emoji \U0001f680\n"
OVERWRITE_TEXT = f"# {DOCUMENT_SENTINEL}: Replace\n\nExact CJK \u4fdd\u5b58 and emoji \U0001f9ea\n"
EXISTING_DESTINATION = b"goal-03 original destination\n"
CANCELLED_DESTINATION = b"goal-03 cancelled destination\n"

SAFE_STRINGS = frozenset(CASE_FLOWS.values()) | INTEGRITY_NAMES
SAMPLE_VERSION_RE = re.compile(r"^[0-9a-f]{24}$")
FINGERPRINT_OBSERVATION_KEYS = frozenset(
    {
        "editor_after_overwrite_cancel",
        "editor_after_save_as_cancel",
        "editor_before_cancellation",
        "new_unicode_editor",
        "paste_unicode_editor",
        "reopened_editor",
        "sample_content",
        "sample_manifest",
        "save_as_cancel_destination_after",
        "save_as_cancel_destination_before",
        "saved_destination",
        "source_after_cancel",
        "source_before",
    }
)
REQUIRED_OBSERVATIONS = {
    CASE_WELCOME: frozenset(),
    CASE_NEW_PASTE: frozenset({"new_unicode_editor", "paste_unicode_editor"}),
    CASE_SAVE_CREATE: frozenset({"saved_destination", "reopened_editor"}),
    CASE_SAVE_CANCEL_OVERWRITE: frozenset(
        {
            "editor_before_cancellation",
            "editor_after_save_as_cancel",
            "editor_after_overwrite_cancel",
            "source_before",
            "source_after_cancel",
            "save_as_cancel_destination_before",
            "save_as_cancel_destination_after",
            "saved_destination",
        }
    ),
    CASE_SAMPLE: frozenset(
        {"sample_file_count", "sample_manifest", "sample_content", "sample_version"}
    ),
    CASE_RECENTS: frozenset({"recent_count", "recent_visible_count"}),
    CASE_CLI: frozenset(),
}


# A press, not a hold: long enough for the pointer down and up to register.
STALE_RECENT_PRESS_SECONDS = 0.05


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


def validate_runtime_scan(value: Any) -> None:
    evidence.validate_runtime_scan(value, require_config=False)


def validate_passed_case(case_id: str, observations: dict[str, Any]) -> None:
    if case_id == CASE_NEW_PASTE:
        validate_fingerprint(observations.get("new_unicode_editor"))
        validate_fingerprint(observations.get("paste_unicode_editor"))
    elif case_id == CASE_SAVE_CREATE:
        saved = validate_fingerprint(observations.get("saved_destination"))
        reopened = validate_fingerprint(observations.get("reopened_editor"))
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
        if observations.get("recent_count") != 10:
            raise ValueError("recent target bound was not exactly ten")
        visible = observations.get("recent_visible_count")
        if not isinstance(visible, int) or isinstance(visible, bool) or not 1 <= visible <= 10:
            raise ValueError("recent rows must show between one and ten targets")


CONTRACT = evidence.EvidenceContract(
    schema=SCHEMA,
    case_flows=CASE_FLOWS,
    observation_keys=frozenset().union(*REQUIRED_OBSERVATIONS.values()),
    fingerprint_keys=FINGERPRINT_OBSERVATION_KEYS,
    required_observations=REQUIRED_OBSERVATIONS,
    validate_runtime_scan=validate_runtime_scan,
    validate_passed_case=validate_passed_case,
    private_sentinels=(DOCUMENT_SENTINEL,),
    safe_strings=SAFE_STRINGS,
    string_patterns={"sample_version": SAMPLE_VERSION_RE},
)


def new_evidence(expected_hash: str) -> dict[str, Any]:
    return evidence.new_evidence(CONTRACT, expected_hash)


def validate_evidence(value: dict[str, Any]) -> None:
    evidence.validate_evidence(value, CONTRACT)


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
    return evidence.scan_case_artifacts(case_root, leak_codes("DOCUMENT_SENTINEL", DOCUMENT_SENTINEL))


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

    def welcome_action(self, app: Any, automation_id: str) -> Any:
        return self.find_control(
            app, automation_id, "Button", "WELCOME_UIA_TIMEOUT", "WELCOME_UIA_CONTRACT_MISMATCH"
        )

    def welcome_entry_points(self, app: Any) -> None:
        self.welcome_open_folder(app)
        for automation_id in (
            WELCOME_NEW_AUTOMATION_ID,
            WELCOME_PASTE_AUTOMATION_ID,
            WELCOME_SAMPLE_AUTOMATION_ID,
        ):
            self.welcome_action(app, automation_id)

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
        return self.buttons_sharing_id(app, WELCOME_RECENT_AUTOMATION_ID, "RECENT")

    def workspace_home_files(self, app: Any) -> list[Any]:
        return self.buttons_sharing_id(app, WORKSPACE_HOME_FILE_AUTOMATION_ID, "WORKSPACE_HOME")

    def wait_workspace_home(self, app: Any) -> None:
        """The welcome gave way to the workspace home, which lists files."""
        self.wait_welcome_absent(app)
        wait_until(
            lambda: self.workspace_home_files(app),
            self.ui_timeout,
            "WORKSPACE_HOME_UIA_TIMEOUT",
        )

    def buttons_sharing_id(self, app: Any, automation_id: str, code: str) -> list[Any]:
        """Every visible Button whose AutomationId is `automation_id`."""
        try:
            uia = self.iuia_class()
            condition = uia.iuia.CreatePropertyCondition(
                uia.UIA_dll.UIA_AutomationIdPropertyId, automation_id
            )
            elements = self.fresh_uia_root(app.hwnd).element.FindAll(
                uia.tree_scope["descendants"], condition
            )
            controls = []
            expected_control_type = uia.known_control_types["Button"]
            for index in range(elements.Length):
                element = elements.GetElement(index)
                if (
                    element.CurrentAutomationId != automation_id
                    or element.CurrentControlType != expected_control_type
                ):
                    raise HarnessFailure(f"{code}_UIA_CONTRACT_MISMATCH")
                control = self.uia_wrapper_class(self.uia_element_info_class(element))
                info = control.element_info
                if (
                    info.automation_id != automation_id
                    or info.control_type != "Button"
                    or not control.is_visible()
                ):
                    raise HarnessFailure(f"{code}_UIA_CONTRACT_MISMATCH")
                controls.append(control)
            return controls
        except HarnessFailure:
            raise
        except Exception as error:
            raise HarnessFailure(f"{code}_UIA_QUERY_FAILED", safe_exception_name(error)) from None

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
            self.click_control(self.welcome_action(new, WELCOME_NEW_AUTOMATION_ID), "WELCOME_NEW_CLICK_FAILED")
            self.wait_welcome_absent(new)
            self.source_editor_ready(new)
            new_text = self.paste_editor(new, NEW_TEXT)
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
                        self.welcome_action(paste, WELCOME_PASTE_AUTOMATION_ID), "WELCOME_PASTE_CLICK_FAILED"
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
            self.click_control(self.welcome_action(first, WELCOME_NEW_AUTOMATION_ID), "WELCOME_NEW_CLICK_FAILED")
            self.source_editor_ready(first)
            editor = self.paste_editor(first, SAVE_TEXT)
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
            self.click_control(self.welcome_action(app, WELCOME_NEW_AUTOMATION_ID), "WELCOME_NEW_CLICK_FAILED")
            self.source_editor_ready(app)
            editor = self.paste_editor(app, OVERWRITE_TEXT)
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
            sample = self.welcome_action(app, WELCOME_SAMPLE_AUTOMATION_ID)
            self.click_control(sample, "WELCOME_SAMPLE_CLICK_FAILED")
            self.wait_workspace_home(app)
            materialized = materialized_sample_inventory(data)
            self.close_app(app)
            observations = {
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
            if stale.is_enabled() or stale.element_info.name != STALE_RECENT_NAME.format(
                name=stale_name
            ):
                raise HarnessFailure("STALE_RECENT_NOT_DEGRADED")
            # A real pointer press on the disabled row opens nothing.
            self.hold_control(
                restarted, stale, STALE_RECENT_PRESS_SECONDS, "STALE_RECENT_PRESS_FAILED"
            )
            self.welcome_open_folder(restarted)
            if not self.editor_absent_while_running(restarted):
                raise HarnessFailure("STALE_RECENT_OPENED_DOCUMENT")
            self.close_app(restarted)
            observations = {
                "recent_count": recent_count,
                "recent_visible_count": len(controls),
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
                self.wait_workspace_home(directory_app)
                self.close_app(directory_app)
            finally:
                self.reap(directory_app)
            observations = {
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
    parser = evidence.build_parser(__doc__.splitlines()[0], "goal-03", REQUIRED_CASE_IDS)
    return parser.parse_args(argv)


def main(argv: list[str] | None = None) -> int:
    args = parse_args(argv)
    return main_native_acceptance(args, native_run_plan())


if __name__ == "__main__":
    raise SystemExit(main())
