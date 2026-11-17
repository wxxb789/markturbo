"""The one evidence schema the native acceptance harnesses (goals 02/03/06/07) write.

Every goal writes the same envelope: schema, `schema_version`, status, times,
the executable binding, the environment, one record per required case and a
summary, plus the goal's own top-level keys (goal 06 `mode`, goal 07
`transport`). Validation is exact: every object has exactly its keys, an
observation is a goal-allowed key holding a scalar or one of the structured
records validated here, strings are fixed safe values, and no document,
answer or credential sentinel may appear anywhere.

A goal supplies only an `EvidenceContract`: its case flows, observation keys,
required observations, runtime-scan validator and `validate_passed_case`.

`SCHEMA_VERSION` 3 is shared by all four goals. Versions 1 (GPUI-era goals
02/03/06/07) and 2 (the first nsh goal 07) are not accepted.
"""

from __future__ import annotations

import argparse
import hashlib
import math
import re
from collections.abc import Callable, Mapping
from dataclasses import dataclass, field
from pathlib import Path
from typing import Any
from urllib.parse import urlsplit

from .runtime import (
    IMAGE_FILE_MACHINE_AMD64,
    PE32_PLUS_MAGIC,
    REASON_CODE_RE,
    SAFE_FAILURE_TYPES,
    SHA256_RE,
    HarnessFailure,
    artifact_contains,
    finite_nonnegative,
    normalize_expected_hash,
    safe_exception_name,
    utc_now,
    validate_environment as validate_runtime_environment,
    validate_fingerprint,
    validate_process_context,
)


REPO = Path(__file__).resolve().parents[3]
SCHEMA_VERSION = 3
DEFAULT_EXE = REPO / "crates" / "mt-app" / ".nsh" / "out" / "MarkTurbo" / "MarkTurbo.exe"

EVIDENCE_KEYS = frozenset(
    {
        "schema",
        "schema_version",
        "status",
        "started_at_utc",
        "completed_at_utc",
        "executable",
        "environment",
        "cases",
        "summary",
    }
)
EXECUTABLE_KEYS = frozenset(
    {
        "expected_sha256",
        "sha256",
        "byte_count",
        "hash_verified",
        "copied_sha256",
        "copy_hash_verified",
        "format",
        "machine",
        "machine_code",
        "optional_magic",
    }
)
ENVIRONMENT_KEYS = frozenset(
    {
        "platform",
        "windows_major",
        "windows_minor",
        "windows_build",
        "architecture",
        "native_machine_code",
        "python_pointer_bits",
        "wts_state",
        "active_console_session_id",
        "harness_is_console_session",
        "input_desktop",
        "thread_desktop",
        "harness_process",
    }
)
PROCESS_CONTEXT_KEYS = frozenset({"session_id", "integrity_rid", "integrity"})
CASE_KEYS = frozenset(
    {"id", "status", "duration_ms", "reason_code", "failure_type", "observations"}
)
SUMMARY_KEYS = frozenset(
    {
        "required_case_count",
        "passed_case_count",
        "blocked_case_count",
        "failed_case_count",
        "not_run_case_count",
    }
)
FINGERPRINT_KEYS = frozenset({"byte_count", "sha256"})
RUNTIME_SCAN_KEYS = frozenset({"files_scanned", "app_logs_scanned", "config_files_scanned"})
PRIVATE_KEYS = frozenset({"request_body", "credential", "api_key", "api-key"})
# Content-free diagnostics a BLOCKED case carries: `runtime.Win32.require_foreground`'s
# and `runtime.open_clipboard_with_retry`'s.
BLOCKED_DIAGNOSTIC_KEYS = frozenset(
    {
        "requested_hwnd",
        "foreground_hwnd",
        "show_window_return",
        "bring_to_top_return",
        "set_foreground_return",
        "foreground_attempts",
        "clipboard_open_attempts",
        "clipboard_owner_window_present",
    }
)
COMMON_OBSERVATION_KEYS = (
    frozenset({"flow", "process_context", "runtime_scan"}) | BLOCKED_DIAGNOSTIC_KEYS
)
COMMON_REQUIRED_OBSERVATIONS = frozenset({"flow", "runtime_scan"})


