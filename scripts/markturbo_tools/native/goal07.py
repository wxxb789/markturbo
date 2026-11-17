"""Goal 07 native acceptance harness.

This harness drives the real Windows UI with a deterministic OpenAI Responses
loopback server, in one app session. The server and evidence boundary never
persist document text, request bodies, or credentials; evidence contains only
hashes, byte counts, booleans, counts and safe reason codes. The harness never
reads or writes the Windows Credential Manager: the child process gets one
fresh synthetic key in its environment only.

One launch runs three phases, each with its own Review -> answer -> Revision
chain against the loopback, each on its own workspace file opened in a new
tab through the files palette (`Ctrl+P`, `ui/rail.rs`), and each with its own
observation-key prefix and failure-code prefix:

1. `decisions` (`revision.md`): Accept writes one hunk, `Ctrl+Z` takes it
   back, the `A` key accepts it again, Reject writes nothing, `Ctrl+Z` takes
   the rejection back, Accept completes the accept-all text, `Copy` round-trips
   the OS clipboard, and `Done` writes nothing more or to disk.
2. `stale` (`stale.md`): a paste over the reviewed text leaves every hunk
   `Can't apply — this part changed` without an Accept; the `A` key reaches
   the field (as in phase 1) and is refused; `Done` writes nothing.
3. `trust` (`trusted.html`): scripts are trusted through the Web pane's
   `Trust scripts` and its Run scripts system alert, the keyboard comes back
   from the WebView, and accepting the executable change leaves the Web pane
   Restricted.

On the nsh UI a Review starts with Ctrl+Enter from the editor, the question is
answered in `review-answer-input`, `Prepare revision` asks for the Revision,
and both sends need a real pointer hold on `consent-send`. The Revision field
decides one docked hunk at a time (`Accept` / `Reject`, `A` / `R`, counted by
`n of m decided`). Each decision writes at once (`ui/revision.rs`); `Ctrl+Z`
is the field's Undo while no text input has the keyboard, so the harness moves
the focus to `Done` first. `Done` leaves an outcome line that `revision-dismiss`
consumes; `review-close` then closes the Review before the next phase.
Editor text is read with the editor's Select All + Copy.

Trust scripts' Run scripts confirmation is an nsh system alert outside the UIA
tree, answered with Enter (`runtime.py` `confirm_system_alert`).

Evidence follows the shared native schema (`evidence.py`).
"""

from __future__ import annotations

import argparse
import json
import uuid
from collections.abc import Iterator
from contextlib import contextmanager
from pathlib import Path
from typing import Any

from . import evidence
from . import goal07_provider as provider_fixtures
from .evidence import credential_target, leak_codes
from .runtime import (
    INTEGRITY_NAMES,
    MENU_EDIT_AUTOMATION_ID,
    MENU_WEB_AUTOMATION_ID,
    VK_A,
    VK_RETURN,
    VK_Z,
    Fingerprint,
    HarnessFailure,
    NativeHarness,
    NativeRunPlan,
    complete_evidence as complete_evidence_envelope,
    fingerprint_bytes,
    fingerprint_text,
    load_pywinauto,
    main_native_acceptance,
    preflight,
    safe_exception_name,
    sha256_file,
    validate_fingerprint,
    wait_until,
    write_durable,
)


SCHEMA = "markturbo.goal-07-native-acceptance"
MAX_EDIT_OUTPUT_BYTES = 4 * 1024 * 1024
TRANSPORT_KEYS = frozenset({"mode", "provider", "deterministic", "request_count"})
VK_P = 0x50

# `ui/review.rs`: the structured result's first line, the answer input, and
# the Buttons, each carrying its own AutomationId (`prims` actions).
REVIEW_RESULT_MARKER = "Understood:"
REVIEW_ANSWER_INPUT_AUTOMATION_ID = "review-answer-input"
REVIEW_ANSWER_AUTOMATION_ID = "review-answer-submit"
PREPARE_REVISION_AUTOMATION_ID = "review-prepare"
REVIEW_CLOSE_AUTOMATION_ID = "review-close"
# `ui/consent.rs` `send_button`: only a pointer hold past `HOLD_MS` sends.
CONSENT_SEND_AUTOMATION_ID = "consent-send"
CONSENT_HOLD_SECONDS = 1.8
# `ui/revision.rs`: the docked hunk's decisions, the count, Copy and Done.
# Accept's Name is checked for absence across the whole window. A decided
# docked hunk shows its decision word until the next undecided one is docked.
REVISION_ACCEPT_NAME = "Accept"
REVISION_ACCEPT_AUTOMATION_ID = "revision-accept"
REVISION_REJECT_AUTOMATION_ID = "revision-reject"
REVISION_DONE_AUTOMATION_ID = "revision-done"
REVISION_DISMISS_AUTOMATION_ID = "revision-dismiss"
REVISION_DECISION_WORDS = ("Accepted", "Rejected")
REVISION_PART_CHANGED_TEXT = "Can't apply \u2014 this part changed"
REVISION_COPY_AUTOMATION_ID = "revision-copy"
REVISION_PROPOSAL_AFTER_AUTOMATION_ID = "revision-proposal-after"
REVISION_PROPOSAL_AFTER_NAME = "Proposed text"
# `state.rs` `finish_revision`: Done's outcome line.
REVISION_APPLIED = "Approved changes applied to the buffer."
REVISION_NOTHING_APPROVED = "No changes approved."
# `ui/rail.rs`: `Ctrl+P` opens the workspace's files; Enter opens the first row.
PALETTE_INPUT_AUTOMATION_ID = "palette-input"
# `ui/preview.rs`. `state.rs` `request_web_trust` confirms with an nsh system
# alert outside the UIA tree, answered with Enter (`confirm_system_alert`).
TRUST_SCRIPTS_AUTOMATION_ID = "web-trust"
TRUSTED_BANNER = "Trusted: scripts may run"
RESTRICTED_BANNER = "Restricted: scripts disabled"

