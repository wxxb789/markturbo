"""Build and verify the Windows release installer.

The release artifact is one Inno Setup installer plus a SHA-256 sidecar and a
JSON manifest. This module owns the release contract: version agreement across
`Cargo.toml`, `nsh.json` and the release tag; the staged payload inventory and
its embedded font/sample markers; Authenticode signing policy; and artifact
verification.

Local development builds an unsigned installer. Stable releases require a
signing certificate through `MARKTURBO_SIGN_PFX` and
`MARKTURBO_SIGN_PASSWORD`; a prerelease may stay unsigned but must say so.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import re
import secrets
import shutil
import subprocess
import sys
import tempfile
import tomllib
from collections.abc import Sequence
from dataclasses import dataclass
from pathlib import Path

from . import privacy

ROOT = Path(__file__).resolve().parents[2]
APP_DIR = ROOT / "crates" / "mt-app"
CARGO_TOML = ROOT / "Cargo.toml"
NSH_JSON = APP_DIR / "nsh.json"
INSTALLER_SCRIPT = ROOT / "installer" / "markturbo.iss"
APP_ICON = APP_DIR / "icon.png"
FONT_DIR = ROOT / "fonts" / "katex"
SAMPLE_DIR = ROOT / "sample"
NOTICES = {
    "LICENSE": ROOT / "LICENSE",
    "THIRD-PARTY-NOTICES.md": ROOT / "THIRD_PARTY_NOTICES.md",
}

PRODUCT = "MarkTurbo"
PLATFORM = "windows-x86_64"
APP_ID = "com.markturbo.app"
APP_PAYLOAD = ("MarkTurbo.exe", "nsh/app.json")
REQUIRED_PAYLOAD = (*APP_PAYLOAD, *NOTICES)
# `MathFonts` excludes this face; see crates/mt-core/src/rendering/mod.rs.
EXCLUDED_FONTS = frozenset({"KaTeX_Caligraphic-Bold.ttf"})
FONT_MARKER_BYTES = 64
TIMESTAMP_URL = "http://timestamp.digicert.com"
SIGN_TIMEOUT_SECONDS = 120

_VERSION = re.compile(
    r"^(?P<numeric>(?:0|[1-9]\d*)\.(?:0|[1-9]\d*)\.(?:0|[1-9]\d*))"
    r"(?P<prerelease>-[0-9A-Za-z.-]+)?(?:\+[0-9A-Za-z.-]+)?$"
)
_CHECKSUM = re.compile(r"^(?P<digest>[0-9a-f]{64})  (?P<name>[^/\\]+)\n$")


class ReleaseError(RuntimeError):
    """The release artifact or its inputs do not satisfy the release contract."""


@dataclass(frozen=True)
class ReleaseVersion:
    version: str
    numeric: str
    channel: str

    @property
    def artifact(self) -> str:
        return f"{PRODUCT}-Setup-{self.version}.exe"

    @property
    def checksum(self) -> str:
        return f"{self.artifact}.sha256"


def release_version(version: str) -> ReleaseVersion:
    """Validate one Cargo version and derive the installer's numeric version."""

    match = _VERSION.fullmatch(version)
    if match is None:
        raise ReleaseError(f"{version!r} is not a semantic release version")
    numeric = match.group("numeric")
    if any(int(part) > 65535 for part in numeric.split(".")):
        raise ReleaseError(f"{version!r} exceeds the 16-bit installer version fields")
    channel = "prerelease" if match.group("prerelease") else "stable"
    return ReleaseVersion(version=version, numeric=numeric, channel=channel)


def check_version_agreement(
    workspace: str, nsh: str, tag: str | None = None
) -> ReleaseVersion:
    """Require Cargo, nsh.json and the release tag to name one version."""

    version = release_version(workspace)
    if nsh != workspace:
        raise ReleaseError(
            f"crates/mt-app/nsh.json version {nsh!r} does not match Cargo workspace "
            f"version {workspace!r}"
        )
    if tag is not None and tag != f"v{workspace}":
        raise ReleaseError(f"release tag {tag!r} does not match version v{workspace}")
    return version