@dataclass(frozen=True)
class EvidenceContract:
    """What one goal's evidence holds beyond the shared envelope."""

    schema: str
    case_flows: Mapping[str, str]
    # Goal-specific observation keys; the common ones are always allowed.
    observation_keys: frozenset[str]
    # Observation keys whose value is a `Fingerprint.evidence()` record.
    fingerprint_keys: frozenset[str]
    # Per case, the observations a PASS needs beyond the common ones.
    required_observations: Mapping[str, frozenset[str]]
    validate_runtime_scan: Callable[[Any], None]
    # Case-specific PASS facts; common facts are checked by `validate_case`.
    validate_passed_case: Callable[[str, dict[str, Any]], None]
    # Document, answer or credential text that must never appear in evidence.
    private_sentinels: tuple[str, ...]
    safe_strings: frozenset[str] = frozenset()
    # Structured (dict or list) observations beyond the common ones.
    structured: Mapping[str, Callable[[Any], None]] = field(default_factory=dict)
    # Free strings a key may hold when they match its pattern.
    string_patterns: Mapping[str, re.Pattern[str]] = field(default_factory=dict)
    # Top-level keys beyond the shared envelope, checked by `validate_extra`.
    extra_keys: frozenset[str] = frozenset()
    validate_extra: Callable[[dict[str, Any]], None] | None = None

    @property
    def case_ids(self) -> tuple[str, ...]:
        return tuple(self.case_flows)


def new_evidence(
    contract: EvidenceContract, expected_hash: str, **extra: Any
) -> dict[str, Any]:
    if set(extra) != contract.extra_keys:
        raise ValueError("evidence extra keys do not match the contract")
    return {
        "schema": contract.schema,
        "schema_version": SCHEMA_VERSION,
        "status": "BLOCKED",
        "started_at_utc": utc_now(),
        "completed_at_utc": None,
        **extra,
        "executable": {
            "expected_sha256": expected_hash,
            "sha256": None,
            "byte_count": None,
            "hash_verified": False,
            "copied_sha256": None,
            "copy_hash_verified": False,
            "format": None,
            "machine": None,
            "machine_code": None,
            "optional_magic": None,
        },
        "environment": {},
        "cases": [
            {
                "id": case_id,
                "status": "NOT_RUN",
                "duration_ms": None,
                "reason_code": None,
                "failure_type": None,
                "observations": {},
            }
            for case_id in contract.case_ids
        ],
        "summary": {},
    }


def require_exact_keys(value: Any, expected: frozenset[str], label: str) -> None:
    if not isinstance(value, dict) or set(value) != expected:
        raise ValueError(f"invalid {label} keys")


def reject_private_content(value: Any, sentinels: tuple[str, ...]) -> None:
    """Reject content-bearing or credential-shaped values at every nesting level."""
    folded = tuple(sentinel.casefold() for sentinel in sentinels)
    if isinstance(value, dict):
        for key, nested in value.items():
            if not isinstance(key, str) or key.casefold() in PRIVATE_KEYS:
                raise ValueError("evidence contains private content")
            if any(sentinel in key.casefold() for sentinel in folded):
                raise ValueError("free-form observation strings are forbidden")
            reject_private_content(nested, sentinels)
    elif isinstance(value, list):
        for nested in value:
            reject_private_content(nested, sentinels)
    elif isinstance(value, str):
        if any(sentinel in value.casefold() for sentinel in folded):
            raise ValueError("free-form observation strings are forbidden")


def validate_context(value: Any) -> dict[str, Any]:
    require_exact_keys(value, PROCESS_CONTEXT_KEYS, "process context")
    return validate_process_context(value)


def validate_exact_fingerprint(value: Any) -> dict[str, Any]:
    require_exact_keys(value, FINGERPRINT_KEYS, "fingerprint")
    return validate_fingerprint(value)


def validate_environment(value: Any) -> dict[str, Any]:
    """A PASS environment; returns the harness process context."""
    require_exact_keys(value, ENVIRONMENT_KEYS, "environment")
    validate_runtime_environment(value)
    return validate_context(value["harness_process"])


def nonnegative_count(value: Any, key: str) -> int:
    count = value.get(key)
    if not isinstance(count, int) or isinstance(count, bool) or count < 0:
        raise ValueError(f"invalid {key}")
    return count


