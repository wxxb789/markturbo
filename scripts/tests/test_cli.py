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