def workspace_version() -> str:
    try:
        with CARGO_TOML.open("rb") as file:
            document = tomllib.load(file)
        version = document["workspace"]["package"]["version"]
    except (OSError, tomllib.TOMLDecodeError, KeyError, TypeError) as error:
        raise ReleaseError(f"could not read the Cargo workspace version: {error}") from error
    if not isinstance(version, str):
        raise ReleaseError("Cargo workspace version is not a string")
    return version


def nsh_metadata() -> dict[str, object]:
    try:
        document = json.loads(NSH_JSON.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError) as error:
        raise ReleaseError(f"could not read crates/mt-app/nsh.json: {error}") from error
    if not isinstance(document, dict):
        raise ReleaseError("crates/mt-app/nsh.json is not an object")
    return document


def repository_version(tag: str | None = None) -> ReleaseVersion:
    metadata = nsh_metadata()
    if metadata.get("name") != PRODUCT:
        raise ReleaseError("crates/mt-app/nsh.json does not name the MarkTurbo product")
    if metadata.get("id") != APP_ID:
        raise ReleaseError("crates/mt-app/nsh.json does not carry the MarkTurbo app id")
    nsh_version = metadata.get("version")
    if not isinstance(nsh_version, str):
        raise ReleaseError("crates/mt-app/nsh.json version is not a string")
    return check_version_agreement(workspace_version(), nsh_version, tag)


def _run(
    command: Sequence[str], *, cwd: Path = ROOT, secret: str | None = None
) -> None:
    display = list(command)
    if secret:
        display = ["***" if argument == secret else argument for argument in display]
    print("+", subprocess.list2cmdline(display), flush=True)
    try:
        completed = subprocess.run(
            list(command),
            cwd=cwd,
            check=False,
            timeout=SIGN_TIMEOUT_SECONDS if secret else None,
        )
    except OSError as error:
        raise ReleaseError(f"could not start {command[0]!r}: {error}") from error
    except subprocess.TimeoutExpired as error:
        raise ReleaseError(f"{command[0]!r} timed out") from error
    if completed.returncode:
        raise ReleaseError(
            f"command failed with exit code {completed.returncode}: {command[0]}"
        )


