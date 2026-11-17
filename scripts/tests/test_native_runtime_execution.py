"""The shared native acceptance lifecycle, run through goal 02 with fake harnesses."""

from __future__ import annotations

import contextlib
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
