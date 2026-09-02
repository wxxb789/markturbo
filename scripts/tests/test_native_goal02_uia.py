"""Goal 02 native harness tests without launching a UI."""

from ._native_goal02_support import *


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
        self.click_count = 0
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
        elements = [control for control in self.controls if control.automation_id == condition[1]]
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

    @property
    def iface_value(self) -> FakeValuePattern:
        self.control.value_pattern_count += 1
        if self.control.value_pattern_missing:
            raise FakeNoPatternError()
        return FakeValuePattern(self.control)


class FakeIUIA:
    def __init__(self) -> None:
        self.UIA_dll = type("UIADll", (), {"UIA_AutomationIdPropertyId": "automation-id"})()
        self.iuia = self
        self.tree_scope = {"descendants": "descendants"}
        self.known_control_types = {"Button": "Button", "Edit": "Edit", "TabItem": "TabItem"}
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
    harness = object.__new__(NATIVE_HARNESS)
    harness.uia_element_info_class = factory.element_info
    harness.uia_wrapper_class = factory.wrapper
    harness.iuia_class = lambda: factory.uia
    harness.no_pattern_error_class = FakeNoPatternError
    harness.ui_timeout = timeout
    return harness


def running_app(hwnd: int = 73) -> object:
    return type("Running", (), {"hwnd": hwnd, "process": type("Process", (), {"pid": 91})()})()


class FakeEditorWin32:
    def __init__(self, control: FakeControl | None = None) -> None:
        self.control = control
        self.shortcuts: list[tuple[int, int]] = []
        self.keys: list[tuple[int, int]] = []
        self.unicode_writes: list[tuple[int, str]] = []

    def send_shortcut(self, hwnd: int, key: int) -> None:
        self.shortcuts.append((hwnd, key))

    def send_unicode(self, hwnd: int, value: str) -> None:
        self.unicode_writes.append((hwnd, value))
        if self.control is not None:
            self.control.value = value
            self.control.value_results = None

    def send_key(self, hwnd: int, key: int) -> None:
        self.keys.append((hwnd, key))
        if self.control is not None and key == VK_BACK:
            self.control.value = ""
            self.control.value_results = None


class FakeOwnedWindow:
    def __init__(self, hwnd: int, process_id: int, owner: int, visible: bool, class_name: str) -> None:
        self.hwnd = hwnd
        self.process_id = process_id
        self.owner = owner
        self.visible = visible
        self.class_name = class_name


class FakeLifecycleWin32:
    def __init__(self, windows: list[FakeOwnedWindow]) -> None:
        self.windows = windows
        self.calls: list[tuple[int, int]] = []

    def owned_task_dialogs(self, process_id: int, owner_hwnd: int) -> list[int]:
        self.calls.append((process_id, owner_hwnd))
        return [
            window.hwnd
            for window in self.windows
            if window.process_id == process_id
            and window.owner == owner_hwnd
            and window.visible
            and window.class_name == "#32770"
        ]




