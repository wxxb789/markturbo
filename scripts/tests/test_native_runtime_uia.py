"""The shared native runtime's UIA, focus and input paths, against fakes."""

from __future__ import annotations

import contextlib
import tempfile
import unittest
from pathlib import Path
from unittest import mock

from scripts.markturbo_tools.native import goal02 as HARNESS
from scripts.markturbo_tools.native import runtime

from ._native_support import HASH

SAME_AS_RAW = object()


class FakeControl:
    def __init__(
        self,
        automation_id: str,
        control_type: str,
        *,
        name: str = "",
        class_name: str = "",
        visible: bool = True,
        enabled: bool = True,
        property_error: BaseException | None = None,
        wrapper_automation_id: object = SAME_AS_RAW,
        wrapper_control_type: object = SAME_AS_RAW,
        value: object = "",
        value_results: list[object] | None = None,
        value_pattern_missing: bool = False,
        click_error: BaseException | None = None,
        bounds: tuple[int, int, int, int] = (0, 0, 10, 10),
    ) -> None:
        self.automation_id = automation_id
        self.control_type = control_type
        self.name = name
        self.class_name = class_name
        self.visible = visible
        self.enabled = enabled
        self.property_error = property_error
        self.wrapper_automation_id = wrapper_automation_id
        self.wrapper_control_type = wrapper_control_type
        self.value = value
        self.value_results = value_results
        self.value_pattern_missing = value_pattern_missing
        self.click_error = click_error
        self.bounds = bounds
        self.click_count = 0
        self.invoke_count = 0
        self.focus_count = 0
        self.value_pattern_count = 0

    def _property(self, value: str) -> str:
        if self.property_error is not None:
            raise self.property_error
        return value

    @property
    def CurrentAutomationId(self) -> str:
        return self._property(self.automation_id)

    @property
    def CurrentControlType(self) -> str:
        return self._property(self.control_type)

    @property
    def CurrentName(self) -> str:
        return self._property(self.name)

    @property
    def CurrentClassName(self) -> str:
        return self._property(self.class_name)


class FakeElementArray:
    def __init__(self, elements: list[FakeControl], get_error: BaseException | None = None) -> None:
        self.elements = elements
        self.get_error = get_error
        self.get_calls: list[int] = []

    @property
    def Length(self) -> int:
        return len(self.elements)

    def GetElement(self, index: int) -> FakeControl:
        self.get_calls.append(index)
        if self.get_error is not None:
            raise self.get_error
        return self.elements[index]


class FakeRoot:
    def __init__(
        self,
        controls: list[FakeControl] | None = None,
        find_error: BaseException | None = None,
        get_error: BaseException | None = None,
    ) -> None:
        self.controls = controls or []
        self.find_error = find_error
        self.get_error = get_error
        self.find_calls: list[tuple[str, tuple[str, str]]] = []
        self.arrays: list[FakeElementArray] = []

    def FindAll(self, scope: str, condition: tuple[str, str]) -> FakeElementArray:
        self.find_calls.append((scope, condition))
        if self.find_error is not None:
            raise self.find_error
        attribute = {"automation-id": "automation_id", "name": "name"}[condition[0]]
        elements = [
            control for control in self.controls if getattr(control, attribute) == condition[1]
        ]
        array = FakeElementArray(elements, self.get_error)
        self.arrays.append(array)
        return array

    def descendants(self, **_query: str) -> list[FakeControl]:
        raise AssertionError("legacy descendants selector must not be used")


class FakeElementInfo:
    def __init__(self, element: FakeControl | FakeRoot) -> None:
        self.element = element
        if isinstance(element, FakeRoot):
            self.automation_id = ""
            self.control_type = "Window"
            return
        automation_id = getattr(element, "wrapper_automation_id", SAME_AS_RAW)
        control_type = getattr(element, "wrapper_control_type", SAME_AS_RAW)
        self.automation_id = (
            element.automation_id if automation_id is SAME_AS_RAW else automation_id
        )
        self.control_type = element.control_type if control_type is SAME_AS_RAW else control_type


class FakeNoPatternError(Exception):
    pass


class FakeValuePattern:
    def __init__(self, control: FakeControl) -> None:
        self.control = control

    @property
    def CurrentValue(self) -> object:
        if self.control.value_results:
            value = self.control.value_results.pop(0)
        else:
            value = self.control.value
        if isinstance(value, BaseException):
            raise value
        return value


class FakeUIAWrapper:
    def __init__(self, element_info: FakeElementInfo) -> None:
        self.element_info = element_info
        self.control = element_info.element

    def is_visible(self) -> bool:
        return self.control.visible

    def is_enabled(self) -> bool:
        return self.control.enabled

    def click_input(self) -> None:
        self.control.click_count += 1
        if self.control.click_error is not None:
            raise self.control.click_error

    def invoke(self) -> None:
        self.control.invoke_count += 1
        if self.control.click_error is not None:
            raise self.control.click_error

    def set_focus(self) -> None:
        self.control.focus_count += 1

    def rectangle(self) -> object:
        left, top, right, bottom = self.control.bounds
        return type("Rect", (), {"left": left, "top": top, "right": right, "bottom": bottom})()

    @property
    def iface_value(self) -> FakeValuePattern:
        self.control.value_pattern_count += 1
        if self.control.value_pattern_missing:
            raise FakeNoPatternError()
        return FakeValuePattern(self.control)


class FakeIUIA:
    def __init__(self) -> None:
        self.UIA_dll = type(
            "UIADll",
            (),
            {"UIA_AutomationIdPropertyId": "automation-id", "UIA_NamePropertyId": "name"},
        )()
        self.iuia = self
        self.tree_scope = {"descendants": "descendants"}
        self.known_control_types = {
            name: name for name in ("Button", "Edit", "Group", "TabItem", "Text")
        }
        self.conditions: list[tuple[str, str]] = []

    def CreatePropertyCondition(self, property_id: str, value: str) -> tuple[str, str]:
        condition = (property_id, value)
        self.conditions.append(condition)
        return condition


