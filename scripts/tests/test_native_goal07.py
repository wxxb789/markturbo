"""Unit tests for the Goal 07 native harness without launching a UI."""

from __future__ import annotations

import argparse
import hashlib
import json
import tempfile
import unittest
from pathlib import Path
from typing import Callable
from unittest import mock

from scripts.markturbo_tools.native import goal07 as HARNESS
from scripts.markturbo_tools.native import goal07_provider as PROVIDER
from scripts.markturbo_tools.native import runtime
from .test_native_goal02_runtime import OwnedKernel, launch_owned, owned_harness


HASH = "a" * 64


def fingerprint(value: bytes) -> dict[str, int | str]:
    return {"byte_count": len(value), "sha256": hashlib.sha256(value).hexdigest()}


def process_context() -> dict[str, int | str]:
    return {"session_id": 7, "integrity_rid": 0x2000, "integrity": "medium"}


def runtime_scan() -> dict[str, int | bool]:
    return {
        "files_scanned": 3,
        "app_logs_scanned": 1,
        "config_files_scanned": 1,
        "utf8_sentinel_absent": True,
        "utf16le_sentinel_absent": True,
        "ephemeral_credential_utf8_absent": True,
        "ephemeral_credential_utf16le_absent": True,
        "answer_sentinel_utf8_absent": True,
        "answer_sentinel_utf16le_absent": True,
        "raw_response_sentinel_utf8_absent": True,
        "raw_response_sentinel_utf16le_absent": True,
    }


def environment() -> dict[str, int | str]:
    return {
        "platform": "Windows 11",
        "windows_major": 10,
        "windows_minor": 0,
        "windows_build": 22631,
        "architecture": "x86_64",
        "native_machine_code": 0x8664,
        "python_pointer_bits": 64,
        "wts_state": "WTSActive",
        "active_console_session_id": 7,
        "harness_is_console_session": True,
        "input_desktop": "Default",
        "thread_desktop": "Default",
        "harness_process": process_context(),
    }


def common(case_id: str) -> dict:
    editor_same = fingerprint(HARNESS.EDITOR_SOURCE_BYTES)
    disk_same = fingerprint(HARNESS.SOURCE_BYTES)
    observations = {
        "flow": HARNESS.CASE_FLOWS[case_id],
        "process_context": process_context(),
        "foreground_verified": True,
        "loopback_provider": True,
        "server_loopback": True,
        "server_deterministic": True,
        "provider_request_count": 2,
        "provider_review_count": 1,
        "provider_revision_count": 1,
        "provider_paths_exact": True,
        "provider_no_request_before_consent_click": True,
        "provider_review_before_revision": True,
        "provider_review_source_sha256_match": True,
        "provider_revision_source_sha256_match": True,
        "provider_revision_snapshot_match": True,
        "provider_revision_answer_sentinel_present": True,
        "runtime_scan": runtime_scan(),
    }
    if case_id == HARNESS.CASE_REJECT_ALL:
        observations.update(
            proposal_received=True,
            reviewed_source_match=True,
            editor_before=editor_same,
            editor_after=editor_same,
            source_before=disk_same,
            source_after=disk_same,
            reject_all_byte_identity=True,
            dirty_before=False,
            dirty_after=False,
            reject_all_dirty_unchanged=True,
        )
    elif case_id == HARNESS.CASE_SELECTIVE_UNDO:
        selected = fingerprint(
            HARNESS.apply_edits(
                HARNESS.EDITOR_SOURCE_BYTES,
                PROVIDER.fixture_edits(HARNESS.EDITOR_SOURCE_TEXT)[:1],
            )
        )
        observations.update(
            editor_before=editor_same,
            selective_preview=selected,
            editor_after_apply=selected,
            editor_after_undo=editor_same,
            source_before=disk_same,
            source_after=disk_same,
            selective_matches_editor=True,
            selective_matches_expected=True,
            one_undo_transaction=True,
            undo_count=1,
        )
    elif case_id == HARNESS.CASE_ACCEPT_ALL_PREVIEW:
        final = fingerprint(
            HARNESS.apply_edits(
                HARNESS.EDITOR_SOURCE_BYTES,
                PROVIDER.fixture_edits(HARNESS.EDITOR_SOURCE_TEXT),
            )
        )
        observations.update(
            editor_before_apply=editor_same,
            editor_after_preview=editor_same,
            preview_fingerprint=final,
            final_preview=final,
            accept_all_matches_expected=True,
            copy_preview_fingerprint=final,
            copy_preview_exact=True,
            copy_editor_before=editor_same,
            copy_editor_after=editor_same,
            copy_editor_unchanged=True,
            copy_dirty_before=False,
            copy_dirty_after=False,
            copy_dirty_unchanged=True,
            copy_source_before=disk_same,
            copy_source_after=disk_same,
            copy_source_unchanged=True,
        )
    elif case_id == HARNESS.CASE_STALE:
        edited = fingerprint(HARNESS.STALE_EDIT_TEXT.encode("utf-8"))
        observations.update(
            editor_before=editor_same,
            editor_after_edit=edited,
            source_before=disk_same,
            source_after=disk_same,
            stale_proposal_received=True,
            stale_visible=True,
            apply_activation_attempted=True,
            apply_disabled_source_contract=True,
            stale_no_mutation=True,
            apply_control_observed=True,
            apply_accessibility_reported_enabled=True,
            editor_after_activation_attempt=edited,
        )
    elif case_id == HARNESS.CASE_SAVE_CONFLICT:
        external = fingerprint(HARNESS.EXTERNAL_SOURCE_BYTES)
        observations.update(
            editor_after_apply=fingerprint(
                HARNESS.apply_edits(
                    HARNESS.EDITOR_SOURCE_BYTES,
                    PROVIDER.fixture_edits(HARNESS.EDITOR_SOURCE_TEXT),
                )
            ),
            external_source_before=external,
            external_source_after=external,
            save_shortcut_sent=True,
            conflict_visible_before_save=False,
            safe_save_conflict_visible=True,
            safe_save_no_overwrite=True,
        )
    elif case_id == HARNESS.CASE_TRUST_REVOKE:
        observations.update(
            trust_before=True,
            editor_before_apply=fingerprint(HARNESS.HTML_EDITOR_SOURCE_BYTES),
            editor_after_apply=fingerprint(
                HARNESS.apply_edits(
                    HARNESS.HTML_EDITOR_SOURCE_BYTES,
                    PROVIDER.fixture_edits(HARNESS.HTML_EDITOR_SOURCE_TEXT),
                )
            ),
            executable_expected=fingerprint(
                HARNESS.apply_edits(
                    HARNESS.HTML_EDITOR_SOURCE_BYTES,
                    PROVIDER.fixture_edits(HARNESS.HTML_EDITOR_SOURCE_TEXT),
                )
            ),
            executable_matches_expected=True,
            restricted_after_apply=True,
            executable_change=True,
            trust_revocation_order_source_contract=True,
            preview_inert_source_contract=True,
            preview_fingerprint=fingerprint(
                HARNESS.apply_edits(
                    HARNESS.HTML_EDITOR_SOURCE_BYTES,
                    PROVIDER.fixture_edits(HARNESS.HTML_EDITOR_SOURCE_TEXT),
                )
            ),
            preview_matches_expected=True,
            preview_contains_executable_text=True,
        )
    return observations


def valid_evidence() -> dict:
    evidence = HARNESS.new_evidence(HASH)
    evidence["transport"]["request_count"] = len(HARNESS.REQUIRED_CASE_IDS) * 2
    evidence["executable"].update(
        {
            "sha256": HASH,
            "byte_count": 1234,
            "hash_verified": True,
            "copied_sha256": HASH,
            "copy_hash_verified": True,
            "format": "PE32+",
            "machine": "x86_64",
            "machine_code": 0x8664,
            "optional_magic": 0x20B,
        }
    )
    evidence["environment"] = environment()
    for case in evidence["cases"]:
        case.update(status="PASS", duration_ms=1.0, observations=common(case["id"]))
    HARNESS.complete_evidence(evidence, "PASS")
    return evidence


def _trust_contract_comment_decoy() -> str:
    return """\
        /* Historical implementation excerpt; this code is intentionally inert.
        let revoke_trust = self.trust == Trust::Trusted
            && matches!(self.document.doc_type(),
                DocType::Html | DocType::Mdx)
            && current_text != final_text;
        if revoke_trust {
            self.trust = Trust::Restricted;
            self.preview.trust_changed(Trust::Restricted);
        }
        self.replace_text(final_text, window, cx);
        */
"""


