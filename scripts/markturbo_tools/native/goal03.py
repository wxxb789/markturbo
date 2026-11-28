"""Exercise Goal 03's first-use document workflow through a real Windows UI.

The harness is fail-closed. A PASS binds every observation to the supplied
x64 executable SHA-256 and uses isolated MARKTURBO_DATA_DIR and
MARKTURBO_CONFIG_DIR roots. Evidence contains fingerprints and counts, never
document text, file names or filesystem paths.

One case, one profile, three launches; only the behaviour that needs a
restart gets one:

1. A no-argument launch on the clean profile (nothing is written before it):
   the welcome (its four actions, New file among them, are enabled Buttons),
   Paste from the real OS clipboard, Save As onto an existing file answered
   No (then the picker's Escape: the one cancel proof) and Yes, a typed edit
   saved to the new path, the clean close into the recent list and Open
   sample, all in one session.
2. A cold launch with the saved file as its argument (the reopen and the
   command-line bypass need a new process). Before it the harness replaces
   the recent history with eleven targets, one of them missing.
3. A no-argument launch reading the history the second launch wrote: the
   bounded rows, the stale row and a reopen from the list.

Headless tests carry what needs no real OS: New file saving through Save As
(`untitled_save.rs`), Save after Save As writing only the new path
(`save_as_overwrite.rs`) and discarding an untitled draft without a recent
entry (`untitled_discard.rs`); Goal 02's KEYBOARD_CLOSE proves Ctrl+W, the
guard and Don't save natively.

Every phase prefixes its failure codes with its own name (`phase`), so a
FAIL names the phase that broke.

It drives the nsh welcome (`ui/welcome.rs`): Open folder
(`welcome-open-folder`), New file (`welcome-new-file`), Paste (`welcome-paste`)
and Open sample (`welcome-open-sample`) are Buttons with their own
AutomationIds (nsh-platform-windows maps `test_id` to the UIA AutomationId and
the accessible label to the Name). The recent rows are Buttons sharing the
`welcome-recent` id, named `<kind> <name>`; at most `WELCOME_RECENT_ROWS` are
listed. A target that no longer exists stays listed, disabled, named
`<kind> <name> — Missing`, and a press opens nothing. An open workspace
replaces the welcome with the workspace home, whose file rows are Buttons
sharing `workspace-home-file`. Save As runs from Ctrl+Shift+S through the
native IFileSaveDialog, whose own overwrite confirmation is the only overwrite
decision: Yes replaces the existing file. nsh 0.7.1 owns that dialog by the
hidden window its surface draws into, so the harness finds it among the
process's task dialogs, not by main-window ownership. The app writes the file
before it swaps the tab, so the harness waits for the title row's file name
(`title-file`, a Button labeled `Show agent context for <name>` since Goal 20)
before it edits the swapped tab. Each open tab has one mounted close control
(`rail-close`) on its row in the expanded sidebar panel (`ensure_sidebar_open`
before counting).
Editor text is read with the editor's Select All + Copy (see
`runtime.EDITOR_NAME`).
"""

from __future__ import annotations

import argparse
import hashlib
import re
import tomllib
from pathlib import Path
from typing import Any

from . import evidence
from .evidence import leak_codes
from .runtime import (
    INTEGRITY_NAMES,
    TAB_CLOSE_AUTOMATION_ID,
    VK_A,
    VK_CONTROL,
    VK_ESCAPE,
    VK_RETURN,
    VK_S,
    VK_W,
    Fingerprint,
    HarnessBlocked,
    HarnessFailure,
    NativeHarness as BaseNativeHarness,
    NativeRunPlan,
    RunningApp,
    fingerprint_text,
    key_input,
    load_pywinauto,
    main_native_acceptance,
    phase,
    preflight,
    safe_exception_name,
    sha256_file,
    validate_fingerprint,
    wait_until,
    write_durable,
)
from .selectors import (
    TITLE_FILE_AUTOMATION_ID,
    WELCOME_ACTION_AUTOMATION_IDS,
    WELCOME_NEW_FILE_AUTOMATION_ID as WELCOME_NEW_AUTOMATION_ID,
    WELCOME_OPEN_FOLDER_AUTOMATION_ID,
    WELCOME_PASTE_AUTOMATION_ID,
    WELCOME_RECENT_AUTOMATION_ID,
    WELCOME_SAMPLE_AUTOMATION_ID,
    WORKSPACE_HOME_FILE_AUTOMATION_ID,
)


