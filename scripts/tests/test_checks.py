"""Tests for the canonical validation command resolver."""

from __future__ import annotations

import json
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

    def test_explicit_tooling_manifest_includes_goal06_native_harness(self) -> None:
        self.assertIn("scripts.tests.test_native_goal06", checks.TOOLING_TESTS)

    def test_explicit_tooling_manifest_includes_goal07_revision_evaluation(self) -> None:
        self.assertIn("scripts.tests.test_revision_evaluation", checks.TOOLING_TESTS)

    def test_explicit_tooling_manifest_includes_goal07_native_harness(self) -> None:
        self.assertIn("scripts.tests.test_native_goal07", checks.TOOLING_TESTS)

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
                ("cargo", "clippy", "--profile", "ci", "--workspace", "--all-targets", "--locked"),
                ("cargo", "test", "--profile", "ci", "--workspace", "--locked"),
            ],
        )


class ValidationBoundaryTests(unittest.TestCase):
    def test_doc_tier_never_requests_desktop_or_workspace_builds(self) -> None:
        with (
            mock.patch.object(checks, "fast") as fast,
            mock.patch.object(checks, "cargo", side_effect=lambda *args: ("cargo", *args)),
            mock.patch.object(checks, "run") as run,
        ):
            checks.run_check("doc", base="base", head="head")
        fast.assert_called_once_with(base="base", head="head")
        commands = [call.args[0] for call in run.call_args_list]
        tests = [command for command in commands if command[1] == "test"]
        self.assertEqual(len(tests), 1)
        self.assertEqual(tests[0][tests[0].index("-p") + 1], "mt-doc")
        self.assertIn("--locked", tests[0])
        self.assertFalse(any("--workspace" in command or "build" in command for command in commands))

    def test_ci_stops_before_tests_when_clippy_fails(self) -> None:
        calls = []

        def run(command: tuple[str, ...]) -> None:
            calls.append(command)
            if "clippy" in command:
                raise checks.CheckFailure("lint failed")

        with (
            mock.patch.object(checks, "fast"),
            mock.patch.object(checks, "cargo", side_effect=lambda *args: ("cargo", *args)),
            mock.patch.object(checks, "run", side_effect=run),
        ):
            with self.assertRaisesRegex(checks.CheckFailure, "lint failed"):
                checks.ci()
        self.assertFalse(any("test" in command for command in calls))


class ReleaseArtifactTests(unittest.TestCase):
    def test_missing_artifact_never_falls_back_to_a_stale_default_binary(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            stale = root / "target" / "release" / "markturbo"
            stale.parent.mkdir(parents=True)
            stale.write_bytes(b"old build")
            with (
                mock.patch.object(checks, "ROOT", root),
                mock.patch.object(checks, "fast"),
                mock.patch.object(checks, "rust_checks"),
                mock.patch.object(checks, "cargo", side_effect=lambda *args: ("cargo", *args)),
                mock.patch.object(checks, "run", return_value='{"reason":"build-finished","success":true}'),
                mock.patch.object(checks.privacy, "scan") as scan,
            ):
                with self.assertRaisesRegex(checks.CheckFailure, "exactly one"):
                    checks.full()
            scan.assert_not_called()

    def test_build_failure_does_not_scan_an_old_artifact(self) -> None:
        with (
            mock.patch.object(checks, "fast"),
            mock.patch.object(checks, "rust_checks"),
            mock.patch.object(checks, "cargo", side_effect=lambda *args: ("cargo", *args)),
            mock.patch.object(checks, "run", side_effect=checks.CheckFailure("build failed")),
            mock.patch.object(checks.privacy, "scan") as scan,
        ):
            with self.assertRaisesRegex(checks.CheckFailure, "build failed"):
                checks.full()
        scan.assert_not_called()

    def test_invalid_or_ambiguous_artifacts_fail_closed(self) -> None:
        def artifact(path: str) -> str:
            return json.dumps({
                "reason": "compiler-artifact",
                "target": {"name": "markturbo", "kind": ["bin"]},
                "executable": path,
            })

        for output in ("not JSON", artifact("a") + "\n" + artifact("b")):
            with (
                self.subTest(output=output),
                mock.patch.object(checks, "cargo", side_effect=lambda *args: ("cargo", *args)),
                mock.patch.object(checks, "run", return_value=output),
            ):
                with self.assertRaises(checks.CheckFailure):
                    checks.build_release_binary()


if __name__ == "__main__":
    unittest.main()