def _write_goal07_source_fixture(
    root: Path,
    *,
    trust_body: str | None = None,
    active_preview: bool = False,
    test_only_trust_markers: bool = False,
    test_only_preview_markers: bool = False,
) -> None:
    """Write the small Rust-source surface consumed by Goal07's checkers."""
    views = root / "crates" / "mt-app" / "src" / "views"
    workspace = views / "workspace.rs"
    review = views / "workspace" / "review.rs"
    document = views / "document.rs"
    preview = views / "document" / "preview.rs"
    for path in (workspace, review, document, preview):
        path.parent.mkdir(parents=True, exist_ok=True)

    workspace_symbols = (
        ("REVIEW_RUN_ACCESSIBILITY_ID", HARNESS.REVIEW_RUN_ACCESSIBILITY_ID),
        ("REVIEW_RESULT_ACCESSIBILITY_ID", HARNESS.REVIEW_RESULT_ACCESSIBILITY_ID),
        ("REVISION_RUN_ACCESSIBILITY_ID", HARNESS.REVISION_RUN_ACCESSIBILITY_ID),
        ("REVISION_STALE_ACCESSIBILITY_ID", HARNESS.REVISION_STALE_ACCESSIBILITY_ID),
        (
            "REVISION_ACCEPT_ALL_ACCESSIBILITY_ID",
            HARNESS.REVISION_ACCEPT_ALL_ACCESSIBILITY_ID,
        ),
        (
            "REVISION_REJECT_ALL_ACCESSIBILITY_ID",
            HARNESS.REVISION_REJECT_ALL_ACCESSIBILITY_ID,
        ),
        ("REVISION_APPLY_ACCESSIBILITY_ID", HARNESS.REVISION_APPLY_ACCESSIBILITY_ID),
        ("REVISION_COPY_ACCESSIBILITY_ID", HARNESS.REVISION_COPY_ACCESSIBILITY_ID),
        (
            "REVISION_RESULT_DISMISS_ACCESSIBILITY_ID",
            HARNESS.REVISION_RESULT_DISMISS_ACCESSIBILITY_ID,
        ),
        (
            "REVISION_DISCARD_ANSWERS_ACCESSIBILITY_ID",
            HARNESS.REVISION_DISCARD_ANSWERS_ACCESSIBILITY_ID,
        ),
    )
    workspace.write_text(
        "\n".join(
            f'const {symbol}: &str = "{value}";'
            for symbol, value in workspace_symbols
        )
        + "\nDocumentEvent::Conflict\n",
        encoding="utf-8",
    )

    if trust_body is None:
        trust_body = """\
        let revoke_trust = self.trust == Trust::Trusted
            && matches!(self.document.doc_type(),
                DocType::Html | DocType::Mdx)
            && current_text != final_text;
        if revoke_trust {
            self.trust = Trust::Restricted;
            self.preview.trust_changed(Trust::Restricted);
        }
        self.replace_text(final_text, window, cx);
"""
    document_source = f'''\
Button::new("trust");
accessibility_id(DOCUMENT_TRUST_ACCESSIBILITY_ID);
const TRUST_ID: &str = "{HARNESS.TRUST_AUTOMATION_ID}";
accessibility_id(CONFLICT_OVERWRITE_ACCESSIBILITY_ID);
const CONFLICT_OVERWRITE_ID: &str = "{HARNESS.CONFLICT_OVERWRITE_ACCESSIBILITY_ID}";
pub fn apply_approved_revision() {{
{trust_body}}}
'''
    if test_only_trust_markers:
        document_source += f'''\
#[cfg(test)]
mod tests {{
    const TRUST_MARKER_DECOY: &str = {json.dumps(_trust_contract_comment_decoy())};
}}
'''
    document.write_text(document_source, encoding="utf-8")

    preview_region = f'''\
.id("revision-preview")
.role(gpui::Role::Group)
.accessibility_id("{HARNESS.REVISION_PREVIEW_ACCESSIBILITY_ID}")
.id("revision-preview-source")
.role(gpui::Role::Label)
.aria_value(preview.clone())
.accessibility_id("{HARNESS.REVISION_PREVIEW_SOURCE_ACCESSIBILITY_ID}")
'''
    if active_preview:
        preview_region += ".child(WebSurface::new(preview.clone()))\n"
    preview_region += ".child(preview)\n.into_any_element()\n"

    review_symbols = tuple(symbol for symbol, _value in workspace_symbols)
    review_accessibility = "\n".join(
        f"accessibility_id({symbol});" for symbol in review_symbols
    )
    review_values = "\n".join(
        f'const REVIEW_UIA_VALUE_{index}: &str = "{value}";'
        for index, value in enumerate(
            (
                HARNESS.REVISION_RESULT_ACCESSIBILITY_ID,
                HARNESS.REVISION_PREVIEW_ACCESSIBILITY_ID,
                HARNESS.REVISION_PREVIEW_SOURCE_ACCESSIBILITY_ID,
                HARNESS.REVISION_QUESTION_PREFIX,
                HARNESS.REVISION_CHANGE_PREFIX,
            )
        )
    )
    review_source = f'''\
{review_accessibility}
{review_values}
'''
    if not test_only_preview_markers:
        review_source += preview_region
    else:
        review_source += (
            "// Preview IDs are also used by test fixtures: "
            f"{HARNESS.REVISION_PREVIEW_ACCESSIBILITY_ID} "
            f"{HARNESS.REVISION_PREVIEW_SOURCE_ACCESSIBILITY_ID}\n"
        )
    review_source += f'''\
.id("revision-stale")
accessibility_id(REVISION_STALE_ACCESSIBILITY_ID)
.role(gpui::Role::Label)
.aria_label(i18n::t(i18n::Key::RevisionStaleInspection, cx))
.into_any_element()
Button::new("revision-apply")
accessibility_id(REVISION_APPLY_ACCESSIBILITY_ID)
.disabled(revision_stale)
this.apply_revision(window, cx)
Button::new("revision-save")
'''
    if test_only_preview_markers:
        review_source += f'''\
#[cfg(test)]
mod tests {{
    fn preview_marker_decoy() {{
{preview_region}    }}
}}
'''
    review.write_text(review_source, encoding="utf-8")

    (views / "document").mkdir(parents=True, exist_ok=True)
    preview.write_text(
        """\
pub(super) fn trust_changed() {
    self.rebuild_web(document, source_path, trust, cx);
}
fn rebuild_web() {
    self.web_revision = self.web_revision.wrapping_add(1);
    self.web_html = Some(match trust {
        Trust::Restricted => web::build_html_raw(document, trust),
        _ => web::build_html_themed(document, trust),
    });
}
""",
        encoding="utf-8",
    )


