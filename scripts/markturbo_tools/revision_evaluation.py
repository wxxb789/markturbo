"""Content-free Goal 07 revision evaluation tooling.

The checked-in Goal 01 corpus is immutable. Everything else used by this
module is owner-local input: an eligibility registry, a machine-produced
revision receipt, a native Goal 07 PASS receipt, and owner judgments. The
module records hashes, counts, stable identifiers, and booleans only. It
never copies source text, answers, proposals, rationales, paths, credentials,
or native UI observations into evaluation evidence.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import re
import sys
import weakref
from collections.abc import Mapping, Sequence
from dataclasses import dataclass
from datetime import UTC, datetime
from pathlib import Path
from types import MappingProxyType

from . import evaluation


REPO = evaluation.REPO
CORPUS_RELATIVE = evaluation.CORPUS_RELATIVE
EXPECTED_ARTIFACT_COUNT = evaluation.EXPECTED_ARTIFACT_COUNT
REGISTRY_SCHEMA = "markturbo-goal-07-eligibility-v2"
EVIDENCE_SCHEMA = "markturbo-goal-07-revision-evaluation-v2"
MACHINE_RECEIPT_SCHEMA = "markturbo-goal-07-machine-receipt-v2"
LOCAL_DIFF_RUNNER_SCHEMA = "markturbo-local-diff-receipt-v2"
NATIVE_ACCEPTANCE_SCHEMA = "markturbo.goal-07-native-acceptance"
NATIVE_REQUIRED_CASE_IDS = frozenset(
    {
        "reject_all_byte_identity",
        "selective_apply_one_undo",
        "accept_all_preview",
        "stale_proposal_blocks_apply",
        "safe_save_conflict",
        "trusted_executable_apply_revokes_trust",
    }
)
REVISION_CONTRACT = (
    "docs/goals/07-apply-only-approved-revisions.md#revision-evaluation-standard"
)
SHA256_PATTERN = re.compile(r"[0-9a-f]{64}")
STABLE_ID_PATTERN = re.compile(r"[A-Za-z0-9][A-Za-z0-9._:-]{0,127}")
REASON_PATTERN = re.compile(r"[a-z0-9][a-z0-9._:-]{0,127}")
TIMESTAMP_PATTERN = re.compile(
    r"[0-9]{4}-[0-9]{2}-[0-9]{2}T[0-9]{2}:"
    r"[0-9]{2}:[0-9]{2}(?:\.[0-9]+)?(?:Z|[+-][0-9]{2}:[0-9]{2})"
)
EMPTY_SHA256 = hashlib.sha256(b"").hexdigest()
MAX_EXTERNAL_JSON_BYTES = 4 * 1024 * 1024
MAX_JSON_INTEGER_DIGITS = 4096
MAX_METADATA_NESTING_DEPTH = 256
MAX_REVISION_OUTPUT_BYTES = 4 * 1024 * 1024
MAX_REVISION_REPLACEMENT_BYTES = 4 * 1024 * 1024
MAX_REVISION_RATIONALE_BYTES = 16 * 1024
MAX_REVISION_CHANGES = 128
MAX_REVISION_HUNKS = 512
_MACHINE_RECEIPT_TRUST_TOKEN = object()
_NATIVE_ACCEPTANCE_TRUST_TOKEN = object()

# Re-export the verifier so callers cannot accidentally use a less strict
# manifest check for a Goal 07 receipt.
verify_manifest = evaluation.verify_manifest


class RevisionEvaluationError(ValueError):
    """A corpus, receipt, registry, or owner judgment is invalid."""


class OwnerInputRequired(RevisionEvaluationError):
    """The record command is missing owner-local input."""


@dataclass(frozen=True)
class EligibilityCase:
    artifact_id: str
    eligible: bool
    material_question_ids: tuple[str, ...]
    intent_violating_change_ids: tuple[str, ...]

    def evidence(self) -> dict[str, object]:
        return {
            "artifact_id": self.artifact_id,
            "eligible": self.eligible,
            "material_question_ids": list(self.material_question_ids),
            "intent_violating_change_ids": list(self.intent_violating_change_ids),
        }


@dataclass(frozen=True)
class EligibilityRegistry:
    schema: str
    corpus_version: str
    manifest_sha256: str
    owner_annotations_sha256: str
    registry_sha256: str
    cases: tuple[EligibilityCase, ...]

    def by_artifact(self) -> dict[str, EligibilityCase]:
        return {case.artifact_id: case for case in self.cases}

    def evidence(self) -> dict[str, object]:
        return {
            "schema": self.schema,
            "corpus_version": self.corpus_version,
            "manifest_sha256": self.manifest_sha256,
            "owner_annotations_sha256": self.owner_annotations_sha256,
            "registry_sha256": self.registry_sha256,
            "owner_approval_required": True,
            "cases": [case.evidence() for case in self.cases],
        }


@dataclass(frozen=True)
class NativeAcceptance:
    raw_sha256: str
    executable_sha256: str
    status: str = "PASS"
    schema: str = NATIVE_ACCEPTANCE_SCHEMA
    trust_token: object | None = None

    def evidence(self) -> dict[str, object]:
        return {
            "schema": self.schema,
            "status": self.status,
            "raw_sha256": self.raw_sha256,
            "executable_sha256": self.executable_sha256,
        }


# Names used by early Goal 07 design notes remain harmless aliases; the
# serialized contract is still the v2 `native_acceptance` object above.
NativePassEvidence = NativeAcceptance
NativeAcceptanceEvidence = NativeAcceptance


@dataclass(frozen=True)
class _MachineCase:
    artifact_id: str
    corpus_artifact_sha256: str
    corpus_artifact_byte_count: int
    request_artifact_sha256: str
    request_artifact_byte_count: int
    corpus_artifact_lens: str
    review_scope_sha256: str
    review_scope_byte_count: int
    editable_source_sha256: str
    editable_source_byte_count: int
    source_binding_sha256: str
    changes: tuple[Mapping[str, object], ...]
    answered_material_question_ids: tuple[str, ...]
    represented_question_ids: tuple[str, ...]
    intentionally_omitted_question_ids: tuple[str, ...]
    reject_all: Mapping[str, object]
    extra_facts: Mapping[str, object]


def _freeze(value: object) -> object:
    if isinstance(value, Mapping):
        return MappingProxyType({key: _freeze(nested) for key, nested in value.items()})
    if isinstance(value, (list, tuple)):
        return tuple(_freeze(item) for item in value)
    return value


def _copy(value: object) -> object:
    if isinstance(value, Mapping):
        return {key: _copy(nested) for key, nested in value.items()}
    if isinstance(value, tuple):
        return [_copy(item) for item in value]
    return value


@dataclass(frozen=True)
class MachineReceipt:
    schema: str
    corpus_version: str
    manifest_sha256: str
    runner_schema: str
    runner_executable_sha256: str
    receipt_sha256: str
    normalized_sha256: str
    cases: tuple[tuple[str, Mapping[str, object]], ...]
    trust_token: object | None = None

    def __post_init__(self) -> None:
        object.__setattr__(
            self,
            "cases",
            tuple((artifact_id, _freeze(case)) for artifact_id, case in self.cases),
        )

    def by_artifact(self) -> dict[str, dict[str, object]]:
        return {
            artifact_id: _copy(case)  # type: ignore[return-value]
            for artifact_id, case in self.cases
        }

    def evidence(self) -> dict[str, object]:
        return {
            "schema": self.schema,
            "corpus_version": self.corpus_version,
            "manifest_sha256": self.manifest_sha256,
            "runner_schema": self.runner_schema,
            "runner_executable_sha256": self.runner_executable_sha256,
            "receipt_sha256": self.receipt_sha256,
            "normalized_sha256": self.normalized_sha256,
        }


class _MachineReceiptTrust:
    __slots__ = ("_receipt_ref",)

    def __init__(self, receipt: MachineReceipt, *, token: object) -> None:
        if token is not _MACHINE_RECEIPT_TRUST_TOKEN:
            raise TypeError("invalid machine receipt trust token")
        self._receipt_ref = weakref.ref(receipt)

    def belongs_to(self, receipt: MachineReceipt) -> bool:
        return self._receipt_ref() is receipt


class _NativeAcceptanceTrust:
    __slots__ = ("_acceptance_ref",)

    def __init__(self, acceptance: NativeAcceptance, *, token: object) -> None:
        if token is not _NATIVE_ACCEPTANCE_TRUST_TOKEN:
            raise TypeError("invalid native acceptance trust token")
        self._acceptance_ref = weakref.ref(acceptance)

    def belongs_to(self, acceptance: NativeAcceptance) -> bool:
        return self._acceptance_ref() is acceptance


@dataclass(frozen=True)
class _OwnerCase:
    artifact_id: str
    change_decisions: tuple[Mapping[str, object], ...]
    intent_judgment: str
    question_coverage: tuple[Mapping[str, str], ...]
    clearer_due_to_answered_question: bool
    approved_output_sha256: str | None
    approved_proposal_sha256: str | None
    approved_decision_set_sha256: str | None
    approved_decision_file_sha256: str | None


_REGISTRY_KEYS = frozenset(
    {"schema", "corpus_version", "manifest_sha256", "owner_annotations_sha256", "cases"}
)
_REGISTRY_CASE_KEYS = frozenset(
    {"artifact_id", "eligible", "material_question_ids", "intent_violating_change_ids"}
)
_MACHINE_RECEIPT_KEYS = frozenset(
    {"schema", "corpus_version", "manifest_sha256", "runner_schema", "runner_executable_sha256", "cases"}
)
_MACHINE_CASE_KEYS = frozenset(
    {
        "artifact_id",
        "corpus_artifact_sha256",
        "corpus_artifact_byte_count",
        "request_artifact_sha256",
        "request_artifact_byte_count",
        "corpus_artifact_lens",
        "review_scope_sha256",
        "review_scope_byte_count",
        "editable_source_sha256",
        "editable_source_byte_count",
        "source_binding_sha256",
        "changes",
        "answered_material_question_ids",
        "represented_question_ids",
        "intentionally_omitted_question_ids",
        "reject_all",
    }
)
_MACHINE_EXTRA_KEYS = frozenset(
    {
        "review_context_sha256",
        "answers_sha256",
        "raw_revision_response_sha256",
        "proposal_sha256",
        "artifact_lens_sha256",
        "source_revision",
        "source_generation",
        "displayed_diff_sha256",
        "question_coverage",
        "approved_output",
    }
)
_MACHINE_CHANGE_KEYS = frozenset({"change_id", "intent_change_ids", "hunks"})
_MACHINE_HUNK_KEYS = frozenset(
    {
        "hunk_id",
        "source_start",
        "source_end",
        "replacement_sha256",
        "replacement_byte_count",
        "rationale_sha256",
        "rationale_byte_count",
        "displayed_diff_sha256",
        "runner_schema",
        "local_diff_verified",
        "utf8_boundaries_verified",
    }
)
_REJECT_ALL_KEYS = frozenset(
    {"source_sha256", "result_sha256", "source_byte_count", "result_byte_count"}
)
_OWNER_KEYS = frozenset(
    {
        "artifact_id",
        "change_decisions",
        "intent_judgment",
        "question_coverage",
        "clearer_due_to_answered_question",
        "approved_output_sha256",
        "approved_proposal_sha256",
        "approved_decision_set_sha256",
        "approved_decision_file_sha256",
    }
)
_OWNER_DECISION_KEYS = frozenset({"change_id", "decision"})
_QUESTION_COVERAGE_KEYS = frozenset({"question_id", "status"})
_FORBIDDEN_KEYS = frozenset(
    {
        "source",
        "source_text",
        "answer",
        "answers",
        "answer_text",
        "proposal",
        "proposal_text",
        "rationale",
        "absolute_path",
        "path",
        "file_path",
        "api_key",
        "credential",
        "credentials",
        "endpoint_url",
        "request_body",
        "response",
    }
)
_MACHINE_UI_CLAIM_TOKENS = ("dirty", "undo", "trust", "filesystem", "file_path", "endpoint")
_DIFF_RECEIPT_DIGEST_FIELDS = (
    "hunk_id",
    "source_start",
    "source_end",
    "replacement_sha256",
    "replacement_byte_count",
    "rationale_sha256",
    "rationale_byte_count",
    "runner_schema",
    "utf8_boundaries_verified",
)


def _is_within(path: Path, parent: Path) -> bool:
    try:
        path.resolve().relative_to(parent.resolve())
    except ValueError:
        return False
    return True


def _absolute_without_resolving(path: Path) -> Path:
    candidate = Path(path)
    if candidate.is_absolute():
        return candidate
    return Path.cwd() / candidate


def _reject_symlink_components(path: Path, *, field: str) -> Path:
    """Reject direct and parent symlinks before resolving an external path."""

    candidate = _absolute_without_resolving(path)
    for ancestor in candidate.parents:
        try:
            if ancestor.is_symlink():
                raise RevisionEvaluationError(f"{field} must not use a symlink path")
        except OSError as error:
            raise RevisionEvaluationError(f"{field} path is unavailable") from error
    try:
        if candidate.is_symlink():
            raise RevisionEvaluationError(f"{field} must not use a symlink path")
    except OSError as error:
        raise RevisionEvaluationError(f"{field} path is unavailable") from error
    return candidate


def _external_path(path: Path, corpus_root: Path, *, field: str) -> Path:
    candidate = _reject_symlink_components(path, field=field)
    try:
        resolved = candidate.resolve(strict=False)
    except OSError as error:
        raise RevisionEvaluationError(f"{field} path is unavailable") from error
    if _is_within(resolved, corpus_root):
        raise RevisionEvaluationError(f"{field} must remain outside the immutable corpus")
    return candidate


def _mapping(value: object, *, field: str) -> Mapping[str, object]:
    if not isinstance(value, Mapping) or not all(isinstance(key, str) for key in value):
        raise RevisionEvaluationError(f"{field} must be an object with string keys")
    return value


def _exact_keys(value: Mapping[str, object], expected: frozenset[str], *, field: str) -> None:
    if set(value) != expected:
        raise RevisionEvaluationError(f"{field} has an invalid schema")


def _safe_sha256(value: object, *, field: str) -> str:
    if not isinstance(value, str) or SHA256_PATTERN.fullmatch(value) is None:
        raise RevisionEvaluationError(f"{field} must be a lowercase SHA-256 digest")
    return value


def _safe_nonempty_sha256(value: object, *, field: str) -> str:
    digest = _safe_sha256(value, field=field)
    if digest == "0" * 64:
        raise RevisionEvaluationError(f"{field} must not be an all-zero digest")
    return digest


def _safe_count(value: object, *, field: str) -> int:
    if not isinstance(value, int) or isinstance(value, bool) or value < 0:
        raise RevisionEvaluationError(f"{field} must be a non-negative integer")
    return value


def _safe_id(value: object, *, field: str) -> str:
    if not isinstance(value, str) or STABLE_ID_PATTERN.fullmatch(value) is None:
        raise RevisionEvaluationError(f"{field} must be a normalized stable identifier")
    return value


def _natural_id_key(value: str) -> tuple[tuple[int, object], ...]:
    return tuple(
        (1, int(part)) if part.isdigit() else (0, part)
        for part in re.split(r"([0-9]+)", value)
        if part
    )


def _owner_change_id(value: object, *, artifact_id: str, expected: set[str]) -> str:
    if isinstance(value, int) and not isinstance(value, bool) and value >= 0:
        candidate = f"{artifact_id}-CH-{value + 1:02d}"
        if candidate in expected:
            return candidate
    return _safe_id(value, field="owner change_id")


def _safe_id_list(value: object, *, field: str) -> tuple[str, ...]:
    if not isinstance(value, list):
        raise RevisionEvaluationError(f"{field} must be a list")
    result = tuple(_safe_id(item, field=f"{field} item") for item in value)
    if len(result) != len(set(result)) or list(result) != sorted(result):
        raise RevisionEvaluationError(f"{field} must be sorted and duplicate-free")
    return result


def _safe_bool(value: object, *, field: str) -> bool:
    if not isinstance(value, bool):
        raise RevisionEvaluationError(f"{field} must be a boolean")
    return value


def _owner_output_binding(
    value: Mapping[str, object],
    machine_case: _MachineCase,
    *,
    field: str,
) -> tuple[str | None, str | None, str | None, str | None]:
    approved = machine_case.extra_facts.get("approved_output")
    if not isinstance(approved, Mapping):
        expected = {
            "approved_output_sha256": None,
            "approved_proposal_sha256": None,
            "approved_decision_set_sha256": None,
            "approved_decision_file_sha256": None,
        }
    elif approved.get("status") == "composed":
        expected = {
            "approved_output_sha256": approved["result_sha256"],
            "approved_proposal_sha256": approved["proposal_sha256"],
            "approved_decision_set_sha256": approved["decision_set_sha256"],
            "approved_decision_file_sha256": approved["decision_file_sha256"],
        }
    else:
        expected = {
            "approved_output_sha256": None,
            "approved_proposal_sha256": None,
            "approved_decision_set_sha256": None,
            "approved_decision_file_sha256": None,
        }
    normalized: list[str | None] = []
    for name in (
        "approved_output_sha256",
        "approved_proposal_sha256",
        "approved_decision_set_sha256",
        "approved_decision_file_sha256",
    ):
        actual = value[name]
        expected_value = expected[name]
        if expected_value is None:
            if actual is not None:
                raise RevisionEvaluationError(f"{field}.{name} does not match the approved output")
            normalized.append(None)
            continue
        normalized_value = _safe_nonempty_sha256(actual, field=f"{field}.{name}")
        if normalized_value != expected_value:
            raise RevisionEvaluationError(f"{field}.{name} does not match the approved output")
        normalized.append(normalized_value)
    return normalized[0], normalized[1], normalized[2], normalized[3]


def _safe_timestamp(value: object) -> str:
    if not isinstance(value, str) or TIMESTAMP_PATTERN.fullmatch(value) is None:
        raise RevisionEvaluationError("created_at must be an RFC 3339 timestamp")
    try:
        parsed = datetime.fromisoformat(value.replace("Z", "+00:00"))
    except ValueError as error:
        raise RevisionEvaluationError("created_at must be an RFC 3339 timestamp") from error
    if parsed.tzinfo is None:
        raise RevisionEvaluationError("created_at must be an RFC 3339 timestamp")
    return value


def _created_at(value: str | None) -> str:
    if value is None:
        return datetime.now(UTC).replace(microsecond=0).isoformat().replace("+00:00", "Z")
    return _safe_timestamp(value)


def _safe_reason(value: object) -> str:
    if not isinstance(value, str) or REASON_PATTERN.fullmatch(value) is None:
        raise RevisionEvaluationError("evaluation reason must be a stable metadata token")
    return value


def _reject_forbidden_content(value: object, *, field: str) -> None:
    """Reject text/path/credential-shaped content before normalization."""

    def visit(item: object, location: str, depth: int) -> None:
        if depth > MAX_METADATA_NESTING_DEPTH:
            raise RevisionEvaluationError(f"{field} exceeds the metadata nesting limit")
        if isinstance(item, Mapping):
            for key, nested in item.items():
                key_text = str(key)
                lowered = key_text.lower()
                if lowered in _FORBIDDEN_KEYS or (lowered.endswith("_text") and isinstance(nested, str)):
                    raise RevisionEvaluationError(
                        f"{field} must be metadata-only; forbidden field {key_text!r}"
                    )
                visit(nested, f"{location}.{key_text}", depth + 1)
        elif isinstance(item, list):
            for index, nested in enumerate(item):
                visit(nested, f"{location}[{index}]", depth + 1)
        elif isinstance(item, str):
            lowered = item.lower()
            if (
                "://" in item
                or item.startswith(("/", "\\"))
                or re.fullmatch(r"[a-z]:[\\/].*", item, re.I) is not None
                or lowered.startswith(
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
                or item.startswith(("AIza", "AKIA"))
            ):
                raise RevisionEvaluationError(f"{field} must not contain paths, URLs, or credentials")

    try:
        visit(value, field, 0)
    except RecursionError as error:
        raise RevisionEvaluationError(f"{field} exceeds the metadata nesting limit") from error


def _reject_machine_ui_claims(value: object, *, field: str) -> None:
    def visit(item: object) -> None:
        if isinstance(item, Mapping):
            for key, nested in item.items():
                lowered = str(key).lower()
                if any(token in lowered for token in _MACHINE_UI_CLAIM_TOKENS):
                    raise RevisionEvaluationError(f"{field} contains a forbidden machine UI claim")
                visit(nested)
        elif isinstance(item, list):
            for nested in item:
                visit(nested)

    visit(value)


def _owner_annotations_sha256(verification: evaluation.ManifestVerification) -> str:
    target = f"{CORPUS_RELATIVE.as_posix()}/OWNER-ANNOTATIONS.md"
    for entry in verification.entries:
        if entry.path == target:
            return entry.sha256
    raise RevisionEvaluationError("fresh manifest does not cover OWNER-ANNOTATIONS.md")


def canonical_registry_digest(payload: Mapping[str, object]) -> str:
    """Return the canonical digest used as the external registry anchor."""

    return hashlib.sha256(
        json.dumps(dict(payload), sort_keys=True, separators=(",", ":"), ensure_ascii=True).encode("utf-8")
    ).hexdigest()


def diff_receipt_digest(receipt: Mapping[str, object]) -> str:
    """Hash only stable, content-free fields of one local diff hunk."""

    try:
        normalized = {key: receipt[key] for key in _DIFF_RECEIPT_DIGEST_FIELDS}
    except KeyError as error:
        raise RevisionEvaluationError("local diff hunk receipt is incomplete") from error
    return hashlib.sha256(
        json.dumps(normalized, sort_keys=True, separators=(",", ":"), ensure_ascii=True).encode("utf-8")
    ).hexdigest()


def _json_load(path: Path) -> tuple[object, str]:
    try:
        raw = path.read_bytes()
    except OSError as error:
        raise RevisionEvaluationError(f"could not read {path.name}") from error
    if len(raw) > MAX_EXTERNAL_JSON_BYTES:
        raise RevisionEvaluationError(f"{path.name} exceeds the JSON input bound")

    def reject_duplicate_keys(pairs: list[tuple[str, object]]) -> dict[str, object]:
        value: dict[str, object] = {}
        for key, nested in pairs:
            if key in value:
                raise RevisionEvaluationError(f"{path.name} contains duplicate JSON keys")
            value[key] = nested
        return value

    def reject_constant(value: str) -> object:
        raise RevisionEvaluationError(f"{path.name} contains a non-standard JSON number")

    def parse_int(value: str) -> int:
        digits = value[1:] if value.startswith("-") else value
        if len(digits) > MAX_JSON_INTEGER_DIGITS:
            raise ValueError("JSON integer exceeds the input bound")
        return int(value)

    try:
        value = json.loads(
            raw.decode("utf-8"),
            object_pairs_hook=reject_duplicate_keys,
            parse_constant=reject_constant,
            parse_int=parse_int,
        )
    except RevisionEvaluationError:
        raise
    except (UnicodeDecodeError, json.JSONDecodeError, ValueError, RecursionError) as error:
        raise RevisionEvaluationError(f"{path.name} is not valid UTF-8 JSON") from error
    return value, hashlib.sha256(raw).hexdigest()


def _fresh_verification(verification: evaluation.ManifestVerification | None) -> evaluation.ManifestVerification:
    root = REPO if verification is None else verification.root
    try:
        return evaluation.verify_manifest(root)
    except evaluation.EvaluationError as error:
        raise RevisionEvaluationError("approved evaluation corpus verification failed") from error


def _expected_artifact_ids(verification: evaluation.ManifestVerification) -> tuple[str, ...]:
    return tuple(artifact.artifact_id for artifact in verification.artifacts)


def _case_mapping(value: object, *, field: str, expected_ids: Sequence[str]) -> dict[str, Mapping[str, object]]:
    mapping = _mapping(value, field=field)
    expected = set(expected_ids)
    if set(mapping) != expected:
        raise RevisionEvaluationError(f"{field} must contain every artifact exactly once")
    return {artifact_id: _mapping(mapping[artifact_id], field=f"{field}.{artifact_id}") for artifact_id in expected_ids}


def _registry_from_payload(
    value: object,
    verification: evaluation.ManifestVerification,
    *,
    approved_registry_sha256: str,
) -> EligibilityRegistry:
    _reject_forbidden_content(value, field="eligibility registry")
    payload = _mapping(value, field="eligibility registry")
    _exact_keys(payload, _REGISTRY_KEYS, field="eligibility registry")
    if payload["schema"] != REGISTRY_SCHEMA:
        raise RevisionEvaluationError("eligibility registry schema is invalid")
    if payload["corpus_version"] != verification.corpus_version:
        raise RevisionEvaluationError("eligibility registry corpus version is invalid")
    if payload["manifest_sha256"] != verification.manifest_sha256:
        raise RevisionEvaluationError("eligibility registry manifest digest is invalid")
    owner_annotations_sha256 = _safe_sha256(
        payload["owner_annotations_sha256"], field="eligibility registry.owner_annotations_sha256"
    )
    if owner_annotations_sha256 != _owner_annotations_sha256(verification):
        raise RevisionEvaluationError("eligibility registry annotation digest is invalid")
    raw_cases = payload["cases"]
    if not isinstance(raw_cases, list):
        raise RevisionEvaluationError("eligibility registry cases must be a list")
    expected_ids = _expected_artifact_ids(verification)
    if len(raw_cases) != len(expected_ids):
        raise RevisionEvaluationError("eligibility registry must cover every corpus artifact")
    cases: list[EligibilityCase] = []
    seen: set[str] = set()
    for raw_case in raw_cases:
        case = _mapping(raw_case, field="eligibility registry case")
        _exact_keys(case, _REGISTRY_CASE_KEYS, field="eligibility registry case")
        artifact_id = _safe_id(case["artifact_id"], field="eligibility registry.artifact_id")
        if artifact_id not in expected_ids or artifact_id in seen:
            raise RevisionEvaluationError("eligibility registry artifact IDs must match the corpus exactly")
        seen.add(artifact_id)
        eligible = _safe_bool(case["eligible"], field=f"eligibility registry.{artifact_id}.eligible")
        material_questions = _safe_id_list(
            case["material_question_ids"], field=f"eligibility registry.{artifact_id}.material_question_ids"
        )
        violating_changes = _safe_id_list(
            case["intent_violating_change_ids"],
            field=f"eligibility registry.{artifact_id}.intent_violating_change_ids",
        )
        cases.append(
            EligibilityCase(
                artifact_id=artifact_id,
                eligible=eligible,
                material_question_ids=material_questions,
                intent_violating_change_ids=violating_changes,
            )
        )
    if set(seen) != set(expected_ids):
        raise RevisionEvaluationError("eligibility registry is missing an artifact")
    ordered = tuple(sorted(cases, key=lambda case: expected_ids.index(case.artifact_id)))
    normalized_payload = {
        "schema": REGISTRY_SCHEMA,
        "corpus_version": verification.corpus_version,
        "manifest_sha256": verification.manifest_sha256,
        "owner_annotations_sha256": owner_annotations_sha256,
        "cases": [case.evidence() for case in ordered],
    }
    approved_registry_sha256 = _safe_nonempty_sha256(approved_registry_sha256, field="approved_registry_sha256")
    registry_sha256 = canonical_registry_digest(normalized_payload)
    if registry_sha256 != approved_registry_sha256:
        raise RevisionEvaluationError("eligibility registry digest does not match its external trust anchor")
    return EligibilityRegistry(
        schema=REGISTRY_SCHEMA,
        corpus_version=verification.corpus_version,
        manifest_sha256=verification.manifest_sha256,
        owner_annotations_sha256=owner_annotations_sha256,
        registry_sha256=registry_sha256,
        cases=ordered,
    )


def load_eligibility_registry(
    path: Path,
    verification: evaluation.ManifestVerification,
    *,
    approved_registry_sha256: str,
) -> EligibilityRegistry:
    """Load owner-approved eligibility from outside the immutable corpus."""

    fresh = _fresh_verification(verification)
    corpus_root = (fresh.root / CORPUS_RELATIVE).resolve()
    path = _external_path(path, corpus_root, field="eligibility registry")
    if not path.is_file():
        raise RevisionEvaluationError("eligibility registry must be an external regular file")
    value, _ = _json_load(path)
    return _registry_from_payload(value, fresh, approved_registry_sha256=approved_registry_sha256)


def _validate_reject_all(value: object, *, field: str) -> dict[str, object]:
    mapping = _mapping(value, field=field)
    _exact_keys(mapping, _REJECT_ALL_KEYS, field=field)
    normalized = {
        "source_sha256": _safe_sha256(mapping["source_sha256"], field=f"{field}.source_sha256"),
        "result_sha256": _safe_sha256(mapping["result_sha256"], field=f"{field}.result_sha256"),
        "source_byte_count": _safe_count(mapping["source_byte_count"], field=f"{field}.source_byte_count"),
        "result_byte_count": _safe_count(mapping["result_byte_count"], field=f"{field}.result_byte_count"),
    }
    if normalized["source_sha256"] != normalized["result_sha256"]:
        raise RevisionEvaluationError("reject-all result must be byte-identical to the editable source")
    if normalized["source_byte_count"] != normalized["result_byte_count"]:
        raise RevisionEvaluationError("reject-all byte count must remain unchanged")
    return normalized


def _validate_machine_hunk(
    value: object,
    *,
    artifact_id: str,
    editable_source_byte_count: int,
) -> dict[str, object]:
    hunk = _mapping(value, field=f"machine case {artifact_id}.hunk")
    allowed_hunk_keys = {_MACHINE_HUNK_KEYS, _MACHINE_HUNK_KEYS | {"is_insertion"}}
    if set(hunk) not in allowed_hunk_keys:
        raise RevisionEvaluationError(f"machine case {artifact_id}.hunk has an invalid schema")
    hunk_id = _safe_id(hunk["hunk_id"], field=f"machine case {artifact_id}.hunk_id")
    start = _safe_count(hunk["source_start"], field=f"machine case {artifact_id}.source_start")
    end = _safe_count(hunk["source_end"], field=f"machine case {artifact_id}.source_end")
    if start > end or end > editable_source_byte_count:
        raise RevisionEvaluationError("machine hunk source range is invalid")
    if "is_insertion" in hunk:
        is_insertion = _safe_bool(hunk["is_insertion"], field=f"machine case {artifact_id}.is_insertion")
        if is_insertion is not (start == end):
            raise RevisionEvaluationError("machine hunk insertion flag does not match its range")
    replacement_sha256 = _safe_sha256(
        hunk["replacement_sha256"], field=f"machine case {artifact_id}.replacement_sha256"
    )
    replacement_count = _safe_count(
        hunk["replacement_byte_count"], field=f"machine case {artifact_id}.replacement_byte_count"
    )
    rationale_sha256 = _safe_sha256(
        hunk["rationale_sha256"], field=f"machine case {artifact_id}.rationale_sha256"
    )
    rationale_count = _safe_count(
        hunk["rationale_byte_count"], field=f"machine case {artifact_id}.rationale_byte_count"
    )
    if replacement_count == 0 and replacement_sha256 != EMPTY_SHA256:
        raise RevisionEvaluationError("zero-byte replacement must use the empty SHA-256 digest")
    if rationale_count == 0:
        raise RevisionEvaluationError("each proposed hunk must retain a non-empty rationale")
    runner_schema = _safe_id(hunk["runner_schema"], field=f"machine case {artifact_id}.runner_schema")
    if runner_schema != LOCAL_DIFF_RUNNER_SCHEMA:
        raise RevisionEvaluationError("machine hunk runner schema is invalid")
    local_diff_verified = _safe_bool(
        hunk["local_diff_verified"], field=f"machine case {artifact_id}.local_diff_verified"
    )
    utf8_boundaries_verified = _safe_bool(
        hunk["utf8_boundaries_verified"], field=f"machine case {artifact_id}.utf8_boundaries_verified"
    )
    normalized = {
        "hunk_id": hunk_id,
        "source_start": start,
        "source_end": end,
        "replacement_sha256": replacement_sha256,
        "replacement_byte_count": replacement_count,
        "rationale_sha256": rationale_sha256,
        "rationale_byte_count": rationale_count,
        "displayed_diff_sha256": _safe_sha256(
            hunk["displayed_diff_sha256"], field=f"machine case {artifact_id}.displayed_diff_sha256"
        ),
        "runner_schema": runner_schema,
        "local_diff_verified": local_diff_verified,
        "utf8_boundaries_verified": utf8_boundaries_verified,
    }
    if normalized["displayed_diff_sha256"] != diff_receipt_digest(normalized):
        raise RevisionEvaluationError("machine hunk displayed diff digest is inconsistent")
    return normalized


def _validate_machine_extra_facts(value: Mapping[str, object], *, field: str) -> dict[str, object]:
    """Validate optional facts emitted by the production Rust evaluator."""

    extras: dict[str, object] = {}
    for name in (
        "review_context_sha256",
        "answers_sha256",
        "raw_revision_response_sha256",
        "proposal_sha256",
        "artifact_lens_sha256",
    ):
        if name in value:
            extras[name] = _safe_nonempty_sha256(value[name], field=f"{field}.{name}")
    for name in ("source_revision", "source_generation"):
        if name in value:
            extras[name] = _safe_count(value[name], field=f"{field}.{name}")
    if "displayed_diff_sha256" in value:
        extras["displayed_diff_sha256"] = _safe_nonempty_sha256(
            value["displayed_diff_sha256"], field=f"{field}.displayed_diff_sha256"
        )
    if "question_coverage" in value:
        raw_coverage = value["question_coverage"]
        if not isinstance(raw_coverage, list):
            raise RevisionEvaluationError(f"{field}.question_coverage must be a list")
        normalized_coverage: list[dict[str, object]] = []
        seen_indexes: set[int] = set()
        for raw_item in raw_coverage:
            item = _mapping(raw_item, field=f"{field}.question_coverage.item")
            _exact_keys(item, frozenset({"question_index", "question_id", "status"}), field=f"{field}.question_coverage.item")
            question_index = _safe_count(item["question_index"], field=f"{field}.question_index")
            if question_index in seen_indexes:
                raise RevisionEvaluationError(f"{field}.question_coverage has duplicate question indexes")
            seen_indexes.add(question_index)
            question_id = _safe_id(item["question_id"], field=f"{field}.question_id")
            status = _mapping(item["status"], field=f"{field}.question_status")
            kind = status.get("kind")
            if kind == "represented":
                _exact_keys(status, frozenset({"kind", "change_ids"}), field=f"{field}.represented_status")
                raw_change_ids = status["change_ids"]
                if not isinstance(raw_change_ids, list):
                    raise RevisionEvaluationError(f"{field}.change_ids must be a list")
                change_ids: list[int] = []
                for raw_change_id in raw_change_ids:
                    if not isinstance(raw_change_id, int) or isinstance(raw_change_id, bool) or raw_change_id < 0:
                        raise RevisionEvaluationError(f"{field}.change_ids contains an invalid value")
                    change_ids.append(raw_change_id)
                if change_ids != sorted(set(change_ids)):
                    raise RevisionEvaluationError(f"{field}.change_ids must be sorted and duplicate-free")
                normalized_status: dict[str, object] = {"kind": kind, "change_ids": change_ids}
            elif kind == "intentionally_omitted":
                _exact_keys(status, frozenset({"kind", "reason_sha256", "reason_byte_count"}), field=f"{field}.omitted_status")
                normalized_status = {
                    "kind": kind,
                    "reason_sha256": _safe_nonempty_sha256(
                        status["reason_sha256"], field=f"{field}.reason_sha256"
                    ),
                    "reason_byte_count": _safe_count(
                        status["reason_byte_count"], field=f"{field}.reason_byte_count"
                    ),
                }
            elif kind == "not_addressed":
                _exact_keys(status, frozenset({"kind"}), field=f"{field}.not_addressed_status")
                normalized_status = {"kind": kind}
            else:
                raise RevisionEvaluationError(f"{field}.question status is invalid")
            normalized_coverage.append(
                {"question_index": question_index, "question_id": question_id, "status": normalized_status}
            )
        extras["question_coverage"] = normalized_coverage
    if "approved_output" in value:
        approved = _mapping(value["approved_output"], field=f"{field}.approved_output")
        _exact_keys(
            approved,
            frozenset(
                {
                    "status",
                    "decision_file_sha256",
                    "decision_set_sha256",
                    "proposal_sha256",
                    "decision_count",
                    "result_sha256",
                    "result_byte_count",
                }
            ),
            field=f"{field}.approved_output",
        )
        status = approved["status"]
        if status == "not_composed":
            if (
                approved["decision_file_sha256"] is not None
                or approved["decision_set_sha256"] is not None
                or approved["proposal_sha256"] is not None
                or approved["result_sha256"] is not None
                or approved["result_byte_count"] is not None
            ):
                raise RevisionEvaluationError(f"{field}.not_composed output must not contain result facts")
            decision_count = _safe_count(approved["decision_count"], field=f"{field}.decision_count")
            if decision_count != 0:
                raise RevisionEvaluationError(f"{field}.not_composed decision count must be zero")
            normalized_approved = {
                "status": status,
                "decision_file_sha256": None,
                "decision_set_sha256": None,
                "proposal_sha256": None,
                "decision_count": 0,
                "result_sha256": None,
                "result_byte_count": None,
            }
        elif status == "composed":
            normalized_approved = {
                "status": status,
                "decision_file_sha256": _safe_nonempty_sha256(
                    approved["decision_file_sha256"], field=f"{field}.decision_file_sha256"
                ),
                "decision_set_sha256": _safe_nonempty_sha256(
                    approved["decision_set_sha256"], field=f"{field}.decision_set_sha256"
                ),
                "proposal_sha256": _safe_nonempty_sha256(
                    approved["proposal_sha256"], field=f"{field}.proposal_sha256"
                ),
                "decision_count": _safe_count(approved["decision_count"], field=f"{field}.decision_count"),
                "result_sha256": _safe_nonempty_sha256(
                    approved["result_sha256"], field=f"{field}.result_sha256"
                ),
                "result_byte_count": _safe_count(
                    approved["result_byte_count"], field=f"{field}.result_byte_count"
                ),
            }
        else:
            raise RevisionEvaluationError(f"{field}.approved_output status is invalid")
        extras["approved_output"] = normalized_approved
    return extras


def _validate_machine_cases(
    value: object,
    verification: evaluation.ManifestVerification,
    registry: EligibilityRegistry | None,
) -> dict[str, _MachineCase]:
    expected_ids = _expected_artifact_ids(verification)
    cases = _case_mapping(value, field="machine cases", expected_ids=expected_ids)
    registry_by_id = {} if registry is None else registry.by_artifact()
    artifact_by_id = {artifact.artifact_id: artifact for artifact in verification.artifacts}
    result: dict[str, _MachineCase] = {}
    for artifact_id in expected_ids:
        case = cases[artifact_id]
        _reject_forbidden_content(case, field=f"machine case {artifact_id}")
        _reject_machine_ui_claims(case, field=f"machine case {artifact_id}")
        case_keys = set(case)
        if not _MACHINE_CASE_KEYS.issubset(case_keys) or case_keys - (_MACHINE_CASE_KEYS | _MACHINE_EXTRA_KEYS):
            raise RevisionEvaluationError(f"machine case {artifact_id} has an invalid schema")
        if case["artifact_id"] != artifact_id:
            raise RevisionEvaluationError("machine case artifact_id does not match its map key")
        corpus_sha = _safe_sha256(
            case["corpus_artifact_sha256"], field=f"machine case {artifact_id}.corpus_artifact_sha256"
        )
        corpus_count = _safe_count(
            case["corpus_artifact_byte_count"], field=f"machine case {artifact_id}.corpus_artifact_byte_count"
        )
        artifact = artifact_by_id[artifact_id]
        if corpus_sha != artifact.sha256 or corpus_count != artifact.byte_count:
            raise RevisionEvaluationError("machine corpus artifact identity does not match the fresh manifest")
        request_sha = _safe_sha256(
            case["request_artifact_sha256"], field=f"machine case {artifact_id}.request_artifact_sha256"
        )
        request_count = _safe_count(
            case["request_artifact_byte_count"], field=f"machine case {artifact_id}.request_artifact_byte_count"
        )
        if request_sha != artifact.sha256 or request_count != artifact.byte_count:
            raise RevisionEvaluationError("machine request artifact identity does not match the fresh manifest")
        corpus_lens = case["corpus_artifact_lens"]
        if not isinstance(corpus_lens, str) or corpus_lens != artifact.lens:
            raise RevisionEvaluationError("machine corpus artifact lens does not match the fresh manifest")
        review_scope_sha = _safe_sha256(
            case["review_scope_sha256"], field=f"machine case {artifact_id}.review_scope_sha256"
        )
        review_scope_count = _safe_count(
            case["review_scope_byte_count"], field=f"machine case {artifact_id}.review_scope_byte_count"
        )
        editable_sha = _safe_sha256(
            case["editable_source_sha256"], field=f"machine case {artifact_id}.editable_source_sha256"
        )
        editable_count = _safe_count(
            case["editable_source_byte_count"], field=f"machine case {artifact_id}.editable_source_byte_count"
        )
        source_binding_sha = _safe_sha256(
            case["source_binding_sha256"], field=f"machine case {artifact_id}.source_binding_sha256"
        )
        if len(artifact.files) == 1:
            sole_file = artifact.files[0]
            if editable_sha != sole_file.sha256 or editable_count != sole_file.byte_count:
                raise RevisionEvaluationError(
                    "machine editable source does not match the fresh single-file corpus artifact"
                )
        elif corpus_lens == "Agent Skill":
            skill_path = f"{artifact.path}/SKILL.md"
            skill_file = next((item for item in artifact.files if item.path == skill_path), None)
            if skill_file is None or editable_sha != skill_file.sha256 or editable_count != skill_file.byte_count:
                raise RevisionEvaluationError(
                    "machine editable source does not match the fresh Agent Skill entrypoint"
                )
        raw_changes = case["changes"]
        if not isinstance(raw_changes, list):
            raise RevisionEvaluationError(f"machine case {artifact_id}.changes must be a list")
        changes: list[Mapping[str, object]] = []
        change_ids: set[str] = set()
        all_hunk_ids: set[str] = set()
        all_ranges: list[tuple[int, int]] = []
        if len(raw_changes) > MAX_REVISION_CHANGES:
            raise RevisionEvaluationError("machine revision contains too many ChangeId groups")
        for raw_change in raw_changes:
            change = _mapping(raw_change, field=f"machine case {artifact_id}.change")
            if set(change) != _MACHINE_CHANGE_KEYS:
                raise RevisionEvaluationError(f"machine case {artifact_id}.change has an invalid schema")
            change_id = _safe_id(change["change_id"], field=f"machine case {artifact_id}.change_id")
            if change_id in change_ids:
                raise RevisionEvaluationError("machine ChangeId values must be unique")
            change_ids.add(change_id)
            intent_ids = _safe_id_list(
                change["intent_change_ids"],
                field=f"machine case {artifact_id}.intent_change_ids",
            )
            raw_hunks = change["hunks"]
            if not isinstance(raw_hunks, list):
                raise RevisionEvaluationError("machine ChangeId hunks must be a list")
            if not raw_hunks:
                raise RevisionEvaluationError("machine ChangeId groups must contain at least one hunk")
            hunks: list[dict[str, object]] = []
            hunk_ids: set[str] = set()
            for raw_hunk in raw_hunks:
                hunk = _validate_machine_hunk(
                    raw_hunk,
                    artifact_id=artifact_id,
                    editable_source_byte_count=editable_count,
                )
                if hunk["hunk_id"] in hunk_ids or hunk["hunk_id"] in all_hunk_ids:
                    raise RevisionEvaluationError("machine hunk identifiers must be unique")
                hunk_ids.add(str(hunk["hunk_id"]))
                all_hunk_ids.add(str(hunk["hunk_id"]))
                hunks.append(hunk)
                all_ranges.append((int(hunk["source_start"]), int(hunk["source_end"])))
            changes.append({"change_id": change_id, "intent_change_ids": list(intent_ids), "hunks": hunks})
        if len(all_ranges) > MAX_REVISION_HUNKS:
            raise RevisionEvaluationError("machine revision contains too many hunks")
        if [str(change["change_id"]) for change in changes] != sorted(
            change_ids, key=_natural_id_key
        ):
            raise RevisionEvaluationError("machine ChangeId groups must be sorted and duplicate-free")
        sorted_ranges = sorted(all_ranges)
        previous: tuple[int, int] | None = None
        for start, end in sorted_ranges:
            if previous is not None:
                previous_start, previous_end = previous
                if start <= previous_start or start < previous_end:
                    raise RevisionEvaluationError("machine hunk source ranges overlap")
            previous = (start, end)
        answered = _safe_id_list(
            case["answered_material_question_ids"],
            field=f"machine case {artifact_id}.answered_material_question_ids",
        )
        represented = _safe_id_list(
            case["represented_question_ids"], field=f"machine case {artifact_id}.represented_question_ids"
        )
        omitted = _safe_id_list(
            case["intentionally_omitted_question_ids"],
            field=f"machine case {artifact_id}.intentionally_omitted_question_ids",
        )
        if set(represented) & set(omitted) or not set(represented).issubset(set(answered)) or not set(answered).issubset(
            set(represented) | set(omitted)
        ):
            raise RevisionEvaluationError("machine question coverage is incomplete or overlapping")
        if registry is not None and not set(answered).issubset(
            set(registry_by_id[artifact_id].material_question_ids)
        ):
            raise RevisionEvaluationError("machine answered questions are not fixed by the eligibility registry")
        reject_all = _validate_reject_all(case["reject_all"], field=f"machine case {artifact_id}.reject_all")
        if reject_all["source_sha256"] != editable_sha or reject_all["source_byte_count"] != editable_count:
            raise RevisionEvaluationError("machine reject-all source does not match editable source")
        extra_facts = _validate_machine_extra_facts(case, field=f"machine case {artifact_id}")
        if registry is not None:
            expected_intents = set(registry_by_id[artifact_id].intent_violating_change_ids)
            actual_intents = {
                str(intent_id)
                for change in changes
                for intent_id in change["intent_change_ids"]  # type: ignore[index]
            }
            if actual_intents != expected_intents:
                raise RevisionEvaluationError(
                    "machine intent_change_ids must exactly match the eligibility registry union"
                )
        approved = extra_facts.get("approved_output")
        if isinstance(approved, Mapping) and approved.get("status") == "composed":
            if approved.get("proposal_sha256") != extra_facts.get("proposal_sha256"):
                raise RevisionEvaluationError("machine approved output is bound to a different proposal")
            if approved.get("decision_count") != len(changes):
                raise RevisionEvaluationError("machine approved output does not cover every ChangeId")
        coverage = extra_facts.get("question_coverage")
        if not isinstance(coverage, list):
            raise RevisionEvaluationError(f"machine case {artifact_id} must include question_coverage")
        coverage_ids: set[str] = set()
        coverage_indexes: set[int] = set()
        known_change_indexes = set(range(len(changes)))
        for item in coverage:
            item_map = _mapping(item, field=f"machine case {artifact_id}.question_coverage.item")
            question_index = _safe_count(
                item_map["question_index"], field=f"machine case {artifact_id}.question_index"
            )
            question_id = _safe_id(
                item_map["question_id"], field=f"machine case {artifact_id}.question_id"
            )
            if question_index in coverage_indexes or question_id in coverage_ids:
                raise RevisionEvaluationError("machine question coverage identifiers must be unique")
            coverage_indexes.add(question_index)
            coverage_ids.add(question_id)
            status = _mapping(item_map["status"], field=f"machine case {artifact_id}.question_status")
            kind = status["kind"]
            if kind == "represented":
                if question_id not in set(represented):
                    raise RevisionEvaluationError("machine represented coverage is not in represented_question_ids")
                raw_change_ids = status["change_ids"]
                if not raw_change_ids or not set(raw_change_ids).issubset(known_change_indexes):
                    raise RevisionEvaluationError("machine question coverage references an unknown ChangeId")
            elif kind == "intentionally_omitted":
                if question_id not in set(omitted):
                    raise RevisionEvaluationError("machine omitted coverage is not in intentionally_omitted_question_ids")
            elif kind == "not_addressed":
                if question_id in set(answered) | set(represented) | set(omitted):
                    raise RevisionEvaluationError("machine not-addressed coverage contradicts question lists")
            else:
                raise RevisionEvaluationError("machine question coverage status is invalid")
        if not (set(answered) | set(represented) | set(omitted)) <= coverage_ids:
            raise RevisionEvaluationError("machine question coverage omits a listed question")
        total_replacement = sum(
            int(hunk["replacement_byte_count"])
            for change in changes
            for hunk in change["hunks"]  # type: ignore[index]
        )
        total_deleted = sum(end - start for start, end in all_ranges)
        if total_replacement > MAX_REVISION_REPLACEMENT_BYTES:
            raise RevisionEvaluationError("machine replacement output exceeds the production bound")
        if total_replacement + editable_count - total_deleted > MAX_REVISION_OUTPUT_BYTES:
            raise RevisionEvaluationError("machine composed output exceeds the production bound")
        if any(
            int(hunk["rationale_byte_count"]) > MAX_REVISION_RATIONALE_BYTES
            or int(hunk["replacement_byte_count"]) > MAX_REVISION_REPLACEMENT_BYTES
            for change in changes
            for hunk in change["hunks"]  # type: ignore[index]
        ):
            raise RevisionEvaluationError("machine rationale or replacement exceeds the production bound")
        result[artifact_id] = _MachineCase(
            artifact_id=artifact_id,
            corpus_artifact_sha256=corpus_sha,
            corpus_artifact_byte_count=corpus_count,
            request_artifact_sha256=request_sha,
            request_artifact_byte_count=request_count,
            corpus_artifact_lens=corpus_lens,
            review_scope_sha256=review_scope_sha,
            review_scope_byte_count=review_scope_count,
            editable_source_sha256=editable_sha,
            editable_source_byte_count=editable_count,
            source_binding_sha256=source_binding_sha,
            changes=tuple(_freeze(change) for change in changes),
            answered_material_question_ids=answered,
            represented_question_ids=represented,
            intentionally_omitted_question_ids=omitted,
            reject_all=MappingProxyType(dict(reject_all)),
            extra_facts=MappingProxyType(dict(_freeze(extra_facts))),
        )
    return result


def _machine_case_payload(case: _MachineCase) -> dict[str, object]:
    payload = {
        "artifact_id": case.artifact_id,
        "corpus_artifact_sha256": case.corpus_artifact_sha256,
        "corpus_artifact_byte_count": case.corpus_artifact_byte_count,
        "request_artifact_sha256": case.request_artifact_sha256,
        "request_artifact_byte_count": case.request_artifact_byte_count,
        "corpus_artifact_lens": case.corpus_artifact_lens,
        "review_scope_sha256": case.review_scope_sha256,
        "review_scope_byte_count": case.review_scope_byte_count,
        "editable_source_sha256": case.editable_source_sha256,
        "editable_source_byte_count": case.editable_source_byte_count,
        "source_binding_sha256": case.source_binding_sha256,
        "changes": [_copy(change) for change in case.changes],
        "answered_material_question_ids": list(case.answered_material_question_ids),
        "represented_question_ids": list(case.represented_question_ids),
        "intentionally_omitted_question_ids": list(case.intentionally_omitted_question_ids),
        "reject_all": _copy(case.reject_all),
    }
    payload.update(_copy(case.extra_facts))  # type: ignore[arg-type]
    return payload


def _result_extra_key(name: str) -> str:
    return "machine_question_coverage" if name == "question_coverage" else name


def _machine_receipt_normalized_digest(
    *,
    schema: str,
    corpus_version: str,
    manifest_sha256: str,
    runner_schema: str,
    runner_executable_sha256: str,
    cases: Mapping[str, dict[str, object]],
) -> str:
    payload = {
        "schema": schema,
        "corpus_version": corpus_version,
        "manifest_sha256": manifest_sha256,
        "runner_schema": runner_schema,
        "runner_executable_sha256": runner_executable_sha256,
        "cases": {artifact_id: cases[artifact_id] for artifact_id in sorted(cases)},
    }
    return canonical_registry_digest(payload)


def load_machine_receipt(
    path: Path,
    verification: evaluation.ManifestVerification,
    *,
    approved_machine_receipt_sha256: str,
    approved_runner_executable_sha256: str,
) -> MachineReceipt:
    """Load a v2 machine receipt and bind both external hashes."""

    fresh = _fresh_verification(verification)
    corpus_root = (fresh.root / CORPUS_RELATIVE).resolve()
    path = _external_path(path, corpus_root, field="machine receipt")
    if not path.is_file():
        raise RevisionEvaluationError("machine receipt must be an external regular file")
    approved_machine_receipt_sha256 = _safe_nonempty_sha256(
        approved_machine_receipt_sha256, field="approved_machine_receipt_sha256"
    )
    approved_runner_executable_sha256 = _safe_nonempty_sha256(
        approved_runner_executable_sha256, field="approved_runner_executable_sha256"
    )
    value, raw_sha256 = _json_load(path)
    if raw_sha256 != approved_machine_receipt_sha256:
        raise RevisionEvaluationError("machine receipt raw digest does not match its external anchor")
    _reject_forbidden_content(value, field="machine receipt")
    _reject_machine_ui_claims(value, field="machine receipt")
    payload = _mapping(value, field="machine receipt")
    _exact_keys(payload, _MACHINE_RECEIPT_KEYS, field="machine receipt")
    if payload["schema"] != MACHINE_RECEIPT_SCHEMA:
        raise RevisionEvaluationError("machine receipt schema is invalid")
    if payload["corpus_version"] != fresh.corpus_version or payload["manifest_sha256"] != fresh.manifest_sha256:
        raise RevisionEvaluationError("machine receipt corpus identity does not match the fresh manifest")
    runner_schema = _safe_id(payload["runner_schema"], field="machine receipt.runner_schema")
    if runner_schema != LOCAL_DIFF_RUNNER_SCHEMA:
        raise RevisionEvaluationError("machine receipt runner schema is invalid")
    runner_executable_sha256 = _safe_sha256(
        payload["runner_executable_sha256"], field="machine receipt.runner_executable_sha256"
    )
    if runner_executable_sha256 != approved_runner_executable_sha256:
        raise RevisionEvaluationError("machine receipt executable digest does not match its external anchor")
    normalized_cases = _validate_machine_cases(payload["cases"], fresh, None)
    case_payloads = {
        artifact_id: _machine_case_payload(normalized_cases[artifact_id])
        for artifact_id in _expected_artifact_ids(fresh)
    }
    normalized_sha256 = _machine_receipt_normalized_digest(
        schema=MACHINE_RECEIPT_SCHEMA,
        corpus_version=fresh.corpus_version,
        manifest_sha256=fresh.manifest_sha256,
        runner_schema=runner_schema,
        runner_executable_sha256=runner_executable_sha256,
        cases=case_payloads,
    )
    receipt = MachineReceipt(
        schema=MACHINE_RECEIPT_SCHEMA,
        corpus_version=fresh.corpus_version,
        manifest_sha256=fresh.manifest_sha256,
        runner_schema=runner_schema,
        runner_executable_sha256=runner_executable_sha256,
        receipt_sha256=raw_sha256,
        normalized_sha256=normalized_sha256,
        cases=tuple((artifact_id, case_payloads[artifact_id]) for artifact_id in _expected_artifact_ids(fresh)),
    )
    object.__setattr__(receipt, "trust_token", _MachineReceiptTrust(receipt, token=_MACHINE_RECEIPT_TRUST_TOKEN))
    return receipt


def _ensure_machine_receipt(
    receipt: MachineReceipt,
    verification: evaluation.ManifestVerification,
    registry: EligibilityRegistry,
    *,
    approved_machine_receipt_sha256: str,
    approved_runner_executable_sha256: str,
) -> tuple[MachineReceipt, dict[str, _MachineCase]]:
    if not isinstance(receipt, MachineReceipt):
        raise RevisionEvaluationError("machine facts must come from load_machine_receipt")
    if not isinstance(receipt.trust_token, _MachineReceiptTrust) or not receipt.trust_token.belongs_to(receipt):
        raise RevisionEvaluationError("machine facts must come from load_machine_receipt")
    if receipt.receipt_sha256 != _safe_nonempty_sha256(
        approved_machine_receipt_sha256, field="approved_machine_receipt_sha256"
    ):
        raise RevisionEvaluationError("machine receipt trust anchor does not match")
    if receipt.runner_executable_sha256 != _safe_nonempty_sha256(
        approved_runner_executable_sha256, field="approved_runner_executable_sha256"
    ):
        raise RevisionEvaluationError("machine receipt executable anchor does not match")
    fresh = _fresh_verification(verification)
    if receipt.schema != MACHINE_RECEIPT_SCHEMA:
        raise RevisionEvaluationError("machine receipt schema is invalid")
    if receipt.corpus_version != fresh.corpus_version or receipt.manifest_sha256 != fresh.manifest_sha256:
        raise RevisionEvaluationError("machine receipt corpus identity is stale")
    machine = _validate_machine_cases(receipt.by_artifact(), fresh, registry)
    case_payloads = {artifact_id: _machine_case_payload(machine[artifact_id]) for artifact_id in machine}
    normalized = _machine_receipt_normalized_digest(
        schema=receipt.schema,
        corpus_version=receipt.corpus_version,
        manifest_sha256=receipt.manifest_sha256,
        runner_schema=receipt.runner_schema,
        runner_executable_sha256=receipt.runner_executable_sha256,
        cases=case_payloads,
    )
    if normalized != receipt.normalized_sha256:
        raise RevisionEvaluationError("machine receipt facts were altered after loading")
    return receipt, machine


def load_native_acceptance(
    path: Path,
    *,
    approved_native_evidence_sha256: str,
    approved_native_executable_sha256: str,
    root: Path = REPO,
) -> NativeAcceptance:
    """Bind an external native Goal 07 PASS evidence file without copying it."""

    corpus_root = (Path(root).resolve() / CORPUS_RELATIVE).resolve()
    path = _external_path(path, corpus_root, field="native acceptance evidence")
    if not path.is_file():
        raise RevisionEvaluationError("native acceptance evidence must be an external regular file")
    approved_native_evidence_sha256 = _safe_nonempty_sha256(
        approved_native_evidence_sha256, field="approved_native_evidence_sha256"
    )
    approved_native_executable_sha256 = _safe_nonempty_sha256(
        approved_native_executable_sha256, field="approved_native_executable_sha256"
    )
    value, raw_sha256 = _json_load(path)
    if raw_sha256 != approved_native_evidence_sha256:
        raise RevisionEvaluationError("native acceptance raw digest does not match its external anchor")
    _reject_forbidden_content(value, field="native acceptance evidence")
    payload = _mapping(value, field="native acceptance evidence")
    if not isinstance(payload.get("schema"), str) or payload.get("schema") != NATIVE_ACCEPTANCE_SCHEMA:
        raise RevisionEvaluationError("native acceptance evidence schema is invalid")
    if type(payload.get("schema_version")) is not int or payload.get("schema_version") != 1:
        raise RevisionEvaluationError("native acceptance evidence schema is invalid")
    if not isinstance(payload.get("status"), str) or payload.get("status") != "PASS":
        raise RevisionEvaluationError("native acceptance evidence must have PASS status")
    executable = _mapping(payload.get("executable"), field="native acceptance executable")
    expected = _safe_sha256(executable.get("expected_sha256"), field="native executable.expected_sha256")
    actual = _safe_sha256(executable.get("sha256"), field="native executable.sha256")
    copied = _safe_sha256(executable.get("copied_sha256"), field="native executable.copied_sha256")
    for name in ("hash_verified", "copy_hash_verified"):
        if name in executable and not isinstance(executable[name], bool):
            raise RevisionEvaluationError(f"native executable.{name} must be a boolean")
    if "byte_count" in executable:
        _safe_count(executable["byte_count"], field="native executable.byte_count")
    if "transport" in payload:
        transport = _mapping(payload["transport"], field="native transport")
        if "request_count" in transport:
            _safe_count(transport["request_count"], field="native transport.request_count")
    if "summary" in payload:
        summary = _mapping(payload["summary"], field="native summary")
        for name in (
            "required_case_count",
            "passed_case_count",
            "blocked_case_count",
            "failed_case_count",
            "not_run_case_count",
        ):
            if name in summary:
                _safe_count(summary[name], field=f"native summary.{name}")
    if expected != approved_native_executable_sha256 or actual != expected or copied != expected:
        raise RevisionEvaluationError("native executable digest does not match its external anchor")
    cases = payload.get("cases")
    if not isinstance(cases, list):
        raise RevisionEvaluationError("native acceptance evidence does not prove PASS for every case")
    case_ids: list[str] = []
    for case in cases:
        if not isinstance(case, Mapping) or not isinstance(case.get("id"), str):
            raise RevisionEvaluationError("native acceptance evidence contains an invalid case identifier")
        if case.get("status") != "PASS":
            raise RevisionEvaluationError("native acceptance evidence does not prove PASS for every case")
        if "duration_ms" in case and case["duration_ms"] is not None and isinstance(case["duration_ms"], bool):
            raise RevisionEvaluationError("native case duration_ms must not be a boolean")
        observations = case.get("observations")
        if isinstance(observations, Mapping):
            for required_flag in (
                "foreground_verified",
                "loopback_provider",
                "server_loopback",
                "server_deterministic",
                "provider_paths_exact",
                "provider_requests_after_consent",
                "provider_review_before_revision",
                "provider_review_source_sha256_match",
                "provider_revision_source_sha256_match",
                "provider_revision_snapshot_match",
                "provider_revision_answer_sentinel_present",
            ):
                if required_flag in observations and observations[required_flag] is not True:
                    raise RevisionEvaluationError("native acceptance evidence failed full harness validation")
        case_ids.append(case["id"])
    if (
        len(cases) != len(NATIVE_REQUIRED_CASE_IDS)
        or len(set(case_ids)) != len(case_ids)
        or set(case_ids) != NATIVE_REQUIRED_CASE_IDS
    ):
        raise RevisionEvaluationError("native acceptance evidence does not prove PASS for every case")
    try:
        from .native import goal07 as native_goal07
        native_goal07.validate_evidence(dict(payload))
    except (KeyError, TypeError, ValueError, RecursionError) as error:
        raise RevisionEvaluationError("native acceptance evidence failed full harness validation") from error
    acceptance = NativeAcceptance(raw_sha256=raw_sha256, executable_sha256=actual)
    object.__setattr__(
        acceptance,
        "trust_token",
        _NativeAcceptanceTrust(acceptance, token=_NATIVE_ACCEPTANCE_TRUST_TOKEN),
    )
    return acceptance


load_native_pass_evidence = load_native_acceptance


def load_owner_inputs(
    directory: Path,
    verification: evaluation.ManifestVerification | None = None,
) -> dict[str, Mapping[str, object]]:
    """Load one metadata-only `<artifact-id>.json` file for every artifact."""

    fresh = _fresh_verification(verification)
    corpus_root = (fresh.root / CORPUS_RELATIVE).resolve()
    try:
        directory = _external_path(directory, corpus_root, field="owner input directory")
    except RevisionEvaluationError as error:
        raise OwnerInputRequired("owner input directory is unavailable") from error
    if not directory.is_dir():
        raise OwnerInputRequired("owner input directory is unavailable")
    result: dict[str, Mapping[str, object]] = {}
    for artifact_id in _expected_artifact_ids(fresh):
        try:
            path = _external_path(
                directory / f"{artifact_id}.json",
                corpus_root,
                field=f"owner input {artifact_id}",
            )
        except RevisionEvaluationError as error:
            raise OwnerInputRequired(f"owner input {artifact_id}.json is unavailable") from error
        if not path.is_file():
            raise OwnerInputRequired(f"owner input {artifact_id}.json is missing")
        value, _ = _json_load(path)
        _reject_forbidden_content(value, field=f"owner input {artifact_id}")
        result[artifact_id] = _mapping(value, field=f"owner input {artifact_id}")
    return result


def _scaffold_result(artifact_id: str, *, registry: EligibilityRegistry | None, machine: _MachineCase | None) -> dict[str, object]:
    result = {
        "artifact_id": artifact_id,
        "status": "awaiting_owner_input",
        "eligible": None if registry is None else registry.by_artifact()[artifact_id].eligible,
        "corpus_artifact_sha256": None if machine is None else machine.corpus_artifact_sha256,
        "corpus_artifact_byte_count": None if machine is None else machine.corpus_artifact_byte_count,
        "request_artifact_sha256": None if machine is None else machine.request_artifact_sha256,
        "request_artifact_byte_count": None if machine is None else machine.request_artifact_byte_count,
        "corpus_artifact_lens": None if machine is None else machine.corpus_artifact_lens,
        "review_scope_sha256": None if machine is None else machine.review_scope_sha256,
        "review_scope_byte_count": None if machine is None else machine.review_scope_byte_count,
        "editable_source_sha256": None if machine is None else machine.editable_source_sha256,
        "editable_source_byte_count": None if machine is None else machine.editable_source_byte_count,
        "source_binding_sha256": None if machine is None else machine.source_binding_sha256,
        "changes": [],
        "answered_material_question_ids": [] if machine is None else list(machine.answered_material_question_ids),
        "machine_represented_question_ids": [] if machine is None else list(machine.represented_question_ids),
        "machine_intentionally_omitted_question_ids": [] if machine is None else list(machine.intentionally_omitted_question_ids),
        "question_coverage": [],
        "intent_judgment": None,
        "clearer_due_to_answered_question": None,
        "reject_all": None,
    }
    if machine is not None:
        result.update(
            {_result_extra_key(str(key)): _copy(value) for key, value in machine.extra_facts.items()}
        )
    return result


def _ensure_registry(registry: EligibilityRegistry, verification: evaluation.ManifestVerification, *, approved_registry_sha256: str) -> EligibilityRegistry:
    if not isinstance(registry, EligibilityRegistry):
        raise RevisionEvaluationError("eligibility registry has an invalid type")
    payload = {
        "schema": registry.schema,
        "corpus_version": registry.corpus_version,
        "manifest_sha256": registry.manifest_sha256,
        "owner_annotations_sha256": registry.owner_annotations_sha256,
        "cases": [case.evidence() for case in registry.cases],
    }
    return _registry_from_payload(payload, _fresh_verification(verification), approved_registry_sha256=approved_registry_sha256)


def _ensure_native_acceptance(native: NativeAcceptance, *, approved_native_evidence_sha256: str, approved_native_executable_sha256: str) -> NativeAcceptance:
    if (
        not isinstance(native, NativeAcceptance)
        or not isinstance(native.trust_token, _NativeAcceptanceTrust)
        or not native.trust_token.belongs_to(native)
        or native.status != "PASS"
        or native.schema != NATIVE_ACCEPTANCE_SCHEMA
    ):
        raise RevisionEvaluationError("native acceptance must be a validated PASS receipt")
    if native.raw_sha256 != _safe_nonempty_sha256(approved_native_evidence_sha256, field="approved_native_evidence_sha256") or native.executable_sha256 != _safe_nonempty_sha256(approved_native_executable_sha256, field="approved_native_executable_sha256"):
        raise RevisionEvaluationError("native acceptance external anchors do not match")
    return native


def scaffold_evidence(
    verification: evaluation.ManifestVerification | None = None,
    *,
    registry: EligibilityRegistry | None = None,
    machine_receipt: MachineReceipt | None = None,
    native_acceptance: NativeAcceptance | None = None,
    approved_registry_sha256: str | None = None,
    approved_machine_receipt_sha256: str | None = None,
    approved_runner_executable_sha256: str | None = None,
    approved_native_evidence_sha256: str | None = None,
    approved_native_executable_sha256: str | None = None,
    created_at: str | None = None,
    reason: str = "owner_local_revision_inputs_required",
) -> dict[str, object]:
    """Build a fail-closed scaffold without inventing owner judgments."""

    fresh = _fresh_verification(verification)
    checked_registry: EligibilityRegistry | None = None
    checked_machine: MachineReceipt | None = None
    machine_cases: dict[str, _MachineCase] | None = None
    if registry is not None:
        if approved_registry_sha256 is None:
            raise RevisionEvaluationError("scaffold registry requires an external registry anchor")
        checked_registry = _ensure_registry(registry, fresh, approved_registry_sha256=approved_registry_sha256)
        if machine_receipt is not None:
            if approved_machine_receipt_sha256 is None or approved_runner_executable_sha256 is None:
                raise RevisionEvaluationError("scaffold machine receipt requires both external anchors")
            checked_machine, machine_cases = _ensure_machine_receipt(
                machine_receipt, fresh, checked_registry,
                approved_machine_receipt_sha256=approved_machine_receipt_sha256,
                approved_runner_executable_sha256=approved_runner_executable_sha256,
            )
    elif machine_receipt is not None or native_acceptance is not None:
        raise RevisionEvaluationError("scaffold machine/native facts require an eligibility registry")
    if native_acceptance is not None:
        if approved_native_evidence_sha256 is None or approved_native_executable_sha256 is None:
            raise RevisionEvaluationError("scaffold native acceptance requires both external anchors")
        _ensure_native_acceptance(native_acceptance, approved_native_evidence_sha256=approved_native_evidence_sha256, approved_native_executable_sha256=approved_native_executable_sha256)
    evidence = {
        "schema": EVIDENCE_SCHEMA,
        "created_at": _created_at(created_at),
        "corpus": _corpus_evidence(fresh),
        "registry": None if checked_registry is None else checked_registry.evidence(),
        "machine_receipt": None if checked_machine is None else checked_machine.evidence(),
        "native_acceptance": None if native_acceptance is None else native_acceptance.evidence(),
        "results": [
            _scaffold_result(artifact_id, registry=checked_registry, machine=None if machine_cases is None else machine_cases[artifact_id])
            for artifact_id in _expected_artifact_ids(fresh)
        ],
        "evaluation": {"status": "not_evaluated", "all_cases_satisfied": False, "reason": _safe_reason(reason)},
    }
    validate_evidence(
        evidence, verification=fresh,
        approved_registry_sha256=approved_registry_sha256,
        machine_receipt=checked_machine,
        approved_machine_receipt_sha256=approved_machine_receipt_sha256,
        approved_runner_executable_sha256=approved_runner_executable_sha256,
        native_acceptance=native_acceptance,
        approved_native_evidence_sha256=approved_native_evidence_sha256,
        approved_native_executable_sha256=approved_native_executable_sha256,
    )
    return evidence


def _validate_owner_inputs(value: Mapping[str, Mapping[str, object]], verification: evaluation.ManifestVerification, registry: EligibilityRegistry, machine: Mapping[str, _MachineCase]) -> dict[str, _OwnerCase]:
    expected_ids = _expected_artifact_ids(verification)
    if set(value) != set(expected_ids):
        raise OwnerInputRequired("owner inputs must cover every corpus artifact")
    result: dict[str, _OwnerCase] = {}
    for artifact_id in expected_ids:
        raw = _mapping(value[artifact_id], field=f"owner case {artifact_id}")
        _reject_forbidden_content(raw, field=f"owner case {artifact_id}")
        _exact_keys(raw, _OWNER_KEYS, field=f"owner case {artifact_id}")
        if raw["artifact_id"] != artifact_id:
            raise RevisionEvaluationError("owner artifact_id does not match its file name")
        raw_decisions = raw["change_decisions"]
        if not isinstance(raw_decisions, list):
            raise RevisionEvaluationError("owner change_decisions must be a list")
        expected_changes = {str(change["change_id"]) for change in machine[artifact_id].changes}
        decisions: list[Mapping[str, object]] = []
        seen_decisions: set[str] = set()
        for raw_decision in raw_decisions:
            decision = _mapping(raw_decision, field=f"owner case {artifact_id}.change_decision")
            _exact_keys(decision, _OWNER_DECISION_KEYS, field=f"owner case {artifact_id}.change_decision")
            change_id = _owner_change_id(
                decision["change_id"], artifact_id=artifact_id, expected=expected_changes
            )
            if change_id in seen_decisions or change_id not in expected_changes:
                raise RevisionEvaluationError("owner ChangeId decisions must match machine groups exactly")
            seen_decisions.add(change_id)
            decision_value = decision["decision"]
            if not isinstance(decision_value, str) or decision_value not in {"accept", "reject"}:
                raise RevisionEvaluationError("owner ChangeId decision must be accept or reject")
            decisions.append({"change_id": change_id, "decision": decision_value})
        if seen_decisions != expected_changes or [str(item["change_id"]) for item in decisions] != sorted(seen_decisions):
            raise RevisionEvaluationError("owner ChangeId decisions must cover every group exactly once")
        intent_judgment = raw["intent_judgment"]
        if not isinstance(intent_judgment, str) or intent_judgment not in {"preserved", "violated", "not_judged"}:
            raise RevisionEvaluationError("owner intent_judgment is invalid")
        raw_coverage = raw["question_coverage"]
        if not isinstance(raw_coverage, list):
            raise RevisionEvaluationError("owner question_coverage must be a list")
        answered = set(machine[artifact_id].answered_material_question_ids)
        represented = set(machine[artifact_id].represented_question_ids)
        omitted = set(machine[artifact_id].intentionally_omitted_question_ids)
        coverage: list[Mapping[str, str]] = []
        seen_questions: set[str] = set()
        for raw_item in raw_coverage:
            item = _mapping(raw_item, field=f"owner case {artifact_id}.question_coverage")
            _exact_keys(item, _QUESTION_COVERAGE_KEYS, field=f"owner case {artifact_id}.question_coverage")
            question_id = _safe_id(item["question_id"], field="owner question_id")
            if question_id in seen_questions or question_id not in answered:
                raise RevisionEvaluationError("owner question coverage must match answered questions")
            status = item["status"]
            if not isinstance(status, str) or status not in {"represented", "intentionally_omitted"}:
                raise RevisionEvaluationError("owner question coverage status is invalid")
            expected_status = "represented" if question_id in represented else "intentionally_omitted"
            if status != expected_status or question_id not in represented | omitted:
                raise RevisionEvaluationError("owner question coverage contradicts machine facts")
            seen_questions.add(question_id)
            coverage.append({"question_id": question_id, "status": status})
        if seen_questions != answered or [str(item["question_id"]) for item in coverage] != sorted(seen_questions):
            raise RevisionEvaluationError("owner question coverage must cover every answered question exactly once")
        clearer = _safe_bool(raw["clearer_due_to_answered_question"], field=f"owner case {artifact_id}.clearer_due_to_answered_question")
        if clearer and not answered:
            raise RevisionEvaluationError("clearer confirmation requires an answered material question")
        output_binding = _owner_output_binding(
            raw,
            machine[artifact_id],
            field=f"owner case {artifact_id}",
        )
        result[artifact_id] = _OwnerCase(
            artifact_id=artifact_id,
            change_decisions=tuple(decisions),
            intent_judgment=intent_judgment,
            question_coverage=tuple(coverage),
            clearer_due_to_answered_question=clearer,
            approved_output_sha256=output_binding[0],
            approved_proposal_sha256=output_binding[1],
            approved_decision_set_sha256=output_binding[2],
            approved_decision_file_sha256=output_binding[3],
        )
    return result


def _machine_case_local_diff_verified(machine: _MachineCase) -> bool:
    return all(
        hunk["local_diff_verified"] is True and hunk["utf8_boundaries_verified"] is True
        for change in machine.changes
        for hunk in change["hunks"]  # type: ignore[index]
    )


def _decision_set_digest(decisions: Sequence[Mapping[str, object]]) -> str:
    normalized: list[dict[str, object]] = []
    for item in decisions:
        change_id = str(item["change_id"])
        suffix = change_id.rsplit("-CH-", 1)
        normalized_id: object = change_id
        if len(suffix) == 2 and suffix[1].isdigit() and int(suffix[1]) > 0:
            normalized_id = int(suffix[1]) - 1
        normalized.append(
            {"change_id": normalized_id, "accepted": str(item["decision"]) == "accept"}
        )
    return canonical_registry_digest({"decisions": normalized})


def _case_satisfied(machine: _MachineCase, owner: _OwnerCase, eligibility: EligibilityCase) -> bool:
    if owner.intent_judgment != "preserved":
        return False
    forbidden = set(eligibility.intent_violating_change_ids)
    decisions = {str(item["change_id"]): str(item["decision"]) for item in owner.change_decisions}
    accepted = {
        change_id
        for change_id, decision in decisions.items()
        if decision == "accept"
    }
    accepted_with_hunks = {
        str(change["change_id"])
        for change in machine.changes
        if str(change["change_id"]) in accepted and change["hunks"]  # type: ignore[index]
    }
    if owner.clearer_due_to_answered_question and not accepted_with_hunks:
        return False
    for change in machine.changes:
        if decisions[str(change["change_id"])] == "accept" and forbidden.intersection(str(item) for item in change["intent_change_ids"]):  # type: ignore[index]
            return False
    if set(item["question_id"] for item in owner.question_coverage) != set(machine.answered_material_question_ids):
        return False
    approved = machine.extra_facts.get("approved_output")
    if not isinstance(approved, Mapping) or approved.get("status") != "composed":
        return False
    if approved.get("decision_count") != len(owner.change_decisions):
        return False
    if approved.get("decision_set_sha256") != _decision_set_digest(owner.change_decisions):
        return False
    if approved.get("proposal_sha256") != machine.extra_facts.get("proposal_sha256"):
        return False
    return _machine_case_local_diff_verified(machine) and machine.reject_all["source_sha256"] == machine.editable_source_sha256


def _recorded_result(machine: _MachineCase, owner: _OwnerCase, eligibility: EligibilityCase) -> dict[str, object]:
    decisions = {str(item["change_id"]): str(item["decision"]) for item in owner.change_decisions}
    result = {
        "artifact_id": machine.artifact_id,
        "status": "recorded",
        "eligible": eligibility.eligible,
        "corpus_artifact_sha256": machine.corpus_artifact_sha256,
        "corpus_artifact_byte_count": machine.corpus_artifact_byte_count,
        "request_artifact_sha256": machine.request_artifact_sha256,
        "request_artifact_byte_count": machine.request_artifact_byte_count,
        "corpus_artifact_lens": machine.corpus_artifact_lens,
        "review_scope_sha256": machine.review_scope_sha256,
        "review_scope_byte_count": machine.review_scope_byte_count,
        "editable_source_sha256": machine.editable_source_sha256,
        "editable_source_byte_count": machine.editable_source_byte_count,
        "source_binding_sha256": machine.source_binding_sha256,
        "changes": [{**_copy(change), "decision": decisions[str(change["change_id"])]} for change in machine.changes],
        "answered_material_question_ids": list(machine.answered_material_question_ids),
        "machine_represented_question_ids": list(machine.represented_question_ids),
        "machine_intentionally_omitted_question_ids": list(machine.intentionally_omitted_question_ids),
        "question_coverage": [dict(item) for item in owner.question_coverage],
        "intent_judgment": owner.intent_judgment,
        "clearer_due_to_answered_question": owner.clearer_due_to_answered_question,
        "approved_output_sha256": owner.approved_output_sha256,
        "approved_proposal_sha256": owner.approved_proposal_sha256,
        "approved_decision_set_sha256": owner.approved_decision_set_sha256,
        "approved_decision_file_sha256": owner.approved_decision_file_sha256,
        "reject_all": _copy(machine.reject_all),
    }
    result.update(
        {_result_extra_key(str(key)): _copy(value) for key, value in machine.extra_facts.items()}
    )
    return result


def _corpus_evidence(verification: evaluation.ManifestVerification) -> dict[str, object]:
    return {
        "version": verification.corpus_version,
        "manifest_sha256": verification.manifest_sha256,
        "owner_annotations_sha256": _owner_annotations_sha256(verification),
        "artifact_count": len(verification.artifacts),
    }


def _validate_native_output(value: object, *, native_acceptance: NativeAcceptance, approved_native_evidence_sha256: str, approved_native_executable_sha256: str) -> None:
    payload = _mapping(value, field="evidence.native_acceptance")
    _exact_keys(payload, frozenset({"schema", "status", "raw_sha256", "executable_sha256"}), field="evidence.native_acceptance")
    if not isinstance(payload["schema"], str) or not isinstance(payload["status"], str):
        raise RevisionEvaluationError("evidence native acceptance metadata has invalid types")
    actual = NativeAcceptance(
        schema=payload["schema"], status=payload["status"],
        raw_sha256=_safe_sha256(payload["raw_sha256"], field="evidence.native_acceptance.raw_sha256"),
        executable_sha256=_safe_sha256(payload["executable_sha256"], field="evidence.native_acceptance.executable_sha256"),
    )
    expected = _ensure_native_acceptance(
        native_acceptance,
        approved_native_evidence_sha256=approved_native_evidence_sha256,
        approved_native_executable_sha256=approved_native_executable_sha256,
    )
    if (
        actual.schema != expected.schema
        or actual.status != expected.status
        or actual.raw_sha256 != expected.raw_sha256
        or actual.executable_sha256 != expected.executable_sha256
    ):
        raise RevisionEvaluationError("evidence native acceptance metadata does not match the external receipt")


def _validate_registry_output(value: object, verification: evaluation.ManifestVerification, *, approved_registry_sha256: str) -> EligibilityRegistry:
    payload = _mapping(value, field="evidence.registry")
    _exact_keys(payload, _REGISTRY_KEYS | {"registry_sha256", "owner_approval_required"}, field="evidence.registry")
    if payload["owner_approval_required"] is not True:
        raise RevisionEvaluationError("evidence registry must retain owner approval")
    embedded = _safe_sha256(payload["registry_sha256"], field="evidence.registry.registry_sha256")
    base = {key: payload[key] for key in _REGISTRY_KEYS}
    if embedded != canonical_registry_digest(base):
        raise RevisionEvaluationError("evidence registry digest is inconsistent")
    return _registry_from_payload(base, verification, approved_registry_sha256=approved_registry_sha256)


def _validate_recorded_result(result: object, *, artifact_id: str, registry: EligibilityRegistry, machine: _MachineCase) -> None:
    result_map = _mapping(result, field=f"evidence result {artifact_id}")
    required = frozenset({
        "artifact_id", "status", "eligible", "corpus_artifact_sha256", "corpus_artifact_byte_count",
        "request_artifact_sha256", "request_artifact_byte_count",
        "corpus_artifact_lens",
        "review_scope_sha256", "review_scope_byte_count", "editable_source_sha256", "editable_source_byte_count",
        "source_binding_sha256",
        "changes", "answered_material_question_ids", "machine_represented_question_ids",
         "machine_intentionally_omitted_question_ids", "question_coverage", "intent_judgment",
         "clearer_due_to_answered_question", "approved_output_sha256",
         "approved_proposal_sha256", "approved_decision_set_sha256",
         "approved_decision_file_sha256", "reject_all",
    })
    allowed_result_keys = required | frozenset(_result_extra_key(str(key)) for key in machine.extra_facts)
    _exact_keys(result_map, allowed_result_keys, field=f"evidence result {artifact_id}")
    if result_map["artifact_id"] != artifact_id or result_map["status"] != "recorded":
        raise RevisionEvaluationError("evidence result identity is invalid")
    expected = _machine_case_payload(machine)
    aliases = {
        "machine_represented_question_ids": "represented_question_ids",
        "machine_intentionally_omitted_question_ids": "intentionally_omitted_question_ids",
    }
    for field in ("corpus_artifact_sha256", "corpus_artifact_byte_count", "request_artifact_sha256", "request_artifact_byte_count", "corpus_artifact_lens", "review_scope_sha256", "review_scope_byte_count", "editable_source_sha256", "editable_source_byte_count", "source_binding_sha256", "answered_material_question_ids", "reject_all"):
        if result_map[field] != expected[field]:
            raise RevisionEvaluationError("evidence result machine facts do not match the receipt")
    for field, expected_field in aliases.items():
        if result_map[field] != expected[expected_field]:
            raise RevisionEvaluationError("evidence result question facts do not match the receipt")
    for field, expected_value in machine.extra_facts.items():
        output_field = _result_extra_key(str(field))
        if result_map[output_field] != _copy(expected_value):
            raise RevisionEvaluationError("evidence result evaluator facts do not match the receipt")
    raw_changes = result_map["changes"]
    if not isinstance(raw_changes, list):
        raise RevisionEvaluationError("evidence result changes must be a list")
    machine_changes = {str(change["change_id"]): change for change in machine.changes}
    seen: set[str] = set()
    for raw_change in raw_changes:
        change = _mapping(raw_change, field=f"evidence result {artifact_id}.change")
        _exact_keys(change, _MACHINE_CHANGE_KEYS | {"decision"}, field=f"evidence result {artifact_id}.change")
        change_id = _safe_id(change["change_id"], field="evidence change_id")
        decision_value = change["decision"]
        if (
            change_id in seen
            or change_id not in machine_changes
            or not isinstance(decision_value, str)
            or decision_value not in {"accept", "reject"}
        ):
            raise RevisionEvaluationError("evidence ChangeId decisions are invalid")
        if change["intent_change_ids"] != _copy(machine_changes[change_id]["intent_change_ids"]) or change["hunks"] != _copy(machine_changes[change_id]["hunks"]):
            raise RevisionEvaluationError("evidence ChangeId machine facts were altered")
        seen.add(change_id)
    if seen != set(machine_changes):
        raise RevisionEvaluationError("evidence ChangeId decisions are incomplete")
    coverage = result_map["question_coverage"]
    if not isinstance(coverage, list):
        raise RevisionEvaluationError("evidence question_coverage must be a list")
    seen_questions: set[str] = set()
    for raw_item in coverage:
        item = _mapping(raw_item, field="evidence question coverage")
        _exact_keys(item, _QUESTION_COVERAGE_KEYS, field="evidence question coverage")
        question_id = _safe_id(item["question_id"], field="evidence question_id")
        status = item["status"]
        if (
            question_id in seen_questions
            or question_id not in set(machine.answered_material_question_ids)
            or not isinstance(status, str)
            or status not in {"represented", "intentionally_omitted"}
        ):
            raise RevisionEvaluationError("evidence question coverage is invalid")
        expected_status = (
            "represented"
            if question_id in set(machine.represented_question_ids)
            else "intentionally_omitted"
        )
        if status != expected_status:
            raise RevisionEvaluationError("evidence question coverage contradicts machine facts")
        seen_questions.add(question_id)
    if seen_questions != set(machine.answered_material_question_ids):
        raise RevisionEvaluationError("evidence question coverage is incomplete")
    intent_judgment = result_map["intent_judgment"]
    if not isinstance(intent_judgment, str) or intent_judgment not in {"preserved", "violated", "not_judged"}:
        raise RevisionEvaluationError("evidence intent judgment is invalid")
    _safe_bool(result_map["clearer_due_to_answered_question"], field="evidence clearer_due_to_answered_question")
    _owner_output_binding(result_map, machine, field=f"evidence result {artifact_id}")
    _validate_reject_all(result_map["reject_all"], field="evidence reject_all")
    if result_map["eligible"] is not registry.by_artifact()[artifact_id].eligible:
        raise RevisionEvaluationError("evidence eligibility does not match the registry")


def _validate_scaffold_result(
    result: object,
    *,
    artifact_id: str,
    registry: EligibilityRegistry | None,
    machine: _MachineCase | None,
) -> None:
    result_map = _mapping(result, field=f"scaffold result {artifact_id}")
    required = frozenset(
        {
            "artifact_id",
            "status",
            "eligible",
            "corpus_artifact_sha256",
            "corpus_artifact_byte_count",
            "request_artifact_sha256",
            "request_artifact_byte_count",
            "corpus_artifact_lens",
            "review_scope_sha256",
            "review_scope_byte_count",
            "editable_source_sha256",
            "editable_source_byte_count",
            "source_binding_sha256",
            "changes",
            "answered_material_question_ids",
            "machine_represented_question_ids",
            "machine_intentionally_omitted_question_ids",
            "question_coverage",
            "intent_judgment",
            "clearer_due_to_answered_question",
            "reject_all",
        }
    )
    allowed_result_keys = required | (
        frozenset(_result_extra_key(str(key)) for key in machine.extra_facts)
        if machine is not None
        else frozenset()
    )
    _exact_keys(result_map, allowed_result_keys, field=f"scaffold result {artifact_id}")
    if result_map["artifact_id"] != artifact_id or result_map["status"] != "awaiting_owner_input":
        raise RevisionEvaluationError("scaffold result identity is invalid")
    expected_eligible = None if registry is None else registry.by_artifact()[artifact_id].eligible
    if result_map["eligible"] is not expected_eligible:
        raise RevisionEvaluationError("scaffold eligibility must remain owner-bound")
    if result_map["changes"] != [] or result_map["question_coverage"] != []:
        raise RevisionEvaluationError("scaffold must not contain owner decisions")
    if result_map["intent_judgment"] is not None or result_map["clearer_due_to_answered_question"] is not None:
        raise RevisionEvaluationError("scaffold must not contain owner judgments")
    if result_map["reject_all"] is not None:
        raise RevisionEvaluationError("scaffold must not contain reject-all facts")
    if machine is None:
        expected = {
            "corpus_artifact_sha256": None,
            "corpus_artifact_byte_count": None,
            "request_artifact_sha256": None,
            "request_artifact_byte_count": None,
            "corpus_artifact_lens": None,
            "review_scope_sha256": None,
            "review_scope_byte_count": None,
            "editable_source_sha256": None,
            "editable_source_byte_count": None,
            "source_binding_sha256": None,
            "answered_material_question_ids": [],
            "machine_represented_question_ids": [],
            "machine_intentionally_omitted_question_ids": [],
        }
    else:
        machine_payload = _machine_case_payload(machine)
        expected = {
            "corpus_artifact_sha256": machine_payload["corpus_artifact_sha256"],
            "corpus_artifact_byte_count": machine_payload["corpus_artifact_byte_count"],
            "request_artifact_sha256": machine_payload["request_artifact_sha256"],
            "request_artifact_byte_count": machine_payload["request_artifact_byte_count"],
            "corpus_artifact_lens": machine_payload["corpus_artifact_lens"],
            "review_scope_sha256": machine_payload["review_scope_sha256"],
            "review_scope_byte_count": machine_payload["review_scope_byte_count"],
            "editable_source_sha256": machine_payload["editable_source_sha256"],
            "editable_source_byte_count": machine_payload["editable_source_byte_count"],
            "source_binding_sha256": machine_payload["source_binding_sha256"],
            "answered_material_question_ids": machine_payload["answered_material_question_ids"],
            "machine_represented_question_ids": machine_payload["represented_question_ids"],
            "machine_intentionally_omitted_question_ids": machine_payload["intentionally_omitted_question_ids"],
        }
        for field, expected_value in machine.extra_facts.items():
            output_field = _result_extra_key(str(field))
            if result_map[output_field] != _copy(expected_value):
                raise RevisionEvaluationError("scaffold evaluator facts do not match the bound receipt")
    for field, expected_value in expected.items():
        if result_map[field] != expected_value:
            raise RevisionEvaluationError("scaffold machine facts do not match the bound receipt")


def validate_evidence(value: object, *, verification: evaluation.ManifestVerification | None = None, approved_registry_sha256: str | None = None, machine_receipt: MachineReceipt | None = None, approved_machine_receipt_sha256: str | None = None, approved_runner_executable_sha256: str | None = None, native_acceptance: NativeAcceptance | None = None, approved_native_evidence_sha256: str | None = None, approved_native_executable_sha256: str | None = None) -> None:
    """Validate a scaffold or recorded v2 evidence object against fresh facts."""

    fresh = _fresh_verification(verification)
    _reject_forbidden_content(value, field="revision evidence")
    evidence = _mapping(value, field="revision evidence")
    required = frozenset({"schema", "created_at", "corpus", "registry", "machine_receipt", "native_acceptance", "results", "evaluation"})
    _exact_keys(evidence, required, field="revision evidence")
    if evidence["schema"] != EVIDENCE_SCHEMA or evidence["corpus"] != _corpus_evidence(fresh):
        raise RevisionEvaluationError("revision evidence schema or corpus identity is invalid")
    _safe_timestamp(evidence["created_at"])
    raw_results = evidence["results"]
    if not isinstance(raw_results, list) or len(raw_results) != EXPECTED_ARTIFACT_COUNT:
        raise RevisionEvaluationError("revision evidence must contain every corpus artifact")
    artifact_ids = _expected_artifact_ids(fresh)
    if [item.get("artifact_id") if isinstance(item, Mapping) else None for item in raw_results] != list(artifact_ids):
        raise RevisionEvaluationError("revision evidence results must be in corpus order")
    registry: EligibilityRegistry | None = None
    if evidence["registry"] is not None:
        if approved_registry_sha256 is None:
            raise RevisionEvaluationError("evidence registry requires an external anchor")
        registry = _validate_registry_output(evidence["registry"], fresh, approved_registry_sha256=approved_registry_sha256)
    elif machine_receipt is not None or native_acceptance is not None:
        raise RevisionEvaluationError("machine/native evidence requires a registry")
    checked_machine: MachineReceipt | None = None
    machine_cases: dict[str, _MachineCase] | None = None
    if evidence["machine_receipt"] is not None:
        if machine_receipt is None or registry is None or approved_machine_receipt_sha256 is None or approved_runner_executable_sha256 is None:
            raise RevisionEvaluationError("evidence machine receipt is not externally bound")
        checked_machine, machine_cases = _ensure_machine_receipt(machine_receipt, fresh, registry, approved_machine_receipt_sha256=approved_machine_receipt_sha256, approved_runner_executable_sha256=approved_runner_executable_sha256)
        if evidence["machine_receipt"] != checked_machine.evidence():
            raise RevisionEvaluationError("evidence machine receipt metadata does not match the receipt")
    elif machine_receipt is not None:
        raise RevisionEvaluationError("evidence is missing machine receipt metadata")
    checked_native: NativeAcceptance | None = None
    if evidence["native_acceptance"] is not None:
        if native_acceptance is None or approved_native_evidence_sha256 is None or approved_native_executable_sha256 is None:
            raise RevisionEvaluationError("evidence native acceptance is not externally bound")
        _validate_native_output(evidence["native_acceptance"], native_acceptance=native_acceptance, approved_native_evidence_sha256=approved_native_evidence_sha256, approved_native_executable_sha256=approved_native_executable_sha256)
        checked_native = native_acceptance
    elif native_acceptance is not None:
        raise RevisionEvaluationError("evidence is missing native acceptance metadata")
    evaluation_value = _mapping(evidence["evaluation"], field="revision evidence evaluation")
    status = evaluation_value.get("status")
    if status == "not_evaluated":
        _exact_keys(evaluation_value, frozenset({"status", "all_cases_satisfied", "reason"}), field="scaffold evaluation")
        if evaluation_value["all_cases_satisfied"] is not False:
            raise RevisionEvaluationError("scaffold all_cases_satisfied must be false")
        _safe_reason(evaluation_value["reason"])
        for artifact_id, raw_result in zip(artifact_ids, raw_results, strict=True):
            _validate_scaffold_result(
                raw_result,
                artifact_id=artifact_id,
                registry=registry,
                machine=None if machine_cases is None else machine_cases[artifact_id],
            )
        return
    if status != "recorded" or registry is None or machine_cases is None or checked_native is None:
        raise RevisionEvaluationError("recorded evidence requires registry, machine receipt, and native PASS")
    recorded_keys = frozenset({"status", "all_cases_satisfied", "eligible_case_count", "intent_preserved_count", "question_coverage_complete_count", "local_diff_verified_count", "reject_all_verified_count", "clearer_due_to_answered_question_count", "contract_reference"})
    _exact_keys(evaluation_value, recorded_keys, field="recorded evaluation")
    if evaluation_value["contract_reference"] != REVISION_CONTRACT:
        raise RevisionEvaluationError("recorded evaluation contract reference is invalid")
    all_cases_satisfied = _safe_bool(evaluation_value["all_cases_satisfied"], field="evaluation.all_cases_satisfied")
    for field in recorded_keys - {"status", "all_cases_satisfied", "contract_reference"}:
        _safe_count(evaluation_value[field], field=f"evaluation.{field}")
    eligible_ids = [artifact_id for artifact_id in artifact_ids if registry.by_artifact()[artifact_id].eligible]
    satisfied_count = intent_count = question_count = local_count = clearer_count = 0
    for artifact_id, raw_result in zip(artifact_ids, raw_results, strict=True):
        _validate_recorded_result(raw_result, artifact_id=artifact_id, registry=registry, machine=machine_cases[artifact_id])
        if not registry.by_artifact()[artifact_id].eligible:
            continue
        result_map = _mapping(raw_result, field=f"evidence result {artifact_id}")
        decisions = {str(item["change_id"]): str(item["decision"]) for item in result_map["changes"]}  # type: ignore[union-attr]
        owner = _OwnerCase(
            artifact_id=artifact_id,
            change_decisions=tuple({"change_id": key, "decision": decisions[key]} for key in sorted(decisions)),
            intent_judgment=result_map["intent_judgment"],
            question_coverage=tuple(_mapping(item, field="evidence question coverage") for item in result_map["question_coverage"]),  # type: ignore[arg-type,union-attr]
            clearer_due_to_answered_question=bool(result_map["clearer_due_to_answered_question"]),
            approved_output_sha256=result_map["approved_output_sha256"],
            approved_proposal_sha256=result_map["approved_proposal_sha256"],
            approved_decision_set_sha256=result_map["approved_decision_set_sha256"],
            approved_decision_file_sha256=result_map["approved_decision_file_sha256"],
        )
        machine_case = machine_cases[artifact_id]
        satisfied = _case_satisfied(machine_case, owner, registry.by_artifact()[artifact_id])
        satisfied_count += satisfied
        intent_count += owner.intent_judgment == "preserved" and satisfied
        question_count += set(item["question_id"] for item in owner.question_coverage) == set(machine_case.answered_material_question_ids)
        local_count += _machine_case_local_diff_verified(machine_case)
        clearer_count += owner.clearer_due_to_answered_question
    expected_all = bool(eligible_ids) and satisfied_count == len(eligible_ids) and clearer_count > 0
    expected_counts = {
        "eligible_case_count": len(eligible_ids),
        "intent_preserved_count": intent_count,
        "question_coverage_complete_count": question_count,
        "local_diff_verified_count": local_count,
        "reject_all_verified_count": len(eligible_ids),
        "clearer_due_to_answered_question_count": clearer_count,
    }
    for key, expected in expected_counts.items():
        if evaluation_value[key] != expected:
            raise RevisionEvaluationError(f"{key} does not match the recorded results")
    if all_cases_satisfied is not expected_all:
        raise RevisionEvaluationError("all_cases_satisfied does not match the explicit all-cases contract")


def evidence_from_owner_inputs(verification: evaluation.ManifestVerification, registry: EligibilityRegistry, machine_receipt: MachineReceipt, owner_cases: Mapping[str, Mapping[str, object]], *, approved_registry_sha256: str, approved_machine_receipt_sha256: str, approved_runner_executable_sha256: str, native_acceptance: NativeAcceptance, approved_native_evidence_sha256: str, approved_native_executable_sha256: str, created_at: str | None = None) -> dict[str, object]:
    """Build and validate a v2 record from owner-local inputs."""

    fresh = _fresh_verification(verification)
    checked_registry = _ensure_registry(registry, fresh, approved_registry_sha256=approved_registry_sha256)
    checked_machine, machine = _ensure_machine_receipt(machine_receipt, fresh, checked_registry, approved_machine_receipt_sha256=approved_machine_receipt_sha256, approved_runner_executable_sha256=approved_runner_executable_sha256)
    checked_native = _ensure_native_acceptance(native_acceptance, approved_native_evidence_sha256=approved_native_evidence_sha256, approved_native_executable_sha256=approved_native_executable_sha256)
    owners = _validate_owner_inputs(owner_cases, fresh, checked_registry, machine)
    registry_by_id = checked_registry.by_artifact()
    artifact_ids = _expected_artifact_ids(fresh)
    results = [_recorded_result(machine[artifact_id], owners[artifact_id], registry_by_id[artifact_id]) for artifact_id in artifact_ids]
    eligible_ids = [artifact_id for artifact_id in artifact_ids if registry_by_id[artifact_id].eligible]
    case_satisfied = {artifact_id: _case_satisfied(machine[artifact_id], owners[artifact_id], registry_by_id[artifact_id]) for artifact_id in eligible_ids}
    clearer_count = sum(owners[artifact_id].clearer_due_to_answered_question for artifact_id in eligible_ids)
    local_count = sum(_machine_case_local_diff_verified(machine[artifact_id]) for artifact_id in eligible_ids)
    intent_count = sum(owners[artifact_id].intent_judgment == "preserved" and case_satisfied[artifact_id] for artifact_id in eligible_ids)
    question_count = sum(set(item["question_id"] for item in owners[artifact_id].question_coverage) == set(machine[artifact_id].answered_material_question_ids) for artifact_id in eligible_ids)
    all_cases_satisfied = bool(eligible_ids) and all(case_satisfied.values()) and clearer_count > 0
    evidence = {
        "schema": EVIDENCE_SCHEMA,
        "created_at": _created_at(created_at),
        "corpus": _corpus_evidence(fresh),
        "registry": checked_registry.evidence(),
        "machine_receipt": checked_machine.evidence(),
        "native_acceptance": checked_native.evidence(),
        "results": results,
        "evaluation": {
            "status": "recorded",
            "all_cases_satisfied": all_cases_satisfied,
            "eligible_case_count": len(eligible_ids),
            "intent_preserved_count": intent_count,
            "question_coverage_complete_count": question_count,
            "local_diff_verified_count": local_count,
            "reject_all_verified_count": len(eligible_ids),
            "clearer_due_to_answered_question_count": clearer_count,
            "contract_reference": REVISION_CONTRACT,
        },
    }
    validate_evidence(evidence, verification=fresh, approved_registry_sha256=approved_registry_sha256, machine_receipt=checked_machine, approved_machine_receipt_sha256=approved_machine_receipt_sha256, approved_runner_executable_sha256=approved_runner_executable_sha256, native_acceptance=checked_native, approved_native_evidence_sha256=approved_native_evidence_sha256, approved_native_executable_sha256=approved_native_executable_sha256)
    return evidence


def write_evidence(path: Path, evidence: Mapping[str, object], *, root: Path = REPO, **validation_kwargs: object) -> None:
    """Write validated evidence using exclusive create-new semantics."""

    root = Path(root).resolve()
    corpus_root = (root / CORPUS_RELATIVE).resolve()
    destination = _external_path(path, corpus_root, field="evidence path")
    if destination.exists() or destination.is_symlink():
        raise RevisionEvaluationError("evidence destination already exists; create-new output is required")
    fresh = _fresh_verification(evaluation.verify_manifest(root))
    validate_evidence(dict(evidence), verification=fresh, **validation_kwargs)
    flags = os.O_WRONLY | os.O_CREAT | os.O_EXCL | getattr(os, "O_BINARY", 0)
    payload = json.dumps(dict(evidence), indent=2, sort_keys=True, ensure_ascii=True) + "\n"
    try:
        destination.parent.mkdir(parents=True, exist_ok=True)
        _reject_symlink_components(destination, field="evidence path")
        descriptor = os.open(str(destination), flags, 0o600)
        with os.fdopen(descriptor, "w", encoding="utf-8", newline="\n") as stream:
            stream.write(payload)
    except FileExistsError as error:
        raise RevisionEvaluationError("evidence destination already exists; create-new output is required") from error
    except OSError as error:
        raise RevisionEvaluationError("could not write revision evidence") from error


def _print_json(value: Mapping[str, object]) -> None:
    print(json.dumps(value, indent=2, sort_keys=True, ensure_ascii=True))


def _add_external_args(parser: argparse.ArgumentParser) -> None:
    parser.add_argument("--eligibility-registry", "--registry", dest="eligibility_registry", type=Path)
    parser.add_argument("--approved-registry-sha256")
    parser.add_argument("--machine-receipt", "--machine", dest="machine_receipt", type=Path)
    parser.add_argument("--approved-machine-receipt-sha256")
    parser.add_argument(
        "--approved-runner-executable-sha256",
        "--approved-machine-executable-sha256",
        dest="approved_runner_executable_sha256",
    )
    parser.add_argument(
        "--native-evidence",
        "--native-acceptance",
        "--native-pass-evidence",
        dest="native_evidence",
        type=Path,
    )
    parser.add_argument(
        "--approved-native-evidence-sha256",
        "--approved-native-evidence-raw-sha256",
        "--native-evidence-sha256",
        dest="approved_native_evidence_sha256",
    )
    parser.add_argument(
        "--approved-native-executable-sha256",
        "--native-executable-sha256",
        dest="approved_native_executable_sha256",
    )


def parser() -> argparse.ArgumentParser:
    result = argparse.ArgumentParser(prog="revision-evaluation", description="Verify Goal 07 revision evaluation facts and owner evidence.")
    subcommands = result.add_subparsers(dest="command", required=True)
    verify = subcommands.add_parser("verify-manifest", help="Verify the immutable Goal 01 corpus manifest.")
    verify.add_argument("--root", type=Path, default=REPO, help=argparse.SUPPRESS)
    verify.add_argument("--json", action="store_true", help=argparse.SUPPRESS)
    scaffold = subcommands.add_parser("scaffold", aliases=["init"], help="Write a fail-closed owner-input scaffold.")
    scaffold.add_argument("--root", type=Path, default=REPO, help=argparse.SUPPRESS)
    scaffold.add_argument("--evidence", type=Path)
    scaffold.add_argument("--created-at", help=argparse.SUPPRESS)
    _add_external_args(scaffold)
    record = subcommands.add_parser("record", aliases=["run"], help="Record owner-local revision judgments.")
    record.add_argument("--root", type=Path, default=REPO, help=argparse.SUPPRESS)
    record.add_argument("--owner-input-dir", type=Path)
    record.add_argument("--evidence", type=Path)
    record.add_argument("--created-at", help=argparse.SUPPRESS)
    _add_external_args(record)
    return result


def _load_external_inputs(namespace: argparse.Namespace, verification: evaluation.ManifestVerification) -> tuple[EligibilityRegistry, MachineReceipt, NativeAcceptance]:
    required = (namespace.eligibility_registry, namespace.approved_registry_sha256, namespace.machine_receipt, namespace.approved_machine_receipt_sha256, namespace.approved_runner_executable_sha256, namespace.native_evidence, namespace.approved_native_evidence_sha256, namespace.approved_native_executable_sha256)
    if any(item is None for item in required):
        raise OwnerInputRequired("registry, machine receipt, native PASS evidence, and all external anchors are required")
    registry = load_eligibility_registry(namespace.eligibility_registry, verification, approved_registry_sha256=namespace.approved_registry_sha256)
    machine = load_machine_receipt(namespace.machine_receipt, verification, approved_machine_receipt_sha256=namespace.approved_machine_receipt_sha256, approved_runner_executable_sha256=namespace.approved_runner_executable_sha256)
    native = load_native_acceptance(namespace.native_evidence, approved_native_evidence_sha256=namespace.approved_native_evidence_sha256, approved_native_executable_sha256=namespace.approved_native_executable_sha256, root=namespace.root)
    return registry, machine, native


def _write_or_print(evidence: Mapping[str, object], *, output: Path | None, root: Path, validation_kwargs: Mapping[str, object] | None = None) -> None:
    if output is None:
        _print_json(evidence)
    else:
        write_evidence(output, evidence, root=root, **dict(validation_kwargs or {}))


def main(argv: Sequence[str] | None = None) -> int:
    namespace = parser().parse_args(argv)
    try:
        verification = verify_manifest(namespace.root)
        if namespace.command == "verify-manifest":
            _print_json(verification.evidence())
            return 0
        if namespace.command in {"scaffold", "init"}:
            registry = machine = native = None
            if namespace.eligibility_registry is not None:
                registry, machine, native = _load_external_inputs(namespace, verification)
            evidence = scaffold_evidence(verification, registry=registry, machine_receipt=machine, native_acceptance=native, approved_registry_sha256=namespace.approved_registry_sha256, approved_machine_receipt_sha256=namespace.approved_machine_receipt_sha256, approved_runner_executable_sha256=namespace.approved_runner_executable_sha256, approved_native_evidence_sha256=namespace.approved_native_evidence_sha256, approved_native_executable_sha256=namespace.approved_native_executable_sha256, created_at=namespace.created_at)
            _write_or_print(evidence, output=namespace.evidence, root=namespace.root)
            return 0
        if namespace.command not in {"record", "run"}:
            raise RevisionEvaluationError("unknown revision evaluation command")
        if namespace.owner_input_dir is None:
            evidence = scaffold_evidence(verification, created_at=namespace.created_at, reason="owner_local_revision_inputs_required")
            _write_or_print(evidence, output=namespace.evidence, root=namespace.root)
            print("error: owner-local revision inputs are required; no endpoint was contacted", file=sys.stderr)
            return 2
        try:
            registry, machine, native = _load_external_inputs(namespace, verification)
            owner_inputs = load_owner_inputs(namespace.owner_input_dir, verification)
            evidence = evidence_from_owner_inputs(verification, registry, machine, owner_inputs, approved_registry_sha256=namespace.approved_registry_sha256, approved_machine_receipt_sha256=namespace.approved_machine_receipt_sha256, approved_runner_executable_sha256=namespace.approved_runner_executable_sha256, native_acceptance=native, approved_native_evidence_sha256=namespace.approved_native_evidence_sha256, approved_native_executable_sha256=namespace.approved_native_executable_sha256, created_at=namespace.created_at)
            validation_kwargs = {"approved_registry_sha256": namespace.approved_registry_sha256, "machine_receipt": machine, "approved_machine_receipt_sha256": namespace.approved_machine_receipt_sha256, "approved_runner_executable_sha256": namespace.approved_runner_executable_sha256, "native_acceptance": native, "approved_native_evidence_sha256": namespace.approved_native_evidence_sha256, "approved_native_executable_sha256": namespace.approved_native_executable_sha256}
            _write_or_print(evidence, output=namespace.evidence, root=namespace.root, validation_kwargs=validation_kwargs)
            return 0 if evidence["evaluation"]["all_cases_satisfied"] is True else 1  # type: ignore[index]
        except OwnerInputRequired as error:
            evidence = scaffold_evidence(verification, created_at=namespace.created_at, reason="owner_local_revision_inputs_incomplete")
            _write_or_print(evidence, output=namespace.evidence, root=namespace.root)
            print(f"error: {error}; no endpoint was contacted", file=sys.stderr)
            return 2
        except RevisionEvaluationError as error:
            evidence = scaffold_evidence(verification, created_at=namespace.created_at, reason="owner_local_revision_inputs_invalid")
            _write_or_print(evidence, output=namespace.evidence, root=namespace.root)
            print(f"error: {error}; no endpoint was contacted", file=sys.stderr)
            return 1
    except RevisionEvaluationError as error:
        print(f"error: {error}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