SCHEMA = "markturbo.goal-03-native-acceptance"
VK_SHIFT = 0x10

# `ui/titlebar.rs` `file_name`: the Button's accessible label names its
# Context action, not the bare file name.
TITLE_FILE_NAME_LABEL = "Show agent context for {name}"
# A file recent row is named `File <name>`; once its target is gone it is
# disabled and named `File <name> — Missing` (`ui/welcome.rs` `recents`).
RECENT_ROW_NAME = "File {name}"
STALE_RECENT_NAME = "File {name} — Missing"
# The welcome lists the first five recent targets (`ui/welcome.rs` `RECENTS`);
# the persisted history holds ten (mt-core `settings::MAX_RECENT_TARGETS`).
WELCOME_RECENT_ROWS = 5
RECENT_TARGET_BOUND = 10
LAUNCH_COUNT = 3

CASE_FIRST_USE = "first_use_lifecycle"
CASE_FLOWS = {
    CASE_FIRST_USE: (
        "clean no-argument welcome -> Paste -> Save As overwrite No and picker cancel,"
        " overwrite Yes -> typed Save -> clean close -> sample"
        " | argv cold reopen | restart recents bound, stale row, reopen from list"
    ),
}
REQUIRED_CASE_IDS = tuple(CASE_FLOWS)

DOCUMENT_SENTINEL = "MTG03-NATIVE-SENTINEL-保存-\U0001f680"
# Typed into the saved tab with SendInput (as Goal 02 types): one line with
# CJK and emoji, so the cold reopens compare exact non-ASCII bytes.
RESAVE_TEXT = f"{DOCUMENT_SENTINEL}: Save 你好 \U0001f9ea"
PASTE_TEXT = f"# {DOCUMENT_SENTINEL}: Paste\n\n日本語 and emoji \U0001f9ea\n"
EXISTING_DESTINATION = b"goal-03 original destination\n"
# The seeded history: eleven file targets, newest first; this one is never
# written, so it is stale from the start.
SEEDED_RECENT_COUNT = 11
STALE_SEED_INDEX = 1

SAFE_STRINGS = frozenset(CASE_FLOWS.values()) | INTEGRITY_NAMES
SAMPLE_VERSION_RE = re.compile(r"^[0-9a-f]{24}$")
FINGERPRINT_OBSERVATION_KEYS = frozenset(
    {
        "paste_clipboard",
        "paste_editor",
        "overwrite_no_destination_before",
        "overwrite_no_destination_after",
        "overwrite_yes_destination",
        "subsequent_save_destination",
        "sample_manifest",
        "sample_content",
        "reopen_editor",
    }
)
COUNT_OBSERVATION_KEYS = frozenset(
    {
        "welcome_recent_rows",
        "saved_close_recent_rows",
        "sample_file_count",
        "sample_home_rows",
        "reopen_recent_count",
        "recents_rows",
        "recents_disabled_rows",
        "recents_open_tabs",
        "recents_count_after_restart",
    }
)
REQUIRED_OBSERVATIONS = {
    CASE_FIRST_USE: FINGERPRINT_OBSERVATION_KEYS
    | COUNT_OBSERVATION_KEYS
    | frozenset({"sample_version", "process_contexts"}),
}


# A press, not a hold: long enough for the pointer down and up to register.
STALE_RECENT_PRESS_SECONDS = 0.05
# The stale-row fence. A press opens its file asynchronously, so no look right
# after the stale press can prove it opened nothing. The saved row is clicked
# after it; once that tab is shown the earlier press has been handled, and
# this settle lets a late tab from either press reach the sidebar before the
# harness counts its close buttons (the one fixed wait the phase keeps).
OPEN_TABS_SETTLE_SECONDS = 0.3


def require(condition: bool, code: str) -> None:
    """Fail the current phase with `code` unless `condition` holds."""
    if not condition:
        raise HarnessFailure(code)


def recent_settings_document(documents: list[Path]) -> bytes:
    return evidence.settings_document(
        {"show-welcome-on-startup": True},
        {
            "recent-targets": [
                {"path": str(path), "kind": "file", "display-name": path.name}
                for path in documents
            ]
        },
    )