def validate_runtime_scan(
    value: Any, *, require_config: bool, extra_counts: frozenset[str] = frozenset()
) -> dict[str, Any]:
    """Counts of a `scan_case_artifacts` record (plus a goal's extra counts)."""
    if not isinstance(value, dict):
        raise ValueError("missing runtime scan evidence")
    require_exact_keys(value, RUNTIME_SCAN_KEYS | extra_counts, "runtime scan")
    for key in RUNTIME_SCAN_KEYS | extra_counts:
        nonnegative_count(value, key)
    files = value["files_scanned"]
    logs = value["app_logs_scanned"]
    config = value["config_files_scanned"]
    if files <= 0 or logs <= 0:
        raise ValueError("runtime scan must cover an application log")
    if require_config and config <= 0:
        raise ValueError("runtime scan must cover a configuration file")
    if files < logs + config + 1:
        raise ValueError("runtime scan file count is incomplete")
    return value


def _validate_contexts(value: Any) -> None:
    if not isinstance(value, list) or not value:
        raise ValueError("invalid process contexts")
    for context in value:
        validate_context(context)


def validate_observations(observations: Any, contract: EvidenceContract) -> None:
    if not isinstance(observations, dict):
        raise ValueError("invalid case observations")
    structured: dict[str, Callable[[Any], Any]] = {
        "process_context": validate_context,
        "process_contexts": _validate_contexts,
        "runtime_scan": contract.validate_runtime_scan,
        **{key: validate_exact_fingerprint for key in contract.fingerprint_keys},
        **contract.structured,
    }
    allowed = COMMON_OBSERVATION_KEYS | contract.observation_keys
    for key, value in observations.items():
        if not isinstance(key, str) or key not in allowed:
            raise ValueError("unknown observation field")
        if key in structured:
            structured[key](value)
        elif isinstance(value, str):
            pattern = contract.string_patterns.get(key)
            if value not in contract.safe_strings and not (
                pattern is not None and pattern.fullmatch(value)
            ):
                raise ValueError("free-form observation strings are forbidden")
        elif isinstance(value, (dict, list)):
            raise ValueError("invalid nested observation structure")
        elif value is not None and not isinstance(value, (bool, int, float)):
            raise ValueError("invalid observation value")


def validate_case(
    contract: EvidenceContract,
    case_id: str,
    observations: dict[str, Any],
    parent: dict[str, Any],
) -> None:
    """The facts a PASS case must prove, common and case-specific."""
    validate_observations(observations, contract)
    required = COMMON_REQUIRED_OBSERVATIONS | contract.required_observations[case_id]
    if not required.issubset(observations):
        raise ValueError("passed case evidence is incomplete")
    if observations.get("flow") != contract.case_flows[case_id]:
        raise ValueError("case flow mechanics do not match the required scenario")
    contexts = observations.get("process_contexts", [])
    if not isinstance(contexts, list):
        raise ValueError("passed case requires process context")
    if "process_context" in observations:
        contexts = [*contexts, observations["process_context"]]
    if not contexts:
        raise ValueError("passed case requires process context")
    for context in contexts:
        if validate_context(context) != parent:
            raise ValueError("case process context differs from harness context")
    contract.validate_passed_case(case_id, observations)


def _validate_executable(executable: Any, passed: bool) -> None:
    if not isinstance(executable, dict):
        raise ValueError("missing executable evidence")
    require_exact_keys(executable, EXECUTABLE_KEYS, "executable")
    expected = executable["expected_sha256"]
    if not isinstance(expected, str) or not SHA256_RE.fullmatch(expected):
        raise ValueError("invalid expected executable SHA-256")
    for key in ("sha256", "copied_sha256"):
        value = executable[key]
        if value is not None and (not isinstance(value, str) or not SHA256_RE.fullmatch(value)):
            raise ValueError(f"invalid executable {key}")
    byte_count = executable["byte_count"]
    if byte_count is not None and (
        not isinstance(byte_count, int) or isinstance(byte_count, bool) or byte_count < 0
    ):
        raise ValueError("invalid executable byte count")
    if executable["hash_verified"] is True and executable["sha256"] != expected:
        raise ValueError("verified executable hash does not match expected hash")
    if executable["copy_hash_verified"] is True and executable["copied_sha256"] != expected:
        raise ValueError("verified copied executable hash does not match expected hash")
    if not passed:
        return
    if executable["hash_verified"] is not True or executable["copy_hash_verified"] is not True:
        raise ValueError("PASS requires verified executable hashes")
    if (
        executable["format"] != "PE32+"
        or executable["machine"] != "x86_64"
        or executable["machine_code"] != IMAGE_FILE_MACHINE_AMD64
        or executable["optional_magic"] != PE32_PLUS_MAGIC
    ):
        raise ValueError("PASS requires an AMD64 PE32+ executable")
    if not byte_count:
        raise ValueError("PASS requires a nonempty executable")


