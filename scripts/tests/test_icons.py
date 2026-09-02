"""Regression tests for app-icon publication contracts."""

from __future__ import annotations

import os
import struct
import subprocess
import tempfile
import unittest
from pathlib import Path
from unittest import mock

from scripts.markturbo_tools import icons

ICONS = icons


def png(size: int) -> bytes:
    return b"\x89PNG\r\n\x1a\n\x00\x00\x00\rIHDR" + struct.pack(">II", size, size)


class AppIconGenerationTests(unittest.TestCase):
    def test_windows_requires_magick_without_querying_convert(self) -> None:
        with (
            mock.patch.object(ICONS.os, "name", "nt"),
            mock.patch.object(ICONS.shutil, "which", return_value=None) as which,
        ):
            with self.assertRaisesRegex(SystemExit, "ImageMagick is required"):
                ICONS.imagemagick()

        self.assertEqual(which.call_args_list, [mock.call("magick")])

    def test_non_windows_uses_convert_as_a_compatibility_fallback(self) -> None:
        with (
            mock.patch.object(ICONS.os, "name", "posix"),
            mock.patch.object(ICONS.shutil, "which", side_effect=[None, "convert"]) as which,
        ):
            self.assertEqual(ICONS.imagemagick(), "convert")

        self.assertEqual(which.call_args_list, [mock.call("magick"), mock.call("convert")])

    def test_generation_failure_preserves_every_destination(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            destinations = self.configure_paths(root)
            before = self.destination_bytes(destinations)

            def fake_resize(_command: str, _source: Path, destination: Path, size: int) -> None:
                destination.write_bytes(png(size))

            def fail_ico(command: list[str], **_kwargs: object) -> None:
                if command[-1].endswith(".ico"):
                    raise subprocess.CalledProcessError(1, command)

            with (
                mock.patch.object(ICONS, "resize", side_effect=fake_resize),
                mock.patch.object(ICONS, "imagemagick", return_value="magick"),
                mock.patch.object(ICONS.subprocess, "run", side_effect=fail_ico),
            ):
                with self.assertRaises(subprocess.CalledProcessError):
                    ICONS.main([])

            self.assertEqual(self.destination_bytes(destinations), before)

    def test_publication_failure_restores_every_destination(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            destinations = self.configure_paths(root)
            before = self.destination_bytes(destinations)
            generated = {
                destination: root / f"new-{destination.name}"
                for destination in destinations
            }
            for path in generated.values():
                path.write_bytes(f"new-{path.name}".encode())

            real_replace = os.replace
            failed = False

            def fail_second_publish(source: Path | str, destination: Path | str) -> None:
                nonlocal failed
                if Path(destination) == destinations[1] and not failed:
                    failed = True
                    raise OSError("simulated publication failure")
                real_replace(source, destination)

            with mock.patch.object(ICONS.os, "replace", side_effect=fail_second_publish):
                with self.assertRaisesRegex(OSError, "simulated publication failure"):
                    ICONS.publish(generated, root)

            self.assertEqual(self.destination_bytes(destinations), before)

    def test_interruption_after_replace_restores_existing_and_missing_destinations(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            destinations = self.configure_paths(root)
            destinations[0].unlink()
            before = self.destination_bytes(destinations)
            generated = {
                destination: root / f"new-{destination.name}"
                for destination in destinations
            }
            for path in generated.values():
                path.write_bytes(f"new-{path.name}".encode())

            real_replace = os.replace
            replacements = 0

            def interrupt_after_second_replace(source: Path | str, destination: Path | str) -> None:
                nonlocal replacements
                if Path(source) in generated.values():
                    replacements += 1
                real_replace(source, destination)
                if Path(source) in generated.values() and Path(destination) == destinations[1]:
                    raise KeyboardInterrupt("simulated interruption")

            with mock.patch.object(ICONS.os, "replace", side_effect=interrupt_after_second_replace):
                with self.assertRaisesRegex(KeyboardInterrupt, "simulated interruption"):
                    ICONS.publish(generated, root)

            self.assertEqual(replacements, 2)
            self.assertEqual(self.destination_bytes(destinations), before)

    def configure_paths(self, root: Path) -> list[Path]:
        icons = root / "icons"
        icons.mkdir()
        master = icons / "markturbo.png"
        master.write_bytes(png(1024))
        destinations = [
            icons / "markturbo-256.png",
            icons / "markturbo-512.png",
            icons / "markturbo.ico",
            icons / "markturbo.icns",
        ]
        for index, path in enumerate(destinations):
            path.write_bytes(f"old-{index}".encode())

        patcher = mock.patch.multiple(
            ICONS,
            ROOT=root,
            ICONS=icons,
            MASTER=master,
            X11=destinations[0],
            LINUX=destinations[1],
            ICO=destinations[2],
            ICNS=destinations[3],
        )
        patcher.start()
        self.addCleanup(patcher.stop)
        return destinations

    @staticmethod
    def destination_bytes(destinations: list[Path]) -> dict[Path, bytes | None]:
        return {path: path.read_bytes() if path.exists() else None for path in destinations}


if __name__ == "__main__":
    unittest.main()
