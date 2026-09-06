"""Secret scans for repository workspace files and the release binary."""

from __future__ import annotations

import os
import subprocess
from collections.abc import Mapping
from pathlib import Path


SECRET_SOURCES = (
    "OPENAI_API_KEY",
    "ANTHROPIC_API_KEY",
    "MARKTURBO_PRIVACY_SENTINEL",
    "MARKTURBO_PRIVATE_REQUEST_SENTINEL",
)
EXCLUDED_UNTRACKED_PATHS = frozenset({".git", "target"})
Candidate = tuple[str, str, tuple[bytes, ...]]


class PrivacyScanError(RuntimeError):
    """A privacy scan could not complete or found a candidate secret."""


def scan(
    repository: Path,
    release_binary: Path,
    *,
    environment: Mapping[str, str] | None = None,
) -> None:
    """Fail when an environment-provided secret occurs in workspace files or the binary."""

    environment = os.environ if environment is None else environment
    candidates: list[Candidate] = []
    for source in SECRET_SOURCES:
        value = environment.get(source)
        if not value:
            continue
        try:
            encodings = tuple(
                dict.fromkeys(
                    (
                        value.encode("utf-8"),
                        value.encode("utf-16-le"),
                        value.encode("utf-16-be"),
                    )
                )
            )
        except UnicodeEncodeError:
            raise PrivacyScanError(f"privacy scan could not encode {source}") from None
        candidates.append((source, value, encodings))
    if not candidates:
        return

    findings: set[tuple[str, str]] = set()
    for object_id, relative_path in _git_index_entries(repository):
        normalized = relative_path.as_posix()
        findings.update(_find_path_candidates(normalized, candidates))
        if object_id is None:
            continue
        findings.update(_scan_index_entry(repository, object_id, normalized, candidates))
        path = repository / relative_path
        if not os.path.lexists(path):
            continue
        findings.update(
            _scan_workspace_entry(repository, relative_path, normalized, candidates)
        )
    for relative_path in _git_paths(repository, "--others", "--exclude-standard"):
        if relative_path.parts and relative_path.parts[0] in EXCLUDED_UNTRACKED_PATHS:
            continue
        normalized = relative_path.as_posix()
        findings.update(_find_path_candidates(normalized, candidates))
        findings.update(
            _scan_workspace_entry(repository, relative_path, normalized, candidates)
        )

    try:
        binary_name = release_binary.relative_to(repository).as_posix()
    except ValueError:
        binary_name = str(release_binary)
    findings.update(_find_path_candidates(binary_name, candidates))
    findings.update(_scan_file(release_binary, binary_name, candidates))

    if findings:
        details = "\n".join(f"{source}: {path}" for source, path in sorted(findings))
        raise PrivacyScanError(f"privacy scan found candidate secrets:\n{details}")


def _git_paths(repository: Path, *arguments: str) -> list[Path]:
    try:
        completed = subprocess.run(
            ("git", "ls-files", *arguments, "-z"),
            cwd=repository,
            check=False,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
        )
    except OSError:
        raise PrivacyScanError("privacy scan could not list repository workspace files") from None
    if completed.returncode:
        raise PrivacyScanError("privacy scan could not list repository workspace files")
    return [Path(os.fsdecode(path)) for path in completed.stdout.split(b"\0") if path]


def _git_index_entries(repository: Path) -> list[tuple[str | None, Path]]:
    try:
        completed = subprocess.run(
            ("git", "ls-files", "--stage", "-z"),
            cwd=repository,
            check=False,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
        )
    except OSError:
        raise PrivacyScanError("privacy scan could not list repository index entries") from None
    if completed.returncode:
        raise PrivacyScanError("privacy scan could not list repository index entries")

    entries: list[tuple[str | None, Path]] = []
    for record in completed.stdout.split(b"\0"):
        if not record:
            continue
        metadata, separator, raw_path = record.partition(b"\t")
        fields = metadata.split()
        if not separator or len(fields) != 3:
            raise PrivacyScanError("privacy scan received an invalid repository index entry")
        mode, object_id, _stage = fields
        try:
            decoded_id = object_id.decode("ascii")
        except UnicodeDecodeError:
            raise PrivacyScanError("privacy scan received an invalid repository object id") from None
        entries.append((None if mode == b"160000" else decoded_id, Path(os.fsdecode(raw_path))))
    return entries


def _scan_index_entry(
    repository: Path,
    object_id: str,
    display_path: str,
    candidates: list[Candidate],
) -> list[tuple[str, str]]:
    safe_path = _redact_path(display_path, candidates)
    try:
        completed = subprocess.run(
            ("git", "cat-file", "blob", object_id),
            cwd=repository,
            check=False,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
        )
    except OSError:
        raise PrivacyScanError(f"privacy scan could not read index entry {safe_path}") from None
    if completed.returncode:
        raise PrivacyScanError(f"privacy scan could not read index entry {safe_path}")
    return _find_candidates(completed.stdout, safe_path, candidates)


def _scan_workspace_entry(
    repository: Path,
    relative_path: Path,
    display_path: str,
    candidates: list[Candidate],
) -> list[tuple[str, str]]:
    path = repository
    for part in relative_path.parts[:-1]:
        path /= part
        if path.is_symlink():
            return []
    path /= relative_path.parts[-1]
    if not path.is_symlink():
        return _scan_file(path, display_path, candidates)

    safe_path = _redact_path(display_path, candidates)
    try:
        content = os.fsencode(os.readlink(path))
    except OSError:
        raise PrivacyScanError(f"privacy scan could not read {safe_path}") from None
    return _find_candidates(content, safe_path, candidates)


def _scan_file(
    path: Path,
    display_path: str,
    candidates: list[Candidate],
) -> list[tuple[str, str]]:
    safe_path = _redact_path(display_path, candidates)
    try:
        content = path.read_bytes()
    except OSError:
        raise PrivacyScanError(f"privacy scan could not read {safe_path}") from None

    return _find_candidates(content, safe_path, candidates)


def _redact_path(display_path: str, candidates: list[Candidate]) -> str:
    matches: list[tuple[int, int]] = []
    for _, value, _ in candidates:
        start = 0
        while (found := display_path.find(value, start)) >= 0:
            matches.append((found, found + len(value)))
            start = found + 1
    if not matches:
        return display_path

    merged: list[tuple[int, int]] = []
    for start, end in sorted(matches):
        if merged and start <= merged[-1][1]:
            merged[-1] = (merged[-1][0], max(merged[-1][1], end))
        else:
            merged.append((start, end))

    parts: list[str] = []
    previous_end = 0
    for start, end in merged:
        parts.extend((display_path[previous_end:start], "<redacted>"))
        previous_end = end
    parts.append(display_path[previous_end:])
    return "".join(parts)


def _find_path_candidates(
    display_path: str,
    candidates: list[Candidate],
) -> list[tuple[str, str]]:
    safe_path = _redact_path(display_path, candidates)
    return [
        (source, safe_path)
        for source, value, _ in candidates
        if value in display_path
    ]


def _find_candidates(
    content: bytes,
    safe_path: str,
    candidates: list[Candidate],
) -> list[tuple[str, str]]:
    return [
        (source, safe_path)
        for source, _, encodings in candidates
        if any(value in content for value in encodings)
    ]
