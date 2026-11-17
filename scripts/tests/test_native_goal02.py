"""Goal 02 native harness tests without launching a UI."""

from __future__ import annotations

import tempfile
import time
import unittest
from pathlib import Path
from types import SimpleNamespace
from typing import Any
from unittest import mock

from scripts.markturbo_tools.native import goal02 as HARNESS
from scripts.markturbo_tools.native import runtime

from ._native_support import (
    case_observations,
    fingerprint,
    passed_evidence,
    process_context,
    runtime_scan,
)

RECOVERY_SCAN = {
    "recovery_artifacts_scanned": 2,
    "canonical_recovery_records_scanned": 1,
    "recovery_leases_scanned": 1,
}
RECORD_NAME = ("a" * 64) + ".mtrecovery"


def recovery_scan(**counts: int) -> dict[str, int]:
    return {**runtime_scan(), **RECOVERY_SCAN, "files_scanned": 5, **counts}


def retired_scan() -> dict[str, int]:
    return recovery_scan(recovery_artifacts_scanned=1, canonical_recovery_records_scanned=0)


def observations(case_id: str) -> dict:
    original = fingerprint(HARNESS.ORIGINAL_BYTES)
    external = fingerprint(HARNESS.EXTERNAL_BYTES)
    conflict = fingerprint(b"conflict")
    pointer = fingerprint(b"pointer")
    window = fingerprint(b"window")
    edited = fingerprint(b"recovered")
    if case_id == HARNESS.CASE_GUARDED_SESSION:
        specific = dict(
            external_modification_loaded_source=original,
            external_modification_rewritten_source=external,
            external_modification_dirty_editor=conflict,
            external_modification_source_after_refused_save=external,
            external_modification_editor_after_refusal=conflict,
            external_modification_overwritten_source=conflict,
            external_modification_mtime_delta_ns=0,
            external_modification_conflict_ms=12.0,
            tab_close_editor=pointer,
            tab_close_source_after_cancel=conflict,
            tab_close_saved_source=pointer,
            window_close_editor=window,
            window_close_editor_after_cancel=window,
            window_close_source_after_cancel=pointer,
            window_close_saved_source=window,
            window_close_exit_code=0,
            runtime_scan=retired_scan(),
        )
    else:
        specific = dict(
            checkpoint_editor=edited,
            checkpoint_ms=9999.0,
            checkpoint_live_records={
                "canonical_record_count": 1,
                "canonical_records": [fingerprint(b"ciphertext")],
            },
            checkpoint_runtime_scan=recovery_scan(),
            restore_editor=edited,
            restore_ms=3.0,
            keyboard_close_source_before=original,
            keyboard_close_source_after=original,
            keyboard_close_ms=4.0,
            keyboard_close_retire_ms=1.0,
            process_contexts=[process_context()] * 2,
            runtime_scan=retired_scan(),
        )
    return case_observations(HARNESS.CASE_FLOWS[case_id], **specific)


def evidence_with(guarded: dict | None = None, recovery: dict | None = None) -> dict:
    supplied = {
        HARNESS.CASE_GUARDED_SESSION: guarded,
        HARNESS.CASE_RECOVERY_SESSION: recovery,
    }
    return passed_evidence(
        HARNESS.new_evidence("a" * 64),
        lambda case_id: supplied[case_id] or observations(case_id),
        lambda value, status: runtime.complete_evidence(value, status, HARNESS.REQUIRED_CASE_IDS),
    )


GUARDED, RECOVERY = 0, 1