CASE_REVISION_SESSION = "revision_session"
CASE_FLOWS = {
    CASE_REVISION_SESSION: (
        "one launch: decisions (Accept, Ctrl+Z, A key, Reject, Ctrl+Z, Accept, Copy, Done) "
        "-> stale proposal refuses the A key -> trusted HTML Accept leaves Restricted"
    ),
}
REQUIRED_CASE_IDS = tuple(CASE_FLOWS)
# Each phase's observation-key prefix; upper-cased, its failure-code prefix.
PHASES = ("decisions", "stale", "trust")
# One Review and one Revision request per phase.
EXPECTED_REQUEST_COUNT = 2 * len(PHASES)

DOCUMENT_SENTINEL = "MTG07-NATIVE-REVISION-SENTINEL"
SOURCE_TEXT = (
    "---\r\n"
    "title: old\r\n"
    "owner: TBD\r\n"
    "---\r\n"
    "\r\n"
    "# Plan 计划 🚀\r\n"
    "\r\n"
    "\x60\x60\x60rust\r\n"
    "fn main() {}\r\n"
    "\x60\x60\x60\r\n"
    "\r\n"
    "[link](https://example.invalid)\r\n"
    f"\r\n<!-- {DOCUMENT_SENTINEL} -->\r\n"
)
SOURCE_BYTES = SOURCE_TEXT.encode("utf-8")
EDITOR_SOURCE_TEXT = SOURCE_TEXT.replace("\r\n", "\n").replace("\r", "\n")
EDITOR_SOURCE_BYTES = EDITOR_SOURCE_TEXT.encode("utf-8")
HTML_SOURCE_TEXT = (
    "<!doctype html>\r\n"
    "<script>window.goal07 = \"old\";</script>\r\n"
    "<p>trusted 文档 🚀</p>\r\n"
    f"<!-- {DOCUMENT_SENTINEL} -->\r\n"
)
HTML_SOURCE_BYTES = HTML_SOURCE_TEXT.encode("utf-8")
HTML_EDITOR_SOURCE_TEXT = HTML_SOURCE_TEXT.replace("\r\n", "\n").replace("\r", "\n")
HTML_EDITOR_SOURCE_BYTES = HTML_EDITOR_SOURCE_TEXT.encode("utf-8")
STALE_EDIT_TEXT = "---\ntitle: locally edited\nowner: TBD\n---\n"
# The workspace: one file per phase, all written before the launch so the
# files palette lists them.
DECISIONS_FILE = "revision.md"
STALE_FILE = "stale.md"
TRUST_FILE = "trusted.html"

SAFE_STRINGS = frozenset(CASE_FLOWS.values()) | INTEGRITY_NAMES
# Every fingerprint is compared with exact expected bytes
# (`expected_fingerprints`); the names say when it was read.
FINGERPRINT_OBSERVATION_KEYS = frozenset(
    {
        "decisions_editor_before",
        "decisions_editor_proposed",
        "decisions_editor_one_accepted",
        "decisions_editor_undone",
        "decisions_editor_one_rejected",
        "decisions_editor_all_accepted",
        "decisions_clipboard_copied",
        "decisions_editor_after_copy",
        "decisions_editor_done",
        "decisions_disk_before",
        "decisions_disk_after",
        "stale_editor_proposed",
        "stale_editor_edited",
        "stale_editor_after_key",
        "stale_editor_done",
        "stale_disk_before",
        "stale_disk_after",
        "trust_editor_proposed",
        "trust_proposal_after",
        "trust_editor_done",
    }
)
# The title row's unsaved mark: clean until the first decision writes, and
# Copy leaves it as it was.
EXPECTED_DIRTY = {
    "decisions_dirty_before": False,
    "decisions_dirty_proposed": False,
    "decisions_dirty_before_copy": True,
    "decisions_dirty_after_copy": True,
}
OBSERVATION_KEYS = (
    FINGERPRINT_OBSERVATION_KEYS | frozenset(EXPECTED_DIRTY) | {"provider_request_count"}
)
REQUIRED_OBSERVATIONS = {CASE_REVISION_SESSION: OBSERVATION_KEYS}


