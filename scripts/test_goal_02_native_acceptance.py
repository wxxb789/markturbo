#!/usr/bin/env -S uv run --script
# /// script
# requires-python = ">=3.11"
# dependencies = []
# ///
"""Unit tests for goal-02-native-acceptance.py without launching a UI."""

from __future__ import annotations

import copy
import hashlib
import json
import runpy
import struct
import sys
import tempfile
import unittest
from pathlib import Path
from unittest import mock


SCRIPT = Path(__file__).with_name("goal-02-native-acceptance.py")
PYWINAUTO_WAS_LOADED = "pywinauto" in sys.modules
HARNESS = runpy.run_path(SCRIPT)

BUILD_CLI_COMMAND = HARNESS["build_cli_command"]
BUILD_LAUNCH_SPEC = HARNESS["build_launch_spec"]
CHECKPOINT_SUCCESS_PRESENT = HARNESS["checkpoint_success_present"]
COMPLETE_EVIDENCE = HARNESS["complete_evidence"]
DOCUMENT_SENTINEL = HARNESS["DOCUMENT_SENTINEL"]
EXECUTABLE_HASH_FAILURE = HARNESS["executable_hash_failure"]
FINGERPRINT_TEXT = HARNESS["fingerprint_text"]
HARNESS_BLOCKED = HARNESS["HarnessBlocked"]
HARNESS_FAILURE = HARNESS["HarnessFailure"]
INSPECT_PE_BYTES = HARNESS["inspect_pe_bytes"]
LAYOUT_SOURCE_AUTOMATION_ID = HARNESS["LAYOUT_SOURCE_AUTOMATION_ID"]
NATIVE_HARNESS = HARNESS["NativeHarness"]
NEW_EVIDENCE = HARNESS["new_evidence"]
NORMALIZE_EXPECTED_HASH = HARNESS["normalize_expected_hash"]
PARSE_ARGS = HARNESS["parse_args"]
PARSE_OUTCOME_LINE = HARNESS["parse_outcome_line"]
PLATFORM_PREFLIGHT_FAILURE = HARNESS["platform_preflight_failure"]
REQUIRED_CASE_IDS = HARNESS["REQUIRED_CASE_IDS"]
RUN = HARNESS["run"]
RUNTIME_ARTIFACT_SCAN = HARNESS["scan_runtime_artifacts"]
LIVE_RECOVERY_SCAN = HARNESS["scan_live_recovery_records"]
RECOVERY_STARTUP_FINISHED_PRESENT = HARNESS["recovery_startup_finished_present"]
SECURITY_CONTEXT = HARNESS["SecurityContext"]
SECURITY_CONTEXT_FAILURE = HARNESS["security_context_failure"]
SAFE_EXCEPTION_NAME = HARNESS["safe_exception_name"]
SAFE_FAILURE_TYPE = HARNESS["safe_failure_type"]
VALIDATE_EVIDENCE = HARNESS["validate_evidence"]
WAIT_UNTIL = HARNESS["wait_until"]
SOURCE_EDITOR_AUTOMATION_ID = HARNESS["SOURCE_EDITOR_AUTOMATION_ID"]
TAB_CLOSE_AUTOMATION_ID = HARNESS["TAB_CLOSE_AUTOMATION_ID"]
CONFLICT_OVERWRITE_AUTOMATION_ID = HARNESS["CONFLICT_OVERWRITE_AUTOMATION_ID"]
VK_A = HARNESS["VK_A"]
VK_BACK = HARNESS["VK_BACK"]

HASH = "a" * 64
SAME_AS_RAW = object()