class SessionIntegrityAndOutcomeTests(unittest.TestCase):
    def test_requires_same_session_and_integrity(self) -> None:
        parent = SECURITY_CONTEXT(3, 0x2000, "medium")

        self.assertIsNone(
            SECURITY_CONTEXT_FAILURE(parent, SECURITY_CONTEXT(3, 0x2000, "medium"))
        )
        self.assertEqual(
            SECURITY_CONTEXT_FAILURE(parent, SECURITY_CONTEXT(4, 0x2000, "medium")),
            "PROCESS_SESSION_MISMATCH",
        )
        self.assertEqual(
            SECURITY_CONTEXT_FAILURE(parent, SECURITY_CONTEXT(3, 0x3000, "high")),
            "PROCESS_INTEGRITY_MISMATCH",
        )

    def test_parses_blocked_outcome_without_accepting_free_form_text(self) -> None:
        self.assertEqual(
            PARSE_OUTCOME_LINE("BLOCKED: INPUT_DESKTOP_LOCKED"),
            ("BLOCKED", "INPUT_DESKTOP_LOCKED"),
        )
        with self.assertRaisesRegex(ValueError, "invalid harness outcome"):
            PARSE_OUTCOME_LINE("BLOCKED: secret document contents")

    def test_product_timeout_is_fail_not_blocked(self) -> None:
        with self.assertRaises(HARNESS_FAILURE) as raised:
            WAIT_UNTIL(lambda: False, 0.001, "PRODUCT_TIMEOUT", interval=0.0)

        self.assertEqual(raised.exception.code, "PRODUCT_TIMEOUT")
        self.assertNotIsInstance(raised.exception, HARNESS_BLOCKED)

    def test_preflight_failure_and_prerequisite_block_have_distinct_exit_codes(self) -> None:
        args = PARSE_ARGS(["--expect-exe-sha256", HASH])

        def fail(*_args: object) -> object:
            raise HARNESS_FAILURE("WINDOWS_11_REQUIRED")

        def block(*_args: object) -> object:
            raise HARNESS_BLOCKED("INPUT_DESKTOP_LOCKED")

        with mock.patch.dict(RUN.__globals__, {"preflight": fail}):
            fail_code, fail_evidence, _ = RUN(args)
        with mock.patch.dict(RUN.__globals__, {"preflight": block}):
            block_code, block_evidence, _ = RUN(args)

        self.assertEqual((fail_code, fail_evidence["status"]), (1, "FAIL"))
        self.assertEqual((block_code, block_evidence["status"]), (2, "BLOCKED"))
        VALIDATE_EVIDENCE(fail_evidence)
        VALIDATE_EVIDENCE(block_evidence)

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

        with self.assertRaises(HARNESS_BLOCKED) as raised:
            win32.require_foreground(123, timeout=0.0)

        self.assertEqual(raised.exception.code, "FOREGROUND_PERMISSION_DENIED")
        self.assertEqual(
            raised.exception.diagnostics,
            {
                "requested_hwnd": 123,
                "foreground_hwnd": 0,
                "show_window_return": True,
                "bring_to_top_return": False,
                "set_foreground_return": False,
                "foreground_attempts": 0,
            },
        )


