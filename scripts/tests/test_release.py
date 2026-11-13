"""Behavioral tests for the Windows release packaging contract."""

from __future__ import annotations

import json
import os
import subprocess
import tempfile
import unittest
from pathlib import Path
from unittest import mock

from scripts.markturbo_tools import release


def fake_app_tree(root: Path, *, version: str = "1.2.3") -> Path:
    """Create the minimum nsh package layout the release checks expect."""

    app = root / "app"
    (app / "nsh").mkdir(parents=True)
    (app / "nsh" / "app.json").write_text(
        json.dumps(
            {
                "name": "MarkTurbo",
                "id": "com.markturbo.app",
                "version": version,
                "appearance": "system",
            }
        ),
        encoding="utf-8",
    )
    binary = app / "MarkTurbo.exe"
    binary.write_bytes(b"MZ" + b"\0" * 1022)
    return app


def fake_payload(root: Path, *, version: str = "1.2.3") -> tuple[Path, Path, Path]:
    sample = root / "sample"
    fonts = root / "fonts"
    sample.mkdir()
    fonts.mkdir()
    (sample / "README.md").write_text("# Sample workspace\n", encoding="utf-8")
    (sample / "skills").mkdir()
    (sample / "skills" / "hello-diagrams.md").write_text(
        "# Diagrams\n", encoding="utf-8"
    )
    font = fonts / "KaTeX_Main-Regular.ttf"
    font.write_bytes(b"TTF!" + bytes(range(60)))
    excluded = fonts / "KaTeX_Caligraphic-Bold.ttf"
    excluded.write_bytes(b"XTRA" + bytes(reversed(range(60))))

    app = fake_app_tree(root, version=version)
    binary = app / "MarkTurbo.exe"
    payload = b"".join(
        [
            binary.read_bytes(),
            b"README.md",
            b"skills/hello-diagrams.md",
            font.read_bytes()[: release.FONT_MARKER_BYTES],
        ]
    )
    binary.write_bytes(payload)
    return app, sample, fonts


def release_version(version: str = "1.2.3") -> release.ReleaseVersion:
    return release.release_version(version)


def write_manifest(
    root: Path,
    *,
    artifact_bytes: bytes,
    version: str = "1.2.3",
    signing: str = "unsigned",
    payload: list[dict[str, object]] | None = None,
    extra: dict[str, object] | None = None,
) -> Path:
    artifact = root / release.release_version(version).artifact
    artifact.write_bytes(artifact_bytes)
    checksum = release.write_checksum(artifact)
    document = {
        "product": "MarkTurbo",
        "version": version,
        "channel": release.release_version(version).channel,
        "platform": "windows-x86_64",
        "artifact": artifact.name,
        "checksum_file": checksum.name,
        "sha256": release._sha256(artifact),
        "bytes": artifact.stat().st_size,
        "signing": signing,
        "payload": payload
        if payload is not None
        else [
            {"path": "MarkTurbo.exe", "bytes": 16},
            {"path": "nsh/app.json", "bytes": 16},
            {"path": "LICENSE", "bytes": 16},
            {"path": "THIRD-PARTY-NOTICES.md", "bytes": 16},
        ],
    }
    if extra:
        document.update(extra)
    manifest = root / "release.json"
    manifest.write_text(json.dumps(document), encoding="utf-8")
    return manifest


def pe_image(*, signed: bool) -> bytes:
    """Build a minimal PE header with or without a security directory entry."""

    data = bytearray(0x400)
    data[:2] = b"MZ"
    pe_offset = 0x80
    data[0x3C:0x40] = pe_offset.to_bytes(4, "little")
    data[pe_offset : pe_offset + 4] = b"PE\0\0"
    optional_size = 240
    data[pe_offset + 20 : pe_offset + 22] = optional_size.to_bytes(2, "little")
    optional = pe_offset + 24
    data[optional : optional + 2] = (0x20B).to_bytes(2, "little")
    entry = optional + 112 + 4 * 8
    if signed:
        data[entry : entry + 4] = (0x400).to_bytes(4, "little")
        data[entry + 4 : entry + 8] = (16).to_bytes(4, "little")
    return bytes(data)