def valid_evidence() -> dict:
    evidence = NEW_EVIDENCE(HASH)
    evidence["executable"].update(
        {
            "sha256": HASH,
            "byte_count": 1234,
            "hash_verified": True,
            "copied_sha256": HASH,
            "copy_hash_verified": True,
            "format": "PE32+",
            "machine": "x86_64",
            "machine_code": 0x8664,
            "optional_magic": 0x20B,
        }
    )
    evidence["environment"] = {
        "platform": "Windows 11",
        "windows_major": 10,
        "windows_build": 22631,
        "architecture": "x86_64",
        "native_machine_code": 0x8664,
        "python_pointer_bits": 64,
        "wts_state": "WTSActive",
        "input_desktop": "Default",
        "thread_desktop": "Default",
        "harness_process": {
            "session_id": 1,
            "integrity_rid": 0x2000,
            "integrity": "medium",
        },
    }
    process_context = {"session_id": 1, "integrity_rid": 0x2000, "integrity": "medium"}
    runtime_scan = {
        "files_scanned": 4,
        "app_logs_scanned": 1,
        "recovery_artifacts_scanned": 2,
        "canonical_recovery_records_scanned": 1,
        "recovery_leases_scanned": 1,
        "utf8_sentinel_absent": True,
        "utf16le_sentinel_absent": True,
        "panic_absent": True,
        "refcell_absent": True,
    }

    def fp(value: bytes) -> dict[str, int | str]:
        return {"byte_count": len(value), "sha256": hashlib.sha256(value).hexdigest()}

    original = fp(b"orig")
    edited = fp(b"edit")
    external = fp(b"swap")
    canonical_record = fp(b"ciphertext")
    live_recovery_scan = {
        "canonical_record_count": 1,
        "canonical_records": [canonical_record],
        "utf8_sentinel_absent": True,
        "utf16le_sentinel_absent": True,
    }
    observations = {
        REQUIRED_CASE_IDS[0]: {
            "editor": edited,
            "source_before": original,
            "source_after_cancel_save": edited,
            "cancel_kept_tab": True,
            "flow": "pointer tab-close -> Cancel -> SendInput Ctrl+S",
            "process_context": process_context,
            "foreground_verified": True,
            "runtime_scan": runtime_scan,
        },
        REQUIRED_CASE_IDS[1]: {
            "discarded_editor": edited,
            "source_before": original,
            "source_after_discard": original,
            "discard_closed_tab": True,
            "flow": "SendInput Ctrl+W -> Discard",
            "process_context": process_context,
            "foreground_verified": True,
            "runtime_scan": runtime_scan,
        },
        REQUIRED_CASE_IDS[2]: {
            "editor": edited,
            "saved_source": edited,
            "flow": "WM_CLOSE -> Save",
            "window_exited": True,
            "process_context": process_context,
            "foreground_verified": True,
            "runtime_scan": runtime_scan,
        },
        REQUIRED_CASE_IDS[3]: {
            "loaded_source": original,
            "external_source": external,
            "dirty_editor": edited,
            "saved_after_explicit_overwrite": edited,
            "same_length": True,
            "same_mtime_ns": True,
            "watcher_conflict_before_save": True,
            "explicit_overwrite_used": True,
            "flow": "watcher conflict -> explicit Overwrite",
            "process_context": process_context,
            "foreground_verified": True,
            "runtime_scan": runtime_scan,
        },
        REQUIRED_CASE_IDS[4]: {
            "edited_text": edited,
            "checkpoint_log_seen": True,
            "checkpoint_signal": "content-free checkpoint success log",
            "live_runtime_scan": runtime_scan,
            "live_recovery_scan": live_recovery_scan,
            "edit_to_signal_ms": 9999.0,
            "restored_editor": edited,
            "recovery_restored_exact": True,
            "restore_ms": 3.0,
            "source_after_recovery_discard": original,
            "restart_editor": original,
            "discarded_recovery_absent": True,
            "discard_restart_absent_ms": 4.0,
            "termination": "TerminateProcess",
            "restart_count": 2,
            "startup_finished_log_seen": True,
            "startup_signal": "content-free recovery startup finished log",
            "startup_observed_before_restart_editor": True,
            "flow": (
                "edit -> checkpoint log -> TerminateProcess -> restart -> Discard -> "
                "restart after startup log"
            ),
            "process_contexts": [process_context, process_context, process_context],
            "foreground_verified": True,
            "runtime_scan": runtime_scan,
        },
    }
    for case in evidence["cases"]:
        case["status"] = "PASS"
        case["duration_ms"] = 1.25
        case["observations"] = observations[case["id"]]
    COMPLETE_EVIDENCE(evidence, "PASS")
    return evidence


