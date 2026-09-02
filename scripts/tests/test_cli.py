"""Integration tests for the canonical MarkTurbo tooling CLI."""

from __future__ import annotations

import subprocess
import sys
import unittest
from contextlib import redirect_stderr
from io import StringIO
from pathlib import Path
from unittest import mock

from scripts.markturbo_tools import cli


ROOT = Path(__file__).resolve().parents[2]


class ToolingCliTests(unittest.TestCase):
    def test_native_module_is_invocable_through_the_cli(self) -> None:
        result = subprocess.run(
            [sys.executable, "scripts/mt.py", "accept", "goal-03", "--", "--help"],
            cwd=ROOT,
            check=False,
            capture_output=True,
            text=True,
        )

        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn("Exercise Goal 03", result.stdout)

    def test_delegated_modules_run_from_the_repository_root(self) -> None:
        completed = subprocess.CompletedProcess([], 0)
        with mock.patch.object(cli.subprocess, "run", return_value=completed) as run:
            self.assertEqual(
                cli.main(["probe", "--", "startup", "--exe", "target/release/markturbo.exe"]),
                0,
            )

        command = run.call_args.args[0]
        self.assertEqual(command[1:3], ["-m", "scripts.markturbo_tools.probe"])
        self.assertEqual(command[-2:], ["--exe", "target/release/markturbo.exe"])
        self.assertEqual(run.call_args.kwargs["cwd"], ROOT)

    def test_capacity_is_dispatched_through_the_cli(self) -> None:
        completed = subprocess.CompletedProcess([], 0)
        with mock.patch.object(cli.subprocess, "run", return_value=completed) as run:
            self.assertEqual(cli.main(["capacity", "--", "--invocations", "1"]), 0)

        command = run.call_args.args[0]
        self.assertEqual(command[1:3], ["-m", "scripts.markturbo_tools.recovery_capacity"])
        self.assertEqual(command[-2:], ["--invocations", "1"])

    def test_check_forwards_the_explicit_ci_range(self) -> None:
        with mock.patch.object(cli.checks, "run_check") as run_check:
            self.assertEqual(
                cli.main(["check", "fast", "--base", "base-sha", "--head", "head-sha"]),
                0,
            )

        run_check.assert_called_once_with("fast", base="base-sha", head="head-sha")

    def test_accept_preserves_native_pass_fail_and_blocked_statuses(self) -> None:
        for expected in (0, 1, 2):
            with self.subTest(expected=expected), mock.patch.object(cli, "run_module", return_value=expected):
                self.assertEqual(cli.main(["accept", "goal-02", "--", "--help"]), expected)

    def test_accept_converts_an_unexpected_child_status_to_failure(self) -> None:
        with mock.patch.object(cli, "run_module", return_value=17), redirect_stderr(StringIO()):
            self.assertEqual(cli.main(["accept", "goal-03"]), 1)
