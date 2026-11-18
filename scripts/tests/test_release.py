"""Behavioral tests for the Windows release packaging contract."""

from __future__ import annotations

import json
import os
import shutil
import struct
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path
from unittest import mock

from scripts.markturbo_tools import checks, release


def fake_binary(root: Path, *, version: str = "1.2.3") -> Path:
    """Create a stand-in for Cargo's `markturbo.exe` carrying its version."""

    binary = root / "target" / "release" / "markturbo.exe"
    binary.parent.mkdir(parents=True)
    binary.write_bytes(b"MZ" + b"\0" * 1022 + version.encode("utf-8"))
    return binary


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

    binary = fake_binary(root, version=version)
    payload = b"".join(
        [
            binary.read_bytes(),
            b"README.md",
            b"skills/hello-diagrams.md",
            font.read_bytes()[: release.FONT_MARKER_BYTES],
        ]
    )
    binary.write_bytes(payload)
    return binary, sample, fonts


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
    def test_staging_ships_the_cargo_binary_under_the_installed_name(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            binary, _, _ = fake_payload(root)
            license_file = root / "license.txt"
            license_file.write_text("license\n", encoding="utf-8")
            notice_file = root / "notices.txt"
            notice_file.write_text("notices\n", encoding="utf-8")
            destination = root / "staged"

            files = release.stage_payload(
                binary,
                destination,
                notices={"LICENSE": license_file, "NOTICE.txt": notice_file},
            )

            self.assertEqual(
                [path.as_posix() for path in files],
                ["LICENSE", "MarkTurbo.exe", "NOTICE.txt"],
            )
            self.assertEqual(
                (destination / "MarkTurbo.exe").read_bytes(), binary.read_bytes()
            )
            self.assertIn(
                {"path": "MarkTurbo.exe", "bytes": binary.stat().st_size},
                release.payload_inventory(destination),
            )

    def test_staging_rejects_a_missing_binary_or_notice(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            with self.assertRaisesRegex(release.ReleaseError, "executable is missing"):
                release.stage_payload(root / "missing.exe", root / "staged", notices={})
            binary = fake_binary(root)
            with self.assertRaisesRegex(release.ReleaseError, "notice is missing"):
                release.stage_payload(
                    binary, root / "staged", notices={"LICENSE": root / "missing"}
                )

    def test_binary_check_requires_every_font_and_sample_entry(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            binary, sample, fonts = fake_payload(root)

            release.verify_binary(binary, release_version(), sample=sample, fonts=fonts)

    def test_binary_check_names_the_missing_font(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            binary, sample, fonts = fake_payload(root)
            (fonts / "KaTeX_AMS-Regular.ttf").write_bytes(b"FONT" + bytes(range(60)))

            with self.assertRaisesRegex(release.ReleaseError, "KaTeX_AMS-Regular.ttf"):
                release.verify_binary(binary, release_version(), sample=sample, fonts=fonts)

    def test_binary_check_names_the_missing_sample_file(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            binary, sample, fonts = fake_payload(root)
            (sample / "extra.md").write_text("extra\n", encoding="utf-8")

            with self.assertRaisesRegex(release.ReleaseError, "sample/extra.md"):
                release.verify_binary(binary, release_version(), sample=sample, fonts=fonts)

    def test_binary_check_rejects_a_binary_built_from_another_version(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            binary, sample, fonts = fake_payload(root, version="9.9.9")

            with self.assertRaisesRegex(release.ReleaseError, "not built from version 1.2.3"):
                release.verify_binary(binary, release_version(), sample=sample, fonts=fonts)


class BuildTests(unittest.TestCase):
    def test_release_builds_the_workspace_binary_with_the_locked_root_profile(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            binary = fake_binary(Path(temporary))
            artifact = json.dumps(
                {
                    "reason": "compiler-artifact",
                    "target": {"name": "markturbo", "kind": ["bin"]},
                    "executable": str(binary),
                }
            )
            with (
                mock.patch.object(checks, "cargo", side_effect=lambda *args: ("cargo", *args)),
                mock.patch.object(checks, "run", return_value=artifact + "\n") as run,
            ):
                self.assertEqual(release.build_app_binary(), binary)

            command = list(run.call_args.args[0])
            self.assertEqual(
                command[:8],
                ["cargo", "build", "--release", "--locked", "-p", "mt-app", "--bin", "markturbo"],
            )
            self.assertNotIn("--manifest-path", command)
            self.assertEqual(run.call_args.kwargs.get("cwd", checks.ROOT), release.ROOT)

    def test_a_failed_build_is_a_release_error(self) -> None:
        with (
            mock.patch.object(checks, "cargo", side_effect=lambda *args: ("cargo", *args)),
            mock.patch.object(checks, "run", side_effect=checks.CheckFailure("cargo failed")),
            self.assertRaisesRegex(release.ReleaseError, "cargo failed"),
        ):
            release.build_app_binary()


class IconResourceTests(unittest.TestCase):
    def test_icon_group_replaces_each_file_offset_with_a_resource_id(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            ico = release.render_icon(Path(temporary) / "app.ico").read_bytes()

        group, images = release.icon_group_resources(ico)

        reserved, kind, count = struct.unpack_from("<HHH", ico)
        self.assertEqual(group[:6], struct.pack("<HHH", reserved, kind, count))
        self.assertEqual(len(group), 6 + 14 * count)
        self.assertEqual([image_id for image_id, _ in images], list(range(1, count + 1)))
        for index, (image_id, image) in enumerate(images):
            entry = struct.unpack_from("<BBBBHHII", ico, 6 + 16 * index)
            group_entry = struct.unpack_from("<BBBBHHIH", group, 6 + 14 * index)
            self.assertEqual(group_entry[:7], entry[:7])
            self.assertEqual(group_entry[7], image_id)
            self.assertEqual(image, ico[entry[7] : entry[7] + entry[6]])

    def test_malformed_icons_are_rejected(self) -> None:
        header = struct.pack("<HHH", 0, 1, 1)
        for data in (b"", b"PNG", header, header + struct.pack("<BBBBHHII", 16, 16, 0, 0, 1, 32, 64, 22)):
            with self.subTest(data=data):
                with self.assertRaises(release.ReleaseError):
                    release.icon_group_resources(data)

    @unittest.skipUnless(sys.platform == "win32", "PE resources are edited through Win32")
    def test_embedded_icon_group_one_round_trips_through_win32(self) -> None:
        import ctypes
        from ctypes import wintypes

        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            executable = root / "MarkTurbo.exe"
            shutil.copyfile(sys.executable, executable)
            icon = release.render_icon(root / "app.ico")
            with self.assertRaises(release.ReleaseError):
                release.verify_icon(executable, icon)

            release.embed_icon(executable, icon)
            release.verify_icon(executable, icon)

            # The same lookup nsh-platform-windows makes for its window class.
            kernel32 = ctypes.WinDLL("kernel32", use_last_error=True)
            user32 = ctypes.WinDLL("user32", use_last_error=True)
            kernel32.LoadLibraryExW.argtypes = [wintypes.LPCWSTR, wintypes.HANDLE, wintypes.DWORD]
            kernel32.LoadLibraryExW.restype = wintypes.HMODULE
            kernel32.FreeLibrary.argtypes = [wintypes.HMODULE]
            user32.LoadIconW.argtypes = [wintypes.HMODULE, ctypes.c_void_p]
            user32.LoadIconW.restype = wintypes.HICON
            module = kernel32.LoadLibraryExW(str(executable), None, 0x20)
            self.assertTrue(module)
            try:
                self.assertTrue(user32.LoadIconW(module, release.ICON_GROUP_ID))
            finally:
                kernel32.FreeLibrary(module)


class PackageReleaseTests(unittest.TestCase):
    def test_package_embeds_the_icon_before_signing_and_ships_no_nsh_metadata(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            binary = fake_binary(root)
            output = root / "dist"
            events: list[tuple[str, str]] = []

            def record(name: str):  # type: ignore[no-untyped-def]
                return lambda path, *_args, **_kwargs: events.append((name, Path(path).name))

            def compile_installer(payload: Path, out: Path, version, *, icon: Path) -> Path:  # type: ignore[no-untyped-def]
                events.append(("compile", sorted(path.name for path in payload.rglob("*"))))
                artifact = out / version.artifact
                artifact.write_bytes(b"installer")
                return artifact

            with (
                mock.patch.object(release, "_signing_credentials", return_value=("a.pfx", "pw")),
                mock.patch.object(release, "build_app_binary", return_value=binary),
                mock.patch.object(release, "embed_icon", side_effect=record("embed_icon")),
                mock.patch.object(release, "verify_icon", side_effect=record("verify_icon")),
                mock.patch.object(release, "verify_binary", side_effect=record("verify_binary")),
                mock.patch.object(release.privacy, "scan"),
                mock.patch.object(release, "sign_file", side_effect=record("sign")),
                mock.patch.object(release, "verify_authenticode"),
                mock.patch.object(release, "compile_installer", side_effect=compile_installer),
            ):
                document = release.package_release(output)

            self.assertEqual(
                events,
                [
                    ("embed_icon", "MarkTurbo.exe"),
                    ("verify_icon", "MarkTurbo.exe"),
                    ("verify_binary", "MarkTurbo.exe"),
                    ("sign", "MarkTurbo.exe"),
                    ("compile", ["LICENSE", "MarkTurbo.exe", "THIRD-PARTY-NOTICES.md"]),
                    ("sign", document["artifact"]),
                ],
            )
            self.assertEqual(
                [entry["path"] for entry in document["payload"]],
                ["LICENSE", "MarkTurbo.exe", "THIRD-PARTY-NOTICES.md"],
            )
            self.assertEqual(document["signing"], "authenticode")


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

            with self.assertRaisesRegex(release.ReleaseError, "LICENSE"):
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