def validate_evidence(evidence: Any, contract: EvidenceContract) -> None:
    if not isinstance(evidence, dict):
        raise ValueError("invalid top-level evidence")
    require_exact_keys(evidence, EVIDENCE_KEYS | contract.extra_keys, "top-level evidence")
    version = evidence["schema_version"]
    if evidence["schema"] != contract.schema or type(version) is not int or version != SCHEMA_VERSION:
        raise ValueError("unsupported evidence schema")
    status = evidence["status"]
    if status not in {"PASS", "FAIL", "BLOCKED"}:
        raise ValueError("invalid evidence status")
    reject_private_content(evidence, contract.private_sentinels)
    if contract.validate_extra is not None:
        contract.validate_extra(evidence)
    _validate_executable(evidence["executable"], status == "PASS")

    cases = evidence["cases"]
    case_ids = contract.case_ids
    if not isinstance(cases, list) or len(cases) != len(case_ids):
        raise ValueError("required case set is incomplete")
    for case in cases:
        require_exact_keys(case, CASE_KEYS, "case")
    ids = [case["id"] for case in cases]
    if len(set(ids)) != len(ids) or set(ids) != set(case_ids):
        raise ValueError("required case set is incomplete")
    environment = evidence["environment"]
    parent = None
    if status == "PASS" or any(case["status"] == "PASS" for case in cases):
        parent = validate_environment(environment)
    elif environment != {}:
        require_exact_keys(environment, ENVIRONMENT_KEYS, "environment")
    for case in cases:
        case_status = case["status"]
        if case_status not in {"PASS", "FAIL", "BLOCKED", "NOT_RUN"}:
            raise ValueError("invalid case status")
        reason = case["reason_code"]
        if reason is not None and (not isinstance(reason, str) or not REASON_CODE_RE.fullmatch(reason)):
            raise ValueError("invalid case reason code")
        failure_type = case["failure_type"]
        if failure_type is not None and (
            not isinstance(failure_type, str) or failure_type not in SAFE_FAILURE_TYPES
        ):
            raise ValueError("invalid case failure type")
        if case_status != "FAIL" and failure_type is not None:
            raise ValueError("only failed cases may contain a failure type")
        duration = case["duration_ms"]
        if case_status == "NOT_RUN":
            if duration is not None:
                raise ValueError("NOT_RUN case cannot have a duration")
        elif not finite_nonnegative(duration):
            raise ValueError("completed case duration must be nonnegative")
        if case_status == "PASS":
            validate_case(contract, case["id"], case["observations"], parent)
        else:
            validate_observations(case["observations"], contract)
    if status == "PASS" and any(case["status"] != "PASS" for case in cases):
        raise ValueError("PASS requires every required case to pass")
    require_exact_keys(evidence["summary"], SUMMARY_KEYS, "case summary")
    expected_summary = {
        "required_case_count": len(case_ids),
        "passed_case_count": sum(case["status"] == "PASS" for case in cases),
        "blocked_case_count": sum(case["status"] == "BLOCKED" for case in cases),
        "failed_case_count": sum(case["status"] == "FAIL" for case in cases),
        "not_run_case_count": sum(case["status"] == "NOT_RUN" for case in cases),
    }
    if evidence["summary"] != expected_summary:
        raise ValueError("case summary does not match case evidence")


def leak_codes(name: str, value: str) -> dict[bytes, str]:
    """A sentinel's UTF-8 and UTF-16LE forms, each with its leak failure code."""
    return {
        value.encode("utf-8"): f"UTF8_{name}_LEAKED",
        value.encode("utf-16-le"): f"UTF16LE_{name}_LEAKED",
    }


