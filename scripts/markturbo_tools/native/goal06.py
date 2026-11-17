"""Exercise Goal 06 Review through the Windows UI.

The default mode needs one app session. Its profile names a fresh
`https://review-<uuid>.invalid/v1/` endpoint and no credential, so no
document content can leave; the harness never queries Credential Manager.
Ctrl+Enter from the focused editor must show the explicit local diagnostic
(`No model set up`); the card's Close (`review-close`) must take it away; and
the title menu's Review row (`title-menu-review-selection`) must bring it back.
Each phase has its own observation keys and failure codes.

The opt-in configured-provider mode sends only the fixed test document to an
already-running loopback provider using a fresh process-only credential, in
two sessions: the document Review (Ctrl+Enter) and the selection Review
(Select All, then the menu row), whose consent card must show the
`Selection` scope. The consent card authorizes only a real pointer hold on
`consent-send` (a UIA Invoke is a tap and sends nothing).

Both modes bind PASS evidence to an executable hash and prove the editor and
source file stayed byte-identical and the document clean.
"""

from __future__ import annotations

import argparse
import json
import time
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
    finite_nonnegative,
    load_pywinauto,
    main_native_acceptance,
    preflight,
    require_true,
    sha256_file,
    validate_fingerprint,
    wait_until,
    write_durable,
)


SCHEMA = "markturbo.goal-06-native-acceptance"
# `ui/review.rs` `unavailable`: the explicit local diagnostic, an anonymous Text.
NOT_SET_UP_MESSAGE = "No model set up"
# `ui/review.rs` `unavailable`: the card's Close, a Button with its own id.
REVIEW_CLOSE_AUTOMATION_ID = "review-close"
# `ui/review.rs` `intent`: the first line of a structured result.
REVIEW_RESULT_MARKER = "Understood:"
# `ui/consent.rs` `send_button`: the hold control carries its own test id.
CONSENT_SEND_AUTOMATION_ID = "consent-send"
# `ui/consent.rs` `scope_row`: the scope word a selection binding shows.
CONSENT_SELECTION_SCOPE = "Selection"
# Longer than the control's 1.2s hold (`HOLD_MS`), so a send completes.
CONSENT_HOLD_SECONDS = 1.8
# `ui/titlebar.rs`: the settings-glyph menu's Review row (`Review selection`).
REVIEW_MENU_AUTOMATION_ID = "title-menu-review-selection"
NOT_SET_UP_MODE = "fresh_endpoint_not_set_up"
CONFIGURED_PROVIDER_MODE = "configured_provider_success"
CONFIGURED_PROVIDER_ENDPOINT = "http://127.0.0.1:4141/v1/"

CASE_NOT_SET_UP = "fresh_endpoint_not_set_up"
CASE_DOCUMENT = "configured_provider_document"
CASE_SELECTION = "configured_provider_selection"
CASE_FLOWS = {
    NOT_SET_UP_MODE: {
        CASE_NOT_SET_UP: (
            "fresh .invalid endpoint without credential -> Ctrl+Enter review -> "
            "No model set up -> Close -> title menu Review row -> No model set up"
        ),
    },
    CONFIGURED_PROVIDER_MODE: {
        CASE_DOCUMENT: (
            "configured provider with ephemeral credential -> Ctrl+Enter document review -> "
            "hold-to-send consent -> structured success"
        ),
        CASE_SELECTION: (
            "configured provider with ephemeral credential -> menu selection review -> "
            "selection-scope hold-to-send consent -> structured success"
        ),
    },
}
CASE_IDS = {mode: tuple(flows) for mode, flows in CASE_FLOWS.items()}
REQUIRED_CASE_IDS = CASE_IDS[NOT_SET_UP_MODE]

DOCUMENT_SENTINEL = "MTG06-NATIVE-READ-ONLY-SENTINEL"
DOCUMENT_TEXT = (
    f"# {DOCUMENT_SENTINEL}\n\n"
    "Review must preserve this source exactly, including this line.\n"
)

SAFE_STRINGS = (
    frozenset(flow for flows in CASE_FLOWS.values() for flow in flows.values())
    | INTEGRITY_NAMES
)