def minimal_pe(machine: int = 0x8664, magic: int = 0x20B) -> bytes:
    data = bytearray(512)
    data[:2] = b"MZ"
    struct.pack_into("<I", data, 0x3C, 0x80)
    data[0x80:0x84] = b"PE\0\0"
    struct.pack_into("<H", data, 0x84, machine)
    struct.pack_into("<H", data, 0x94, 0xF0)
    struct.pack_into("<H", data, 0x98, magic)
    return bytes(data)


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


class EvidenceSchemaTests(unittest.TestCase):
    def test_accepts_complete_versioned_evidence(self) -> None:
        evidence = valid_evidence()

        VALIDATE_EVIDENCE(evidence)

        self.assertEqual(evidence["schema_version"], 1)
        self.assertEqual([case["id"] for case in evidence["cases"]], list(REQUIRED_CASE_IDS))

    def test_rejects_missing_required_case(self) -> None:
        evidence = valid_evidence()
        evidence["cases"].pop()
        COMPLETE_EVIDENCE(evidence, "PASS")

        with self.assertRaisesRegex(ValueError, "required case set is incomplete"):
            VALIDATE_EVIDENCE(evidence)

    def test_rejects_duplicate_required_case(self) -> None:
        evidence = valid_evidence()
        evidence["cases"][-1] = copy.deepcopy(evidence["cases"][0])
        COMPLETE_EVIDENCE(evidence, "PASS")

        with self.assertRaisesRegex(ValueError, "duplicate required case ids"):
            VALIDATE_EVIDENCE(evidence)

    def test_rejects_invalid_observation_hash(self) -> None:
        evidence = valid_evidence()
        evidence["cases"][0]["observations"]["editor"]["sha256"] = "short"

        with self.assertRaisesRegex(ValueError, "invalid observation SHA-256"):
            VALIDATE_EVIDENCE(evidence)

    def test_failed_case_accepts_legacy_missing_or_safe_exception_type(self) -> None:
        evidence = valid_evidence()
        failed = evidence["cases"][0]
        failed["status"] = "FAIL"
        failed["reason_code"] = "SOURCE_EDITOR_UIA_TIMEOUT"
        failed.pop("failure_type")
        COMPLETE_EVIDENCE(evidence, "FAIL")

        VALIDATE_EVIDENCE(evidence)

        failed["failure_type"] = "HarnessFailure"
        VALIDATE_EVIDENCE(evidence)

        failed["failure_type"] = "UNIQUE_SECRET"
        with self.assertRaisesRegex(ValueError, "invalid case failure type"):
            VALIDATE_EVIDENCE(evidence)

    def test_pass_requires_matching_verified_executable_hash(self) -> None:
        evidence = valid_evidence()
        evidence["executable"]["sha256"] = "b" * 64

        with self.assertRaisesRegex(ValueError, "does not match expected"):
            VALIDATE_EVIDENCE(evidence)

    def test_pass_requires_environment_and_matching_process_context(self) -> None:
        evidence = valid_evidence()
        evidence["environment"].pop("harness_process")
        with self.assertRaisesRegex(ValueError, "missing process context"):
            VALIDATE_EVIDENCE(evidence)

        evidence = valid_evidence()
        evidence["cases"][0]["observations"]["process_context"]["session_id"] = 2
        with self.assertRaisesRegex(ValueError, "differs from harness context"):
            VALIDATE_EVIDENCE(evidence)

    def test_pass_requires_true_case_facts_and_runtime_scan(self) -> None:
        evidence = valid_evidence()
        evidence["cases"][3]["observations"]["watcher_conflict_before_save"] = False
        with self.assertRaisesRegex(ValueError, "requires true watcher_conflict_before_save"):
            VALIDATE_EVIDENCE(evidence)

        evidence = valid_evidence()
        evidence["cases"][0]["observations"]["runtime_scan"][
            "utf8_sentinel_absent"
        ] = False
        with self.assertRaisesRegex(ValueError, "requires true utf8_sentinel_absent"):
            VALIDATE_EVIDENCE(evidence)

    def test_pass_rejects_signal_over_budget_and_negative_duration(self) -> None:
        evidence = valid_evidence()
        evidence["cases"][4]["observations"]["edit_to_signal_ms"] = 10_000.001
        with self.assertRaisesRegex(ValueError, "exceeds 10000ms"):
            VALIDATE_EVIDENCE(evidence)

        evidence = valid_evidence()
        evidence["cases"][2]["duration_ms"] = -0.1
        with self.assertRaisesRegex(ValueError, "duration must be nonnegative"):
            VALIDATE_EVIDENCE(evidence)

        evidence = valid_evidence()
        evidence["cases"][2]["duration_ms"] = float("nan")
        with self.assertRaisesRegex(ValueError, "duration must be nonnegative"):
            VALIDATE_EVIDENCE(evidence)

    def test_pass_rejects_case_specific_hash_mismatch(self) -> None:
        evidence = valid_evidence()
        evidence["cases"][0]["observations"]["source_after_cancel_save"] = {
            "byte_count": 4,
            "sha256": "b" * 64,
        }

        with self.assertRaisesRegex(ValueError, "Cancel did not preserve exact"):
            VALIDATE_EVIDENCE(evidence)

    def test_pass_rejects_contradictory_windows_and_process_evidence(self) -> None:
        mutations = (
            ("windows_build", 21999, "Windows 11 build 22000"),
            ("native_machine_code", 0xAA64, "x64 OS and Python"),
            ("python_pointer_bits", 32, "x64 OS and Python"),
            ("wts_state", "WTSDisconnected", "WTSActive"),
            ("input_desktop", "Winlogon", "unlocked Default input desktop"),
        )
        for key, value, message in mutations:
            with self.subTest(key=key):
                evidence = valid_evidence()
                evidence["environment"][key] = value
                with self.assertRaisesRegex(ValueError, message):
                    VALIDATE_EVIDENCE(evidence)

    def test_pass_rejects_contradictory_pe_evidence(self) -> None:
        evidence = valid_evidence()
        evidence["executable"]["machine_code"] = 0x14C

        with self.assertRaisesRegex(ValueError, "AMD64 PE32"):
            VALIDATE_EVIDENCE(evidence)

    def test_pass_rejects_contradictory_flow_mechanics(self) -> None:
        evidence = valid_evidence()
        evidence["cases"][1]["observations"]["flow"] = "WM_CLOSE -> Save"
        with self.assertRaisesRegex(ValueError, "flow mechanics"):
            VALIDATE_EVIDENCE(evidence)

        evidence = valid_evidence()
        recovery = evidence["cases"][4]["observations"]
        recovery["restart_count"] = 1
        with self.assertRaisesRegex(ValueError, "requires two restarts"):
            VALIDATE_EVIDENCE(evidence)

    def test_pass_requires_startup_barrier_and_live_canonical_record(self) -> None:
        evidence = valid_evidence()
        recovery = evidence["cases"][4]["observations"]
        recovery["startup_observed_before_restart_editor"] = False
        with self.assertRaisesRegex(ValueError, "startup_observed_before_restart_editor"):
            VALIDATE_EVIDENCE(evidence)

        evidence = valid_evidence()
        live = evidence["cases"][4]["observations"]["live_recovery_scan"]
        live["canonical_record_count"] = 0
        live["canonical_records"] = []
        with self.assertRaisesRegex(ValueError, "requires a canonical record"):
            VALIDATE_EVIDENCE(evidence)

        evidence = valid_evidence()
        recovery = evidence["cases"][4]["observations"]
        recovery["live_runtime_scan"]["canonical_recovery_records_scanned"] = 0
        with self.assertRaisesRegex(ValueError, "omitted canonical recovery records"):
            VALIDATE_EVIDENCE(evidence)

        evidence = valid_evidence()
        recovery = evidence["cases"][4]["observations"]
        recovery["live_runtime_scan"]["recovery_leases_scanned"] = 0
        with self.assertRaisesRegex(ValueError, "omitted recovery lease"):
            VALIDATE_EVIDENCE(evidence)