class EvidenceTests(unittest.TestCase):
    def test_complete_pass_is_hash_bound_and_requires_all_cases(self) -> None:
        evidence = valid_evidence()
        HARNESS.validate_evidence(evidence)
        self.assertEqual(evidence["status"], "PASS")
        self.assertEqual(
            evidence["summary"]["passed_case_count"], len(HARNESS.REQUIRED_CASE_IDS)
        )

    def test_reject_all_requires_exact_byte_identity(self) -> None:
        evidence = valid_evidence()
        evidence["cases"][0]["observations"]["editor_after"] = fingerprint(b"changed")
        with self.assertRaisesRegex(ValueError, "reject-all changed"):
            HARNESS.validate_evidence(evidence)

    def test_selective_case_requires_one_undo(self) -> None:
        evidence = valid_evidence()
        evidence["cases"][1]["observations"]["undo_count"] = 2
        with self.assertRaisesRegex(ValueError, "exactly one undo"):
            HARNESS.validate_evidence(evidence)

    def test_transport_count_must_match_case_observations(self) -> None:
        evidence = valid_evidence()
        evidence["transport"]["request_count"] -= 1
        with self.assertRaisesRegex(ValueError, "transport request count"):
            HARNESS.validate_evidence(evidence)

    def test_copy_and_stale_proofs_are_required(self) -> None:
        evidence = valid_evidence()
        evidence["cases"][2]["observations"]["copy_preview_exact"] = False
        with self.assertRaisesRegex(ValueError, "requires true copy_preview_exact"):
            HARNESS.validate_evidence(evidence)

        evidence = valid_evidence()
        evidence["cases"][3]["observations"]["apply_activation_attempted"] = False
        with self.assertRaisesRegex(ValueError, "requires true apply_activation_attempted"):
            HARNESS.validate_evidence(evidence)

        evidence = valid_evidence()
        evidence["cases"][3]["observations"]["apply_disabled_source_contract"] = False
        with self.assertRaisesRegex(ValueError, "requires true apply_disabled_source_contract"):
            HARNESS.validate_evidence(evidence)

    def test_trust_proof_requires_runtime_and_source_contracts(self) -> None:
        evidence = valid_evidence()
        evidence["cases"][5]["observations"]["restricted_after_apply"] = False
        with self.assertRaisesRegex(ValueError, "requires true restricted_after_apply"):
            HARNESS.validate_evidence(evidence)

        evidence = valid_evidence()
        evidence["cases"][5]["observations"]["executable_matches_expected"] = False
        with self.assertRaisesRegex(ValueError, "requires true executable_matches_expected"):
            HARNESS.validate_evidence(evidence)

        evidence = valid_evidence()
        evidence["cases"][5]["observations"]["trust_revocation_order_source_contract"] = False
        with self.assertRaisesRegex(
            ValueError, "requires true trust_revocation_order_source_contract"
        ):
            HARNESS.validate_evidence(evidence)

        evidence = valid_evidence()
        evidence["cases"][5]["observations"]["preview_inert_source_contract"] = False
        with self.assertRaisesRegex(ValueError, "requires true preview_inert_source_contract"):
            HARNESS.validate_evidence(evidence)

    def test_pass_rejects_hash_mismatch_and_private_content(self) -> None:
        evidence = valid_evidence()
        evidence["executable"]["copied_sha256"] = "b" * 64
        with self.assertRaisesRegex(ValueError, "verified copied executable hash"):
            HARNESS.validate_evidence(evidence)

        evidence = valid_evidence()
        evidence["cases"][0]["observations"]["flow"] = HARNESS.DOCUMENT_SENTINEL
        with self.assertRaisesRegex(ValueError, "free-form observation strings"):
            HARNESS.validate_evidence(evidence)

    def test_blocked_evidence_is_valid_without_windows_environment(self) -> None:
        evidence = HARNESS.new_evidence(HASH)
        evidence["cases"][0].update(
            status="BLOCKED",
            reason_code="WINDOWS_REQUIRED",
            duration_ms=0.0,
            observations={},
        )
        for case in evidence["cases"][1:]:
            case.update(
                status="BLOCKED",
                reason_code="SKIPPED_AFTER_BLOCKED",
                duration_ms=0.0,
            )
        HARNESS.complete_evidence(evidence, "BLOCKED")
        HARNESS.validate_evidence(evidence)
        self.assertEqual(
            evidence["summary"]["blocked_case_count"], len(HARNESS.REQUIRED_CASE_IDS)
        )

    def test_evidence_objects_have_exact_keys_and_private_content_is_recursive(self) -> None:
        for location in ("top-level", "executable", "environment", "case"):
            evidence = valid_evidence()
            if location == "top-level":
                evidence["unexpected"] = True
            elif location == "executable":
                evidence["executable"]["unexpected"] = True
            elif location == "environment":
                evidence["environment"]["unexpected"] = True
            else:
                evidence["cases"][0]["unexpected"] = True
            with self.subTest(location=location), self.assertRaisesRegex(ValueError, "keys"):
                HARNESS.validate_evidence(evidence)

        for sentinel in (
            HARNESS.DOCUMENT_SENTINEL,
            PROVIDER.ANSWER_SENTINEL,
            PROVIDER.RAW_RESPONSE_SENTINEL,
        ):
            evidence = valid_evidence()
            evidence["cases"][0]["observations"]["flow"] = sentinel
            with self.subTest(sentinel=sentinel), self.assertRaisesRegex(
                ValueError, "free-form observation strings"
            ):
                HARNESS.validate_evidence(evidence)

        evidence = valid_evidence()
        evidence["cases"][0]["observations"]["process_context"] = {
            "request_body": True,
        }
        with self.assertRaisesRegex(ValueError, "private content"):
            HARNESS.validate_evidence(evidence)

        for key in ("process_context", "runtime_scan", "editor_before"):
            evidence = valid_evidence()
            evidence["cases"][0]["observations"][key]["unexpected"] = True
            with self.subTest(nested_key=key), self.assertRaisesRegex(ValueError, "keys"):
                HARNESS.validate_evidence(evidence)

    def test_runtime_scan_requires_positive_and_consistent_config_counts(self) -> None:
        evidence = valid_evidence()
        evidence["cases"][0]["observations"]["runtime_scan"]["config_files_scanned"] = 0
        with self.assertRaisesRegex(ValueError, "application log"):
            HARNESS.validate_evidence(evidence)

        evidence = valid_evidence()
        evidence["cases"][0]["observations"]["runtime_scan"].update(
            files_scanned=1,
            app_logs_scanned=1,
            config_files_scanned=1,
        )
        with self.assertRaisesRegex(ValueError, "counts are inconsistent"):
            HARNESS.validate_evidence(evidence)

        evidence = valid_evidence()
        evidence["cases"][0]["observations"]["runtime_scan"].update(
            files_scanned=2,
            app_logs_scanned=1,
            config_files_scanned=1,
        )
        with self.assertRaisesRegex(ValueError, "file count is incomplete"):
            HARNESS.validate_evidence(evidence)