# Per mode: the fingerprints (each `*_after` must equal its `*_before`), the
# measured booleans a PASS needs False, and the measured durations.
NOT_SET_UP_PHASES = ("shortcut", "menu")
OBSERVATIONS = {
    NOT_SET_UP_MODE: {
        "fingerprints": frozenset(
            {"editor_before", "source_before"}
            | {f"{phase}_{side}_after" for phase in NOT_SET_UP_PHASES for side in ("editor", "source")}
        ),
        "false": frozenset(
            {"shortcut_diagnostic_before"}
            | {f"{phase}_document_dirty" for phase in NOT_SET_UP_PHASES}
        ),
        "durations": frozenset(
            {"shortcut_diagnostic_ms", "close_diagnostic_gone_ms", "menu_diagnostic_ms"}
        ),
    },
    CONFIGURED_PROVIDER_MODE: {
        "fingerprints": frozenset(
            {"editor_before", "source_before", "editor_after", "source_after"}
        ),
        "false": frozenset({"document_dirty"}),
        "durations": frozenset({"consent_ms", "result_ms"}),
    },
}
# The selection binding alone must show its scope on the consent card.
CASE_TRUE_OBSERVATIONS = {CASE_SELECTION: frozenset({"consent_selection_scope"})}


def _after_pairs(fingerprints: frozenset[str]) -> list[tuple[str, str]]:
    """(after, before) fingerprint pairs: editor to editor, source to source."""
    return [
        (key, "editor_before" if key.endswith("editor_after") else "source_before")
        for key in sorted(fingerprints)
        if key.endswith("_after")
    ]


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
    keys = OBSERVATIONS[mode]
    required = {
        case_id: keys["fingerprints"]
        | keys["false"]
        | keys["durations"]
        | CASE_TRUE_OBSERVATIONS.get(case_id, frozenset())
        for case_id in CASE_IDS[mode]
    }

    def validate_runtime_scan(value: Any) -> None:
        evidence.validate_runtime_scan(value, require_config=True)

    def validate_mode(value: dict[str, Any]) -> None:
        if value["mode"] != mode:
            raise ValueError("invalid native Review mode")

    def validate_passed_case(case_id: str, observations: dict[str, Any]) -> None:
        require_true(observations, *CASE_TRUE_OBSERVATIONS.get(case_id, ()))
        for key in sorted(keys["false"]):
            if observations.get(key) is not False:
                raise ValueError(f"passed case requires false {key}")
        for key in sorted(keys["durations"]):
            if not finite_nonnegative(observations.get(key)):
                raise ValueError(f"passed case requires a measured {key}")
        prints = {key: validate_fingerprint(observations.get(key)) for key in keys["fingerprints"]}
        if prints["editor_before"] != prints["source_before"]:
            raise ValueError("editor and source text differ before Review")
        for after, before in _after_pairs(keys["fingerprints"]):
            if prints[after] != prints[before]:
                kind = "editor" if before == "editor_before" else "source"
                raise ValueError(f"Review changed {kind} text ({after})")

    return evidence.EvidenceContract(
        schema=SCHEMA,
        case_flows=CASE_FLOWS[mode],
        observation_keys=frozenset().union(*required.values()),
        fingerprint_keys=keys["fingerprints"],
        required_observations=required,
        validate_runtime_scan=validate_runtime_scan,
        validate_passed_case=validate_passed_case,
        private_sentinels=(DOCUMENT_SENTINEL,),
        safe_strings=SAFE_STRINGS,
        extra_keys=frozenset({"mode"}),
        validate_extra=validate_mode,
    )


CONTRACTS = {mode: _contract(mode) for mode in CASE_FLOWS}


def _mode(configured_provider: bool) -> str:
    return CONFIGURED_PROVIDER_MODE if configured_provider else NOT_SET_UP_MODE


def new_evidence(expected_hash: str, configured_provider: bool = False) -> dict[str, Any]:
    mode = _mode(configured_provider)
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