class EvidenceSchemaTests(unittest.TestCase):
    def test_accepts_two_cases_with_every_phase_observed(self) -> None:
        evidence = evidence_with()

        HARNESS.validate_evidence(evidence)

        self.assertEqual(
            [case["id"] for case in evidence["cases"]],
            [HARNESS.CASE_GUARDED_SESSION, HARNESS.CASE_RECOVERY_SESSION],
        )

    def test_every_phase_has_its_own_observation_prefix(self) -> None:
        prefixes = {
            HARNESS.CASE_GUARDED_SESSION: ("external_modification_", "tab_close_", "window_close_"),
            HARNESS.CASE_RECOVERY_SESSION: ("checkpoint_", "restore_", "keyboard_close_"),
        }
        for case_id, required in HARNESS.REQUIRED_OBSERVATIONS.items():
            with self.subTest(case=case_id):
                phase_keys = required - {"process_context", "process_contexts"}
                for prefix in prefixes[case_id]:
                    self.assertTrue(any(key.startswith(prefix) for key in phase_keys), prefix)
                self.assertTrue(
                    all(key.startswith(prefixes[case_id]) for key in phase_keys), phase_keys
                )

    def test_each_phase_rejects_its_own_broken_fact(self) -> None:
        other = fingerprint(b"other")
        cases = (
            (GUARDED, "external_modification_rewritten_source", fingerprint(b"longer external"),
             "not same-length"),
            (GUARDED, "external_modification_mtime_delta_ns", 1, "loaded mtime"),
            (GUARDED, "external_modification_conflict_ms", -1.0, "nonnegative timing"),
            (GUARDED, "external_modification_source_after_refused_save", fingerprint(b"conflict"),
             "refused Save changed"),
            (GUARDED, "external_modification_editor_after_refusal", other, "replaced the dirty"),
            (GUARDED, "external_modification_overwritten_source", other, "explicit overwrite"),
            (GUARDED, "tab_close_source_after_cancel", other, "tab close Cancel changed"),
            (GUARDED, "tab_close_saved_source", other, "Cancel did not preserve"),
            (GUARDED, "window_close_editor_after_cancel", other, "Alt\\+F4 Cancel changed"),
            (GUARDED, "window_close_source_after_cancel", other, "Alt\\+F4 Cancel changed"),
            (GUARDED, "window_close_saved_source", other, "window Save did not preserve"),
            (GUARDED, "window_close_exit_code", 1, "exit the process cleanly"),
            (RECOVERY, "checkpoint_ms", 10_000.001, "exceeds 10000ms"),
            (RECOVERY, "restore_editor", other, "did not restore exact"),
            (RECOVERY, "restore_ms", float("nan"), "nonnegative timing"),
            (RECOVERY, "keyboard_close_source_after", other, "keyboard close Don't save changed"),
            (RECOVERY, "keyboard_close_retire_ms", -0.1, "nonnegative timing"),
            (RECOVERY, "runtime_scan", recovery_scan(), "discarded recovery record was retained"),
            (RECOVERY, "process_contexts", [process_context()] * 3, "one interrupted"),
            (RECOVERY, "checkpoint_runtime_scan", recovery_scan(recovery_leases_scanned=0),
             "omitted recovery lease"),
            (RECOVERY, "checkpoint_runtime_scan",
             recovery_scan(canonical_recovery_records_scanned=0, recovery_artifacts_scanned=1),
             "omitted canonical recovery records"),
            (RECOVERY, "checkpoint_live_records",
             {"canonical_record_count": 0, "canonical_records": []}, "requires a canonical record"),
        )
        for index, key, value, message in cases:
            with self.subTest(key=key, value=value):
                evidence = evidence_with()
                evidence["cases"][index]["observations"][key] = value
                with self.assertRaisesRegex(ValueError, message):
                    HARNESS.validate_evidence(evidence)

    def test_rejects_missing_phase_and_retired_constant_observations(self) -> None:
        evidence = evidence_with()
        del evidence["cases"][GUARDED]["observations"]["tab_close_saved_source"]
        with self.assertRaisesRegex(ValueError, "incomplete"):
            HARNESS.validate_evidence(evidence)

        for key, value in (
            ("termination", "TerminateProcess"),
            ("restart_count", 2),
            ("cancel_kept_tab", True),
        ):
            with self.subTest(key=key):
                evidence = evidence_with()
                evidence["cases"][RECOVERY]["observations"][key] = value
                with self.assertRaisesRegex(ValueError, "unknown observation field"):
                    HARNESS.validate_evidence(evidence)

    def test_runtime_scan_counts_must_be_consistent(self) -> None:
        evidence = evidence_with()
        evidence["cases"][GUARDED]["observations"]["runtime_scan"]["recovery_leases_scanned"] = 2
        with self.assertRaisesRegex(ValueError, "recovery artifact counts are inconsistent"):
            HARNESS.validate_evidence(evidence)

        evidence = evidence_with()
        evidence["cases"][GUARDED]["observations"]["runtime_scan"]["files_scanned"] = 3
        with self.assertRaisesRegex(ValueError, "file count is incomplete"):
            HARNESS.validate_evidence(evidence)


