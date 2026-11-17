"""Goal 07 native acceptance harness.

This harness drives the real Windows UI with a deterministic OpenAI Responses
loopback server. Each case has isolated data/config/workspace roots. The
server and evidence boundary never persist document text, request bodies, or
credentials; evidence contains only hashes, byte counts, booleans, and safe
reason codes.

On the nsh UI a Review starts with Ctrl+Enter from the editor, the question is
answered in `review-answer-input`, `Prepare revision` asks for the Revision,
and both sends need a real pointer hold on `consent-send`. The Revision field
decides one docked hunk at a time (`Accept` / `Reject`, counted by `n of m
decided`). Each decision writes at once (`ui/revision.rs`): `Accept` puts its
replacement into the buffer, `Reject` writes nothing, `Undo` takes the last
decision back with exactly its text, and every write first revokes the tab's
Web script trust (`state.rs` `set_revision_change_applied`). `Done` writes
nothing more; undecided hunks are discarded as not applied. A hunk whose part
the user edited says `Can't apply — this part changed` and offers no Accept.
`Copy` puts the approved text on the clipboard; the docked hunk's proposed
lines are the `revision-proposal-after` group labelled `Proposed text`.
Editor text is read with the editor's Select All + Copy; Save runs from the
title menu.

Ctrl+Enter is state-dependent: it starts Review from the title row
(`ui/titlebar.rs`), sends from the consent card (`ui/consent.rs`), and is Done
on the Revision field (`ui/revision.rs`). It is sent only to start a Review;
consent is always a pointer hold. Trust scripts' Run scripts confirmation is
an nsh system alert outside the UIA tree, answered with Enter (`runtime.py`
`confirm_system_alert`).

Evidence follows the shared native schema (`evidence.py`).
"""

from __future__ import annotations

import argparse
import json
import uuid
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
    CONFLICT_HEADING,
    Fingerprint,
    HarnessBlocked,
    HarnessFailure,
    NativeHarness,
    NativeRunPlan,
    complete_evidence as complete_evidence_envelope,
    fingerprint_bytes,
    fingerprint_text,
    load_pywinauto,
    main_native_acceptance,
    preflight,
    require_true,
    safe_exception_name,
    sha256_file,
    validate_fingerprint,
    wait_until,
    write_durable,
)


SCHEMA = "markturbo.goal-07-native-acceptance"
MAX_EDIT_OUTPUT_BYTES = 4 * 1024 * 1024
TRANSPORT_KEYS = frozenset({"mode", "provider", "deterministic", "request_count"})

# `ui/review.rs`: the structured result's first line, the answer input, and
# the Buttons, each carrying its own AutomationId (`prims` actions).
REVIEW_RESULT_MARKER = "Understood:"
REVIEW_ANSWER_INPUT_AUTOMATION_ID = "review-answer-input"
REVIEW_ANSWER_AUTOMATION_ID = "review-answer-submit"
PREPARE_REVISION_AUTOMATION_ID = "review-prepare"
# `ui/consent.rs` `send_button`: only a pointer hold past `HOLD_MS` sends.
CONSENT_SEND_AUTOMATION_ID = "consent-send"
CONSENT_HOLD_SECONDS = 1.8
# `ui/revision.rs`: the docked hunk's decisions, the count, Undo, Copy and
# Done. Accept's Name is checked for absence across the whole window. A decided
# docked hunk shows its decision word until the next undecided one is docked.
REVISION_ACCEPT_NAME = "Accept"
REVISION_ACCEPT_AUTOMATION_ID = "revision-accept"
REVISION_REJECT_AUTOMATION_ID = "revision-reject"
REVISION_UNDO_AUTOMATION_ID = "revision-undo"
REVISION_DONE_AUTOMATION_ID = "revision-done"
REVISION_DECISION_WORDS = ("Accepted", "Rejected")
REVISION_PART_CHANGED_TEXT = "Can't apply \u2014 this part changed"
REVISION_COPY_AUTOMATION_ID = "revision-copy"
REVISION_PROPOSAL_AFTER_AUTOMATION_ID = "revision-proposal-after"
REVISION_PROPOSAL_AFTER_NAME = "Proposed text"
# `state.rs` `finish_revision`: Done's outcome when nothing was accepted.
REVISION_NOTHING_APPROVED = "No changes approved."
# `ui/preview.rs`. `state.rs` `request_web_trust` confirms with an nsh system
# alert outside the UIA tree, answered with Enter (`confirm_system_alert`).
TRUST_SCRIPTS_AUTOMATION_ID = "web-trust"
TRUSTED_BANNER = "Trusted: scripts may run"
RESTRICTED_BANNER = "Restricted: scripts disabled"