def _elapsed_ms(started: float) -> float:
    return round((time.perf_counter() - started) * 1000, 3)


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

    def send_from_editor(self, app: Any, key: int, code: str) -> bool:
        """Ctrl+`key` from the focused editor. Returns the measured fact that
        the app's main window owned the keyboard as the key went out."""
        self.focus_editor(app)
        owned = self.win32.foreground_focus(app.hwnd) == app.hwnd
        if not owned:
            raise HarnessFailure(f"{code}_NATIVE_FOCUS_LOST")
        self.win32.send_shortcut(app.hwnd, key)
        return owned

    def request_review(self, app: Any, target: str) -> bool:
        """Document: Ctrl+Enter from the editor. Selection: Select All in the
        editor, then the title menu's Review row. Returns the measured
        keyboard ownership of the key sent."""
        if target != CASE_SELECTION:
            return self.send_from_editor(app, VK_RETURN, "REVIEW_SHORTCUT")
        owned = self.send_from_editor(app, VK_A, "REVIEW_SELECT_ALL")
        self.menu_command(app, REVIEW_MENU_AUTOMATION_ID)
        return owned

    def approve_review_consent(self, app: Any, target: str) -> bool | None:
        """Hold `consent-send`; for a selection, first measure its scope word."""
        send = self.find_control(
            app,
            CONSENT_SEND_AUTOMATION_ID,
            "Button",
            "REVIEW_CONSENT_UIA_TIMEOUT",
            "REVIEW_CONSENT_UIA_CONTRACT_MISMATCH",
        )
        scope_shown = None
        if target == CASE_SELECTION:
            scope_shown = self.name_present(app, CONSENT_SELECTION_SCOPE, "Text")
            if not scope_shown:
                raise HarnessFailure("REVIEW_CONSENT_SELECTION_SCOPE_MISSING")
        self.hold_control(app, send, CONSENT_HOLD_SECONDS, "REVIEW_CONSENT_HOLD_FAILED")
        return scope_shown

    def read_only_phase(
        self, app: Any, source: Path, before: dict[str, Any], phase: str | None
    ) -> dict[str, Any]:
        """Measure the editor, the file and the dirty mark after one Review
        phase; each change fails with the phase's own code."""
        prefix = f"{phase}_" if phase else ""
        code = f"{phase.upper()}_REVIEW" if phase else "REVIEW"
        editor = self.editor_fingerprint(app)
        file = sha256_file(source)
        dirty = self.document_dirty(app)
        if editor.evidence() != before["editor_before"]:
            raise HarnessFailure(f"{code}_CHANGED_EDITOR")
        if file.evidence() != before["source_before"]:
            raise HarnessFailure(f"{code}_CHANGED_SOURCE")
        if dirty:
            raise HarnessFailure(f"{code}_MARKED_DOCUMENT_DIRTY")
        return {
            f"{prefix}editor_after": editor.evidence(),
            f"{prefix}source_after": file.evidence(),
            f"{prefix}document_dirty": dirty,
        }

    def not_set_up_shown(self, app: Any) -> bool:
        return self.name_present(app, NOT_SET_UP_MESSAGE, "Text")

    def wait_not_set_up(self, app: Any, timeout_code: str) -> float:
        started = time.perf_counter()
        self.wait_name(app, NOT_SET_UP_MESSAGE, "Text", timeout_code)
        return _elapsed_ms(started)

    def close_not_set_up(self, app: Any) -> float:
        """Close the diagnostic card and wait until neither its text nor its
        Close remains, so the menu phase cannot pass on the old card."""
        close = self.find_control(
            app,
            REVIEW_CLOSE_AUTOMATION_ID,
            "Button",
            "CLOSE_REVIEW_UIA_TIMEOUT",
            "CLOSE_REVIEW_UIA_CONTRACT_MISMATCH",
        )
        started = time.perf_counter()
        self.invoke_control(close, "CLOSE_REVIEW_INVOKE_FAILED")
        wait_until(
            lambda: not self.not_set_up_shown(app)
            and self.control_absent(
                app,
                REVIEW_CLOSE_AUTOMATION_ID,
                "Button",
                "CLOSE_REVIEW_UIA_QUERY_FAILED",
                "CLOSE_REVIEW_UIA_CONTRACT_MISMATCH",
            ),
            self.ui_timeout,
            "CLOSE_DIAGNOSTIC_STILL_SHOWN",
        )
        return _elapsed_ms(started)

    def initial_state(self, app: Any, source: Path) -> dict[str, Any]:
        self.activate_source_layout(app)
        editor_before = self.editor_fingerprint(app)
        source_before = sha256_file(source)
        if editor_before != source_before:
            raise HarnessFailure("INITIAL_EDITOR_SOURCE_MISMATCH")
        return {
            "editor_before": editor_before.evidence(),
            "source_before": source_before.evidence(),
        }

    def not_set_up_session(self, app: Any, source: Path) -> dict[str, Any]:
        observations = self.initial_state(app, source)
        # Shortcut phase: the card must be absent first, so its appearance
        # is this key's effect.
        observations["shortcut_diagnostic_before"] = self.not_set_up_shown(app)
        if observations["shortcut_diagnostic_before"]:
            raise HarnessFailure("SHORTCUT_DIAGNOSTIC_PRESENT_BEFORE_REVIEW")
        observations["foreground_verified"] = self.send_from_editor(app, VK_RETURN, "SHORTCUT")
        observations["shortcut_diagnostic_ms"] = self.wait_not_set_up(
            app, "SHORTCUT_DIAGNOSTIC_UIA_TIMEOUT"
        )
        observations.update(self.read_only_phase(app, source, observations, "shortcut"))
        # Close phase, then the menu phase on an empty field.
        observations["close_diagnostic_gone_ms"] = self.close_not_set_up(app)
        started = time.perf_counter()
        self.menu_command(app, REVIEW_MENU_AUTOMATION_ID)
        self.wait_name(app, NOT_SET_UP_MESSAGE, "Text", "MENU_DIAGNOSTIC_UIA_TIMEOUT")
        observations["menu_diagnostic_ms"] = _elapsed_ms(started)
        observations.update(self.read_only_phase(app, source, observations, "menu"))
        return observations

    def configured_session(self, app: Any, source: Path, case_id: str) -> dict[str, Any]:
        observations = self.initial_state(app, source)
        started = time.perf_counter()
        observations["foreground_verified"] = self.request_review(app, case_id)
        scope_shown = self.approve_review_consent(app, case_id)
        observations["consent_ms"] = _elapsed_ms(started)
        if scope_shown is not None:
            observations["consent_selection_scope"] = scope_shown
        started = time.perf_counter()
        self.wait_name(
            app, REVIEW_RESULT_MARKER, "Text", "REVIEW_RESULT_UIA_TIMEOUT", max(self.ui_timeout, 60.0)
        )
        observations["result_ms"] = _elapsed_ms(started)
        observations.update(self.read_only_phase(app, source, observations, None))
        return observations

    def scenario(self, case_id: str) -> dict[str, Any]:
        case_root, data, config, workspace, stderr, source = self.profile(case_id)
        app = self.launch_app(source, data, config, workspace, stderr)
        try:
            if self.configured_provider:
                observations = self.configured_session(app, source, case_id)
            else:
                observations = self.not_set_up_session(app, source)
            self.close_app(app)
            observations.update(
                flow=CASE_FLOWS[_mode(self.configured_provider)][case_id],
                process_context=app.security_context.evidence(),
            )
        finally:
            self.reap(app)
        observations["runtime_scan"] = scan_case_artifacts(
            case_root,
            app.spec.env.get("OPENAI_API_KEY") if self.configured_provider else None,
        )
        return observations


