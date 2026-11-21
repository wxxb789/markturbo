"""Exercise Goal 06 Review through the Windows UI.

The default mode needs one app session. Its profile names a fresh
`https://review-<uuid>.invalid/v1/` endpoint and no credential, so no
document content can leave.
Ctrl+Enter from the focused editor must show the explicit local diagnostic
(`No model set up`). The card's Close and the title menu's Review row are UIA
Invokes with no native-only behaviour; the headless `state.rs` test
`review_without_a_model_closes_and_the_menu_row_shows_it_again` proves them.
Each phase has its own observation keys, and every failure inside a phase,
including one a shared helper raises, carries the phase's code prefix.

The opt-in configured-provider mode sends only the fixed test document to an
already-running loopback provider using a fresh process-only credential, in
one session of two phases: the document Review (Ctrl+Enter), closed with
`review-close`, then the selection Review (Select All, then the menu row),
whose consent card must show the `Selection` scope. The consent card
authorizes only a real pointer hold on `consent-send` (a UIA Invoke is a tap
and sends nothing).

The harness itself never queries Credential Manager. The shipped app it
drives still resolves a Review's credential through the platform store first
(`mt-core` `CredentialVault::resolve`), so a run reads the store at its
endpoint's target: a fresh one in the default mode, the fixed loopback
identity in the configured-provider mode.

Both modes bind PASS evidence to an executable hash and prove the source file
stayed byte-identical and the document clean (a close request ends the
process without the unsaved-changes guard).
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
    GUARD_HEADING,
    INTEGRITY_NAMES,
    VK_A,
    VK_RETURN,
    HarnessFailure,
    NativeHarness as BaseNativeHarness,
    NativeRunPlan,
    elapsed_ms,
    finite_nonnegative,
    load_pywinauto,
    main_native_acceptance,
    phase,
    preflight,
    sha256_file,
    validate_fingerprint,
    wait_until,
    write_durable,
)


SCHEMA = "markturbo.goal-06-native-acceptance"
# `ui/review.rs` `unavailable`: the explicit local diagnostic, an anonymous Text.
NOT_SET_UP_MESSAGE = "No model set up"
# `ui/review.rs`: the result card's Close, a Button with its own id.
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
CASE_CONFIGURED = "configured_provider_session"
CASE_FLOWS = {
    NOT_SET_UP_MODE: {
        CASE_NOT_SET_UP: (
            "fresh .invalid endpoint without credential -> Ctrl+Enter review -> "
            "No model set up -> clean close"
        ),
    },
    CONFIGURED_PROVIDER_MODE: {
        CASE_CONFIGURED: (
            "configured provider with ephemeral credential -> Ctrl+Enter document review -> "
            "hold-to-send consent -> structured success -> Close -> Select All -> "
            "menu selection review -> selection-scope hold-to-send consent -> structured success"
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

# Per mode: the source file fingerprints (each `*_source_after` must equal
# `source_before`) and the measured durations.
# `clean_close_ms` is how the document's clean state is measured: the title
# row's unsaved mark (`title-dirty`) is a Text inside the `title-file` Button,
# which nsh 0.6.1's UIA folds away, so a close request that ends the process
# without the 'Close without saving?' guard is the native proof.
CONFIGURED_PHASES = ("document", "selection")
OBSERVATIONS = {
    NOT_SET_UP_MODE: {
        "fingerprints": frozenset({"source_before", "shortcut_source_after"}),
        "durations": frozenset({"shortcut_diagnostic_ms", "clean_close_ms"}),
    },
    CONFIGURED_PROVIDER_MODE: {
        "fingerprints": frozenset(
            {"source_before"} | {f"{phase}_source_after" for phase in CONFIGURED_PHASES}
        ),
        "durations": frozenset(
            {"document_consent_ms", "document_result_ms", "document_close_ms"}
            | {"selection_consent_ms", "selection_result_ms", "clean_close_ms"}
        ),
    },
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
    keys = OBSERVATIONS[mode]
    required = {case_id: keys["fingerprints"] | keys["durations"] for case_id in CASE_IDS[mode]}

    def validate_runtime_scan(value: Any) -> None:
        evidence.validate_runtime_scan(value, require_config=True)

    def validate_mode(value: dict[str, Any]) -> None:
        if value["mode"] != mode:
            raise ValueError("invalid native Review mode")

    def validate_passed_case(case_id: str, observations: dict[str, Any]) -> None:
        for key in sorted(keys["durations"]):
            if not finite_nonnegative(observations.get(key)):
                raise ValueError(f"passed case requires a measured {key}")
        prints = {key: validate_fingerprint(observations.get(key)) for key in keys["fingerprints"]}
        for after in sorted(keys["fingerprints"] - {"source_before"}):
            if prints[after] != prints["source_before"]:
                raise ValueError(f"Review changed source text ({after})")

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


def _window_exists(win32: Any, hwnd: int) -> bool:
    return bool(win32.user32.IsWindow(hwnd))


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

    def send_from_editor(self, app: Any, key: int, code: str) -> None:
        """Ctrl+`key` from the focused editor, once the app's main window owns
        the keyboard; the activation that hands it over is awaited, not
        sampled once."""
        self.focus_editor(app)
        wait_until(
            lambda: self.win32.foreground_focus(app.hwnd) == app.hwnd,
            self.ui_timeout,
            f"{code}_NATIVE_FOCUS_LOST",
        )
        self.win32.send_shortcut(app.hwnd, key)

    def request_review(self, app: Any, selection: bool) -> None:
        """Document: Ctrl+Enter from the editor. Selection: Select All in the
        editor, then the title menu's Review row."""
        if not selection:
            self.send_from_editor(app, VK_RETURN, "REVIEW_SHORTCUT")
            return
        self.send_from_editor(app, VK_A, "REVIEW_SELECT_ALL")
        self.menu_command(app, REVIEW_MENU_AUTOMATION_ID)

    def approve_review_consent(self, app: Any, selection: bool) -> None:
        """Hold `consent-send`; a selection's card must first show its scope."""
        send = self.find_control(
            app,
            CONSENT_SEND_AUTOMATION_ID,
            "Button",
            "REVIEW_CONSENT_UIA_TIMEOUT",
            "REVIEW_CONSENT_UIA_CONTRACT_MISMATCH",
        )
        if selection and not self.name_present(app, CONSENT_SELECTION_SCOPE, "Text"):
            raise HarnessFailure("REVIEW_CONSENT_SELECTION_SCOPE_MISSING")
        self.hold_control(app, send, CONSENT_HOLD_SECONDS, "REVIEW_CONSENT_HOLD_FAILED")

    def read_only_phase(
        self, source: Path, before: dict[str, Any], phase: str
    ) -> dict[str, Any]:
        """Hash the source file after one Review phase (`phase` names its
        observation key). The editor is not read: the guard-free close
        proves the buffer stayed clean."""
        file = sha256_file(source)
        if file.evidence() != before["source_before"]:
            raise HarnessFailure("REVIEW_CHANGED_SOURCE")
        return {f"{phase}_source_after": file.evidence()}

    def close_clean(self, app: Any) -> float:
        """Request the window's close and measure until the process ends.
        A dirty document raises the in-app guard instead (`request_quit`).
        nsh destroys a clean window before its process exits, so a gone
        window, or a query that fails while it goes, is still closing: only a
        guard actually found fails the close; the exit wait decides the rest."""

        def settled() -> str | None:
            if app.process.poll() is not None or not _window_exists(self.win32, app.hwnd):
                return "closing"
            try:
                return "guard" if self.name_present(app, GUARD_HEADING, "Text") else None
            except HarnessFailure:
                return None

        started = time.perf_counter()
        self.win32.post_close(app.hwnd)
        if wait_until(settled, self.ui_timeout, "PROCESS_EXIT_TIMEOUT") == "guard":
            raise HarnessFailure("REVIEW_LEFT_DOCUMENT_DIRTY")
        self.wait_process_exit(app)
        return elapsed_ms(started)

    def wait_not_set_up(self, app: Any, timeout_code: str) -> float:
        started = time.perf_counter()
        self.wait_name(app, NOT_SET_UP_MESSAGE, "Text", timeout_code)
        return elapsed_ms(started)

    def close_review_card(self, app: Any, shown_text: str) -> float:
        """Close the Review card and wait until neither `shown_text` nor its
        Close remains, so the next phase cannot pass on the old card."""
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
            lambda: not self.name_present(app, shown_text, "Text")
            and self.control_absent(
                app,
                REVIEW_CLOSE_AUTOMATION_ID,
                "Button",
                "CLOSE_REVIEW_UIA_QUERY_FAILED",
                "CLOSE_REVIEW_UIA_CONTRACT_MISMATCH",
            ),
            self.ui_timeout,
            "CLOSE_REVIEW_STILL_SHOWN",
        )
        return elapsed_ms(started)

    def initial_state(self, app: Any, source: Path) -> dict[str, Any]:
        self.activate_source_layout(app)
        return {"source_before": sha256_file(source).evidence()}

    def not_set_up_session(self, app: Any, source: Path) -> dict[str, Any]:
        observations = self.initial_state(app, source)
        # Shortcut phase: the card must be absent first, so its appearance
        # is this key's effect.
        with phase("SHORTCUT"):
            if self.name_present(app, NOT_SET_UP_MESSAGE, "Text"):
                raise HarnessFailure("DIAGNOSTIC_PRESENT_BEFORE_REVIEW")
            self.send_from_editor(app, VK_RETURN, "SHORTCUT")
            observations["shortcut_diagnostic_ms"] = self.wait_not_set_up(
                app, "SHORTCUT_DIAGNOSTIC_UIA_TIMEOUT"
            )
            observations.update(self.read_only_phase(source, observations, "shortcut"))
        return observations

    def configured_review(
        self, app: Any, source: Path, before: dict[str, Any], name: str
    ) -> dict[str, Any]:
        """One successful Review (`name`: document or selection) and its
        read-only proof, under its own observation and failure prefixes."""
        selection = name == "selection"
        with phase(name.upper()):
            started = time.perf_counter()
            self.request_review(app, selection)
            self.approve_review_consent(app, selection)
            observed = {f"{name}_consent_ms": elapsed_ms(started)}
            started = time.perf_counter()
            self.wait_name(
                app,
                REVIEW_RESULT_MARKER,
                "Text",
                "REVIEW_RESULT_UIA_TIMEOUT",
                max(self.ui_timeout, 60.0),
            )
            observed[f"{name}_result_ms"] = elapsed_ms(started)
            observed.update(self.read_only_phase(source, before, name))
            if not selection:
                # The next phase must not pass on this card's result.
                observed["document_close_ms"] = self.close_review_card(
                    app, REVIEW_RESULT_MARKER
                )
            return observed

    def configured_session(self, app: Any, source: Path) -> dict[str, Any]:
        observations = self.initial_state(app, source)
        for name in CONFIGURED_PHASES:
            observations.update(self.configured_review(app, source, observations, name))
        return observations

    def scenario(self, case_id: str) -> dict[str, Any]:
        case_root, data, config, workspace, stderr, source = self.profile(case_id)
        app = self.launch_app(source, data, config, workspace, stderr)
        try:
            if self.configured_provider:
                observations = self.configured_session(app, source)
            else:
                observations = self.not_set_up_session(app, source)
            observations["clean_close_ms"] = self.close_clean(app)
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
        preflight=preflight,
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