def apply_edits(source: bytes, edits: list[dict[str, Any]]) -> bytes:
    """Apply validated non-overlapping byte edits for expected-preview checks."""
    if not isinstance(source, bytes) or len(source) > MAX_EDIT_OUTPUT_BYTES:
        raise ValueError("invalid native fixture edit")
    if not isinstance(edits, list):
        raise ValueError("invalid native fixture edit")
    try:
        decoded_source = source.decode("utf-8")
    except UnicodeDecodeError:
        raise ValueError("invalid native fixture edit") from None
    utf8_boundaries = {0}
    offset = 0
    for character in decoded_source:
        offset += len(character.encode("utf-8"))
        utf8_boundaries.add(offset)
    for edit in edits:
        if (
            not isinstance(edit, dict)
            or set(edit) != {"range", "expected_source", "replacement"}
            or not isinstance(edit["range"], dict)
            or set(edit["range"]) != {"start", "end"}
            or not isinstance(edit["range"].get("start"), int)
            or isinstance(edit["range"].get("start"), bool)
            or not isinstance(edit["range"].get("end"), int)
            or isinstance(edit["range"].get("end"), bool)
            or not isinstance(edit.get("expected_source"), str)
            or not isinstance(edit.get("replacement"), str)
        ):
            raise ValueError("invalid native fixture edit")
    ordered = sorted(edits, key=lambda edit: edit["range"]["start"])
    output = bytearray()
    cursor = 0
    seen_offsets: set[int] = set()
    for edit in ordered:
        if (
            not isinstance(edit, dict)
            or set(edit) != {"range", "expected_source", "replacement"}
            or not isinstance(edit["range"], dict)
            or set(edit["range"]) != {"start", "end"}
            or not isinstance(edit["range"].get("start"), int)
            or isinstance(edit["range"].get("start"), bool)
            or not isinstance(edit["range"].get("end"), int)
            or isinstance(edit["range"].get("end"), bool)
            or not isinstance(edit.get("expected_source"), str)
            or not isinstance(edit.get("replacement"), str)
        ):
            raise ValueError("invalid native fixture edit")
        start = edit["range"]["start"]
        end = edit["range"]["end"]
        if (
            start < 0
            or end < start
            or end > len(source)
            or start not in utf8_boundaries
            or end not in utf8_boundaries
            or start in seen_offsets
            or start < cursor
        ):
            raise ValueError("invalid native fixture edit")
        seen_offsets.add(start)
        expected = edit["expected_source"].encode("utf-8")
        replacement = edit["replacement"].encode("utf-8")
        if len(replacement) > MAX_EDIT_OUTPUT_BYTES:
            raise ValueError("invalid native fixture edit")
        if source[start:end] != expected:
            raise ValueError("invalid native fixture edit")
        if source[start:end] == replacement:
            raise ValueError("invalid native fixture edit")
        output.extend(source[cursor:start])
        output.extend(replacement)
        if len(output) + len(source) - end > MAX_EDIT_OUTPUT_BYTES:
            raise ValueError("invalid native fixture edit")
        cursor = end
    maximum_subset_size = len(source)
    for edit in ordered:
        start = edit["range"]["start"]
        end = edit["range"]["end"]
        replacement_size = len(edit["replacement"].encode("utf-8"))
        removed_size = end - start
        if replacement_size > removed_size:
            maximum_subset_size += replacement_size - removed_size
            if maximum_subset_size > MAX_EDIT_OUTPUT_BYTES:
                raise ValueError("invalid native fixture edit")
    output.extend(source[cursor:])
    if len(output) > MAX_EDIT_OUTPUT_BYTES:
        raise ValueError("invalid native fixture edit")
    return bytes(output)


def expected_fingerprints() -> dict[str, dict[str, Any]]:
    """The exact bytes every fingerprint observation must have."""
    edits = provider_fixtures.fixture_edits(EDITOR_SOURCE_TEXT)
    html_edits = provider_fixtures.fixture_edits(HTML_EDITOR_SOURCE_TEXT)
    reviewed = fingerprint_bytes(EDITOR_SOURCE_BYTES)
    one = fingerprint_bytes(apply_edits(EDITOR_SOURCE_BYTES, edits[:1]))
    approved = fingerprint_bytes(apply_edits(EDITOR_SOURCE_BYTES, edits))
    disk = fingerprint_bytes(SOURCE_BYTES)
    edited = fingerprint_text(STALE_EDIT_TEXT)
    html_applied = apply_edits(HTML_EDITOR_SOURCE_BYTES, html_edits)
    proposed_line = next(
        line
        for line in html_applied.decode("utf-8").split("\n")
        if html_edits[0]["replacement"] in line
    )
    expected = {
        "decisions_editor_before": reviewed,
        "decisions_editor_proposed": reviewed,
        "decisions_editor_one_accepted": one,
        "decisions_editor_undone": reviewed,
        "decisions_editor_one_rejected": one,
        "decisions_editor_all_accepted": approved,
        "decisions_clipboard_copied": approved,
        "decisions_editor_after_copy": approved,
        "decisions_editor_done": approved,
        "decisions_disk_before": disk,
        "decisions_disk_after": disk,
        "stale_editor_proposed": reviewed,
        "stale_editor_edited": edited,
        "stale_editor_after_key": edited,
        "stale_editor_done": edited,
        "stale_disk_before": disk,
        "stale_disk_after": disk,
        "trust_editor_proposed": fingerprint_bytes(HTML_EDITOR_SOURCE_BYTES),
        "trust_proposal_after": fingerprint_text(proposed_line),
        "trust_editor_done": fingerprint_bytes(html_applied),
    }
    return {key: value.evidence() for key, value in expected.items()}


def _request_count(evidence: dict[str, Any]) -> int:
    count = 0
    for case in evidence.get("cases", ()):
        observations = case.get("observations") if isinstance(case, dict) else None
        if not isinstance(observations, dict) or "provider_request_count" not in observations:
            continue
        observed = observations["provider_request_count"]
        if not isinstance(observed, int) or isinstance(observed, bool) or observed < 0:
            raise ValueError("invalid provider request count")
        count += observed
    return count


