"""Goal 02 native harness tests without launching a UI."""

from __future__ import annotations

import tempfile
import time
import unittest
from pathlib import Path
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


def recovery_scan() -> dict[str, int]:
    return {**runtime_scan(), **RECOVERY_SCAN, "files_scanned": 5}


def observations(case_id: str) -> dict:
    original = fingerprint(b"orig")
    edited = fingerprint(b"edit")
    external = fingerprint(b"swap")
    flow = HARNESS.CASE_FLOWS[case_id]
    specific = {
        HARNESS.CASE_POINTER_CANCEL: dict(
            editor=edited,
            source_before=original,
            source_after_cancel_save=edited,
            cancel_kept_tab=True,
        ),
        HARNESS.CASE_KEYBOARD_DISCARD: dict(
            discarded_editor=edited,
            source_before=original,
            source_after_discard=original,
            discard_closed_tab=True,
        ),
        HARNESS.CASE_WINDOW_SAVE: dict(
            editor=edited,
            alt_f4_cancel_kept_window=True,
            editor_after_alt_f4_cancel=edited,
            source_after_alt_f4_cancel=fingerprint(HARNESS.ORIGINAL_BYTES),
            saved_source=edited,
            window_exited=True,
        ),
        HARNESS.CASE_EXTERNAL_CONFLICT: dict(
            loaded_source=original,
            external_source=external,
            dirty_editor=edited,
            source_after_refused_save=external,
            editor_after_conflict=edited,
            saved_after_explicit_overwrite=edited,
            same_length=True,
            same_mtime_ns=True,
            save_refused_with_conflict=True,
            explicit_overwrite_used=True,
        ),
        HARNESS.CASE_RECOVERY: dict(
            edited_text=edited,
            checkpoint_record_seen=True,
            checkpoint_signal=HARNESS.CHECKPOINT_SIGNAL,
            live_runtime_scan=recovery_scan(),
            live_recovery_scan={
                "canonical_record_count": 1,
                "canonical_records": [fingerprint(b"ciphertext")],
            },
            edit_to_signal_ms=9999.0,
            recovery_offer_seen=True,
            restored_editor=edited,
            recovery_restored_exact=True,
            restore_ms=3.0,
            source_after_recovery_discard=original,
            restart_editor=original,
            discarded_recovery_absent=True,
            discard_restart_absent_ms=4.0,
            termination="TerminateProcess",
            restart_count=2,
            startup_signal=HARNESS.STARTUP_SIGNAL,
            startup_observed_before_restart_editor=True,
            process_contexts=[process_context()] * 3,
        ),
    }[case_id]
    return case_observations(flow, **specific, runtime_scan=recovery_scan())


def valid_evidence() -> dict:
    return passed_evidence(
        HARNESS.new_evidence("a" * 64),
        observations,
        lambda value, status: runtime.complete_evidence(value, status, HARNESS.REQUIRED_CASE_IDS),
    )


