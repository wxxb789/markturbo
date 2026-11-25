"""Tests for the canonical validation command resolver."""

from __future__ import annotations

import json
import re
import tempfile
import tomllib
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

    def test_rejects_an_incomplete_ci_range_before_running_git(self) -> None:
        with mock.patch.object(checks, "run") as run:
            with self.assertRaisesRegex(checks.CheckFailure, "must be provided together"):
                checks.check_diff(base="base-sha", head=None)

        run.assert_not_called()


class ValidationBoundaryTests(unittest.TestCase):
    def test_current_and_native_tooling_suites_do_not_overlap(self) -> None:
        self.assertFalse(set(checks.TOOLING_TESTS) & set(checks.NATIVE_TOOLING_TESTS))

    def test_shards_are_deterministic_and_cover_every_test_once(self) -> None:
        ids = [f"tests.test_{index}" for index in range(17)]
        shards = checks.test_shards(ids, 5)

        self.assertEqual([len(shard) for shard in shards], [4, 4, 3, 3, 3])
        self.assertEqual(sorted(item for shard in shards for item in shard), sorted(ids))
        self.assertEqual(shards, checks.test_shards(ids, 5))

    def test_native_tier_runs_only_native_harness_tests(self) -> None:
        with (
            mock.patch.object(checks, "check_diff") as diff,
            mock.patch.object(checks, "run_tooling_suite") as suite,
        ):
            checks.native()

        diff.assert_called_once_with(base=None, head=None)
        suite.assert_called_once_with(checks.NATIVE_TOOLING_TESTS)

    def test_ci_stops_before_any_nextest_run_when_clippy_fails(self) -> None:
        calls: list[tuple[str, ...]] = []

        def run(command: tuple[str, ...], **_: object) -> None:
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

        self.assertEqual(
            calls,
            [
                ("cargo", "fmt", "--all", "--", "--check"),
                ("cargo", "nextest", "--version"),
                (
                    "cargo", "clippy", "--profile", "ci", "--workspace",
                    "--all-targets", "--locked",
                ),
            ],
        )


class RustCheckTests(unittest.TestCase):
    def run_rust_checks(self, profile: str) -> list[tuple[str, ...]]:
        calls: list[tuple[str, ...]] = []

        def record(command: tuple[str, ...], **_: object) -> None:
            calls.append(command)

        with (
            mock.patch.object(checks, "cargo", side_effect=lambda *args: ("cargo", *args)),
            mock.patch.object(checks, "run", side_effect=record),
        ):
            checks.rust_checks(profile)
        return calls

    def test_rust_steps_run_in_contract_order_with_the_requested_profile(self) -> None:
        for profile in ("ci", "release"):
            with self.subTest(profile=profile):
                self.assertEqual(
                    self.run_rust_checks(profile),
                    [
                        ("cargo", "fmt", "--all", "--", "--check"),
                        ("cargo", "nextest", "--version"),
                        (
                            "cargo", "clippy", "--profile", profile, "--workspace",
                            "--all-targets", "--locked",
                        ),
                        (
                            "cargo", "nextest", "run", "--locked",
                            "--cargo-profile", profile, "-p", "mt-core",
                            "--features", "model-transport,test-support",
                        ),
                        (
                            "cargo", "nextest", "run", "--locked",
                            "--cargo-profile", profile, "-p", "mt-app",
                            "--lib", "--bins", "--test", "suite",
                        ),
                    ],
                )

    def test_mt_app_runs_as_one_suite_binary_under_nextest(self) -> None:
        commands = self.run_rust_checks("ci")

        self.assertIn(
            (
                "cargo", "nextest", "run", "--locked", "--cargo-profile", "ci",
                "-p", "mt-app", "--lib", "--bins", "--test", "suite",
            ),
            commands,
        )

    def test_mt_core_keeps_the_workspace_feature_set_under_nextest(self) -> None:
        commands = self.run_rust_checks("ci")

        self.assertIn(
            (
                "cargo", "nextest", "run", "--locked", "--cargo-profile", "ci",
                "-p", "mt-core", "--features", "model-transport,test-support",
            ),
            commands,
        )

    def test_no_plain_cargo_test_remains_in_the_rust_gate(self) -> None:
        for profile in ("ci", "release"):
            with self.subTest(profile=profile):
                commands = self.run_rust_checks(profile)
                self.assertFalse(
                    [command for command in commands if command[1] == "test"]
                )

    def test_full_uses_the_release_profile(self) -> None:
        with (
            mock.patch.object(checks, "fast"),
            mock.patch.object(checks, "rust_checks") as rust_checks,
            mock.patch.object(
                checks, "build_release_binary", side_effect=checks.CheckFailure("stop")
            ),
        ):
            with self.assertRaisesRegex(checks.CheckFailure, "stop"):
                checks.full()

        rust_checks.assert_called_once_with("release")

    def test_nextest_is_detected_through_cargo(self) -> None:
        with (
            mock.patch.object(checks, "cargo", side_effect=lambda *args: ("cargo", *args)),
            mock.patch.object(checks, "run") as run,
        ):
            checks.require_nextest()

        run.assert_called_once_with(
            ("cargo", "nextest", "--version"), capture_stdout=True
        )

    def test_missing_nextest_fails_with_the_install_command(self) -> None:
        with (
            mock.patch.object(checks, "cargo", side_effect=lambda *args: ("cargo", *args)),
            mock.patch.object(checks, "run", side_effect=checks.CheckFailure("no such command")),
        ):
            with self.assertRaisesRegex(
                checks.CheckFailure, "cargo install cargo-nextest --locked"
            ):
                checks.require_nextest()


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


class DoctestGateTests(unittest.TestCase):
    def test_every_enabled_workspace_doctest_has_a_gate_step(self) -> None:
        with mock.patch.object(checks, "cargo", side_effect=lambda *args: ("cargo", *args)):
            gate_runs_doctests = any(
                "--doc" in command for command in checks.rust_commands("ci")
            )

        root = checks.ROOT
        members = tomllib.loads((root / "Cargo.toml").read_text(encoding="utf-8"))[
            "workspace"
        ]["members"]
        libraries: list[tuple[str, bool]] = []
        for member in members:
            directory = root / member
            manifest = tomllib.loads(
                (directory / "Cargo.toml").read_text(encoding="utf-8")
            )
            if "lib" in manifest or (directory / "src" / "lib.rs").is_file():
                libraries.append(
                    (
                        manifest["package"]["name"],
                        manifest.get("lib", {}).get("doctest", True),
                    )
                )

        self.assertTrue(libraries)
        enabled = [name for name, doctests in libraries if doctests]
        if enabled and not gate_runs_doctests:
            self.fail(
                f"{', '.join(enabled)} enable doctests "
                "but rust_commands never passes --doc"
            )


class SuiteTests(unittest.TestCase):
    def test_suite_includes_every_app_integration_test(self) -> None:
        # The app suites build only `suite`, so a test file it does not name
        # would silently drop out of them.
        tests = checks.ROOT / "crates" / "mt-app" / "tests"
        suite = (tests / "suite.rs").read_text(encoding="utf-8")
        included = set(re.findall(r'^#\[path = "([^"]+)"\]\nmod \w+;$', suite, re.MULTILINE))
        files = {path.name for path in tests.glob("*.rs")} - {"suite.rs"}
        self.assertEqual(included, files)


if __name__ == "__main__":
    unittest.main()
