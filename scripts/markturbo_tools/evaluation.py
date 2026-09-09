"""Offline Goal 06 Review evaluation metadata and evidence tooling.

This module deliberately has no HTTP, model SDK, or subprocess dependency. It
verifies the immutable checked-in corpus and turns owner-local, metadata-only
judgments into a content-free evidence record. A missing or invalid owner
input is represented as a blocked scaffold; the tool never invents model
responses or threshold results.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import re
import sys
from dataclasses import dataclass
from datetime import UTC, datetime
from pathlib import Path, PurePosixPath
from typing import Mapping, Sequence


REPO = Path(__file__).resolve().parents[2]
CORPUS_VERSION = "goal-01-v1"
CORPUS_RELATIVE = Path("evaluation") / "goal-01"
MANIFEST_RELATIVE = CORPUS_RELATIVE / "MANIFEST.sha256"
MANIFEST_SCHEMA = "markturbo-goal-06-manifest-v1"
EVIDENCE_SCHEMA = "markturbo-goal-06-evaluation-v1"
REFERENCE_MODEL_REPORTED_ID = "gpt-5.6-terra"
# Owner-approved digest for the immutable goal-01-v1 manifest bytes.
APPROVED_MANIFEST_SHA256 = "290a0421753974dabb0cef1552f158cba4ddf2cafd3d91f0ea256d406f561f7c"
EXPECTED_ARTIFACT_COUNT = 12
EXPECTED_SCORING_ITEM_COUNT = 60
MIN_USEFUL_COUNT = 10
MIN_SURFACED_ITEM_COUNT = 45
MAX_FALSE_SOURCE_ANCHOR_COUNT = 0
MAX_MATERIALLY_MISLEADING_COUNT = 1
MAX_QUESTION_COUNT = 5
SHA256_PATTERN = re.compile(r"[0-9a-f]{64}")
ARTIFACT_ID_PATTERN = re.compile(r"(?:TP|SP|AI|AS)-[0-9]{2}")
ITEM_ID_PATTERN = re.compile(r"(?:TP|SP|AI|AS)-[0-9]{2}-HI-[0-9]{2}")
MODEL_ID_PATTERN = re.compile(r"[A-Za-z0-9][A-Za-z0-9._:/-]{0,127}")
TIMESTAMP_PATTERN = re.compile(
    r"[0-9]{4}-[0-9]{2}-[0-9]{2}T[0-9]{2}:[0-9]{2}:[0-9]{2}(?:\.[0-9]+)?(?:Z|[+-][0-9]{2}:[0-9]{2})"
)

_CORPUS_VERSION_HEADER = "# corpus-version: goal-01-v1"
_MANIFEST_FORMAT_HEADER = "# format: sha256  repository-relative-path"
_MANIFEST_ENTRY_PATTERN = re.compile(r"([0-9a-f]{64})  ([^\r\n]+)")
_ARTIFACT_ROW_PATTERN = re.compile(r"^(TP|SP|AI|AS)-[0-9]{2}$")
_SCORING_SECTION_PATTERN = re.compile(r"^### ((?:TP|SP|AI|AS)-[0-9]{2}) - .+$")
_SCORING_ITEM_PATTERN = re.compile(r"^\| `((?:TP|SP|AI|AS)-[0-9]{2}-HI-[0-9]{2})` \| .+ \|$")


class EvaluationError(ValueError):
    """A checked-in corpus or owner-local evidence input is invalid."""


class OwnerInputRequired(EvaluationError):
    """The offline runner cannot proceed without owner-local judgments."""


@dataclass(frozen=True)
class ManifestEntry:
    path: str
    sha256: str
    byte_count: int


@dataclass(frozen=True)
class ArtifactMetadata:
    artifact_id: str
    lens: str
    path: str
    file_count: int
    byte_count: int
    sha256: str
    files: tuple[ManifestEntry, ...]

    def evidence(self) -> dict[str, object]:
        return {
            "artifact_id": self.artifact_id,
            "lens": self.lens,
            "path": self.path,
            "file_count": self.file_count,
            "byte_count": self.byte_count,
            "sha256": self.sha256,
            "files": [
                {
                    "path": item.path,
                    "byte_count": item.byte_count,
                    "sha256": item.sha256,
                }
                for item in self.files
            ],
        }


@dataclass(frozen=True)
class ManifestVerification:
    root: Path
    corpus_version: str
    manifest_sha256: str
    entries: tuple[ManifestEntry, ...]
    artifacts: tuple[ArtifactMetadata, ...]
    scoring_item_ids: tuple[tuple[str, tuple[str, ...]], ...]

    def evidence(self) -> dict[str, object]:
        return {
            "schema": MANIFEST_SCHEMA,
            "corpus_version": self.corpus_version,
            "manifest_sha256": self.manifest_sha256,
            "manifest_entry_count": len(self.entries),
            "artifact_count": len(self.artifacts),
            "artifacts": [artifact.evidence() for artifact in self.artifacts],
        }

    def item_ids_for(self, artifact_id: str) -> frozenset[str]:
        for scored_artifact_id, item_ids in self.scoring_item_ids:
            if scored_artifact_id == artifact_id:
                return frozenset(item_ids)
        raise EvaluationError("scoring registry does not define the artifact")


def _sha256(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def _safe_relative_path(value: object, *, field: str) -> str:
    if not isinstance(value, str) or not value or "\\" in value:
        raise EvaluationError(f"{field} must be a normalized repository-relative path")
    path = PurePosixPath(value)
    if path.is_absolute() or ".." in path.parts or "." in path.parts:
        raise EvaluationError(f"{field} must be a normalized repository-relative path")
    normalized = path.as_posix()
    if normalized != value:
        raise EvaluationError(f"{field} must be a normalized repository-relative path")
    return value


def _safe_artifact_id(value: object) -> str:
    if not isinstance(value, str) or ARTIFACT_ID_PATTERN.fullmatch(value) is None:
        raise EvaluationError("artifact_id is invalid")
    return value


def _safe_item_ids(
    value: object,
    *,
    field: str,
    allowed_item_ids: frozenset[str] | None = None,
) -> list[str]:
    if not isinstance(value, list):
        raise EvaluationError(f"{field} must be a list")
    result: list[str] = []
    for item in value:
        if not isinstance(item, str) or ITEM_ID_PATTERN.fullmatch(item) is None:
            raise EvaluationError(f"{field} contains an invalid item identifier")
        if allowed_item_ids is not None and item not in allowed_item_ids:
            raise EvaluationError(f"{field} contains an item not fixed for this artifact")
        result.append(item)
    if result != sorted(set(result)):
        raise EvaluationError(f"{field} must be sorted and duplicate-free")
    return result


def _safe_timestamp(value: object) -> str:
    if not isinstance(value, str) or TIMESTAMP_PATTERN.fullmatch(value) is None:
        raise EvaluationError("created_at must be an RFC 3339 timestamp")
    try:
        parsed = datetime.fromisoformat(value.replace("Z", "+00:00"))
    except ValueError as error:
        raise EvaluationError("created_at must be an RFC 3339 timestamp") from error
    if parsed.tzinfo is None:
        raise EvaluationError("created_at must be an RFC 3339 timestamp")
    return value


def _safe_model_id(value: object) -> str | None:
    if value is None:
        return None
    if (
        not isinstance(value, str)
        or MODEL_ID_PATTERN.fullmatch(value) is None
        or value.lower().startswith(
            (
                "sk-",
                "sk_",
                "gsk_",
                "xai-",
                "hf_",
                "ghp_",
                "github_pat_",
                "glpat-",
                "glpat_",
                "bearer-",
                "bearer_",
            )
        )
        or value.startswith(("AIza", "AKIA"))
        or "://" in value
        or value.startswith("/")
        or re.fullmatch(r"[A-Za-z]:[\\/].*", value) is not None
    ):
        raise EvaluationError("model_reported_id is invalid")
    return value


def _safe_nonnegative_count(value: object, *, field: str) -> int:
    if not isinstance(value, int) or isinstance(value, bool) or value < 0:
        raise EvaluationError(f"{field} must be a non-negative integer")
    return value


def _read_bytes(path: Path, *, label: str) -> bytes:
    try:
        return path.read_bytes()
    except OSError as error:
        raise EvaluationError(f"could not read {label}") from error


def _parse_manifest(manifest_bytes: bytes) -> tuple[str, list[ManifestEntry]]:
    try:
        text = manifest_bytes.decode("utf-8")
    except UnicodeDecodeError as error:
        raise EvaluationError("manifest is not valid UTF-8") from error

    lines = text.splitlines()
    if len(lines) < 3 or lines[0] != _CORPUS_VERSION_HEADER or lines[1] != _MANIFEST_FORMAT_HEADER:
        raise EvaluationError("manifest headers do not match the approved corpus")

    entries: list[ManifestEntry] = []
    seen: set[str] = set()
    for line in lines[2:]:
        if not line:
            continue
        match = _MANIFEST_ENTRY_PATTERN.fullmatch(line)
        if match is None:
            raise EvaluationError("manifest contains an invalid entry")
        digest, path = match.groups()
        path = _safe_relative_path(path, field="manifest path")
        if path in seen:
            raise EvaluationError("manifest contains a duplicate path")
        seen.add(path)
        entries.append(ManifestEntry(path=path, sha256=digest, byte_count=-1))
    if not entries:
        raise EvaluationError("manifest contains no entries")
    return CORPUS_VERSION, entries


def _artifact_rows(corpus_bytes: bytes) -> list[tuple[str, str, str]]:
    try:
        text = corpus_bytes.decode("utf-8")
    except UnicodeDecodeError as error:
        raise EvaluationError("CORPUS.md is not valid UTF-8") from error

    rows: list[tuple[str, str, str]] = []
    for line in text.splitlines():
        if not line.startswith("|"):
            continue
        columns = [column.strip() for column in line.strip().strip("|").split("|")]
        if len(columns) < 3 or _ARTIFACT_ROW_PATTERN.fullmatch(columns[0]) is None:
            continue
        artifact_id, lens, path = columns[:3]
        path = path.strip("`")
        corpus_relative_path = path.rstrip("/")
        _safe_relative_path(corpus_relative_path, field="corpus artifact path")
        path = f"{CORPUS_RELATIVE.as_posix()}/{corpus_relative_path}"
        if any(existing[0] == artifact_id for existing in rows):
            raise EvaluationError("CORPUS.md contains a duplicate artifact id")
        rows.append((artifact_id, lens, path))
    if len(rows) != EXPECTED_ARTIFACT_COUNT:
        raise EvaluationError("CORPUS.md does not define exactly 12 evaluation artifacts")
    return rows


def _parse_scoring_registry(
    scoring_bytes: bytes,
    *,
    artifact_ids: Sequence[str],
) -> tuple[tuple[str, tuple[str, ...]], ...]:
    """Read the immutable scoring registry into artifact-bound item identifiers."""

    try:
        text = scoring_bytes.decode("utf-8")
    except UnicodeDecodeError as error:
        raise EvaluationError("SCORING.md is not valid UTF-8") from error

    expected_artifact_ids = set(artifact_ids)
    items_by_artifact: dict[str, list[str]] = {}
    current_artifact_id: str | None = None
    for line in text.splitlines():
        section_match = _SCORING_SECTION_PATTERN.fullmatch(line)
        if section_match is not None:
            current_artifact_id = section_match.group(1)
            if current_artifact_id not in expected_artifact_ids:
                raise EvaluationError("SCORING.md defines an unknown artifact")
            if current_artifact_id in items_by_artifact:
                raise EvaluationError("SCORING.md defines an artifact more than once")
            items_by_artifact[current_artifact_id] = []
            continue

        item_match = _SCORING_ITEM_PATTERN.fullmatch(line)
        if item_match is None:
            continue
        if current_artifact_id is None:
            raise EvaluationError("SCORING.md item is not assigned to an artifact")
        item_id = item_match.group(1)
        if not item_id.startswith(f"{current_artifact_id}-HI-"):
            raise EvaluationError("SCORING.md item does not belong to its artifact")
        items_by_artifact[current_artifact_id].append(item_id)

    if set(items_by_artifact) != expected_artifact_ids:
        raise EvaluationError("SCORING.md does not define every corpus artifact")
    item_ids = [item_id for items in items_by_artifact.values() for item_id in items]
    if len(item_ids) != EXPECTED_SCORING_ITEM_COUNT or len(set(item_ids)) != EXPECTED_SCORING_ITEM_COUNT:
        raise EvaluationError("SCORING.md does not define exactly 60 unique items")
    if any(items != sorted(items) for items in items_by_artifact.values()):
        raise EvaluationError("SCORING.md item identifiers must be sorted")
    return tuple((artifact_id, tuple(items_by_artifact[artifact_id])) for artifact_id in artifact_ids)


def _is_within(path: Path, parent: Path) -> bool:
    try:
        path.resolve().relative_to(parent.resolve())
    except ValueError:
        return False
    return True


def _directory_digest(files: Sequence[ManifestEntry]) -> str:
    digest = hashlib.sha256()
    for item in files:
        path_bytes = item.path.encode("utf-8")
        digest.update(len(path_bytes).to_bytes(8, "big"))
        digest.update(path_bytes)
        digest.update(item.byte_count.to_bytes(8, "big"))
        digest.update(bytes.fromhex(item.sha256))
    return digest.hexdigest()


def verify_manifest(root: Path = REPO, manifest_path: Path | None = None) -> ManifestVerification:
    """Verify the approved corpus manifest and return content-free metadata."""

    root = root.resolve()
    manifest_path = (root / MANIFEST_RELATIVE) if manifest_path is None else manifest_path
    manifest_path = manifest_path.resolve()
    corpus_root = (root / CORPUS_RELATIVE).resolve()
    if not _is_within(manifest_path, corpus_root) or manifest_path.name != "MANIFEST.sha256":
        raise EvaluationError("manifest path must be evaluation/goal-01/MANIFEST.sha256")
    if not corpus_root.is_dir() or manifest_path.is_symlink():
        raise EvaluationError("approved evaluation corpus is unavailable")

    manifest_bytes = _read_bytes(manifest_path, label="manifest")
    manifest_sha256 = _sha256(manifest_bytes)
    if manifest_sha256 != APPROVED_MANIFEST_SHA256:
        raise EvaluationError("manifest digest does not match the approved immutable trust anchor")
    corpus_version, parsed_entries = _parse_manifest(manifest_bytes)
    if corpus_version != CORPUS_VERSION:
        raise EvaluationError("manifest corpus version is not goal-01-v1")

    expected_paths = {entry.path for entry in parsed_entries}
    actual_paths: set[str] = set()
    for candidate in corpus_root.rglob("*"):
        if candidate.is_symlink():
            raise EvaluationError("evaluation corpus must not contain symbolic links")
        if candidate.is_file():
            relative = candidate.relative_to(root).as_posix()
            if relative != MANIFEST_RELATIVE.as_posix():
                actual_paths.add(relative)
    if actual_paths != expected_paths:
        raise EvaluationError("manifest coverage does not match regular files in the corpus")

    entries: list[ManifestEntry] = []
    for parsed in parsed_entries:
        candidate = (root / Path(*PurePosixPath(parsed.path).parts)).resolve()
        if not _is_within(candidate, corpus_root) or not candidate.is_file() or candidate.is_symlink():
            raise EvaluationError("manifest references an unavailable corpus file")
        data = _read_bytes(candidate, label="corpus file")
        digest = _sha256(data)
        if digest != parsed.sha256:
            raise EvaluationError("corpus manifest hash mismatch")
        entries.append(ManifestEntry(path=parsed.path, sha256=digest, byte_count=len(data)))

    entry_by_path = {entry.path: entry for entry in entries}
    rows = _artifact_rows(_read_bytes(corpus_root / "CORPUS.md", label="CORPUS.md"))
    artifacts: list[ArtifactMetadata] = []
    for artifact_id, lens, artifact_path in rows:
        root_path = artifact_path.rstrip("/")
        candidate = (root / Path(*PurePosixPath(root_path).parts)).resolve()
        if not _is_within(candidate, corpus_root):
            raise EvaluationError("artifact path escapes the approved corpus")
        prefix = root_path + "/"
        if candidate.is_file():
            selected = [entry_by_path[root_path]] if root_path in entry_by_path else []
        elif candidate.is_dir():
            selected = [entry for path, entry in entry_by_path.items() if path.startswith(prefix)]
        else:
            raise EvaluationError("CORPUS.md references an unavailable artifact")
        selected.sort(key=lambda item: item.path)
        if not selected:
            raise EvaluationError("CORPUS.md references an empty artifact")
        artifacts.append(
            ArtifactMetadata(
                artifact_id=artifact_id,
                lens=lens,
                path=root_path,
                file_count=len(selected),
                byte_count=sum(item.byte_count for item in selected),
                sha256=_directory_digest(selected),
                files=tuple(selected),
            )
        )

    if len(artifacts) != EXPECTED_ARTIFACT_COUNT:
        raise EvaluationError("verified artifact count is not 12")
    scoring_path = f"{CORPUS_RELATIVE.as_posix()}/SCORING.md"
    scoring_entry = entry_by_path.get(scoring_path)
    if scoring_entry is None:
        raise EvaluationError("manifest does not cover SCORING.md")
    scoring_item_ids = _parse_scoring_registry(
        _read_bytes(root / Path(*PurePosixPath(scoring_entry.path).parts), label="SCORING.md"),
        artifact_ids=[artifact.artifact_id for artifact in artifacts],
    )
    return ManifestVerification(
        root=root,
        corpus_version=corpus_version,
        manifest_sha256=manifest_sha256,
        entries=tuple(entries),
        artifacts=tuple(artifacts),
        scoring_item_ids=scoring_item_ids,
    )


def _created_at(value: str | None) -> str:
    if value is None:
        return datetime.now(UTC).replace(microsecond=0).isoformat().replace("+00:00", "Z")
    return _safe_timestamp(value)


def _base_configuration() -> dict[str, object]:
    return {
        "provider_wire_format": "openai-responses",
        "model_requested": "gpt-5.6-terra",
        "reasoning_effort": "medium",
        "max_output_tokens": 8192,
        "sampling": "provider-defaults-omitted",
        "prompt_version": "review-v1",
        "tools": False,
        "browsing": False,
        "memory": False,
        "agent_actions": False,
    }


def _result_scaffold(verification: ManifestVerification, artifact: ArtifactMetadata) -> dict[str, object]:
    return {
        "artifact_id": artifact.artifact_id,
        "corpus_version": verification.corpus_version,
        "manifest_sha256": verification.manifest_sha256,
        "artifact_sha256": artifact.sha256,
        "artifact_byte_count": artifact.byte_count,
        "status": "awaiting_owner_input",
        "decoded_completely": None,
        "surfaced_item_ids": [],
        "unsupported_claim_ids": [],
        "unsupported_claim_count": None,
        "false_source_anchor_count": None,
        "boilerplate_question_count": None,
        "question_count": None,
        "materially_misleading": None,
        "usefulness": None,
        "model_reported_id": None,
    }


def scaffold_evidence(
    verification: ManifestVerification,
    *,
    created_at: str | None = None,
    reason: str = "owner_local_response_inputs_required",
) -> dict[str, object]:
    """Return a fail-closed 12-artifact evidence scaffold."""

    evidence = {
        "schema": EVIDENCE_SCHEMA,
        "created_at": _created_at(created_at),
        "corpus": {
            "version": verification.corpus_version,
            "manifest_sha256": verification.manifest_sha256,
            "artifact_count": len(verification.artifacts),
        },
        "configuration": _base_configuration(),
        "artifact_metadata": [artifact.evidence() for artifact in verification.artifacts],
        "results": [_result_scaffold(verification, artifact) for artifact in verification.artifacts],
        "evaluation": {
            "status": "not_evaluated",
            "eligible_for_threshold": False,
            "reason": reason,
        },
    }
    validate_evidence(evidence, verification=verification)
    return evidence


_OWNER_INPUT_KEYS = frozenset(
    {
        "artifact_id",
        "decoded_completely",
        "surfaced_item_ids",
        "unsupported_claim_ids",
        "unsupported_claim_count",
        "false_source_anchor_count",
        "boilerplate_question_count",
        "question_count",
        "materially_misleading",
        "usefulness",
        "model_reported_id",
    }
)


def _load_owner_input(
    value: object,
    *,
    artifact_id: str,
    allowed_item_ids: frozenset[str],
) -> dict[str, object]:
    if not isinstance(value, dict) or set(value) != _OWNER_INPUT_KEYS:
        raise EvaluationError("owner-local input must contain metadata-only judgment fields")
    if _safe_artifact_id(value.get("artifact_id")) != artifact_id:
        raise EvaluationError("owner-local input artifact_id does not match its artifact")
    decoded = value.get("decoded_completely")
    if not isinstance(decoded, bool):
        raise EvaluationError("decoded_completely must be boolean")
    surfaced_item_ids = _safe_item_ids(
        value.get("surfaced_item_ids"),
        field="surfaced_item_ids",
        allowed_item_ids=allowed_item_ids,
    )
    unsupported_claim_ids = _safe_item_ids(
        value.get("unsupported_claim_ids"),
        field="unsupported_claim_ids",
        allowed_item_ids=allowed_item_ids,
    )
    unsupported_claim_count = _safe_nonnegative_count(
        value.get("unsupported_claim_count"), field="unsupported_claim_count"
    )
    if unsupported_claim_count < len(unsupported_claim_ids):
        raise EvaluationError("unsupported_claim_count cannot be below unsupported_claim_ids")
    _safe_nonnegative_count(
        value.get("false_source_anchor_count"), field="false_source_anchor_count"
    )
    _safe_nonnegative_count(
        value.get("boilerplate_question_count"), field="boilerplate_question_count"
    )
    disqualified_hits = set(surfaced_item_ids) & set(unsupported_claim_ids)
    if disqualified_hits:
        raise EvaluationError("a surfaced item cannot also be unsupported")
    question_count = value.get("question_count")
    if (
        not isinstance(question_count, int)
        or isinstance(question_count, bool)
        or not 0 <= question_count <= MAX_QUESTION_COUNT
    ):
        raise EvaluationError("question_count must be an integer from 0 through 5")
    materially_misleading = value.get("materially_misleading")
    if not isinstance(materially_misleading, bool):
        raise EvaluationError("materially_misleading must be boolean")
    usefulness = value.get("usefulness")
    if usefulness not in {"useful", "not_useful"}:
        raise EvaluationError("usefulness must be useful or not_useful")
    if _safe_model_id(value.get("model_reported_id")) is None:
        raise EvaluationError("recorded owner-local input requires model_reported_id")
    return dict(value)


def load_owner_inputs(
    directory: Path,
    verification: ManifestVerification,
) -> dict[str, dict[str, object]]:
    """Read one metadata-only JSON judgment per artifact from an owner-local directory."""

    if not directory.is_dir():
        raise OwnerInputRequired("owner-local response inputs are required")
    artifact_ids = [artifact.artifact_id for artifact in verification.artifacts]
    expected = set(artifact_ids)
    loaded: dict[str, dict[str, object]] = {}
    for artifact_id in artifact_ids:
        path = directory / f"{artifact_id}.json"
        if not path.is_file() or path.is_symlink():
            raise OwnerInputRequired("owner-local response inputs are incomplete")
        try:
            value = json.loads(path.read_text(encoding="utf-8"))
        except (OSError, UnicodeDecodeError, json.JSONDecodeError) as error:
            raise EvaluationError("owner-local response input is not valid JSON") from error
        loaded[artifact_id] = _load_owner_input(
            value,
            artifact_id=artifact_id,
            allowed_item_ids=verification.item_ids_for(artifact_id),
        )
    extras = {
        path.stem
        for path in directory.glob("*.json")
        if path.is_file() and not path.is_symlink()
    } - expected
    if extras:
        raise EvaluationError("owner-local response directory contains an unknown artifact")
    return loaded


def _record_from_owner_input(
    verification: ManifestVerification,
    artifact: ArtifactMetadata,
    owner_input: Mapping[str, object],
) -> dict[str, object]:
    value = _load_owner_input(
        dict(owner_input),
        artifact_id=artifact.artifact_id,
        allowed_item_ids=verification.item_ids_for(artifact.artifact_id),
    )
    return {
        "artifact_id": artifact.artifact_id,
        "corpus_version": verification.corpus_version,
        "manifest_sha256": verification.manifest_sha256,
        "artifact_sha256": artifact.sha256,
        "artifact_byte_count": artifact.byte_count,
        "status": "recorded",
        "decoded_completely": value["decoded_completely"],
        "surfaced_item_ids": value["surfaced_item_ids"],
        "unsupported_claim_ids": value["unsupported_claim_ids"],
        "unsupported_claim_count": value["unsupported_claim_count"],
        "false_source_anchor_count": value["false_source_anchor_count"],
        "boilerplate_question_count": value["boilerplate_question_count"],
        "question_count": value["question_count"],
        "materially_misleading": value["materially_misleading"],
        "usefulness": value["usefulness"],
        "model_reported_id": value["model_reported_id"],
    }


def evidence_from_owner_inputs(
    verification: ManifestVerification,
    owner_inputs: Mapping[str, Mapping[str, object]],
    *,
    created_at: str | None = None,
) -> dict[str, object]:
    """Build complete evidence from owner-local metadata, never model content."""

    verification = verify_manifest(verification.root)
    expected = {artifact.artifact_id for artifact in verification.artifacts}
    if set(owner_inputs) != expected:
        raise OwnerInputRequired("owner-local response inputs are incomplete")
    results = [
        _record_from_owner_input(verification, artifact, owner_inputs[artifact.artifact_id])
        for artifact in verification.artifacts
    ]
    useful_count = sum(result["usefulness"] == "useful" for result in results)
    decoded_count = sum(result["decoded_completely"] is True for result in results)
    misleading_count = sum(result["materially_misleading"] is True for result in results)
    max_questions = max(int(result["question_count"]) for result in results)
    surfaced = sorted({item for result in results for item in result["surfaced_item_ids"]})
    false_source_anchor_count = sum(int(result["false_source_anchor_count"]) for result in results)
    eligible_for_threshold = (
        decoded_count == EXPECTED_ARTIFACT_COUNT
        and useful_count >= MIN_USEFUL_COUNT
        and len(surfaced) >= MIN_SURFACED_ITEM_COUNT
        and false_source_anchor_count <= MAX_FALSE_SOURCE_ANCHOR_COUNT
        and misleading_count <= MAX_MATERIALLY_MISLEADING_COUNT
        and max_questions <= MAX_QUESTION_COUNT
    )
    evidence = {
        "schema": EVIDENCE_SCHEMA,
        "created_at": _created_at(created_at),
        "corpus": {
            "version": verification.corpus_version,
            "manifest_sha256": verification.manifest_sha256,
            "artifact_count": len(verification.artifacts),
        },
        "configuration": _base_configuration(),
        "artifact_metadata": [artifact.evidence() for artifact in verification.artifacts],
        "results": results,
        "evaluation": {
            "status": "recorded",
            "eligible_for_threshold": eligible_for_threshold,
            "decoded_complete_count": decoded_count,
            "useful_count": useful_count,
            "surfaced_item_count": len(surfaced),
            "materially_misleading_count": misleading_count,
            "false_source_anchor_count": false_source_anchor_count,
            "max_question_count": max_questions,
            "surfaced_item_ids": surfaced,
            "contract_reference": "PRODUCT.md#review-evaluation-contract",
        },
    }
    validate_evidence(evidence, verification=verification)
    return evidence


def _validate_file_metadata(value: object) -> None:
    if not isinstance(value, dict) or set(value) != {"path", "byte_count", "sha256"}:
        raise EvaluationError("invalid artifact file metadata")
    _safe_relative_path(value["path"], field="artifact file path")
    if not isinstance(value["byte_count"], int) or isinstance(value["byte_count"], bool) or value["byte_count"] < 0:
        raise EvaluationError("invalid artifact byte count")
    if not isinstance(value["sha256"], str) or SHA256_PATTERN.fullmatch(value["sha256"]) is None:
        raise EvaluationError("invalid artifact file digest")


def _validate_artifact_metadata(value: object) -> None:
    required = {"artifact_id", "lens", "path", "file_count", "byte_count", "sha256", "files"}
    if not isinstance(value, dict) or set(value) != required:
        raise EvaluationError("invalid artifact metadata")
    _safe_artifact_id(value["artifact_id"])
    if not isinstance(value["lens"], str) or not value["lens"]:
        raise EvaluationError("invalid artifact lens")
    _safe_relative_path(value["path"], field="artifact path")
    if not isinstance(value["file_count"], int) or isinstance(value["file_count"], bool) or value["file_count"] <= 0:
        raise EvaluationError("invalid artifact file count")
    if not isinstance(value["byte_count"], int) or isinstance(value["byte_count"], bool) or value["byte_count"] < 0:
        raise EvaluationError("invalid artifact byte count")
    if not isinstance(value["sha256"], str) or SHA256_PATTERN.fullmatch(value["sha256"]) is None:
        raise EvaluationError("invalid artifact digest")
    files = value["files"]
    if not isinstance(files, list) or len(files) != value["file_count"]:
        raise EvaluationError("invalid artifact file metadata count")
    paths: list[str] = []
    for item in files:
        _validate_file_metadata(item)
        paths.append(item["path"])
    if paths != sorted(paths) or len(paths) != len(set(paths)):
        raise EvaluationError("artifact file metadata must be sorted and duplicate-free")


def _validate_result(
    value: object,
    *,
    complete: bool,
    artifact: ArtifactMetadata,
    verification: ManifestVerification,
) -> None:
    required = {
        "artifact_id",
        "corpus_version",
        "manifest_sha256",
        "artifact_sha256",
        "artifact_byte_count",
        "status",
        "decoded_completely",
        "surfaced_item_ids",
        "unsupported_claim_ids",
        "unsupported_claim_count",
        "false_source_anchor_count",
        "boilerplate_question_count",
        "question_count",
        "materially_misleading",
        "usefulness",
        "model_reported_id",
    }
    if not isinstance(value, dict) or set(value) != required:
        raise EvaluationError("invalid evaluation result schema")
    if value["artifact_id"] != artifact.artifact_id:
        raise EvaluationError("evaluation result artifact does not match the verified corpus")
    if value["corpus_version"] != verification.corpus_version:
        raise EvaluationError("evaluation result corpus version is invalid")
    if value["manifest_sha256"] != verification.manifest_sha256:
        raise EvaluationError("evaluation result manifest digest does not match the verified manifest")
    if value["artifact_sha256"] != artifact.sha256:
        raise EvaluationError("evaluation result digest does not match the verified artifact")
    if value["artifact_byte_count"] != artifact.byte_count:
        raise EvaluationError("evaluation result byte count does not match the verified artifact")
    expected_status = "recorded" if complete else "awaiting_owner_input"
    if value["status"] != expected_status:
        raise EvaluationError("evaluation result status is invalid")
    if complete:
        if not isinstance(value["decoded_completely"], bool):
            raise EvaluationError("decoded_completely is invalid")
        if value["question_count"] is None:
            raise EvaluationError("complete result must record question_count")
        if value["materially_misleading"] is None or value["usefulness"] is None:
            raise EvaluationError("complete result must record owner judgments")
    elif any(
        value[field] not in (None, [])
        for field in (
            "decoded_completely",
            "unsupported_claim_count",
            "false_source_anchor_count",
            "boilerplate_question_count",
            "question_count",
            "materially_misleading",
            "usefulness",
        )
    ):
        raise EvaluationError("scaffold result must not invent owner judgments")
    for field in (
        "surfaced_item_ids",
        "unsupported_claim_ids",
    ):
        _safe_item_ids(
            value[field],
            field=field,
            allowed_item_ids=verification.item_ids_for(artifact.artifact_id),
        )
    if complete:
        _load_owner_input(
            {
                "artifact_id": value["artifact_id"],
                "decoded_completely": value["decoded_completely"],
                "surfaced_item_ids": value["surfaced_item_ids"],
                "unsupported_claim_ids": value["unsupported_claim_ids"],
                "unsupported_claim_count": value["unsupported_claim_count"],
                "false_source_anchor_count": value["false_source_anchor_count"],
                "boilerplate_question_count": value["boilerplate_question_count"],
                "question_count": value["question_count"],
                "materially_misleading": value["materially_misleading"],
                "usefulness": value["usefulness"],
                "model_reported_id": value["model_reported_id"],
            },
            artifact_id=value["artifact_id"],
            allowed_item_ids=verification.item_ids_for(artifact.artifact_id),
        )
    else:
        _safe_model_id(value["model_reported_id"])


def validate_evidence(
    value: object,
    *,
    verification: ManifestVerification | None = None,
) -> None:
    """Validate content-free evidence against a fresh, verified corpus manifest."""

    verification = verify_manifest() if verification is None else verify_manifest(verification.root)
    if not isinstance(value, dict):
        raise EvaluationError("evidence root must be an object")
    required = {"schema", "created_at", "corpus", "configuration", "artifact_metadata", "results", "evaluation"}
    if set(value) != required or value["schema"] != EVIDENCE_SCHEMA:
        raise EvaluationError("invalid Goal 06 evidence schema")
    _safe_timestamp(value["created_at"])
    expected_corpus = {
        "version": verification.corpus_version,
        "manifest_sha256": verification.manifest_sha256,
        "artifact_count": EXPECTED_ARTIFACT_COUNT,
    }
    if value["corpus"] != expected_corpus:
        raise EvaluationError("evidence corpus identity does not match the verified manifest")
    if value["configuration"] != _base_configuration():
        raise EvaluationError("evidence configuration does not match the reference configuration")
    artifacts = value["artifact_metadata"]
    expected_artifacts = [artifact.evidence() for artifact in verification.artifacts]
    if artifacts != expected_artifacts:
        raise EvaluationError("evidence artifact metadata does not match the verified manifest")
    results = value["results"]
    if not isinstance(results, list) or len(results) != EXPECTED_ARTIFACT_COUNT:
        raise EvaluationError("evidence results must contain 12 artifacts")
    evaluation = value["evaluation"]
    if not isinstance(evaluation, dict):
        raise EvaluationError("invalid evidence evaluation status")
    complete = evaluation.get("status") == "recorded"
    if complete:
        required_evaluation = {
            "status",
            "eligible_for_threshold",
            "decoded_complete_count",
            "useful_count",
            "surfaced_item_count",
            "materially_misleading_count",
            "false_source_anchor_count",
            "max_question_count",
            "surfaced_item_ids",
            "contract_reference",
        }
        if set(evaluation) != required_evaluation:
            raise EvaluationError("recorded evidence evaluation schema is invalid")
    else:
        if set(evaluation) != {"status", "eligible_for_threshold", "reason"} or evaluation["status"] != "not_evaluated":
            raise EvaluationError("fail-closed evidence evaluation schema is invalid")
        if evaluation["eligible_for_threshold"] is not False or not isinstance(evaluation["reason"], str):
            raise EvaluationError("fail-closed evidence must explain why it is not evaluated")

    for result, artifact in zip(results, verification.artifacts, strict=True):
        _validate_result(result, complete=complete, artifact=artifact, verification=verification)

    if not complete:
        return
    for field in (
        "decoded_complete_count",
        "useful_count",
        "surfaced_item_count",
        "materially_misleading_count",
        "false_source_anchor_count",
        "max_question_count",
    ):
        if not isinstance(evaluation[field], int) or isinstance(evaluation[field], bool) or evaluation[field] < 0:
            raise EvaluationError("recorded evidence summary is invalid")
    if not isinstance(evaluation["surfaced_item_ids"], list):
        raise EvaluationError("recorded evidence surfaced item summary is invalid")
    all_item_ids = frozenset(
        item_id
        for artifact in verification.artifacts
        for item_id in verification.item_ids_for(artifact.artifact_id)
    )
    _safe_item_ids(
        evaluation["surfaced_item_ids"],
        field="evaluation surfaced_item_ids",
        allowed_item_ids=all_item_ids,
    )
    if evaluation["contract_reference"] != "PRODUCT.md#review-evaluation-contract":
        raise EvaluationError("recorded evidence contract reference is invalid")
    expected_decoded = sum(result["decoded_completely"] is True for result in results)
    expected_useful = sum(result["usefulness"] == "useful" for result in results)
    expected_surfaced = sorted({item for result in results for item in result["surfaced_item_ids"]})
    expected_misleading = sum(result["materially_misleading"] is True for result in results)
    expected_false_source_anchors = sum(int(result["false_source_anchor_count"]) for result in results)
    expected_max_questions = max(int(result["question_count"]) for result in results)
    expected_summary = {
        "decoded_complete_count": expected_decoded,
        "useful_count": expected_useful,
        "surfaced_item_count": len(expected_surfaced),
        "materially_misleading_count": expected_misleading,
        "false_source_anchor_count": expected_false_source_anchors,
        "max_question_count": expected_max_questions,
    }
    for field, expected_value in expected_summary.items():
        if evaluation[field] != expected_value:
            raise EvaluationError("recorded evidence summary does not match results")
    if evaluation["surfaced_item_ids"] != expected_surfaced:
        raise EvaluationError("recorded evidence surfaced ids do not match results")
    expected_eligible = (
        expected_decoded == EXPECTED_ARTIFACT_COUNT
        and expected_useful >= MIN_USEFUL_COUNT
        and len(expected_surfaced) >= MIN_SURFACED_ITEM_COUNT
        and expected_false_source_anchors <= MAX_FALSE_SOURCE_ANCHOR_COUNT
        and expected_misleading <= MAX_MATERIALLY_MISLEADING_COUNT
        and expected_max_questions <= MAX_QUESTION_COUNT
    )
    if evaluation["eligible_for_threshold"] is not expected_eligible:
        raise EvaluationError("recorded evidence eligibility does not match the fixed contract thresholds")


def write_evidence(
    path: Path,
    evidence: Mapping[str, object],
    *,
    root: Path = REPO,
) -> None:
    """Write validated JSON without copying owner-local content into it."""

    root = root.resolve()
    path = path.resolve()
    corpus_root = (root / CORPUS_RELATIVE).resolve()
    if _is_within(path, corpus_root):
        raise EvaluationError("evidence path must not be inside the immutable evaluation corpus")
    validate_evidence(dict(evidence), verification=verify_manifest(root))
    try:
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(json.dumps(evidence, indent=2, sort_keys=True) + "\n", encoding="utf-8")
    except OSError as error:
        raise EvaluationError("could not write evidence") from error


def _print_json(value: Mapping[str, object]) -> None:
    print(json.dumps(value, indent=2, sort_keys=True))


def parser() -> argparse.ArgumentParser:
    result = argparse.ArgumentParser(
        prog="evaluation",
        description="Verify Goal 06's offline corpus and create content-free evaluation evidence.",
    )
    subcommands = result.add_subparsers(dest="command", required=True)

    verify = subcommands.add_parser(
        "verify-manifest",
        help="Verify evaluation/goal-01/MANIFEST.sha256 and print artifact metadata.",
    )
    verify.add_argument("--root", type=Path, default=REPO, help=argparse.SUPPRESS)
    verify.add_argument("--json", action="store_true", help="print machine-readable metadata")

    scaffold = subcommands.add_parser(
        "scaffold",
        aliases=["init"],
        help="Create a fail-closed 12-artifact owner-input scaffold.",
    )
    scaffold.add_argument("--root", type=Path, default=REPO, help=argparse.SUPPRESS)
    scaffold.add_argument("--evidence", type=Path, help="write JSON evidence to this path")
    scaffold.add_argument("--created-at", help=argparse.SUPPRESS)

    record = subcommands.add_parser(
        "record",
        aliases=["run"],
        help="Record owner-local metadata judgments; never contacts a model endpoint.",
    )
    record.add_argument("--root", type=Path, default=REPO, help=argparse.SUPPRESS)
    record.add_argument(
        "--owner-input-dir",
        "--response-dir",
        dest="owner_input_dir",
        type=Path,
        help="directory containing one metadata-only JSON file per artifact",
    )
    record.add_argument("--evidence", type=Path, help="write JSON evidence to this path")
    record.add_argument("--created-at", help=argparse.SUPPRESS)
    return result


def main(argv: Sequence[str] | None = None) -> int:
    namespace = parser().parse_args(argv)
    try:
        verification = verify_manifest(namespace.root)
        if namespace.command == "verify-manifest":
            _print_json(verification.evidence())
            return 0
        if namespace.command in {"scaffold", "init"}:
            evidence = scaffold_evidence(verification, created_at=namespace.created_at)
            if namespace.evidence is None:
                _print_json(evidence)
            else:
                write_evidence(namespace.evidence, evidence, root=namespace.root)
            return 0

        if namespace.owner_input_dir is None:
            evidence = scaffold_evidence(verification, created_at=namespace.created_at)
            if namespace.evidence is None:
                _print_json(evidence)
            else:
                write_evidence(namespace.evidence, evidence, root=namespace.root)
            print("error: owner-local response inputs are required; no endpoint was contacted", file=sys.stderr)
            return 2
        try:
            owner_inputs = load_owner_inputs(
                namespace.owner_input_dir,
                verification,
            )
            evidence = evidence_from_owner_inputs(verification, owner_inputs, created_at=namespace.created_at)
        except OwnerInputRequired as error:
            evidence = scaffold_evidence(
                verification,
                created_at=namespace.created_at,
                reason="owner_local_response_inputs_incomplete",
            )
            if namespace.evidence is None:
                _print_json(evidence)
            else:
                write_evidence(namespace.evidence, evidence, root=namespace.root)
            print(f"error: {error}; no endpoint was contacted", file=sys.stderr)
            return 2
        except EvaluationError as error:
            evidence = scaffold_evidence(
                verification,
                created_at=namespace.created_at,
                reason="owner_local_response_inputs_invalid",
            )
            if namespace.evidence is None:
                _print_json(evidence)
            else:
                write_evidence(namespace.evidence, evidence, root=namespace.root)
            print(f"error: {error}; no endpoint was contacted", file=sys.stderr)
            return 1
        if namespace.evidence is None:
            _print_json(evidence)
        else:
            write_evidence(namespace.evidence, evidence, root=namespace.root)
        return 0
    except EvaluationError as error:
        print(f"error: {error}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