CASE_REJECT_ALL = "reject_all_byte_identity"
CASE_SELECTIVE_UNDO = "selective_apply_one_undo"
CASE_ACCEPT_ALL_PREVIEW = "accept_all_preview"
CASE_STALE = "stale_proposal_blocks_apply"
CASE_SAVE_CONFLICT = "safe_save_conflict"
CASE_TRUST_REVOKE = "trusted_executable_apply_revokes_trust"
CASE_FLOWS = {
    CASE_REJECT_ALL: "Review -> Revision -> Reject every hunk -> Done -> exact editor and source identity",
    CASE_SELECTIVE_UNDO: (
        "Review -> Revision -> Accept one hunk writes it -> one Undo restores -> "
        "Accept one, Reject one -> Done"
    ),
    CASE_ACCEPT_ALL_PREVIEW: (
        "Review -> Revision -> Accept every hunk writes it -> Copy approved result -> "
        "Done keeps it exact"
    ),
    CASE_STALE: (
        "Revision -> edit source -> part changed visible -> Accept withheld -> "
        "Done writes nothing"
    ),
    CASE_SAVE_CONFLICT: (
        "Revision -> Accept every hunk -> Done -> external write -> menu Save conflict"
    ),
    CASE_TRUST_REVOKE: "trusted HTML -> Accept executable change -> Done -> Restricted trust",
}
REQUIRED_CASE_IDS = tuple(CASE_FLOWS)

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
EXTERNAL_SOURCE_BYTES = b"external writer won\r\n"

SAFE_STRINGS = frozenset(CASE_FLOWS.values()) | INTEGRITY_NAMES
FINGERPRINT_OBSERVATION_KEYS = frozenset(
    {
        "editor_after",
        "editor_after_apply",
        "editor_after_activation_attempt",
        "editor_after_edit",
        "editor_after_accept_all",
        "editor_after_undo",
        "editor_before",
        "editor_before_apply",
        "executable_expected",
        "external_source_after",
        "external_source_before",
        "accept_all_expected",
        "editor_after_done",
        "preview_fingerprint",
        "proposal_expected",
        "selective_expected",
        "source_after",
        "source_before",
        "copy_editor_after",
        "copy_editor_before",
        "copy_preview_fingerprint",
        "copy_source_after",
        "copy_source_before",
    }
)
PROVIDER_OBSERVATION_KEYS = frozenset(
    {
        "provider_request_count",
        "provider_review_count",
        "provider_revision_count",
        "provider_paths_exact",
        "provider_no_request_before_consent_click",
        "provider_review_before_revision",
        "provider_review_source_sha256_match",
        "provider_revision_source_sha256_match",
        "provider_revision_snapshot_match",
        "provider_revision_answer_sentinel_present",
    }
)
REQUIRED_OBSERVATIONS = {
    CASE_REJECT_ALL: frozenset(
        {
            "proposal_received",
            "reviewed_source_match",
            "editor_before",
            "editor_after",
            "source_before",
            "source_after",
            "reject_all_byte_identity",
            "dirty_before",
            "dirty_after",
            "reject_all_dirty_unchanged",
        }
    ),
    CASE_SELECTIVE_UNDO: frozenset(
        {
            "editor_before",
            "selective_expected",
            "editor_after_apply",
            "editor_after_undo",
            "editor_after_done",
            "source_before",
            "source_after",
            "selective_matches_expected",
            "one_undo_transaction",
        }
    ),
    CASE_ACCEPT_ALL_PREVIEW: frozenset(
        {
            "editor_before_apply",
            "editor_after_accept_all",
            "accept_all_expected",
            "editor_after_done",
            "accept_all_matches_expected",
            "copy_preview_fingerprint",
            "copy_preview_exact",
            "copy_editor_before",
            "copy_editor_after",
            "copy_editor_unchanged",
            "copy_dirty_before",
            "copy_dirty_after",
            "copy_dirty_unchanged",
            "copy_source_before",
            "copy_source_after",
            "copy_source_unchanged",
        }
    ),
    CASE_STALE: frozenset(
        {
            "editor_before",
            "editor_after_edit",
            "source_before",
            "source_after",
            "stale_no_mutation",
            "editor_after_activation_attempt",
            "editor_after_done",
        }
    ),
    CASE_SAVE_CONFLICT: frozenset(
        {
            "editor_after_apply",
            "external_source_before",
            "external_source_after",
            "conflict_visible_before_save",
            "safe_save_no_overwrite",
        }
    ),
    CASE_TRUST_REVOKE: frozenset(
        {
            "editor_before_apply",
            "editor_after_apply",
            "executable_change",
            "executable_expected",
            "executable_matches_expected",
            "preview_fingerprint",
            "proposal_expected",
            "preview_matches_expected",
            "preview_contains_executable_text",
        }
    ),
}
# Every case also records the loopback provider's request contract.
REQUIRED_OBSERVATIONS = {
    case_id: keys | PROVIDER_OBSERVATION_KEYS for case_id, keys in REQUIRED_OBSERVATIONS.items()
}
OBSERVATION_KEYS = frozenset().union(*REQUIRED_OBSERVATIONS.values())


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