class PhaseTests(unittest.TestCase):
    def test_a_phase_prefixes_failures_once_and_leaves_blocks_alone(self) -> None:
        with self.assertRaises(runtime.HarnessFailure) as raised:
            with HARNESS.phase("TAB_CLOSE"):
                raise runtime.HarnessFailure("GUARD_CARD_UIA_TIMEOUT", "TimeoutError")
        self.assertEqual(raised.exception.code, "TAB_CLOSE_GUARD_CARD_UIA_TIMEOUT")
        self.assertEqual(raised.exception.detail, "TimeoutError")

        with self.assertRaises(runtime.HarnessFailure) as raised:
            with HARNESS.phase("TAB_CLOSE"):
                raise runtime.HarnessFailure("TAB_CLOSE_DONE")
        self.assertEqual(raised.exception.code, "TAB_CLOSE_DONE")

        with self.assertRaises(runtime.HarnessBlocked) as blocked:
            with HARNESS.phase("TAB_CLOSE"):
                raise runtime.HarnessBlocked("FOREGROUND_UNAVAILABLE")
        self.assertEqual(blocked.exception.code, "FOREGROUND_UNAVAILABLE")


class FakeWin32:
    def __init__(self, desk: "FakeDesk") -> None:
        self.desk = desk

    def require_foreground(self, hwnd: int, _timeout: float) -> None:
        self.desk.events.append(("foreground", hwnd))

    def foreground_focus(self, hwnd: int) -> int:
        return hwnd

    def send_inputs(self, inputs: list[Any]) -> None:
        keys = [(item.ki.wVk, item.ki.dwFlags) for item in inputs]
        self.desk.events.append(("inputs", tuple(keys)))
        if keys[:2] == [(HARNESS.VK_MENU, 0), (HARNESS.VK_F4, 0)]:
            self.desk.guard = "window"

    def send_key(self, _hwnd: int, key: int) -> None:
        self.desk.events.append(("key", key))
        if key == runtime.VK_RETURN and self.desk.enter_overwrites:
            self.desk.write_editor()

    def send_shortcut(self, _hwnd: int, key: int) -> None:
        self.desk.events.append(("shortcut", key))
        if key == HARNESS.VK_W:
            self.desk.guard = "tab"

    def post_close(self, _hwnd: int) -> None:
        self.desk.events.append(("wm_close",))
        self.desk.guard = "window"