class ReleaseVersionTests(unittest.TestCase):
    def test_stable_version_derives_the_installer_artifact(self) -> None:
        version = release_version("1.2.3")

        self.assertEqual(version.numeric, "1.2.3")
        self.assertEqual(version.channel, "stable")
        self.assertEqual(version.artifact, "MarkTurbo-Setup-1.2.3.exe")
        self.assertEqual(version.checksum, "MarkTurbo-Setup-1.2.3.exe.sha256")

    def test_prerelease_and_build_metadata_keep_the_numeric_version(self) -> None:
        version = release_version("0.1.4-alpha.2+build.7")

        self.assertEqual(version.numeric, "0.1.4")
        self.assertEqual(version.channel, "prerelease")

    def test_invalid_versions_are_rejected(self) -> None:
        for value in ("1.2", "v1.2.3", "01.2.3", "1.2.3.4", "1.2.65536"):
            with self.subTest(value=value):
                with self.assertRaises(release.ReleaseError):
                    release.release_version(value)

    def test_version_disagreement_names_the_conflicting_input(self) -> None:
        with self.assertRaisesRegex(release.ReleaseError, "nsh.json"):
            release.check_version_agreement("1.2.3", "1.2.4")
        with self.assertRaisesRegex(release.ReleaseError, "tag"):
            release.check_version_agreement("1.2.3", "1.2.3", "v1.2.4")

    def test_repository_versions_agree(self) -> None:
        version = release.repository_version()

        self.assertEqual(version.version, release.workspace_version())
        self.assertEqual(release.nsh_metadata()["version"], version.version)

    def test_partial_signing_configuration_is_rejected(self) -> None:
        with mock.patch.dict(
            os.environ,
            {"MARKTURBO_SIGN_PFX": "certificate.pfx"},
            clear=True,
        ):
            with self.assertRaisesRegex(release.ReleaseError, "must be set together"):
                release._signing_credentials()


