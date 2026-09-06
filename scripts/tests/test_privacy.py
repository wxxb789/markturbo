"""Tests for repository and release-binary privacy scans."""

from __future__ import annotations

import os
import subprocess
import sys
import tempfile
import unittest
from contextlib import nullcontext
from pathlib import Path
from unittest import mock

from scripts.markturbo_tools import checks, privacy


def init_repository(root: Path) -> None:
    subprocess.run(
        ("git", "init", "--quiet"),
        cwd=root,
        check=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
    )


def track(root: Path, relative_path: str, content: bytes) -> Path:
    path = root / relative_path
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_bytes(content)
    subprocess.run(
        ("git", "add", "--", relative_path),
        cwd=root,
        check=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
    )
    return path


class PrivacyScanTests(unittest.TestCase):
    def repository(self) -> Path:
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        root = Path(temporary.name)
        init_repository(root)
        return root

    def test_reports_a_secret_in_a_tracked_repository_file_without_disclosing_it(self) -> None:
        root = self.repository()
        sentinel = "repo-" + "secret-value"
        track(root, "tracked.bin", b"prefix\x00" + sentinel.encode() + b"\xffsuffix")
        binary = root / "target" / "release" / "markturbo.exe"
        binary.parent.mkdir(parents=True)
        binary.write_bytes(b"clean binary")

        with self.assertRaises(privacy.PrivacyScanError) as raised:
            privacy.scan(root, binary, environment={"OPENAI_API_KEY": sentinel})

        message = str(raised.exception)
        self.assertIn("OPENAI_API_KEY: tracked.bin", message)
        self.assertFalse(sentinel in message, "privacy scan error disclosed a candidate value")

    def test_reports_a_request_body_sentinel_in_the_release_binary(self) -> None:
        root = self.repository()
        sentinel = "private-" + "request-body"
        track(root, "tracked.txt", b"clean repository content")
        binary = root / "target" / "release" / "markturbo.exe"
        binary.parent.mkdir(parents=True)
        binary.write_bytes(b"prefix\x00" + sentinel.encode() + b"\xffsuffix")

        with self.assertRaises(privacy.PrivacyScanError) as raised:
            privacy.scan(
                root,
                binary,
                environment={"MARKTURBO_PRIVATE_REQUEST_SENTINEL": sentinel},
            )

        message = str(raised.exception)
        self.assertIn(
            "MARKTURBO_PRIVATE_REQUEST_SENTINEL: target/release/markturbo.exe",
            message,
        )
        self.assertFalse(sentinel in message, "privacy scan error disclosed a candidate value")

    def test_reports_a_secret_in_an_untracked_non_ignored_repository_file(self) -> None:
        root = self.repository()
        sentinel = "untracked-" + "secret-value"
        track(root, "tracked.txt", b"clean repository content")
        (root / "untracked.bin").write_bytes(sentinel.encode())
        binary = root / "target" / "release" / "markturbo.exe"
        binary.parent.mkdir(parents=True)
        binary.write_bytes(b"clean binary")

        with self.assertRaises(privacy.PrivacyScanError) as raised:
            privacy.scan(root, binary, environment={"ANTHROPIC_API_KEY": sentinel})

        message = str(raised.exception)
        self.assertIn("ANTHROPIC_API_KEY: untracked.bin", message)
        self.assertFalse(sentinel in message, "privacy scan error disclosed a candidate value")

    def test_redacts_overlapping_candidates_without_disclosing_a_suffix(self) -> None:
        root = self.repository()
        short = "overlapping-secret"
        long = f"{short}-private-suffix"
        track(root, f"artifact-{long}.txt", long.encode())
        binary = root / "target" / "release" / "markturbo.exe"
        binary.parent.mkdir(parents=True)
        binary.write_bytes(b"clean binary")

        with self.assertRaises(privacy.PrivacyScanError) as raised:
            privacy.scan(
                root,
                binary,
                environment={
                    "OPENAI_API_KEY": short,
                    "MARKTURBO_PRIVACY_SENTINEL": long,
                },
            )

        message = str(raised.exception)
        self.assertNotIn(short, message)
        self.assertNotIn("private-suffix", message)

    def test_redacts_partially_overlapping_candidates_as_one_range(self) -> None:
        root = self.repository()
        first = "private-prefix-overlap"
        second = "overlap-secret-tail"
        track(root, f"artifact-{first}-secret-tail.txt", b"clean repository content")
        binary = root / "target" / "release" / "markturbo.exe"
        binary.parent.mkdir(parents=True)
        binary.write_bytes(b"clean binary")

        with self.assertRaises(privacy.PrivacyScanError) as raised:
            privacy.scan(
                root,
                binary,
                environment={
                    "OPENAI_API_KEY": first,
                    "MARKTURBO_PRIVACY_SENTINEL": second,
                },
            )

        message = str(raised.exception)
        self.assertIn("artifact-<redacted>.txt", message)
        self.assertNotIn("private-prefix", message)
        self.assertNotIn("secret-tail", message)

    def test_reports_secret_bearing_paths_without_putting_them_in_process_arguments(self) -> None:
        root = self.repository()
        sentinel = "path-only-" + "secret-value"
        track(root, f"artifact-{sentinel}.txt", b"clean repository content")
        binary = root / "target" / "release" / "markturbo.exe"
        binary.parent.mkdir(parents=True)
        binary.write_bytes(b"clean binary")
        commands: list[tuple[str, ...]] = []
        run = subprocess.run

        def record(command: tuple[str, ...], **kwargs: object) -> subprocess.CompletedProcess[bytes]:
            commands.append(command)
            return run(command, **kwargs)

        with (
            mock.patch.object(privacy.subprocess, "run", side_effect=record),
            self.assertRaises(privacy.PrivacyScanError) as raised,
        ):
            privacy.scan(root, binary, environment={"OPENAI_API_KEY": sentinel})

        message = str(raised.exception)
        self.assertIn("OPENAI_API_KEY: artifact-<redacted>.txt", message)
        self.assertNotIn(sentinel, message)
        self.assertTrue(commands)
        self.assertTrue(
            all(sentinel not in argument for command in commands for argument in command)
        )

    def test_ignores_untracked_ignored_and_build_output_files(self) -> None:
        root = self.repository()
        sentinel = "excluded-" + "secret-value"
        track(root, ".gitignore", b"ignored/\ntarget/\n")
        track(root, "tracked.txt", b"clean repository content")
        build_output = root / "target" / "debug" / "private.bin"
        build_output.parent.mkdir(parents=True)
        build_output.write_bytes(sentinel.encode())
        ignored = root / "ignored" / "private.bin"
        ignored.parent.mkdir()
        ignored.write_bytes(sentinel.encode())
        binary = root / "target" / "release" / "markturbo.exe"
        binary.parent.mkdir(parents=True)
        binary.write_bytes(b"clean binary")

        privacy.scan(root, binary, environment={"ANTHROPIC_API_KEY": sentinel})

    def test_scans_tracked_build_output_paths(self) -> None:
        root = self.repository()
        sentinel = "tracked-target-" + "secret-value"
        track(root, "target/debug/private.bin", sentinel.encode())
        binary = root / "target" / "release" / "markturbo.exe"
        binary.parent.mkdir(parents=True, exist_ok=True)
        binary.write_bytes(b"clean binary")

        with self.assertRaises(privacy.PrivacyScanError) as raised:
            privacy.scan(root, binary, environment={"OPENAI_API_KEY": sentinel})

        self.assertIn("OPENAI_API_KEY: target/debug/private.bin", str(raised.exception))

    def test_scans_staged_content_even_when_the_worktree_copy_is_clean(self) -> None:
        root = self.repository()
        sentinel = "staged-only-" + "secret-value"
        path = track(root, "tracked.txt", sentinel.encode())
        path.write_bytes(b"clean worktree")
        binary = root / "target" / "release" / "markturbo.exe"
        binary.parent.mkdir(parents=True)
        binary.write_bytes(b"clean binary")

        with self.assertRaises(privacy.PrivacyScanError) as raised:
            privacy.scan(root, binary, environment={"OPENAI_API_KEY": sentinel})

        self.assertIn("OPENAI_API_KEY: tracked.txt", str(raised.exception))

    def test_scans_unstaged_content_in_a_tracked_file(self) -> None:
        root = self.repository()
        sentinel = "unstaged-only-" + "secret-value"
        path = track(root, "tracked.txt", b"clean index")
        path.write_bytes(sentinel.encode())
        binary = root / "target" / "release" / "markturbo.exe"
        binary.parent.mkdir(parents=True)
        binary.write_bytes(b"clean binary")

        with self.assertRaises(privacy.PrivacyScanError) as raised:
            privacy.scan(root, binary, environment={"OPENAI_API_KEY": sentinel})

        self.assertIn("OPENAI_API_KEY: tracked.txt", str(raised.exception))

    def test_scans_utf16_encoded_sentinels(self) -> None:
        root = self.repository()
        sentinel = "unicode-" + "secret-value"
        track(root, "tracked.txt", sentinel.encode("utf-16-le"))
        binary = root / "target" / "release" / "markturbo.exe"
        binary.parent.mkdir(parents=True)
        binary.write_bytes(b"clean binary")

        with self.assertRaises(privacy.PrivacyScanError) as raised:
            privacy.scan(root, binary, environment={"OPENAI_API_KEY": sentinel})

        self.assertIn("OPENAI_API_KEY: tracked.txt", str(raised.exception))

    def test_scans_tracked_dot_scratch_evidence(self) -> None:
        root = self.repository()
        sentinel = "tracked-evidence-" + "secret-value"
        track(root, ".scratch/evidence/private.bin", sentinel.encode())
        binary = root / "target" / "release" / "markturbo.exe"
        binary.parent.mkdir(parents=True)
        binary.write_bytes(b"clean binary")

        with self.assertRaises(privacy.PrivacyScanError) as raised:
            privacy.scan(root, binary, environment={"OPENAI_API_KEY": sentinel})

        self.assertIn("OPENAI_API_KEY: .scratch/evidence/private.bin", str(raised.exception))

    def test_does_not_follow_a_symlink_to_a_file_outside_the_repository(self) -> None:
        root = self.repository()
        sentinel = "external-" + "secret-value"
        track(root, "tracked.txt", b"clean repository content")
        external_temporary = tempfile.TemporaryDirectory()
        self.addCleanup(external_temporary.cleanup)
        external = Path(external_temporary.name) / "private.bin"
        external.write_bytes(sentinel.encode())
        link = root / "outside-link.bin"
        try:
            link.symlink_to(external)
        except OSError:
            link.write_bytes(b"placeholder")
            original_is_symlink = Path.is_symlink

            def is_symlink(path: Path) -> bool:
                return path == link or original_is_symlink(path)

            symlink_patch = (
                mock.patch.object(Path, "is_symlink", is_symlink),
                mock.patch.object(os, "readlink", return_value=str(external)),
            )
        else:
            symlink_patch = (nullcontext(), nullcontext())
        binary = root / "target" / "release" / "markturbo.exe"
        binary.parent.mkdir(parents=True)
        binary.write_bytes(b"clean binary")

        with symlink_patch[0], symlink_patch[1]:
            privacy.scan(root, binary, environment={"MARKTURBO_PRIVACY_SENTINEL": sentinel})

    def test_scans_the_symlink_blob_without_following_its_target(self) -> None:
        root = self.repository()
        sentinel = "symlink-blob-" + "secret-value"
        link = root / "tracked-link"
        link.write_text("placeholder")
        binary = root / "target" / "release" / "markturbo.exe"
        binary.parent.mkdir(parents=True)
        binary.write_bytes(b"clean binary")

        original_is_symlink = Path.is_symlink

        def is_symlink(path: Path) -> bool:
            return path == link or original_is_symlink(path)

        with (
            mock.patch.object(
                privacy,
                "_git_index_entries",
                return_value=[("synthetic-object-id", Path("tracked-link"))],
            ),
            mock.patch.object(privacy, "_git_paths", return_value=[]),
            mock.patch.object(
                privacy, "_scan_index_entry", return_value=[]
            ),
            mock.patch.object(Path, "is_symlink", is_symlink),
            mock.patch.object(os, "readlink", return_value=sentinel),
            self.assertRaises(privacy.PrivacyScanError) as raised,
        ):
            privacy.scan(root, binary, environment={"MARKTURBO_PRIVACY_SENTINEL": sentinel})

        message = str(raised.exception)
        self.assertIn("MARKTURBO_PRIVACY_SENTINEL: tracked-link", message)
        self.assertNotIn(sentinel, message)

    def test_scans_a_gitlink_path_without_reading_submodule_content(self) -> None:
        root = self.repository()
        sentinel = "gitlink-" + "secret-value"
        binary = root / "target" / "release" / "markturbo.exe"
        binary.parent.mkdir(parents=True)
        binary.write_bytes(b"clean binary")

        with (
            mock.patch.object(
                privacy,
                "_git_index_entries",
                return_value=[(None, Path(f"submodule-{sentinel}"))],
            ),
            mock.patch.object(privacy, "_git_paths", return_value=[]),
            self.assertRaises(privacy.PrivacyScanError) as raised,
        ):
            privacy.scan(root, binary, environment={"OPENAI_API_KEY": sentinel})

        message = str(raised.exception)
        self.assertIn("OPENAI_API_KEY: submodule-<redacted>", message)
        self.assertNotIn(sentinel, message)

    def test_is_a_no_op_when_no_candidates_are_configured(self) -> None:
        privacy.scan(
            Path("missing-repository"),
            Path("missing-release-binary"),
            environment={
                "OPENAI_API_KEY": "",
                "ANTHROPIC_API_KEY": "",
                "MARKTURBO_PRIVACY_SENTINEL": "",
                "MARKTURBO_PRIVATE_REQUEST_SENTINEL": "",
            },
        )