class FreshRootFactory:
    def __init__(self, roots: list[FakeRoot], roots_by_handle: dict[int, FakeRoot] | None = None) -> None:
        self.roots = roots
        self.roots_by_handle = roots_by_handle or {}
        self.calls = 0
        self.handles: list[int] = []
        self.uia = FakeIUIA()
        self.wrappers: list[FakeUIAWrapper] = []

    def element_info(self, handle_or_element: int | FakeControl) -> FakeElementInfo:
        if not isinstance(handle_or_element, int):
            return FakeElementInfo(handle_or_element)
        self.calls += 1
        self.handles.append(handle_or_element)
        root = self.roots_by_handle.get(handle_or_element)
        if root is None:
            root = self.roots[min(self.calls - 1, len(self.roots) - 1)]
        return FakeElementInfo(root)

    def wrapper(self, element_info: FakeElementInfo) -> FakeUIAWrapper:
        wrapper = FakeUIAWrapper(element_info)
        self.wrappers.append(wrapper)
        return wrapper


def native_harness(factory: FreshRootFactory, timeout: float = 0.1) -> object:
    harness = object.__new__(HARNESS.Goal02Harness)
    harness.uia_element_info_class = factory.element_info
    harness.uia_wrapper_class = factory.wrapper
    harness.iuia_class = lambda: factory.uia
    harness.no_pattern_error_class = FakeNoPatternError
    harness.ui_timeout = timeout
    return harness


def running_app(hwnd: int = 73, exited: bool = False) -> object:
    process = type("Process", (), {"pid": 91, "poll": lambda self: 0 if exited else None})()
    return type("Running", (), {"hwnd": hwnd, "process": process})()


def editor_control() -> FakeControl:
    return FakeControl("", "Edit", name=runtime.EDITOR_NAME)


class FakeEditorWin32:
    """The editor's document behind its own keyboard: Ctrl+A, Ctrl+C, typing."""

    def __init__(self, document: str = "", copies: list[str | None] | None = None) -> None:
        self.document = document
        self.copies = copies
        self.clipboard: str | None = "user clipboard"
        self.shortcuts: list[tuple[int, int]] = []
        self.keys: list[tuple[int, int]] = []
        self.unicode_writes: list[tuple[int, str]] = []
        self.foreground: list[int] = []

    def require_foreground(self, hwnd: int, _timeout: float = 2.0) -> None:
        self.foreground.append(hwnd)

    def send_shortcut(self, hwnd: int, key: int) -> None:
        self.shortcuts.append((hwnd, key))
        if key == runtime.VK_C:
            # nsh-base 0.5.1 copies an empty selection line-wise, so an empty
            # document copies as its one empty line (`editor/commands.rs` `copy`).
            copied = self.copies.pop(0) if self.copies else (self.document or "\n")
            if copied is not None:
                self.clipboard = copied

    def send_unicode(self, hwnd: int, value: str) -> None:
        self.unicode_writes.append((hwnd, value))
        self.document = value

    def send_key(self, hwnd: int, key: int) -> None:
        self.keys.append((hwnd, key))
        if key == runtime.VK_BACK:
            self.document = ""


def editor_harness(
    win32: FakeEditorWin32, controls: list[FakeControl] | None = None, timeout: float = 0.1
) -> tuple[object, FakeControl]:
    editor = editor_control()
    harness = native_harness(FreshRootFactory([FakeRoot([editor, *(controls or [])])]), timeout)
    harness.win32 = win32
    harness.read_text_clipboard = lambda: win32.clipboard
    harness.write_text_clipboard = lambda value: setattr(win32, "clipboard", value)
    return harness, editor


class SessionIntegrityAndOutcomeTests(unittest.TestCase):
    def test_requires_same_session_and_integrity(self) -> None:
        parent = runtime.SecurityContext(3, 0x2000, "medium")

        self.assertIsNone(
            runtime.security_context_failure(parent, runtime.SecurityContext(3, 0x2000, "medium"))
        )
        self.assertEqual(
            runtime.security_context_failure(parent, runtime.SecurityContext(4, 0x2000, "medium")),
            "PROCESS_SESSION_MISMATCH",
        )
        self.assertEqual(
            runtime.security_context_failure(parent, runtime.SecurityContext(3, 0x3000, "high")),
            "PROCESS_INTEGRITY_MISMATCH",
        )

    def test_parses_blocked_outcome_without_accepting_free_form_text(self) -> None:
        self.assertEqual(
            runtime.parse_outcome_line("BLOCKED: INPUT_DESKTOP_LOCKED"),
            ("BLOCKED", "INPUT_DESKTOP_LOCKED"),
        )
        with self.assertRaisesRegex(ValueError, "invalid harness outcome"):
            runtime.parse_outcome_line("BLOCKED: secret document contents")

    def test_product_timeout_is_fail_not_blocked(self) -> None:
        with self.assertRaises(runtime.HarnessFailure) as raised:
            runtime.wait_until(lambda: False, 0.001, "PRODUCT_TIMEOUT", interval=0.0)

        self.assertEqual(raised.exception.code, "PRODUCT_TIMEOUT")
        self.assertNotIsInstance(raised.exception, runtime.HarnessBlocked)

    def test_preflight_failure_and_prerequisite_block_have_distinct_exit_codes(self) -> None:
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        exe = Path(temporary.name) / "markturbo.exe"
        exe.write_bytes(b"candidate")
        args = HARNESS.parse_args(["--exe", str(exe), "--expect-exe-sha256", HASH])

        def fail(*_args: object) -> object:
            raise runtime.HarnessFailure("WINDOWS_11_REQUIRED")

        def block(*_args: object) -> object:
            raise runtime.HarnessBlocked("INPUT_DESKTOP_LOCKED")

        with mock.patch.dict(HARNESS.__dict__, {"preflight": fail}):
            fail_code, fail_evidence, fail_reason = runtime.run_native_acceptance(
                args, HARNESS.native_run_plan()
            )
        with mock.patch.dict(HARNESS.__dict__, {"preflight": block}):
            block_code, block_evidence, _ = runtime.run_native_acceptance(
                args, HARNESS.native_run_plan()
            )

        self.assertEqual((fail_code, fail_evidence["status"]), (1, "FAIL"))
        self.assertEqual((block_code, block_evidence["status"]), (2, "BLOCKED"))
        self.assertEqual(fail_reason, "WINDOWS_11_REQUIRED")
        HARNESS.validate_evidence(fail_evidence)
        HARNESS.validate_evidence(block_evidence)

    def test_foreground_failure_exposes_only_content_free_diagnostics(self) -> None:
        class FakeUser32:
            def ShowWindow(self, _hwnd, _command):
                return 1

            def BringWindowToTop(self, _hwnd):
                return 0

            def SetForegroundWindow(self, _hwnd):
                return 0

            def GetForegroundWindow(self):
                return 456

        win32 = object.__new__(runtime.Win32)
        win32.user32 = FakeUser32()
        win32.send_inputs = lambda _inputs: None

        with self.assertRaises(runtime.HarnessBlocked) as raised:
            win32.require_foreground(123, timeout=0.0)

        self.assertEqual(raised.exception.code, "FOREGROUND_PERMISSION_DENIED")
        self.assertEqual(
            raised.exception.diagnostics,
            {
                "requested_hwnd": 123,
                "foreground_hwnd": 456,
                "show_window_return": True,
                "bring_to_top_return": False,
                "set_foreground_return": False,
                "foreground_attempts": 0,
            },
        )

    def test_foreground_activation_retries_until_the_window_is_ready(self) -> None:
        class FakeUser32:
            def __init__(self) -> None:
                self.foreground = 0
                self.set_calls = 0

            def ShowWindow(self, _hwnd, _command):
                return 1

            def BringWindowToTop(self, _hwnd):
                return 1

            def SetForegroundWindow(self, hwnd):
                self.set_calls += 1
                if self.set_calls == 2:
                    self.foreground = hwnd
                return self.foreground == hwnd

            def GetForegroundWindow(self):
                return self.foreground

        user32 = FakeUser32()
        win32 = object.__new__(runtime.Win32)
        win32.user32 = user32
        win32.send_inputs = lambda _inputs: None

        win32.require_foreground(123, timeout=0.2)

        self.assertEqual(user32.foreground, 123)
        self.assertEqual(user32.set_calls, 2)

    def test_foreground_activation_uses_one_alt_unlock_before_rechecking(self) -> None:
        class FakeUser32:
            def __init__(self) -> None:
                self.foreground = 0
                self.alt_sent = False

            def ShowWindow(self, _hwnd, _command):
                return 1

            def BringWindowToTop(self, _hwnd):
                return 1

            def SetForegroundWindow(self, hwnd):
                if self.alt_sent:
                    self.foreground = hwnd
                return self.foreground == hwnd

            def GetForegroundWindow(self):
                return self.foreground

        user32 = FakeUser32()
        sent: list[list[runtime.INPUT]] = []
        win32 = object.__new__(runtime.Win32)
        win32.user32 = user32

        def send_inputs(inputs):
            sent.append(inputs)
            user32.alt_sent = True

        win32.send_inputs = send_inputs

        win32.require_foreground(123, timeout=0.0)

        self.assertEqual(len(sent), 1)
        self.assertEqual([item.ki.wVk for item in sent[0]], [runtime.VK_MENU, runtime.VK_MENU])
        self.assertEqual(
            [item.ki.dwFlags for item in sent[0]],
            [0, runtime.KEYEVENTF_KEYUP],
        )
        self.assertEqual(user32.foreground, 123)