def finalize_evidence(evidence: dict[str, Any]) -> None:
    request_count = 0
    for case in evidence.get("cases", ()):
        if not isinstance(case, dict):
            continue
        observations = case.get("observations")
        if not isinstance(observations, dict) or "provider_request_count" not in observations:
            continue
        observed = observations["provider_request_count"]
        if not isinstance(observed, int) or isinstance(observed, bool) or observed < 0:
            raise ValueError("invalid provider request count")
        request_count += observed
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
    cases = evidence.get("cases")
    if isinstance(cases, list):
        observed_request_count = 0
        for case in cases:
            if not isinstance(case, dict):
                continue
            observations = case.get("observations")
            if not isinstance(observations, dict) or "provider_request_count" not in observations:
                continue
            count = observations["provider_request_count"]
            if not isinstance(count, int) or isinstance(count, bool) or count < 0:
                raise ValueError("invalid provider request count")
            observed_request_count += count
        if transport["request_count"] != observed_request_count:
            raise ValueError("top-level transport request count is not synchronized")


def validate_runtime_scan(value: Any) -> None:
    evidence.validate_runtime_scan(value, require_config=True)


def validate_passed_case(case_id: str, observations: dict[str, Any]) -> None:
    require_true(
        observations,
        "provider_paths_exact",
        "provider_no_request_before_consent_click",
        "provider_review_before_revision",
        "provider_review_source_sha256_match",
        "provider_revision_source_sha256_match",
        "provider_revision_snapshot_match",
        "provider_revision_answer_sentinel_present",
    )
    if (
        not isinstance(observations.get("provider_request_count"), int)
        or observations["provider_request_count"] != 2
        or observations.get("provider_review_count") != 1
        or observations.get("provider_revision_count") != 1
    ):
        raise ValueError("loopback provider did not receive exactly one Review and Revision request")

    if case_id == CASE_REJECT_ALL:
        before = validate_fingerprint(observations["editor_before"])
        after = validate_fingerprint(observations["editor_after"])
        source_before = validate_fingerprint(observations["source_before"])
        source_after = validate_fingerprint(observations["source_after"])
        require_true(
            observations,
            "proposal_received",
            "reviewed_source_match",
            "reject_all_byte_identity",
            "reject_all_dirty_unchanged",
        )
        if (
            before != after
            or source_before != source_after
            or observations.get("dirty_before") is not observations.get("dirty_after")
        ):
            raise ValueError("reject-all changed editor or source bytes")
    elif case_id == CASE_SELECTIVE_UNDO:
        before = validate_fingerprint(observations["editor_before"])
        selected = validate_fingerprint(observations["selective_expected"])
        applied = validate_fingerprint(observations["editor_after_apply"])
        undone = validate_fingerprint(observations["editor_after_undo"])
        done = validate_fingerprint(observations["editor_after_done"])
        source_before = validate_fingerprint(observations["source_before"])
        source_after = validate_fingerprint(observations["source_after"])
        require_true(observations, "selective_matches_expected", "one_undo_transaction")
        if (
            applied != selected
            or undone != before
            or done != selected
            or source_before != source_after
        ):
            raise ValueError("selective Accept or one undo did not preserve exact bytes")
    elif case_id == CASE_ACCEPT_ALL_PREVIEW:
        before = validate_fingerprint(observations["editor_before_apply"])
        after = validate_fingerprint(observations["editor_after_accept_all"])
        final = validate_fingerprint(observations["accept_all_expected"])
        done = validate_fingerprint(observations["editor_after_done"])
        copied = validate_fingerprint(observations["copy_preview_fingerprint"])
        copy_before = validate_fingerprint(observations["copy_editor_before"])
        copy_after = validate_fingerprint(observations["copy_editor_after"])
        copy_source_before = validate_fingerprint(observations["copy_source_before"])
        copy_source_after = validate_fingerprint(observations["copy_source_after"])
        require_true(
            observations,
            "accept_all_matches_expected",
            "copy_preview_exact",
            "copy_editor_unchanged",
            "copy_dirty_unchanged",
            "copy_source_unchanged",
        )
        # Each Accept writes at once; Done adds nothing; Copy changes nothing.
        if (
            before == after
            or after != final
            or done != final
            or copied != final
            or copy_before != copy_after
            or copy_source_before != copy_source_after
            or observations.get("copy_dirty_before")
            is not observations.get("copy_dirty_after")
        ):
            raise ValueError("accept-all did not write exactly the approved text")
    elif case_id == CASE_STALE:
        before = validate_fingerprint(observations["editor_before"])
        edited = validate_fingerprint(observations["editor_after_edit"])
        after_observation = validate_fingerprint(observations["editor_after_activation_attempt"])
        after_done = validate_fingerprint(observations["editor_after_done"])
        source_before = validate_fingerprint(observations["source_before"])
        source_after = validate_fingerprint(observations["source_after"])
        require_true(observations, "stale_no_mutation")
        if (
            before == edited
            or edited != after_observation
            or edited != after_done
            or source_before != source_after
        ):
            raise ValueError("stale proposal did not remain fail-closed")
    elif case_id == CASE_SAVE_CONFLICT:
        before = validate_fingerprint(observations["external_source_before"])
        after = validate_fingerprint(observations["external_source_after"])
        require_true(observations, "safe_save_no_overwrite")
        if before != after:
            raise ValueError("safe-save conflict overwrote the external source")
        validate_fingerprint(observations["editor_after_apply"])
    elif case_id == CASE_TRUST_REVOKE:
        before = validate_fingerprint(observations["editor_before_apply"])
        actual = validate_fingerprint(observations["editor_after_apply"])
        expected = validate_fingerprint(observations["executable_expected"])
        preview = validate_fingerprint(observations["preview_fingerprint"])
        proposal = validate_fingerprint(observations["proposal_expected"])
        require_true(
            observations,
            "executable_change",
            "executable_matches_expected",
            "preview_matches_expected",
            "preview_contains_executable_text",
        )
        if preview != proposal or actual != expected or before == actual:
            raise ValueError("trusted executable Apply did not produce the expected restricted change")


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


