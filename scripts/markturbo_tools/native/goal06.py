"""Exercise Goal 06 Review through the Windows UI.

The default harness uses a fresh custom HTTPS endpoint and no credential. It
never sends document content, and proves the explicit local missing-credential
diagnostic. The explicit configured-provider mode sends only the fixed test
document to an already-running loopback provider using a fresh process-only
credential. Both modes bind PASS evidence to an executable hash and prove the
editor and source file stayed byte-identical.

On the nsh UI the document Review is Ctrl+Enter from the editor; the consent
card authorizes only a real pointer hold on `consent-send` (a UIA Invoke is a
tap and sends nothing). The selection Review needs a selection command the
UI must expose as the title-menu row `title-menu-review-selection`.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import math
import uuid
from pathlib import Path
from typing import Any

from .source_contract import production_markers_present
from .runtime import (
    IMAGE_FILE_MACHINE_AMD64,
    INTEGRITY_NAMES,
    PE32_PLUS_MAGIC,
    REASON_CODE_RE,
    SAFE_FAILURE_TYPES,
    SHA256_RE,
    VK_A,
    VK_RETURN,
    HarnessBlocked,
    HarnessFailure,
    NativeHarness as BaseNativeHarness,
    NativeRunPlan,
    artifact_contains,
    complete_evidence as complete_evidence_envelope,
    finite_nonnegative,
    load_pywinauto,
    main_native_acceptance,
    normalize_expected_hash,
    preflight,
    require_true,
    safe_exception_name,
    sha256_file,
    utc_now,
    validate_environment as validate_runtime_environment,
    validate_fingerprint,
    validate_process_context,
    write_durable,
)


REPO = Path(__file__).resolve().parents[3]
DEFAULT_EXE = REPO / "target" / "release" / "markturbo.exe"
DEFAULT_EVIDENCE = REPO / "target" / "native-acceptance" / "goal-06-native-acceptance-v1.json"

SCHEMA = "markturbo.goal-06-native-acceptance"
SCHEMA_VERSION = 1
# `ui/review.rs` `unavailable`: the explicit local diagnostic, an anonymous Text.
MISSING_CREDENTIAL_MESSAGE = "No model set up"
# `ui/review.rs` `intent`: the first line of a structured result.
REVIEW_RESULT_MARKER = "Understood:"
# `ui/consent.rs` `send_button`: the hold control carries its own test id.
CONSENT_SEND_AUTOMATION_ID = "consent-send"
# `ui/consent.rs` `scope_row`: the scope word a selection binding shows.
CONSENT_SELECTION_SCOPE = "Selection"
# Longer than the control's 1.2s hold (`HOLD_MS`), so a send completes.
CONSENT_HOLD_SECONDS = 1.8
# Not in the nsh UI yet: the selection Review command this harness requires.
REVIEW_SELECTION_MENU_AUTOMATION_ID = "title-menu-review-selection"
UI_SOURCE_REQUIREMENTS = {
    "crates/mt-app/src/ui/titlebar.rs": ('"mod+enter"', '"title-settings"'),
    "crates/mt-app/src/ui/review.rs": ('"No model set up"', '"Understood:"'),
    "crates/mt-app/src/ui/consent.rs": (
        '"consent-send"',
        '"Selection"',
        ".long_press_delay(HOLD_MS)",
        ".on_press_down(",
    ),
    "crates/mt-app/src/ui/document.rs": ('"Markdown source"',),
}

MISSING_CREDENTIAL_MODE = "missing_credential_diagnostic"
CONFIGURED_PROVIDER_MODE = "configured_provider_success"
CONFIGURED_PROVIDER_ENDPOINT = "http://127.0.0.1:4141/v1/"

CASE_DOCUMENT = "configured_provider_missing_credential_document"
CASE_SELECTION = "configured_provider_missing_credential_selection"
REQUIRED_CASE_IDS = (CASE_DOCUMENT, CASE_SELECTION)
CASE_FLOWS = {
    CASE_DOCUMENT: (
        "configured provider without credential -> Ctrl+Enter document review -> "
        "explicit diagnostic"
    ),
    CASE_SELECTION: (
        "configured provider without credential -> menu selection review -> "
        "explicit diagnostic"
    ),
}
CONFIGURED_PROVIDER_CASE_FLOWS = {
    CASE_DOCUMENT: (
        "configured provider with ephemeral credential -> Ctrl+Enter document review -> "
        "hold-to-send consent -> structured success"
    ),
    CASE_SELECTION: (
        "configured provider with ephemeral credential -> menu selection review -> "
        "selection-scope hold-to-send consent -> structured success"
    ),
}

DOCUMENT_SENTINEL = "MTG06-NATIVE-READ-ONLY-SENTINEL"
DOCUMENT_TEXT = (
    f"# {DOCUMENT_SENTINEL}\n\n"
    "Review must preserve this source exactly, including this line.\n"
)

SAFE_STRINGS = (
    frozenset(CASE_FLOWS.values())
    | frozenset(CONFIGURED_PROVIDER_CASE_FLOWS.values())
    | INTEGRITY_NAMES
)
ALLOWED_OBSERVATION_KEYS = {
    "app_logs_scanned",
    "byte_count",
    "bring_to_top_return",
    "config_files_scanned",
    "document_shortcut",
    "ephemeral_credential_utf16le_absent",
    "ephemeral_credential_utf8_absent",
    "editor_after",
    "editor_before",
    "files_scanned",
    "flow",
    "foreground_attempts",
    "foreground_hwnd",
    "foreground_verified",
    "integrity",
    "integrity_rid",
    "missing_credential_diagnostic",
    "no_dirty_interlock",
    "process_context",
    "runtime_scan",
    "selection_shortcut",
    "set_foreground_return",
    "session_id",
    "sha256",
    "show_window_return",
    "source_after",
    "source_before",
    "requested_hwnd",
    "review_consent_approved",
    "structured_success",
    "utf16le_sentinel_absent",
    "utf8_sentinel_absent",
}


def configured_provider_environment_key_identity() -> str:
    """Return the non-secret settings identity for the fixed local endpoint."""
    digest = hashlib.sha256()
    for component in (
        "io.github.wxxb789.markturbo",
        "openai-responses",
        "http",
        "127.0.0.1",
        "4141",
        "/v1/",
    ):
        digest.update(component.encode("utf-8"))
        digest.update(b"\0")
    return (
        "io.github.wxxb789.markturbo:model-credential:v2|wire=openai-responses"
        f"|host=127.0.0.1|identity-sha256={digest.hexdigest()}"
    )


def review_settings_document(
    endpoint: str, environment_key_identity: str | None = None
) -> bytes:
    """Build the isolated non-secret Review profile."""
    document = (
        "show-welcome-on-startup = false\n"
        'model-provider = "openai-responses"\n'
        'model-name = "gpt-5.6-terra"\n'
        f"model-base-url = {json.dumps(endpoint)}\n"
    )
    if environment_key_identity is not None:
        document += f"model-environment-key-identity = {json.dumps(environment_key_identity)}\n"
    return document.encode("utf-8")


def new_evidence(expected_hash: str, configured_provider: bool = False) -> dict[str, Any]:
    return {
        "schema": SCHEMA,
        "schema_version": SCHEMA_VERSION,
        "mode": CONFIGURED_PROVIDER_MODE if configured_provider else MISSING_CREDENTIAL_MODE,
        "status": "BLOCKED",
        "started_at_utc": utc_now(),
        "completed_at_utc": None,
        "executable": {
            "expected_sha256": expected_hash,
            "sha256": None,
            "byte_count": None,
            "hash_verified": False,
            "copied_sha256": None,
            "copy_hash_verified": False,
            "format": None,
            "machine": None,
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
            for case_id in REQUIRED_CASE_IDS
        ],
        "summary": {},
    }


def complete_evidence(evidence: dict[str, Any], status: str) -> None:
    complete_evidence_envelope(evidence, status, REQUIRED_CASE_IDS)


def validate_observations(value: Any, key: str | None = None) -> None:
    if isinstance(value, dict):
        for nested_key, nested in value.items():
            if nested_key not in ALLOWED_OBSERVATION_KEYS:
                raise ValueError("unknown observation field")
            if nested_key == "sha256" and (
                not isinstance(nested, str) or not SHA256_RE.fullmatch(nested)
            ):
                raise ValueError("invalid observation SHA-256")
            validate_observations(nested, nested_key)
    elif isinstance(value, list):
        for nested in value:
            validate_observations(nested, key)
    elif isinstance(value, str):
        if key != "sha256" and value not in SAFE_STRINGS:
            raise ValueError("free-form observation strings are forbidden")
    elif value is not None and not isinstance(value, (bool, int, float)):
        raise ValueError("invalid observation value")


def validate_runtime_scan(value: Any, configured_provider: bool) -> None:
    if not isinstance(value, dict):
        raise ValueError("missing runtime scan evidence")
    for key in ("files_scanned", "app_logs_scanned", "config_files_scanned"):
        if (
            not isinstance(value.get(key), int)
            or isinstance(value[key], bool)
            or value[key] < 0
        ):
            raise ValueError(f"invalid {key}")
    if value["files_scanned"] <= 0 or value["app_logs_scanned"] <= 0:
        raise ValueError("runtime scan must cover an application log")
    require_true(value, "utf8_sentinel_absent", "utf16le_sentinel_absent")
    if configured_provider:
        require_true(
            value,
            "ephemeral_credential_utf8_absent",
            "ephemeral_credential_utf16le_absent",
        )


def validate_environment(value: Any) -> dict[str, Any]:
    if not isinstance(value, dict):
        raise ValueError("invalid environment evidence")
    validate_runtime_environment(value)
    return validate_process_context(value.get("harness_process"))


def required_observations(case_id: str, configured_provider: bool) -> set[str]:
    common = {
        "editor_before",
        "editor_after",
        "source_before",
        "source_after",
        "no_dirty_interlock",
        "flow",
        "process_context",
        "foreground_verified",
        "runtime_scan",
    }
    if configured_provider:
        common |= {"review_consent_approved", "structured_success"}
    else:
        common.add("missing_credential_diagnostic")
    shortcut = "document_shortcut" if case_id == CASE_DOCUMENT else "selection_shortcut"
    return common | {shortcut}


def validate_passed_case(
    case_id: str,
    observations: dict[str, Any],
    parent: dict[str, Any],
    configured_provider: bool,
) -> None:
    flows = CONFIGURED_PROVIDER_CASE_FLOWS if configured_provider else CASE_FLOWS
    if observations.get("flow") != flows[case_id]:
        raise ValueError("case flow mechanics do not match the required scenario")
    if validate_process_context(observations.get("process_context")) != parent:
        raise ValueError("case process context differs from harness context")
    require_true(
        observations,
        "foreground_verified",
        "no_dirty_interlock",
        "document_shortcut" if case_id == CASE_DOCUMENT else "selection_shortcut",
    )
    if configured_provider:
        require_true(observations, "review_consent_approved", "structured_success")
    else:
        require_true(observations, "missing_credential_diagnostic")
    validate_runtime_scan(observations.get("runtime_scan"), configured_provider)
    editor_before = validate_fingerprint(observations.get("editor_before"))
    editor_after = validate_fingerprint(observations.get("editor_after"))
    source_before = validate_fingerprint(observations.get("source_before"))
    source_after = validate_fingerprint(observations.get("source_after"))
    if editor_before != editor_after:
        raise ValueError("Review changed editor text")
    if source_before != source_after:
        raise ValueError("Review changed source text")
    if editor_before != source_before:
        raise ValueError("editor and source text differ before Review")


def validate_evidence(evidence: dict[str, Any]) -> None:
    if evidence.get("schema") != SCHEMA or evidence.get("schema_version") != SCHEMA_VERSION:
        raise ValueError("unsupported evidence schema")
    if evidence.get("status") not in {"PASS", "FAIL", "BLOCKED"}:
        raise ValueError("invalid evidence status")
    mode = evidence.get("mode")
    if mode not in {MISSING_CREDENTIAL_MODE, CONFIGURED_PROVIDER_MODE}:
        raise ValueError("invalid native Review mode")
    configured_provider = mode == CONFIGURED_PROVIDER_MODE
    executable = evidence.get("executable")
    if not isinstance(executable, dict):
        raise ValueError("missing executable evidence")
    expected = executable.get("expected_sha256")
    if not isinstance(expected, str) or not SHA256_RE.fullmatch(expected):
        raise ValueError("invalid expected executable SHA-256")
    actual = executable.get("sha256")
    if actual is not None and (not isinstance(actual, str) or not SHA256_RE.fullmatch(actual)):
        raise ValueError("invalid executable SHA-256")
    copied = executable.get("copied_sha256")
    if copied is not None and (not isinstance(copied, str) or not SHA256_RE.fullmatch(copied)):
        raise ValueError("invalid copied executable SHA-256")
    if executable.get("hash_verified") is True and actual != expected:
        raise ValueError("verified executable hash does not match expected hash")
    if executable.get("copy_hash_verified") is True and copied != expected:
        raise ValueError("verified copied executable hash does not match expected hash")
    if evidence["status"] == "PASS":
        if not executable.get("hash_verified") or not executable.get("copy_hash_verified"):
            raise ValueError("PASS requires verified executable hashes")
        if executable.get("format") != "PE32+" or executable.get("machine") != "x86_64":
            raise ValueError("PASS requires an AMD64 PE32+ executable")
        if executable.get("machine_code") != IMAGE_FILE_MACHINE_AMD64 or executable.get("optional_magic") != PE32_PLUS_MAGIC:
            raise ValueError("PASS requires an AMD64 PE32+ executable")
        if (
            not isinstance(executable.get("byte_count"), int)
            or isinstance(executable["byte_count"], bool)
            or executable["byte_count"] <= 0
        ):
            raise ValueError("PASS requires a nonempty executable")

    cases = evidence.get("cases")
    if not isinstance(cases, list) or len(cases) != len(REQUIRED_CASE_IDS):
        raise ValueError("required case set is incomplete")
    ids = [case.get("id") for case in cases if isinstance(case, dict)]
    if len(set(ids)) != len(ids) or set(ids) != set(REQUIRED_CASE_IDS):
        raise ValueError("required case set is incomplete")
    parent = None
    if evidence["status"] == "PASS" or any(
        isinstance(case, dict) and case.get("status") == "PASS" for case in cases
    ):
        parent = validate_environment(evidence.get("environment"))
    for case in cases:
        status = case.get("status")
        if status not in {"PASS", "FAIL", "BLOCKED", "NOT_RUN"}:
            raise ValueError("invalid case status")
        reason = case.get("reason_code")
        if reason is not None and (
            not isinstance(reason, str) or not REASON_CODE_RE.fullmatch(reason)
        ):
            raise ValueError("invalid case reason code")
        failure_type = case.get("failure_type")
        if failure_type is not None and (
            not isinstance(failure_type, str) or failure_type not in SAFE_FAILURE_TYPES
        ):
            raise ValueError("invalid case failure type")
        if status != "FAIL" and failure_type is not None:
            raise ValueError("only failed cases may contain a failure type")
        duration = case.get("duration_ms")
        if status == "NOT_RUN":
            if duration is not None:
                raise ValueError("NOT_RUN case cannot have a duration")
        elif not finite_nonnegative(duration):
            raise ValueError("completed case duration must be nonnegative")
        observations = case.get("observations")
        if not isinstance(observations, dict):
            raise ValueError("invalid case observations")
        validate_observations(observations)
        if status == "PASS":
            if not required_observations(case["id"], configured_provider).issubset(observations):
                raise ValueError("passed case evidence is incomplete")
            if parent is None:
                raise ValueError("passed case requires validated environment evidence")
            validate_passed_case(case["id"], observations, parent, configured_provider)
    if evidence["status"] == "PASS" and any(case["status"] != "PASS" for case in cases):
        raise ValueError("PASS requires every required case to pass")
    summary = evidence.get("summary")
    expected_summary = {
        "required_case_count": len(REQUIRED_CASE_IDS),
        "passed_case_count": sum(case["status"] == "PASS" for case in cases),
        "blocked_case_count": sum(case["status"] == "BLOCKED" for case in cases),
        "failed_case_count": sum(case["status"] == "FAIL" for case in cases),
        "not_run_case_count": sum(case["status"] == "NOT_RUN" for case in cases),
    }
    if summary != expected_summary:
        raise ValueError("case summary does not match case evidence")


def source_contract_failure() -> str | None:
    """The nsh selectors this harness drives must exist in production source."""
    if not production_markers_present(REPO, UI_SOURCE_REQUIREMENTS):
        return "REVIEW_SOURCE_CONTRACT_MISSING"
    return None


def scan_case_artifacts(case_root: Path, ephemeral_credential: str | None = None) -> dict[str, Any]:
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
    utf8 = DOCUMENT_SENTINEL.encode("utf-8")
    utf16 = DOCUMENT_SENTINEL.encode("utf-16-le")
    credential_utf8 = (
        ephemeral_credential.encode("utf-8") if ephemeral_credential is not None else None
    )
    credential_utf16 = (
        ephemeral_credential.encode("utf-16-le") if ephemeral_credential is not None else None
    )
    for path in paths:
        try:
            patterns = (utf8, utf16)
            if credential_utf8 is not None and credential_utf16 is not None:
                patterns += (credential_utf8, credential_utf16)
            leaked = artifact_contains(path, patterns)
        except OSError as error:
            raise HarnessFailure("RUNTIME_ARTIFACT_SCAN_FAILED", safe_exception_name(error)) from None
        if leaked == utf8:
            raise HarnessFailure("UTF8_DOCUMENT_SENTINEL_LEAKED")
        if leaked == utf16:
            raise HarnessFailure("UTF16LE_DOCUMENT_SENTINEL_LEAKED")
        if credential_utf8 is not None and leaked == credential_utf8:
            raise HarnessFailure("UTF8_EPHEMERAL_CREDENTIAL_LEAKED")
        if credential_utf16 is not None and leaked == credential_utf16:
            raise HarnessFailure("UTF16LE_EPHEMERAL_CREDENTIAL_LEAKED")
    config_root = case_root / "config"
    scan = {
        "files_scanned": len(paths),
        "app_logs_scanned": len(app_logs),
        "config_files_scanned": sum(config_root in path.parents for path in paths),
        "utf8_sentinel_absent": True,
        "utf16le_sentinel_absent": True,
    }
    if ephemeral_credential is not None:
        scan.update(
            ephemeral_credential_utf8_absent=True,
            ephemeral_credential_utf16le_absent=True,
        )
    return scan


def goal06_preflight(configured_provider: bool):
    """Return the mode-specific preflight before any child process is created."""

    def verify(exe: Path, expected_hash: str, evidence: dict[str, Any]) -> tuple[Any, Any]:
        win32, parent_context = preflight(exe, expected_hash, evidence)
        if configured_provider and win32.persistent_credential_target_exists(
            configured_provider_environment_key_identity()
        ):
            raise HarnessBlocked("PERSISTENT_CREDENTIAL_PRESENT")
        return win32, parent_context

    return verify


class Goal06Harness(BaseNativeHarness):
    def __init__(self, *args: Any, configured_provider: bool = False) -> None:
        super().__init__(*args)
        self.configured_provider = configured_provider

    def openai_api_key_for_child(self) -> str | None:
        if not self.configured_provider:
            return None
        return f"markturbo-goal06-{uuid.uuid4().hex}"

    def profile(self, case_id: str) -> tuple[Path, Path, Path, Path, Path, Path]:
        data_root, config_root, workspace_root, stderr_path = self.case_roots(case_id)
        case_root = data_root.parent
        source = (workspace_root / "review.md").resolve()
        endpoint = (
            CONFIGURED_PROVIDER_ENDPOINT
            if self.configured_provider
            else f"https://review-{uuid.uuid4().hex}.invalid/v1/"
        )
        environment_key_identity = (
            configured_provider_environment_key_identity()
            if self.configured_provider
            else None
        )
        write_durable(source, DOCUMENT_TEXT.encode("utf-8"))
        write_durable(
            config_root / "settings.toml",
            review_settings_document(endpoint, environment_key_identity),
        )
        return case_root, data_root, config_root, workspace_root, stderr_path, source

    def request_review(self, app: Any, target: str) -> None:
        """Document: Ctrl+Enter from the editor. Selection: select all, then
        the title menu's selection Review row."""
        self.focus_editor(app)
        if target == CASE_SELECTION:
            self.win32.send_shortcut(app.hwnd, VK_A)
            self.menu_command(app, REVIEW_SELECTION_MENU_AUTOMATION_ID)
            return
        self.win32.send_shortcut(app.hwnd, VK_RETURN)

    def approve_review_consent(self, app: Any, target: str) -> None:
        send = self.find_control(
            app,
            CONSENT_SEND_AUTOMATION_ID,
            "Button",
            "REVIEW_CONSENT_UIA_TIMEOUT",
            "REVIEW_CONSENT_UIA_CONTRACT_MISMATCH",
        )
        if target == CASE_SELECTION and not self.name_present(
            app, CONSENT_SELECTION_SCOPE, "Text"
        ):
            raise HarnessFailure("REVIEW_CONSENT_SELECTION_SCOPE_MISSING")
        self.hold_control(app, send, CONSENT_HOLD_SECONDS, "REVIEW_CONSENT_HOLD_FAILED")

    def require_missing_credential_diagnostic(self, app: Any) -> None:
        self.wait_name(app, MISSING_CREDENTIAL_MESSAGE, "Text", "REVIEW_DIAGNOSTIC_UIA_TIMEOUT")

    def require_structured_success_result(self, app: Any) -> None:
        self.wait_name(
            app, REVIEW_RESULT_MARKER, "Text", "REVIEW_RESULT_UIA_TIMEOUT", max(self.ui_timeout, 60.0)
        )

    def scenario(self, case_id: str) -> dict[str, Any]:
        case_root, data, config, workspace, stderr, source = self.profile(case_id)
        app = self.launch_app(source, data, config, workspace, stderr)
        try:
            self.activate_source_layout(app)
            editor_before = self.editor_fingerprint(app)
            source_before = sha256_file(source)
            if editor_before != source_before:
                raise HarnessFailure("INITIAL_EDITOR_SOURCE_MISMATCH")
            self.request_review(app, case_id)
            if self.configured_provider:
                self.approve_review_consent(app, case_id)
                self.require_structured_success_result(app)
            else:
                self.require_missing_credential_diagnostic(app)
            editor_after = self.editor_fingerprint(app)
            source_after = sha256_file(source)
            if editor_after != editor_before:
                raise HarnessFailure("REVIEW_CHANGED_EDITOR")
            if source_after != source_before:
                raise HarnessFailure("REVIEW_CHANGED_SOURCE")
            if self.document_dirty(app):
                raise HarnessFailure("REVIEW_MARKED_DOCUMENT_DIRTY")
            self.close_app(app)
            observations = {
                "editor_before": editor_before.evidence(),
                "editor_after": editor_after.evidence(),
                "source_before": source_before.evidence(),
                "source_after": source_after.evidence(),
                "no_dirty_interlock": True,
                "flow": (
                    CONFIGURED_PROVIDER_CASE_FLOWS[case_id]
                    if self.configured_provider
                    else CASE_FLOWS[case_id]
                ),
                "process_context": app.security_context.evidence(),
                "foreground_verified": True,
                "document_shortcut": case_id == CASE_DOCUMENT,
                "selection_shortcut": case_id == CASE_SELECTION,
            }
            if self.configured_provider:
                observations.update(review_consent_approved=True, structured_success=True)
            else:
                observations["missing_credential_diagnostic"] = True
        finally:
            self.reap(app)
        observations["runtime_scan"] = scan_case_artifacts(
            case_root,
            app.spec.env.get("OPENAI_API_KEY") if self.configured_provider else None,
        )
        return observations


