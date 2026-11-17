"""Behavioral tests for the canonical tooling CLI's exit contracts."""

from __future__ import annotations

import json
import subprocess
import sys
import tempfile
import unittest
from contextlib import redirect_stderr
from io import StringIO
from pathlib import Path
from unittest import mock

from scripts.markturbo_tools import cli


ROOT = Path(__file__).resolve().parents[2]


class ToolingCliTests(unittest.TestCase):
    def test_goal07_accept_reports_and_persists_a_preflight_failure(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            evidence_path = root / "goal07-evidence.json"
            result = subprocess.run(
                [
                    sys.executable,
                    "scripts/mt.py",
                    "accept",
                    "goal-07",
                    "--",
                    "--exe",
                    str(root / "missing.exe"),
                    "--expect-exe-sha256",
                    "0" * 64,
                    "--evidence",
                    str(evidence_path),
                ],
                cwd=ROOT,
                capture_output=True,
                text=True,
                check=False,
            )

            self.assertEqual(result.returncode, 1, result.stderr)
            self.assertIn("FAIL: EXECUTABLE_MISSING", result.stderr)
            evidence = json.loads(evidence_path.read_text(encoding="utf-8"))
            self.assertEqual(evidence["schema"], "markturbo.goal-07-native-acceptance")
            self.assertEqual(evidence["status"], "FAIL")
            self.assertEqual(evidence["transport"]["request_count"], 0)
            self.assertTrue(
                all(
                    case["status"] == "NOT_RUN"
                    and case["reason_code"] == "EXECUTABLE_MISSING"
                    for case in evidence["cases"]
                )
            )

    def test_accept_all_writes_one_evidence_file_per_goal_and_stops_after_a_failure(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            result = subprocess.run(
                [
                    sys.executable,
                    "scripts/mt.py",
                    "accept",
                    "all",
                    "--exe",
                    str(root / "missing.exe"),
                    "--expect-exe-sha256",
                    "0" * 64,
                    "--evidence-dir",
                    str(root),
                ],
                cwd=ROOT,
                capture_output=True,
                text=True,
                check=False,
            )

            self.assertEqual(result.returncode, 1, result.stderr)
            self.assertLess(result.stdout.index("== goal-06"), result.stdout.index("== goal-03"))
            self.assertIn("overall: FAIL", result.stderr)
            reasons = {}
            for goal in cli.ACCEPT_ALL_ORDER:
                value = json.loads((root / f"{goal}-native-acceptance.json").read_text("utf-8"))
                self.assertEqual(value["schema"], f"markturbo.{goal}-native-acceptance")
                self.assertEqual(value["executable"]["expected_sha256"], "0" * 64)
                self.assertEqual(value["status"], "FAIL")
                reasons[goal] = {case["reason_code"] for case in value["cases"]}

        self.assertEqual(cli.ACCEPT_ALL_ORDER, ("goal-06", "goal-02", "goal-07", "goal-03"))
        self.assertEqual(reasons["goal-06"], {"EXECUTABLE_MISSING"})
        for goal in ("goal-02", "goal-07", "goal-03"):
            self.assertEqual(reasons[goal], {"SKIPPED_AFTER_FAILURE"})

    def test_accept_all_runs_each_goal_once_in_order_with_the_shared_options(self) -> None:
        with mock.patch(
            "scripts.markturbo_tools.native.runtime.main_native_acceptance_suite", return_value=2
        ) as suite:
            result = cli.main(
                [
                    "accept",
                    "all",
                    "--expect-exe-sha256",
                    "A" * 64,
                    "--evidence-dir",
                    "receipts",
                    "--ui-timeout",
                    "20",
                    "--keep-going",
                ]
            )

        self.assertEqual(result, 2)
        (runs,), options = suite.call_args
        self.assertEqual(options, {"keep_going": True})
        self.assertEqual([name for name, _args, _plan in runs], list(cli.ACCEPT_ALL_ORDER))
        for name, args, plan in runs:
            self.assertEqual(args.expect_exe_sha256, "a" * 64)
            self.assertEqual(args.ui_timeout, 20.0)
            self.assertEqual(args.evidence, Path("receipts") / f"{name}-native-acceptance.json")
            self.assertTrue(plan.required_case_ids)

    def test_check_failure_returns_one_and_reports_the_error(self) -> None:
        stderr = StringIO()
        with (
            mock.patch.object(
                cli.checks,
                "run_check",
                side_effect=cli.checks.CheckFailure("validation failed"),
            ),
            redirect_stderr(stderr),
        ):
            result = cli.main(["check", "fast"])

        self.assertEqual(result, 1)
        self.assertEqual(stderr.getvalue(), "error: validation failed\n")

    def test_native_exit_code_preserves_pass_fail_and_blocked_statuses(self) -> None:
        for expected in (0, 1, 2):
            with self.subTest(expected=expected):
                self.assertEqual(cli.native_exit_code(expected), expected)

    def test_native_exit_code_maps_unexpected_status_to_failure(self) -> None:
        stderr = StringIO()
        with redirect_stderr(stderr):
            result = cli.native_exit_code(17)

        self.assertEqual(result, 1)
        self.assertIn("unexpectedly with 17", stderr.getvalue())