class FakeDesk:
    """The app's observable side: editor bytes, dirty mark, conflict, the file."""

    def __init__(
        self,
        root: Path,
        *,
        keep_mine_writes: bool = False,
        enter_overwrites: bool = True,
        cancel_clears_dirty: bool = False,
        discard_retires: bool = True,
        failing_guard: int | None = None,
    ) -> None:
        self.root = root
        self.keep_mine_writes = keep_mine_writes
        self.enter_overwrites = enter_overwrites
        self.cancel_clears_dirty = cancel_clears_dirty
        self.discard_retires = discard_retires
        self.failing_guard = failing_guard
        self.guards = 0
        self.events: list[tuple[Any, ...]] = []
        self.editor = b""
        self.dirty = False
        self.conflict = False
        self.guard: str | None = None
        self.tab_open = True
        self.document: Path | None = None
        self.known: bytes | None = None
        self.launches: list[Any] = []

    def harness(self) -> HARNESS.Goal02Harness:
        harness = object.__new__(HARNESS.Goal02Harness)
        harness.ui_timeout = 0.2
        harness.win32 = FakeWin32(self)
        harness.case_roots = self.case_roots
        harness.launch = self.launch
        harness.replace_editor = self.replace_editor
        harness.editor_fingerprint = lambda _app: runtime.fingerprint_bytes(self.editor)
        harness.document_dirty = lambda _app: self.dirty
        harness.name_present = lambda _app, _name, _type: self.conflict
        harness.wait_name = self.wait_name
        harness.save_via_menu = self.save_via_menu
        harness.find_control = lambda _app, automation_id, *_args: automation_id
        harness.click_control = self.click_control
        harness.close_tab_by_pointer = self.close_tab_by_pointer
        harness.click_lifecycle_decision = self.click_lifecycle_decision
        harness.wait_process_exit = self.wait_process_exit
        harness.close_app = self.close_app
        harness.terminate = self.terminate
        harness.reap = mock.Mock()
        harness.focus_editor = lambda _app: None
        harness.wait_editor_fingerprint = self.wait_editor_fingerprint
        harness.editor_absent_while_running = lambda _app: not self.tab_open
        return harness

    def case_roots(self, case_id: str) -> tuple[Path, Path, Path, Path]:
        case = self.root / case_id
        roots = (case / "data", case / "config", case / "workspace")
        for directory in roots:
            directory.mkdir(parents=True, exist_ok=True)
        return (*roots, case / "stderr.log")

    def launch(self, document: Path, data: Path, *_roots: Path) -> SimpleNamespace:
        self.document, self.data = document, data
        self.known = document.read_bytes()
        self.editor, self.dirty, self.tab_open = self.known, False, True
        self.events.append(("launch",))
        app = SimpleNamespace(
            hwnd=73,
            process=SimpleNamespace(returncode=None),
            security_context=runtime.SecurityContext(7, 0x2000, "medium"),
        )
        app.process.poll = lambda: app.process.returncode
        self.app = app
        self.launches.append(app)
        return app

    def replace_editor(self, _app: Any, value: str) -> runtime.Fingerprint:
        self.editor, self.dirty = value.encode("utf-8"), True
        self.events.append(("edit",))
        if value == HARNESS.RECOVERY_EDIT:
            recovery = self.data / "recovery"
            recovery.mkdir(parents=True, exist_ok=True)
            (recovery / RECORD_NAME).write_bytes(b"ciphertext")
            (recovery / HARNESS.RECOVERY_LEASE_NAME).write_bytes(b"")
        return runtime.fingerprint_bytes(self.editor)

    def write_editor(self) -> None:
        runtime.write_durable(self.document, self.editor)
        self.known, self.dirty, self.conflict = self.editor, False, False

    def wait_name(self, _app: Any, _name: str, _type: str, code: str) -> None:
        if not self.conflict:
            raise runtime.HarnessFailure(code)

    def save_via_menu(self, _app: Any) -> None:
        self.events.append(("menu save",))
        if self.document.read_bytes() != self.known:
            self.conflict = True
        else:
            self.write_editor()

    def click_control(self, control: str, _code: str) -> None:
        self.events.append(("click", control))
        if control == HARNESS.KEEP_MINE_AUTOMATION_ID and self.keep_mine_writes:
            self.write_editor()
        elif control == HARNESS.RESTORE_AUTOMATION_ID:
            self.editor, self.dirty = HARNESS.RECOVERY_EDIT.encode("utf-8"), True

    def close_tab_by_pointer(self, _app: Any) -> None:
        self.events.append(("rail close",))
        self.guard = "tab"

    def click_lifecycle_decision(self, app: Any, name: str) -> None:
        self.guards += 1
        if self.guards == self.failing_guard or self.guard is None:
            raise runtime.HarnessFailure("GUARD_CARD_UIA_TIMEOUT")
        self.events.append(("guard", name))
        guard, self.guard = self.guard, None
        if name == "Cancel":
            self.dirty = self.dirty and not self.cancel_clears_dirty
        elif name == "Save" and guard == "window":
            self.write_editor()
            app.process.returncode = 0
        elif name == "Discard":
            self.tab_open = False
            if self.discard_retires:
                (self.data / "recovery" / RECORD_NAME).unlink()

    def wait_process_exit(self, app: Any, _timeout: float | None = None) -> None:
        if app.process.returncode != 0:
            raise runtime.HarnessFailure("PROCESS_EXIT_TIMEOUT")

    def close_app(self, app: Any) -> None:
        self.events.append(("close app",))
        app.process.returncode = 0

    def terminate(self, app: Any) -> None:
        self.events.append(("terminate",))
        app.process.returncode = 1

    def wait_editor_fingerprint(
        self, _app: Any, expected: runtime.Fingerprint | None, _timeout: float
    ) -> tuple[runtime.Fingerprint, float]:
        actual = runtime.fingerprint_bytes(self.editor)
        if expected is not None and actual != expected:
            raise runtime.HarnessFailure("EDITOR_FINGERPRINT_TIMEOUT")
        return actual, 0.0


