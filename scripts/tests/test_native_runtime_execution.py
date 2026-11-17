"""The shared native acceptance lifecycle, run through goal 02 with fake harnesses."""

from __future__ import annotations

import argparse
import contextlib
import dataclasses
import hashlib
import io
import json
import shutil
import tempfile
import unittest
from pathlib import Path
from unittest import mock

from scripts.markturbo_tools.native import evidence, goal03, goal06, goal07, runtime
from scripts.markturbo_tools.native import goal02 as HARNESS

from ._native_support import HASH


class PrivacyAndCliTests(unittest.TestCase):
    def test_isolation_cleanup_retries_transient_permission_errors(self) -> None:
        root = Path("temporary-native-root")
        with (
            mock.patch.object(
                runtime.shutil,
                "rmtree",
                side_effect=[PermissionError(), PermissionError(), None],
            ) as remove,
            mock.patch.object(runtime.time, "sleep") as sleep,
        ):
            runtime.remove_tree_with_retry(root, timeout=1.0)

        self.assertEqual(remove.call_count, 3)
        self.assertEqual(sleep.call_count, 2)

    def test_isolation_cleanup_still_fails_after_the_retry_budget(self) -> None:
        with mock.patch.object(
            runtime.shutil, "rmtree", side_effect=PermissionError()
        ):
            with self.assertRaises(PermissionError):
                runtime.remove_tree_with_retry(Path("temporary-native-root"), timeout=0.0)

    def test_exception_sanitizer_never_returns_exception_text(self) -> None:
        secret = "UNIQUE-DOCUMENT-CONTENT"

        result = runtime.safe_exception_name(RuntimeError(secret))

        self.assertEqual(result, "RuntimeError")
        self.assertNotIn(secret, result)

    def test_failure_type_prefers_safe_detail_and_rejects_unsafe_detail(self) -> None:
        for name in ("PermissionError", "TypeError", "COMError"):
            with self.subTest(name=name):
                self.assertEqual(
                    runtime.safe_failure_type(runtime.HarnessFailure("RUNTIME_ARTIFACT_SCAN_FAILED", name)),
                    name,
                )
        for detail in ("UNIQUE_SECRET", "secret text", "secret-text"):
            with self.subTest(detail=detail):
                self.assertEqual(
                    runtime.safe_failure_type(runtime.HarnessFailure("RUNTIME_ARTIFACT_SCAN_FAILED", detail)),
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
                spec = runtime.build_launch_spec(
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

    def test_default_failure_receipts_preserve_retained_evidence(self) -> None:
        for module in (HARNESS, goal03, goal06, goal07):
            with self.subTest(goal=module.SCHEMA), tempfile.TemporaryDirectory() as temporary:
                args = module.parse_args(["--expect-exe-sha256", HASH])
                relative = args.evidence.relative_to(evidence.REPO)
                root = Path(temporary)
                retained = root / "docs" / "evidence" / relative.name
                retained.parent.mkdir(parents=True)
                original = b'{"status": "PASS"}\n'
                retained.write_bytes(original)
                output = root / relative
                stderr = io.StringIO()
                with (
                    mock.patch.object(evidence, "REPO", root),
                    contextlib.redirect_stderr(stderr),
                ):
                    returncode = module.main(
                        ["--exe", str(root / "missing.exe"), "--expect-exe-sha256", HASH]
                    )

                self.assertEqual(returncode, 1)
                self.assertIn("FAIL: EXECUTABLE_MISSING", stderr.getvalue())
                self.assertEqual(retained.read_bytes(), original)
                self.assertEqual(json.loads(output.read_text(encoding="utf-8"))["status"], "FAIL")
                explicit = root / "chosen-receipt.json"
                self.assertEqual(
                    module.parse_args(
                        ["--expect-exe-sha256", HASH, "--evidence", str(explicit)]
                    ).evidence,
                    explicit,
                )

    def test_parser_exposes_debug_case_and_workdir_retention(self) -> None:
        case = HARNESS.REQUIRED_CASE_IDS[0]
        args = HARNESS.parse_args(
            [
                "--expect-exe-sha256",
                HASH,
                "--case",
                case,
                "--keep-workdir-on-failure",
            ]
        )
        self.assertEqual(args.case, case)
        self.assertTrue(args.keep_workdir_on_failure)

    def test_single_case_run_never_claims_acceptance_pass(self) -> None:
        class FakeHarness:
            def __init__(self, *_args):
                pass

            def scenario_pointer_cancel(self):
                return {}

            scenario_keyboard_discard = scenario_pointer_cancel
            scenario_window_save = scenario_pointer_cancel
            scenario_external_conflict = scenario_pointer_cancel
            scenario_recovery = scenario_pointer_cancel

            def cleanup(self):
                pass

        with tempfile.TemporaryDirectory() as temporary:
            exe = Path(temporary) / "markturbo.exe"
            exe.write_bytes(b"test executable")
            expected = hashlib.sha256(exe.read_bytes()).hexdigest()
            args = HARNESS.parse_args(
                [
                    "--exe",
                    str(exe),
                    "--expect-exe-sha256",
                    expected,
                    "--case",
                    HARNESS.REQUIRED_CASE_IDS[0],
                ]
            )
            with mock.patch.dict(
                HARNESS.__dict__,
                {
                    "preflight": lambda *_args: (object(), object()),
                    "load_pywinauto": lambda: (object(), object(), object(), object(), object()),
                    "Goal02Harness": FakeHarness,
                },
            ):
                returncode, evidence, code = runtime.run_native_acceptance(
                    args, HARNESS.native_run_plan()
                )

        self.assertEqual((returncode, evidence["status"], code), (1, "FAIL", "PARTIAL_CASE_RUN"))
        self.assertEqual(evidence["cases"][0]["status"], "PASS")
        self.assertTrue(all(case["status"] == "NOT_RUN" for case in evidence["cases"][1:]))

    def test_run_lifecycle_cleans_success_and_keeps_failure_or_partial_workdirs(self) -> None:
        roots: list[Path] = []

        class FakeHarness:
            def __init__(self, _exe, root, *_args):
                roots.append(root)

            def scenario_pointer_cancel(self):
                return {}

            scenario_keyboard_discard = scenario_pointer_cancel
            scenario_window_save = scenario_pointer_cancel
            scenario_external_conflict = scenario_pointer_cancel
            scenario_recovery = scenario_pointer_cancel

            def cleanup(self):
                pass

        with tempfile.TemporaryDirectory() as temporary:
            exe = Path(temporary) / "markturbo.exe"
            exe.write_bytes(b"test executable")
            expected = hashlib.sha256(exe.read_bytes()).hexdigest()
            patch_run = {
                "preflight": lambda *_args: (object(), object()),
                "load_pywinauto": lambda: (object(), object(), object(), object(), object()),
                "Goal02Harness": FakeHarness,
            }

            with mock.patch.dict(HARNESS.__dict__, patch_run):
                success_args = HARNESS.parse_args(["--exe", str(exe), "--expect-exe-sha256", expected])
                success_code, _, _ = runtime.run_native_acceptance(
                    success_args, HARNESS.native_run_plan()
                )

                partial_args = HARNESS.parse_args(
                    [
                        "--exe",
                        str(exe),
                        "--expect-exe-sha256",
                        expected,
                        "--case",
                        HARNESS.REQUIRED_CASE_IDS[0],
                        "--keep-workdir-on-failure",
                    ]
                )
                partial_code, _, partial_reason = runtime.run_native_acceptance(
                    partial_args, HARNESS.native_run_plan()
                )

            self.assertEqual(success_code, 0)
            self.assertFalse(roots[0].exists())
            self.assertEqual((partial_code, partial_reason), (1, "PARTIAL_CASE_RUN"))
            self.assertIsNotNone(partial_args.debug_workdir)
            self.assertTrue(partial_args.debug_workdir.exists())
            shutil.rmtree(partial_args.debug_workdir)

        roots.clear()

        class FailingHarness(FakeHarness):
            def scenario_pointer_cancel(self):
                raise runtime.HarnessFailure("TEST_CASE_FAILURE")

        with tempfile.TemporaryDirectory() as temporary:
            exe = Path(temporary) / "markturbo.exe"
            exe.write_bytes(b"test executable")
            expected = hashlib.sha256(exe.read_bytes()).hexdigest()
            args = HARNESS.parse_args(
                [
                    "--exe",
                    str(exe),
                    "--expect-exe-sha256",
                    expected,
                    "--keep-workdir-on-failure",
                ]
            )
            with mock.patch.dict(
                HARNESS.__dict__,
                {
                    "preflight": lambda *_args: (object(), object()),
                    "load_pywinauto": lambda: (object(), object(), object(), object(), object()),
                    "Goal02Harness": FailingHarness,
                },
            ):
                returncode, _, code = runtime.run_native_acceptance(
                    args, HARNESS.native_run_plan()
                )

            self.assertEqual((returncode, code), (1, "TEST_CASE_FAILURE"))
            self.assertIsNotNone(args.debug_workdir)
            self.assertTrue(args.debug_workdir.exists())
            shutil.rmtree(args.debug_workdir)

    def test_isolation_copy_failure_is_fail_closed_and_keeps_requested_workdir(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            exe = Path(temporary) / "markturbo.exe"
            exe.write_bytes(b"test executable")
            expected = hashlib.sha256(exe.read_bytes()).hexdigest()
            args = HARNESS.parse_args(
                [
                    "--exe",
                    str(exe),
                    "--expect-exe-sha256",
                    expected,
                    "--keep-workdir-on-failure",
                ]
            )
            with (
                mock.patch.dict(
                    HARNESS.__dict__,
                    {
                        "preflight": lambda *_args: (object(), object()),
                        "load_pywinauto": lambda: (object(), object(), object(), object(), object()),
                    },
                ),
                mock.patch.object(runtime.shutil, "copy2", side_effect=OSError("denied")),
            ):
                returncode, evidence, code = runtime.run_native_acceptance(
                    args, HARNESS.native_run_plan()
                )

            self.assertEqual((returncode, evidence["status"], code), (1, "FAIL", "ISOLATION_COPY_FAILED"))
            self.assertTrue(all(case["status"] == "NOT_RUN" for case in evidence["cases"]))
            self.assertIsNotNone(args.debug_workdir)
            self.assertTrue(args.debug_workdir.exists())
            shutil.rmtree(args.debug_workdir)

    def test_isolation_copy_failure_persists_fail_closed_evidence(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            exe = root / "markturbo.exe"
            evidence_path = root / "evidence.json"
            exe.write_bytes(b"test executable")
            expected = hashlib.sha256(exe.read_bytes()).hexdigest()
            with (
                mock.patch.dict(
                    HARNESS.__dict__,
                    {
                        "preflight": lambda *_args: (object(), object()),
                        "load_pywinauto": lambda: (object(), object(), object(), object(), object()),
                    },
                ),
                mock.patch.object(runtime.shutil, "copy2", side_effect=OSError("denied")),
            ):
                with contextlib.redirect_stderr(io.StringIO()):
                    returncode = HARNESS.main(
                        [
                            "--exe",
                            str(exe),
                            "--expect-exe-sha256",
                            expected,
                            "--evidence",
                            str(evidence_path),
                        ]
                    )

            evidence = json.loads(evidence_path.read_text(encoding="utf-8"))
            self.assertEqual(returncode, 1)
            self.assertEqual(evidence["status"], "FAIL")
            self.assertTrue(all(case["status"] == "NOT_RUN" for case in evidence["cases"]))


SYNTHETIC_CASES = ("first", "second", "third")
SYNTHETIC_CONTRACT = evidence.EvidenceContract(
    schema="markturbo.synthetic-native-acceptance",
    case_flows={case: case for case in SYNTHETIC_CASES},
    observation_keys=frozenset(),
    fingerprint_keys=frozenset(),
    required_observations={case: frozenset() for case in SYNTHETIC_CASES},
    validate_runtime_scan=lambda _value: None,
    validate_passed_case=lambda _case, _observations: None,
    private_sentinels=("UNIQUE-DOCUMENT-CONTENT",),
)


class SyntheticHarness:
    def __init__(self, _exe, root, *_args) -> None:
        self.root = root

    def cleanup(self) -> None:
        pass


def synthetic_plan(
    outcomes: tuple[object, ...], preflight: object | None = None
) -> runtime.NativeRunPlan:
    """A plan whose cases pass ({}), FAIL or BLOCK (an exception to raise)."""

    def scenario(outcome: object):
        def run() -> dict:
            if isinstance(outcome, BaseException):
                raise outcome
            return {}

        return run

    return runtime.NativeRunPlan(
        required_case_ids=SYNTHETIC_CASES,
        workdir_prefix="markturbo-synthetic-native-",
        new_evidence=lambda expected: evidence.new_evidence(SYNTHETIC_CONTRACT, expected),
        # The envelope validator is tested on its own; these runs bind no real executable.
        validate_evidence=lambda _value: None,
        preflight=preflight or (lambda *_args: (object(), object())),
        ui_types_loader=lambda: (),
        harness_factory=SyntheticHarness,
        scenarios=lambda _harness: tuple(scenario(outcome) for outcome in outcomes),
    )


def synthetic_args(exe: Path, evidence_path: Path | None = None, **extra: object) -> argparse.Namespace:
    return argparse.Namespace(
        exe=exe,
        expect_exe_sha256=hashlib.sha256(exe.read_bytes()).hexdigest(),
        evidence=evidence_path,
        ui_timeout=1.0,
        case=None,
        keep_workdir_on_failure=False,
        keep_going=False,
        **extra,
    )


class KeepGoingTests(unittest.TestCase):
    def run_cases(self, outcomes: tuple[object, ...], keep_going: bool) -> tuple[int, dict, str]:
        with tempfile.TemporaryDirectory() as temporary:
            exe = Path(temporary) / "markturbo.exe"
            exe.write_bytes(b"test executable")
            args = synthetic_args(exe)
            args.keep_going = keep_going
            return runtime.run_native_acceptance(args, synthetic_plan(outcomes))

    def statuses(self, value: dict) -> list[tuple[str, str | None]]:
        return [(case["status"], case["reason_code"]) for case in value["cases"]]

    def test_a_failure_stops_the_later_cases_by_default(self) -> None:
        code, value, reason = self.run_cases(
            (runtime.HarnessFailure("FIRST_FAILED"), {}, {}), keep_going=False
        )

        self.assertEqual((code, value["status"], reason), (1, "FAIL", "FIRST_FAILED"))
        self.assertEqual(
            self.statuses(value),
            [
                ("FAIL", "FIRST_FAILED"),
                ("NOT_RUN", "SKIPPED_AFTER_FAILURE"),
                ("NOT_RUN", "SKIPPED_AFTER_FAILURE"),
            ],
        )

    def test_keep_going_runs_every_case_and_the_first_failure_decides(self) -> None:
        code, value, reason = self.run_cases(
            (runtime.HarnessFailure("FIRST_FAILED"), {}, RuntimeError("UNIQUE-DOCUMENT-CONTENT")),
            keep_going=True,
        )

        self.assertEqual((code, value["status"], reason), (1, "FAIL", "FIRST_FAILED"))
        self.assertEqual(
            self.statuses(value),
            [("FAIL", "FIRST_FAILED"), ("PASS", None), ("FAIL", "INTERNAL_FAILURE")],
        )

    def test_keep_going_still_stops_at_blocked(self) -> None:
        code, value, reason = self.run_cases(
            (runtime.HarnessFailure("FIRST_FAILED"), runtime.HarnessBlocked("SESSION_NOT_ACTIVE"), {}),
            keep_going=True,
        )

        self.assertEqual((code, value["status"], reason), (1, "FAIL", "FIRST_FAILED"))
        self.assertEqual(
            self.statuses(value),
            [
                ("FAIL", "FIRST_FAILED"),
                ("BLOCKED", "SESSION_NOT_ACTIVE"),
                ("BLOCKED", "SKIPPED_AFTER_BLOCKED"),
            ],
        )

    def test_keep_going_passes_only_when_every_case_passes(self) -> None:
        code, value, reason = self.run_cases(({}, {}, {}), keep_going=True)

        self.assertEqual((code, value["status"], reason), (0, "PASS", "ALL_REQUIRED_CASES"))

    def test_fail_outranks_blocked_and_pass(self) -> None:
        self.assertEqual(runtime.worst_returncode(0, 2), 2)
        self.assertEqual(runtime.worst_returncode(2, 1, 0), 1)
        self.assertEqual(runtime.worst_returncode(0, 0), 0)


class SessionFake:
    """The real preflight's boundaries: hash + environment, Win32, clipboard."""

    def __init__(self, clipboard: str | None = "user clipboard", restore_fails: bool = False) -> None:
        self.events: list[object] = []
        self.clipboard = clipboard
        self.restore_fails = restore_fails
        fake = self

        class Win32:
            def keep_display_awake(self) -> None:
                fake.events.append("awake")

            def allow_display_sleep(self) -> None:
                fake.events.append("sleep")

        self.win32 = Win32()

    def preflight(self, exe: Path, expected: str, scratch: dict, _fingerprint=None):
        self.events.append("preflight")
        scratch["executable"].update(sha256=expected, hash_verified=True)
        scratch["environment"] = {"input_desktop": "Default"}
        return self.win32, runtime.SecurityContext(1, 0x2000, "medium")

    def read(self) -> str | None:
        self.events.append("save-clipboard")
        return self.clipboard

    def write(self, value: str | None) -> None:
        self.events.append(("restore-clipboard", value))
        if self.restore_fails:
            raise runtime.HarnessBlocked("CLIPBOARD_UNAVAILABLE")

    def patches(self) -> contextlib.ExitStack:
        stack = contextlib.ExitStack()
        stack.enter_context(mock.patch.object(runtime, "_preflight", self.preflight))
        stack.enter_context(mock.patch.object(runtime, "read_clipboard_text", self.read))
        stack.enter_context(mock.patch.object(runtime, "write_clipboard_text", self.write))
        return stack


class NativeSessionTests(unittest.TestCase):
    def test_a_single_goal_run_saves_and_restores_the_clipboard_once(self) -> None:
        fake = SessionFake()
        observed: list[object] = []

        def scenario_events() -> dict:
            observed.append(list(fake.events))
            return {}

        plan = synthetic_plan(({}, {}, {}), preflight=runtime.preflight)
        plan = dataclasses.replace(
            plan, scenarios=lambda _harness: (scenario_events,) * len(SYNTHETIC_CASES)
        )
        with tempfile.TemporaryDirectory() as temporary, fake.patches():
            exe = Path(temporary) / "markturbo.exe"
            exe.write_bytes(b"test executable")
            code, value, reason = runtime.run_native_acceptance(synthetic_args(exe), plan)

        self.assertEqual((code, reason), (0, "ALL_REQUIRED_CASES"))
        self.assertEqual(observed[0], ["preflight", "awake", "save-clipboard"])
        self.assertEqual(
            fake.events,
            ["preflight", "awake", "save-clipboard", ("restore-clipboard", "user clipboard"), "sleep"],
        )
        self.assertEqual(value["environment"], {"input_desktop": "Default"})
        self.assertIsNone(runtime._ACTIVE_SESSION.get())

    def test_a_clipboard_that_cannot_be_restored_fails_the_run(self) -> None:
        fake = SessionFake(restore_fails=True)
        with tempfile.TemporaryDirectory() as temporary, fake.patches():
            exe = Path(temporary) / "markturbo.exe"
            exe.write_bytes(b"test executable")
            code, value, reason = runtime.run_native_acceptance(
                synthetic_args(exe), synthetic_plan(({}, {}, {}), preflight=runtime.preflight)
            )

        self.assertEqual((code, value["status"], reason), (1, "FAIL", "CLIPBOARD_RESTORE_FAILED"))
        self.assertEqual(value["cases"][-1]["reason_code"], "CLIPBOARD_RESTORE_FAILED")
        self.assertIn("sleep", fake.events)

    def test_a_non_text_clipboard_blocks_before_any_case(self) -> None:
        fake = SessionFake()

        def non_text() -> str | None:
            raise runtime.HarnessBlocked("CLIPBOARD_CONTAINS_NON_TEXT")

        with tempfile.TemporaryDirectory() as temporary, fake.patches():
            exe = Path(temporary) / "markturbo.exe"
            exe.write_bytes(b"test executable")
            with mock.patch.object(runtime, "read_clipboard_text", non_text):
                code, value, reason = runtime.run_native_acceptance(
                    synthetic_args(exe), synthetic_plan(({}, {}, {}), preflight=runtime.preflight)
                )

        self.assertEqual((code, value["status"], reason), (2, "BLOCKED", "CLIPBOARD_CONTAINS_NON_TEXT"))
        # Nothing was saved, so nothing is written back; the display is released.
        self.assertEqual(fake.events, ["preflight", "awake", "sleep"])

    def test_a_suite_preflights_copies_and_saves_the_clipboard_once(self) -> None:
        fake = SessionFake()
        copies: list[Path] = []
        real_copy = runtime.shutil.copy2

        def copy2(source, target):
            copies.append(Path(target))
            return real_copy(source, target)

        with tempfile.TemporaryDirectory() as temporary, fake.patches():
            root = Path(temporary)
            exe = root / "markturbo.exe"
            exe.write_bytes(b"test executable")
            runs = [
                (
                    name,
                    synthetic_args(exe, root / f"{name}.json"),
                    synthetic_plan(({}, {}, {}), preflight=runtime.preflight),
                )
                for name in ("goal-a", "goal-b")
            ]
            stdout, stderr = io.StringIO(), io.StringIO()
            with (
                mock.patch.object(runtime.shutil, "copy2", copy2),
                contextlib.redirect_stdout(stdout),
                contextlib.redirect_stderr(stderr),
            ):
                code = runtime.main_native_acceptance_suite(runs, keep_going=False)

            values = [json.loads((root / f"{name}.json").read_text("utf-8")) for name in ("goal-a", "goal-b")]
            self.assertFalse(copies[0].parent.exists())

        self.assertEqual(code, 0)
        self.assertEqual(
            fake.events,
            ["preflight", "awake", "save-clipboard", ("restore-clipboard", "user clipboard"), "sleep"],
        )
        self.assertEqual(len(copies), 1)
        for value in values:
            self.assertEqual(value["status"], "PASS")
            self.assertTrue(value["executable"]["copy_hash_verified"])
        self.assertIn("overall: PASS", stdout.getvalue())

    def run_suite(self, first: tuple[object, ...], keep_going: bool) -> tuple[int, list[dict]]:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            exe = root / "markturbo.exe"
            exe.write_bytes(b"test executable")
            runs = [
                ("goal-a", synthetic_args(exe, root / "a.json"), synthetic_plan(first)),
                ("goal-b", synthetic_args(exe, root / "b.json"), synthetic_plan(({}, {}, {}))),
            ]
            with contextlib.redirect_stdout(io.StringIO()), contextlib.redirect_stderr(io.StringIO()):
                code = runtime.main_native_acceptance_suite(runs, keep_going=keep_going)
            return code, [json.loads((root / name).read_text("utf-8")) for name in ("a.json", "b.json")]

    def test_a_suite_stops_after_a_failed_goal_unless_keep_going(self) -> None:
        failing = (runtime.HarnessFailure("FIRST_FAILED"), {}, {})

        code, (first, second) = self.run_suite(failing, keep_going=False)
        self.assertEqual((code, first["status"], second["status"]), (1, "FAIL", "FAIL"))
        self.assertEqual(
            {case["reason_code"] for case in second["cases"]}, {"SKIPPED_AFTER_FAILURE"}
        )
        self.assertEqual(second["summary"]["not_run_case_count"], len(SYNTHETIC_CASES))

        code, (first, second) = self.run_suite(failing, keep_going=True)
        self.assertEqual((code, first["status"], second["status"]), (1, "FAIL", "PASS"))
        self.assertEqual(first["cases"][1]["status"], "PASS")

    def test_a_suite_always_stops_after_a_blocked_goal(self) -> None:
        code, (first, second) = self.run_suite(
            (runtime.HarnessBlocked("SESSION_NOT_ACTIVE"), {}, {}), keep_going=True
        )

        self.assertEqual((code, first["status"], second["status"]), (2, "BLOCKED", "BLOCKED"))
        self.assertEqual(
            {case["reason_code"] for case in second["cases"]}, {"SKIPPED_AFTER_BLOCKED"}
        )