def finalize_evidence(evidence: dict[str, Any]) -> None:
    request_count = _request_count(evidence)
    transport = evidence.get("transport")
    if not isinstance(transport, dict):
        raise ValueError("invalid loopback transport evidence")
    transport["request_count"] = request_count


def complete_evidence(evidence: dict[str, Any], status: str) -> None:
    finalize_evidence(evidence)
    complete_evidence_envelope(evidence, status, REQUIRED_CASE_IDS)


def validate_transport(evidence: dict[str, Any]) -> None:
    transport = evidence["transport"]
    if (
        not isinstance(transport, dict)
        or set(transport) != TRANSPORT_KEYS
        or transport.get("mode") != "keyless_loopback"
        or transport.get("provider") != "openai-responses"
        or transport.get("deterministic") is not True
        or not isinstance(transport.get("request_count"), int)
        or isinstance(transport.get("request_count"), bool)
        or transport["request_count"] < 0
    ):
        raise ValueError("invalid loopback transport evidence")
    if isinstance(evidence.get("cases"), list) and transport["request_count"] != _request_count(
        evidence
    ):
        raise ValueError("top-level transport request count is not synchronized")


def validate_runtime_scan(value: Any) -> None:
    evidence.validate_runtime_scan(value, require_config=True)


def validate_passed_case(case_id: str, observations: dict[str, Any]) -> None:
    for key, expected in expected_fingerprints().items():
        if validate_fingerprint(observations[key]) != expected:
            raise ValueError(f"{key} does not hold the expected bytes")
    for key, expected in EXPECTED_DIRTY.items():
        if observations[key] is not expected:
            raise ValueError(f"{key} does not hold the expected dirty state")
    if observations["provider_request_count"] != EXPECTED_REQUEST_COUNT:
        raise ValueError("loopback provider did not receive one Review and Revision per phase")


CONTRACT = evidence.EvidenceContract(
    schema=SCHEMA,
    case_flows=CASE_FLOWS,
    observation_keys=OBSERVATION_KEYS,
    fingerprint_keys=FINGERPRINT_OBSERVATION_KEYS,
    required_observations=REQUIRED_OBSERVATIONS,
    validate_runtime_scan=validate_runtime_scan,
    validate_passed_case=validate_passed_case,
    private_sentinels=(
        DOCUMENT_SENTINEL,
        provider_fixtures.ANSWER_SENTINEL,
        provider_fixtures.RAW_RESPONSE_SENTINEL,
    ),
    safe_strings=SAFE_STRINGS,
    extra_keys=frozenset({"transport"}),
    validate_extra=validate_transport,
)


def new_evidence(expected_hash: str) -> dict[str, Any]:
    return evidence.new_evidence(
        CONTRACT,
        expected_hash,
        transport={
            "mode": "keyless_loopback",
            "provider": "openai-responses",
            "deterministic": True,
            "request_count": 0,
        },
    )


def validate_evidence(value: dict[str, Any]) -> None:
    evidence.validate_evidence(value, CONTRACT)


def scan_case_artifacts(case_root: Path, ephemeral_credential: str) -> dict[str, Any]:
    return evidence.scan_case_artifacts(
        case_root,
        {
            **leak_codes("DOCUMENT_SENTINEL", DOCUMENT_SENTINEL),
            **leak_codes("EPHEMERAL_CREDENTIAL", ephemeral_credential),
            **leak_codes("ANSWER_SENTINEL", provider_fixtures.ANSWER_SENTINEL),
            **leak_codes("RAW_RESPONSE_SENTINEL", provider_fixtures.RAW_RESPONSE_SENTINEL),
        },
        require_config=True,
    )


def review_settings_document(endpoint: str) -> bytes:
    identity = credential_target(endpoint, "openai-responses")
    return (
        "show-welcome-on-startup = false\n"
        'model-provider = "openai-responses"\n'
        'model-name = "goal07-loopback-model"\n'
        f"model-base-url = {json.dumps(endpoint)}\n"
        f"model-environment-key-identity = {json.dumps(identity)}\n"
    ).encode("utf-8")


def require_expected(observed: dict[str, Any]) -> dict[str, Any]:
    """A phase's measurements as evidence, failing on the first unexpected one.

    The key carries its phase, so the failure code (`<KEY>_UNEXPECTED`) says
    which phase measured what; `validate_passed_case` checks them all again.
    """
    expected = {**expected_fingerprints(), **EXPECTED_DIRTY}
    recorded = {
        key: value.evidence() if isinstance(value, Fingerprint) else value
        for key, value in observed.items()
    }
    for key, value in recorded.items():
        if value != expected[key]:
            raise HarnessFailure(f"{key.upper()}_UNEXPECTED")
    return recorded


@contextmanager
def phase_failures(phase: str) -> Iterator[None]:
    """Prefix a phase's failure codes with its name, so a FAIL is locatable.

    A BLOCKED environment stays as it is: it is not the phase's behaviour.
    """
    try:
        yield
    except HarnessFailure as error:
        raise HarnessFailure(f"{phase.upper()}_{error.code}", error.detail) from None