class ScenarioTests(unittest.TestCase):
    def run_scenario(self, name: str, scans: list[dict[str, int]], **desk_options: Any):
        directory = tempfile.TemporaryDirectory()
        self.addCleanup(directory.cleanup)
        desk = FakeDesk(Path(directory.name), **desk_options)
        harness = desk.harness()
        with (
            mock.patch.object(runtime, "SYSTEM_ALERT_SETTLE_SECONDS", 0),
            mock.patch.object(HARNESS, "scan_runtime_artifacts", side_effect=scans),
        ):
            return getattr(harness, name)(), desk

    def run_guarded(self, **desk_options: Any):
        return self.run_scenario("scenario_guarded_session", [retired_scan()], **desk_options)

    def run_recovery(self, **desk_options: Any):
        return self.run_scenario(
            "scenario_recovery_session", [recovery_scan(), retired_scan()], **desk_options
        )

    def test_guarded_session_runs_every_phase_in_one_launch(self) -> None:
        observations, desk = self.run_guarded()

        self.assertEqual(len(desk.launches), 1)
        alt_f4 = (
            (HARNESS.VK_MENU, 0),
            (HARNESS.VK_F4, 0),
            (HARNESS.VK_F4, runtime.KEYEVENTF_KEYUP),
            (HARNESS.VK_MENU, runtime.KEYEVENTF_KEYUP),
        )
        self.assertEqual(
            [event for event in desk.events if event[0] != "foreground"],
            [
                ("launch",),
                ("edit",),
                ("menu save",),
                ("click", HARNESS.KEEP_MINE_AUTOMATION_ID),
                ("key", runtime.VK_RETURN),
                ("edit",),
                ("rail close",),
                ("guard", "Cancel"),
                ("menu save",),
                ("edit",),
                ("inputs", alt_f4),
                ("guard", "Cancel"),
                ("wm_close",),
                ("guard", "Save"),
            ],
        )
        self.assertEqual(desk.document.read_bytes(), HARNESS.WINDOW_EDIT.encode("utf-8"))
        HARNESS.validate_evidence(evidence_with(guarded=observations))

    def test_external_modification_overwrites_only_after_enter_confirms(self) -> None:
        for options, code in (
            (dict(keep_mine_writes=True, enter_overwrites=False),
             "EXTERNAL_MODIFICATION_KEEP_MINE_WROTE_BEFORE_CONFIRMATION"),
            (dict(enter_overwrites=False), "EXTERNAL_MODIFICATION_OVERWRITE_SAVE_TIMEOUT"),
        ):
            with self.subTest(code=code):
                with self.assertRaises(runtime.HarnessFailure) as raised:
                    self.run_guarded(**options)
                self.assertEqual(raised.exception.code, code)

    def test_guarded_session_failures_name_their_phase(self) -> None:
        for options, code in (
            (dict(failing_guard=1), "TAB_CLOSE_GUARD_CARD_UIA_TIMEOUT"),
            (dict(failing_guard=2), "WINDOW_CLOSE_GUARD_CARD_UIA_TIMEOUT"),
            (dict(failing_guard=3), "WINDOW_CLOSE_GUARD_CARD_UIA_TIMEOUT"),
            (dict(cancel_clears_dirty=True), "TAB_CLOSE_CANCEL_CLEARED_DIRTY_MARK"),
        ):
            with self.subTest(code=code, options=options):
                with self.assertRaises(runtime.HarnessFailure) as raised:
                    self.run_guarded(**options)
                self.assertEqual(raised.exception.code, code)

    def test_recovery_session_restarts_once_and_closes_with_ctrl_w(self) -> None:
        observations, desk = self.run_recovery()

        self.assertEqual(len(desk.launches), 2)
        self.assertEqual(
            [event for event in desk.events if event[0] != "foreground"],
            [
                ("launch",),
                ("edit",),
                ("terminate",),
                ("launch",),
                ("click", HARNESS.RESTORE_AUTOMATION_ID),
                ("shortcut", HARNESS.VK_W),
                ("guard", "Discard"),
                ("close app",),
            ],
        )
        self.assertEqual(desk.document.read_bytes(), HARNESS.ORIGINAL_BYTES)
        self.assertLessEqual(observations["checkpoint_ms"], 10_000)
        HARNESS.validate_evidence(evidence_with(recovery=observations))

    def test_recovery_session_fails_when_dont_save_keeps_the_record(self) -> None:
        with self.assertRaises(runtime.HarnessFailure) as raised:
            self.run_recovery(discard_retires=False)
        self.assertEqual(raised.exception.code, "KEYBOARD_CLOSE_RECOVERY_RECORD_RETAINED")