class EvidenceFinalizationTests(unittest.TestCase):
    def run_native_cases(
        self,
        root: Path,
        scenarios: tuple[object, ...],
        *,
        case: str | None = None,
        preflight: object | None = None,
    ) -> tuple[int, dict, str]:
        executable = root / "markturbo.exe"
        executable.write_bytes(b"native evidence finalization fixture")
        expected_hash = runtime.sha256_file(executable).sha256
        goal07_plan = HARNESS.native_run_plan()

        class FakeHarness:
            def cleanup(self) -> None:
                pass

        plan = runtime.NativeRunPlan(
            required_case_ids=HARNESS.REQUIRED_CASE_IDS,
            workdir_prefix="markturbo-goal07-evidence-test-",
            new_evidence=goal07_plan.new_evidence,
            validate_evidence=goal07_plan.validate_evidence,
            preflight=preflight or (lambda *_args: (object(), object())),
            ui_types_loader=lambda: (),
            harness_factory=lambda *_args: FakeHarness(),
            scenarios=lambda _harness: scenarios,
            finalize_evidence=goal07_plan.finalize_evidence,
        )
        args = type("Args", (), {})()
        args.exe = executable
        args.expect_exe_sha256 = expected_hash
        args.ui_timeout = 1.0
        args.case = case
        args.keep_workdir_on_failure = False
        return runtime.run_native_acceptance(args, plan)

    def test_interleaved_evidence_instances_keep_transport_counts_separate(self) -> None:
        first = HARNESS.new_evidence(HASH)
        second = HARNESS.new_evidence(HASH)
        first["cases"][0]["observations"]["provider_request_count"] = 2
        second["cases"][2]["observations"]["provider_request_count"] = 4

        HARNESS.complete_evidence(first, "FAIL")
        HARNESS.complete_evidence(second, "BLOCKED")
        self.assertEqual(first["transport"]["request_count"], 2)
        self.assertEqual(second["transport"]["request_count"], 4)

        first["cases"][1]["observations"]["provider_request_count"] = 1
        HARNESS.complete_evidence(first, "FAIL")
        self.assertEqual(first["transport"]["request_count"], 3)
        self.assertEqual(second["transport"]["request_count"], 4)

    def test_standalone_scenario_does_not_mutate_other_evidence_instances(self) -> None:
        first = HARNESS.new_evidence(HASH)
        second = HARNESS.new_evidence(HASH)
        observations = common(HARNESS.CASE_REJECT_ALL)
        harness = object.__new__(HARNESS.Goal07Harness)
        harness.parent_context = runtime.SecurityContext(7, 0x2000, "medium")
        harness.scenario_reject_all = lambda: observations

        result = harness.scenario(HARNESS.CASE_REJECT_ALL)

        self.assertIs(result, observations)
        self.assertEqual(first["transport"]["request_count"], 0)
        self.assertEqual(second["transport"]["request_count"], 0)

    def test_request_count_rejects_boolean_case_observations(self) -> None:
        evidence = valid_evidence()
        evidence["cases"][0]["observations"]["provider_request_count"] = True
        with self.assertRaisesRegex(ValueError, "invalid provider request count"):
            HARNESS.validate_evidence(evidence)

        with self.assertRaisesRegex(ValueError, "invalid provider request count"):
            HARNESS.complete_evidence(evidence, "PASS")

    def test_failed_privacy_scan_counts_only_previously_persisted_cases(self) -> None:
        def failed_scan() -> dict:
            with tempfile.TemporaryDirectory() as temporary:
                case_root = Path(temporary)
                logs = case_root / "data" / "logs"
                logs.mkdir(parents=True)
                (case_root / "config").mkdir()
                (logs / "markturbo.log").write_bytes(
                    HARNESS.DOCUMENT_SENTINEL.encode("utf-8")
                )
                return HARNESS.scan_case_artifacts(case_root, "synthetic-key")

        scenarios = (
            lambda: {"provider_request_count": 2},
            failed_scan,
            *(lambda: {} for _ in range(len(HARNESS.REQUIRED_CASE_IDS) - 2)),
        )
        with tempfile.TemporaryDirectory() as temporary:
            returncode, evidence, reason = self.run_native_cases(
                Path(temporary), scenarios
            )

        self.assertEqual((returncode, evidence["status"], reason), (1, "FAIL", "UTF8_DOCUMENT_SENTINEL_LEAKED"))
        self.assertEqual(evidence["cases"][0]["status"], "PASS")
        self.assertEqual(evidence["cases"][1]["status"], "FAIL")
        self.assertEqual(evidence["cases"][1]["observations"], {})
        self.assertEqual(evidence["transport"]["request_count"], 2)

    def test_partial_case_counts_only_the_selected_observation(self) -> None:
        scenarios = tuple(
            (lambda: {"provider_request_count": 2})
            for _ in HARNESS.REQUIRED_CASE_IDS
        )
        with tempfile.TemporaryDirectory() as temporary:
            returncode, evidence, reason = self.run_native_cases(
                Path(temporary), scenarios, case=HARNESS.CASE_STALE
            )

        self.assertEqual((returncode, evidence["status"], reason), (1, "FAIL", "PARTIAL_CASE_RUN"))
        self.assertEqual(evidence["transport"]["request_count"], 2)
        for item in evidence["cases"]:
            expected = "PASS" if item["id"] == HARNESS.CASE_STALE else "NOT_RUN"
            self.assertEqual(item["status"], expected)
            if expected == "NOT_RUN":
                self.assertEqual(item["observations"], {})

    def test_early_blocked_plan_keeps_request_count_zero(self) -> None:
        def blocked(*_args: object) -> tuple[object, object]:
            raise runtime.HarnessBlocked("WTS_SESSION_NOT_ACTIVE")

        scenarios = tuple(lambda: {} for _ in HARNESS.REQUIRED_CASE_IDS)
        with tempfile.TemporaryDirectory() as temporary:
            returncode, evidence, reason = self.run_native_cases(
                Path(temporary), scenarios, preflight=blocked
            )

        self.assertEqual((returncode, evidence["status"], reason), (2, "BLOCKED", "WTS_SESSION_NOT_ACTIVE"))
        self.assertEqual(evidence["transport"]["request_count"], 0)