class PlatformAndExecutableTests(unittest.TestCase):
    def test_accepts_windows_11_x64_only(self) -> None:
        self.assertIsNone(PLATFORM_PREFLIGHT_FAILURE("Windows", 10, 22631, "AMD64", 64))
        self.assertEqual(
            PLATFORM_PREFLIGHT_FAILURE("Windows", 10, 19045, "AMD64", 64),
            "WINDOWS_11_REQUIRED",
        )
        self.assertEqual(
            PLATFORM_PREFLIGHT_FAILURE("Linux", 6, 0, "x86_64", 64),
            "WINDOWS_REQUIRED",
        )
        self.assertEqual(
            PLATFORM_PREFLIGHT_FAILURE("Windows", 10, 22631, "ARM64", 64),
            "WINDOWS_X64_REQUIRED",
        )

    def test_parses_amd64_pe32_plus(self) -> None:
        result = INSPECT_PE_BYTES(minimal_pe())

        self.assertEqual(result["machine"], "x86_64")
        self.assertEqual(result["format"], "PE32+")

    def test_rejects_non_x64_pe(self) -> None:
        with self.assertRaisesRegex(ValueError, "not AMD64 PE32"):
            INSPECT_PE_BYTES(minimal_pe(machine=0x14C, magic=0x10B))

    def test_hash_comparison_fails_closed(self) -> None:
        self.assertIsNone(EXECUTABLE_HASH_FAILURE(HASH, HASH))
        self.assertEqual(
            EXECUTABLE_HASH_FAILURE("b" * 64, HASH), "EXECUTABLE_HASH_MISMATCH"
        )
        self.assertEqual(EXECUTABLE_HASH_FAILURE("short", HASH), "EXECUTABLE_HASH_INVALID")


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

    def test_recovery_waits_for_success_log_not_record_existence(self) -> None:
        source = SCRIPT.read_text(encoding="utf-8")
        body = source.split("    def scenario_recovery", 1)[1].split("\n\ndef preflight", 1)[0]

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


