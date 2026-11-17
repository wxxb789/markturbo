"""Exercise Goal 06 Review through the Windows UI.

The default harness uses a fresh custom HTTPS endpoint and no credential. It
never sends document content, and proves the explicit local missing-credential
diagnostic. The explicit configured-provider mode sends only the fixed test
document to an already-running loopback provider using a fresh process-only
credential. Both modes bind PASS evidence to an executable hash and prove the
editor and source file stayed byte-identical.

On the nsh UI the document Review is Ctrl+Enter from the editor; the consent
card authorizes only a real pointer hold on `consent-send` (a UIA Invoke is a
tap and sends nothing). The selection Review selects the source with the
editor's Select All, then runs the title menu's `Review selection` row
(`title-menu-review-selection`), which reviews the mounted source editor's
live selection; a configured provider's consent card then shows the
`Selection` scope.
"""

from __future__ import annotations

import argparse
import json
import uuid
from pathlib import Path
from typing import Any

from . import evidence
from .evidence import credential_target, leak_codes
from .runtime import (
    INTEGRITY_NAMES,
    VK_A,
    VK_RETURN,
    HarnessBlocked,
    HarnessFailure,
    NativeHarness as BaseNativeHarness,
    NativeRunPlan,
    load_pywinauto,
    main_native_acceptance,
    preflight,
    require_true,
    sha256_file,
    validate_fingerprint,
    write_durable,
)


SCHEMA = "markturbo.goal-06-native-acceptance"
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
# `ui/titlebar.rs`: the settings-glyph menu's selection Review row.
REVIEW_SELECTION_MENU_AUTOMATION_ID = "title-menu-review-selection"
MISSING_CREDENTIAL_MODE = "missing_credential_diagnostic"
CONFIGURED_PROVIDER_MODE = "configured_provider_success"
CONFIGURED_PROVIDER_ENDPOINT = "http://127.0.0.1:4141/v1/"

CASE_DOCUMENT = "configured_provider_missing_credential_document"
CASE_SELECTION = "configured_provider_missing_credential_selection"
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
REQUIRED_CASE_IDS = tuple(CASE_FLOWS)
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
FINGERPRINT_OBSERVATION_KEYS = frozenset(
    {"editor_before", "editor_after", "source_before", "source_after"}
)
SHORTCUT_OBSERVATIONS = {CASE_DOCUMENT: "document_shortcut", CASE_SELECTION: "selection_shortcut"}
MODE_OBSERVATIONS = {
    MISSING_CREDENTIAL_MODE: frozenset({"missing_credential_diagnostic"}),
    CONFIGURED_PROVIDER_MODE: frozenset({"review_consent_approved", "structured_success"}),
}


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


def _contract(mode: str) -> evidence.EvidenceContract:
    configured_provider = mode == CONFIGURED_PROVIDER_MODE
    common = FINGERPRINT_OBSERVATION_KEYS | {"no_dirty_interlock"} | MODE_OBSERVATIONS[mode]
    required = {
        case_id: frozenset(common | {shortcut})
        for case_id, shortcut in SHORTCUT_OBSERVATIONS.items()
    }

    def validate_runtime_scan(value: Any) -> None:
        evidence.validate_runtime_scan(value, require_config=True)

    def validate_mode(value: dict[str, Any]) -> None:
        if value["mode"] != mode:
            raise ValueError("invalid native Review mode")

    def validate_passed_case(case_id: str, observations: dict[str, Any]) -> None:
        require_true(observations, *required[case_id] - FINGERPRINT_OBSERVATION_KEYS)
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

    return evidence.EvidenceContract(
        schema=SCHEMA,
        case_flows=CONFIGURED_PROVIDER_CASE_FLOWS if configured_provider else CASE_FLOWS,
        observation_keys=frozenset().union(*required.values()),
        fingerprint_keys=FINGERPRINT_OBSERVATION_KEYS,
        required_observations=required,
        validate_runtime_scan=validate_runtime_scan,
        validate_passed_case=validate_passed_case,
        private_sentinels=(DOCUMENT_SENTINEL,),
        safe_strings=SAFE_STRINGS,
        extra_keys=frozenset({"mode"}),
        validate_extra=validate_mode,
    )


CONTRACTS = {mode: _contract(mode) for mode in MODE_OBSERVATIONS}


def new_evidence(expected_hash: str, configured_provider: bool = False) -> dict[str, Any]:
    mode = CONFIGURED_PROVIDER_MODE if configured_provider else MISSING_CREDENTIAL_MODE
    return evidence.new_evidence(CONTRACTS[mode], expected_hash, mode=mode)


def validate_evidence(value: dict[str, Any]) -> None:
    mode = value.get("mode") if isinstance(value, dict) else None
    contract = CONTRACTS.get(mode) if isinstance(mode, str) else None
    if contract is None:
        raise ValueError("invalid native Review mode")
    evidence.validate_evidence(value, contract)


def scan_case_artifacts(case_root: Path, ephemeral_credential: str | None = None) -> dict[str, Any]:
    codes = leak_codes("DOCUMENT_SENTINEL", DOCUMENT_SENTINEL)
    if ephemeral_credential is not None:
        codes |= leak_codes("EPHEMERAL_CREDENTIAL", ephemeral_credential)
    return evidence.scan_case_artifacts(case_root, codes, require_config=True)


def goal06_preflight(configured_provider: bool):
    """Return the mode-specific preflight before any child process is created."""

    def verify(exe: Path, expected_hash: str, evidence: dict[str, Any]) -> tuple[Any, Any]:
        win32, parent_context = preflight(exe, expected_hash, evidence)
        if configured_provider and win32.persistent_credential_target_exists(
            credential_target(CONFIGURED_PROVIDER_ENDPOINT, "openai-responses")
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
            credential_target(CONFIGURED_PROVIDER_ENDPOINT, "openai-responses")
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
    parser = evidence.build_parser(__doc__.splitlines()[0], "goal-06", REQUIRED_CASE_IDS)
    parser.add_argument(
        "--configured-provider",
        action="store_true",
        help=(
            "send the fixed test document to an already-running local provider at "
            "http://127.0.0.1:4141/v1/ using a fresh process-only credential"
        ),
    )
    return parser.parse_args(argv)


def main(argv: list[str] | None = None) -> int:
    args = parse_args(argv)
    return main_native_acceptance(args, native_run_plan(args.configured_provider))


if __name__ == "__main__":
    raise SystemExit(main())