class FocusUser32:
    def __init__(self) -> None:
        self.focus = 73
        self.foreground = 73

    def GetForegroundWindow(self):
        return self.foreground

    def GetGUIThreadInfo(self, thread, pointer):
        assert thread == 0
        info = runtime.ctypes.cast(pointer, runtime.ctypes.POINTER(runtime.GUITHREADINFO)).contents
        assert info.cbSize == runtime.ctypes.sizeof(runtime.GUITHREADINFO)
        info.hwndFocus = self.focus
        return True


class NativeFocusTests(unittest.TestCase):
    def test_focus_query_does_not_accept_another_foreground_or_unreadable_state(self):
        win32 = object.__new__(runtime.Win32)
        os = FocusUser32()
        win32.user32 = os
        self.assertEqual(win32.foreground_focus(73), 73)
        os.foreground = 99
        self.assertEqual(win32.foreground_focus(73), 0)
        os.foreground = 73
        os.GetGUIThreadInfo = lambda *_args: False
        with self.assertRaisesRegex(runtime.HarnessFailure, "NATIVE_FOCUS_QUERY_FAILED"):
            win32.foreground_focus(73)


class SelectorAndOrchestrationTests(unittest.TestCase):
    def test_selectors_match_the_nsh_production_source(self) -> None:
        self.assertEqual(HARNESS.TAB_CLOSE_AUTOMATION_ID, "rail-close")
        self.assertEqual(runtime.EDITOR_NAME, "Markdown source")
        self.assertEqual(
            runtime.GUARD_BUTTON_NAMES,
            {"Save": "Save", "Discard": "Don't save", "Cancel": "Cancel"},
        )
        self.assertEqual(
            runtime.GUARD_BUTTON_AUTOMATION_IDS,
            {"Save": "guard-save", "Discard": "guard-dont-save", "Cancel": "guard-cancel"},
        )

    def test_source_layout_waits_for_the_editor_without_the_menu(self) -> None:
        anchor = FakeControl(runtime.MENU_BUTTON_AUTOMATION_ID, "Button")
        harness = native_harness(FreshRootFactory([FakeRoot([editor_control(), anchor])]))

        harness.activate_source_layout(running_app())

        self.assertEqual(anchor.invoke_count, 0)

    def test_source_layout_opens_the_edit_mode_from_the_title_menu(self) -> None:
        anchor = FakeControl(runtime.MENU_BUTTON_AUTOMATION_ID, "Button")
        edit = FakeControl(runtime.MENU_EDIT_AUTOMATION_ID, "Button")
        editor = editor_control()
        root = FakeRoot([anchor])
        edit.click_error = None
        harness = native_harness(FreshRootFactory([root]), timeout=0.05)
        original = FakeUIAWrapper.invoke

        def invoke(wrapper: FakeUIAWrapper) -> None:
            original(wrapper)
            if wrapper.control is anchor:
                root.controls.append(edit)
            elif wrapper.control is edit:
                root.controls[:] = [anchor, editor]

        with mock.patch.object(FakeUIAWrapper, "invoke", invoke):
            harness.activate_source_layout(running_app())

        self.assertEqual((anchor.invoke_count, edit.invoke_count), (1, 1))

    def test_menu_command_reuses_an_open_menu(self) -> None:
        anchor = FakeControl(runtime.MENU_BUTTON_AUTOMATION_ID, "Button")
        save = FakeControl(runtime.MENU_SAVE_AUTOMATION_ID, "Button")
        harness = native_harness(FreshRootFactory([FakeRoot([anchor, save])]))

        harness.save_via_menu(running_app())

        self.assertEqual((anchor.invoke_count, save.invoke_count, save.click_count), (0, 1, 0))

    def guard_root(self) -> tuple[FakeRoot, dict[str, FakeControl], FakeControl]:
        buttons = {
            decision: FakeControl(
                runtime.GUARD_BUTTON_AUTOMATION_IDS[decision], "Button", name=name
            )
            for decision, name in runtime.GUARD_BUTTON_NAMES.items()
        }
        title_save = FakeControl("title-save", "Button", name="Save")
        heading = FakeControl("", "Text", name=runtime.GUARD_HEADING)
        return FakeRoot([title_save, heading, *buttons.values()]), buttons, title_save

    def test_guard_card_finds_its_buttons_by_their_own_ids(self) -> None:
        root, buttons, title_save = self.guard_root()
        harness = native_harness(FreshRootFactory([root]))

        found = harness.lifecycle_dialog(running_app())
        harness.click_lifecycle_decision(running_app(), "Discard")

        self.assertEqual(set(found), {"Save", "Discard", "Cancel"})
        self.assertEqual(buttons["Discard"].click_count, 1)
        self.assertEqual(buttons["Save"].click_count, 0)
        self.assertEqual(title_save.click_count, 0)

    def test_guard_card_requires_its_heading_and_a_running_process(self) -> None:
        root, _buttons, _title_save = self.guard_root()
        root.controls = [control for control in root.controls if control.control_type != "Text"]
        missing = native_harness(FreshRootFactory([root]), timeout=0.001)
        with self.assertRaises(runtime.HarnessFailure) as timeout:
            missing.lifecycle_dialog(running_app())
        self.assertEqual(timeout.exception.code, "GUARD_CARD_UIA_TIMEOUT")

        exited = native_harness(FreshRootFactory([self.guard_root()[0]]))
        with self.assertRaises(runtime.HarnessFailure) as closed:
            exited.lifecycle_dialog(running_app(exited=True))
        self.assertEqual(closed.exception.code, "PROCESS_EXITED_BEFORE_GUARD")

    def test_guard_card_rejects_two_buttons_with_one_decision(self) -> None:
        root, _buttons, _title_save = self.guard_root()
        root.controls.append(FakeControl("guard-cancel", "Button", name="Cancel"))
        harness = native_harness(FreshRootFactory([root]))
        with self.assertRaises(runtime.HarnessFailure) as raised:
            harness.lifecycle_dialog(running_app())
        self.assertEqual(raised.exception.code, "GUARD_BUTTON_UIA_CONTRACT_MISMATCH")

    def test_hold_control_presses_the_pointer_at_the_control_centre(self) -> None:
        control = FakeControl("consent-send", "Button", bounds=(100, 40, 264, 76))
        harness = native_harness(FreshRootFactory([FakeRoot([control])]))
        harness.win32 = mock.Mock()

        harness.hold_control(
            running_app(), FakeUIAWrapper(FakeElementInfo(control)), 1.8, "HOLD_FAILED"
        )

        harness.win32.press_and_hold.assert_called_once_with(73, 182, 58, 1.8)
        self.assertEqual((control.click_count, control.invoke_count), (0, 0))

    def test_find_control_refreshes_root_after_a_stale_lookup(self) -> None:
        stale = FakeRoot()
        current_control = FakeControl(HARNESS.TAB_CLOSE_AUTOMATION_ID, "Edit")
        current = FakeRoot([current_control])
        factory = FreshRootFactory([stale, current])
        harness = native_harness(factory)

        control = harness.find_control(
            running_app(),
            HARNESS.TAB_CLOSE_AUTOMATION_ID,
            "Edit",
            "SOURCE_EDITOR_UIA_TIMEOUT",
            "SOURCE_EDITOR_UIA_CONTRACT_MISMATCH",
        )

        self.assertIs(control, factory.wrappers[0])
        self.assertGreaterEqual(factory.calls, 2)
        self.assertEqual(factory.handles, [73] * factory.calls)
        self.assertIs(factory.wrappers[0].element_info.element, current_control)

    def test_find_control_uses_target_specific_timeout_without_label_fallback(self) -> None:
        root = FakeRoot([FakeControl("other-id", "Edit")])
        harness = native_harness(FreshRootFactory([root]), timeout=0.001)

        with self.assertRaises(runtime.HarnessFailure) as raised:
            harness.find_control(
                running_app(),
                HARNESS.TAB_CLOSE_AUTOMATION_ID,
                "Edit",
                "SOURCE_EDITOR_UIA_TIMEOUT",
                "SOURCE_EDITOR_UIA_CONTRACT_MISMATCH",
            )

        self.assertEqual(raised.exception.code, "SOURCE_EDITOR_UIA_TIMEOUT")
        self.assertEqual(
            root.find_calls[0],
            ("descendants", ("automation-id", HARNESS.TAB_CLOSE_AUTOMATION_ID)),
        )

    def test_find_control_fails_immediately_on_id_or_type_mismatch(self) -> None:
        mismatched = FakeRoot([FakeControl(HARNESS.TAB_CLOSE_AUTOMATION_ID, "Button")])
        factory = FreshRootFactory([mismatched])
        harness = native_harness(factory)

        with self.assertRaises(runtime.HarnessFailure) as raised:
            harness.find_control(
                running_app(),
                HARNESS.TAB_CLOSE_AUTOMATION_ID,
                "Edit",
                "SOURCE_EDITOR_UIA_TIMEOUT",
                "SOURCE_EDITOR_UIA_CONTRACT_MISMATCH",
            )

        self.assertEqual(raised.exception.code, "SOURCE_EDITOR_UIA_CONTRACT_MISMATCH")
        self.assertEqual(factory.wrappers, [])

    def test_find_control_retries_raw_property_fault_before_wrapper_construction(self) -> None:
        secret = "UNIQUE-DOCUMENT-CONTENT"
        stale = FakeRoot(
            [
                FakeControl(
                    HARNESS.TAB_CLOSE_AUTOMATION_ID,
                    "Edit",
                    property_error=RuntimeError(secret),
                )
            ]
        )
        current_control = FakeControl(HARNESS.TAB_CLOSE_AUTOMATION_ID, "Edit")
        factory = FreshRootFactory([stale, FakeRoot([current_control])])
        harness = native_harness(factory)

        control = harness.find_control(
            running_app(),
            HARNESS.TAB_CLOSE_AUTOMATION_ID,
            "Edit",
            "SOURCE_EDITOR_UIA_TIMEOUT",
            "SOURCE_EDITOR_UIA_CONTRACT_MISMATCH",
        )

        self.assertIs(control, factory.wrappers[0])
        self.assertEqual(factory.calls, 2)
        self.assertNotIn(secret, control.element_info.automation_id)

    def test_find_control_retries_post_wrapper_missing_metadata(self) -> None:
        stale = FakeRoot(
            [
                FakeControl(
                    HARNESS.TAB_CLOSE_AUTOMATION_ID,
                    "Edit",
                    wrapper_automation_id=None,
                )
            ]
        )
        fresh_control = FakeControl(HARNESS.TAB_CLOSE_AUTOMATION_ID, "Edit")
        factory = FreshRootFactory([stale, FakeRoot([fresh_control])])
        harness = native_harness(factory)

        control = harness.find_control(
            running_app(),
            HARNESS.TAB_CLOSE_AUTOMATION_ID,
            "Edit",
            "SOURCE_EDITOR_UIA_TIMEOUT",
            "SOURCE_EDITOR_UIA_CONTRACT_MISMATCH",
        )

        self.assertIs(control, factory.wrappers[-1])
        self.assertEqual(factory.calls, 2)
        self.assertIs(factory.wrappers[-1].element_info.element, fresh_control)

    def test_find_control_retries_hidden_and_disabled_controls_until_ready(self) -> None:
        hidden = FakeRoot([FakeControl(HARNESS.TAB_CLOSE_AUTOMATION_ID, "Edit", visible=False)])
        disabled = FakeRoot([FakeControl(HARNESS.TAB_CLOSE_AUTOMATION_ID, "Edit", enabled=False)])
        ready_control = FakeControl(HARNESS.TAB_CLOSE_AUTOMATION_ID, "Edit")
        ready = FakeRoot([ready_control])
        factory = FreshRootFactory([hidden, disabled, ready])
        harness = native_harness(factory)

        elapsed = [0.0]

        def advance(seconds: float) -> None:
            elapsed[0] += seconds

        with (
            mock.patch.object(runtime.time, "perf_counter", side_effect=lambda: elapsed[0]),
            mock.patch.object(runtime.time, "sleep", side_effect=advance),
        ):
            control = harness.find_control(
                running_app(),
                HARNESS.TAB_CLOSE_AUTOMATION_ID,
                "Edit",
                "SOURCE_EDITOR_UIA_TIMEOUT",
                "SOURCE_EDITOR_UIA_CONTRACT_MISMATCH",
            )

        self.assertIs(control, factory.wrappers[-1])
        self.assertEqual(factory.calls, 3)
        self.assertIs(factory.wrappers[-1].element_info.element, ready_control)

    def test_raw_findall_and_getelement_errors_fail_closed_without_text(self) -> None:
        secret = "UNIQUE-DOCUMENT-CONTENT"
        roots = (
            FakeRoot(find_error=RuntimeError(secret)),
            FakeRoot([FakeControl(HARNESS.TAB_CLOSE_AUTOMATION_ID, "Edit")], get_error=RuntimeError(secret)),
        )
        for root in roots:
            with self.subTest(root=root), self.assertRaises(runtime.HarnessFailure) as raised:
                native_harness(FreshRootFactory([root]), timeout=0.001).find_control(
                    running_app(),
                    HARNESS.TAB_CLOSE_AUTOMATION_ID,
                    "Edit",
                    "SOURCE_EDITOR_UIA_TIMEOUT",
                    "SOURCE_EDITOR_UIA_CONTRACT_MISMATCH",
                )
            self.assertEqual(raised.exception.code, "SOURCE_EDITOR_UIA_TIMEOUT")
            self.assertEqual(raised.exception.detail, "RuntimeError")
            self.assertNotIn(secret, raised.exception.detail)

    def test_control_absent_does_not_treat_query_errors_as_absence(self) -> None:
        secret = "UNIQUE-DOCUMENT-CONTENT"
        harness = native_harness(
            FreshRootFactory([FakeRoot(find_error=RuntimeError(secret))])
        )

        with self.assertRaises(runtime.HarnessFailure) as raised:
            harness.control_absent(
                running_app(),
                HARNESS.TAB_CLOSE_AUTOMATION_ID,
                "Edit",
                "DISCARD_EDITOR_ABSENCE_UIA_QUERY_FAILED",
                "DISCARD_EDITOR_ABSENCE_UIA_CONTRACT_MISMATCH",
            )

        self.assertEqual(raised.exception.code, "DISCARD_EDITOR_ABSENCE_UIA_QUERY_FAILED")
        self.assertEqual(raised.exception.detail, "RuntimeError")
        self.assertNotIn(secret, raised.exception.detail)

    def test_control_absent_does_not_treat_raw_property_errors_as_absence(self) -> None:
        secret = "UNIQUE-DOCUMENT-CONTENT"
        root = FakeRoot(
            [
                FakeControl(
                    HARNESS.TAB_CLOSE_AUTOMATION_ID,
                    "Edit",
                    property_error=RuntimeError(secret),
                )
            ]
        )
        harness = native_harness(FreshRootFactory([root]))

        with self.assertRaises(runtime.HarnessFailure) as raised:
            harness.control_absent(
                running_app(),
                HARNESS.TAB_CLOSE_AUTOMATION_ID,
                "Edit",
                "DISCARD_EDITOR_ABSENCE_UIA_QUERY_FAILED",
                "DISCARD_EDITOR_ABSENCE_UIA_CONTRACT_MISMATCH",
            )

        self.assertEqual(raised.exception.code, "DISCARD_EDITOR_ABSENCE_UIA_QUERY_FAILED")
        self.assertEqual(raised.exception.detail, "RuntimeError")
        self.assertNotIn(secret, raised.exception.detail)

    def test_control_absent_fails_closed_on_post_wrapper_missing_metadata(self) -> None:
        root = FakeRoot(
            [
                FakeControl(
                    HARNESS.TAB_CLOSE_AUTOMATION_ID,
                    "Edit",
                    wrapper_control_type=None,
                )
            ]
        )
        harness = native_harness(FreshRootFactory([root]))

        with self.assertRaises(runtime.HarnessFailure) as raised:
            harness.control_absent(
                running_app(),
                HARNESS.TAB_CLOSE_AUTOMATION_ID,
                "Edit",
                "DISCARD_EDITOR_ABSENCE_UIA_QUERY_FAILED",
                "DISCARD_EDITOR_ABSENCE_UIA_CONTRACT_MISMATCH",
            )

        self.assertEqual(raised.exception.code, "DISCARD_EDITOR_ABSENCE_UIA_QUERY_FAILED")
        self.assertEqual(raised.exception.detail, "RuntimeError")

    def test_control_absent_requires_a_successful_zero_result_query(self) -> None:
        root = FakeRoot()
        harness = native_harness(FreshRootFactory([root]))

        self.assertTrue(
            harness.control_absent(
                running_app(),
                HARNESS.TAB_CLOSE_AUTOMATION_ID,
                "Edit",
                "DISCARD_EDITOR_ABSENCE_UIA_QUERY_FAILED",
                "DISCARD_EDITOR_ABSENCE_UIA_CONTRACT_MISMATCH",
            )
        )

    def test_control_absent_fails_on_a_same_id_wrong_type(self) -> None:
        root = FakeRoot([FakeControl(HARNESS.TAB_CLOSE_AUTOMATION_ID, "Button")])
        harness = native_harness(FreshRootFactory([root]))

        with self.assertRaises(runtime.HarnessFailure) as raised:
            harness.control_absent(
                running_app(),
                HARNESS.TAB_CLOSE_AUTOMATION_ID,
                "Edit",
                "DISCARD_EDITOR_ABSENCE_UIA_QUERY_FAILED",
                "DISCARD_EDITOR_ABSENCE_UIA_CONTRACT_MISMATCH",
            )

        self.assertEqual(raised.exception.code, "DISCARD_EDITOR_ABSENCE_UIA_CONTRACT_MISMATCH")

    def test_editor_text_is_copied_by_select_all_and_the_clipboard_restored(self) -> None:
        # An empty document copies line-wise as "\n" (nsh-base 0.5.1
        # `editor/commands.rs` `copy`), like a document of one line break: a
        # read cannot tell them apart and reports exactly what was copied.
        for value, copied in (
            ("", "\n"),
            ("ASCII", "ASCII"),
            ("CJK-\u4fdd\u5b58-\U0001f680", "CJK-\u4fdd\u5b58-\U0001f680"),
            ("e\u0301", "e\u0301"),
            ("a\nb", "a\nb"),
        ):
            with self.subTest(value=value):
                win32 = FakeEditorWin32(value)
                harness, editor = editor_harness(win32)

                self.assertEqual(
                    harness.read_editor_fingerprint(running_app()), runtime.fingerprint_text(copied)
                )
                self.assertEqual(win32.shortcuts, [(73, runtime.VK_A), (73, runtime.VK_C)])
                self.assertEqual(win32.clipboard, "user clipboard")
                self.assertEqual(editor.focus_count, 1)
                self.assertEqual(editor.value_pattern_count, 0)

    def test_editor_read_is_none_without_an_editor(self) -> None:
        win32 = FakeEditorWin32("text")
        harness = native_harness(FreshRootFactory([FakeRoot()]))
        harness.win32 = win32

        self.assertIsNone(harness.read_editor_fingerprint(running_app()))
        self.assertEqual(win32.shortcuts, [])

    def test_wait_editor_fingerprint_retries_until_the_exact_copy(self) -> None:
        win32 = FakeEditorWin32("after", copies=["before", "after"])
        harness, _editor = editor_harness(win32, timeout=1.0)

        actual, _ = harness.wait_editor_fingerprint(
            running_app(), runtime.fingerprint_text("after"), 1.0
        )

        self.assertEqual(actual, runtime.fingerprint_text("after"))
        self.assertEqual(win32.shortcuts.count((73, runtime.VK_C)), 2)
        self.assertEqual(win32.clipboard, "user clipboard")

    def test_editor_timeouts_distinguish_wrong_and_uncopyable_text(self) -> None:
        wrong, _editor = editor_harness(FakeEditorWin32("wrong"), timeout=0.001)
        with self.assertRaises(runtime.HarnessFailure) as wrong_timeout:
            wrong.wait_editor_fingerprint(running_app(), runtime.fingerprint_text("expected"), 0.001)
        self.assertEqual(wrong_timeout.exception.code, "EDITOR_EXACT_BYTES_TIMEOUT")

        silent = FakeEditorWin32("UNIQUE-DOCUMENT-CONTENT", copies=[None] * 1000)
        uncopyable, _editor = editor_harness(silent, timeout=0.001)
        with self.assertRaises(runtime.HarnessFailure) as unreadable:
            uncopyable.wait_editor_fingerprint(
                running_app(), runtime.fingerprint_text("expected"), 0.001
            )
        self.assertEqual(unreadable.exception.code, "EDITOR_TEXT_TIMEOUT")
        self.assertEqual(unreadable.exception.detail, "HarnessFailure")
        self.assertEqual(silent.clipboard, "user clipboard")

    def test_replace_editor_types_over_select_all_and_verifies_the_copy(self) -> None:
        value = "CJK-\u4fdd\u5b58-\U0001f680"
        win32 = FakeEditorWin32("before")
        harness, editor = editor_harness(win32)

        self.assertEqual(harness.replace_editor(running_app(), value), runtime.fingerprint_text(value))
        self.assertEqual(win32.shortcuts, [(73, runtime.VK_A), (73, runtime.VK_A), (73, runtime.VK_C)])
        self.assertEqual(win32.unicode_writes, [(73, value)])
        self.assertEqual(editor.click_count, 0)

    def test_replace_editor_clears_nonempty_value_with_backspace(self) -> None:
        win32 = FakeEditorWin32("before")
        harness, _editor = editor_harness(win32)

        self.assertEqual(harness.replace_editor(running_app(), ""), runtime.fingerprint_text(""))
        self.assertEqual(win32.document, "")
        self.assertEqual(win32.keys, [(73, runtime.VK_BACK)])
        self.assertEqual(win32.unicode_writes, [])

    def test_click_control_never_retries_a_failed_input(self) -> None:
        control = FakeControl(
            HARNESS.TAB_CLOSE_AUTOMATION_ID,
            "Button",
            click_error=RuntimeError("UNIQUE-DOCUMENT-CONTENT"),
        )
        harness = object.__new__(HARNESS.Goal02Harness)

        with self.assertRaises(runtime.HarnessFailure) as raised:
            harness.click_control(
                FakeUIAWrapper(FakeElementInfo(control)), "EDITOR_POINTER_FOCUS_FAILED"
            )

        self.assertEqual(raised.exception.code, "EDITOR_POINTER_FOCUS_FAILED")
        self.assertEqual(raised.exception.detail, "RuntimeError")
        self.assertEqual(control.click_count, 1)

    def test_launch_uses_the_configured_foreground_timeout(self) -> None:
        events: list[tuple[object, ...]] = []
        context = runtime.SecurityContext(1, 0x2000, "medium")

        class FakeProcess:
            pid = 91

            def start(self):
                events.append(("start",))

            def poll(self):
                return None

        class FakeWindow:
            handle = 73

            def wait(self, state, timeout):
                events.append(("wait", state, timeout))

        class FakeApplication:
            def __init__(self, backend):
                events.append(("backend", backend))

            def connect(self, process, timeout):
                events.append(("connect", process, timeout))
                return self

            def top_window(self):
                return FakeWindow()

        class FakeWin32:
            def security_context(self, pid):
                events.append(("context", pid))
                return context

            def require_foreground(self, hwnd, timeout):
                events.append(("foreground", hwnd, timeout))

        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary).resolve()
            data = root / "data"
            config = root / "config"
            workspace = root / "workspace"
            stderr = root / "stderr.log"
            for directory in (data, config, workspace):
                directory.mkdir()
            process = FakeProcess()
            harness = runtime.NativeHarness(
                root / "markturbo.exe",
                root,
                3.5,
                FakeWin32(),
                FakeApplication,
                object,
                object,
                object,
                object,
                context,
            )
            with mock.patch.object(runtime, "OwnedProcess", return_value=process) as owned:
                app = harness.launch_app(None, data, config, workspace, stderr)

        self.assertEqual(app.spec.args, (str(root / "markturbo.exe"),))
        self.assertEqual(events[0], ("start",))
        self.assertEqual(events[-1], ("foreground", 73, 2.0))
        owned.assert_called_once_with(harness.win32, app.spec)

    def test_launch_clicks_the_window_center_before_retrying_foreground(self) -> None:
        events: list[tuple[object, ...]] = []
        context = runtime.SecurityContext(1, 0x2000, "medium")

        class FakeProcess:
            pid = 91

            def start(self):
                events.append(("start",))

            def poll(self):
                return None

        class FakeBounds:
            def width(self):
                return 800

            def height(self):
                return 600

        class FakeWindow:
            handle = 73

            def wait(self, state, timeout):
                events.append(("wait", state, timeout))

            def rectangle(self):
                return FakeBounds()

            def click_input(self, coords):
                events.append(("click", coords))

        class FakeApplication:
            def __init__(self, backend):
                events.append(("backend", backend))

            def connect(self, process, timeout):
                events.append(("connect", process, timeout))
                return self

            def top_window(self):
                return FakeWindow()

        class FakeWin32:
            def __init__(self) -> None:
                self.calls = 0

            def security_context(self, pid):
                return context

            def require_foreground(self, hwnd, timeout):
                self.calls += 1
                events.append(("foreground", hwnd, timeout))
                if self.calls == 1:
                    raise runtime.HarnessBlocked("FOREGROUND_PERMISSION_DENIED")

        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary).resolve()
            data = root / "data"
            config = root / "config"
            workspace = root / "workspace"
            stderr = root / "stderr.log"
            for directory in (data, config, workspace):
                directory.mkdir()
            process = FakeProcess()
            harness = runtime.NativeHarness(
                root / "markturbo.exe",
                root,
                3.5,
                FakeWin32(),
                FakeApplication,
                object,
                object,
                object,
                object,
                context,
            )
            with mock.patch.object(runtime, "OwnedProcess", return_value=process):
                harness.launch_app(None, data, config, workspace, stderr)

        self.assertEqual(events[0], ("start",))
        self.assertIn(("foreground", 73, 2.0), events)
        self.assertIn(("click", (400, 300)), events)
        self.assertEqual(events[-1], ("foreground", 73, 3.5))