class SelectorAndOrchestrationTests(unittest.TestCase):
    def test_ids_match_the_rust_accessibility_contract(self) -> None:
        self.assertEqual(LAYOUT_SOURCE_AUTOMATION_ID, "markturbo-layout-source")
        self.assertEqual(SOURCE_EDITOR_AUTOMATION_ID, "markturbo-document-source-editor")
        self.assertEqual(TAB_CLOSE_AUTOMATION_ID, "markturbo-document-tab-close")
        self.assertEqual(CONFLICT_OVERWRITE_AUTOMATION_ID, "markturbo-conflict-overwrite")

    def test_lifecycle_dialog_uses_owned_taskdialog_and_exact_raw_buttons(self) -> None:
        save = FakeControl("CommandButton_1", "Button", name="Save", class_name="CCPushButton")
        discard = FakeControl("CommandButton_-2", "Button", name="Discard", class_name="CCPushButton")
        cancel = FakeControl("CommandButton_2", "Button", name="Cancel", class_name="CCPushButton")
        system_close = FakeControl("Close", "Button", name="Close", class_name="CCPushButton")
        dialog = FakeRoot([save, discard, cancel, system_close])
        windows = [
            FakeOwnedWindow(80, 91, 72, True, "#32770"),
            FakeOwnedWindow(81, 92, 73, True, "#32770"),
            FakeOwnedWindow(82, 91, 73, False, "#32770"),
            FakeOwnedWindow(83, 91, 73, True, "Other"),
            FakeOwnedWindow(84, 91, 73, True, "#32770"),
        ]
        win32 = FakeLifecycleWin32(windows)
        factory = FreshRootFactory([], {84: dialog})
        harness = native_harness(factory)
        harness.win32 = win32

        buttons = harness.lifecycle_dialog(running_app())
        harness.click_lifecycle_decision(running_app(), "Discard")

        self.assertEqual(win32.calls, [(91, 73), (91, 73)])
        self.assertEqual(set(buttons), {"Save", "Discard", "Cancel"})
        self.assertEqual(discard.click_count, 1)
        self.assertEqual(save.click_count, 0)
        self.assertEqual(cancel.click_count, 0)
        self.assertEqual(
            dialog.find_calls[:3],
            [
                ("descendants", ("automation-id", "CommandButton_1")),
                ("descendants", ("automation-id", "CommandButton_-2")),
                ("descendants", ("automation-id", "CommandButton_2")),
            ],
        )

    def test_lifecycle_dialog_retries_zero_and_rejects_multiple_or_wrong_contract(self) -> None:
        empty_factory = FreshRootFactory([])
        empty_harness = native_harness(empty_factory, timeout=0.001)
        empty_harness.win32 = FakeLifecycleWin32([])
        with self.assertRaises(HARNESS_FAILURE) as empty:
            empty_harness.lifecycle_dialog(running_app())
        self.assertEqual(empty.exception.code, "LIFECYCLE_TASK_DIALOG_TIMEOUT")

        windows = [
            FakeOwnedWindow(84, 91, 73, True, "#32770"),
            FakeOwnedWindow(85, 91, 73, True, "#32770"),
        ]
        multiple_harness = native_harness(FreshRootFactory([]))
        multiple_harness.win32 = FakeLifecycleWin32(windows)
        with self.assertRaises(HARNESS_FAILURE) as multiple:
            multiple_harness.lifecycle_dialog(running_app())
        self.assertEqual(multiple.exception.code, "MULTIPLE_LIFECYCLE_TASK_DIALOGS")

        wrong = FakeRoot(
            [
                FakeControl("CommandButton_1", "Button", name="save", class_name="CCPushButton"),
                FakeControl("CommandButton_-2", "Button", name="Discard", class_name="CCPushButton"),
                FakeControl("CommandButton_2", "Button", name="Cancel", class_name="CCPushButton"),
            ]
        )
        wrong_harness = native_harness(FreshRootFactory([], {84: wrong}))
        wrong_harness.win32 = FakeLifecycleWin32([windows[0]])
        with self.assertRaises(HARNESS_FAILURE) as mismatch:
            wrong_harness.lifecycle_dialog(running_app())
        self.assertEqual(mismatch.exception.code, "LIFECYCLE_BUTTON_CONTRACT_MISMATCH")

    def test_find_control_refreshes_root_after_a_stale_lookup(self) -> None:
        stale = FakeRoot()
        current_control = FakeControl(SOURCE_EDITOR_AUTOMATION_ID, "Edit")
        current = FakeRoot([current_control])
        factory = FreshRootFactory([stale, current])
        harness = native_harness(factory)

        control = harness.find_control(
            running_app(),
            SOURCE_EDITOR_AUTOMATION_ID,
            "Edit",
            "SOURCE_EDITOR_UIA_TIMEOUT",
            "SOURCE_EDITOR_UIA_CONTRACT_MISMATCH",
        )

        self.assertIs(control, factory.wrappers[0])
        self.assertGreaterEqual(factory.calls, 2)
        self.assertEqual(factory.handles, [73] * factory.calls)
        self.assertEqual(
            stale.find_calls + current.find_calls,
            [
                ("descendants", ("automation-id", SOURCE_EDITOR_AUTOMATION_ID)),
                ("descendants", ("automation-id", SOURCE_EDITOR_AUTOMATION_ID)),
            ],
        )
        self.assertEqual(
            factory.uia.conditions,
            [("automation-id", SOURCE_EDITOR_AUTOMATION_ID)] * factory.calls,
        )
        self.assertEqual(current.arrays[0].get_calls, [0])
        self.assertIs(factory.wrappers[0].element_info.element, current_control)

    def test_find_control_uses_target_specific_timeout_without_label_fallback(self) -> None:
        root = FakeRoot([FakeControl("other-id", "Edit")])
        harness = native_harness(FreshRootFactory([root]), timeout=0.001)

        with self.assertRaises(HARNESS_FAILURE) as raised:
            harness.find_control(
                running_app(),
                SOURCE_EDITOR_AUTOMATION_ID,
                "Edit",
                "SOURCE_EDITOR_UIA_TIMEOUT",
                "SOURCE_EDITOR_UIA_CONTRACT_MISMATCH",
            )

        self.assertEqual(raised.exception.code, "SOURCE_EDITOR_UIA_TIMEOUT")
        self.assertEqual(
            root.find_calls[0],
            ("descendants", ("automation-id", SOURCE_EDITOR_AUTOMATION_ID)),
        )

    def test_find_control_fails_immediately_on_id_or_type_mismatch(self) -> None:
        mismatched = FakeRoot([FakeControl(SOURCE_EDITOR_AUTOMATION_ID, "Button")])
        factory = FreshRootFactory([mismatched])
        harness = native_harness(factory)

        with self.assertRaises(HARNESS_FAILURE) as raised:
            harness.find_control(
                running_app(),
                SOURCE_EDITOR_AUTOMATION_ID,
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
                    SOURCE_EDITOR_AUTOMATION_ID,
                    "Edit",
                    property_error=RuntimeError(secret),
                )
            ]
        )
        current_control = FakeControl(SOURCE_EDITOR_AUTOMATION_ID, "Edit")
        factory = FreshRootFactory([stale, FakeRoot([current_control])])
        harness = native_harness(factory)

        control = harness.find_control(
            running_app(),
            SOURCE_EDITOR_AUTOMATION_ID,
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
                    SOURCE_EDITOR_AUTOMATION_ID,
                    "Edit",
                    wrapper_automation_id=None,
                )
            ]
        )
        fresh_control = FakeControl(SOURCE_EDITOR_AUTOMATION_ID, "Edit")
        factory = FreshRootFactory([stale, FakeRoot([fresh_control])])
        harness = native_harness(factory)

        control = harness.find_control(
            running_app(),
            SOURCE_EDITOR_AUTOMATION_ID,
            "Edit",
            "SOURCE_EDITOR_UIA_TIMEOUT",
            "SOURCE_EDITOR_UIA_CONTRACT_MISMATCH",
        )

        self.assertIs(control, factory.wrappers[-1])
        self.assertEqual(factory.calls, 2)
        self.assertIs(factory.wrappers[-1].element_info.element, fresh_control)

    def test_find_control_retries_hidden_and_disabled_controls_until_ready(self) -> None:
        hidden = FakeRoot([FakeControl(SOURCE_EDITOR_AUTOMATION_ID, "Edit", visible=False)])
        disabled = FakeRoot([FakeControl(SOURCE_EDITOR_AUTOMATION_ID, "Edit", enabled=False)])
        ready_control = FakeControl(SOURCE_EDITOR_AUTOMATION_ID, "Edit")
        ready = FakeRoot([ready_control])
        factory = FreshRootFactory([hidden, disabled, ready])
        harness = native_harness(factory)

        control = harness.find_control(
            running_app(),
            SOURCE_EDITOR_AUTOMATION_ID,
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
            FakeRoot([FakeControl(SOURCE_EDITOR_AUTOMATION_ID, "Edit")], get_error=RuntimeError(secret)),
        )
        for root in roots:
            with self.subTest(root=root), self.assertRaises(HARNESS_FAILURE) as raised:
                native_harness(FreshRootFactory([root]), timeout=0.001).find_control(
                    running_app(),
                    SOURCE_EDITOR_AUTOMATION_ID,
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

        with self.assertRaises(HARNESS_FAILURE) as raised:
            harness.control_absent(
                running_app(),
                SOURCE_EDITOR_AUTOMATION_ID,
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
                    SOURCE_EDITOR_AUTOMATION_ID,
                    "Edit",
                    property_error=RuntimeError(secret),
                )
            ]
        )
        harness = native_harness(FreshRootFactory([root]))

        with self.assertRaises(HARNESS_FAILURE) as raised:
            harness.control_absent(
                running_app(),
                SOURCE_EDITOR_AUTOMATION_ID,
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
                    SOURCE_EDITOR_AUTOMATION_ID,
                    "Edit",
                    wrapper_control_type=None,
                )
            ]
        )
        harness = native_harness(FreshRootFactory([root]))

        with self.assertRaises(HARNESS_FAILURE) as raised:
            harness.control_absent(
                running_app(),
                SOURCE_EDITOR_AUTOMATION_ID,
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
                SOURCE_EDITOR_AUTOMATION_ID,
                "Edit",
                "DISCARD_EDITOR_ABSENCE_UIA_QUERY_FAILED",
                "DISCARD_EDITOR_ABSENCE_UIA_CONTRACT_MISMATCH",
            )
        )
        self.assertEqual(
            root.find_calls,
            [("descendants", ("automation-id", SOURCE_EDITOR_AUTOMATION_ID))],
        )

    def test_control_absent_fails_on_a_same_id_wrong_type(self) -> None:
        root = FakeRoot([FakeControl(SOURCE_EDITOR_AUTOMATION_ID, "Button")])
        harness = native_harness(FreshRootFactory([root]))

        with self.assertRaises(HARNESS_FAILURE) as raised:
            harness.control_absent(
                running_app(),
                SOURCE_EDITOR_AUTOMATION_ID,
                "Edit",
                "DISCARD_EDITOR_ABSENCE_UIA_QUERY_FAILED",
                "DISCARD_EDITOR_ABSENCE_UIA_CONTRACT_MISMATCH",
            )

        self.assertEqual(raised.exception.code, "DISCARD_EDITOR_ABSENCE_UIA_CONTRACT_MISMATCH")

    def test_wait_editor_fingerprint_clicks_once_while_reads_retry(self) -> None:
        control = FakeControl(
            SOURCE_EDITOR_AUTOMATION_ID,
            "Edit",
            value_results=[RuntimeError("UNIQUE-DOCUMENT-CONTENT"), "after"],
        )
        harness = native_harness(FreshRootFactory([FakeRoot([control])]))
        win32 = FakeEditorWin32()
        harness.win32 = win32
        expected = FINGERPRINT_TEXT("after")

        def retry_twice(predicate: object, _timeout: float, code: str, **_kwargs: object) -> object:
            assert callable(predicate)
            if code == "SOURCE_EDITOR_UIA_TIMEOUT":
                return predicate()
            self.assertEqual(code, "EDITOR_EXACT_BYTES_TIMEOUT")
            self.assertIsNone(predicate())
            result = predicate()
            self.assertEqual(result, expected)
            return result

        with mock.patch.dict(
            NATIVE_HARNESS.wait_editor_fingerprint.__globals__,
            {"wait_until": retry_twice},
        ):
            actual, _ = harness.wait_editor_fingerprint(running_app(), expected, 1.0)

        self.assertEqual(actual, expected)
        self.assertEqual(control.click_count, 1)
        self.assertEqual(control.value_pattern_count, 2)
        self.assertEqual(win32.shortcuts, [])
        self.assertEqual(win32.unicode_writes, [])

    def test_editor_fingerprint_retries_until_first_readable_value(self) -> None:
        control = FakeControl(
            SOURCE_EDITOR_AUTOMATION_ID,
            "Edit",
            value_results=[RuntimeError("UNIQUE-DOCUMENT-CONTENT"), "fresh"],
        )
        harness = native_harness(FreshRootFactory([FakeRoot([control])]))
        harness.win32 = FakeEditorWin32()

        def retry_twice(predicate: object, _timeout: float, code: str, **_kwargs: object) -> object:
            assert callable(predicate)
            if code == "SOURCE_EDITOR_UIA_TIMEOUT":
                return predicate()
            self.assertEqual(code, "EDITOR_UIA_VALUE_TIMEOUT")
            self.assertIsNone(predicate())
            return predicate()

        with mock.patch.dict(
            NATIVE_HARNESS.wait_editor_fingerprint.__globals__,
            {"wait_until": retry_twice},
        ):
            actual = harness.editor_fingerprint(running_app())

        self.assertEqual(actual, FINGERPRINT_TEXT("fresh"))
        self.assertEqual(control.click_count, 1)
        self.assertEqual(control.value_pattern_count, 2)

    def test_editor_fingerprint_reports_persistent_unreadable_value(self) -> None:
        control = FakeControl(
            SOURCE_EDITOR_AUTOMATION_ID,
            "Edit",
            value=RuntimeError("UNIQUE-DOCUMENT-CONTENT"),
        )
        harness = native_harness(FreshRootFactory([FakeRoot([control])]), timeout=0.001)
        harness.win32 = FakeEditorWin32()

        with self.assertRaises(HARNESS_FAILURE) as raised:
            harness.editor_fingerprint(running_app())

        self.assertEqual(raised.exception.code, "EDITOR_UIA_VALUE_TIMEOUT")
        self.assertEqual(raised.exception.detail, "RuntimeError")

    def test_editor_value_readback_fingerprints_exact_unicode_without_persisting_text(self) -> None:
        for value in ("", "ASCII", "CJK-\u4fdd\u5b58-\U0001f680", "e\u0301"):
            with self.subTest(value=value):
                control = FakeControl(SOURCE_EDITOR_AUTOMATION_ID, "Edit", value=value)
                harness = native_harness(FreshRootFactory([FakeRoot([control])]))

                self.assertEqual(
                    harness.read_editor_fingerprint(running_app()), FINGERPRINT_TEXT(value)
                )
                self.assertEqual(control.value_pattern_count, 1)

    def test_editor_value_contract_mismatch_is_immediate(self) -> None:
        controls = (
            FakeControl(SOURCE_EDITOR_AUTOMATION_ID, "Edit", value_pattern_missing=True),
            FakeControl(SOURCE_EDITOR_AUTOMATION_ID, "Edit", value=7),
        )
        for control in controls:
            with self.subTest(control=control), self.assertRaises(HARNESS_FAILURE) as raised:
                native_harness(FreshRootFactory([FakeRoot([control])])).read_editor_fingerprint(
                    running_app()
                )
            self.assertEqual(raised.exception.code, "EDITOR_UIA_VALUE_CONTRACT_MISMATCH")

    def test_editor_value_timeouts_distinguish_wrong_and_unreadable_values(self) -> None:
        wrong = FakeControl(SOURCE_EDITOR_AUTOMATION_ID, "Edit", value="wrong")
        wrong_harness = native_harness(FreshRootFactory([FakeRoot([wrong])]), timeout=0.001)
        wrong_harness.win32 = FakeEditorWin32()
        with self.assertRaises(HARNESS_FAILURE) as wrong_timeout:
            wrong_harness.wait_editor_fingerprint(running_app(), FINGERPRINT_TEXT("expected"), 0.001)
        self.assertEqual(wrong_timeout.exception.code, "EDITOR_EXACT_BYTES_TIMEOUT")

        unreadable = FakeControl(
            SOURCE_EDITOR_AUTOMATION_ID,
            "Edit",
            value=RuntimeError("UNIQUE-DOCUMENT-CONTENT"),
        )
        unreadable_harness = native_harness(
            FreshRootFactory([FakeRoot([unreadable])]), timeout=0.001
        )
        unreadable_harness.win32 = FakeEditorWin32()
        with self.assertRaises(HARNESS_FAILURE) as unreadable_timeout:
            unreadable_harness.wait_editor_fingerprint(
                running_app(), FINGERPRINT_TEXT("expected"), 0.001
            )
        self.assertEqual(unreadable_timeout.exception.code, "EDITOR_UIA_VALUE_TIMEOUT")
        self.assertEqual(unreadable_timeout.exception.detail, "RuntimeError")

    def test_replace_editor_writes_with_one_click_ctrl_a_and_unicode_input(self) -> None:
        value = "CJK-\u4fdd\u5b58-\U0001f680"
        control = FakeControl(SOURCE_EDITOR_AUTOMATION_ID, "Edit", value="before")
        harness = native_harness(FreshRootFactory([FakeRoot([control])]))
        win32 = FakeEditorWin32(control)
        harness.win32 = win32

        self.assertEqual(harness.replace_editor(running_app(), value), FINGERPRINT_TEXT(value))
        self.assertEqual(control.click_count, 1)
        self.assertEqual(win32.shortcuts, [(73, VK_A)])
        self.assertEqual(win32.unicode_writes, [(73, value)])

    def test_replace_editor_clears_nonempty_value_with_backspace(self) -> None:
        control = FakeControl(SOURCE_EDITOR_AUTOMATION_ID, "Edit", value="before")
        harness = native_harness(FreshRootFactory([FakeRoot([control])]))
        win32 = FakeEditorWin32(control)
        harness.win32 = win32

        self.assertEqual(harness.replace_editor(running_app(), ""), FINGERPRINT_TEXT(""))
        self.assertEqual(control.value, "")
        self.assertEqual(control.click_count, 1)
        self.assertEqual(win32.shortcuts, [(73, VK_A)])
        self.assertEqual(win32.keys, [(73, VK_BACK)])
        self.assertEqual(win32.unicode_writes, [])

    def test_click_control_never_retries_a_failed_input(self) -> None:
        control = FakeControl(
            SOURCE_EDITOR_AUTOMATION_ID,
            "Edit",
            click_error=RuntimeError("UNIQUE-DOCUMENT-CONTENT"),
        )
        harness = object.__new__(NATIVE_HARNESS)

        with self.assertRaises(HARNESS_FAILURE) as raised:
            harness.click_control(
                FakeUIAWrapper(FakeElementInfo(control)), "EDITOR_POINTER_FOCUS_FAILED"
            )

        self.assertEqual(raised.exception.code, "EDITOR_POINTER_FOCUS_FAILED")
        self.assertEqual(raised.exception.detail, "RuntimeError")
        self.assertEqual(control.click_count, 1)

    def test_external_conflict_waits_for_watcher_before_explicit_overwrite(self) -> None:
        source = SCRIPT.read_text(encoding="utf-8")
        body = source.split("    def scenario_external_conflict", 1)[1].split(
            "    def scenario_recovery", 1
        )[0]

        watcher = body.index("CONFLICT_OVERWRITE_AUTOMATION_ID")
        explicit = body.index('click_control(overwrite, "CONFLICT_OVERWRITE_CLICK_FAILED")')
        self.assertLess(watcher, explicit)
        self.assertNotIn("VK_S", body[:explicit])

    def test_launch_uses_the_configured_foreground_timeout(self) -> None:
        events: list[tuple[object, ...]] = []
        context = runtime.SecurityContext(1, 0x2000, "medium")

        class FakeProcess:
            pid = 91

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
            with mock.patch.object(runtime.subprocess, "Popen", return_value=process) as popen:
                app = harness.launch_app(None, data, config, workspace, stderr)

        self.assertEqual(app.spec.args, (str(root / "markturbo.exe"),))
        self.assertEqual(events[-1], ("foreground", 73, 3.5))
        self.assertEqual(popen.call_args.args[0], app.spec.args)

    def test_recovery_waits_for_success_log_not_record_existence(self) -> None:
        source = SCRIPT.read_text(encoding="utf-8")
        body = source.split("    def scenario_recovery", 1)[1].split("\n\ndef native_run_plan", 1)[0]

        self.assertIn("wait_checkpoint_log", body)
        self.assertNotIn("glob(", body)
        self.assertNotIn("wait_recovery_record", source)
        checkpoint = body.index("wait_checkpoint_log")
        live_records = body.index("scan_live_recovery_records", checkpoint)
        terminate = body.index("self.terminate(first)")
        live_runtime = body.index("scan_runtime_artifacts", terminate)
        second = body.index("second = self.launch")
        self.assertLess(checkpoint, live_records)
        self.assertLess(live_records, terminate)
        self.assertLess(terminate, live_runtime)
        self.assertLess(live_runtime, second)

        third = body.split("third = self.launch", 1)[1]
        startup = third.index("wait_recovery_startup_finished")
        observe = third.index("wait_editor_fingerprint")
        self.assertLess(startup, observe)