class RuntimeEvidenceTests(unittest.TestCase):
    def test_checkpoint_log_parser_requires_written_marker_after_offset(self) -> None:
        marker = b"DEBUG recovery checkpoint written\n"
        self.assertTrue(CHECKPOINT_SUCCESS_PRESENT(marker))
        self.assertFalse(CHECKPOINT_SUCCESS_PRESENT(b"recovery checkpoint failed; durable=false\n"))
        self.assertFalse(CHECKPOINT_SUCCESS_PRESENT(b"recovery checkpoint written but failed\n"))
        self.assertFalse(CHECKPOINT_SUCCESS_PRESENT(marker + b"other\n", len(marker)))

    def test_startup_log_parser_requires_finished_marker_after_offset(self) -> None:
        marker = b"DEBUG recovery startup finished\n"
        self.assertTrue(RECOVERY_STARTUP_FINISHED_PRESENT(marker))
        self.assertFalse(RECOVERY_STARTUP_FINISHED_PRESENT(b"recovery startup began\n"))
        self.assertFalse(RECOVERY_STARTUP_FINISHED_PRESENT(marker + b"other\n", len(marker)))

    def test_live_recovery_scan_requires_canonical_encrypted_record(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            data = Path(temporary) / "data"
            recovery = data / "recovery"
            recovery.mkdir(parents=True)
            (recovery / "junk.mtrecovery").write_bytes(b"ignored")
            with self.assertRaises(HARNESS_FAILURE) as missing:
                LIVE_RECOVERY_SCAN(data)
            self.assertEqual(missing.exception.code, "CANONICAL_RECOVERY_RECORD_MISSING")

            canonical = recovery / (("a" * 64) + ".mtrecovery")
            canonical.write_bytes(b"")
            with self.assertRaises(HARNESS_FAILURE) as empty:
                LIVE_RECOVERY_SCAN(data)
            self.assertEqual(empty.exception.code, "CANONICAL_RECOVERY_RECORD_EMPTY")

            canonical.write_bytes(b"encrypted-record")
            result = LIVE_RECOVERY_SCAN(data)
            self.assertEqual(result["canonical_record_count"], 1)
            self.assertEqual(len(result["canonical_records"]), 1)

            canonical.write_bytes(DOCUMENT_SENTINEL.encode("utf-8"))
            with self.assertRaises(HARNESS_FAILURE) as leaked:
                LIVE_RECOVERY_SCAN(data)
            self.assertEqual(leaked.exception.code, "UTF8_DOCUMENT_SENTINEL_LEAKED")

    def test_runtime_scan_covers_stderr_app_logs_and_recovery_artifacts(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            data = root / "data"
            logs = data / "logs"
            recovery = data / "recovery"
            logs.mkdir(parents=True)
            recovery.mkdir()
            stderr = root / "stderr.log"
            stderr.write_bytes(b"")
            (logs / "markturbo-1.log").write_bytes(b"startup ok\n")
            (recovery / (("a" * 64) + ".mtrecovery")).write_bytes(b"ciphertext")
            (recovery / ".markturbo-recovery.lock").write_bytes(b"lease")

            result = RUNTIME_ARTIFACT_SCAN(data, stderr)

            self.assertEqual(result["files_scanned"], 4)
            self.assertEqual(result["app_logs_scanned"], 1)
            self.assertEqual(result["recovery_artifacts_scanned"], 2)
            self.assertEqual(result["canonical_recovery_records_scanned"], 1)
            self.assertEqual(result["recovery_leases_scanned"], 1)

    def test_runtime_scan_counts_records_and_leases_separately(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            data = root / "data"
            logs = data / "logs"
            recovery = data / "recovery"
            logs.mkdir(parents=True)
            recovery.mkdir()
            stderr = root / "stderr.log"
            stderr.write_bytes(b"")
            (logs / "markturbo-1.log").write_bytes(b"startup ok\n")

            (recovery / ".markturbo-recovery.lock").write_bytes(b"lease")
            lease_only = RUNTIME_ARTIFACT_SCAN(data, stderr)
            self.assertEqual(lease_only["canonical_recovery_records_scanned"], 0)
            self.assertEqual(lease_only["recovery_leases_scanned"], 1)

            (recovery / ".markturbo-recovery.lock").unlink()
            (recovery / (("a" * 64) + ".mtrecovery")).write_bytes(b"ciphertext")
            record_only = RUNTIME_ARTIFACT_SCAN(data, stderr)
            self.assertEqual(record_only["canonical_recovery_records_scanned"], 1)
            self.assertEqual(record_only["recovery_leases_scanned"], 0)

    def test_recovery_scans_fail_closed_on_permission_error(self) -> None:
        secret = "UNIQUE-DOCUMENT-CONTENT"
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            data = root / "data"
            logs = data / "logs"
            recovery = data / "recovery"
            logs.mkdir(parents=True)
            recovery.mkdir()
            stderr = root / "stderr.log"
            stderr.write_bytes(b"")
            (logs / "markturbo-1.log").write_bytes(b"startup ok\n")
            record = recovery / (("a" * 64) + ".mtrecovery")
            record.write_bytes(b"ciphertext")
            original_read_bytes = Path.read_bytes

            def denied(path: Path) -> bytes:
                if path == record:
                    raise PermissionError(secret)
                return original_read_bytes(path)

            with mock.patch.object(Path, "read_bytes", new=denied):
                with self.assertRaises(HARNESS_FAILURE) as live:
                    LIVE_RECOVERY_SCAN(data)
            self.assertEqual(live.exception.code, "LIVE_RECOVERY_RECORD_SCAN_FAILED")
            self.assertEqual(live.exception.detail, "PermissionError")

            with mock.patch.object(Path, "read_bytes", new=denied):
                with self.assertRaises(HARNESS_FAILURE) as runtime:
                    RUNTIME_ARTIFACT_SCAN(data, stderr)
            self.assertEqual(runtime.exception.code, "RUNTIME_ARTIFACT_SCAN_FAILED")
            self.assertEqual(runtime.exception.detail, "PermissionError")
            self.assertNotIn(secret, runtime.exception.detail)

    def test_runtime_scan_rejects_utf8_utf16_panic_and_refcell(self) -> None:
        payloads = (
            (DOCUMENT_SENTINEL.encode("utf-8"), "UTF8_DOCUMENT_SENTINEL_LEAKED"),
            (DOCUMENT_SENTINEL.encode("utf-16-le"), "UTF16LE_DOCUMENT_SENTINEL_LEAKED"),
            (b"thread panicked at source", "PANIC_LOGGED"),
            (b"RefCell already borrowed", "REFCELL_BORROW_PANIC_LOGGED"),
        )
        for payload, code in payloads:
            with self.subTest(code=code), tempfile.TemporaryDirectory() as temporary:
                root = Path(temporary)
                data = root / "data"
                logs = data / "logs"
                logs.mkdir(parents=True)
                stderr = root / "stderr.log"
                stderr.write_bytes(b"")
                (logs / "markturbo-1.log").write_bytes(payload)

                with self.assertRaises(HARNESS_FAILURE) as raised:
                    RUNTIME_ARTIFACT_SCAN(data, stderr)
                self.assertEqual(raised.exception.code, code)

    def test_runtime_scan_requires_app_log_and_scans_recovery_payload(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            data = root / "data"
            stderr = root / "stderr.log"
            stderr.write_bytes(b"")
            with self.assertRaises(HARNESS_FAILURE) as missing:
                RUNTIME_ARTIFACT_SCAN(data, stderr)
            self.assertEqual(missing.exception.code, "APP_LOG_MISSING")

            logs = data / "logs"
            recovery = data / "recovery"
            logs.mkdir(parents=True)
            recovery.mkdir()
            (logs / "markturbo-1.log").write_bytes(b"startup ok")
            (recovery / "record.mtrecovery").write_bytes(
                DOCUMENT_SENTINEL.encode("utf-16-le")
            )
            with self.assertRaises(HARNESS_FAILURE) as leaked:
                RUNTIME_ARTIFACT_SCAN(data, stderr)
            self.assertEqual(leaked.exception.code, "UTF16LE_DOCUMENT_SENTINEL_LEAKED")

    def test_cleanup_failure_is_a_product_failure(self) -> None:
        class BadProcess:
            def poll(self) -> None:
                return None

            def kill(self) -> None:
                raise OSError("cannot kill")

        harness = object.__new__(NATIVE_HARNESS)
        harness.processes = [BadProcess()]

        with self.assertRaises(HARNESS_FAILURE) as raised:
            harness.cleanup()

        self.assertEqual(raised.exception.code, "CLEANUP_REAP_FAILED")


class PrivacyAndCliTests(unittest.TestCase):
    def test_loading_parser_does_not_import_pywinauto(self) -> None:
        if not PYWINAUTO_WAS_LOADED:
            self.assertNotIn("pywinauto", sys.modules)

    def test_editor_readback_does_not_use_clipboard_or_setvalue(self) -> None:
        source = SCRIPT.read_text(encoding="utf-8")
        forbidden_apis = (
            "Clipboard",
            "CF_UNICODETEXT",
            "GMEM_",
            "Global",
            "SetValue",
        )
        for forbidden in forbidden_apis:
            with self.subTest(forbidden=forbidden):
                self.assertNotIn(forbidden, source)
                self.assertIn(forbidden, f"legacy call: {forbidden}")

    def test_document_text_never_appears_in_serialized_observation(self) -> None:
        secret = "UNIQUE-SECRET-\u4fdd\u5b58-\U0001f680"
        evidence = valid_evidence()
        evidence["cases"][0]["observations"]["editor"] = FINGERPRINT_TEXT(
            secret
        ).evidence()
        serialized = json.dumps(evidence, ensure_ascii=False)

        self.assertNotIn(secret, serialized)
        self.assertNotIn("UNIQUE-SECRET", serialized)
        self.assertIn(hashlib.sha256(secret.encode("utf-8")).hexdigest(), serialized)

        evidence["cases"][0]["observations"]["raw_text"] = secret
        with self.assertRaisesRegex(ValueError, "unknown observation field"):
            VALIDATE_EVIDENCE(evidence)

    def test_exception_sanitizer_never_returns_exception_text(self) -> None:
        secret = "UNIQUE-DOCUMENT-CONTENT"

        result = SAFE_EXCEPTION_NAME(RuntimeError(secret))

        self.assertEqual(result, "RuntimeError")
        self.assertNotIn(secret, result)

    def test_failure_type_prefers_safe_detail_and_rejects_unsafe_detail(self) -> None:
        for name in ("PermissionError", "TypeError", "COMError"):
            with self.subTest(name=name):
                self.assertEqual(
                    SAFE_FAILURE_TYPE(HARNESS_FAILURE("RUNTIME_ARTIFACT_SCAN_FAILED", name)),
                    name,
                )
        for detail in ("UNIQUE_SECRET", "secret text", "secret-text"):
            with self.subTest(detail=detail):
                self.assertEqual(
                    SAFE_FAILURE_TYPE(HARNESS_FAILURE("RUNTIME_ARTIFACT_SCAN_FAILED", detail)),
                    "HarnessFailure",
                )

    def test_constructs_isolated_absolute_launch_without_starting_process(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary).resolve()
            exe = root / "bin" / "markturbo.exe"
            target = root / "workspace" / "case.md"
            data = root / "data"
            config = root / "config"
            workspace = root / "workspace"
            stderr = root / "stderr.log"
            with mock.patch("subprocess.Popen") as popen:
                spec = BUILD_LAUNCH_SPEC(
                    exe,
                    target,
                    data,
                    config,
                    workspace,
                    stderr,
                    {
                        "OPENAI_API_KEY": "must-not-propagate",
                        "ANTHROPIC_API_KEY": "must-not-propagate",
                        "PATH": "path",
                    },
                )

            popen.assert_not_called()
            self.assertEqual(spec.args, (str(exe), str(target)))
            self.assertEqual(spec.cwd, str(workspace))
            self.assertEqual(spec.env["MARKTURBO_DATA_DIR"], str(data))
            self.assertEqual(spec.env["MARKTURBO_CONFIG_DIR"], str(config))
            self.assertEqual(spec.env["RUST_LOG"], "debug")
            self.assertNotIn("OPENAI_API_KEY", spec.env)
            self.assertNotIn("ANTHROPIC_API_KEY", spec.env)

    def test_constructs_cli_without_starting_process(self) -> None:
        exe = Path("C:/release/markturbo.exe")
        evidence = Path("C:/evidence/goal-02.json")
        with mock.patch("subprocess.Popen") as popen:
            command = BUILD_CLI_COMMAND(exe, HASH, evidence)
            args = PARSE_ARGS(command[2:])

        popen.assert_not_called()
        self.assertEqual(args.exe, exe)
        self.assertEqual(args.expect_exe_sha256, HASH)
        self.assertEqual(args.evidence, evidence)
        self.assertEqual(NORMALIZE_EXPECTED_HASH(HASH.upper()), HASH)


if __name__ == "__main__":
    unittest.main()