def validate_runtime_scan(value: Any) -> None:
    evidence.validate_runtime_scan(value, require_config=False)


def validate_passed_case(case_id: str, observations: dict[str, Any]) -> None:
    if case_id != CASE_FIRST_USE:
        return
    fp = {key: validate_fingerprint(observations.get(key)) for key in FINGERPRINT_OBSERVATION_KEYS}
    count = {
        key: evidence.nonnegative_count(observations, key) for key in COUNT_OBSERVATION_KEYS
    }
    contexts = observations.get("process_contexts")
    if not isinstance(contexts, list) or len(contexts) != LAUNCH_COUNT:
        raise ValueError("the case must record one process context per launch")
    if count["welcome_recent_rows"] != 0:
        raise ValueError("the first launch did not start from a clean profile")
    if fp["paste_clipboard"] != fingerprint_text(PASTE_TEXT).evidence():
        raise ValueError("the clipboard did not hold the CJK and emoji paste text")
    if fp["paste_editor"] != fp["paste_clipboard"]:
        raise ValueError("Paste did not create the exact clipboard text")
    if fp["overwrite_no_destination_before"] != fp["overwrite_no_destination_after"]:
        raise ValueError("overwrite cancellation changed the destination")
    # The cancelled buffer is the one the confirmed overwrite then wrote.
    if (
        fp["overwrite_yes_destination"] != fp["paste_editor"]
        or fp["overwrite_yes_destination"] == fp["overwrite_no_destination_before"]
    ):
        raise ValueError("confirmed overwrite did not replace the destination")
    if fp["subsequent_save_destination"] != fingerprint_text(RESAVE_TEXT).evidence():
        raise ValueError("Save after Save As did not write the typed text to the new path")
    if count["saved_close_recent_rows"] != 1:
        raise ValueError("the Save As destination was not the one recent target")
    if count["sample_file_count"] <= 0 or count["sample_home_rows"] <= 0:
        raise ValueError("sample file inventory must be nonempty")
    if fp["sample_manifest"]["byte_count"] <= 0 or fp["sample_content"]["byte_count"] <= 0:
        raise ValueError("sample fingerprints must be nonempty")
    version = observations.get("sample_version")
    if not isinstance(version, str) or not SAMPLE_VERSION_RE.fullmatch(version):
        raise ValueError("sample version must be a 24-character SHA-256 prefix")
    if fp["sample_content"]["sha256"][:24] != version:
        raise ValueError("sample version does not match the materialized content")
    saved = fp["subsequent_save_destination"]
    if fp["reopen_editor"] != saved:
        raise ValueError("direct reopen did not preserve exact Save As text")
    if count["recents_open_tabs"] != 1:
        raise ValueError("the recent rows must open exactly the saved file")
    if (
        count["reopen_recent_count"] != RECENT_TARGET_BOUND
        or count["recents_count_after_restart"] != RECENT_TARGET_BOUND
    ):
        raise ValueError("recent target bound was not exactly ten")
    if count["recents_rows"] != WELCOME_RECENT_ROWS:
        raise ValueError("the welcome must list exactly five recent rows")
    if count["recents_disabled_rows"] != 1:
        raise ValueError("exactly the stale recent row must be disabled")


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


def seed_recent_history(config_root: Path, workspace_root: Path) -> list[Path]:
    """Replace the profile's recent history with eleven file targets.

    The one at `STALE_SEED_INDEX` is never written, so it is missing.
    """
    history = workspace_root / "history"
    documents = []
    for index in range(SEEDED_RECENT_COUNT):
        path = (history / f"recent-{index:02}.md").resolve()
        if index != STALE_SEED_INDEX:
            write_durable(path, f"recent {index}\n".encode("utf-8"))
        documents.append(path)
    write_durable(config_root / "settings.toml", recent_settings_document(documents))
    return documents


