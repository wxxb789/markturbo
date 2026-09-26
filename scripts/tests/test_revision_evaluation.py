"""Goal 07 v2 owner-local revision evaluation contract tests."""

from __future__ import annotations

import hashlib
import io
import json
import os
import tempfile
import unittest
from contextlib import redirect_stderr, redirect_stdout
from pathlib import Path
from typing import Any
from unittest import mock

from scripts.markturbo_tools import evaluation
from scripts.markturbo_tools import revision_evaluation as revision
from scripts.markturbo_tools.native import goal07 as native_goal07


ROOT = Path(__file__).resolve().parents[2]


def digest(label: str) -> str:
    return hashlib.sha256(label.encode("utf-8")).hexdigest()


class Goal07Fixtures:
    def __init__(self, external_root: Path) -> None:
        self.root = external_root
        self.verification = evaluation.verify_manifest(ROOT)
        self.artifact_ids = tuple(item.artifact_id for item in self.verification.artifacts)
        self.registry_value = self._registry_value()
        self.registry_path = external_root / "eligibility.json"
        self.registry_path.write_text(json.dumps(self.registry_value, sort_keys=True) + "\n", encoding="utf-8")
        self.registry_anchor = revision.canonical_registry_digest(self.registry_value)
        self.machine_value = self._machine_value()
        self.machine_path = external_root / "machine-receipt.json"
        self.machine_path.write_text(json.dumps(self.machine_value, sort_keys=True) + "\n", encoding="utf-8")
        self.machine_anchor = hashlib.sha256(self.machine_path.read_bytes()).hexdigest()
        self.runner_anchor = self.machine_value["runner_executable_sha256"]
        self.native_value = self._native_value()
        self.native_path = external_root / "native-pass.json"
        self.native_path.write_text(json.dumps(self.native_value, sort_keys=True) + "\n", encoding="utf-8")
        self.native_anchor = hashlib.sha256(self.native_path.read_bytes()).hexdigest()
        self.native_executable_anchor = self.native_value["executable"]["sha256"]
        self.registry = revision.load_eligibility_registry(
            self.registry_path,
            self.verification,
            approved_registry_sha256=self.registry_anchor,
        )
        self.machine = revision.load_machine_receipt(
            self.machine_path,
            self.verification,
            approved_machine_receipt_sha256=self.machine_anchor,
            approved_runner_executable_sha256=self.runner_anchor,
        )
        self.native = revision.load_native_acceptance(
            self.native_path,
            approved_native_evidence_sha256=self.native_anchor,
            approved_native_executable_sha256=self.native_executable_anchor,
        )
        self.owner_cases = self._owner_cases()

    def _registry_value(self) -> dict[str, Any]:
        annotation_sha = next(
            item.sha256
            for item in self.verification.entries
            if item.path == "evaluation/goal-01/OWNER-ANNOTATIONS.md"
        )
        return {
            "schema": revision.REGISTRY_SCHEMA,
            "corpus_version": self.verification.corpus_version,
            "manifest_sha256": self.verification.manifest_sha256,
            "owner_annotations_sha256": annotation_sha,
            "cases": [
                {
                    "artifact_id": artifact.artifact_id,
                    "eligible": artifact.artifact_id == "TP-01",
                    "material_question_ids": ["TP-01-Q-01"] if artifact.artifact_id == "TP-01" else [],
                    "intent_violating_change_ids": ["TP-01-IV-01"] if artifact.artifact_id == "TP-01" else [],
                }
                for artifact in self.verification.artifacts
            ],
        }

    def _hunk(
        self,
        artifact_id: str,
        suffix: str = "01",
        start: int = 0,
        end: int = 4,
    ) -> dict[str, Any]:
        value = {
            "hunk_id": f"{artifact_id}-H-{suffix}",
            "source_start": start,
            "source_end": end,
            "replacement_sha256": revision.EMPTY_SHA256,
            "replacement_byte_count": 0,
            "rationale_sha256": digest(f"{artifact_id}:why"),
            "rationale_byte_count": 3,
            "runner_schema": revision.LOCAL_DIFF_RUNNER_SCHEMA,
            "local_diff_verified": True,
            "utf8_boundaries_verified": True,
        }
        value["displayed_diff_sha256"] = revision.diff_receipt_digest(value)
        return value

    def _machine_value(self) -> dict[str, Any]:
        cases: dict[str, Any] = {}
        for artifact in self.verification.artifacts:
            artifact_id = artifact.artifact_id
            changes: list[dict[str, Any]] = []
            if artifact_id == "TP-01":
                changes = [
                    {
                        "change_id": "TP-01-CH-01",
                        "intent_change_ids": ["TP-01-IV-01"],
                        "hunks": [self._hunk(artifact_id)],
                    },
                    {
                        "change_id": "TP-01-CH-02",
                        "intent_change_ids": [],
                        "hunks": [self._hunk(artifact_id, "02", 5, 9)],
                    },
                ]
            if len(artifact.files) == 1:
                editable = artifact.files[0].sha256
                editable_count = artifact.files[0].byte_count
            else:
                skill_file = next(item for item in artifact.files if item.path == f"{artifact.path}/SKILL.md")
                editable = skill_file.sha256
                editable_count = skill_file.byte_count
            cases[artifact_id] = {
                "artifact_id": artifact_id,
                "corpus_artifact_lens": artifact.lens,
                "corpus_artifact_sha256": artifact.sha256,
                "corpus_artifact_byte_count": artifact.byte_count,
                "request_artifact_sha256": artifact.sha256,
                "request_artifact_byte_count": artifact.byte_count,
                "review_scope_sha256": digest(f"{artifact_id}:review-scope"),
                "review_scope_byte_count": 30,
                "editable_source_sha256": editable,
                "editable_source_byte_count": editable_count,
                "source_binding_sha256": digest(f"{artifact_id}:source-binding"),
                "review_context_sha256": digest(f"{artifact_id}:review-context"),
                "changes": changes,
                "answered_material_question_ids": ["TP-01-Q-01"] if artifact_id == "TP-01" else [],
                "represented_question_ids": ["TP-01-Q-01"] if artifact_id == "TP-01" else [],
                "intentionally_omitted_question_ids": [],
                "question_coverage": (
                    [
                        {
                            "question_index": 0,
                            "question_id": "TP-01-Q-01",
                            "status": {"kind": "represented", "change_ids": [1]},
                        }
                    ]
                    if artifact_id == "TP-01"
                    else []
                ),
                "reject_all": {
                    "source_sha256": editable,
                    "result_sha256": editable,
                    "source_byte_count": editable_count,
                    "result_byte_count": editable_count,
                },
            }
            if artifact_id == "TP-01":
                cases[artifact_id].update(
                    {
                        "proposal_sha256": digest("TP-01:proposal"),
                        "source_revision": 0,
                        "source_generation": 0,
                        "artifact_lens_sha256": digest(artifact.lens),
                        "approved_output": {
                            "status": "composed",
                            "decision_file_sha256": digest("TP-01:decision-file"),
                            "decision_set_sha256": revision.canonical_registry_digest(
                                {
                                    "decisions": [
                                        {"change_id": 0, "accepted": False},
                                        {"change_id": 1, "accepted": True},
                                    ]
                                }
                            ),
                            "proposal_sha256": digest("TP-01:proposal"),
                            "decision_count": 2,
                            "result_sha256": digest("TP-01:approved"),
                            "result_byte_count": 20,
                        },
                    }
                )
        return {
            "schema": revision.MACHINE_RECEIPT_SCHEMA,
            "corpus_version": self.verification.corpus_version,
            "manifest_sha256": self.verification.manifest_sha256,
            "runner_schema": revision.LOCAL_DIFF_RUNNER_SCHEMA,
            "runner_executable_sha256": digest("goal07-runner.exe"),
            "cases": cases,
        }

    def _native_value(self) -> dict[str, Any]:
        executable = digest("markturbo.exe")
        parent = {"session_id": 1, "integrity_rid": 1000, "integrity": "medium"}
        fingerprint = {"byte_count": 1, "sha256": digest("native-before")}
        changed_fingerprint = {"byte_count": 2, "sha256": digest("native-after")}
        runtime_scan = {
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
        common = {
            "process_context": parent,
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
            "runtime_scan": runtime_scan,
        }
        observations: dict[str, dict[str, Any]] = {}
        for case_id in native_goal07.REQUIRED_CASE_IDS:
            item = {"flow": native_goal07.CASE_FLOWS[case_id], **common}
            if case_id == native_goal07.CASE_REJECT_ALL:
                item.update(
                    {
                        "proposal_received": True,
                        "reviewed_source_match": True,
                        "editor_before": fingerprint,
                        "editor_after": fingerprint,
                        "source_before": fingerprint,
                        "source_after": fingerprint,
                        "reject_all_byte_identity": True,
                        "dirty_before": False,
                        "dirty_after": False,
                        "reject_all_dirty_unchanged": True,
                    }
                )
            elif case_id == native_goal07.CASE_SELECTIVE_UNDO:
                item.update(
                    {
                        "editor_before": fingerprint,
                        "selective_preview": changed_fingerprint,
                        "editor_after_apply": changed_fingerprint,
                        "editor_after_undo": fingerprint,
                        "source_before": fingerprint,
                        "source_after": fingerprint,
                        "selective_matches_editor": True,
                        "selective_matches_expected": True,
                        "one_undo_transaction": True,
                        "undo_count": 1,
                    }
                )
            elif case_id == native_goal07.CASE_ACCEPT_ALL_PREVIEW:
                item.update(
                    {
                        "editor_before_apply": fingerprint,
                        "editor_after_preview": fingerprint,
                        "preview_fingerprint": changed_fingerprint,
                        "final_preview": changed_fingerprint,
                        "accept_all_matches_expected": True,
                        "copy_preview_fingerprint": changed_fingerprint,
                        "copy_preview_exact": True,
                        "copy_editor_before": fingerprint,
                        "copy_editor_after": fingerprint,
                        "copy_editor_unchanged": True,
                        "copy_dirty_before": False,
                        "copy_dirty_after": False,
                        "copy_dirty_unchanged": True,
                        "copy_source_before": fingerprint,
                        "copy_source_after": fingerprint,
                        "copy_source_unchanged": True,
                    }
                )
            elif case_id == native_goal07.CASE_STALE:
                item.update(
                    {
                        "editor_before": fingerprint,
                        "editor_after_edit": changed_fingerprint,
                        "editor_after_activation_attempt": changed_fingerprint,
                        "source_before": fingerprint,
                        "source_after": fingerprint,
                        "stale_proposal_received": True,
                        "stale_visible": True,
                        "apply_activation_attempted": True,
                        "apply_disabled_source_contract": True,
                        "stale_no_mutation": True,
                        "apply_control_observed": True,
                        "apply_accessibility_reported_enabled": True,
                    }
                )
            elif case_id == native_goal07.CASE_SAVE_CONFLICT:
                item.update(
                    {
                        "editor_after_apply": changed_fingerprint,
                        "external_source_before": fingerprint,
                        "external_source_after": fingerprint,
                        "save_shortcut_sent": True,
                        "conflict_visible_before_save": True,
                        "safe_save_conflict_visible": True,
                        "safe_save_no_overwrite": True,
                    }
                )
            else:
                item.update(
                    {
                        "editor_before_apply": fingerprint,
                        "editor_after_apply": changed_fingerprint,
                        "restricted_after_apply": True,
                        "executable_change": True,
                        "executable_expected": changed_fingerprint,
                        "executable_matches_expected": True,
                        "trust_before": True,
                        "trust_revocation_order_source_contract": True,
                        "preview_inert_source_contract": True,
                        "preview_fingerprint": changed_fingerprint,
                        "preview_matches_expected": True,
                        "preview_contains_executable_text": True,
                    }
                )
            observations[case_id] = item
        return {
            "schema": revision.NATIVE_ACCEPTANCE_SCHEMA,
            "schema_version": 1,
            "status": "PASS",
            "started_at_utc": "2026-09-18T00:00:00Z",
            "completed_at_utc": "2026-09-18T00:00:01Z",
            "transport": {
                "mode": "keyless_loopback",
                "provider": "openai-responses",
                "deterministic": True,
                "request_count": 12,
            },
            "executable": {
                "expected_sha256": executable,
                "sha256": executable,
                "byte_count": 1,
                "hash_verified": True,
                "copied_sha256": executable,
                "copy_hash_verified": True,
                "format": "PE32+",
                "machine": "x86_64",
                "machine_code": native_goal07.IMAGE_FILE_MACHINE_AMD64,
                "optional_magic": native_goal07.PE32_PLUS_MAGIC,
            },
            "environment": {
                "platform": "Windows 11",
                "windows_major": 10,
                "windows_minor": 0,
                "windows_build": 22621,
                "architecture": "x86_64",
                "native_machine_code": native_goal07.IMAGE_FILE_MACHINE_AMD64,
                "python_pointer_bits": 64,
                "wts_state": "WTSActive",
                "active_console_session_id": 1,
                "harness_is_console_session": True,
                "input_desktop": "Default",
                "thread_desktop": "Default",
                "harness_process": parent,
            },
            "cases": [
                {
                    "id": case_id,
                    "status": "PASS",
                    "duration_ms": 0,
                    "reason_code": None,
                    "failure_type": None,
                    "observations": observations[case_id],
                }
                for case_id in sorted(revision.NATIVE_REQUIRED_CASE_IDS)
            ],
            "summary": {
                "required_case_count": len(revision.NATIVE_REQUIRED_CASE_IDS),
                "passed_case_count": len(revision.NATIVE_REQUIRED_CASE_IDS),
                "blocked_case_count": 0,
                "failed_case_count": 0,
                "not_run_case_count": 0,
            },
        }

    def _owner_cases(self) -> dict[str, dict[str, Any]]:
        result: dict[str, dict[str, Any]] = {}
        for artifact in self.verification.artifacts:
            artifact_id = artifact.artifact_id
            approved_output = self.machine_value["cases"][artifact_id].get("approved_output", {})
            result[artifact_id] = {
                "artifact_id": artifact_id,
                "change_decisions": (
                    [
                        {"change_id": "TP-01-CH-01", "decision": "reject"},
                        {"change_id": "TP-01-CH-02", "decision": "accept"},
                    ]
                    if artifact_id == "TP-01"
                    else []
                ),
                "intent_judgment": "preserved" if artifact_id == "TP-01" else "not_judged",
                "question_coverage": (
                    [{"question_id": "TP-01-Q-01", "status": "represented"}]
                    if artifact_id == "TP-01"
                    else []
                ),
                "clearer_due_to_answered_question": artifact_id == "TP-01",
                "approved_output_sha256": approved_output.get("result_sha256"),
                "approved_proposal_sha256": approved_output.get("proposal_sha256"),
                "approved_decision_set_sha256": approved_output.get("decision_set_sha256"),
                "approved_decision_file_sha256": approved_output.get("decision_file_sha256"),
            }
        return result

    def record(self) -> dict[str, Any]:
        return revision.evidence_from_owner_inputs(
            self.verification,
            self.registry,
            self.machine,
            self.owner_cases,
            approved_registry_sha256=self.registry_anchor,
            approved_machine_receipt_sha256=self.machine_anchor,
            approved_runner_executable_sha256=self.runner_anchor,
            native_acceptance=self.native,
            approved_native_evidence_sha256=self.native_anchor,
            approved_native_executable_sha256=self.native_executable_anchor,
            created_at="2026-09-18T00:00:00Z",
        )


class RevisionEvaluationV2Tests(unittest.TestCase):
    def setUp(self) -> None:
        self.temp = tempfile.TemporaryDirectory()
        self.fixtures = Goal07Fixtures(Path(self.temp.name))

    def tearDown(self) -> None:
        self.temp.cleanup()

    def test_schema_is_v2_and_completion_is_explicit_all_cases(self) -> None:
        evidence = self.fixtures.record()
        self.assertEqual(evidence["schema"], revision.EVIDENCE_SCHEMA)
        self.assertNotIn("threshold", json.dumps(evidence))
        self.assertTrue(evidence["evaluation"]["all_cases_satisfied"])
        self.assertEqual(evidence["evaluation"]["clearer_due_to_answered_question_count"], 1)
        self.assertEqual(evidence["native_acceptance"]["raw_sha256"], self.fixtures.native_anchor)
        self.assertEqual(evidence["native_acceptance"]["executable_sha256"], self.fixtures.native_executable_anchor)

    def test_native_case_contract_matches_the_goal07_harness(self) -> None:
        self.assertEqual(
            revision.NATIVE_REQUIRED_CASE_IDS,
            frozenset(native_goal07.REQUIRED_CASE_IDS),
        )

    def test_machine_receipt_separates_three_source_scopes(self) -> None:
        case = self.fixtures.machine.by_artifact()["TP-01"]
        self.assertEqual(
            {
                "corpus_artifact_sha256",
                "review_scope_sha256",
                "editable_source_sha256",
            },
            {key for key in case if key.endswith("_sha256") and key in {"corpus_artifact_sha256", "review_scope_sha256", "editable_source_sha256"}},
        )
        self.assertNotIn("dirty_before", json.dumps(case))
        self.assertNotIn("undo", json.dumps(case).lower())
        self.assertNotIn("filesystem", json.dumps(case).lower())

    def test_machine_receipt_preserves_independent_source_binding(self) -> None:
        case = self.fixtures.machine.by_artifact()["TP-01"]
        source_binding = case["source_binding_sha256"]
        editable_source = case["editable_source_sha256"]
        self.assertIsInstance(source_binding, str)
        self.assertIsInstance(editable_source, str)
        self.assertNotEqual(source_binding, editable_source)

    def test_machine_source_binding_requires_lowercase_sha256(self) -> None:
        for invalid in ("z" * 63, "A" * 64):
            machine = json.loads(json.dumps(self.fixtures.machine_value))
            machine["cases"]["TP-01"]["source_binding_sha256"] = invalid
            path = Path(self.temp.name) / f"invalid-source-binding-{len(invalid)}.json"
            path.write_text(json.dumps(machine, sort_keys=True) + "\n", encoding="utf-8")
            with self.subTest(invalid=invalid), self.assertRaisesRegex(
                revision.RevisionEvaluationError, "source_binding_sha256"
            ):
                revision.load_machine_receipt(
                    path,
                    self.fixtures.verification,
                    approved_machine_receipt_sha256=hashlib.sha256(path.read_bytes()).hexdigest(),
                    approved_runner_executable_sha256=self.fixtures.runner_anchor,
                )

    def test_machine_receipt_uses_review_context_not_legacy_review_output(self) -> None:
        machine = json.loads(json.dumps(self.fixtures.machine_value))
        machine["cases"]["TP-01"]["review_context_sha256"] = digest("TP-01:review-context")
        path = Path(self.temp.name) / "review-context-machine.json"
        path.write_text(json.dumps(machine, sort_keys=True) + "\n", encoding="utf-8")
        receipt = revision.load_machine_receipt(
            path,
            self.fixtures.verification,
            approved_machine_receipt_sha256=hashlib.sha256(path.read_bytes()).hexdigest(),
            approved_runner_executable_sha256=self.fixtures.runner_anchor,
        )
        self.assertEqual(
            receipt.by_artifact()["TP-01"]["review_context_sha256"],
            digest("TP-01:review-context"),
        )
        legacy = json.loads(json.dumps(self.fixtures.machine_value))
        del legacy["cases"]["TP-01"]["review_context_sha256"]
        legacy["cases"]["TP-01"]["review_output_sha256"] = digest("legacy-review-output")
        legacy_path = Path(self.temp.name) / "legacy-review-output-machine.json"
        legacy_path.write_text(json.dumps(legacy, sort_keys=True) + "\n", encoding="utf-8")
        with self.assertRaisesRegex(revision.RevisionEvaluationError, "invalid schema"):
            revision.load_machine_receipt(
                legacy_path,
                self.fixtures.verification,
                approved_machine_receipt_sha256=hashlib.sha256(legacy_path.read_bytes()).hexdigest(),
                approved_runner_executable_sha256=self.fixtures.runner_anchor,
            )

    def test_production_rust_machine_extras_round_trip_without_content(self) -> None:
        machine = json.loads(json.dumps(self.fixtures.machine_value))
        case = machine["cases"]["TP-01"]
        case.update(
            {
                "review_context_sha256": digest("review-context"),
                "answers_sha256": digest("answers"),
                "raw_revision_response_sha256": digest("raw-revision"),
                "proposal_sha256": digest("proposal"),
                "displayed_diff_sha256": digest("displayed-diff"),
                "question_coverage": [
                    {
                        "question_index": 0,
                        "question_id": "TP-01-Q-01",
                        "status": {"kind": "represented", "change_ids": [0]},
                    }
                ],
                "approved_output": {
                    "status": "not_composed",
                    "decision_file_sha256": None,
                    "decision_set_sha256": None,
                    "proposal_sha256": None,
                    "decision_count": 0,
                    "result_sha256": None,
                    "result_byte_count": None,
                },
            }
        )
        path = Path(self.temp.name) / "rust-machine.json"
        path.write_text(json.dumps(machine, sort_keys=True) + "\n", encoding="utf-8")
        receipt = revision.load_machine_receipt(
            path,
            self.fixtures.verification,
            approved_machine_receipt_sha256=hashlib.sha256(path.read_bytes()).hexdigest(),
            approved_runner_executable_sha256=self.fixtures.runner_anchor,
        )
        self.assertEqual(
            receipt.by_artifact()["TP-01"]["question_coverage"][0]["status"]["kind"],
            "represented",
        )
        owner_cases = json.loads(json.dumps(self.fixtures.owner_cases))
        for field in (
            "approved_output_sha256",
            "approved_proposal_sha256",
            "approved_decision_set_sha256",
            "approved_decision_file_sha256",
        ):
            owner_cases["TP-01"][field] = None
        evidence = revision.evidence_from_owner_inputs(
            self.fixtures.verification,
            self.fixtures.registry,
            receipt,
            owner_cases,
            approved_registry_sha256=self.fixtures.registry_anchor,
            approved_machine_receipt_sha256=hashlib.sha256(path.read_bytes()).hexdigest(),
            approved_runner_executable_sha256=self.fixtures.runner_anchor,
            native_acceptance=self.fixtures.native,
            approved_native_evidence_sha256=self.fixtures.native_anchor,
            approved_native_executable_sha256=self.fixtures.native_executable_anchor,
        )
        result = next(item for item in evidence["results"] if item["artifact_id"] == "TP-01")
        self.assertEqual(result["machine_question_coverage"][0]["status"]["kind"], "represented")
        self.assertEqual(
            result["source_binding_sha256"],
            receipt.by_artifact()["TP-01"]["source_binding_sha256"],
        )
        self.assertEqual(
            result["review_context_sha256"],
            receipt.by_artifact()["TP-01"]["review_context_sha256"],
        )

    def test_empty_proposal_and_zero_byte_deletion_are_valid(self) -> None:
        machine = json.loads(json.dumps(self.fixtures.machine_value))
        machine["cases"]["TP-02"]["changes"] = []
        machine["cases"]["TP-01"]["changes"][0]["hunks"][0]["replacement_sha256"] = revision.EMPTY_SHA256
        path = Path(self.temp.name) / "empty-machine.json"
        path.write_text(json.dumps(machine, sort_keys=True) + "\n", encoding="utf-8")
        loaded = revision.load_machine_receipt(
            path,
            self.fixtures.verification,
            approved_machine_receipt_sha256=hashlib.sha256(path.read_bytes()).hexdigest(),
            approved_runner_executable_sha256=self.fixtures.runner_anchor,
        )
        self.assertEqual(loaded.by_artifact()["TP-02"]["changes"], [])
        self.assertEqual(
            loaded.by_artifact()["TP-01"]["changes"][0]["hunks"][0]["replacement_byte_count"], 0
        )

    def test_owner_decisions_are_per_change_id_not_per_hunk(self) -> None:
        evidence = self.fixtures.record()
        result = next(item for item in evidence["results"] if item["artifact_id"] == "TP-01")
        self.assertEqual(result["changes"][0]["decision"], "reject")
        self.assertIn("hunks", result["changes"][0])
        self.assertNotIn("decision", result["changes"][0]["hunks"][0])

    def test_owner_enum_list_fails_closed(self) -> None:
        owners = json.loads(json.dumps(self.fixtures.owner_cases))
        owners["TP-01"]["intent_judgment"] = []
        with self.assertRaisesRegex(revision.RevisionEvaluationError, "owner intent_judgment is invalid"):
            revision.evidence_from_owner_inputs(
                self.fixtures.verification,
                self.fixtures.registry,
                self.fixtures.machine,
                owners,
                approved_registry_sha256=self.fixtures.registry_anchor,
                approved_machine_receipt_sha256=self.fixtures.machine_anchor,
                approved_runner_executable_sha256=self.fixtures.runner_anchor,
                native_acceptance=self.fixtures.native,
                approved_native_evidence_sha256=self.fixtures.native_anchor,
                approved_native_executable_sha256=self.fixtures.native_executable_anchor,
            )

    def test_owner_judgment_cannot_replay_against_changed_approved_output(self) -> None:
        machine_payload = json.loads(json.dumps(self.fixtures.machine_value))
        hunk = machine_payload["cases"]["TP-01"]["changes"][0]["hunks"][0]
        hunk["rationale_sha256"] = digest("TP-01:new-rationale")
        hunk["displayed_diff_sha256"] = revision.diff_receipt_digest(hunk)
        machine_payload["cases"]["TP-01"]["approved_output"]["result_sha256"] = digest(
            "TP-01:approved-v2"
        )
        path = Path(self.temp.name) / "changed-machine.json"
        path.write_text(json.dumps(machine_payload, sort_keys=True) + "\n", encoding="utf-8")
        machine_anchor = hashlib.sha256(path.read_bytes()).hexdigest()
        changed_machine = revision.load_machine_receipt(
            path,
            self.fixtures.verification,
            approved_machine_receipt_sha256=machine_anchor,
            approved_runner_executable_sha256=self.fixtures.runner_anchor,
        )
        with self.assertRaisesRegex(revision.RevisionEvaluationError, "approved output"):
            revision.evidence_from_owner_inputs(
                self.fixtures.verification,
                self.fixtures.registry,
                changed_machine,
                self.fixtures.owner_cases,
                approved_registry_sha256=self.fixtures.registry_anchor,
                approved_machine_receipt_sha256=machine_anchor,
                approved_runner_executable_sha256=self.fixtures.runner_anchor,
                native_acceptance=self.fixtures.native,
                approved_native_evidence_sha256=self.fixtures.native_anchor,
                approved_native_executable_sha256=self.fixtures.native_executable_anchor,
            )

    def test_missing_clearer_confirmation_fails_all_cases(self) -> None:
        owners = json.loads(json.dumps(self.fixtures.owner_cases))
        owners["TP-01"]["clearer_due_to_answered_question"] = False
        evidence = revision.evidence_from_owner_inputs(
            self.fixtures.verification,
            self.fixtures.registry,
            self.fixtures.machine,
            owners,
            approved_registry_sha256=self.fixtures.registry_anchor,
            approved_machine_receipt_sha256=self.fixtures.machine_anchor,
            approved_runner_executable_sha256=self.fixtures.runner_anchor,
            native_acceptance=self.fixtures.native,
            approved_native_evidence_sha256=self.fixtures.native_anchor,
            approved_native_executable_sha256=self.fixtures.native_executable_anchor,
        )
        self.assertFalse(evidence["evaluation"]["all_cases_satisfied"])
        self.assertEqual(evidence["evaluation"]["clearer_due_to_answered_question_count"], 0)

    def test_forbidden_accepted_change_blocks_all_cases(self) -> None:
        owners = json.loads(json.dumps(self.fixtures.owner_cases))
        owners["TP-01"]["change_decisions"][0]["decision"] = "accept"
        evidence = revision.evidence_from_owner_inputs(
            self.fixtures.verification,
            self.fixtures.registry,
            self.fixtures.machine,
            owners,
            approved_registry_sha256=self.fixtures.registry_anchor,
            approved_machine_receipt_sha256=self.fixtures.machine_anchor,
            approved_runner_executable_sha256=self.fixtures.runner_anchor,
            native_acceptance=self.fixtures.native,
            approved_native_evidence_sha256=self.fixtures.native_anchor,
            approved_native_executable_sha256=self.fixtures.native_executable_anchor,
        )
        self.assertFalse(evidence["evaluation"]["all_cases_satisfied"])

    def test_native_anchor_is_separate_from_machine_receipt_anchor(self) -> None:
        with self.assertRaisesRegex(revision.RevisionEvaluationError, "native acceptance raw digest"):
            revision.load_native_acceptance(
                self.fixtures.native_path,
                approved_native_evidence_sha256=digest("wrong-native-evidence"),
                approved_native_executable_sha256=self.fixtures.native_executable_anchor,
            )
        with self.assertRaisesRegex(revision.RevisionEvaluationError, "native executable digest"):
            revision.load_native_acceptance(
                self.fixtures.native_path,
                approved_native_evidence_sha256=self.fixtures.native_anchor,
                approved_native_executable_sha256=digest("wrong-native-executable"),
            )

    def test_native_acceptance_requires_the_exact_goal07_case_set(self) -> None:
        for cases in (
            self.fixtures.native_value["cases"][:-1],
            [*self.fixtures.native_value["cases"], self.fixtures.native_value["cases"][0]],
            [
                *self.fixtures.native_value["cases"][:-1],
                {"id": "invented_case", "status": "PASS"},
            ],
        ):
            with self.subTest(case_ids=[case["id"] for case in cases]):
                value = {**self.fixtures.native_value, "cases": cases}
                path = self.fixtures.native_path.parent / "invalid-native-cases.json"
                path.write_text(json.dumps(value, sort_keys=True) + "\n", encoding="utf-8")
                anchor = hashlib.sha256(path.read_bytes()).hexdigest()
                with self.assertRaisesRegex(
                    revision.RevisionEvaluationError,
                    "does not prove PASS for every case",
                ):
                    revision.load_native_acceptance(
                        path,
                        approved_native_evidence_sha256=anchor,
                        approved_native_executable_sha256=self.fixtures.native_executable_anchor,
                    )

    def test_native_acceptance_is_instance_bound(self) -> None:
        forged = revision.NativeAcceptance(
            raw_sha256=self.fixtures.native_anchor,
            executable_sha256=self.fixtures.native_executable_anchor,
        )
        with self.assertRaisesRegex(revision.RevisionEvaluationError, "validated PASS"):
            revision.evidence_from_owner_inputs(
                self.fixtures.verification,
                self.fixtures.registry,
                self.fixtures.machine,
                self.fixtures.owner_cases,
                approved_registry_sha256=self.fixtures.registry_anchor,
                approved_machine_receipt_sha256=self.fixtures.machine_anchor,
                approved_runner_executable_sha256=self.fixtures.runner_anchor,
                native_acceptance=forged,
                approved_native_evidence_sha256=self.fixtures.native_anchor,
                approved_native_executable_sha256=self.fixtures.native_executable_anchor,
            )

    def test_machine_receipt_is_deeply_immutable_and_instance_bound(self) -> None:
        with self.assertRaises(TypeError):
            self.fixtures.machine.cases[0][1]["cases"] = []  # type: ignore[index]
        copied = self.fixtures.machine.by_artifact()
        copied["TP-01"]["changes"][0]["hunks"][0]["source_start"] = 99
        self.assertEqual(self.fixtures.machine.by_artifact()["TP-01"]["changes"][0]["hunks"][0]["source_start"], 0)
        forged = revision.MachineReceipt(
            schema=self.fixtures.machine.schema,
            corpus_version=self.fixtures.machine.corpus_version,
            manifest_sha256=self.fixtures.machine.manifest_sha256,
            runner_schema=self.fixtures.machine.runner_schema,
            runner_executable_sha256=self.fixtures.machine.runner_executable_sha256,
            receipt_sha256=self.fixtures.machine.receipt_sha256,
            normalized_sha256=self.fixtures.machine.normalized_sha256,
            cases=self.fixtures.machine.cases,
        )
        with self.assertRaisesRegex(revision.RevisionEvaluationError, "load_machine_receipt"):
            revision.evidence_from_owner_inputs(
                self.fixtures.verification,
                self.fixtures.registry,
                forged,
                self.fixtures.owner_cases,
                approved_registry_sha256=self.fixtures.registry_anchor,
                approved_machine_receipt_sha256=self.fixtures.machine_anchor,
                approved_runner_executable_sha256=self.fixtures.runner_anchor,
                native_acceptance=self.fixtures.native,
                approved_native_evidence_sha256=self.fixtures.native_anchor,
                approved_native_executable_sha256=self.fixtures.native_executable_anchor,
            )

    def test_machine_ui_claims_are_rejected(self) -> None:
        payload = json.loads(json.dumps(self.fixtures.machine_value))
        payload["cases"]["TP-01"]["dirty_before"] = False
        path = Path(self.temp.name) / "dirty-machine.json"
        path.write_text(json.dumps(payload, sort_keys=True) + "\n", encoding="utf-8")
        with self.assertRaisesRegex(revision.RevisionEvaluationError, "invalid schema|machine UI claim"):
            revision.load_machine_receipt(
                path,
                self.fixtures.verification,
                approved_machine_receipt_sha256=hashlib.sha256(path.read_bytes()).hexdigest(),
                approved_runner_executable_sha256=self.fixtures.runner_anchor,
            )

    def test_reject_all_has_no_dirty_undo_or_trust_claims(self) -> None:
        evidence = self.fixtures.record()
        serialized = json.dumps(evidence).lower()
        self.assertNotIn("dirty_before", serialized)
        self.assertNotIn("dirty_after", serialized)
        self.assertNotIn("undo", serialized)
        self.assertNotIn("trust", serialized)

    def test_stale_manifest_is_rechecked(self) -> None:
        stale = self.fixtures.verification.__class__(
            root=self.fixtures.verification.root,
            corpus_version=self.fixtures.verification.corpus_version,
            manifest_sha256="0" * 64,
            entries=self.fixtures.verification.entries,
            artifacts=self.fixtures.verification.artifacts,
            scoring_item_ids=self.fixtures.verification.scoring_item_ids,
        )
        evidence = revision.evidence_from_owner_inputs(
            stale,
            self.fixtures.registry,
            self.fixtures.machine,
            self.fixtures.owner_cases,
            approved_registry_sha256=self.fixtures.registry_anchor,
            approved_machine_receipt_sha256=self.fixtures.machine_anchor,
            approved_runner_executable_sha256=self.fixtures.runner_anchor,
            native_acceptance=self.fixtures.native,
            approved_native_evidence_sha256=self.fixtures.native_anchor,
            approved_native_executable_sha256=self.fixtures.native_executable_anchor,
        )
        self.assertEqual(evidence["corpus"]["manifest_sha256"], self.fixtures.verification.manifest_sha256)

    def test_scaffold_is_fail_closed_and_content_free(self) -> None:
        evidence = revision.scaffold_evidence(self.fixtures.verification, created_at="2026-09-18T00:00:00Z")
        self.assertEqual(evidence["evaluation"]["status"], "not_evaluated")
        self.assertFalse(evidence["evaluation"]["all_cases_satisfied"])
        self.assertIsNone(evidence["registry"])
        self.assertNotIn("source_text", json.dumps(evidence))

    def test_scaffold_can_bind_machine_facts_without_owner_values(self) -> None:
        evidence = revision.scaffold_evidence(
            self.fixtures.verification,
            registry=self.fixtures.registry,
            machine_receipt=self.fixtures.machine,
            approved_registry_sha256=self.fixtures.registry_anchor,
            approved_machine_receipt_sha256=self.fixtures.machine_anchor,
            approved_runner_executable_sha256=self.fixtures.runner_anchor,
        )
        self.assertEqual(evidence["evaluation"]["status"], "not_evaluated")
        result = next(item for item in evidence["results"] if item["artifact_id"] == "TP-01")
        self.assertIsNone(result["intent_judgment"])
        self.assertEqual(result["changes"], [])
        self.assertEqual(result["editable_source_sha256"], self.fixtures.machine.by_artifact()["TP-01"]["editable_source_sha256"])
        self.assertEqual(result["source_binding_sha256"], self.fixtures.machine.by_artifact()["TP-01"]["source_binding_sha256"])
        self.assertEqual(result["review_context_sha256"], self.fixtures.machine.by_artifact()["TP-01"]["review_context_sha256"])

    def test_write_evidence_is_create_new_and_hardlink_safe(self) -> None:
        evidence = revision.scaffold_evidence(self.fixtures.verification)
        destination = Path(self.temp.name) / "out.json"
        revision.write_evidence(destination, evidence, root=self.fixtures.verification.root)
        with self.assertRaisesRegex(revision.RevisionEvaluationError, "create-new"):
            revision.write_evidence(destination, evidence, root=self.fixtures.verification.root)
        target = Path(self.temp.name) / "target.json"
        target.write_bytes(b"keep")
        hardlink = Path(self.temp.name) / "hardlink.json"
        try:
            os.link(target, hardlink)
        except OSError as error:
            self.skipTest(f"hardlinks unavailable: {error}")
        with self.assertRaisesRegex(revision.RevisionEvaluationError, "create-new"):
            revision.write_evidence(hardlink, evidence, root=self.fixtures.verification.root)
        self.assertEqual(target.read_bytes(), b"keep")

    def test_owner_input_loader_requires_all_artifacts(self) -> None:
        directory = Path(self.temp.name) / "owners"
        directory.mkdir()
        (directory / "TP-01.json").write_text("{}", encoding="utf-8")
        with self.assertRaisesRegex(revision.OwnerInputRequired, "missing"):
            revision.load_owner_inputs(directory, self.fixtures.verification)

    def test_cli_record_without_owner_inputs_writes_scaffold_and_returns_two(self) -> None:
        destination = Path(self.temp.name) / "cli-scaffold.json"
        stdout = io.StringIO()
        stderr = io.StringIO()
        with redirect_stdout(stdout), redirect_stderr(stderr):
            result = revision.main(["record", "--evidence", str(destination)])
        self.assertEqual(result, 2)
        value = json.loads(destination.read_text(encoding="utf-8"))
        self.assertEqual(value["evaluation"]["status"], "not_evaluated")
        self.assertFalse(value["evaluation"]["all_cases_satisfied"])
        self.assertIn("owner-local revision inputs", stderr.getvalue())

    def test_cli_verify_manifest_returns_zero(self) -> None:
        stdout = io.StringIO()
        with redirect_stdout(stdout):
            result = revision.main(["verify-manifest"])
        self.assertEqual(result, 0)
        self.assertEqual(json.loads(stdout.getvalue())["corpus_version"], "goal-01-v1")

    def test_machine_intent_change_ids_are_required_and_cannot_hide_registry_violation(self) -> None:
        missing = json.loads(json.dumps(self.fixtures.machine_value))
        del missing["cases"]["TP-01"]["changes"][0]["intent_change_ids"]
        missing_path = Path(self.temp.name) / "missing-intent.json"
        missing_path.write_text(json.dumps(missing, sort_keys=True) + "\n", encoding="utf-8")
        with self.assertRaisesRegex(revision.RevisionEvaluationError, "change has an invalid schema"):
            revision.load_machine_receipt(
                missing_path,
                self.fixtures.verification,
                approved_machine_receipt_sha256=hashlib.sha256(missing_path.read_bytes()).hexdigest(),
                approved_runner_executable_sha256=self.fixtures.runner_anchor,
            )

        hidden = json.loads(json.dumps(self.fixtures.machine_value))
        hidden["cases"]["TP-01"]["changes"][0]["change_id"] = "TP-01-IV-01"
        hidden["cases"]["TP-01"]["changes"][1]["change_id"] = "TP-01-ZZ-02"
        hidden["cases"]["TP-01"]["changes"][0]["intent_change_ids"] = []
        hidden_path = Path(self.temp.name) / "hidden-intent.json"
        hidden_path.write_text(json.dumps(hidden, sort_keys=True) + "\n", encoding="utf-8")
        receipt = revision.load_machine_receipt(
            hidden_path,
            self.fixtures.verification,
            approved_machine_receipt_sha256=hashlib.sha256(hidden_path.read_bytes()).hexdigest(),
            approved_runner_executable_sha256=self.fixtures.runner_anchor,
        )
        with self.assertRaisesRegex(revision.RevisionEvaluationError, "exactly match"):
            revision._ensure_machine_receipt(
                receipt,
                self.fixtures.verification,
                self.fixtures.registry,
                approved_machine_receipt_sha256=hashlib.sha256(hidden_path.read_bytes()).hexdigest(),
                approved_runner_executable_sha256=self.fixtures.runner_anchor,
            )

    def test_machine_intent_union_rejects_omitted_and_partial_annotations(self) -> None:
        for intent_ids in ([], ["TP-01-IV-02"]):
            payload = json.loads(json.dumps(self.fixtures.machine_value))
            payload["cases"]["TP-01"]["changes"][0]["intent_change_ids"] = intent_ids
            path = Path(self.temp.name) / f"intent-{len(intent_ids)}.json"
            path.write_text(json.dumps(payload, sort_keys=True) + "\n", encoding="utf-8")
            receipt = revision.load_machine_receipt(
                path,
                self.fixtures.verification,
                approved_machine_receipt_sha256=hashlib.sha256(path.read_bytes()).hexdigest(),
                approved_runner_executable_sha256=self.fixtures.runner_anchor,
            )
            with self.subTest(intent_ids=intent_ids), self.assertRaisesRegex(
                revision.RevisionEvaluationError, "exactly match"
            ):
                revision._ensure_machine_receipt(
                    receipt,
                    self.fixtures.verification,
                    self.fixtures.registry,
                    approved_machine_receipt_sha256=hashlib.sha256(path.read_bytes()).hexdigest(),
                    approved_runner_executable_sha256=self.fixtures.runner_anchor,
                )

    def test_machine_source_binding_rejects_forged_single_file_and_support_digest(self) -> None:
        forged = json.loads(json.dumps(self.fixtures.machine_value))
        forged["cases"]["TP-01"]["editable_source_sha256"] = digest("forged-source")
        forged_path = Path(self.temp.name) / "forged-source.json"
        forged_path.write_text(json.dumps(forged, sort_keys=True) + "\n", encoding="utf-8")
        with self.assertRaisesRegex(revision.RevisionEvaluationError, "single-file corpus"):
            revision.load_machine_receipt(
                forged_path,
                self.fixtures.verification,
                approved_machine_receipt_sha256=hashlib.sha256(forged_path.read_bytes()).hexdigest(),
                approved_runner_executable_sha256=self.fixtures.runner_anchor,
            )

        altered_support = json.loads(json.dumps(self.fixtures.machine_value))
        altered_support["cases"]["AS-01"]["request_artifact_sha256"] = digest("altered-support")
        altered_path = Path(self.temp.name) / "altered-support.json"
        altered_path.write_text(json.dumps(altered_support, sort_keys=True) + "\n", encoding="utf-8")
        with self.assertRaisesRegex(revision.RevisionEvaluationError, "request artifact identity"):
            revision.load_machine_receipt(
                altered_path,
                self.fixtures.verification,
                approved_machine_receipt_sha256=hashlib.sha256(altered_path.read_bytes()).hexdigest(),
                approved_runner_executable_sha256=self.fixtures.runner_anchor,
            )

        missing_digest = json.loads(json.dumps(self.fixtures.machine_value))
        del missing_digest["cases"]["AS-01"]["request_artifact_sha256"]
        missing_path = Path(self.temp.name) / "missing-request-digest.json"
        missing_path.write_text(json.dumps(missing_digest, sort_keys=True) + "\n", encoding="utf-8")
        with self.assertRaisesRegex(revision.RevisionEvaluationError, "invalid schema"):
            revision.load_machine_receipt(
                missing_path,
                self.fixtures.verification,
                approved_machine_receipt_sha256=hashlib.sha256(missing_path.read_bytes()).hexdigest(),
                approved_runner_executable_sha256=self.fixtures.runner_anchor,
            )

    def test_machine_question_coverage_requires_known_change_ids(self) -> None:
        payload = json.loads(json.dumps(self.fixtures.machine_value))
        payload["cases"]["TP-01"]["question_coverage"][0]["status"]["change_ids"] = [99]
        path = Path(self.temp.name) / "unknown-question-change.json"
        path.write_text(json.dumps(payload, sort_keys=True) + "\n", encoding="utf-8")
        with self.assertRaisesRegex(revision.RevisionEvaluationError, "unknown ChangeId"):
            revision.load_machine_receipt(
                path,
                self.fixtures.verification,
                approved_machine_receipt_sha256=hashlib.sha256(path.read_bytes()).hexdigest(),
                approved_runner_executable_sha256=self.fixtures.runner_anchor,
            )

    def test_machine_change_ids_use_numeric_order_past_two_digits(self) -> None:
        payload = json.loads(json.dumps(self.fixtures.machine_value))
        case = payload["cases"]["TP-01"]
        case["changes"] = [
            {
                "change_id": f"TP-01-CH-{ordinal:02}",
                "intent_change_ids": ["TP-01-IV-01"] if ordinal == 1 else [],
                "hunks": [
                    self.fixtures._hunk(
                        "TP-01",
                        f"{ordinal:03}",
                        ordinal - 1,
                        ordinal - 1,
                    )
                ],
            }
            for ordinal in range(1, 101)
        ]
        case["question_coverage"][0]["status"]["change_ids"] = [99]
        case["approved_output"]["decision_count"] = 100
        path = Path(self.temp.name) / "hundred-changes.json"
        path.write_text(json.dumps(payload, sort_keys=True) + "\n", encoding="utf-8")

        receipt = revision.load_machine_receipt(
            path,
            self.fixtures.verification,
            approved_machine_receipt_sha256=hashlib.sha256(path.read_bytes()).hexdigest(),
            approved_runner_executable_sha256=self.fixtures.runner_anchor,
        )

        self.assertEqual(len(receipt.by_artifact()["TP-01"]["changes"]), 100)

    def test_clearer_confirmation_cannot_pass_reject_all(self) -> None:
        owners = json.loads(json.dumps(self.fixtures.owner_cases))
        owners["TP-01"]["change_decisions"] = [
            {"change_id": "TP-01-CH-01", "decision": "reject"},
            {"change_id": "TP-01-CH-02", "decision": "reject"},
        ]
        evidence = revision.evidence_from_owner_inputs(
            self.fixtures.verification,
            self.fixtures.registry,
            self.fixtures.machine,
            owners,
            approved_registry_sha256=self.fixtures.registry_anchor,
            approved_machine_receipt_sha256=self.fixtures.machine_anchor,
            approved_runner_executable_sha256=self.fixtures.runner_anchor,
            native_acceptance=self.fixtures.native,
            approved_native_evidence_sha256=self.fixtures.native_anchor,
            approved_native_executable_sha256=self.fixtures.native_executable_anchor,
        )
        self.assertFalse(evidence["evaluation"]["all_cases_satisfied"])

    def test_native_schema_version_and_unhashable_case_id_fail_closed(self) -> None:
        wrong_type = {**self.fixtures.native_value, "schema_version": True}
        path = Path(self.temp.name) / "native-bool-version.json"
        path.write_text(json.dumps(wrong_type, sort_keys=True) + "\n", encoding="utf-8")
        with self.assertRaisesRegex(revision.RevisionEvaluationError, "schema is invalid"):
            revision.load_native_acceptance(
                path,
                approved_native_evidence_sha256=hashlib.sha256(path.read_bytes()).hexdigest(),
                approved_native_executable_sha256=self.fixtures.native_executable_anchor,
            )

        invalid_semantics = json.loads(json.dumps(self.fixtures.native_value))
        invalid_semantics["cases"][0]["observations"]["foreground_verified"] = False
        path = Path(self.temp.name) / "native-invalid-semantics.json"
        path.write_text(json.dumps(invalid_semantics, sort_keys=True) + "\n", encoding="utf-8")
        with self.assertRaisesRegex(revision.RevisionEvaluationError, "full harness validation"):
            revision.load_native_acceptance(
                path,
                approved_native_evidence_sha256=hashlib.sha256(path.read_bytes()).hexdigest(),
                approved_native_executable_sha256=self.fixtures.native_executable_anchor,
            )

        unhashable = json.loads(json.dumps(self.fixtures.native_value))
        unhashable["cases"][0]["id"] = ["not-an-id"]
        path = Path(self.temp.name) / "native-unhashable-id.json"
        path.write_text(json.dumps(unhashable, sort_keys=True) + "\n", encoding="utf-8")
        with self.assertRaisesRegex(revision.RevisionEvaluationError, "invalid case identifier"):
            revision.load_native_acceptance(
                path,
                approved_native_evidence_sha256=hashlib.sha256(path.read_bytes()).hexdigest(),
                approved_native_executable_sha256=self.fixtures.native_executable_anchor,
            )

    def test_native_acceptance_runs_full_validator_without_legacy_request_key(self) -> None:
        payload = json.loads(json.dumps(self.fixtures.native_value))
        self.assertNotIn(
            "provider_requests_after_consent",
            payload["cases"][0]["observations"],
        )
        payload["cases"][0]["observations"]["provider_request_count"] = 1
        path = Path(self.temp.name) / "native-invalid-provider-count.json"
        path.write_text(json.dumps(payload, sort_keys=True) + "\n", encoding="utf-8")
        with self.assertRaisesRegex(revision.RevisionEvaluationError, "full harness validation"):
            revision.load_native_acceptance(
                path,
                approved_native_evidence_sha256=hashlib.sha256(path.read_bytes()).hexdigest(),
                approved_native_executable_sha256=self.fixtures.native_executable_anchor,
            )

    def test_json_integer_bound_fails_closed(self) -> None:
        path = Path(self.temp.name) / "oversized-integer.json"
        path.write_text('{"value": ' + ("7" * 5000) + "}\n", encoding="utf-8")
        with self.assertRaises(revision.RevisionEvaluationError):
            revision._json_load(path)

    def test_forbidden_content_depth_fails_closed(self) -> None:
        value: object = {}
        for _ in range(998):
            value = [value]
        with self.assertRaises(revision.RevisionEvaluationError):
            revision._reject_forbidden_content(value, field="deep input")

    def test_external_json_duplicate_keys_are_rejected(self) -> None:
        path = Path(self.temp.name) / "duplicate-registry.json"
        path.write_text(
            '{"schema":"markturbo-goal-07-eligibility-v2","schema":"duplicate"}\n',
            encoding="utf-8",
        )
        with self.assertRaisesRegex(revision.RevisionEvaluationError, "duplicate JSON keys"):
            revision.load_eligibility_registry(
                path,
                self.fixtures.verification,
                approved_registry_sha256=self.fixtures.registry_anchor,
            )

    def test_external_inputs_reject_parent_symlink_and_corpus_output(self) -> None:
        link_root = Path(self.temp.name) / "linked"
        try:
            os.symlink(self.temp.name, link_root, target_is_directory=True)
        except OSError as error:
            self.skipTest(f"symlinks unavailable: {error}")
        with self.assertRaisesRegex(revision.RevisionEvaluationError, "symlink"):
            revision.load_eligibility_registry(
                link_root / self.fixtures.registry_path.name,
                self.fixtures.verification,
                approved_registry_sha256=self.fixtures.registry_anchor,
            )
        evidence = revision.scaffold_evidence(self.fixtures.verification)
        with self.assertRaisesRegex(revision.RevisionEvaluationError, "immutable corpus"):
            revision.write_evidence(
                ROOT / "evaluation" / "goal-01" / "forbidden-evidence.json",
                evidence,
                root=ROOT,
            )


if __name__ == "__main__":
    unittest.main()
