"""Unit tests for the Goal 07 native harness without launching a UI."""

from __future__ import annotations

import argparse
import hashlib
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
            selective_expected=selected,
            editor_after_apply=selected,
            editor_after_undo=editor_same,
            editor_after_done=selected,
            source_before=disk_same,
            source_after=disk_same,
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
            editor_after_accept_all=final,
            accept_all_expected=final,
            editor_after_done=final,
            accept_all_matches_expected=True,
            copy_preview_fingerprint=final,
            copy_preview_exact=True,
            copy_editor_before=final,
            copy_editor_after=final,
            copy_editor_unchanged=True,
            copy_dirty_before=True,
            copy_dirty_after=True,
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
            part_changed_visible=True,
            accept_withheld=True,
            apply_activation_attempted=True,
            apply_disabled_source_contract=True,
            stale_no_mutation=True,
            editor_after_activation_attempt=edited,
            editor_after_done=edited,
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
            save_command_sent=True,
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
            preview_fingerprint=fingerprint(b'<script>goal07 = "new"</script>'),
            proposal_expected=fingerprint(b'<script>goal07 = "new"</script>'),
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


REPO = Path(__file__).resolve().parents[2]
STATE_RS = "crates/mt-app/src/state.rs"
REVISION_RS = "crates/mt-app/src/ui/revision.rs"
SAFE_STATE = """\
    pub fn set_revision_change_applied(&self, change_id: u32, applied: bool) -> bool {
        let written = match ledger.set_applied(change, applied, text) {
            Ok(written) => written,
            Err(LedgerRefusal::PartChanged) => return false,
        };
        self.write_model_text(&tab, text);
        true
    }
    fn write_model_text(&self, tab: &Tab, text: String) {
        tab.web_trusted.set(false);
        tab.text.set(SharedString::from(text));
    }
    pub fn finish_revision(&self, undecided: usize) {
        self.revision_status.set(Some(SharedString::from("No changes approved.")));
    }
"""
SAFE_REVISION = """\
const LABELS: &[&str] = &["Accept", "Reject", "Accepted", "Rejected", "Undo", "Done", "Copy"];
const MORE: &[&str] = &["Proposed text", "Can't apply — this part changed"];
const IDS: &[&str] = &["revision-accept", "revision-reject", "revision-undo", "revision-done"];
const COPY: &str = "revision-copy";
fn count(n: usize, m: usize) -> String { format!("{n} of {m} decided") }
fn docked_proposal(state: AppState, hunk: Hunk) {
    let accept_condition = move || hunk.with(|hunk| cannot_apply(&state, hunk));
    view().test_id("revision-proposal-after");
}
fn decide(state: &AppState, accepted: bool) {
    if accepted && cannot_apply(state, &hunks[index]) {
        return;
    }
    state.set_revision_change_applied(change_id, accepted);
}
fn cannot_apply(state: &AppState, hunk: &Hunk) -> bool {
    state.revision_hunk_part_changed(hunk.ordinal)
}
"""