class Goal07Harness(NativeHarness):
    def __init__(self, *args: Any) -> None:
        super().__init__(*args)
        self._credential = f"markturbo-goal07-{uuid.uuid4().hex}"

    def openai_api_key_for_child(self) -> str | None:
        return self._credential

    def profile(self, endpoint: str) -> tuple[Path, Path, Path, Path, Path]:
        """The session's isolated roots, its workspace files and its settings."""
        data_root, config_root, workspace_root, stderr_path = self.case_roots(
            CASE_REVISION_SESSION
        )
        for name, value in (
            (DECISIONS_FILE, SOURCE_BYTES),
            (STALE_FILE, SOURCE_BYTES),
            (TRUST_FILE, HTML_SOURCE_BYTES),
        ):
            write_durable(workspace_root / name, value)
        write_durable(config_root / "settings.toml", review_settings_document(endpoint))
        return data_root.parent, data_root, config_root, workspace_root, stderr_path

    def _require_foreground(self, app: Any) -> bool:
        self.win32.require_foreground(app.hwnd, self.ui_timeout)
        return True

    def _press(self, app: Any, automation_id: str, code: str) -> None:
        self._require_foreground(app)
        control = self.find_control(
            app, automation_id, "Button", f"{code}_UIA_TIMEOUT", f"{code}_UIA_CONTRACT_MISMATCH"
        )
        self.click_control(control, f"{code}_CLICK_FAILED")

    def _named_texts(self, app: Any, automation_id: str, name: str, code: str) -> str:
        """The line texts inside one exposed group named `name`, joined by newlines."""
        group = self.find_control(
            app, automation_id, "Group", f"{code}_UIA_TIMEOUT", f"{code}_UIA_CONTRACT_MISMATCH"
        )
        try:
            group_name = group.element_info.name
            names = [
                child.element_info.name
                for child in group.children()
                if child.element_info.control_type == "Text"
                and isinstance(child.element_info.name, str)
            ]
        except Exception as error:
            raise HarnessFailure(f"{code}_UIA_QUERY_FAILED", safe_exception_name(error)) from None
        if group_name != name:
            raise HarnessFailure(f"{code}_UIA_CONTRACT_MISMATCH")
        if not names:
            raise HarnessFailure(f"{code}_TEXT_UNAVAILABLE")
        return "\n".join(names)

    def _type_into(self, app: Any, automation_id: str, value: str, code: str) -> None:
        """Type `value` through the focused input, the same path a user takes."""
        field = self.find_control(
            app, automation_id, "Edit", f"{code}_UIA_TIMEOUT", f"{code}_UIA_CONTRACT_MISMATCH"
        )
        try:
            field.set_focus()
        except Exception as error:
            raise HarnessFailure(f"{code}_FOCUS_FAILED", safe_exception_name(error)) from None
        self.win32.send_unicode(app.hwnd, value)

        def typed() -> bool:
            control = self.control_by_id(
                app.hwnd, automation_id, "Edit", f"{code}_UIA_CONTRACT_MISMATCH"
            )
            return control is not None and control.iface_value.CurrentValue == value

        wait_until(typed, self.ui_timeout, f"{code}_INPUT_TIMEOUT", interval=0.025)

    def _open_consent_gate(
        self,
        provider: provider_fixtures.LoopbackRevisionServer,
        expected_request_count: int,
        open_gate: Any,
    ) -> None:
        snapshot = provider.request_snapshot()
        if (
            set(snapshot) != {"request_count", "invalid_request"}
            or snapshot["request_count"] != expected_request_count
            or snapshot["invalid_request"] is not False
        ):
            operation = "REVIEW" if expected_request_count == 0 else "REVISION"
            raise HarnessFailure(f"{operation}_REQUEST_BEFORE_CONSENT_CLICK")
        open_gate()

    def _approve_consent(
        self,
        app: Any,
        failure_code: str,
        *,
        provider: provider_fixtures.LoopbackRevisionServer,
        expected_request_count: int | None = None,
        open_gate: Any = None,
    ) -> None:
        send = self.find_control(
            app,
            CONSENT_SEND_AUTOMATION_ID,
            "Button",
            "CONSENT_SEND_UIA_TIMEOUT",
            "CONSENT_SEND_UIA_CONTRACT_MISMATCH",
        )
        if expected_request_count is None or open_gate is None:
            raise HarnessFailure("CONSENT_GATE_CONFIGURATION_INVALID")
        self._open_consent_gate(provider, expected_request_count, open_gate)
        hold = lambda: self.hold_control(app, send, CONSENT_HOLD_SECONDS, failure_code)
        provider.dispatch_consent_click(expected_request_count, hold)

    def run_review(
        self, app: Any, provider: provider_fixtures.LoopbackRevisionServer,
        *, preserve_focus: bool = False,
    ) -> None:
        if preserve_focus:
            if self.win32.foreground_focus(app.hwnd) != app.hwnd:
                raise HarnessFailure("WEB_TO_SOURCE_NATIVE_FOCUS_LOST")
        else:
            self.focus_editor(app)
            self._require_foreground(app)
        self.win32.send_shortcut(app.hwnd, VK_RETURN)
        self._approve_consent(
            app,
            "REVIEW_CONSENT_HOLD_FAILED",
            provider=provider,
            expected_request_count=0,
            open_gate=provider.grant_review_consent,
        )
        self.wait_name(
            app, REVIEW_RESULT_MARKER, "Text", "REVIEW_RESULT_UIA_TIMEOUT", max(self.ui_timeout, 60.0)
        )

    def _set_answer(self, app: Any, value: str) -> None:
        self._type_into(app, REVIEW_ANSWER_INPUT_AUTOMATION_ID, value, "REVIEW_ANSWER")
        self._press(app, REVIEW_ANSWER_AUTOMATION_ID, "REVIEW_ANSWER_SUBMIT")

    def request_revision(
        self,
        app: Any,
        *,
        provider: provider_fixtures.LoopbackRevisionServer,
        hunks: int,
    ) -> None:
        self._set_answer(app, provider_fixtures.ANSWER_TEXT)
        self._press(app, PREPARE_REVISION_AUTOMATION_ID, "REVISION_PREPARE")
        self._approve_consent(
            app,
            "REVISION_CONSENT_HOLD_FAILED",
            provider=provider,
            expected_request_count=1,
            open_gate=provider.grant_revision_consent,
        )
        self.wait_name(
            app,
            f"0 of {hunks} decided",
            "Text",
            "REVISION_PROPOSAL_UIA_TIMEOUT",
            max(self.ui_timeout, 60.0),
        )

    def review_and_revise(
        self,
        app: Any,
        provider: provider_fixtures.LoopbackRevisionServer,
        source: str,
        *,
        preserve_focus: bool = False,
    ) -> int:
        """One phase's own Review -> answer -> Revision chain; its hunk count."""
        provider.begin_chain(source)
        hunks = len(provider_fixtures.fixture_edits(source))
        self.run_review(app, provider, preserve_focus=preserve_focus)
        self.request_revision(app, provider=provider, hunks=hunks)
        return hunks

    def _wait_decided(self, app: Any, decided: int, total: int, code: str) -> None:
        self.wait_name(app, f"{decided} of {total} decided", "Text", code)

    def _wait_next_hunk(self, app: Any) -> None:
        wait_until(
            lambda: not any(
                self.name_present(app, word, "Text") for word in REVISION_DECISION_WORDS
            ),
            self.ui_timeout,
            "REVISION_NEXT_HUNK_TIMEOUT",
            interval=0.05,
        )

    def decide_hunks(
        self, app: Any, decisions: list[bool], total: int, decided: int = 0
    ) -> None:
        """Press Accept/Reject on the docked hunk, in order, from `decided` of `total`.

        Each decision is waited for by its count. While hunks remain undecided
        the field keeps the decided one docked (showing `Accepted`/`Rejected`)
        until it brings the next undecided one forward, so the next press waits
        for that word to go: a press in between would decide the same hunk
        again.
        """
        for accepted in decisions:
            automation_id = (
                REVISION_ACCEPT_AUTOMATION_ID if accepted else REVISION_REJECT_AUTOMATION_ID
            )
            self._press(app, automation_id, "REVISION_ACCEPT" if accepted else "REVISION_REJECT")
            decided += 1
            self._wait_decided(app, decided, total, "REVISION_DECISION_TIMEOUT")
            if decided < total:
                self._wait_next_hunk(app)

    def _field_key(self, app: Any, key: int, *, ctrl: bool = False) -> None:
        """Send one key to the Revision field, off every text input.

        Reading the editor focuses it, and a focused text input keeps `A` and
        `Ctrl+Z` for itself, so the keyboard moves to `Done` first.
        """
        done = self.find_control(
            app,
            REVISION_DONE_AUTOMATION_ID,
            "Button",
            "REVISION_DONE_UIA_TIMEOUT",
            "REVISION_DONE_UIA_CONTRACT_MISMATCH",
        )
        try:
            done.set_focus()
        except Exception as error:
            raise HarnessFailure("REVISION_DONE_FOCUS_FAILED", safe_exception_name(error)) from None
        wait_until(
            lambda: not self.source_editor_has_focus(app),
            self.ui_timeout,
            "REVISION_FIELD_FOCUS_TIMEOUT",
            interval=0.025,
        )
        if ctrl:
            self.win32.send_shortcut(app.hwnd, key)
        else:
            self.win32.send_key(app.hwnd, key)

    def undo_decision(self, app: Any, decided_after: int, total: int) -> None:
        """`Ctrl+Z`: the field takes the last decision back with its text."""
        self._field_key(app, VK_Z, ctrl=True)
        self._wait_decided(app, decided_after, total, "REVISION_UNDO_TIMEOUT")

    def accept_by_key(self, app: Any, decided_after: int, total: int) -> None:
        """The `A` key decides the docked hunk, as the stale phase relies on."""
        self._field_key(app, VK_A)
        self._wait_decided(app, decided_after, total, "REVISION_KEY_DECISION_TIMEOUT")
        if decided_after < total:
            self._wait_next_hunk(app)

    def done(self, app: Any, outcome: str) -> None:
        self._press(app, REVISION_DONE_AUTOMATION_ID, "REVISION_DONE")
        self.wait_name(app, outcome, "Text", "REVISION_DONE_OUTCOME_TIMEOUT")

    def copy_approved(self, app: Any) -> str:
        """`Copy`'s text through the real OS clipboard; the user's text comes back."""
        clipboard_before = self.read_text_clipboard()
        try:
            # Clear the clipboard first so an unchanged old value cannot
            # accidentally satisfy the copy assertion.
            self.write_text_clipboard(None)
            copy = self.find_control(
                app,
                REVISION_COPY_AUTOMATION_ID,
                "Button",
                "REVISION_COPY_UIA_TIMEOUT",
                "REVISION_COPY_UIA_CONTRACT_MISMATCH",
            )
            self.click_control(copy, "REVISION_COPY_CLICK_FAILED")
            return self.wait_clipboard_text("REVISION_COPY_CLIPBOARD_TIMEOUT")
        finally:
            self.write_text_clipboard(clipboard_before)

    def close_review(self, app: Any) -> None:
        """Consume Done's outcome, then close the Review, before the next phase."""
        self._press(app, REVISION_DISMISS_AUTOMATION_ID, "REVISION_DISMISS")
        self._press(app, REVIEW_CLOSE_AUTOMATION_ID, "REVIEW_CLOSE")
        wait_until(
            lambda: not self.name_present(app, REVIEW_RESULT_MARKER, "Text"),
            self.ui_timeout,
            "REVIEW_CLOSE_TIMEOUT",
            interval=0.05,
        )

    def open_workspace_file(self, app: Any, name: str, editor_bytes: bytes) -> None:
        """Open `name` in a new tab from the files palette; wait for its exact text."""
        self._require_foreground(app)
        self.win32.send_shortcut(app.hwnd, VK_P)
        self._type_into(app, PALETTE_INPUT_AUTOMATION_ID, name, "PALETTE")
        self.win32.send_key(app.hwnd, VK_RETURN)
        self.activate_source_layout(app)
        self.wait_editor_fingerprint(app, fingerprint_bytes(editor_bytes), self.ui_timeout)

    def phase_decisions(
        self, app: Any, provider: provider_fixtures.LoopbackRevisionServer, source: Path
    ) -> dict[str, Any]:
        self.activate_source_layout(app)
        observed: dict[str, Any] = {
            "decisions_disk_before": sha256_file(source),
            "decisions_editor_before": self.editor_fingerprint(app),
            "decisions_dirty_before": self.document_dirty(app),
        }
        total = self.review_and_revise(app, provider, EDITOR_SOURCE_TEXT)
        observed["decisions_editor_proposed"] = self.editor_fingerprint(app)
        observed["decisions_dirty_proposed"] = self.document_dirty(app)
        # Accept writes the first hunk at once; Ctrl+Z takes exactly it back.
        self.decide_hunks(app, [True], total)
        observed["decisions_editor_one_accepted"] = self.editor_fingerprint(app)
        self.undo_decision(app, 0, total)
        observed["decisions_editor_undone"] = self.editor_fingerprint(app)
        # The A key accepts it again; Reject writes nothing for the next one.
        self.accept_by_key(app, 1, total)
        self.decide_hunks(app, [False] * (total - 1), total, decided=1)
        observed["decisions_editor_one_rejected"] = self.editor_fingerprint(app)
        # Ctrl+Z takes the rejection back; Accept completes the accept-all text.
        self.undo_decision(app, total - 1, total)
        self.decide_hunks(app, [True], total, decided=total - 1)
        observed["decisions_editor_all_accepted"] = self.editor_fingerprint(app)
        observed["decisions_dirty_before_copy"] = self.document_dirty(app)
        observed["decisions_clipboard_copied"] = fingerprint_text(self.copy_approved(app))
        observed["decisions_editor_after_copy"] = self.editor_fingerprint(app)
        observed["decisions_dirty_after_copy"] = self.document_dirty(app)
        self.done(app, REVISION_APPLIED)
        observed["decisions_editor_done"] = self.editor_fingerprint(app)
        observed["decisions_disk_after"] = sha256_file(source)
        provider.verify_chain()
        self.close_review(app)
        return observed

    def phase_stale(
        self, app: Any, provider: provider_fixtures.LoopbackRevisionServer, source: Path
    ) -> dict[str, Any]:
        self.open_workspace_file(app, source.name, EDITOR_SOURCE_BYTES)
        observed: dict[str, Any] = {"stale_disk_before": sha256_file(source)}
        total = self.review_and_revise(app, provider, EDITOR_SOURCE_TEXT)
        observed["stale_editor_proposed"] = self.editor_fingerprint(app)
        # One paste over the whole text touches every hunk's part.
        observed["stale_editor_edited"] = self.paste_editor(app, STALE_EDIT_TEXT)
        self.wait_name(app, REVISION_PART_CHANGED_TEXT, "Text", "REVISION_PART_CHANGED_UIA_TIMEOUT")
        if self.name_present(app, REVISION_ACCEPT_NAME, "Button"):
            raise HarnessFailure("REVISION_OFFERS_ACCEPT")
        # The A key that accepted in the decisions phase reaches the field and
        # decides nothing here.
        self._field_key(app, VK_A)
        observed["stale_editor_after_key"] = self.editor_fingerprint(app)
        if not self.name_present(app, f"0 of {total} decided", "Text"):
            raise HarnessFailure("REVISION_KEY_DECIDED_CHANGED_PART")
        self.done(app, REVISION_NOTHING_APPROVED)
        observed["stale_editor_done"] = self.editor_fingerprint(app)
        observed["stale_disk_after"] = sha256_file(source)
        provider.verify_chain()
        self.close_review(app)
        return observed

    def phase_trust(
        self, app: Any, provider: provider_fixtures.LoopbackRevisionServer, source: Path
    ) -> dict[str, Any]:
        self.open_workspace_file(app, source.name, HTML_EDITOR_SOURCE_BYTES)
        self.menu_command(app, MENU_WEB_AUTOMATION_ID)
        self._press(app, TRUST_SCRIPTS_AUTOMATION_ID, "WEB_TRUST")
        # The Run scripts alert is outside the UIA tree; Enter answers it
        # with Run scripts, proven by the Trusted banner.
        self.confirm_system_alert(
            app,
            lambda: not self.name_present(app, TRUSTED_BANNER, "Text"),
            "SCRIPTS_TRUSTED_BEFORE_CONFIRMATION",
        )
        self.wait_name(app, TRUSTED_BANNER, "Text", "SCRIPTS_TRUSTED_TIMEOUT")
        # Leaving the Web pane must hand keyboard ownership back to the main
        # window before the Review shortcut is sent, without a refocus.
        self.menu_command(app, MENU_EDIT_AUTOMATION_ID)
        self.editor_control(app)
        wait_until(
            lambda: self.win32.foreground_focus(app.hwnd) == app.hwnd,
            self.ui_timeout,
            "WEB_TO_SOURCE_NATIVE_FOCUS_TIMEOUT",
        )
        total = self.review_and_revise(
            app, provider, HTML_EDITOR_SOURCE_TEXT, preserve_focus=True
        )
        observed: dict[str, Any] = {
            "trust_proposal_after": fingerprint_text(
                self._named_texts(
                    app,
                    REVISION_PROPOSAL_AFTER_AUTOMATION_ID,
                    REVISION_PROPOSAL_AFTER_NAME,
                    "REVISION_PROPOSAL_AFTER",
                )
            ),
            "trust_editor_proposed": self.editor_fingerprint(app),
        }
        self.decide_hunks(app, [True] * total, total)
        self.done(app, REVISION_APPLIED)
        observed["trust_editor_done"] = self.editor_fingerprint(app)
        self.menu_command(app, MENU_WEB_AUTOMATION_ID)
        self.wait_name(app, RESTRICTED_BANNER, "Text", "SCRIPTS_RESTRICTED_TIMEOUT")
        provider.verify_chain()
        return observed

    def _finalize_observations(
        self,
        provider: provider_fixtures.LoopbackRevisionServer,
        case_root: Path,
        app: Any,
        observations: dict[str, Any],
    ) -> dict[str, Any]:
        """Reap the app before the provider's last check and the privacy scan."""
        self.reap(app)
        provider.verify_chain()
        observations["provider_request_count"] = provider.request_count
        observations["runtime_scan"] = scan_case_artifacts(case_root, self._credential)
        return observations

    def scenario_revision_session(self) -> dict[str, Any]:
        provider = provider_fixtures.LoopbackRevisionServer().start()
        app = None
        try:
            case_root, data, config, workspace, stderr = self.profile(provider.base_url)
            app = self.launch_app(workspace / DECISIONS_FILE, data, config, workspace, stderr)
            observations: dict[str, Any] = {
                "flow": CASE_FLOWS[CASE_REVISION_SESSION],
                "process_context": app.security_context.evidence(),
            }
            phases = (
                (self.phase_decisions, DECISIONS_FILE),
                (self.phase_stale, STALE_FILE),
                (self.phase_trust, TRUST_FILE),
            )
            for (run, name), phase in zip(phases, PHASES, strict=True):
                with phase_failures(phase):
                    observed = run(app, provider, workspace / name)
                observations.update(require_expected(observed))
            observations["foreground_verified"] = self._require_foreground(app)
            # Every tab is dirty: quit through the guard, unsaved.
            with phase_failures("quit"):
                self.quit_via_menu(app)
                self.click_lifecycle_decision(app, "Discard")
                self.wait_process_exit(app)
            return self._finalize_observations(provider, case_root, app, observations)
        finally:
            try:
                if app is not None and app.process.poll() is None:
                    self.reap(app)
            finally:
                provider.close()

    def scenario(self, case_id: str) -> dict[str, Any]:
        observations = {CASE_REVISION_SESSION: self.scenario_revision_session}[case_id]()
        try:
            evidence.validate_case(
                CONTRACT, case_id, observations, self.parent_context.evidence()
            )
        except (KeyError, TypeError, ValueError) as error:
            raise HarnessFailure("CASE_CONTRACT_FAILED", safe_exception_name(error)) from None
        return observations


def native_run_plan() -> NativeRunPlan:
    return NativeRunPlan(
        required_case_ids=REQUIRED_CASE_IDS,
        workdir_prefix="markturbo-goal-07-native-",
        new_evidence=new_evidence,
        validate_evidence=validate_evidence,
        finalize_evidence=finalize_evidence,
        preflight=preflight,
        ui_types_loader=load_pywinauto,
        harness_factory=Goal07Harness,
        scenarios=lambda harness: tuple(
            lambda case_id=case_id: harness.scenario(case_id)
            for case_id in REQUIRED_CASE_IDS
        ),
    )


def parse_args(argv: list[str] | None = None) -> argparse.Namespace:
    parser = evidence.build_parser(__doc__.splitlines()[0], "goal-07", REQUIRED_CASE_IDS)
    return parser.parse_args(argv)


def main(argv: list[str] | None = None) -> int:
    args = parse_args(argv)
    return main_native_acceptance(args, native_run_plan())


if __name__ == "__main__":
    raise SystemExit(main())