class Goal07Harness(NativeHarness):
    def __init__(self, *args: Any) -> None:
        super().__init__(*args)
        self._credential = f"markturbo-goal07-{uuid.uuid4().hex}"

    def openai_api_key_for_child(self) -> str | None:
        return self._credential

    def profile(
        self, case_id: str, endpoint: str, *, html: bool = False
    ) -> tuple[Path, Path, Path, Path, Path, Path]:
        target = credential_target(endpoint, "openai-responses")
        if self.win32.persistent_credential_target_exists(target):
            raise HarnessBlocked("PERSISTENT_CREDENTIAL_PRESENT")
        data_root, config_root, workspace_root, stderr_path = self.case_roots(case_id)
        case_root = data_root.parent
        source = (workspace_root / ("trusted.html" if html else "revision.md")).resolve()
        write_durable(source, HTML_SOURCE_BYTES if html else SOURCE_BYTES)
        write_durable(config_root / "settings.toml", review_settings_document(endpoint))
        return case_root, data_root, config_root, workspace_root, stderr_path, source

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
        # Type through the focused input, the same path a user takes.
        answer = self.find_control(
            app,
            REVIEW_ANSWER_INPUT_AUTOMATION_ID,
            "Edit",
            "REVIEW_ANSWER_UIA_TIMEOUT",
            "REVIEW_ANSWER_UIA_CONTRACT_MISMATCH",
        )
        try:
            answer.set_focus()
        except Exception as error:
            raise HarnessFailure("REVIEW_ANSWER_FOCUS_FAILED", safe_exception_name(error)) from None
        self.win32.send_unicode(app.hwnd, value)

        def typed() -> bool:
            control = self.control_by_id(
                app.hwnd,
                REVIEW_ANSWER_INPUT_AUTOMATION_ID,
                "Edit",
                "REVIEW_ANSWER_UIA_CONTRACT_MISMATCH",
            )
            return control is not None and control.iface_value.CurrentValue == value

        wait_until(typed, self.ui_timeout, "REVIEW_ANSWER_INPUT_TIMEOUT", interval=0.025)
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

    def decide_hunks(self, app: Any, decisions: list[bool], total: int | None = None) -> None:
        """Decide the docked hunk, in order, from none decided out of `total`.

        Each decision is waited for by its count. While hunks remain undecided
        the field keeps the decided one docked (showing `Accepted`/`Rejected`)
        until it brings the next undecided one forward, so the next press waits
        for that word to go: a press in between would decide the same hunk
        again.
        """
        total = len(decisions) if total is None else total
        for index, accepted in enumerate(decisions):
            automation_id = (
                REVISION_ACCEPT_AUTOMATION_ID if accepted else REVISION_REJECT_AUTOMATION_ID
            )
            self._press(app, automation_id, "REVISION_ACCEPT" if accepted else "REVISION_REJECT")
            self.wait_name(
                app, f"{index + 1} of {total} decided", "Text", "REVISION_DECISION_TIMEOUT"
            )
            if index + 1 < total:
                wait_until(
                    lambda: not any(
                        self.name_present(app, word, "Text") for word in REVISION_DECISION_WORDS
                    ),
                    self.ui_timeout,
                    "REVISION_NEXT_HUNK_TIMEOUT",
                    interval=0.05,
                )

    def undo_decision(self, app: Any, decided_after: int, total: int) -> None:
        """The field's Undo: the last decision goes back with its text."""
        self._press(app, REVISION_UNDO_AUTOMATION_ID, "REVISION_UNDO")
        self.wait_name(
            app, f"{decided_after} of {total} decided", "Text", "REVISION_UNDO_TIMEOUT"
        )

    def done(self, app: Any) -> None:
        self._press(app, REVISION_DONE_AUTOMATION_ID, "REVISION_DONE")

    def _common_observations(
        self, provider: provider_fixtures.LoopbackRevisionServer
    ) -> dict[str, Any]:
        return provider.contract_evidence()

    def _finalize_observations(
        self,
        provider: provider_fixtures.LoopbackRevisionServer,
        case_root: Path,
        app: Any,
        observations: dict[str, Any],
    ) -> dict[str, Any]:
        """Reap the app before the final privacy scan and provider close."""
        self.reap(app)
        observations.update(self._common_observations(provider))
        observations["runtime_scan"] = scan_case_artifacts(case_root, self._credential)
        return observations

    def _expected(self, source: bytes, edits: list[dict[str, Any]]) -> Fingerprint:
        return fingerprint_bytes(apply_edits(source, edits))

    def scenario_reject_all(self) -> dict[str, Any]:
        provider = provider_fixtures.LoopbackRevisionServer(EDITOR_SOURCE_TEXT).start()
        app = None
        try:
            case_root, data, config, workspace, stderr, source = self.profile(
                CASE_REJECT_ALL, provider.base_url
            )
            hunks = len(provider_fixtures.fixture_edits(EDITOR_SOURCE_TEXT))
            app = self.launch_app(source, data, config, workspace, stderr)
            self.activate_source_layout(app)
            editor_before = self.editor_fingerprint(app)
            source_before = sha256_file(source)
            dirty_before = self.document_dirty(app)
            self.run_review(app, provider)
            self.request_revision(app, provider=provider, hunks=hunks)
            self.decide_hunks(app, [False] * hunks)
            self.done(app)
            editor_after = self.editor_fingerprint(app)
            source_after = sha256_file(source)
            dirty_after = self.document_dirty(app)
            observations = {
                "flow": CASE_FLOWS[CASE_REJECT_ALL],
                "process_context": app.security_context.evidence(),
                "foreground_verified": self._require_foreground(app),
                "proposal_received": hunks > 0,
                "reviewed_source_match": editor_before
                == fingerprint_bytes(EDITOR_SOURCE_BYTES),
                "editor_before": editor_before.evidence(),
                "editor_after": editor_after.evidence(),
                "source_before": source_before.evidence(),
                "source_after": source_after.evidence(),
                "reject_all_byte_identity": (
                    editor_before == editor_after and source_before == source_after
                ),
                "dirty_before": dirty_before,
                "dirty_after": dirty_after,
                "reject_all_dirty_unchanged": dirty_before == dirty_after,
            }
            return self._finalize_observations(provider, case_root, app, observations)
        finally:
            try:
                if app is not None and app.process.poll() is None:
                    self.reap(app)
            finally:
                provider.close()

    def scenario_selective_undo(self) -> dict[str, Any]:
        provider = provider_fixtures.LoopbackRevisionServer(EDITOR_SOURCE_TEXT).start()
        app = None
        try:
            case_root, data, config, workspace, stderr, source = self.profile(
                CASE_SELECTIVE_UNDO, provider.base_url
            )
            edits = provider_fixtures.fixture_edits(EDITOR_SOURCE_TEXT)
            app = self.launch_app(source, data, config, workspace, stderr)
            self.activate_source_layout(app)
            editor_before = self.editor_fingerprint(app)
            source_before = sha256_file(source)
            self.run_review(app, provider)
            self.request_revision(app, provider=provider, hunks=len(edits))
            expected = self._expected(EDITOR_SOURCE_BYTES, edits[:1])
            # Accept writes the first hunk at once; one Undo takes exactly it back.
            self.decide_hunks(app, [True], total=len(edits))
            editor_after_apply = self.editor_fingerprint(app)
            self.undo_decision(app, 0, len(edits))
            editor_after_undo = self.editor_fingerprint(app)
            # Decide again, the second hunk rejected; Done writes nothing more.
            self.decide_hunks(app, [True] + [False] * (len(edits) - 1))
            self.done(app)
            editor_after_done = self.editor_fingerprint(app)
            source_after = sha256_file(source)
            observations = {
                "flow": CASE_FLOWS[CASE_SELECTIVE_UNDO],
                "process_context": app.security_context.evidence(),
                "foreground_verified": self._require_foreground(app),
                "editor_before": editor_before.evidence(),
                "selective_expected": expected.evidence(),
                "editor_after_apply": editor_after_apply.evidence(),
                "editor_after_undo": editor_after_undo.evidence(),
                "editor_after_done": editor_after_done.evidence(),
                "source_before": source_before.evidence(),
                "source_after": source_after.evidence(),
                "selective_matches_expected": (
                    editor_after_apply == expected and editor_after_done == expected
                ),
                "one_undo_transaction": editor_after_undo == editor_before,
            }
            return self._finalize_observations(provider, case_root, app, observations)
        finally:
            try:
                if app is not None and app.process.poll() is None:
                    self.reap(app)
            finally:
                provider.close()

    def scenario_accept_all_preview(self) -> dict[str, Any]:
        provider = provider_fixtures.LoopbackRevisionServer(EDITOR_SOURCE_TEXT).start()
        app = None
        try:
            case_root, data, config, workspace, stderr, source = self.profile(
                CASE_ACCEPT_ALL_PREVIEW, provider.base_url
            )
            edits = provider_fixtures.fixture_edits(EDITOR_SOURCE_TEXT)
            app = self.launch_app(source, data, config, workspace, stderr)
            self.activate_source_layout(app)
            self.run_review(app, provider)
            self.request_revision(app, provider=provider, hunks=len(edits))
            editor_before = self.editor_fingerprint(app)
            expected_bytes = apply_edits(EDITOR_SOURCE_BYTES, edits)
            expected = fingerprint_bytes(expected_bytes)
            # Every Accept writes at once: the buffer is the approved text now.
            self.decide_hunks(app, [True] * len(edits))
            editor_after_accept_all = self.editor_fingerprint(app)
            copy_editor_before = editor_after_accept_all
            copy_dirty_before = self.document_dirty(app)
            copy_source_before = sha256_file(source)
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
                copied = self.wait_clipboard_text("REVISION_COPY_CLIPBOARD_TIMEOUT")
            finally:
                self.write_text_clipboard(clipboard_before)
            copy_editor_after = self.editor_fingerprint(app)
            copy_dirty_after = self.document_dirty(app)
            copy_source_after = sha256_file(source)
            self.done(app)
            editor_after_done = self.editor_fingerprint(app)
            observations = {
                "flow": CASE_FLOWS[CASE_ACCEPT_ALL_PREVIEW],
                "process_context": app.security_context.evidence(),
                "foreground_verified": self._require_foreground(app),
                "editor_before_apply": editor_before.evidence(),
                "editor_after_accept_all": editor_after_accept_all.evidence(),
                "accept_all_expected": expected.evidence(),
                "editor_after_done": editor_after_done.evidence(),
                "accept_all_matches_expected": (
                    editor_after_accept_all == expected and editor_after_done == expected
                ),
                "copy_preview_fingerprint": fingerprint_text(copied).evidence(),
                "copy_preview_exact": copied.encode("utf-8") == expected_bytes,
                "copy_editor_before": copy_editor_before.evidence(),
                "copy_editor_after": copy_editor_after.evidence(),
                "copy_editor_unchanged": copy_editor_before == copy_editor_after,
                "copy_dirty_before": copy_dirty_before,
                "copy_dirty_after": copy_dirty_after,
                "copy_dirty_unchanged": copy_dirty_before == copy_dirty_after,
                "copy_source_before": copy_source_before.evidence(),
                "copy_source_after": copy_source_after.evidence(),
                "copy_source_unchanged": copy_source_before == copy_source_after,
            }
            return self._finalize_observations(provider, case_root, app, observations)
        finally:
            try:
                if app is not None and app.process.poll() is None:
                    self.reap(app)
            finally:
                provider.close()

    def scenario_stale(self) -> dict[str, Any]:
        provider = provider_fixtures.LoopbackRevisionServer(EDITOR_SOURCE_TEXT).start()
        app = None
        try:
            case_root, data, config, workspace, stderr, source = self.profile(
                CASE_STALE, provider.base_url
            )
            hunks = len(provider_fixtures.fixture_edits(EDITOR_SOURCE_TEXT))
            app = self.launch_app(source, data, config, workspace, stderr)
            self.activate_source_layout(app)
            self.run_review(app, provider)
            self.request_revision(app, provider=provider, hunks=hunks)
            editor_before = self.editor_fingerprint(app)
            source_before = sha256_file(source)
            # One paste over the whole text touches every hunk's part.
            self.paste_editor(app, STALE_EDIT_TEXT)
            editor_after_edit = self.editor_fingerprint(app)
            self.wait_name(
                app, REVISION_PART_CHANGED_TEXT, "Text", "REVISION_PART_CHANGED_UIA_TIMEOUT"
            )
            if self.name_present(app, REVISION_ACCEPT_NAME, "Button"):
                raise HarnessFailure("STALE_REVISION_OFFERS_ACCEPT")
            # The A key still reaches the field once no text input holds the
            # keyboard: move the focus to Done, then press it.
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
                raise HarnessFailure(
                    "REVISION_DONE_FOCUS_FAILED", safe_exception_name(error)
                ) from None
            wait_until(
                lambda: not self.source_editor_has_focus(app),
                self.ui_timeout,
                "REVISION_FIELD_FOCUS_TIMEOUT",
                interval=0.025,
            )
            self.win32.send_key(app.hwnd, VK_A)
            self.wait_name(app, f"0 of {hunks} decided", "Text", "REVISION_DECISION_TIMEOUT")
            editor_after_activation_attempt = self.editor_fingerprint(app)
            self.done(app)
            self.wait_name(app, REVISION_NOTHING_APPROVED, "Text", "REVISION_DONE_OUTCOME_TIMEOUT")
            editor_after_done = self.editor_fingerprint(app)
            source_after = sha256_file(source)
            observations = {
                "flow": CASE_FLOWS[CASE_STALE],
                "process_context": app.security_context.evidence(),
                "foreground_verified": self._require_foreground(app),
                "editor_before": editor_before.evidence(),
                "editor_after_edit": editor_after_edit.evidence(),
                "source_before": source_before.evidence(),
                "source_after": source_after.evidence(),
                "editor_after_activation_attempt": editor_after_activation_attempt.evidence(),
                "editor_after_done": editor_after_done.evidence(),
                "stale_no_mutation": (
                    source_before == source_after
                    and editor_after_edit == editor_after_activation_attempt
                    and editor_after_edit == editor_after_done
                ),
            }
            return self._finalize_observations(provider, case_root, app, observations)
        finally:
            try:
                if app is not None and app.process.poll() is None:
                    self.reap(app)
            finally:
                provider.close()

    def scenario_save_conflict(self) -> dict[str, Any]:
        provider = provider_fixtures.LoopbackRevisionServer(EDITOR_SOURCE_TEXT).start()
        app = None
        try:
            case_root, data, config, workspace, stderr, source = self.profile(
                CASE_SAVE_CONFLICT, provider.base_url
            )
            hunks = len(provider_fixtures.fixture_edits(EDITOR_SOURCE_TEXT))
            app = self.launch_app(source, data, config, workspace, stderr)
            self.activate_source_layout(app)
            self.run_review(app, provider)
            self.request_revision(app, provider=provider, hunks=hunks)
            self.decide_hunks(app, [True] * hunks)
            self.done(app)
            editor_after_apply = self.editor_fingerprint(app)
            write_durable(source, EXTERNAL_SOURCE_BYTES)
            external_source_before = sha256_file(source)
            conflict_visible_before_save = self.name_present(app, CONFLICT_HEADING, "Text")
            self.save_via_menu(app)
            self.wait_name(app, CONFLICT_HEADING, "Text", "SAVE_CONFLICT_UIA_TIMEOUT")
            external_source_after = sha256_file(source)
            observations = {
                "flow": CASE_FLOWS[CASE_SAVE_CONFLICT],
                "process_context": app.security_context.evidence(),
                "foreground_verified": self._require_foreground(app),
                "editor_after_apply": editor_after_apply.evidence(),
                "external_source_before": external_source_before.evidence(),
                "external_source_after": external_source_after.evidence(),
                "conflict_visible_before_save": conflict_visible_before_save,
                "safe_save_no_overwrite": external_source_before == external_source_after,
            }
            return self._finalize_observations(provider, case_root, app, observations)
        finally:
            try:
                if app is not None and app.process.poll() is None:
                    self.reap(app)
            finally:
                provider.close()

    def scenario_trust_revoke(self) -> dict[str, Any]:
        provider = provider_fixtures.LoopbackRevisionServer(HTML_EDITOR_SOURCE_TEXT).start()
        app = None
        try:
            case_root, data, config, workspace, stderr, source = self.profile(
                CASE_TRUST_REVOKE, provider.base_url, html=True
            )
            edits = provider_fixtures.fixture_edits(HTML_EDITOR_SOURCE_TEXT)
            app = self.launch_app(source, data, config, workspace, stderr)
            self.menu_command(app, MENU_WEB_AUTOMATION_ID)
            self._press(app, TRUST_SCRIPTS_AUTOMATION_ID, "TRUST_BUTTON")
            # The Run scripts alert is outside the UIA tree; Enter answers it
            # with Run scripts, proven by the Trusted banner.
            self.confirm_system_alert(
                app,
                lambda: not self.name_present(app, TRUSTED_BANNER, "Text"),
                "TRUST_ENABLED_BEFORE_CONFIRMATION",
            )
            self.wait_name(app, TRUSTED_BANNER, "Text", "TRUST_ENABLE_STATE_TIMEOUT")
            # Leaving the Web pane must hand keyboard ownership back to the
            # main window before the Review shortcut is sent.
            self.menu_command(app, MENU_EDIT_AUTOMATION_ID)
            self.editor_control(app)
            wait_until(
                lambda: self.win32.foreground_focus(app.hwnd) == app.hwnd,
                self.ui_timeout,
                "WEB_TO_SOURCE_NATIVE_FOCUS_TIMEOUT",
            )
            self.run_review(app, provider, preserve_focus=True)
            self.request_revision(app, provider=provider, hunks=len(edits))
            preview_text = self._named_texts(
                app,
                REVISION_PROPOSAL_AFTER_AUTOMATION_ID,
                REVISION_PROPOSAL_AFTER_NAME,
                "REVISION_PROPOSAL_AFTER",
            )
            preview = fingerprint_text(preview_text)
            expected_bytes = apply_edits(HTML_EDITOR_SOURCE_BYTES, edits)
            expected = fingerprint_bytes(expected_bytes)
            proposal = fingerprint_text(
                next(
                    line
                    for line in expected_bytes.decode("utf-8").split("\n")
                    if edits[0]["replacement"] in line
                )
            )
            editor_before_apply = self.editor_fingerprint(app)
            self.decide_hunks(app, [True] * len(edits))
            self.done(app)
            editor_after_apply = self.editor_fingerprint(app)
            self.menu_command(app, MENU_WEB_AUTOMATION_ID)
            self.wait_name(app, RESTRICTED_BANNER, "Text", "TRUST_RESTRICTED_STATE_TIMEOUT")
            observations = {
                "flow": CASE_FLOWS[CASE_TRUST_REVOKE],
                "process_context": app.security_context.evidence(),
                "foreground_verified": self._require_foreground(app),
                "editor_before_apply": editor_before_apply.evidence(),
                "editor_after_apply": editor_after_apply.evidence(),
                "executable_expected": expected.evidence(),
                "executable_matches_expected": editor_after_apply == expected,
                "executable_change": editor_before_apply != editor_after_apply,
                "preview_fingerprint": preview.evidence(),
                "proposal_expected": proposal.evidence(),
                "preview_matches_expected": preview == proposal,
                "preview_contains_executable_text": (
                    "<script>" in preview_text and 'goal07 = "new"' in preview_text
                ),
            }
            # The applied buffer is dirty: quit through the guard, unsaved.
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
        observations = {
            CASE_REJECT_ALL: self.scenario_reject_all,
            CASE_SELECTIVE_UNDO: self.scenario_selective_undo,
            CASE_ACCEPT_ALL_PREVIEW: self.scenario_accept_all_preview,
            CASE_STALE: self.scenario_stale,
            CASE_SAVE_CONFLICT: self.scenario_save_conflict,
            CASE_TRUST_REVOKE: self.scenario_trust_revoke,
        }[case_id]()
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