def _sha256(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as file:
        for chunk in iter(lambda: file.read(1 << 20), b""):
            digest.update(chunk)
    return digest.hexdigest()


def _relative_payload_files(payload: Path) -> list[Path]:
    files = [
        path.relative_to(payload)
        for path in sorted(payload.rglob("*"))
        if path.is_file()
    ]
    if not files:
        raise ReleaseError(f"{payload} contains no payload files")
    for path in files:
        if path.is_absolute() or ".." in path.parts:
            raise ReleaseError(f"payload path escapes the installer directory: {path}")
    return files


def _embedded_markers(
    sample: Path, fonts: Path, excluded_fonts: frozenset[str]
) -> list[tuple[str, bytes]]:
    """Return unique byte markers every release executable must embed."""

    if not sample.is_dir():
        raise ReleaseError(f"bundled sample directory is missing: {sample}")
    if not fonts.is_dir():
        raise ReleaseError(f"math font directory is missing: {fonts}")
    markers: list[tuple[str, bytes]] = []
    for path in sorted(sample.rglob("*")):
        if path.is_file():
            relative = path.relative_to(sample).as_posix()
            markers.append((f"sample/{relative}", relative.encode("utf-8")))
    for path in sorted(fonts.glob("KaTeX_*.ttf")):
        if path.name in excluded_fonts:
            continue
        marker = path.read_bytes()[:FONT_MARKER_BYTES]
        if len(marker) < FONT_MARKER_BYTES:
            raise ReleaseError(f"{path} is too small to be an embedded font")
        markers.append((f"fonts/katex/{path.name}", marker))
    if not any(name.startswith("sample/") for name, _ in markers):
        raise ReleaseError("the bundled sample contains no files")
    return markers


def verify_binary(
    binary: Path,
    version: ReleaseVersion,
    *,
    sample: Path = SAMPLE_DIR,
    fonts: Path = FONT_DIR,
    excluded_fonts: frozenset[str] = EXCLUDED_FONTS,
) -> None:
    """Fail when the executable lacks an embedded font or sample entry."""

    try:
        data = binary.read_bytes()
    except OSError as error:
        raise ReleaseError(f"could not read {binary}: {error}") from error
    if not data:
        raise ReleaseError(f"{binary} is empty")
    markers = _embedded_markers(sample, fonts, excluded_fonts)
    for name, marker in markers:
        if marker not in data:
            raise ReleaseError(f"{binary} does not embed {name}")
    app_json = binary.parent / "nsh" / "app.json"
    if not app_json.is_file():
        raise ReleaseError(f"{app_json} is missing from the packaged payload")
    try:
        metadata = json.loads(app_json.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError) as error:
        raise ReleaseError(f"could not read {app_json}: {error}") from error
    if not isinstance(metadata, dict):
        raise ReleaseError(f"{app_json} is not an object")
    if metadata.get("version") != version.version:
        raise ReleaseError(
            f"{app_json} version {metadata.get('version')!r} does not match "
            f"{version.version!r}"
        )
    if metadata.get("name") != PRODUCT:
        raise ReleaseError(f"{app_json} does not name {PRODUCT}")
    if metadata.get("id") != APP_ID:
        raise ReleaseError(f"{app_json} does not carry {APP_ID}")


def stage_payload(
    source: Path,
    destination: Path,
    *,
    notices: dict[str, Path] = NOTICES,
) -> list[Path]:
    """Copy one packaged app directory and validate its required files."""

    if not source.is_dir():
        raise ReleaseError(f"packaged app directory is missing: {source}")
    for relative in APP_PAYLOAD:
        if not (source / relative).is_file():
            raise ReleaseError(f"packaged app is missing {relative}")
    if destination.exists():
        shutil.rmtree(destination)
    shutil.copytree(source, destination)
    for relative, notice in notices.items():
        if not notice.is_file():
            raise ReleaseError(f"release notice is missing: {notice}")
        shutil.copyfile(notice, destination / relative)
    return _relative_payload_files(destination)


def payload_inventory(payload: Path) -> list[dict[str, object]]:
    return [
        {"path": path.as_posix(), "bytes": (payload / path).stat().st_size}
        for path in _relative_payload_files(payload)
    ]


def nsh_executable() -> str:
    executable = os.environ.get("NSH") or shutil.which("nsh")
    if executable is None:
        local = os.environ.get("LOCALAPPDATA")
        if local:
            candidate = Path(local) / "nsh" / "bin" / ("nsh.exe" if os.name == "nt" else "nsh")
            if candidate.is_file():
                executable = str(candidate)
    if executable is None:
        raise ReleaseError("the nsh CLI was not found on PATH or under %LOCALAPPDATA%\\nsh")
    return executable


def build_packaged_app(output: Path) -> Path:
    """Ask nsh for the Windows app bundle that the installer ships."""

    output.mkdir(parents=True, exist_ok=True)
    _run(
        [nsh_executable(), "package", "--target", "windows", "--out", str(output)],
        cwd=APP_DIR,
    )
    name = nsh_metadata().get("name")
    if not isinstance(name, str) or not name:
        raise ReleaseError("crates/mt-app/nsh.json does not name the packaged app")
    packaged = output / name
    if not packaged.is_dir():
        raise ReleaseError(f"nsh package did not produce {packaged}")
    return packaged


def render_icon(destination: Path) -> Path:
    try:
        from PIL import Image
    except ImportError as error:
        raise ReleaseError("Pillow is required to render the installer icon") from error
    if not APP_ICON.is_file():
        raise ReleaseError(f"application icon is missing: {APP_ICON}")
    destination.parent.mkdir(parents=True, exist_ok=True)
    with Image.open(APP_ICON) as image:
        image.load()
        image.save(
            destination,
            format="ICO",
            sizes=[(size, size) for size in (16, 24, 32, 48, 64, 128, 256)],
        )
    return destination


def inno_compiler() -> str:
    executable = os.environ.get("ISCC") or shutil.which("iscc") or shutil.which("ISCC")
    if executable is None and os.name == "nt":
        candidates = [
            *Path(os.environ.get("LOCALAPPDATA", "")).glob("Programs/Inno Setup */ISCC.exe"),
            *Path("C:/Program Files (x86)").glob("Inno Setup */ISCC.exe"),
            *Path("C:/Program Files").glob("Inno Setup */ISCC.exe"),
        ]
        executable = next((str(path) for path in candidates if path.is_file()), None)
    if executable is None:
        raise ReleaseError("the Inno Setup compiler (ISCC.exe) was not found")
    return executable


def compile_installer(
    payload: Path, output: Path, version: ReleaseVersion, *, icon: Path
) -> Path:
    if not INSTALLER_SCRIPT.is_file():
        raise ReleaseError(f"installer script is missing: {INSTALLER_SCRIPT}")
    output.mkdir(parents=True, exist_ok=True)
    artifact = output / version.artifact
    _run(
        [
            inno_compiler(),
            f"/DAppVersion={version.version}",
            f"/DAppVersionNumeric={version.numeric}",
            f"/DPayloadDir={payload.resolve()}",
            f"/DOutputDir={output.resolve()}",
            f"/DOutputBaseFilename={artifact.stem}",
            f"/DSetupIcon={icon.resolve()}",
            f"/DRepoRoot={ROOT.resolve()}",
            str(INSTALLER_SCRIPT),
        ]
    )
    if not artifact.is_file():
        raise ReleaseError(f"Inno Setup did not produce {artifact}")
    return artifact


def _signing_credentials() -> tuple[str, str] | None:
    pfx = os.environ.get("MARKTURBO_SIGN_PFX", "")
    password = os.environ.get("MARKTURBO_SIGN_PASSWORD", "")
    if not pfx and not password:
        return None
    if not pfx or not password:
        raise ReleaseError(
            "MARKTURBO_SIGN_PFX and MARKTURBO_SIGN_PASSWORD must be set together"
        )
    if not Path(pfx).is_file():
        raise ReleaseError(f"signing certificate does not exist: {pfx}")
    return pfx, password


def signtool() -> str:
    executable = os.environ.get("SIGNTOOL") or shutil.which("signtool")
    if executable is None and os.name == "nt":
        kits = Path("C:/Program Files (x86)/Windows Kits/10/bin")
        if kits.is_dir():
            candidates = sorted(
                (path / "x64" / "signtool.exe" for path in kits.iterdir() if path.is_dir()),
                reverse=True,
            )
            executable = next((str(path) for path in candidates if path.is_file()), None)
    if executable is None:
        raise ReleaseError("signtool was not found; Windows SDK signing tools are required")
    return executable


SigningCredentials = tuple[str, str]


def generate_self_signed_certificate(
    directory: Path, *, days: int = 30
) -> tuple[SigningCredentials, Path, str]:
    """Create a throwaway code-signing certificate for a test release."""

    if os.name != "nt":
        raise ReleaseError("self-signed release signing requires Windows")
    pfx = directory / "self-signed.pfx"
    certificate = directory / "self-signed.cer"
    password = secrets.token_urlsafe(24)
    script = directory / "create-self-signed.ps1"
    script.write_text(
        """
$ErrorActionPreference = 'Stop'
$certificate = New-SelfSignedCertificate `
    -Type CodeSigningCert `
    -Subject 'CN=MarkTurbo Self-Signed' `
    -CertStoreLocation 'Cert:\\CurrentUser\\My' `
    -KeyExportPolicy Exportable `
    -NotAfter (Get-Date).AddDays([int]$env:MARKTURBO_SELF_SIGNED_DAYS)
$password = ConvertTo-SecureString -String $env:MARKTURBO_SELF_SIGNED_PASSWORD -Force -AsPlainText
Export-PfxCertificate -Cert $certificate -FilePath $env:MARKTURBO_SELF_SIGNED_PFX -Password $password | Out-Null
Export-Certificate -Cert $certificate -FilePath $env:MARKTURBO_SELF_SIGNED_CER | Out-Null
$thumbprint = $certificate.Thumbprint
$certificate | Remove-Item -Force
$thumbprint
""".lstrip(),
        encoding="utf-8",
        newline="\n",
    )
    environment = os.environ.copy()
    environment.update(
        {
            "MARKTURBO_SELF_SIGNED_DAYS": str(days),
            "MARKTURBO_SELF_SIGNED_PASSWORD": password,
            "MARKTURBO_SELF_SIGNED_PFX": str(pfx),
            "MARKTURBO_SELF_SIGNED_CER": str(certificate),
        }
    )
    try:
        completed = subprocess.run(
            [
                "powershell",
                "-NoProfile",
                "-NonInteractive",
                "-ExecutionPolicy",
                "Bypass",
                "-File",
                str(script),
            ],
            env=environment,
            capture_output=True,
            text=True,
            timeout=SIGN_TIMEOUT_SECONDS,
            check=False,
        )
    except OSError as error:
        raise ReleaseError(f"could not start PowerShell: {error}") from error
    except subprocess.TimeoutExpired as error:
        raise ReleaseError("creating the self-signed certificate timed out") from error
    if completed.returncode:
        raise ReleaseError(
            "could not create a self-signed certificate: "
            + (completed.stderr.strip() or completed.stdout.strip())
        )
    thumbprint = completed.stdout.strip().splitlines()[-1] if completed.stdout.strip() else ""
    if not thumbprint:
        raise ReleaseError("PowerShell did not report the self-signed certificate thumbprint")
    if not pfx.is_file() or not certificate.is_file():
        raise ReleaseError("PowerShell did not export the self-signed certificate")
    return (str(pfx), password), certificate, thumbprint


def sign_file(path: Path, credentials: SigningCredentials | None = None) -> None:
    credentials = credentials or _signing_credentials()
    if credentials is None:
        raise ReleaseError("signing was requested without MARKTURBO_SIGN_PFX credentials")
    pfx, password = credentials
    _run(
        [
            signtool(),
            "sign",
            "/fd",
            "sha256",
            "/f",
            pfx,
            "/p",
            password,
            "/tr",
            os.environ.get("MARKTURBO_SIGN_TIMESTAMP_URL", TIMESTAMP_URL),
            "/td",
            "sha256",
            str(path),
        ],
        secret=password,
    )


def verify_authenticode(path: Path, *, allow_untrusted: bool = False) -> None:
    try:
        completed = subprocess.run(
            [signtool(), "verify", "/pa", "/v", str(path)],
            capture_output=True,
            text=True,
            timeout=SIGN_TIMEOUT_SECONDS,
            check=False,
        )
    except OSError as error:
        raise ReleaseError(f"could not start signtool: {error}") from error
    except subprocess.TimeoutExpired as error:
        raise ReleaseError("signtool verification timed out") from error
    if completed.returncode == 0:
        return
    output = f"{completed.stdout}\n{completed.stderr}".lower()
    if allow_untrusted and ("not trusted" in output or "0x800b0109" in output):
        return
    raise ReleaseError(
        f"signtool verification failed for {path}: "
        + (completed.stderr.strip() or completed.stdout.strip())
    )


def has_pe_certificate(path: Path) -> bool:
    """Read the PE security directory, which carries an Authenticode signature."""

    data = path.read_bytes()
    if len(data) < 0x40 or data[:2] != b"MZ":
        return False
    pe_offset = int.from_bytes(data[0x3C:0x40], "little")
    if pe_offset + 24 > len(data) or data[pe_offset : pe_offset + 4] != b"PE\0\0":
        return False
    optional_size = int.from_bytes(data[pe_offset + 20 : pe_offset + 22], "little")
    optional = pe_offset + 24
    if optional + optional_size > len(data):
        return False
    magic = int.from_bytes(data[optional : optional + 2], "little")
    if magic == 0x20B:
        directories = optional + 112
    elif magic == 0x10B:
        directories = optional + 96
    else:
        return False
    entry = directories + 4 * 8
    if entry + 8 > len(data):
        return False
    address = int.from_bytes(data[entry : entry + 4], "little")
    size = int.from_bytes(data[entry + 4 : entry + 8], "little")
    return address != 0 and size != 0


def write_checksum(artifact: Path) -> Path:
    checksum = artifact.with_name(f"{artifact.name}.sha256")
    digest = _sha256(artifact)
    checksum.write_text(f"{digest}  {artifact.name}\n", encoding="utf-8", newline="\n")
    return checksum


def read_checksum(path: Path) -> tuple[str, str]:
    try:
        text = path.read_text(encoding="utf-8")
    except OSError as error:
        raise ReleaseError(f"could not read {path}: {error}") from error
    match = _CHECKSUM.fullmatch(text)
    if match is None:
        raise ReleaseError(f"{path} is not a sha256sum-format checksum file")
    return match.group("digest"), match.group("name")


def _safe_payload_entry(entry: object) -> dict[str, object]:
    if not isinstance(entry, dict):
        raise ReleaseError("release manifest payload entries must be objects")
    relative = entry.get("path")
    size = entry.get("bytes")
    if not isinstance(relative, str) or not relative:
        raise ReleaseError("release manifest payload entry has no path")
    path = Path(relative)
    if path.is_absolute() or ".." in path.parts or relative != path.as_posix():
        raise ReleaseError(f"release manifest payload path is not normalized: {relative}")
    if not isinstance(size, int) or size <= 0:
        raise ReleaseError(f"release manifest payload size is invalid for {relative}")
    return {"path": relative, "bytes": size}


def verify_artifact(
    manifest_path: Path,
    *,
    require_signature: bool = False,
    expected_version: str | None = None,
) -> dict[str, object]:
    """Verify one manifest, its artifact and the channel signing policy."""

    try:
        document = json.loads(manifest_path.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError) as error:
        raise ReleaseError(f"could not read {manifest_path}: {error}") from error
    if not isinstance(document, dict):
        raise ReleaseError(f"{manifest_path} is not an object")
    version_text = document.get("version")
    if not isinstance(version_text, str):
        raise ReleaseError("release manifest has no version")
    version = release_version(version_text)
    if expected_version is not None and version.version != expected_version:
        raise ReleaseError(
            f"release manifest version {version.version!r} does not match "
            f"repository version {expected_version!r}"
        )
    if document.get("product") != PRODUCT:
        raise ReleaseError("release manifest does not name the MarkTurbo product")
    if document.get("platform") != PLATFORM:
        raise ReleaseError("release manifest does not name the Windows x64 platform")
    if document.get("channel") != version.channel:
        raise ReleaseError("release manifest channel does not match its version")
    artifact_name = document.get("artifact")
    if artifact_name != version.artifact:
        raise ReleaseError(
            f"release manifest artifact {artifact_name!r} does not match {version.artifact!r}"
        )
    artifact = manifest_path.parent / version.artifact
    if not artifact.is_file():
        raise ReleaseError(f"release artifact is missing: {artifact}")
    if document.get("checksum_file") != version.checksum:
        raise ReleaseError(
            f"release manifest checksum file {document.get('checksum_file')!r} "
            f"does not match {version.checksum!r}"
        )
    checksum_path = manifest_path.parent / version.checksum
    digest, checksum_name = read_checksum(checksum_path)
    if checksum_name != artifact.name:
        raise ReleaseError(f"{checksum_path} names {checksum_name!r}, not {artifact.name!r}")
    if digest != _sha256(artifact):
        raise ReleaseError(f"{artifact} does not match {checksum_path}")
    if document.get("sha256") != digest:
        raise ReleaseError("release manifest digest does not match the checksum file")
    if document.get("bytes") != artifact.stat().st_size:
        raise ReleaseError("release manifest byte size does not match the artifact")
    payload = document.get("payload")
    if not isinstance(payload, list) or not payload:
        raise ReleaseError("release manifest has no payload inventory")
    entries = [_safe_payload_entry(entry) for entry in payload]
    paths = [str(entry["path"]) for entry in entries]
    if len(set(paths)) != len(paths):
        raise ReleaseError("release manifest payload inventory repeats a path")
    for required in REQUIRED_PAYLOAD:
        if required not in paths:
            raise ReleaseError(f"release manifest omits required payload {required}")
    signing = document.get("signing")
    signed = has_pe_certificate(artifact)
    if signing == "authenticode":
        if not signed:
            raise ReleaseError("release manifest claims a signature the artifact lacks")
        verify_authenticode(artifact)
    elif signing == "self-signed":
        if not signed:
            raise ReleaseError("release manifest claims a self-signed signature the artifact lacks")
        if version.channel == "stable":
            raise ReleaseError("a stable release cannot be signed with a self-signed certificate")
        if require_signature:
            raise ReleaseError(
                "this channel requires trusted Authenticode signing; "
                "a self-signed certificate is not publication signing"
            )
        certificate_name = document.get("certificate")
        certificate_digest = document.get("certificate_sha256")
        if not isinstance(certificate_name, str) or not isinstance(certificate_digest, str):
            raise ReleaseError("a self-signed release manifest must name its certificate")
        certificate_relative = Path(certificate_name)
        if (
            certificate_relative.is_absolute()
            or ".." in certificate_relative.parts
            or certificate_relative.as_posix() != certificate_name
        ):
            raise ReleaseError("release manifest certificate path is not normalized")
        if re.fullmatch(r"[0-9a-f]{64}", certificate_digest) is None:
            raise ReleaseError("release manifest certificate digest is invalid")
        certificate_path = manifest_path.parent / certificate_name
        if not certificate_path.is_file():
            raise ReleaseError(f"release certificate is missing: {certificate_path}")
        if _sha256(certificate_path) != certificate_digest:
            raise ReleaseError("release certificate does not match its manifest digest")
    elif signing == "unsigned":
        if signed:
            raise ReleaseError("release artifact is signed but the manifest says unsigned")
        if require_signature:
            raise ReleaseError("this channel requires a signed artifact; got an unsigned one")
    else:
        raise ReleaseError(
            "release manifest signing state must be authenticode, self-signed or unsigned"
        )
    return document


def package_release(
    output: Path,
    *,
    payload_source: Path | None = None,
    sign: bool = True,
    require_signature: bool = False,
    self_signed: bool = False,
) -> dict[str, object]:
    version = repository_version()
    if require_signature and not sign:
        raise ReleaseError("signing is required for this release; --no-sign is not allowed")
    if self_signed and not sign:
        raise ReleaseError("--self-signed cannot be combined with --no-sign")
    if require_signature and self_signed:
        raise ReleaseError("a stable release requires trusted Authenticode signing")
    if self_signed and _signing_credentials() is not None:
        raise ReleaseError(
            "configured MARKTURBO_SIGN_PFX credentials conflict with --self-signed"
        )
    credentials = _signing_credentials() if sign and not self_signed else None
    if require_signature and credentials is None:
        raise ReleaseError(
            "this release requires Authenticode signing, but signing credentials are unavailable"
        )
    output.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix="markturbo-release-") as temporary:
        scratch = Path(temporary)
        certificate_source: Path | None = None
        certificate_thumbprint: str | None = None
        if self_signed:
            credentials, certificate_source, certificate_thumbprint = (
                generate_self_signed_certificate(scratch)
            )
        packaged = (
            payload_source if payload_source is not None else build_packaged_app(scratch / "package")
        )
        payload = scratch / "payload"
        stage_payload(packaged, payload)
        verify_binary(payload / "MarkTurbo.exe", version)
        executable = payload / "MarkTurbo.exe"
        try:
            privacy.scan(ROOT, executable)
        except privacy.PrivacyScanError as error:
            raise ReleaseError(str(error)) from None
        if credentials is not None:
            sign_file(executable, credentials)
            verify_authenticode(executable, allow_untrusted=self_signed)
        icon = render_icon(scratch / "MarkTurbo.ico")
        artifact = compile_installer(payload, output, version, icon=icon)
        if credentials is not None:
            sign_file(artifact, credentials)
            verify_authenticode(artifact, allow_untrusted=self_signed)
        signing = "self-signed" if self_signed else (
            "authenticode" if credentials is not None else "unsigned"
        )
        if signing == "unsigned" and require_signature:
            raise ReleaseError("signing was required but the artifact is unsigned")
        certificate_name: str | None = None
        if self_signed and certificate_source is not None:
            certificate_name = f"{PRODUCT}-Setup-{version.version}.cer"
            shutil.copyfile(certificate_source, output / certificate_name)
        artifact_digest = _sha256(artifact)
        checksum = write_checksum(artifact)
        document: dict[str, object] = {
            "product": PRODUCT,
            "version": version.version,
            "channel": version.channel,
            "platform": PLATFORM,
            "artifact": artifact.name,
            "checksum_file": checksum.name,
            "sha256": artifact_digest,
            "bytes": artifact.stat().st_size,
            "signing": signing,
            "payload": payload_inventory(payload),
        }
        if certificate_name is not None:
            document["certificate"] = certificate_name
            document["certificate_sha256"] = _sha256(output / certificate_name)
            document["certificate_thumbprint"] = certificate_thumbprint
        manifest = output / "release.json"
        manifest.write_text(
            json.dumps(document, indent=2, sort_keys=True) + "\n",
            encoding="utf-8",
            newline="\n",
        )
    if require_signature:
        verify_artifact(manifest, require_signature=True, expected_version=version.version)
    return document