def native_run_plan(configured_provider: bool = False) -> NativeRunPlan:
    return NativeRunPlan(
        required_case_ids=CASE_IDS[_mode(configured_provider)],
        workdir_prefix="markturbo-goal-06-native-",
        new_evidence=lambda expected_hash: new_evidence(expected_hash, configured_provider),
        validate_evidence=validate_evidence,
        preflight=goal06_preflight(configured_provider),
        ui_types_loader=load_pywinauto,
        harness_factory=lambda *args: Goal06Harness(
            *args, configured_provider=configured_provider
        ),
        scenarios=lambda harness: tuple(
            (lambda case_id=case_id: harness.scenario(case_id))
            for case_id in CASE_IDS[_mode(configured_provider)]
        ),
    )


def parse_args(argv: list[str] | None = None) -> argparse.Namespace:
    all_case_ids = tuple(case_id for ids in CASE_IDS.values() for case_id in ids)
    parser = evidence.build_parser(__doc__.splitlines()[0], "goal-06", all_case_ids)
    parser.add_argument(
        "--configured-provider",
        action="store_true",
        help=(
            "send the fixed test document to an already-running local provider at "
            "http://127.0.0.1:4141/v1/ using a fresh process-only credential"
        ),
    )
    args = parser.parse_args(argv)
    if args.case is not None and args.case not in CASE_IDS[_mode(args.configured_provider)]:
        parser.error("--case names a case of the other Review mode")
    return args


def main(argv: list[str] | None = None) -> int:
    args = parse_args(argv)
    return main_native_acceptance(args, native_run_plan(args.configured_provider))


if __name__ == "__main__":
    raise SystemExit(main())