def scan_case_artifacts(
    case_root: Path, codes: Mapping[bytes, str], *, require_config: bool = False
) -> dict[str, int]:
    """Scan a case's data, config and stderr for every private byte pattern.

    A pattern found fails the case with its code from `codes` (in order); the
    record exists only for a scan that found none.
    """
    try:
        roots = (case_root / "data", case_root / "config")
        paths = [path for root in roots for path in root.rglob("*") if path.is_file()]
        stderr = case_root / "stderr.log"
        if stderr.exists():
            paths.append(stderr)
        paths = sorted(set(paths))
    except OSError as error:
        raise HarnessFailure("RUNTIME_ARTIFACT_ENUM_FAILED", safe_exception_name(error)) from None
    if not paths:
        raise HarnessFailure("RUNTIME_ARTIFACTS_MISSING")
    logs_root = case_root / "data" / "logs"
    app_logs = [path for path in paths if logs_root in path.parents]
    if not app_logs:
        raise HarnessFailure("APP_LOG_MISSING")
    patterns = tuple(codes)
    for path in paths:
        try:
            leaked = artifact_contains(path, patterns)
        except OSError as error:
            raise HarnessFailure("RUNTIME_ARTIFACT_SCAN_FAILED", safe_exception_name(error)) from None
        if leaked is not None:
            raise HarnessFailure(codes[leaked])
    config_root = case_root / "config"
    config_files = [path for path in paths if config_root in path.parents]
    if require_config and not config_files:
        raise HarnessFailure("CONFIG_FILE_MISSING")
    if len(paths) < len(app_logs) + len(config_files) + 1:
        raise HarnessFailure("RUNTIME_FILE_COUNT_INCOMPLETE")
    return {
        "files_scanned": len(paths),
        "app_logs_scanned": len(app_logs),
        "config_files_scanned": len(config_files),
    }


def credential_target(endpoint: str, wire: str) -> str:
    """Mirror mt-core `EndpointIdentity::credential_target` for `wire`.

    The non-secret settings identity of a model credential; it never holds
    the credential.
    """
    parsed = urlsplit(endpoint)
    scheme = parsed.scheme.casefold()
    host = parsed.hostname
    if (
        scheme not in {"http", "https"}
        or not host
        or parsed.username is not None
        or parsed.password is not None
        or parsed.query
        or parsed.fragment
    ):
        raise ValueError("endpoint identity requires an HTTP(S) host")
    try:
        port = parsed.port or (80 if scheme == "http" else 443)
    except ValueError as error:
        raise ValueError("endpoint identity has an invalid port") from error
    path = parsed.path or "/"
    if not path.endswith("/"):
        path += "/"
    digest = hashlib.sha256()
    for component in ("io.github.wxxb789.markturbo", wire, scheme, host.casefold(), str(port), path):
        digest.update(component.encode("utf-8"))
        digest.update(b"\0")
    return (
        f"io.github.wxxb789.markturbo:model-credential:v2|wire={wire}"
        f"|host={host.casefold()}|identity-sha256={digest.hexdigest()}"
    )


def ui_timeout_seconds(value: str) -> float:
    timeout = float(value)
    if not math.isfinite(timeout) or timeout <= 0:
        raise argparse.ArgumentTypeError("--ui-timeout must be greater than zero")
    return timeout


def build_parser(description: str, goal: str, case_ids: tuple[str, ...]) -> argparse.ArgumentParser:
    """The arguments every native acceptance harness takes."""
    parser = argparse.ArgumentParser(description=description)
    parser.add_argument(
        "--exe",
        type=Path,
        default=DEFAULT_EXE,
        help="release executable to copy and test (default: the packaged MarkTurbo.exe)",
    )
    parser.add_argument(
        "--expect-exe-sha256",
        type=normalize_expected_hash,
        required=True,
        help="required SHA-256 binding for the release executable",
    )
    parser.add_argument(
        "--evidence",
        type=Path,
        default=REPO
        / "target"
        / "native-acceptance"
        / f"{goal}-native-acceptance.json",
        help="JSON run receipt path (default: target/native-acceptance)",
    )
    parser.add_argument(
        "--ui-timeout",
        type=ui_timeout_seconds,
        default=15.0,
        help="seconds allowed for ordinary UI state transitions (default: 15)",
    )
    parser.add_argument(
        "--case",
        choices=case_ids,
        help="run one case for debugging; it never produces acceptance PASS",
    )
    parser.add_argument(
        "--keep-going",
        action="store_true",
        help="after a case FAIL, still run the later cases (the run stays FAIL); BLOCKED stops",
    )
    parser.add_argument(
        "--keep-workdir-on-failure",
        action="store_true",
        help="preserve the isolated data, config, logs, and stderr directory after a non-PASS run",
    )
    return parser