class CheckIntegrationTests(unittest.TestCase):
    def test_full_scans_the_release_binary_after_building_it(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            binary = root / "target" / "release" / "markturbo.exe"
            events: list[str] = []

            def run(command: tuple[str, ...]) -> None:
                self.assertEqual(
                    command,
                    ("cargo", "build", "--release", "--locked", "-p", "mt-app", "--bin", "markturbo"),
                )
                binary.parent.mkdir(parents=True)
                binary.write_bytes(b"release binary")
                events.append("build")

            def scan(repository: Path, release_binary: Path) -> None:
                self.assertEqual((repository, release_binary), (root, binary))
                events.append("scan")

            with (
                mock.patch.object(checks, "ROOT", root),
                mock.patch.object(checks.sys, "platform", "win32"),
                mock.patch.object(checks, "ci"),
                mock.patch.object(checks, "cargo", side_effect=lambda *args: ("cargo", *args)),
                mock.patch.object(checks, "run", side_effect=run),
                mock.patch.object(checks.privacy, "scan", side_effect=scan),
            ):
                checks.full()

        self.assertEqual(events, ["build", "scan"])

    def test_ci_does_not_run_the_release_binary_privacy_scan(self) -> None:
        with (
            mock.patch.object(checks, "fast"),
            mock.patch.object(checks, "cargo", side_effect=lambda *args: ("cargo", *args)),
            mock.patch.object(checks, "run"),
            mock.patch.object(checks.privacy, "scan") as scan,
        ):
            checks.ci()

        scan.assert_not_called()

    def test_full_reports_privacy_failures_as_check_failures(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            binary = root / "target" / "release" / "markturbo.exe"

            def run(_command: tuple[str, ...]) -> None:
                binary.parent.mkdir(parents=True)
                binary.write_bytes(b"release binary")

            with (
                mock.patch.object(checks, "ROOT", root),
                mock.patch.object(checks.sys, "platform", "win32"),
                mock.patch.object(checks, "ci"),
                mock.patch.object(checks, "cargo", side_effect=lambda *args: ("cargo", *args)),
                mock.patch.object(checks, "run", side_effect=run),
                mock.patch.object(
                    checks.privacy,
                    "scan",
                    side_effect=privacy.PrivacyScanError(
                        "privacy scan found candidate secrets:\nOPENAI_API_KEY: tracked.bin"
                    ),
                ),
            ):
                with self.assertRaises(checks.CheckFailure):
                    checks.full()

    def test_tooling_manifest_includes_privacy_tests(self) -> None:
        self.assertIn("scripts.tests.test_privacy", checks.TOOLING_TESTS)


if __name__ == "__main__":
    unittest.main()