def verify_command(args: argparse.Namespace) -> int:
    manifest = Path(args.manifest)
    expected = None if args.ignore_repository_version else repository_version().version
    document = verify_artifact(
        manifest,
        require_signature=args.require_signature,
        expected_version=expected,
    )
    print(
        f"{document['artifact']}: sha256 {document['sha256']}, "
        f"signing {document['signing']}, channel {document['channel']}"
    )
    return 0


def versions_command(args: argparse.Namespace) -> int:
    version = repository_version(args.tag)
    print(
        json.dumps(
            {
                "version": version.version,
                "numeric": version.numeric,
                "channel": version.channel,
                "artifact": version.artifact,
                "checksum": version.checksum,
            },
            indent=2,
            sort_keys=True,
        )
    )
    return 0


def package_command(args: argparse.Namespace) -> int:
    payload = Path(args.payload) if args.payload else None
    document = package_release(
        Path(args.out),
        payload_source=payload,
        sign=args.sign,
        require_signature=args.require_signature,
        self_signed=args.self_signed,
    )
    print(
        f"{document['artifact']}: sha256 {document['sha256']}, "
        f"signing {document['signing']} (manifest {Path(args.out) / 'release.json'})"
    )
    return 0


def parser() -> argparse.ArgumentParser:
    result = argparse.ArgumentParser(
        prog="mt.py release",
        description="Build and verify the Windows release installer.",
    )
    subcommands = result.add_subparsers(dest="command", required=True)

    package = subcommands.add_parser("package", help="Build the installer and manifest.")
    package.add_argument("--out", default="dist", help="output directory (default: dist)")
    package.add_argument(
        "--payload",
        help="use an existing nsh package directory instead of running nsh package",
    )
    package.add_argument(
        "--no-sign",
        dest="sign",
        action="store_false",
        help="do not sign even when signing credentials are configured",
    )
    package.add_argument(
        "--require-signature",
        action="store_true",
        help="fail unless the artifact is signed",
    )
    package.add_argument(
        "--self-signed",
        action="store_true",
        help="sign a test build with a throwaway self-signed certificate",
    )
    package.set_defaults(handler=package_command)

    verify = subcommands.add_parser("verify", help="Verify an artifact and its manifest.")
    verify.add_argument("--manifest", default="dist/release.json")
    verify.add_argument("--require-signature", action="store_true")
    verify.add_argument(
        "--ignore-repository-version",
        action="store_true",
        help="verify a downloaded artifact without comparing it to this checkout",
    )
    verify.set_defaults(handler=verify_command)

    versions = subcommands.add_parser("versions", help="Check version agreement.")
    versions.add_argument("--tag", help="release tag that must match the version")
    versions.set_defaults(handler=versions_command)
    return result


def main(argv: Sequence[str] | None = None) -> int:
    args = parser().parse_args(argv)
    try:
        return int(args.handler(args))
    except ReleaseError as error:
        print(f"release: {error}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