class RuntimeArtifactTests(unittest.TestCase):
    def test_checkpoint_signal_is_only_a_canonical_recovery_record(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            data = Path(temporary) / "data"
            self.assertEqual(HARNESS.canonical_recovery_records(data), [])
            recovery = data / "recovery"
            recovery.mkdir(parents=True)
            (recovery / "junk.mtrecovery").write_bytes(b"ignored")
            (recovery / (("b" * 64) + ".tmp")).write_bytes(b"partial")
            (recovery / (("c" * 64) + ".mtrecovery")).mkdir()
            self.assertEqual(HARNESS.canonical_recovery_records(data), [])

            canonical = recovery / (("a" * 64) + ".mtrecovery")
            canonical.write_bytes(b"sealed")
            self.assertEqual(HARNESS.canonical_recovery_records(data), [canonical])

    def test_retained_recovery_artifacts_ignore_only_the_lease(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            data = Path(temporary) / "data"
            self.assertEqual(HARNESS.retained_recovery_artifacts(data), [])
            recovery = data / "recovery"
            recovery.mkdir(parents=True)
            (recovery / HARNESS.RECOVERY_LEASE_NAME).write_bytes(b"")
            self.assertEqual(HARNESS.retained_recovery_artifacts(data), [])

            marker = recovery / (".markturbo-recovery-retiring-" + "a" * 64)
            marker.write_bytes(b"")
            record = recovery / RECORD_NAME
            record.write_bytes(b"sealed")
            self.assertEqual(HARNESS.retained_recovery_artifacts(data), [marker, record])

    def test_checkpoint_wait_is_bounded_by_the_ten_second_loss_window(self) -> None:
        harness = object.__new__(HARNESS.Goal02Harness)
        with tempfile.TemporaryDirectory() as temporary:
            data = Path(temporary) / "data"
            (data / "recovery").mkdir(parents=True)
            (data / "recovery" / (("a" * 64) + ".mtrecovery")).write_bytes(b"sealed")
            started = time.perf_counter()
            self.assertLessEqual(harness.wait_checkpoint_record(data, started), 10_000)

            with self.assertRaises(runtime.HarnessFailure) as late:
                harness.wait_checkpoint_record(data, started - 10.5)
            self.assertEqual(late.exception.code, "RECOVERY_RECORD_EXCEEDED_10_SECONDS")

            empty = Path(temporary) / "empty"
            with self.assertRaises(runtime.HarnessFailure) as missing:
                harness.wait_checkpoint_record(empty, time.perf_counter() - 9.95)
            self.assertEqual(missing.exception.code, "RECOVERY_RECORD_EXCEEDED_10_SECONDS")

    def test_live_recovery_scan_requires_canonical_encrypted_record(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            data = Path(temporary) / "data"
            recovery = data / "recovery"
            recovery.mkdir(parents=True)
            (recovery / "junk.mtrecovery").write_bytes(b"ignored")
            with self.assertRaises(runtime.HarnessFailure) as missing:
                HARNESS.scan_live_recovery_records(data)
            self.assertEqual(missing.exception.code, "CANONICAL_RECOVERY_RECORD_MISSING")

            canonical = recovery / (("a" * 64) + ".mtrecovery")
            canonical.write_bytes(b"")
            with self.assertRaises(runtime.HarnessFailure) as empty:
                HARNESS.scan_live_recovery_records(data)
            self.assertEqual(empty.exception.code, "CANONICAL_RECOVERY_RECORD_EMPTY")

            canonical.write_bytes(b"encrypted-record")
            result = HARNESS.scan_live_recovery_records(data)
            self.assertEqual(result["canonical_record_count"], 1)
            self.assertEqual(len(result["canonical_records"]), 1)

            canonical.write_bytes(HARNESS.DOCUMENT_SENTINEL.encode("utf-8"))
            with self.assertRaises(runtime.HarnessFailure) as leaked:
                HARNESS.scan_live_recovery_records(data)
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

            result = HARNESS.scan_runtime_artifacts(data, stderr)

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
            lease_only = HARNESS.scan_runtime_artifacts(data, stderr)
            self.assertEqual(lease_only["canonical_recovery_records_scanned"], 0)
            self.assertEqual(lease_only["recovery_leases_scanned"], 1)

            (recovery / ".markturbo-recovery.lock").unlink()
            (recovery / (("a" * 64) + ".mtrecovery")).write_bytes(b"ciphertext")
            record_only = HARNESS.scan_runtime_artifacts(data, stderr)
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
                with self.assertRaises(runtime.HarnessFailure) as live:
                    HARNESS.scan_live_recovery_records(data)
            self.assertEqual(live.exception.code, "LIVE_RECOVERY_RECORD_SCAN_FAILED")
            self.assertEqual(live.exception.detail, "PermissionError")

            with mock.patch.object(Path, "read_bytes", new=denied):
                with self.assertRaises(runtime.HarnessFailure) as scanned:
                    HARNESS.scan_runtime_artifacts(data, stderr)
            self.assertEqual(scanned.exception.code, "RUNTIME_ARTIFACT_SCAN_FAILED")
            self.assertEqual(scanned.exception.detail, "PermissionError")
            self.assertNotIn(secret, scanned.exception.detail)

    def test_runtime_scan_rejects_utf8_utf16_panic_and_refcell(self) -> None:
        payloads = (
            (HARNESS.DOCUMENT_SENTINEL.encode("utf-8"), "UTF8_DOCUMENT_SENTINEL_LEAKED"),
            (HARNESS.DOCUMENT_SENTINEL.encode("utf-16-le"), "UTF16LE_DOCUMENT_SENTINEL_LEAKED"),
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

                with self.assertRaises(runtime.HarnessFailure) as raised:
                    HARNESS.scan_runtime_artifacts(data, stderr)
                self.assertEqual(raised.exception.code, code)

    def test_runtime_scan_requires_app_log_and_scans_recovery_payload(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            data = root / "data"
            stderr = root / "stderr.log"
            stderr.write_bytes(b"")
            with self.assertRaises(runtime.HarnessFailure) as missing:
                HARNESS.scan_runtime_artifacts(data, stderr)
            self.assertEqual(missing.exception.code, "APP_LOG_MISSING")

            logs = data / "logs"
            recovery = data / "recovery"
            logs.mkdir(parents=True)
            recovery.mkdir()
            (logs / "markturbo-1.log").write_bytes(b"startup ok")
            (recovery / "record.mtrecovery").write_bytes(
                HARNESS.DOCUMENT_SENTINEL.encode("utf-16-le")
            )
            with self.assertRaises(runtime.HarnessFailure) as leaked:
                HARNESS.scan_runtime_artifacts(data, stderr)
            self.assertEqual(leaked.exception.code, "UTF16LE_DOCUMENT_SENTINEL_LEAKED")