class EvidenceSchemaTests(unittest.TestCase):
    def test_accepts_complete_versioned_evidence(self) -> None:
        evidence = valid_evidence()

        HARNESS.validate_evidence(evidence)

        self.assertEqual([case["id"] for case in evidence["cases"]], list(HARNESS.REQUIRED_CASE_IDS))

    def test_window_case_requires_an_alt_f4_cancel_that_changed_nothing(self) -> None:
        for key, value in (
            ("editor_after_alt_f4_cancel", {"byte_count": 1, "sha256": "c" * 64}),
            ("source_after_alt_f4_cancel", {"byte_count": 1, "sha256": "c" * 64}),
        ):
            with self.subTest(key=key):
                evidence = valid_evidence()
                evidence["cases"][2]["observations"][key] = value
                with self.assertRaisesRegex(ValueError, "Alt\\+F4 Cancel changed"):
                    HARNESS.validate_evidence(evidence)

        evidence = valid_evidence()
        evidence["cases"][2]["observations"]["alt_f4_cancel_kept_window"] = False
        with self.assertRaisesRegex(ValueError, "requires true alt_f4_cancel_kept_window"):
            HARNESS.validate_evidence(evidence)

    def test_window_scenario_cancels_alt_f4_before_saving_through_wm_close(self) -> None:
        harness = object.__new__(HARNESS.Goal02Harness)
        harness.ui_timeout = 1.0
        order = mock.Mock()
        app = order.app
        app.hwnd = 73
        app.process.poll.return_value = None
        app.security_context.evidence.return_value = {}
        edited = HARNESS.fingerprint_bytes(b"edit")
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            harness.case_roots = mock.Mock(
                return_value=(root / "data", root / "config", root, root / "stderr.log")
            )
            harness.launch = mock.Mock(return_value=app)
            harness.replace_editor = mock.Mock(return_value=edited)
            harness.editor_fingerprint = mock.Mock(return_value=edited)
            harness.win32 = order.win32
            harness.click_lifecycle_decision = order.click_lifecycle_decision

            def exit_after_save(_app, _timeout):
                (root / "window.md").write_bytes(b"edit")

            harness.wait_process_exit = mock.Mock(side_effect=exit_after_save)
            harness.reap = mock.Mock()
            with mock.patch.object(HARNESS, "scan_runtime_artifacts", return_value={}):
                observations = harness.scenario_window_save()

        calls = [call for call in order.mock_calls if not call[0].startswith("app.")]
        sent = calls[1].args[0]
        self.assertEqual(
            [(item.ki.wVk, item.ki.dwFlags) for item in sent],
            [
                (HARNESS.VK_MENU, 0),
                (HARNESS.VK_F4, 0),
                (HARNESS.VK_F4, runtime.KEYEVENTF_KEYUP),
                (HARNESS.VK_MENU, runtime.KEYEVENTF_KEYUP),
            ],
        )
        self.assertEqual(
            calls,
            [
                mock.call.win32.require_foreground(73, 1.0),
                mock.call.win32.send_inputs(sent),
                mock.call.click_lifecycle_decision(app, "Cancel"),
                mock.call.win32.post_close(73),
                mock.call.click_lifecycle_decision(app, "Save"),
            ],
        )
        self.assertTrue(observations["alt_f4_cancel_kept_window"])
        self.assertEqual(
            observations["source_after_alt_f4_cancel"],
            HARNESS.fingerprint_bytes(HARNESS.ORIGINAL_BYTES).evidence(),
        )

    def test_pass_requires_true_case_facts_and_runtime_scan(self) -> None:
        evidence = valid_evidence()
        evidence["cases"][3]["observations"]["save_refused_with_conflict"] = False
        with self.assertRaisesRegex(ValueError, "requires true save_refused_with_conflict"):
            HARNESS.validate_evidence(evidence)

        evidence = valid_evidence()
        evidence["cases"][0]["observations"]["runtime_scan"]["recovery_leases_scanned"] = 2
        with self.assertRaisesRegex(ValueError, "recovery artifact counts are inconsistent"):
            HARNESS.validate_evidence(evidence)

        evidence = valid_evidence()
        evidence["cases"][0]["observations"]["runtime_scan"]["files_scanned"] = 4
        with self.assertRaisesRegex(ValueError, "file count is incomplete"):
            HARNESS.validate_evidence(evidence)

    def test_pass_rejects_signal_over_budget_and_negative_duration(self) -> None:
        evidence = valid_evidence()
        evidence["cases"][4]["observations"]["edit_to_signal_ms"] = 10_000.001
        with self.assertRaisesRegex(ValueError, "exceeds 10000ms"):
            HARNESS.validate_evidence(evidence)

        evidence = valid_evidence()
        evidence["cases"][2]["duration_ms"] = -0.1
        with self.assertRaisesRegex(ValueError, "duration must be nonnegative"):
            HARNESS.validate_evidence(evidence)

        evidence = valid_evidence()
        evidence["cases"][2]["duration_ms"] = float("nan")
        with self.assertRaisesRegex(ValueError, "duration must be nonnegative"):
            HARNESS.validate_evidence(evidence)

    def test_pass_rejects_case_specific_hash_mismatch(self) -> None:
        evidence = valid_evidence()
        evidence["cases"][0]["observations"]["source_after_cancel_save"] = {
            "byte_count": 4,
            "sha256": "b" * 64,
        }

        with self.assertRaisesRegex(ValueError, "Cancel did not preserve exact"):
            HARNESS.validate_evidence(evidence)

    def test_pass_rejects_contradictory_flow_mechanics(self) -> None:
        evidence = valid_evidence()
        evidence["cases"][1]["observations"]["flow"] = HARNESS.CASE_FLOWS[HARNESS.REQUIRED_CASE_IDS[2]]
        with self.assertRaisesRegex(ValueError, "flow mechanics"):
            HARNESS.validate_evidence(evidence)

        evidence = valid_evidence()
        recovery = evidence["cases"][4]["observations"]
        recovery["restart_count"] = 1
        with self.assertRaisesRegex(ValueError, "requires two restarts"):
            HARNESS.validate_evidence(evidence)

    def test_pass_requires_startup_barrier_and_live_canonical_record(self) -> None:
        evidence = valid_evidence()
        recovery = evidence["cases"][4]["observations"]
        recovery["startup_observed_before_restart_editor"] = False
        with self.assertRaisesRegex(ValueError, "startup_observed_before_restart_editor"):
            HARNESS.validate_evidence(evidence)

        evidence = valid_evidence()
        live = evidence["cases"][4]["observations"]["live_recovery_scan"]
        live["canonical_record_count"] = 0
        live["canonical_records"] = []
        with self.assertRaisesRegex(ValueError, "requires a canonical record"):
            HARNESS.validate_evidence(evidence)

        evidence = valid_evidence()
        recovery = evidence["cases"][4]["observations"]
        recovery["live_runtime_scan"]["canonical_recovery_records_scanned"] = 0
        with self.assertRaisesRegex(ValueError, "omitted canonical recovery records"):
            HARNESS.validate_evidence(evidence)

        evidence = valid_evidence()
        recovery = evidence["cases"][4]["observations"]
        recovery["live_runtime_scan"]["recovery_leases_scanned"] = 0
        with self.assertRaisesRegex(ValueError, "omitted recovery lease"):
            HARNESS.validate_evidence(evidence)


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