def _write_goal07_source_fixture(
    root: Path, *, state: str = SAFE_STATE, revision: str = SAFE_REVISION
) -> None:
    """The production UI files the selectors need, plus small check fixtures."""
    for relative in HARNESS.UI_SOURCE_REQUIREMENTS:
        target = root / relative
        target.parent.mkdir(parents=True, exist_ok=True)
        target.write_bytes((REPO / relative).read_bytes())
    (root / STATE_RS).write_text(
        'fn request_web_trust() {\n'
        '    alert_dialog().destructive("trust", "Run scripts").cancel("cancel", "Cancel");\n'
        '}\n'
        + "impl Fixture {\n"
        + state
        + "}\n",
        encoding="utf-8",
    )
    (root / REVISION_RS).write_text(revision, encoding="utf-8")


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

        evidence = valid_evidence()
        evidence["cases"][1]["observations"]["editor_after_done"] = fingerprint(
            HARNESS.EDITOR_SOURCE_BYTES
        )
        with self.assertRaisesRegex(ValueError, "selective Accept or one undo"):
            HARNESS.validate_evidence(evidence)

    def test_accept_all_must_write_at_once_and_done_must_add_nothing(self) -> None:
        unchanged = fingerprint(HARNESS.EDITOR_SOURCE_BYTES)
        for key in ("editor_after_accept_all", "editor_after_done"):
            with self.subTest(key=key):
                evidence = valid_evidence()
                evidence["cases"][2]["observations"][key] = unchanged
                with self.assertRaisesRegex(ValueError, "accept-all did not write"):
                    HARNESS.validate_evidence(evidence)

    def test_part_changed_case_requires_done_to_write_nothing(self) -> None:
        evidence = valid_evidence()
        evidence["cases"][3]["observations"]["editor_after_done"] = fingerprint(
            HARNESS.EDITOR_SOURCE_BYTES
        )
        with self.assertRaisesRegex(ValueError, "stale proposal did not remain fail-closed"):
            HARNESS.validate_evidence(evidence)

        evidence = valid_evidence()
        evidence["cases"][3]["observations"]["accept_withheld"] = False
        with self.assertRaisesRegex(ValueError, "requires true accept_withheld"):
            HARNESS.validate_evidence(evidence)

    def test_schema_version_1_is_historical_only(self) -> None:
        evidence = valid_evidence()
        self.assertEqual(evidence["schema_version"], 2)
        with self.assertRaisesRegex(ValueError, "not a historical"):
            HARNESS.validate_historical_evidence(evidence)
        evidence["schema_version"] = 1
        with self.assertRaisesRegex(ValueError, "unsupported evidence schema"):
            HARNESS.validate_evidence(evidence)
        HARNESS.validate_historical_evidence(evidence)
        for version in (True, "1", 1.0):
            evidence["schema_version"] = version
            with self.subTest(version=version), self.assertRaisesRegex(
                ValueError, "not a historical"
            ):
                HARNESS.validate_historical_evidence(evidence)

    def test_retained_gpui_era_files_validate_only_as_historical(self) -> None:
        import json

        retained = sorted((REPO / "docs" / "evidence").glob("goal-07-*.json"))
        self.assertEqual(
            [path.name for path in retained],
            [
                "goal-07-final-ablation-f1ec266c-r2.json",
                "goal-07-final-ablation-f1ec266c.json",
                "goal-07-native-acceptance-v1.json",
                "goal-07-post-ablation-trust-f1ec266c.json",
            ],
        )
        for path in retained:
            with self.subTest(file=path.name):
                evidence = json.loads(path.read_text(encoding="utf-8"))
                self.assertEqual(evidence["schema_version"], 1)
                HARNESS.validate_historical_evidence(evidence)
                with self.assertRaisesRegex(ValueError, "unsupported evidence schema"):
                    HARNESS.validate_evidence(evidence)

        broken = json.loads(retained[0].read_text(encoding="utf-8"))
        broken["summary"]["passed_case_count"] -= 1
        with self.assertRaisesRegex(ValueError, "case summary"):
            HARNESS.validate_historical_evidence(broken)
        broken = json.loads(retained[0].read_text(encoding="utf-8"))
        broken["cases"][0]["observations"]["flow"] = HARNESS.DOCUMENT_SENTINEL
        with self.assertRaisesRegex(ValueError, "free-form observation strings"):
            HARNESS.validate_historical_evidence(broken)

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
    def checks(self, **fixture: str) -> tuple[str | None, bool, bool, bool]:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            _write_goal07_source_fixture(root, **fixture)
            with mock.patch.object(HARNESS, "REPO", root):
                return (
                    HARNESS.source_contract_failure(),
                    HARNESS.trust_apply_source_contract_ok(),
                    HARNESS.preview_inert_source_contract_ok(),
                    HARNESS.part_changed_apply_source_contract_ok(),
                )

    def test_real_repository_satisfies_the_selector_contract(self) -> None:
        self.assertIsNone(HARNESS.source_contract_failure())
        self.assertTrue(HARNESS.trust_apply_source_contract_ok())
        self.assertTrue(HARNESS.preview_inert_source_contract_ok())
        self.assertTrue(HARNESS.part_changed_apply_source_contract_ok())

    def test_safe_source_fixture_passes_all_goal07_source_checkers(self) -> None:
        self.assertEqual(self.checks(), (None, True, True, True))

    def test_source_contract_requires_enter_to_answer_the_trust_alert_with_run_scripts(self) -> None:
        # The harness answers the UIA-invisible alert with Enter, which presses
        # the first button unless a Default-styled `.action` exists.
        safe = 'alert_dialog().destructive("trust", "Run scripts").cancel("cancel", "Cancel")'
        for unsafe in (
            'alert_dialog().cancel("cancel", "Cancel").destructive("trust", "Run scripts")',
            'alert_dialog().destructive("trust", "Run scripts").action("later", "Later")',
        ):
            with self.subTest(unsafe=unsafe), tempfile.TemporaryDirectory() as directory:
                root = Path(directory)
                _write_goal07_source_fixture(root)
                state = root / STATE_RS
                text = state.read_text(encoding="utf-8")
                self.assertIn(safe, text)
                state.write_text(text.replace(safe, unsafe), encoding="utf-8")
                with mock.patch.object(HARNESS, "REPO", root):
                    self.assertEqual(
                        HARNESS.source_contract_failure(), "REVISION_SOURCE_CONTRACT_MISSING"
                    )

    def test_each_missing_selector_returns_the_source_contract_failure(self) -> None:
        for relative, markers in HARNESS.UI_SOURCE_REQUIREMENTS.items():
            for marker in markers:
                with self.subTest(file=relative, marker=marker), tempfile.TemporaryDirectory() as directory:
                    root = Path(directory)
                    _write_goal07_source_fixture(root)
                    target = root / relative
                    target.write_text(
                        target.read_text(encoding="utf-8").replace(marker, ""),
                        encoding="utf-8",
                    )
                    with mock.patch.object(HARNESS, "REPO", root):
                        self.assertEqual(
                            HARNESS.source_contract_failure(),
                            "REVISION_SOURCE_CONTRACT_MISSING",
                        )

    def test_unsafe_projection_fails_every_source_check(self) -> None:
        malformed = "\n#[cfg(test)] mod checks {\n"
        self.assertEqual(
            self.checks(state=SAFE_STATE + malformed, revision=SAFE_REVISION + malformed),
            ("REVISION_SOURCE_CONTRACT_MISSING", False, False, False),
        )

    def test_source_contract_rejects_replace_before_trust_revocation(self) -> None:
        wrong_order = SAFE_STATE.replace(
            "        tab.web_trusted.set(false);\n        tab.text.set(SharedString::from(text));\n",
            "        tab.text.set(SharedString::from(text));\n        tab.web_trusted.set(false);\n",
        )
        self.assertNotEqual(wrong_order, SAFE_STATE)
        self.assertFalse(self.checks(state=wrong_order)[1])

    def test_source_contract_rejects_a_done_that_writes_text(self) -> None:
        writing_done = SAFE_STATE.replace(
            "        self.revision_status.set(",
            "        tab.text.set(SharedString::from(text));\n        self.revision_status.set(",
        )
        self.assertNotEqual(writing_done, SAFE_STATE)
        self.assertFalse(self.checks(state=writing_done)[1])

    def test_source_contract_rejects_a_decision_that_bypasses_the_revoking_write(self) -> None:
        bypass = SAFE_STATE.replace(
            "        self.write_model_text(&tab, text);\n",
            "        tab.text.set(SharedString::from(text));\n",
        )
        self.assertNotEqual(bypass, SAFE_STATE)
        self.assertFalse(self.checks(state=bypass)[1])

    def test_source_contract_rejects_comment_only_trust_markers(self) -> None:
        for decoy in (
            "// tab.web_trusted.set(false);\n",
            "/* tab.web_trusted.set(false); */\n",
            'let _ = "tab.web_trusted.set(false);";\n',
        ):
            with self.subTest(decoy=decoy):
                state = SAFE_STATE.replace("        tab.web_trusted.set(false);\n", decoy)
                self.assertFalse(self.checks(state=state)[1])

    def test_source_contract_ignores_trust_markers_only_in_test_module(self) -> None:
        test_only = (
            "#[cfg(test)]\nmod tests {\n"
            + SAFE_STATE
            + "}\n"
            + SAFE_STATE.replace("        tab.web_trusted.set(false);\n", "")
        )
        self.assertFalse(self.checks(state=test_only)[1])

    def test_source_contract_rejects_active_revision_preview(self) -> None:
        for active in (
            "    web_view(html);\n",
            "    let _ = WebView::new();\n",
            "    .java_script_enabled(true);\n",
        ):
            with self.subTest(active=active):
                revision = SAFE_REVISION.replace("}\n", active + "}\n", 1)
                self.assertFalse(self.checks(revision=revision)[2])

    def test_source_contract_ignores_preview_markers_only_in_test_module(self) -> None:
        revision = (
            SAFE_REVISION.replace('view().test_id("revision-proposal-after");\n', "")
            + '#[cfg(test)]\nmod tests {\n    const ID: &str = "revision-proposal-after";\n}\n'
        )
        self.assertFalse(self.checks(revision=revision)[2])

    def test_part_changed_accept_requires_every_gate(self) -> None:
        for name, fixture in (
            (
                "ledger",
                {"state": SAFE_STATE.replace("LedgerRefusal::PartChanged", "LedgerRefusal::Other")},
            ),
            (
                "button",
                {"revision": SAFE_REVISION.replace("cannot_apply(&state, hunk)", "false")},
            ),
            (
                "key",
                {"revision": SAFE_REVISION.replace("cannot_apply(state, &hunks[index])", "false")},
            ),
            (
                "part",
                {"revision": SAFE_REVISION.replace("revision_hunk_part_changed(", "never(")},
            ),
        ):
            with self.subTest(gate=name):
                self.assertFalse(self.checks(**fixture)[3])