def native_run_plan(configured_provider: bool = False) -> NativeRunPlan:
    return NativeRunPlan(
        required_case_ids=REQUIRED_CASE_IDS,
        workdir_prefix="markturbo-goal-06-native-",
        new_evidence=lambda expected_hash: new_evidence(expected_hash, configured_provider),
        validate_evidence=validate_evidence,
        source_contract=source_contract_failure,
        preflight=goal06_preflight(configured_provider),
        ui_types_loader=load_pywinauto,
        harness_factory=lambda *args: Goal06Harness(
            *args, configured_provider=configured_provider
        ),
        scenarios=lambda harness: (
            lambda: harness.scenario(CASE_DOCUMENT),
            lambda: harness.scenario(CASE_SELECTION),
        ),
    )


def parse_args(argv: list[str] | None = None) -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--exe", type=Path, default=DEFAULT_EXE)
    parser.add_argument("--expect-exe-sha256", type=normalize_expected_hash, required=True)
    parser.add_argument("--evidence", type=Path, default=DEFAULT_EVIDENCE)
    parser.add_argument("--ui-timeout", type=float, default=15.0)
    parser.add_argument(
        "--case",
        choices=REQUIRED_CASE_IDS,
        help="run one case for debugging; it never produces acceptance PASS",
    )
    parser.add_argument(
        "--configured-provider",
        action="store_true",
        help=(
            "send the fixed test document to an already-running local provider at "
            "http://127.0.0.1:4141/v1/ using a fresh process-only credential"
        ),
    )
    parser.add_argument(
        "--keep-workdir-on-failure",
        action="store_true",
        help="preserve the isolated data, config, logs, and stderr directory after a non-PASS run",
    )
    args = parser.parse_args(argv)
    if not math.isfinite(args.ui_timeout) or args.ui_timeout <= 0:
        parser.error("--ui-timeout must be greater than zero")
    return args


def main(argv: list[str] | None = None) -> int:
    args = parse_args(argv)
    return main_native_acceptance(args, native_run_plan(args.configured_provider))


if __name__ == "__main__":
    raise SystemExit(main())