class AlertWin32:
    """Keyboard ownership and the keys that reach an nsh system alert."""

    def __init__(self, focus: int = 73, on_enter=None) -> None:
        self.focus = focus
        self.on_enter = on_enter
        self.events: list[tuple[object, ...]] = []

    def require_foreground(self, hwnd: int, _timeout: float = 2.0) -> None:
        self.events.append(("foreground", hwnd))

    def foreground_focus(self, _hwnd: int) -> int:
        return self.focus

    def send_key(self, hwnd: int, key: int) -> None:
        self.events.append(("key", hwnd, key))
        if key == runtime.VK_RETURN and self.on_enter is not None:
            self.on_enter()


class SystemAlertTests(unittest.TestCase):
    """nsh 0.5.1 alerts are outside UIA: Enter answers them, effects prove them."""

    def harness(self, win32: AlertWin32) -> object:
        harness = native_harness(FreshRootFactory([FakeRoot()]))
        harness.win32 = win32
        return harness

    def test_enter_answers_the_alert_from_the_main_windows_keyboard_focus(self) -> None:
        win32 = AlertWin32()
        with mock.patch.object(runtime, "SYSTEM_ALERT_SETTLE_SECONDS", 0):
            self.harness(win32).confirm_system_alert(running_app(), lambda: True, "EARLY")
        self.assertEqual(win32.events, [("foreground", 73), ("key", 73, runtime.VK_RETURN)])

    def test_an_effect_before_enter_fails_without_sending_enter(self) -> None:
        win32 = AlertWin32()
        with (
            mock.patch.object(runtime, "SYSTEM_ALERT_SETTLE_SECONDS", 0),
            self.assertRaises(runtime.HarnessFailure) as raised,
        ):
            self.harness(win32).confirm_system_alert(running_app(), lambda: False, "EARLY")
        self.assertEqual(raised.exception.code, "EARLY")
        self.assertEqual(win32.events, [])

    def test_enter_is_withheld_while_another_window_owns_the_keyboard(self) -> None:
        win32 = AlertWin32(focus=99)
        with (
            mock.patch.object(runtime, "SYSTEM_ALERT_SETTLE_SECONDS", 0),
            self.assertRaises(runtime.HarnessFailure) as raised,
        ):
            self.harness(win32).confirm_system_alert(running_app(), lambda: True, "EARLY")
        self.assertEqual(raised.exception.code, "SYSTEM_ALERT_KEYBOARD_FOCUS_TIMEOUT")
        self.assertNotIn(("key", 73, runtime.VK_RETURN), win32.events)

    def conflict_scenario(self, *, overwrite_on_keep_mine: bool, overwrite_on_enter: bool):
        directory = tempfile.TemporaryDirectory()
        self.addCleanup(directory.cleanup)
        harness = self.harness(AlertWin32())
        harness.root = Path(directory.name)
        app = running_app()
        app.security_context = runtime.SecurityContext(1, 0x2000, "medium")
        document = (
            harness.root / "cases" / HARNESS.CASE_EXTERNAL_CONFLICT / "workspace" / "conflict.md"
        ).resolve()
        mine = HARNESS.CONFLICT_EDIT.encode("utf-8")
        dirty = runtime.fingerprint_bytes(mine)
        keep_mine = object()
        harness.launch = mock.Mock(return_value=app)
        harness.replace_editor = mock.Mock(return_value=dirty)
        harness.save_via_menu = mock.Mock()
        harness.wait_name = mock.Mock()
        harness.editor_fingerprint = mock.Mock(return_value=dirty)
        harness.find_control = mock.Mock(return_value=keep_mine)
        harness.close_app = mock.Mock()
        harness.reap = mock.Mock()

        def keep_mine_pressed(control: object, _code: str) -> None:
            self.assertIs(control, keep_mine)
            if overwrite_on_keep_mine:
                runtime.write_durable(document, mine)

        harness.click_control = mock.Mock(side_effect=keep_mine_pressed)
        if overwrite_on_enter:
            harness.win32.on_enter = lambda: runtime.write_durable(document, mine)
        patches = contextlib.ExitStack()
        self.addCleanup(patches.close)
        patches.enter_context(mock.patch.object(runtime, "SYSTEM_ALERT_SETTLE_SECONDS", 0))
        patches.enter_context(mock.patch.object(HARNESS, "scan_runtime_artifacts", return_value={}))
        return harness, app

    def test_external_conflict_overwrites_only_after_enter_confirms_the_alert(self) -> None:
        harness, app = self.conflict_scenario(
            overwrite_on_keep_mine=False, overwrite_on_enter=True
        )
        observations = harness.scenario_external_conflict()
        self.assertTrue(observations["explicit_overwrite_used"])
        self.assertEqual(
            observations["saved_after_explicit_overwrite"], observations["dirty_editor"]
        )
        self.assertIn(("key", app.hwnd, runtime.VK_RETURN), harness.win32.events)
        harness.find_control.assert_called_once()
        self.assertEqual(
            harness.find_control.call_args.args[1:3], (HARNESS.KEEP_MINE_AUTOMATION_ID, "Button")
        )

    def test_external_conflict_fails_when_keep_mine_writes_without_the_alert(self) -> None:
        harness, app = self.conflict_scenario(
            overwrite_on_keep_mine=True, overwrite_on_enter=False
        )
        with self.assertRaises(runtime.HarnessFailure) as raised:
            harness.scenario_external_conflict()
        self.assertEqual(raised.exception.code, "KEEP_MINE_WROTE_BEFORE_CONFIRMATION")
        self.assertNotIn(("key", app.hwnd, runtime.VK_RETURN), harness.win32.events)

    def test_external_conflict_fails_when_enter_never_confirms(self) -> None:
        harness, _app = self.conflict_scenario(
            overwrite_on_keep_mine=False, overwrite_on_enter=False
        )
        with self.assertRaises(runtime.HarnessFailure) as raised:
            harness.scenario_external_conflict()
        self.assertEqual(raised.exception.code, "EXPLICIT_OVERWRITE_SAVE_TIMEOUT")


