"""Tests for the canonical validation command resolver."""

from __future__ import annotations

import tempfile
import sys
import unittest
from pathlib import Path
from unittest import mock

from scripts.markturbo_tools import checks


class CargoResolutionTests(unittest.TestCase):
    def test_prefers_cargo_from_path(self) -> None:
        with mock.patch.object(checks.shutil, "which", return_value="toolchain/cargo"):
            self.assertEqual(checks.cargo("test"), ("toolchain/cargo", "test"))

    def test_falls_back_to_the_standard_cargo_home(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            fallback = Path(temporary) / "cargo.exe"
            fallback.write_bytes(b"")
            with (
                mock.patch.object(checks.shutil, "which", return_value=None),
                mock.patch.object(checks, "CARGO_FALLBACK", fallback),
            ):
                self.assertEqual(checks.cargo("fmt"), (str(fallback), "fmt"))

    def test_reports_when_cargo_is_unavailable(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            missing = Path(temporary) / "cargo.exe"
            with (
                mock.patch.object(checks.shutil, "which", return_value=None),
                mock.patch.object(checks, "CARGO_FALLBACK", missing),
            ):
                with self.assertRaisesRegex(checks.CheckFailure, "cargo was not found"):
                    checks.cargo("test")


class DiffCheckTests(unittest.TestCase):
    def test_checks_both_unstaged_and_staged_diffs(self) -> None:
        with mock.patch.dict(checks.os.environ, {}, clear=True), mock.patch.object(checks, "run") as run:
            checks.check_diff()

        self.assertEqual(
            [call.args[0] for call in run.call_args_list],
            [
                ("git", "diff", "--check"),
                ("git", "diff", "--cached", "--check"),
            ],
        )

    def test_checks_an_explicit_base_head_range_without_using_the_local_index(self) -> None:
        with mock.patch.object(checks, "run") as run:
            checks.check_diff(base="base-sha", head="head-sha")

        self.assertEqual(
            [call.args[0] for call in run.call_args_list],
            [
                ("git", "diff", "--check", "base-sha", "head-sha"),
            ],
        )

    def test_reads_the_ci_revision_range_from_the_environment(self) -> None:
        self.assertEqual(
            checks.diff_range(environment={"BASE_SHA": "base-sha", "HEAD_SHA": "head-sha"}),
            ("base-sha", "head-sha"),
        )

    def test_explicit_tooling_manifest_includes_the_cli_integration_tests(self) -> None:
        self.assertIn("scripts.tests.test_cli", checks.TOOLING_TESTS)

    def test_explicit_tooling_manifest_collects_each_goal02_module_once(self) -> None:
        expected = {
            "scripts.tests.test_native_goal02_evidence",
            "scripts.tests.test_native_goal02_uia",
            "scripts.tests.test_native_goal02_runtime",
            "scripts.tests.test_native_goal02_execution",
        }

        self.assertTrue(expected.issubset(checks.TOOLING_TESTS))
        self.assertNotIn("scripts.tests.test_native_goal02", checks.TOOLING_TESTS)
        self.assertEqual(sum(name.startswith("scripts.tests.test_native_goal02_") for name in checks.TOOLING_TESTS), 4)

    def test_rejects_an_incomplete_ci_range_before_running_git(self) -> None:
        with mock.patch.object(checks, "run") as run:
            with self.assertRaisesRegex(checks.CheckFailure, "must be provided together"):
                checks.check_diff(base="base-sha", head=None)

        run.assert_not_called()

    def test_ci_forwards_the_explicit_range_and_omits_local_diff_checks(self) -> None:
        with (
            mock.patch.object(checks, "run") as run,
            mock.patch.object(checks, "cargo", side_effect=lambda *args: ("cargo", *args)),
        ):
            checks.ci(base="base-sha", head="head-sha")

        commands = [call.args[0] for call in run.call_args_list]
        self.assertEqual(commands[0], ("git", "diff", "--check", "base-sha", "head-sha"))
        self.assertNotIn(("git", "diff", "--check"), commands)
        self.assertNotIn(("git", "diff", "--cached", "--check"), commands)
        self.assertEqual(
            commands[1:],
            [
                (sys.executable, "-m", "unittest", *checks.TOOLING_TESTS),
                ("cargo", "fmt", "--all", "--", "--check"),
                ("cargo", "clippy", "--workspace", "--all-targets", "--locked"),
                ("cargo", "test", "--release", "--workspace", "--locked"),
            ],
        )


if __name__ == "__main__":
    unittest.main()