class PayloadTests(unittest.TestCase):
    def test_staging_copies_every_packaged_file_and_reports_it(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            app, _, _ = fake_payload(root)
            destination = root / "staged"

            files = release.stage_payload(app, destination, notices={})

            self.assertEqual(
                [path.as_posix() for path in files],
                ["MarkTurbo.exe", "nsh/app.json"],
            )
            self.assertEqual(
                release.payload_inventory(destination),
                [
                    {"path": "MarkTurbo.exe", "bytes": (destination / "MarkTurbo.exe").stat().st_size},
                    {"path": "nsh/app.json", "bytes": (destination / "nsh" / "app.json").stat().st_size},
                ],
            )

    def test_staging_copies_shipped_notices(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            app = fake_app_tree(root)
            license_file = root / "license.txt"
            license_file.write_text("license\n", encoding="utf-8")
            notice_file = root / "notices.txt"
            notice_file.write_text("notices\n", encoding="utf-8")

            files = release.stage_payload(
                app,
                root / "staged",
                notices={"LICENSE": license_file, "NOTICE.txt": notice_file},
            )

            self.assertEqual(
                [path.as_posix() for path in files],
                ["LICENSE", "MarkTurbo.exe", "NOTICE.txt", "nsh/app.json"],
            )

    def test_staging_rejects_a_package_without_the_runner_metadata(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            app = fake_app_tree(Path(temporary))
            (app / "nsh" / "app.json").unlink()

            with self.assertRaisesRegex(release.ReleaseError, "nsh/app.json"):
                release.stage_payload(app, Path(temporary) / "staged")

    def test_binary_check_requires_every_font_and_sample_entry(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            app, sample, fonts = fake_payload(root)

            release.verify_binary(
                app / "MarkTurbo.exe",
                release_version(),
                sample=sample,
                fonts=fonts,
            )

    def test_binary_check_names_the_missing_font(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            app, sample, fonts = fake_payload(root)
            (fonts / "KaTeX_AMS-Regular.ttf").write_bytes(b"FONT" + bytes(range(60)))

            with self.assertRaisesRegex(release.ReleaseError, "KaTeX_AMS-Regular.ttf"):
                release.verify_binary(
                    app / "MarkTurbo.exe",
                    release_version(),
                    sample=sample,
                    fonts=fonts,
                )

    def test_binary_check_names_the_missing_sample_file(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            app, sample, fonts = fake_payload(root)
            (sample / "extra.md").write_text("extra\n", encoding="utf-8")

            with self.assertRaisesRegex(release.ReleaseError, "sample/extra.md"):
                release.verify_binary(
                    app / "MarkTurbo.exe",
                    release_version(),
                    sample=sample,
                    fonts=fonts,
                )

    def test_binary_check_rejects_a_version_disagreement_in_the_runner_metadata(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            app, sample, fonts = fake_payload(root)
            metadata = json.loads((app / "nsh" / "app.json").read_text(encoding="utf-8"))
            metadata["version"] = "9.9.9"
            (app / "nsh" / "app.json").write_text(json.dumps(metadata), encoding="utf-8")

            with self.assertRaisesRegex(release.ReleaseError, "does not match"):
                release.verify_binary(
                    app / "MarkTurbo.exe",
                    release_version(),
                    sample=sample,
                    fonts=fonts,
                )


class PEHeaderTests(unittest.TestCase):
    def test_security_directory_distinguishes_signed_and_unsigned_images(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            signed = root / "signed.exe"
            unsigned = root / "unsigned.exe"
            signed.write_bytes(pe_image(signed=True))
            unsigned.write_bytes(pe_image(signed=False))

            self.assertTrue(release.has_pe_certificate(signed))
            self.assertFalse(release.has_pe_certificate(unsigned))

    def test_arbitrary_bytes_are_not_treated_as_a_signed_image(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            path = Path(temporary) / "artifact.exe"
            path.write_bytes(b"not a PE")

            self.assertFalse(release.has_pe_certificate(path))


class ArtifactVerificationTests(unittest.TestCase):
    def test_valid_unsigned_manifest_verifies_with_its_checksum(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            manifest = write_manifest(root, artifact_bytes=pe_image(signed=False))

            document = release.verify_artifact(
                manifest,
                expected_version="1.2.3",
            )

            self.assertEqual(document["signing"], "unsigned")
            self.assertEqual(document["artifact"], "MarkTurbo-Setup-1.2.3.exe")

    def test_tampered_artifact_is_rejected(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            manifest = write_manifest(root, artifact_bytes=pe_image(signed=False))
            (root / "MarkTurbo-Setup-1.2.3.exe").write_bytes(b"tampered")

            with self.assertRaisesRegex(release.ReleaseError, "does not match"):
                release.verify_artifact(manifest)

    def test_signature_required_channels_reject_an_unsigned_artifact(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            manifest = write_manifest(root, artifact_bytes=pe_image(signed=False))

            with self.assertRaisesRegex(release.ReleaseError, "requires a signed artifact"):
                release.verify_artifact(manifest, require_signature=True)

    def test_manifest_signature_claim_must_match_the_artifact(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            signed = write_manifest(root, artifact_bytes=pe_image(signed=True), signing="unsigned")
            with self.assertRaisesRegex(release.ReleaseError, "signed but the manifest says unsigned"):
                release.verify_artifact(signed)

            root = Path(temporary) / "second"
            root.mkdir()
            claimed = write_manifest(
                root, artifact_bytes=pe_image(signed=False), signing="authenticode"
            )
            with self.assertRaisesRegex(release.ReleaseError, "lacks"):
                release.verify_artifact(claimed)

    def test_authenticode_requires_a_trusted_signature_not_just_a_pe_header(self) -> None:
        for require_signature in (False, True):
            with self.subTest(require_signature=require_signature):
                with tempfile.TemporaryDirectory() as temporary:
                    root = Path(temporary)
                    manifest = write_manifest(
                        root, artifact_bytes=pe_image(signed=True), signing="authenticode"
                    )
                    result = subprocess.CompletedProcess(
                        [], 1, stdout="", stderr="signature digest is invalid"
                    )
                    with (
                        mock.patch.object(release, "signtool", return_value="signtool"),
                        mock.patch.object(release.subprocess, "run", return_value=result) as run,
                        self.assertRaisesRegex(release.ReleaseError, "digest is invalid"),
                    ):
                        release.verify_artifact(manifest, require_signature=require_signature)
                    self.assertEqual(
                        run.call_args.args[0],
                        ["signtool", "verify", "/pa", "/v", str(root / "MarkTurbo-Setup-1.2.3.exe")],
                    )

    def test_authenticode_rejects_an_untrusted_certificate(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            manifest = write_manifest(
                root, artifact_bytes=pe_image(signed=True), signing="authenticode"
            )
            result = subprocess.CompletedProcess(
                [], 1, stdout="", stderr="0x800B0109: certificate is not trusted"
            )
            with (
                mock.patch.object(release, "signtool", return_value="signtool"),
                mock.patch.object(release.subprocess, "run", return_value=result),
                self.assertRaisesRegex(release.ReleaseError, "not trusted"),
            ):
                release.verify_artifact(manifest, require_signature=True)

    def test_authenticode_verifies_when_windows_trust_verification_succeeds(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            manifest = write_manifest(
                root, artifact_bytes=pe_image(signed=True), signing="authenticode"
            )
            result = subprocess.CompletedProcess([], 0, stdout="verified", stderr="")
            with (
                mock.patch.object(release, "signtool", return_value="signtool"),
                mock.patch.object(release.subprocess, "run", return_value=result),
            ):
                document = release.verify_artifact(manifest, require_signature=True)
            self.assertEqual(document["signing"], "authenticode")

    def test_authenticode_fails_if_trust_verification_is_unavailable(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            manifest = write_manifest(
                root, artifact_bytes=pe_image(signed=True), signing="authenticode"
            )
            with (
                mock.patch.object(
                    release, "signtool", side_effect=release.ReleaseError("signtool unavailable")
                ),
                self.assertRaisesRegex(release.ReleaseError, "signtool unavailable"),
            ):
                release.verify_artifact(manifest, require_signature=True)

    def test_manifest_rejects_payload_paths_outside_the_payload(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            manifest = write_manifest(
                root,
                artifact_bytes=pe_image(signed=False),
                payload=[
                    {"path": "MarkTurbo.exe", "bytes": 12},
                    {"path": "../escape", "bytes": 12},
                ],
            )

            with self.assertRaisesRegex(release.ReleaseError, "normalized"):
                release.verify_artifact(manifest)

    def test_manifest_requires_every_required_payload_entry(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            manifest = write_manifest(
                root,
                artifact_bytes=pe_image(signed=False),
                payload=[{"path": "MarkTurbo.exe", "bytes": 12}],
            )

            with self.assertRaisesRegex(release.ReleaseError, "nsh/app.json"):
                release.verify_artifact(manifest)

    def test_manifest_version_must_match_the_checkout_under_test(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            manifest = write_manifest(root, artifact_bytes=pe_image(signed=False))

            with self.assertRaisesRegex(release.ReleaseError, "repository version"):
                release.verify_artifact(manifest, expected_version="9.9.9")

    def test_checksum_file_must_use_sha256sum_format(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            path = Path(temporary) / "checksum.sha256"
            path.write_text("not a checksum\n", encoding="utf-8")

            with self.assertRaisesRegex(release.ReleaseError, "sha256sum"):
                release.read_checksum(path)


class SelfSignedVerificationTests(unittest.TestCase):
    def build(self, root: Path, *, version: str = "1.2.3-alpha.1") -> Path:
        certificate = root / f"MarkTurbo-Setup-{version}.cer"
        certificate.write_bytes(b"certificate")
        return write_manifest(
            root,
            artifact_bytes=pe_image(signed=True),
            version=version,
            signing="self-signed",
            extra={
                "certificate": certificate.name,
                "certificate_sha256": release._sha256(certificate),
                "certificate_thumbprint": "A" * 40,
            },
        )

    def test_self_signed_prerelease_verifies_with_its_certificate(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            manifest = self.build(root)

            document = release.verify_artifact(
                manifest, expected_version="1.2.3-alpha.1"
            )

            self.assertEqual(document["signing"], "self-signed")

    def test_self_signed_cannot_be_a_stable_or_required_signature(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            manifest = self.build(root)
            stable_root = Path(temporary) / "stable"
            stable_root.mkdir()
            stable = self.build(stable_root, version="1.2.4")
            with self.assertRaisesRegex(release.ReleaseError, "stable"):
                release.verify_artifact(stable)
            with self.assertRaisesRegex(release.ReleaseError, "trusted"):
                release.verify_artifact(manifest, require_signature=True)

    def test_self_signed_certificate_must_match_the_manifest(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            manifest = self.build(root)
            (root / "MarkTurbo-Setup-1.2.3-alpha.1.cer").write_bytes(b"other")

            with self.assertRaisesRegex(release.ReleaseError, "certificate"):
                release.verify_artifact(manifest)


if __name__ == "__main__":
    unittest.main()