class PrivacyAndRuntimeTests(unittest.TestCase):
    def test_runtime_scan_detects_split_boundary_private_bytes(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / "data" / "logs").mkdir(parents=True)
            (root / "config").mkdir()
            split = 1024 * 1024 - 2
            (root / "data" / "logs" / "app.log").write_bytes(
                b"x" * split + HARNESS.DOCUMENT_SENTINEL.encode("utf-8")
            )
            with self.assertRaisesRegex(
                runtime.HarnessFailure, "UTF8_DOCUMENT_SENTINEL_LEAKED"
            ):
                HARNESS.scan_case_artifacts(root, "synthetic-key")

    def test_runtime_scan_rejects_credential_leak_without_recording_value(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / "data" / "logs").mkdir(parents=True)
            (root / "config").mkdir()
            (root / "data" / "logs" / "app.log").write_text(
                "synthetic-key", encoding="utf-8"
            )
            with self.assertRaisesRegex(
                runtime.HarnessFailure, "UTF8_EPHEMERAL_CREDENTIAL_LEAKED"
            ):
                HARNESS.scan_case_artifacts(root, "synthetic-key")

    def test_runtime_scan_rejects_answer_and_raw_response_sentinels(self) -> None:
        for sentinel, code in (
            (PROVIDER.ANSWER_SENTINEL, "UTF8_ANSWER_SENTINEL_LEAKED"),
            (PROVIDER.RAW_RESPONSE_SENTINEL, "UTF8_RAW_RESPONSE_SENTINEL_LEAKED"),
        ):
            with self.subTest(code=code), tempfile.TemporaryDirectory() as directory:
                root = Path(directory)
                (root / "data" / "logs").mkdir(parents=True)
                (root / "config").mkdir()
                (root / "data" / "logs" / "app.log").write_text(
                    sentinel, encoding="utf-8"
                )
                with self.assertRaisesRegex(runtime.HarnessFailure, code):
                    HARNESS.scan_case_artifacts(root, "synthetic-key")

    def test_evidence_does_not_allow_request_body_or_credentials(self) -> None:
        evidence = HARNESS.new_evidence(HASH)
        evidence["transport"]["request_body"] = "forbidden"
        with self.assertRaisesRegex(ValueError, "invalid loopback transport"):
            HARNESS.validate_evidence(evidence)

    def test_artifact_read_errors_keep_the_goal_runtime_scan_code(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            (root / "data" / "logs").mkdir(parents=True)
            (root / "config").mkdir()
            (root / "data" / "logs" / "markturbo.log").write_bytes(b"startup")
            with (
                mock.patch.object(
                    HARNESS,
                    "artifact_contains",
                    side_effect=PermissionError("document content"),
                ),
                self.assertRaises(runtime.HarnessFailure) as raised,
            ):
                HARNESS.scan_case_artifacts(root, "synthetic-key")

        self.assertEqual(raised.exception.code, "RUNTIME_ARTIFACT_SCAN_FAILED")
        self.assertEqual(raised.exception.detail, "PermissionError")

    def test_privacy_scan_waits_for_owned_descendants_even_after_parent_exit(self) -> None:
        for parent_exited in (False, True):
            with self.subTest(parent_exited=parent_exited), tempfile.TemporaryDirectory() as directory:
                harness, kernel = owned_harness(Path(directory), HARNESS.Goal07Harness)
                app = launch_owned(harness)
                kernel.parent_exited = parent_exited
                case_root = harness.root / "cases" / "owned"
                logs = case_root / "data" / "logs"
                logs.mkdir()
                (logs / "app.log").write_bytes(b"content-free log")
                (case_root / "config" / "settings.toml").write_bytes(b"keyless")
                (case_root / "stderr.log").write_bytes(b"")
                provider = mock.Mock()
                provider.contract_evidence.return_value = {"provider_request_count": 2}
                real_scan = HARNESS.scan_case_artifacts

                def scan(*args):
                    self.assertFalse(kernel.descendant_alive)
                    self.assertIn("completion-4", kernel.events)
                    kernel.events.append("scan")
                    return real_scan(*args)

                with mock.patch.object(HARNESS, "scan_case_artifacts", side_effect=scan):
                    result = harness._finalize_observations(provider, case_root, app, {})
                self.assertEqual(kernel.events[-2:], ["completion-4", "scan"])
                self.assertEqual(result["runtime_scan"]["files_scanned"], 3)
                self.assertCountEqual(kernel.closed, kernel.opened)

    def test_quiescence_failure_never_reaches_provider_finalization_or_privacy_scan(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            harness, kernel = owned_harness(
                Path(directory), HARNESS.Goal07Harness, OwnedKernel("completion")
            )
            app = launch_owned(harness)
            kernel.parent_exited = True
            provider = mock.Mock()
            observations = {}
            with (
                mock.patch.object(runtime.ctypes, "get_last_error", return_value=258, create=True),
                mock.patch.object(HARNESS, "scan_case_artifacts") as scan,
                self.assertRaisesRegex(runtime.HarnessFailure, "PROCESS_JOB_QUIESCENCE_TIMEOUT"),
            ):
                harness._finalize_observations(provider, Path(directory), app, observations)
            scan.assert_not_called()
            provider.contract_evidence.assert_not_called()
            self.assertEqual(observations, {})
            self.assertCountEqual(kernel.closed, kernel.opened)

    def test_isolation_removal_requires_successful_owned_job_quiescence(self) -> None:
        for fail in ("", "completion"):
            with self.subTest(fail=fail), tempfile.TemporaryDirectory() as directory:
                root = Path(directory)
                executable = root / "app.exe"
                executable.write_bytes(b"synthetic fixture")
                kernel = OwnedKernel(fail)
                harnesses = []

                def factory(_exe, workdir, *_args):
                    harness, _kernel = owned_harness(workdir, HARNESS.Goal07Harness, kernel)
                    harnesses.append(harness)
                    return harness

                def scenario():
                    launch_owned(harnesses[0])
                    kernel.parent_exited = True
                    raise runtime.HarnessBlocked("FOREGROUND_PERMISSION_DENIED")

                base = HARNESS.native_run_plan()
                plan = runtime.NativeRunPlan(
                    required_case_ids=HARNESS.REQUIRED_CASE_IDS,
                    workdir_prefix="markturbo-owned-job-test-",
                    new_evidence=base.new_evidence,
                    validate_evidence=base.validate_evidence,
                    preflight=lambda *_args: (None, None),
                    ui_types_loader=lambda: (),
                    harness_factory=factory,
                    scenarios=lambda _harness: (scenario,) * len(HARNESS.REQUIRED_CASE_IDS),
                    finalize_evidence=base.finalize_evidence,
                )
                args = argparse.Namespace(
                    exe=executable, expect_exe_sha256=runtime.sha256_file(executable).sha256,
                    ui_timeout=1.0, case=None, keep_workdir_on_failure=False,
                )
                real_remove = runtime.remove_tree_with_retry

                def remove(workdir):
                    self.assertFalse(kernel.descendant_alive)
                    self.assertIn("completion-4", kernel.events)
                    kernel.events.append("remove")
                    real_remove(workdir)

                try:
                    with (
                        mock.patch.object(runtime.ctypes, "get_last_error", return_value=258, create=True),
                        mock.patch.object(runtime, "remove_tree_with_retry", side_effect=remove) as removal,
                    ):
                        result, evidence, reason = runtime.run_native_acceptance(args, plan)
                    self.assertCountEqual(kernel.closed, kernel.opened)
                    if fail:
                        self.assertEqual((result, reason), (1, "PROCESS_JOB_QUIESCENCE_TIMEOUT"))
                        self.assertEqual(evidence["cases"][0]["status"], "FAIL")
                        removal.assert_not_called()
                        self.assertTrue(args.debug_workdir.is_dir())
                    else:
                        self.assertEqual((result, reason), (2, "FOREGROUND_PERMISSION_DENIED"))
                        self.assertEqual(kernel.events[-2:], ["completion-4", "remove"])
                        self.assertIsNone(args.debug_workdir)
                        self.assertFalse(harnesses[0].root.exists())
                finally:
                    if args.debug_workdir is not None:
                        # The fake owns no OS processes; remove the deliberately retained fixture.
                        real_remove(args.debug_workdir)

class SourceContractFixtureTests(unittest.TestCase):
    def source_failure(
        self,
        *,
        trust_body: str | None = None,
        active_preview: bool = False,
        test_only_trust_markers: bool = False,
        test_only_preview_markers: bool = False,
    ) -> str | None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            _write_goal07_source_fixture(
                root,
                trust_body=trust_body,
                active_preview=active_preview,
                test_only_trust_markers=test_only_trust_markers,
                test_only_preview_markers=test_only_preview_markers,
            )
            with mock.patch.object(HARNESS, "REPO", root):
                return HARNESS.source_contract_failure()

    def test_safe_source_fixture_passes_all_goal07_source_checkers(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            _write_goal07_source_fixture(root)
            with mock.patch.object(HARNESS, "REPO", root):
                self.assertIsNone(HARNESS.source_contract_failure())
                self.assertTrue(HARNESS.trust_apply_source_contract_ok())
                self.assertTrue(HARNESS.preview_inert_source_contract_ok())

    def test_real_repository_satisfies_source_contract(self) -> None:
        self.assertIsNone(HARNESS.source_contract_failure())

    def test_alternate_test_modules_cannot_supply_any_source_surface(self) -> None:
        cases = (
            ("workspace.rs", "REVISION_UIA_CONTRACT_MISSING"),
            ("workspace/review.rs", "REVISION_SOURCE_CONTRACT_MISSING"),
            ("document.rs", "REVISION_BOUNDARY_CONTRACT_MISSING"),
            ("document/preview.rs", "REVISION_TRUST_SOURCE_CONTRACT_MISSING"),
        )
        for relative, expected in cases:
            with self.subTest(source=relative), tempfile.TemporaryDirectory() as directory:
                root = Path(directory)
                _write_goal07_source_fixture(root)
                source = root / "crates" / "mt-app" / "src" / "views" / relative
                original = source.read_text(encoding="utf-8")
                source.write_text(
                    "#[cfg(test)] mod checks {\n"
                    + original
                    + "\n}\nfn production_after_tests() {}\n",
                    encoding="utf-8",
                )
                with mock.patch.object(HARNESS, "REPO", root):
                    self.assertEqual(HARNESS.source_contract_failure(), expected)

    def test_test_imports_and_modules_do_not_hide_later_production(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            _write_goal07_source_fixture(root)
            views = root / "crates" / "mt-app" / "src" / "views"
            for relative in (
                "workspace.rs", "workspace/review.rs",
                "document.rs", "document/preview.rs",
            ):
                source = views / relative
                original = source.read_text(encoding="utf-8")
                source.write_text(
                    "#[cfg(test)] use crate::fixtures::TestOnly;\n"
                    "#[cfg(test)]\nmod tests { fn decoy() {} }\n"
                    + original,
                    encoding="utf-8",
                )
            with mock.patch.object(HARNESS, "REPO", root):
                self.assertIsNone(HARNESS.source_contract_failure())

    def test_unsafe_projection_returns_existing_source_contract_codes(self) -> None:
        cases = (
            ("workspace.rs", "REVISION_UIA_CONTRACT_MISSING"),
            ("workspace/review.rs", "REVISION_UIA_CONTRACT_MISSING"),
            ("document.rs", "REVISION_BOUNDARY_CONTRACT_MISSING"),
            ("document/preview.rs", "REVISION_TRUST_SOURCE_CONTRACT_MISSING"),
        )
        for relative, expected in cases:
            with self.subTest(source=relative), tempfile.TemporaryDirectory() as directory:
                root = Path(directory)
                _write_goal07_source_fixture(root)
                source = root / "crates" / "mt-app" / "src" / "views" / relative
                original = source.read_text(encoding="utf-8")
                source.write_text(
                    original + "\n#[cfg(test)] mod checks {\n",
                    encoding="utf-8",
                )
                with mock.patch.object(HARNESS, "REPO", root):
                    self.assertEqual(HARNESS.source_contract_failure(), expected)

    def test_source_contract_rejects_replace_before_trust_revocation(self) -> None:
        wrong_order = """\
        self.replace_text(final_text, window, cx);
        let revoke_trust = self.trust == Trust::Trusted
            && matches!(self.document.doc_type(),
                DocType::Html | DocType::Mdx)
            && current_text != final_text;
        if revoke_trust {
            self.trust = Trust::Restricted;
            self.preview.trust_changed(Trust::Restricted);
        }
"""
        self.assertEqual(
            self.source_failure(trust_body=wrong_order),
            "REVISION_TRUST_SOURCE_CONTRACT_MISSING",
        )

    def test_source_contract_rejects_active_revision_preview(self) -> None:
        self.assertEqual(
            self.source_failure(active_preview=True),
            "REVISION_PREVIEW_INERT_CONTRACT_MISSING",
        )

    def test_source_contract_ignores_trust_markers_only_in_test_module(self) -> None:
        self.assertEqual(
            self.source_failure(
                trust_body="",
                test_only_trust_markers=True,
            ),
            "REVISION_TRUST_SOURCE_CONTRACT_MISSING",
        )

    def test_source_contract_ignores_preview_markers_only_in_test_module(self) -> None:
        self.assertEqual(
            self.source_failure(test_only_preview_markers=True),
            "REVISION_PREVIEW_INERT_CONTRACT_MISSING",
        )

    def test_source_contract_rejects_review_values_only_in_comments(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            _write_goal07_source_fixture(root)
            review = root / "crates" / "mt-app" / "src" / "views" / "workspace" / "review.rs"
            source = review.read_text(encoding="utf-8")
            value = HARNESS.REVISION_RESULT_ACCESSIBILITY_ID
            source = source.replace(
                f'const REVIEW_UIA_VALUE_0: &str = "{value}";',
                f'// Removed result ID: {value}',
                1,
            )
            review.write_text(source, encoding="utf-8")

            with mock.patch.object(HARNESS, "REPO", root):
                self.assertEqual(
                    HARNESS.source_contract_failure(),
                    "REVISION_UIA_CONTRACT_MISSING",
                )

    def test_source_contract_rejects_comment_only_trust_markers(self) -> None:
        comment_decoy = f"\n{_trust_contract_comment_decoy()}\n"
        self.assertEqual(
            self.source_failure(trust_body=comment_decoy),
            "REVISION_TRUST_SOURCE_CONTRACT_MISSING",
        )


class SourceAndCliTests(unittest.TestCase):
    def test_review_after_web_handoff_sends_keys_without_explicit_refocus(self) -> None:
        for preserve in (False, True):
            with self.subTest(preserve_focus=preserve):
                harness = object.__new__(HARNESS.Goal07Harness)
                harness.focus_editor = mock.Mock()
                harness._require_foreground = mock.Mock()
                harness.win32 = mock.Mock()
                harness._click_id = mock.Mock()
                harness._approve_consent = mock.Mock()
                harness.find_control = mock.Mock()
                app, provider = mock.Mock(), mock.Mock()
                app.hwnd = 73
                harness.win32.foreground_focus.return_value = app.hwnd

                harness.run_review(app, provider, preserve_focus=preserve)

                self.assertEqual(harness.focus_editor.call_count, 0 if preserve else 1)
                self.assertEqual(harness._require_foreground.call_count, 0 if preserve else 1)
                if preserve:
                    harness.win32.foreground_focus.assert_called_once_with(app.hwnd)
                keys = harness.win32.send_inputs.call_args.args[0]
                self.assertEqual([item.ki.wVk for item in keys], [
                    HARNESS.VK_CONTROL, HARNESS.VK_SHIFT, HARNESS.VK_R,
                    HARNESS.VK_R, HARNESS.VK_SHIFT, HARNESS.VK_CONTROL,
                ])
                harness._approve_consent.assert_called_once_with(
                    app, "REVIEW_CONSENT_CLICK_FAILED", provider=provider,
                    expected_request_count=0, open_gate=provider.grant_review_consent,
                )

    def test_preserved_focus_never_sends_keys_to_a_different_native_owner(self) -> None:
        for focused in (0, 74):
            with self.subTest(focused=focused):
                harness = object.__new__(HARNESS.Goal07Harness)
                harness.win32 = mock.Mock()
                harness.win32.foreground_focus.return_value = focused
                harness.focus_editor = mock.Mock()
                harness._require_foreground = mock.Mock()

                with self.assertRaisesRegex(runtime.HarnessFailure, "WEB_TO_SOURCE_NATIVE_FOCUS_LOST"):
                    harness.run_review(mock.Mock(hwnd=73), mock.Mock(), preserve_focus=True)

                harness.win32.send_inputs.assert_not_called()
                harness.focus_editor.assert_not_called()
                harness._require_foreground.assert_not_called()

    def test_trust_scenario_requires_handoff_before_review_or_editor_helpers(self) -> None:
        for broken in (False, True):
            with self.subTest(broken=broken):
                harness = object.__new__(HARNESS.Goal07Harness)
                harness.ui_timeout = 1.0
                app, provider = mock.Mock(), mock.Mock()
                app.security_context.evidence.return_value = process_context()
                app.process.poll.return_value = None
                provider.start.return_value = provider
                harness.profile = mock.Mock(return_value=(Path("unused"),) * 6)
                harness.launch_app = mock.Mock(return_value=app)
                harness.find_control = mock.Mock()
                harness.click_control = mock.Mock()
                harness._trust_label = mock.Mock(side_effect=["Trusted", "Restricted"])
                harness.activate_source_layout = mock.Mock(side_effect=AssertionError("premature Source"))
                harness.activate_source_from_focused_web = mock.Mock()
                harness.run_review = mock.Mock()
                harness.request_revision = mock.Mock()
                harness._click_id = mock.Mock()
                harness._require_foreground = mock.Mock(return_value=True)
                final_text = HARNESS.apply_edits(
                    HARNESS.HTML_EDITOR_SOURCE_BYTES,
                    PROVIDER.fixture_edits(HARNESS.HTML_EDITOR_SOURCE_TEXT),
                ).decode("utf-8")
                harness._preview_source_text = mock.Mock(return_value=final_text)
                harness.editor_fingerprint = mock.Mock(side_effect=[
                    runtime.fingerprint_bytes(HARNESS.HTML_EDITOR_SOURCE_BYTES),
                    runtime.fingerprint_text(final_text),
                ])
                harness.win32 = mock.Mock()
                harness.click_lifecycle_decision = mock.Mock()
                harness.wait_process_exit = mock.Mock(
                    side_effect=lambda _app: setattr(app.process.poll, "return_value", 0)
                )
                harness.reap = mock.Mock()
                harness._finalize_observations = lambda _provider, _root, _app, obs: obs
                order = mock.Mock()
                for name in ("activate_source_from_focused_web", "run_review", "editor_fingerprint"):
                    order.attach_mock(getattr(harness, name), name)
                if broken:
                    harness.activate_source_from_focused_web.side_effect = runtime.HarnessFailure(
                        "WEB_TO_SOURCE_NATIVE_FOCUS_TIMEOUT"
                    )
                with (
                    mock.patch.object(PROVIDER, "LoopbackRevisionServer", return_value=provider),
                    mock.patch.object(HARNESS, "wait_until", side_effect=lambda check, *_a, **_k: check()),
                    mock.patch.object(HARNESS, "trust_apply_source_contract_ok", return_value=True),
                    mock.patch.object(HARNESS, "preview_inert_source_contract_ok", return_value=True),
                ):
                    if broken:
                        with self.assertRaisesRegex(runtime.HarnessFailure, "WEB_TO_SOURCE_NATIVE_FOCUS_TIMEOUT"):
                            harness.scenario_trust_revoke()
                        harness.run_review.assert_not_called()
                        harness.editor_fingerprint.assert_not_called()
                        harness.reap.assert_called_once_with(app)
                    else:
                        observations = harness.scenario_trust_revoke()
                        self.assertTrue(observations["trust_before"])
                        self.assertEqual(order.mock_calls[:2], [
                            mock.call.activate_source_from_focused_web(app),
                            mock.call.run_review(app, provider, preserve_focus=True),
                        ])
                        harness.request_revision.assert_called_once_with(app, provider=provider)
                        harness.click_lifecycle_decision.assert_called_once_with(app, "Discard")
                harness.activate_source_layout.assert_not_called()
                provider.close.assert_called_once_with()

    def test_scenario_contract_failure_is_raised_before_shared_pass_status(self) -> None:
        harness = object.__new__(HARNESS.Goal07Harness)
        observations = common(HARNESS.CASE_REJECT_ALL)
        observations["reject_all_byte_identity"] = False
        harness.parent_context = mock.Mock()
        harness.parent_context.evidence.return_value = observations["process_context"]
        harness.scenario_reject_all = lambda: observations
        evidence = HARNESS.new_evidence(HASH)

        with self.assertRaisesRegex(runtime.HarnessFailure, "CASE_CONTRACT_FAILED"):
            harness.scenario(HARNESS.CASE_REJECT_ALL)

        self.assertEqual(evidence["transport"]["request_count"], 0)
        HARNESS.complete_evidence(evidence, "BLOCKED")
        HARNESS.validate_evidence(evidence)

    def test_preview_source_accepts_name_only_text_but_not_the_static_label(self) -> None:
        class NoValuePattern(Exception):
            pass

        class Control:
            def __init__(self, name: str) -> None:
                self.element_info = mock.Mock()
                self.element_info.name = name

            @property
            def iface_value(self) -> object:
                raise NoValuePattern

            def children(self) -> list[object]:
                return []

        harness = object.__new__(HARNESS.Goal07Harness)
        harness.no_pattern_error_class = NoValuePattern
        harness.find_control = lambda *_args, **_kwargs: Control("approved\npreview")

        self.assertEqual(harness._preview_source_text(mock.Mock()), "approved\npreview")

        harness.find_control = lambda *_args, **_kwargs: Control(
            "Final approved Revision source"
        )
        with self.assertRaisesRegex(
            HARNESS.HarnessFailure, "REVISION_PREVIEW_SOURCE_TEXT_CONTRACT_MISMATCH"
        ):
            harness._preview_source_text(mock.Mock())

    def test_answer_input_uses_the_focused_unicode_keyboard_path(self) -> None:
        harness = object.__new__(HARNESS.Goal07Harness)
        harness.ui_timeout = 0.1
        events: list[tuple[str, object]] = []

        class Control:
            def is_enabled(self) -> bool:
                return True

        control = Control()
        app = mock.Mock(hwnd=41)
        harness._click_id = lambda _app, automation_id: events.append(
            ("answer-state", automation_id)
        )
        harness.control_by_id = lambda *_args, **_kwargs: control
        harness.win32 = mock.Mock()
        harness.win32.send_unicode.side_effect = lambda hwnd, text: events.append(
            ("send-unicode", (hwnd, text))
        )
        harness._text_control = lambda *_args, **_kwargs: "kept intent"

        harness._set_answer(app, 0, "kept intent")

        self.assertEqual(events[0][0], "answer-state")
        self.assertEqual(events[1], ("send-unicode", (41, "kept intent")))

    def test_bottom_bar_covered_revision_control_scrolls_from_visible_review_content(self) -> None:
        harness = object.__new__(HARNESS.Goal07Harness)
        harness.ui_timeout = 0.1
        window_rect = mock.Mock(top=50, bottom=1031)
        offscreen = mock.Mock(top=972, bottom=1000)
        onscreen = mock.Mock(top=864, bottom=892)
        control = mock.Mock()
        control.rectangle.side_effect = [offscreen, onscreen]
        anchor = mock.Mock()
        anchor.rectangle.return_value = mock.Mock(top=100, bottom=120)
        app = mock.Mock(hwnd=41)
        app.window.rectangle.return_value = window_rect
        harness.control_by_id = mock.Mock(
            side_effect=lambda _hwnd, automation_id, *_args: (
                control if automation_id == "target" else anchor
            )
        )

        result = harness._scroll_revision_control_into_view(
            app, "target", "Button", control
        )

        self.assertIs(result, control)
        anchor.wheel_mouse_input.assert_called_once_with(wheel_dist=-3)

    def test_stale_control_scrolls_up_until_accessible(self) -> None:
        harness = object.__new__(HARNESS.Goal07Harness)
        harness.ui_timeout = 0.1
        stale = mock.Mock()
        stale.rectangle.return_value = mock.Mock(top=120, bottom=140)
        anchor = mock.Mock()
        anchor.rectangle.return_value = mock.Mock(top=700, bottom=760)
        app = mock.Mock(hwnd=41)
        app.window.rectangle.return_value = mock.Mock(top=50, bottom=900)
        stale_calls = 0

        def control_by_id(_hwnd, automation_id, *_args, **_kwargs):
            nonlocal stale_calls
            if automation_id == HARNESS.REVISION_STALE_ACCESSIBILITY_ID:
                stale_calls += 1
                return stale if stale_calls > 1 else None
            if automation_id == HARNESS.REVISION_PREVIEW_ACCESSIBILITY_ID:
                return anchor
            return None

        harness.control_by_id = control_by_id

        result = harness._find_stale_control(app)

        self.assertIs(result, stale)
        anchor.wheel_mouse_input.assert_called_once_with(wheel_dist=3)

    def test_apply_state_scrolls_above_the_bottom_bar_before_querying(self) -> None:
        harness = object.__new__(HARNESS.Goal07Harness)
        button = mock.Mock()
        button.is_enabled.return_value = True
        app = mock.Mock(hwnd=41)
        harness.control_by_id = mock.Mock(return_value=button)
        harness._scroll_revision_control_into_view = mock.Mock(return_value=button)

        result, reported_enabled = harness._apply_button_state(app)

        self.assertIs(result, button)
        self.assertTrue(reported_enabled)
        harness._scroll_revision_control_into_view.assert_called_once_with(
            app,
            HARNESS.REVISION_APPLY_ACCESSIBILITY_ID,
            "Button",
            button,
        )

    def test_consent_gate_rejects_any_pre_click_request_snapshot(self) -> None:
        harness = object.__new__(HARNESS.Goal07Harness)
        harness.ui_timeout = 0.1
        harness.win32 = mock.Mock()
        harness.win32.owned_task_dialogs.return_value = [99]
        harness.control_by_id = lambda *_args, **_kwargs: object()
        events: list[str] = []

        class FakeProvider:
            def request_snapshot(self) -> dict[str, int | bool]:
                return {"request_count": 1, "invalid_request": False}

            def dispatch_consent_click(
                self, _count: int, click: Callable[[], None]
            ) -> None:
                events.append("marker")
                click()

        with self.assertRaisesRegex(
            runtime.HarnessFailure, "REVIEW_REQUEST_BEFORE_CONSENT_CLICK"
        ):
            harness._approve_consent(
                mock.Mock(process=mock.Mock(pid=7), hwnd=8),
                "REVIEW_CONSENT_CLICK_FAILED",
                provider=FakeProvider(),
                expected_request_count=0,
                open_gate=lambda: events.append("gate"),
            )
        self.assertEqual(events, [])

    def test_consent_gate_records_rejected_request_before_click(self) -> None:
        harness = object.__new__(HARNESS.Goal07Harness)
        provider = mock.Mock()
        provider.request_snapshot.return_value = {
            "request_count": 0,
            "invalid_request": True,
        }
        with self.assertRaisesRegex(
            runtime.HarnessFailure, "REVISION_REQUEST_BEFORE_CONSENT_CLICK"
        ):
            harness._open_consent_gate(provider, 1, lambda: None)

    def test_consent_dispatch_requires_request_count_and_gate(self) -> None:
        harness = object.__new__(HARNESS.Goal07Harness)
        harness.ui_timeout = 0.1
        harness.win32 = mock.Mock()
        harness.win32.owned_task_dialogs.return_value = [99]
        harness.control_by_id = lambda *_args, **_kwargs: object()
        provider = mock.Mock()
        app = mock.Mock(process=mock.Mock(pid=7), hwnd=8)

        for expected_request_count, open_gate in (
            (None, lambda: None),
            (0, None),
        ):
            with self.subTest(expected_request_count=expected_request_count), self.assertRaisesRegex(
                runtime.HarnessFailure, "CONSENT_GATE_CONFIGURATION_INVALID"
            ):
                harness._approve_consent(
                    app,
                    "REVIEW_CONSENT_CLICK_FAILED",
                    provider=provider,
                    expected_request_count=expected_request_count,
                    open_gate=open_gate,
                )

    def test_consent_gate_opens_before_the_consent_button_click(self) -> None:
        harness = object.__new__(HARNESS.Goal07Harness)
        harness.ui_timeout = 0.1
        events: list[str] = []
        harness.win32 = mock.Mock()
        harness.win32.owned_task_dialogs.return_value = [99]
        harness.control_by_id = lambda *_args, **_kwargs: object()
        harness.click_control = lambda _control, _failure: events.append("click")

        class FakeProvider:
            def request_snapshot(self) -> dict[str, int | bool]:
                return {"request_count": 0, "invalid_request": False}

            def dispatch_consent_click(
                self, count: int, click: Callable[[], None]
            ) -> None:
                events.append(f"marker-{count}")
                click()

        harness._approve_consent(
            mock.Mock(process=mock.Mock(pid=7), hwnd=8),
            "REVIEW_CONSENT_CLICK_FAILED",
            provider=FakeProvider(),
            expected_request_count=0,
            open_gate=lambda: events.append("gate"),
        )

        self.assertEqual(events, ["gate", "marker-0", "click"])

    def test_app_owned_click_requires_foreground_first(self) -> None:
        harness = object.__new__(HARNESS.Goal07Harness)
        harness.ui_timeout = 0.1
        events: list[str] = []
        harness.win32 = mock.Mock()
        harness.win32.require_foreground.side_effect = lambda *_args: events.append(
            "foreground"
        )
        harness.find_control = lambda *_args, **_kwargs: object()
        harness._scroll_revision_control_into_view = (
            lambda _app, _automation_id, _control_type, control: (
                events.append("scroll") or control
            )
        )
        harness.click_control = lambda _control, _failure: events.append("click")

        harness._click_id(mock.Mock(hwnd=41), "markturbo-revision-accept-all")

        self.assertEqual(events, ["foreground", "scroll", "click"])

    def test_revision_run_waits_until_enabled_before_click(self) -> None:
        harness = object.__new__(HARNESS.Goal07Harness)
        harness.ui_timeout = 0.1
        states = iter((False, True))
        events: list[str] = []

        class Control:
            def is_enabled(self) -> bool:
                events.append("enabled")
                return next(states)

        control = Control()
        harness.win32 = mock.Mock()
        harness.find_control = lambda *_args, **_kwargs: control
        harness._scroll_revision_control_into_view = (
            lambda _app, _automation_id, _control_type, candidate: candidate
        )
        harness.control_by_id = lambda *_args, **_kwargs: control
        harness.click_control = lambda _control, _failure: events.append("click")

        harness._click_id(mock.Mock(hwnd=41), HARNESS.REVISION_RUN_ACCESSIBILITY_ID)

        self.assertEqual(events, ["enabled", "enabled", "click"])

    def test_loopback_profile_is_keyless_and_uses_only_a_process_synthetic_key(self) -> None:
        settings = HARNESS.review_settings_document("http://127.0.0.1:4141/v1/").decode(
            "utf-8"
        )
        self.assertIn('model-provider = "openai-responses"', settings)
        self.assertNotIn("api-key", settings.casefold())
        self.assertIn("model-environment-key-identity", settings)
        self.assertIn(
            HARNESS.endpoint_environment_key_identity("http://127.0.0.1:4141/v1/"),
            settings,
        )
        harness = object.__new__(HARNESS.Goal07Harness)
        harness._credential = "markturbo-goal07-loopback-key"
        self.assertEqual(
            harness.openai_api_key_for_child(), "markturbo-goal07-loopback-key"
        )

    def test_harness_credentials_are_fresh_uuid_values(self) -> None:
        with mock.patch.object(HARNESS.ClipboardNativeHarness, "__init__", return_value=None):
            first = HARNESS.Goal07Harness()
            second = HARNESS.Goal07Harness()
        self.assertRegex(first.openai_api_key_for_child(), r"^markturbo-goal07-[0-9a-f]{32}$")
        self.assertRegex(second.openai_api_key_for_child(), r"^markturbo-goal07-[0-9a-f]{32}$")
        self.assertNotEqual(first.openai_api_key_for_child(), second.openai_api_key_for_child())

    def test_profile_blocks_existing_persistent_credential_after_dynamic_endpoint(self) -> None:
        harness = object.__new__(HARNESS.Goal07Harness)
        harness.win32 = mock.Mock()
        harness.win32.persistent_credential_target_exists.return_value = True
        with tempfile.TemporaryDirectory() as directory:
            harness.root = Path(directory)
            with self.assertRaisesRegex(runtime.HarnessBlocked, "PERSISTENT_CREDENTIAL_PRESENT"):
                harness.profile("synthetic", "http://127.0.0.1:43123/v1/")
        harness.win32.persistent_credential_target_exists.assert_called_once_with(
            HARNESS.endpoint_environment_key_identity("http://127.0.0.1:43123/v1/")
        )

    def test_dynamic_endpoint_identity_includes_port_and_path(self) -> None:
        first = HARNESS.endpoint_environment_key_identity("http://127.0.0.1:4141/v1/")
        second = HARNESS.endpoint_environment_key_identity("http://127.0.0.1:4141/other/")
        third = HARNESS.endpoint_environment_key_identity("http://127.0.0.1:4142/v1/")
        self.assertNotEqual(first, second)
        self.assertNotEqual(first, third)
        self.assertEqual(
            first,
            "io.github.wxxb789.markturbo:model-credential:v2|wire=openai-responses|"
            "host=127.0.0.1|identity-sha256=76d1c8cb04c37287006720b64f66be02bb85723be756cc668cbc0acc13bb1eb4",
        )

    def test_question_and_change_ids_match_production_shape(self) -> None:
        self.assertRegex(
            HARNESS.revision_question_accessibility_id(0),
            r"^markturbo-revision-question-[0-9a-f]{64}$",
        )
        self.assertEqual(
            HARNESS.revision_change_accessibility_id(7, "accept"),
            "markturbo-revision-change-7-accept",
        )

    def test_parser_requires_hash_and_positive_timeout(self) -> None:
        args = HARNESS.parse_args(["--expect-exe-sha256", HASH, "--ui-timeout", "1.5"])
        self.assertEqual(args.expect_exe_sha256, HASH)
        self.assertEqual(args.ui_timeout, 1.5)
        with self.assertRaises(argparse.ArgumentTypeError):
            HARNESS.normalize_expected_hash("not-a-hash")

    def test_single_case_is_not_a_full_acceptance_run(self) -> None:
        args = HARNESS.parse_args(
            ["--expect-exe-sha256", HASH, "--case", HARNESS.CASE_STALE]
        )
        self.assertEqual(args.case, HARNESS.CASE_STALE)

    def test_apply_edits_rejects_invalid_ranges_duplicate_offsets_and_oversized_output(self) -> None:
        invalid = [
            {"range": {"start": -1, "end": 0}, "expected_source": "", "replacement": "x"},
            {"range": {"start": 0, "end": 4}, "expected_source": "abc", "replacement": "x"},
        ]
        for edit in invalid:
            with self.subTest(edit=edit), self.assertRaisesRegex(ValueError, "invalid native fixture edit"):
                HARNESS.apply_edits(b"abc", [edit])

        split_character = {
            "range": {"start": 1, "end": 1},
            "expected_source": "",
            "replacement": "x",
        }
        with self.assertRaisesRegex(ValueError, "invalid native fixture edit"):
            HARNESS.apply_edits("é".encode("utf-8"), [split_character])

        duplicate_insertions = [
            {"range": {"start": 1, "end": 1}, "expected_source": "", "replacement": "x"},
            {"range": {"start": 1, "end": 1}, "expected_source": "", "replacement": "y"},
        ]
        with self.assertRaisesRegex(ValueError, "invalid native fixture edit"):
            HARNESS.apply_edits(b"abc", duplicate_insertions)

        overlapping = [
            {"range": {"start": 0, "end": 2}, "expected_source": "ab", "replacement": "x"},
            {"range": {"start": 1, "end": 3}, "expected_source": "bc", "replacement": "y"},
        ]
        with self.assertRaisesRegex(ValueError, "invalid native fixture edit"):
            HARNESS.apply_edits(b"abc", overlapping)

        with mock.patch.object(HARNESS, "MAX_EDIT_OUTPUT_BYTES", 3):
            oversized = {
                "range": {"start": 0, "end": 1},
                "expected_source": "a",
                "replacement": "xxxx",
            }
            with self.assertRaisesRegex(ValueError, "invalid native fixture edit"):
                HARNESS.apply_edits(b"abc", [oversized])

        no_op = {
            "range": {"start": 0, "end": 1},
            "expected_source": "a",
            "replacement": "a",
        }
        with self.assertRaisesRegex(ValueError, "invalid native fixture edit"):
            HARNESS.apply_edits(b"abc", [no_op])

        with mock.patch.object(HARNESS, "MAX_EDIT_OUTPUT_BYTES", 4):
            subset_only_oversized = [
                {
                    "range": {"start": 0, "end": 3},
                    "expected_source": "abc",
                    "replacement": "",
                },
                {
                    "range": {"start": 4, "end": 4},
                    "expected_source": "",
                    "replacement": "xyz",
                },
            ]
            with self.assertRaisesRegex(ValueError, "invalid native fixture edit"):
                HARNESS.apply_edits(b"abcd", subset_only_oversized)


if __name__ == "__main__":
    unittest.main()