class SourceEditorTests(unittest.TestCase):
    def test_paste_editor_pastes_and_restores_the_text_clipboard(self) -> None:
        events = []

        class FakeWin32:
            def send_shortcut(self, hwnd, key):
                events.append(("shortcut", hwnd, key))

        class FakeHarness:
            ui_timeout = 1.0
            win32 = FakeWin32()

            def read_text_clipboard(self):
                events.append(("read",))
                return "previous text"

            def write_text_clipboard(self, value):
                events.append(("write", value))

            def focus_editor(self, app):
                events.append(("focus", app.hwnd))

            def wait_editor_fingerprint(self, app, expected, timeout):
                events.append(("wait", timeout))
                return expected, 0.0

        app = type("App", (), {"hwnd": 42})()
        result = runtime.NativeHarness.paste_editor(FakeHarness(), app, "emoji \U0001f680")

        self.assertEqual(result, runtime.fingerprint_text("emoji \U0001f680"))
        self.assertEqual(
            events,
            [
                ("read",),
                ("write", "emoji \U0001f680"),
                ("focus", 42),
                ("shortcut", 42, runtime.VK_A),
                ("shortcut", 42, runtime.VK_V),
                ("write", "previous text"),
                ("wait", 1.0),
            ],
        )

    def test_source_editor_focus_reads_uia_without_refocusing(self) -> None:
        class FakeControl:
            def has_keyboard_focus(self):
                return True

        class FakeHarness:
            def control_by_name(self, hwnd, name, control_type, mismatch_code):
                self.lookup = (hwnd, name, control_type, mismatch_code)
                return FakeControl()

        harness = FakeHarness()
        app = type("App", (), {"hwnd": 42})()

        self.assertTrue(runtime.NativeHarness.source_editor_has_focus(harness, app))
        self.assertEqual(
            harness.lookup, (42, runtime.EDITOR_NAME, "Edit", "SOURCE_EDITOR_UIA_CONTRACT_MISMATCH")
        )

        class BrokenHarness:
            def control_by_name(self, *_args):
                raise RuntimeError("UNIQUE-DOCUMENT-CONTENT")

        with self.assertRaises(runtime.HarnessFailure) as raised:
            runtime.NativeHarness.source_editor_has_focus(BrokenHarness(), app)
        self.assertEqual(
            (raised.exception.code, raised.exception.detail),
            ("SOURCE_EDITOR_FOCUS_QUERY_FAILED", "RuntimeError"),
        )