class SourceAndCliTests(unittest.TestCase):
    def test_review_after_web_handoff_sends_keys_without_explicit_refocus(self) -> None:
        for preserve in (False, True):
            with self.subTest(preserve_focus=preserve):
                harness = object.__new__(HARNESS.Goal07Harness)
                harness.ui_timeout = 1.0
                harness.focus_editor = mock.Mock()
                harness._require_foreground = mock.Mock()
                harness.win32 = mock.Mock()
                harness._approve_consent = mock.Mock()
                harness.wait_name = mock.Mock()
                app, provider = mock.Mock(), mock.Mock()
                app.hwnd = 73
                harness.win32.foreground_focus.return_value = app.hwnd

                harness.run_review(app, provider, preserve_focus=preserve)

                self.assertEqual(harness.focus_editor.call_count, 0 if preserve else 1)
                self.assertEqual(harness._require_foreground.call_count, 0 if preserve else 1)
                if preserve:
                    harness.win32.foreground_focus.assert_called_once_with(app.hwnd)
                harness.win32.send_shortcut.assert_called_once_with(73, runtime.VK_RETURN)
                harness._approve_consent.assert_called_once_with(
                    app, "REVIEW_CONSENT_HOLD_FAILED", provider=provider,
                    expected_request_count=0, open_gate=provider.grant_review_consent,
                )
                harness.wait_name.assert_called_once_with(
                    app, HARNESS.REVIEW_RESULT_MARKER, "Text", "REVIEW_RESULT_UIA_TIMEOUT", 60.0
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

                harness.win32.send_shortcut.assert_not_called()
                harness.win32.send_inputs.assert_not_called()
                harness.focus_editor.assert_not_called()
                harness._require_foreground.assert_not_called()

    def test_trust_scenario_requires_handoff_before_review_or_editor_helpers(self) -> None:
        for broken in (False, True):
            with self.subTest(broken=broken):
                harness = object.__new__(HARNESS.Goal07Harness)
                harness.ui_timeout = 1.0
                app, provider = mock.Mock(), mock.Mock()
                app.hwnd = 73
                app.security_context.evidence.return_value = process_context()
                app.process.poll.return_value = None
                provider.start.return_value = provider
                harness.profile = mock.Mock(return_value=(Path("unused"),) * 6)
                harness.launch_app = mock.Mock(return_value=app)
                harness.menu_command = mock.Mock()
                harness._press = mock.Mock()
                harness.confirm_system_alert = mock.Mock()
                harness.name_present = mock.Mock(return_value=False)
                harness.wait_name = mock.Mock()
                harness.editor_control = mock.Mock()
                harness.activate_source_layout = mock.Mock(side_effect=AssertionError("premature Source"))
                harness.run_review = mock.Mock()
                harness.request_revision = mock.Mock()
                harness.decide_hunks = mock.Mock()
                harness.undo_decision = mock.Mock()
                harness.done = mock.Mock()
                harness._require_foreground = mock.Mock(return_value=True)
                final_text = HARNESS.apply_edits(
                    HARNESS.HTML_EDITOR_SOURCE_BYTES,
                    PROVIDER.fixture_edits(HARNESS.HTML_EDITOR_SOURCE_TEXT),
                ).decode("utf-8")
                proposal_line = next(
                    line for line in final_text.split("\n")
                    if PROVIDER.fixture_edits(HARNESS.HTML_EDITOR_SOURCE_TEXT)[0]["replacement"] in line
                )
                harness._named_texts = mock.Mock(return_value=proposal_line)
                harness.editor_fingerprint = mock.Mock(side_effect=[
                    runtime.fingerprint_bytes(HARNESS.HTML_EDITOR_SOURCE_BYTES),
                    runtime.fingerprint_text(final_text),
                ])
                harness.win32 = mock.Mock()
                harness.win32.foreground_focus.return_value = 0 if broken else app.hwnd
                harness.quit_via_menu = mock.Mock()
                harness.click_lifecycle_decision = mock.Mock()
                harness.wait_process_exit = mock.Mock(
                    side_effect=lambda _app: setattr(app.process.poll, "return_value", 0)
                )
                harness.reap = mock.Mock()
                harness._finalize_observations = lambda _provider, _root, _app, obs: obs
                order = mock.Mock()
                for name in ("menu_command", "editor_control", "run_review", "editor_fingerprint"):
                    order.attach_mock(getattr(harness, name), name)
                trust = mock.Mock()
                for name in ("_press", "confirm_system_alert", "wait_name"):
                    trust.attach_mock(getattr(harness, name), name)

                def wait_once(check, _timeout, code, **_kwargs):
                    if not check():
                        raise runtime.HarnessFailure(code)

                with (
                    mock.patch.object(PROVIDER, "LoopbackRevisionServer", return_value=provider),
                    mock.patch.object(HARNESS, "wait_until", side_effect=wait_once),
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
                        self.assertTrue(observations["preview_matches_expected"])
                        self.assertTrue(observations["preview_contains_executable_text"])
                        self.assertTrue(observations["executable_matches_expected"])
                        self.assertEqual(order.mock_calls[:4], [
                            mock.call.menu_command(app, runtime.MENU_WEB_AUTOMATION_ID),
                            mock.call.menu_command(app, runtime.MENU_EDIT_AUTOMATION_ID),
                            mock.call.editor_control(app),
                            mock.call.run_review(app, provider, preserve_focus=True),
                        ])
                        # The Run scripts alert is outside UIA: it is answered with
                        # Enter only while trust is still off, then proven by the banner.
                        self.assertEqual(trust.mock_calls[:3], [
                            mock.call._press(app, HARNESS.TRUST_SCRIPTS_AUTOMATION_ID, "TRUST_BUTTON"),
                            mock.call.confirm_system_alert(
                                app, mock.ANY, "TRUST_ENABLED_BEFORE_CONFIRMATION"
                            ),
                            mock.call.wait_name(
                                app, HARNESS.TRUSTED_BANNER, "Text", "TRUST_ENABLE_STATE_TIMEOUT"
                            ),
                        ])
                        self.assertEqual(
                            [call.args[1] for call in harness._press.call_args_list],
                            [HARNESS.TRUST_SCRIPTS_AUTOMATION_ID],
                        )
                        still_undecided = harness.confirm_system_alert.call_args.args[1]
                        self.assertTrue(still_undecided())
                        harness.name_present.return_value = True
                        self.assertFalse(still_undecided())
                        harness.name_present.assert_called_with(app, HARNESS.TRUSTED_BANNER, "Text")
                        harness.wait_name.assert_any_call(
                            app, HARNESS.RESTRICTED_BANNER, "Text", "TRUST_RESTRICTED_STATE_TIMEOUT"
                        )
                        harness.request_revision.assert_called_once_with(
                            app, provider=provider,
                            hunks=len(PROVIDER.fixture_edits(HARNESS.HTML_EDITOR_SOURCE_TEXT)),
                        )
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

    def test_proposal_text_joins_only_the_exposed_group_text_lines(self) -> None:
        def child(name: object, control_type: str = "Text") -> object:
            item = mock.Mock()
            item.element_info.name = name
            item.element_info.control_type = control_type
            return item

        group = mock.Mock()
        group.element_info.name = HARNESS.REVISION_PROPOSAL_AFTER_NAME
        group.children.return_value = [
            child("approved"), child("Accept", "Button"), child(None), child("preview"),
        ]
        harness = object.__new__(HARNESS.Goal07Harness)
        harness.find_control = mock.Mock(return_value=group)

        def texts() -> str:
            return harness._named_texts(
                mock.Mock(),
                HARNESS.REVISION_PROPOSAL_AFTER_AUTOMATION_ID,
                HARNESS.REVISION_PROPOSAL_AFTER_NAME,
                "P",
            )

        self.assertEqual(texts(), "approved\npreview")
        self.assertEqual(harness.find_control.call_args.args[2], "Group")

        group.children.return_value = [child("Accept", "Button")]
        with self.assertRaisesRegex(HARNESS.HarnessFailure, "P_TEXT_UNAVAILABLE"):
            texts()

        group.children.return_value = [child("approved")]
        group.element_info.name = "Something else"
        with self.assertRaisesRegex(HARNESS.HarnessFailure, "P_UIA_CONTRACT_MISMATCH"):
            texts()

    def test_answer_input_uses_the_focused_unicode_keyboard_path(self) -> None:
        harness = object.__new__(HARNESS.Goal07Harness)
        harness.ui_timeout = 0.1
        events: list[tuple[str, object]] = []
        value = mock.Mock(CurrentValue="")

        class Control:
            iface_value = value

            def set_focus(self) -> None:
                events.append(("focus", None))

        control = Control()
        app = mock.Mock(hwnd=41)
        harness.find_control = lambda *_args, **_kwargs: control
        harness.control_by_id = lambda *_args, **_kwargs: control
        harness.win32 = mock.Mock()

        def type_text(hwnd: int, text: str) -> None:
            events.append(("send-unicode", (hwnd, text)))
            value.CurrentValue = text

        harness.win32.send_unicode.side_effect = type_text
        harness._press = lambda _app, name, code: events.append(("press", (name, code)))

        harness._set_answer(app, "kept intent")

        self.assertEqual(
            events,
            [
                ("focus", None),
                ("send-unicode", (41, "kept intent")),
                ("press", (HARNESS.REVIEW_ANSWER_AUTOMATION_ID, "REVIEW_ANSWER_SUBMIT")),
            ],
        )

    def test_answer_input_times_out_before_submitting_a_wrong_value(self) -> None:
        harness = object.__new__(HARNESS.Goal07Harness)
        harness.ui_timeout = 0.01
        control = mock.Mock()
        control.iface_value.CurrentValue = "other"
        harness.find_control = lambda *_args, **_kwargs: control
        harness.control_by_id = lambda *_args, **_kwargs: control
        harness.win32 = mock.Mock()
        harness._press = mock.Mock()

        with self.assertRaisesRegex(runtime.HarnessFailure, "REVIEW_ANSWER_INPUT_TIMEOUT"):
            harness._set_answer(mock.Mock(hwnd=41), "kept intent")
        harness._press.assert_not_called()

    def test_hunk_decisions_wait_for_each_decided_count(self) -> None:
        harness = object.__new__(HARNESS.Goal07Harness)
        harness.ui_timeout = 1.0
        events: list[tuple[str, str]] = []
        # The decided hunk stays docked with its word for one poll, then the
        # next undecided hunk comes forward.
        docked_word = iter([True, False])
        harness._press = lambda _app, name, _code: events.append(("press", name))
        harness.wait_name = lambda _app, name, *_args: events.append(("wait", name))

        def name_present(_app, name, control_type):
            self.assertIn(name, HARNESS.REVISION_DECISION_WORDS)
            self.assertEqual(control_type, "Text")
            present = next(docked_word, False) if name == "Accepted" else False
            events.append(("docked", name)) if present else None
            return present

        harness.name_present = name_present

        harness.decide_hunks(mock.Mock(), [True, False])
        harness.undo_decision(mock.Mock(), 1, 2)
        harness.done(mock.Mock())

        self.assertEqual(
            events,
            [
                ("press", HARNESS.REVISION_ACCEPT_AUTOMATION_ID),
                ("wait", "1 of 2 decided"),
                ("docked", "Accepted"),
                ("press", HARNESS.REVISION_REJECT_AUTOMATION_ID),
                ("wait", "2 of 2 decided"),
                ("press", HARNESS.REVISION_UNDO_AUTOMATION_ID),
                ("wait", "1 of 2 decided"),
                ("press", HARNESS.REVISION_DONE_AUTOMATION_ID),
            ],
        )

        events.clear()
        harness.decide_hunks(mock.Mock(), [True], total=2)
        self.assertEqual(
            events,
            [("press", HARNESS.REVISION_ACCEPT_AUTOMATION_ID), ("wait", "1 of 2 decided")],
        )

    def test_selective_case_undoes_one_written_accept_before_deciding_again(self) -> None:
        harness = object.__new__(HARNESS.Goal07Harness)
        edits = PROVIDER.fixture_edits(HARNESS.EDITOR_SOURCE_TEXT)
        before = runtime.fingerprint_bytes(HARNESS.EDITOR_SOURCE_BYTES)
        selected = runtime.fingerprint_bytes(
            HARNESS.apply_edits(HARNESS.EDITOR_SOURCE_BYTES, edits[:1])
        )
        app, provider = mock.Mock(), mock.Mock()
        app.security_context.evidence.return_value = process_context()
        app.process.poll.return_value = 0
        provider.start.return_value = provider
        source = mock.Mock()
        harness.profile = mock.Mock(return_value=(Path("unused"),) * 5 + (source,))
        harness.launch_app = mock.Mock(return_value=app)
        harness.activate_source_layout = mock.Mock()
        harness.run_review = mock.Mock()
        harness.request_revision = mock.Mock()
        order = mock.Mock()
        harness.decide_hunks = order.decide_hunks
        harness.undo_decision = order.undo_decision
        harness.done = order.done
        harness.editor_fingerprint = order.editor_fingerprint
        order.editor_fingerprint.side_effect = [before, selected, before, selected]
        harness._require_foreground = mock.Mock(return_value=True)
        harness._finalize_observations = lambda _provider, _root, _app, obs: obs

        with (
            mock.patch.object(PROVIDER, "LoopbackRevisionServer", return_value=provider),
            mock.patch.object(
                HARNESS, "sha256_file", return_value=runtime.fingerprint_bytes(b"disk")
            ),
        ):
            observations = harness.scenario_selective_undo()

        self.assertEqual(
            order.mock_calls,
            [
                mock.call.editor_fingerprint(app),
                mock.call.decide_hunks(app, [True], total=len(edits)),
                mock.call.editor_fingerprint(app),
                mock.call.undo_decision(app, 0, len(edits)),
                mock.call.editor_fingerprint(app),
                mock.call.decide_hunks(app, [True] + [False] * (len(edits) - 1)),
                mock.call.done(app),
                mock.call.editor_fingerprint(app),
            ],
        )
        self.assertTrue(observations["selective_matches_expected"])
        self.assertTrue(observations["one_undo_transaction"])
        HARNESS.validate_passed_case(
            HARNESS.CASE_SELECTIVE_UNDO,
            {**common(HARNESS.CASE_SELECTIVE_UNDO), **observations},
            process_context(),
        )

    def test_part_changed_case_presses_a_off_the_editor_then_done(self) -> None:
        harness = object.__new__(HARNESS.Goal07Harness)
        harness.ui_timeout = 1.0
        before = runtime.fingerprint_bytes(HARNESS.EDITOR_SOURCE_BYTES)
        edited = runtime.fingerprint_text(HARNESS.STALE_EDIT_TEXT)
        app, provider = mock.Mock(hwnd=73), mock.Mock()
        app.security_context.evidence.return_value = process_context()
        app.process.poll.return_value = 0
        provider.start.return_value = provider
        harness.profile = mock.Mock(return_value=(Path("unused"),) * 6)
        harness.launch_app = mock.Mock(return_value=app)
        harness.activate_source_layout = mock.Mock()
        harness.run_review = mock.Mock()
        harness.request_revision = mock.Mock()
        harness._require_foreground = mock.Mock(return_value=True)
        harness._finalize_observations = lambda _provider, _root, _app, obs: obs
        order = mock.Mock()
        done_button = order.done_button
        harness.find_control = mock.Mock(return_value=done_button)
        harness.editor_fingerprint = order.editor_fingerprint
        order.editor_fingerprint.side_effect = [before, edited, edited, edited]
        harness.replace_editor = order.replace_editor
        harness.wait_name = order.wait_name
        harness.name_present = order.name_present
        order.name_present.return_value = False
        harness.source_editor_has_focus = order.source_editor_has_focus
        order.source_editor_has_focus.return_value = False
        harness.win32 = order.win32
        harness.done = order.done

        with (
            mock.patch.object(PROVIDER, "LoopbackRevisionServer", return_value=provider),
            mock.patch.object(
                HARNESS, "sha256_file", return_value=runtime.fingerprint_bytes(b"disk")
            ),
            mock.patch.object(HARNESS, "part_changed_apply_source_contract_ok", return_value=True),
        ):
            observations = harness.scenario_stale()

        hunks = len(PROVIDER.fixture_edits(HARNESS.EDITOR_SOURCE_TEXT))
        self.assertEqual(
            order.mock_calls,
            [
                mock.call.editor_fingerprint(app),
                mock.call.replace_editor(app, HARNESS.STALE_EDIT_TEXT),
                mock.call.editor_fingerprint(app),
                mock.call.wait_name(
                    app, HARNESS.REVISION_PART_CHANGED_TEXT, "Text",
                    "REVISION_PART_CHANGED_UIA_TIMEOUT",
                ),
                mock.call.name_present(app, HARNESS.REVISION_ACCEPT_NAME, "Button"),
                mock.call.done_button.set_focus(),
                mock.call.source_editor_has_focus(app),
                mock.call.win32.send_key(73, runtime.VK_A),
                mock.call.wait_name(
                    app, f"0 of {hunks} decided", "Text", "REVISION_DECISION_TIMEOUT"
                ),
                mock.call.editor_fingerprint(app),
                mock.call.done(app),
                mock.call.wait_name(
                    app, HARNESS.REVISION_NOTHING_APPROVED, "Text",
                    "REVISION_DONE_OUTCOME_TIMEOUT",
                ),
                mock.call.editor_fingerprint(app),
            ],
        )
        self.assertTrue(observations["stale_no_mutation"])
        HARNESS.validate_passed_case(
            HARNESS.CASE_STALE,
            {**common(HARNESS.CASE_STALE), **observations},
            process_context(),
        )

        order.name_present.return_value = True
        order.editor_fingerprint.side_effect = [before, edited]
        with (
            mock.patch.object(PROVIDER, "LoopbackRevisionServer", return_value=provider),
            mock.patch.object(
                HARNESS, "sha256_file", return_value=runtime.fingerprint_bytes(b"disk")
            ),
            self.assertRaisesRegex(runtime.HarnessFailure, "STALE_REVISION_OFFERS_ACCEPT"),
        ):
            harness.scenario_stale()

    def test_consent_gate_rejects_any_pre_click_request_snapshot(self) -> None:
        harness = object.__new__(HARNESS.Goal07Harness)
        harness.ui_timeout = 0.1
        harness.win32 = mock.Mock()
        harness.find_control = lambda *_args, **_kwargs: object()
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
                "REVIEW_CONSENT_HOLD_FAILED",
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
        harness.find_control = lambda *_args, **_kwargs: object()
        harness.hold_control = mock.Mock()
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
        provider.dispatch_consent_click.assert_not_called()
        harness.hold_control.assert_not_called()

    def test_consent_gate_opens_before_the_consent_hold(self) -> None:
        harness = object.__new__(HARNESS.Goal07Harness)
        harness.ui_timeout = 0.1
        events: list[str] = []
        send = object()
        harness.find_control = lambda _app, automation_id, *_args: (
            send if automation_id == HARNESS.CONSENT_SEND_AUTOMATION_ID else None
        )
        harness.hold_control = lambda _app, control, seconds, failure: events.append(
            f"hold-{control is send}-{seconds}-{failure}"
        )
        harness.click_control = lambda *_args: events.append("click")
        harness.invoke_control = lambda *_args: events.append("invoke")

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
            "REVIEW_CONSENT_HOLD_FAILED",
            provider=FakeProvider(),
            expected_request_count=0,
            open_gate=lambda: events.append("gate"),
        )

        self.assertEqual(
            events,
            ["gate", "marker-0", f"hold-True-{HARNESS.CONSENT_HOLD_SECONDS}-REVIEW_CONSENT_HOLD_FAILED"],
        )
        self.assertGreater(HARNESS.CONSENT_HOLD_SECONDS, 1.2)

    def test_press_requires_foreground_first_and_finds_the_control_by_id(self) -> None:
        harness = object.__new__(HARNESS.Goal07Harness)
        harness.ui_timeout = 0.1
        events: list[str] = []
        harness.win32 = mock.Mock()
        harness.win32.require_foreground.side_effect = lambda *_args: events.append(
            "foreground"
        )
        harness.find_control = lambda _app, automation_id, control_type, *_args: (
            events.append(f"find-{automation_id}-{control_type}") or object()
        )
        harness.click_control = lambda _control, failure: events.append(f"click-{failure}")

        harness._press(mock.Mock(hwnd=41), HARNESS.REVISION_ACCEPT_AUTOMATION_ID, "REVISION_ACCEPT")

        self.assertEqual(
            events,
            ["foreground", "find-revision-accept-Button", "click-REVISION_ACCEPT_CLICK_FAILED"],
        )

    def test_revision_request_submits_answer_prepares_and_waits_for_hunks(self) -> None:
        harness = object.__new__(HARNESS.Goal07Harness)
        harness.ui_timeout = 0.1
        order = mock.Mock()
        for name in ("_set_answer", "_press", "_approve_consent", "wait_name"):
            setattr(harness, name, getattr(order, name))
        app, provider = mock.Mock(), mock.Mock()

        harness.request_revision(app, provider=provider, hunks=2)

        self.assertEqual(
            order.mock_calls,
            [
                mock.call._set_answer(app, PROVIDER.ANSWER_TEXT),
                mock.call._press(app, HARNESS.PREPARE_REVISION_AUTOMATION_ID, "REVISION_PREPARE"),
                mock.call._approve_consent(
                    app,
                    "REVISION_CONSENT_HOLD_FAILED",
                    provider=provider,
                    expected_request_count=1,
                    open_gate=provider.grant_revision_consent,
                ),
                mock.call.wait_name(
                    app, "0 of 2 decided", "Text", "REVISION_PROPOSAL_UIA_TIMEOUT", 60.0
                ),
            ],
        )

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