def sample_inventory(root: Path) -> tuple[int, Fingerprint, Fingerprint]:
    """Fingerprint a sample tree without retaining paths or file contents."""
    # The app names its sample version after the files sorted by their UTF-8
    # path bytes; Windows path order ignores case, so sort the bytes here too.
    files = sorted(
        (path for path in root.rglob("*") if path.is_file()),
        key=lambda path: path.relative_to(root).as_posix().encode("utf-8"),
    )
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

    def welcome_buttons(self, app: Any) -> dict[str, list[Any]]:
        """The visible welcome action and recent-row Buttons by AutomationId,
        from one UIA query over every Button."""
        try:
            uia = self.iuia_class()
            condition = uia.iuia.CreatePropertyCondition(
                uia.UIA_dll.UIA_ControlTypePropertyId, uia.known_control_types["Button"]
            )
            elements = self.fresh_uia_root(app.hwnd).element.FindAll(
                uia.tree_scope["descendants"], condition
            )
            buttons: dict[str, list[Any]] = {
                automation_id: []
                for automation_id in (*WELCOME_ACTION_AUTOMATION_IDS, WELCOME_RECENT_AUTOMATION_ID)
            }
            for index in range(elements.Length):
                element = elements.GetElement(index)
                found = buttons.get(element.CurrentAutomationId)
                if found is None:
                    continue
                control = self.uia_wrapper_class(self.uia_element_info_class(element))
                if control.is_visible():
                    found.append(control)
            return buttons
        except HarnessFailure:
            raise
        except Exception as error:
            raise HarnessFailure("WELCOME_UIA_QUERY_FAILED", safe_exception_name(error)) from None

    def welcome_entry_points(self, app: Any) -> int:
        """Wait until Open folder, New file, Paste and Open sample are each one
        enabled Button; return how many recent rows the same query saw."""

        def ready() -> tuple[int] | None:
            buttons = self.welcome_buttons(app)
            actions = [buttons[automation_id] for automation_id in WELCOME_ACTION_AUTOMATION_IDS]
            if any(len(found) > 1 for found in actions):
                raise HarnessFailure("WELCOME_UIA_CONTRACT_MISMATCH")
            if all(len(found) == 1 and found[0].is_enabled() for found in actions):
                return (len(buttons[WELCOME_RECENT_AUTOMATION_ID]),)
            return None

        (recent_rows,) = wait_until(ready, self.ui_timeout, "WELCOME_UIA_TIMEOUT", interval=0.025)
        return recent_rows

    def welcome_absent(self, app: Any) -> bool:
        return self.control_absent(
            app,
            WELCOME_OPEN_FOLDER_AUTOMATION_ID,
            "Button",
            "WELCOME_UIA_QUERY_FAILED",
            "WELCOME_UIA_CONTRACT_MISMATCH",
        )

    def wait_welcome_absent(self, app: Any) -> None:
        wait_until(lambda: self.welcome_absent(app), self.ui_timeout, "WELCOME_DID_NOT_CLOSE")

    def source_editor_ready(self, app: Any) -> None:
        self.activate_source_layout(app)

    def close_tab_by_keyboard(self, app: Any) -> None:
        """Ctrl+W from the focused editor: the active tab's close."""
        self.focus_editor(app)
        self.win32.send_shortcut(app.hwnd, VK_W)

    def common_file_dialog(self, app: Any) -> tuple[int, Any]:
        """Wait for the Save As picker: the task dialog whose filename combo
        holds exactly one ready Edit.

        nsh 0.7.1 owns the picker by the hidden window its surface draws
        into, not the visible main window, so it is found among the
        process's own task dialogs; the picker's nested overwrite
        confirmation holds no `FileNameControlHost` and is skipped.
        """
        last_error: str | None = None

        def locate() -> tuple[int, Any] | None:
            nonlocal last_error
            try:
                found: list[tuple[int, Any]] = []
                for hwnd in self.win32.task_dialogs(app.process.pid):
                    combo = self.control_by_id(
                        hwnd,
                        "FileNameControlHost",
                        "ComboBox",
                        "SAVE_AS_FILE_DIALOG_UIA_CONTRACT_MISMATCH",
                    )
                    if combo is None:
                        continue
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
                        continue
                    found.append((hwnd, edit))
                if len(found) > 1:
                    raise HarnessFailure("MULTIPLE_SAVE_AS_FILE_DIALOGS")
                return found[0] if found else None
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
            nested = self.win32.task_dialogs(app.process.pid, file_dialog_hwnd)
            if len(nested) > 1:
                raise HarnessFailure("MULTIPLE_NATIVE_OVERWRITE_DIALOGS")
            if not nested:
                if file_dialog_hwnd not in self.win32.task_dialogs(app.process.pid):
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
            lambda: hwnd not in self.win32.task_dialogs(app.process.pid),
            self.ui_timeout,
            code,
            interval=0.05,
        )
        self.win32.require_foreground(app.hwnd, self.ui_timeout)

    def select_existing_save_path(self, app: Any, path: Path, *, replace: bool) -> None:
        """Save As onto an existing file: answer the native overwrite prompt."""
        hwnd = self.type_save_path(app, path)
        choice = "Yes" if replace else "No"
        button = self.native_overwrite_confirmation(app, hwnd)[choice]
        self.click_control(button, f"NATIVE_OVERWRITE_{choice.upper()}_CLICK_FAILED")
        if not replace:
            # No returns to the picker; its Escape is the user's cancel, the
            # one picker-cancel proof (the app sees no path either way).
            self.common_file_dialog(app)
            self.win32.send_key(hwnd, VK_ESCAPE)
        self.wait_file_dialog_closed(app, hwnd, "SAVE_AS_FILE_DIALOG_CLOSE_TIMEOUT")

    def wait_title_file(self, app: Any, name: str) -> None:
        """Wait until the title row names `name` as the active tab's file."""
        expected = TITLE_FILE_NAME_LABEL.format(name=name)

        def named() -> bool:
            control = self.control_by_id(
                app.hwnd, TITLE_FILE_AUTOMATION_ID, "Button", "TITLE_FILE_UIA_CONTRACT_MISMATCH"
            )
            return control is not None and control.element_info.name == expected

        wait_until(named, self.ui_timeout, "TITLE_FILE_TIMEOUT", interval=0.025)

    def open_tab_count(self, app: Any) -> int:
        """How many tabs are open: one mounted close button each in the panel."""
        self.ensure_sidebar_open(app)
        closes = self.buttons_sharing_id(
            app, TAB_CLOSE_AUTOMATION_ID, "RAIL_CLOSE", require_visible=False
        )
        return len(closes)

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

    def wait_recent_controls(self, app: Any) -> list[Any]:
        """The welcome is back, and its recent rows."""
        self.welcome_open_folder(app)
        return wait_until(lambda: self.recent_controls(app), self.ui_timeout, "RECENT_ROWS_TIMEOUT")

    def workspace_home_files(self, app: Any) -> list[Any]:
        return self.buttons_sharing_id(app, WORKSPACE_HOME_FILE_AUTOMATION_ID, "WORKSPACE_HOME")

    def wait_workspace_home(self, app: Any) -> list[Any]:
        """The welcome gave way to the workspace home, which lists files."""
        self.wait_welcome_absent(app)
        return wait_until(
            lambda: self.workspace_home_files(app),
            self.ui_timeout,
            "WORKSPACE_HOME_UIA_TIMEOUT",
        )

    def buttons_sharing_id(
        self, app: Any, automation_id: str, code: str, *, require_visible: bool = True
    ) -> list[Any]:
        """The Buttons with this id; require visibility unless counting mounted controls."""
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
                    or (require_visible and not control.is_visible())
                ):
                    raise HarnessFailure(f"{code}_UIA_CONTRACT_MISMATCH")
                controls.append(control)
            return controls
        except HarnessFailure:
            raise
        except Exception as error:
            raise HarnessFailure(f"{code}_UIA_QUERY_FAILED", safe_exception_name(error)) from None

    # Launch 1: the clean-profile, no-argument session.

    def first_session(self, app: RunningApp, data: Path, workspace: Path) -> dict[str, Any]:
        replaced = (workspace / "replace.md").resolve()
        write_durable(replaced, EXISTING_DESTINATION)
        observed: dict[str, Any] = {}

        with phase("WELCOME"):
            # New file stays proven here as an enabled Button; its press and
            # Save As are headless (`untitled_save.rs`).
            observed["welcome_recent_rows"] = self.welcome_entry_points(app)
            require(observed["welcome_recent_rows"] == 0, "PROFILE_NOT_CLEAN")

        with phase("PASTE"):
            # The session restores the user's clipboard when the run ends.
            self.write_text_clipboard(PASTE_TEXT)
            clipboard_text = self.read_text_clipboard()
            if clipboard_text is None:
                raise HarnessBlocked("CLIPBOARD_TEXT_READ_FAILED")
            clipboard = fingerprint_text(clipboard_text)
            if clipboard != fingerprint_text(PASTE_TEXT):
                raise HarnessBlocked("CLIPBOARD_TEXT_MISMATCH")
            self.click_control(
                self.welcome_action(app, WELCOME_PASTE_AUTOMATION_ID),
                "WELCOME_PASTE_CLICK_FAILED",
            )
            self.wait_welcome_absent(app)
            self.source_editor_ready(app)
            pasted = self.editor_fingerprint(app)
            if pasted != clipboard:
                raise HarnessFailure("EXACT_TEXT_MISMATCH")
            observed["paste_clipboard"] = clipboard.evidence()
            observed["paste_editor"] = pasted.evidence()

        with phase("OVERWRITE_NO"):
            # No, then the picker's Escape. The buffer staying exact is proven
            # by the bytes OVERWRITE_YES writes from it.
            replaced_before = sha256_file(replaced)
            observed["overwrite_no_destination_before"] = replaced_before.evidence()
            self.request_save_as_shortcut(app)
            self.select_existing_save_path(app, replaced, replace=False)
            self.require_source_editor_focus(app, "FOCUS_NOT_RESTORED")
            replaced_after = sha256_file(replaced)
            observed["overwrite_no_destination_after"] = replaced_after.evidence()
            require(replaced_after == replaced_before, "CHANGED_STATE")

        with phase("OVERWRITE_YES"):
            self.request_save_as_shortcut(app)
            self.select_existing_save_path(app, replaced, replace=True)
            overwritten = self.wait_file(replaced, pasted, "DESTINATION_TIMEOUT")
            observed["overwrite_yes_destination"] = overwritten.evidence()
            # The file is written before the tab is swapped for it.
            self.wait_title_file(app, replaced.name)

        with phase("SUBSEQUENT_SAVE"):
            # A typed edit of the swapped, file-backed tab; Save writes it to
            # the new path (that it writes only there is `save_as_overwrite.rs`).
            self.source_editor_ready(app)
            resaved = self.replace_editor(app, RESAVE_TEXT)
            self.save_via_menu(app)
            observed["subsequent_save_destination"] = self.wait_file(
                replaced, resaved, "DESTINATION_TIMEOUT"
            ).evidence()

        with phase("SAVED_CLOSE"):
            # The tab is clean: Ctrl+W closes it without a guard, and the
            # welcome lists the path Save As wrote; the untitled paste none.
            self.close_tab_by_keyboard(app)
            rows = self.wait_recent_controls(app)
            if (rows[0].element_info.name or "") != RECENT_ROW_NAME.format(name=replaced.name):
                raise HarnessFailure("RECENT_NOT_NEW_PATH")
            observed["saved_close_recent_rows"] = len(rows)
            require(len(rows) == 1, "RECENT_COUNT")

        with phase("SAMPLE"):
            self.click_control(
                self.welcome_action(app, WELCOME_SAMPLE_AUTOMATION_ID),
                "WELCOME_SAMPLE_CLICK_FAILED",
            )
            observed["sample_home_rows"] = len(self.wait_workspace_home(app))
            observed.update(materialized_sample_inventory(data))
            self.close_app(app)
        return observed

    # Launch 2: the saved file on the command line.

    def cold_reopen(self, app: RunningApp, config: Path, saved: Fingerprint) -> dict[str, Any]:
        with phase("REOPEN"):
            self.source_editor_ready(app)
            if not self.welcome_absent(app):
                raise HarnessFailure("WELCOME_SHOWN")
            # SUBSEQUENT_SAVE left the saved text on the clipboard.
            self.forget_clipboard_text(saved)
            reopened = self.editor_fingerprint(app)
            if reopened != saved:
                raise HarnessFailure("EXACT_TEXT_MISMATCH")
            self.close_app(app)
            # The reopen recorded one more target over the seeded eleven.
            recent_count = persisted_recent_count(config)
            if recent_count != RECENT_TARGET_BOUND:
                raise HarnessFailure("RECENT_BOUND_NOT_TEN")
        return {"reopen_editor": reopened.evidence(), "reopen_recent_count": recent_count}

    # Launch 3: a restart onto the history the second launch wrote.

    def restart_recents(
        self, app: RunningApp, config: Path, stale: Path, saved_path: Path
    ) -> dict[str, Any]:
        with phase("RECENTS"):
            rows = self.wait_recent_controls(app)
            require(len(rows) == WELCOME_RECENT_ROWS, "ROW_COUNT")
            stale_rows = [row for row in rows if stale.name in (row.element_info.name or "")]
            if len(stale_rows) != 1:
                raise HarnessFailure("STALE_UIA_CONTRACT_MISMATCH")
            stale_row = stale_rows[0]
            if stale_row.is_enabled() or stale_row.element_info.name != STALE_RECENT_NAME.format(
                name=stale.name
            ):
                raise HarnessFailure("STALE_NOT_DEGRADED")
            disabled_rows = sum(1 for row in rows if not row.is_enabled())
            require(disabled_rows == 1, "DISABLED_COUNT")
            saved_rows = [
                row
                for row in rows
                if row.element_info.name == RECENT_ROW_NAME.format(name=saved_path.name)
            ]
            if len(saved_rows) != 1:
                raise HarnessFailure("SAVED_ROW_UIA_CONTRACT_MISMATCH")
            # A real pointer press on the disabled row, then a real click on
            # the saved row: the saved tab shown fences the stale press, and
            # one sidebar close afterwards proves the stale press (or a mis-hit
            # neighbour) opened nothing.
            self.hold_control(app, stale_row, STALE_RECENT_PRESS_SECONDS, "STALE_PRESS_FAILED")
            self.click_control(saved_rows[0], "SAVED_ROW_CLICK_FAILED")
            self.source_editor_ready(app)
            self.wait_title_file(app, saved_path.name)
            self.sleep(OPEN_TABS_SETTLE_SECONDS)
            # Nothing runs between this count and the close, so it is also
            # the count just before WM_CLOSE.
            open_tabs = self.open_tab_count(app)
            require(open_tabs == 1, "EXTRA_TAB_OPENED")
            self.close_app(app)
            recent_count = persisted_recent_count(config)
            if recent_count != RECENT_TARGET_BOUND:
                raise HarnessFailure("RECENT_BOUND_NOT_TEN")
        return {
            "recents_rows": len(rows),
            "recents_disabled_rows": disabled_rows,
            "recents_open_tabs": open_tabs,
            "recents_count_after_restart": recent_count,
        }

    def scenario_first_use(self) -> dict[str, Any]:
        data, config, workspace, stderr = self.case_roots(CASE_FIRST_USE)
        case_root = data.parent
        saved_path = (workspace / "replace.md").resolve()
        saved = fingerprint_text(RESAVE_TEXT)
        observations: dict[str, Any] = {
            "flow": CASE_FLOWS[CASE_FIRST_USE],
        }
        contexts = []

        # The clean profile: nothing is written to it before this launch.
        with phase("FIRST_LAUNCH"):
            first = self.launch_app(None, data, config, workspace, stderr)
        try:
            contexts.append(first.security_context.evidence())
            observations.update(self.first_session(first, data, workspace))
        finally:
            self.reap(first)

        with phase("RECENTS_SEED"):
            stale = seed_recent_history(config, workspace)[STALE_SEED_INDEX]

        with phase("REOPEN_LAUNCH"):
            second = self.launch_app(saved_path, data, config, workspace, stderr)
        try:
            contexts.append(second.security_context.evidence())
            observations.update(self.cold_reopen(second, config, saved))
        finally:
            self.reap(second)

        with phase("RECENTS_LAUNCH"):
            third = self.launch_app(None, data, config, workspace, stderr)
        try:
            contexts.append(third.security_context.evidence())
            observations.update(self.restart_recents(third, config, stale, saved_path))
        finally:
            self.reap(third)

        observations["process_contexts"] = contexts
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
        scenarios=lambda harness: (harness.scenario_first_use,),
    )


def parse_args(argv: list[str] | None = None) -> argparse.Namespace:
    parser = evidence.build_parser(__doc__.splitlines()[0], "goal-03", REQUIRED_CASE_IDS)
    return parser.parse_args(argv)


def main(argv: list[str] | None = None) -> int:
    args = parse_args(argv)
    return main_native_acceptance(args, native_run_plan())


if __name__ == "__main__":
    raise SystemExit(main())
