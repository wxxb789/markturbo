"""Derive the platform app icons from the checked-in 1024 px master PNG."""

from __future__ import annotations

import argparse
import os
import shutil
import struct
import subprocess
import tempfile
from pathlib import Path


ROOT = Path(__file__).resolve().parents[2]
ICONS = ROOT / "crates" / "mt-app" / "resources" / "icons"
MASTER = ICONS / "markturbo.png"
X11 = ICONS / "markturbo-256.png"
LINUX = ICONS / "markturbo-512.png"
ICO = ICONS / "markturbo.ico"
ICNS = ICONS / "markturbo.icns"


def imagemagick() -> str:
    # `convert.exe` is a Windows filesystem utility, never ImageMagick.
    command = shutil.which("magick")
    if command is None and os.name != "nt":
        command = shutil.which("convert")
    if command is None:
        expected = "`magick`" if os.name == "nt" else "`magick` or `convert`"
        raise SystemExit(f"ImageMagick is required (expected {expected})")
    return command


def png_size(path: Path) -> tuple[int, int]:
    with path.open("rb") as file:
        header = file.read(24)
    if header[:8] != b"\x89PNG\r\n\x1a\n" or header[12:16] != b"IHDR":
        raise SystemExit(f"{path} is not a PNG")
    return struct.unpack(">II", header[16:24])


def resize(command: str, source: Path, destination: Path, size: int) -> None:
    subprocess.run(
        [
            command,
            str(source),
            "-filter",
            "Lanczos",
            "-resize",
            f"{size}x{size}",
            "-unsharp",
            "0x0.65+0.75+0.02",
            "-strip",
            f"PNG32:{destination}",
        ],
        check=True,
    )


def validate_ico(path: Path) -> None:
    contents = path.read_bytes()
    if len(contents) < 6:
        raise SystemExit(f"{path} is not an ICO")
    reserved, icon_type, count = struct.unpack("<HHH", contents[:6])
    table_size = 6 + count * 16
    if reserved != 0 or icon_type != 1 or count == 0 or len(contents) < table_size:
        raise SystemExit(f"{path} is not an ICO")
    for entry in range(count):
        offset = 6 + entry * 16
        size, data_offset = struct.unpack("<II", contents[offset + 8 : offset + 16])
        if size == 0 or data_offset < table_size or data_offset + size > len(contents):
            raise SystemExit(f"{path} is not an ICO")


def validate_icns(path: Path) -> None:
    contents = path.read_bytes()
    if len(contents) < 8 or contents[:4] != b"icns":
        raise SystemExit(f"{path} is not an ICNS")
    (total_size,) = struct.unpack(">I", contents[4:8])
    if total_size != len(contents):
        raise SystemExit(f"{path} is not an ICNS")
    offset = 8
    while offset < len(contents):
        if offset + 8 > len(contents):
            raise SystemExit(f"{path} is not an ICNS")
        (chunk_size,) = struct.unpack(">I", contents[offset + 4 : offset + 8])
        if chunk_size < 8 or offset + chunk_size > len(contents):
            raise SystemExit(f"{path} is not an ICNS")
        offset += chunk_size
    if offset != len(contents) or offset == 8:
        raise SystemExit(f"{path} is not an ICNS")


def write_icns(images: dict[int, Path], destination: Path) -> None:
    # Modern ICNS entries contain complete PNG files. The duplicate physical
    # sizes are Retina representations with different logical dimensions.
    entries = [
        (b"icp4", 16),
        (b"icp5", 32),
        (b"icp6", 64),
        (b"ic07", 128),
        (b"ic08", 256),
        (b"ic09", 512),
        (b"ic10", 1024),
        (b"ic11", 32),
        (b"ic12", 64),
        (b"ic13", 256),
        (b"ic14", 512),
    ]
    chunks = []
    for kind, size in entries:
        payload = images[size].read_bytes()
        chunks.append(kind + struct.pack(">I", len(payload) + 8) + payload)
    body = b"".join(chunks)
    destination.write_bytes(b"icns" + struct.pack(">I", len(body) + 8) + body)


def publish(generated: dict[Path, Path], temporary: Path) -> None:
    backups: dict[Path, Path | None] = {}
    for destination in generated:
        if destination.exists():
            backup = temporary / f"previous-{destination.name}"
            shutil.copyfile(destination, backup)
            backups[destination] = backup
        else:
            backups[destination] = None

    published: list[Path] = []
    try:
        for destination, source in generated.items():
            published.append(destination)
            os.replace(source, destination)
    # An interrupt after one replacement is just as partial as an I/O failure.
    except BaseException:
        for destination in published:
            backup = backups[destination]
            if backup is None:
                destination.unlink(missing_ok=True)
            else:
                os.replace(backup, destination)
        raise


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.parse_args(argv)
    if png_size(MASTER) != (1024, 1024):
        raise SystemExit(f"{MASTER} must be exactly 1024x1024")

    command = imagemagick()
    sizes = (16, 24, 32, 48, 64, 128, 256, 512, 1024)
    # Keep staging on the target volume so publication is atomic on Windows.
    with tempfile.TemporaryDirectory(dir=ICONS, prefix=".markturbo-icons-") as directory:
        temporary = Path(directory)
        images = {size: temporary / f"markturbo-{size}.png" for size in sizes}
        for size, path in images.items():
            resize(command, MASTER, path, size)
            if png_size(path) != (size, size):
                raise SystemExit(f"{path} must be exactly {size}x{size}")

        generated = {
            X11: temporary / f"output-{X11.name}",
            LINUX: temporary / f"output-{LINUX.name}",
            ICO: temporary / f"output-{ICO.name}",
            ICNS: temporary / f"output-{ICNS.name}",
        }
        shutil.copyfile(images[256], generated[X11])
        shutil.copyfile(images[512], generated[LINUX])
        subprocess.run(
            [command, *(str(images[size]) for size in sizes[:-2]), str(generated[ICO])],
            check=True,
        )
        write_icns(images, generated[ICNS])

        validate_ico(generated[ICO])
        validate_icns(generated[ICNS])
        publish(generated, temporary)

    print(f"wrote {X11.relative_to(ROOT)}")
    print(f"wrote {LINUX.relative_to(ROOT)}")
    print(f"wrote {ICO.relative_to(ROOT)}")
    print(f"wrote {ICNS.relative_to(ROOT)}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
