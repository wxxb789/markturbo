"""Offline Goal 06 evaluation metadata and evidence tests."""

from __future__ import annotations

import hashlib
import json
import subprocess
import sys
import tempfile
import unittest
from collections.abc import Callable
from pathlib import Path

from scripts.markturbo_tools import evaluation


ROOT = Path(__file__).resolve().parents[2]


def owner_input(
    artifact_id: str,
    *,
    useful: str = "useful",
    surfaced_item_ids: list[str] | None = None,
) -> dict[str, object]:
    return {
        "artifact_id": artifact_id,
        "decoded_completely": True,
        "surfaced_item_ids": surfaced_item_ids or [],
        "unsupported_claim_ids": [],
        "unsupported_claim_count": 0,
        "false_source_anchor_count": 0,
        "boilerplate_question_count": 0,
        "question_count": 0,
        "materially_misleading": False,
        "usefulness": useful,
        "model_reported_id": "gpt-5.6-terra",
    }


def complete_owner_inputs(
    verification: evaluation.ManifestVerification,
) -> dict[str, dict[str, object]]:
    return {
        artifact.artifact_id: owner_input(
            artifact.artifact_id,
            surfaced_item_ids=sorted(verification.item_ids_for(artifact.artifact_id)),
        )
        for artifact in verification.artifacts
    }


def copy_corpus(destination_root: Path) -> None:
    source = ROOT / "evaluation" / "goal-01"
    corpus = destination_root / "evaluation" / "goal-01"
    for path in source.rglob("*"):
        destination = corpus / path.relative_to(source)
        if path.is_dir():
            destination.mkdir(parents=True, exist_ok=True)
        elif path.is_file():
            destination.parent.mkdir(parents=True, exist_ok=True)
            destination.write_bytes(path.read_bytes())


class ManifestTests(unittest.TestCase):
    def test_approved_manifest_verifies_to_twelve_artifacts(self) -> None:
        verified = evaluation.verify_manifest(ROOT)

        self.assertEqual(verified.corpus_version, "goal-01-v1")
        self.assertEqual(len(verified.artifacts), 12)
        self.assertEqual(len(verified.entries), 45)
        self.assertEqual(len(verified.manifest_sha256), 64)
        self.assertEqual(verified.manifest_sha256, evaluation.APPROVED_MANIFEST_SHA256)
        self.assertEqual(
            [artifact.artifact_id for artifact in verified.artifacts],
            [
                "TP-01",
                "TP-02",
                "TP-03",
                "TP-04",
                "SP-01",
                "SP-02",
                "SP-03",
                "SP-04",
                "AI-01",
                "AI-02",
                "AS-01",
                "AS-02",
            ],
        )

    def test_manifest_hash_mismatch_is_rejected(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            corpus = root / "evaluation" / "goal-01"
            copy_corpus(root)
            target = corpus / "CORPUS.md"
            target.write_bytes(target.read_bytes() + b"\n")

            with self.assertRaisesRegex(evaluation.EvaluationError, "hash mismatch"):
                evaluation.verify_manifest(root)

    def test_recomputed_tampered_manifest_is_rejected_by_approved_trust_anchor(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            copy_corpus(root)
            corpus_file = root / "evaluation" / "goal-01" / "CORPUS.md"
            original_bytes = corpus_file.read_bytes()
            corpus_file.write_bytes(original_bytes + b"\n")
            manifest_path = root / "evaluation" / "goal-01" / "MANIFEST.sha256"
            manifest_path.write_bytes(
                manifest_path.read_bytes().replace(
                    hashlib.sha256(original_bytes).hexdigest().encode("ascii"),
                    hashlib.sha256(corpus_file.read_bytes()).hexdigest().encode("ascii"),
                    1,
                )
            )

            with self.assertRaisesRegex(evaluation.EvaluationError, "approved immutable trust anchor"):
                evaluation.verify_manifest(root)


class EvidenceTests(unittest.TestCase):
    def setUp(self) -> None:
        self.verification = evaluation.verify_manifest(ROOT)

    def test_scaffold_is_fail_closed_and_content_free(self) -> None:
        evidence = evaluation.scaffold_evidence(self.verification, created_at="2026-09-07T00:00:00Z")

        self.assertEqual(evidence["evaluation"]["status"], "not_evaluated")
        self.assertFalse(evidence["evaluation"]["eligible_for_threshold"])
        self.assertEqual(len(evidence["results"]), 12)
        self.assertEqual(evidence["configuration"]["max_output_tokens"], 8192)
        serialized = json.dumps(evidence, sort_keys=True)
        for forbidden in ('"response":', '"source_text":', '"request_body":', '"api_key":', '"endpoint_url":'):
            self.assertNotIn(forbidden, serialized)
        for artifact in evidence["artifact_metadata"]:
            for item in artifact["files"]:
                self.assertFalse(Path(item["path"]).is_absolute())

    def test_complete_owner_metadata_records_only_safe_fields(self) -> None:
        inputs = complete_owner_inputs(self.verification)
        evidence = evaluation.evidence_from_owner_inputs(
            self.verification,
            inputs,
            created_at="2026-09-07T00:00:00Z",
        )

        self.assertEqual(evidence["evaluation"]["status"], "recorded")
        self.assertTrue(evidence["evaluation"]["eligible_for_threshold"])
        self.assertEqual(evidence["evaluation"]["decoded_complete_count"], 12)
        self.assertEqual(evidence["evaluation"]["useful_count"], 12)
        self.assertEqual(evidence["evaluation"]["surfaced_item_count"], 60)
        self.assertEqual(evidence["evaluation"]["false_source_anchor_count"], 0)
        self.assertEqual(evidence["evaluation"]["max_question_count"], 0)
        self.assertEqual(evidence["configuration"]["max_output_tokens"], 8192)
        self.assertTrue(all("model_reported_id" in result for result in evidence["results"]))

    def test_evidence_rejects_a_non_fixed_output_token_cap(self) -> None:
        evidence = evaluation.scaffold_evidence(self.verification)
        evidence["configuration"]["max_output_tokens"] = 4096

        with self.assertRaisesRegex(evaluation.EvaluationError, "reference configuration"):
            evaluation.validate_evidence(evidence, verification=self.verification)

    def test_owner_input_with_response_content_is_rejected(self) -> None:
        value = owner_input("TP-01")
        value["response"] = "private model output"
        inputs = {
            artifact.artifact_id: (
                value if artifact.artifact_id == "TP-01" else owner_input(artifact.artifact_id)
            )
            for artifact in self.verification.artifacts
        }

        with self.assertRaisesRegex(evaluation.EvaluationError, "metadata-only"):
            evaluation.evidence_from_owner_inputs(self.verification, inputs)

    def test_owner_input_requires_a_model_reported_id(self) -> None:
        inputs = complete_owner_inputs(self.verification)
        inputs["TP-01"]["model_reported_id"] = None

        with self.assertRaisesRegex(evaluation.EvaluationError, "requires model_reported_id"):
            evaluation.evidence_from_owner_inputs(self.verification, inputs)

    def test_owner_input_rejects_model_urls_and_absolute_paths(self) -> None:
        for model_reported_id in (
            "https://models.example.test/gpt-5.6-terra",
            "file:///C:/private/model-id",
            "/private/model-id",
            "C:/private/model-id",
            "sk-abcdefghijklmnopqrstuvwxyz1234567890",
            "gsk_abcdefghijklmnopqrstuvwxyz1234567890",
            "xai-abcdefghijklmnopqrstuvwxyz1234567890",
            "hf_abcdefghijklmnopqrstuvwxyz1234567890",
            "ghp_abcdefghijklmnopqrstuvwxyz1234567890",
            "github_pat_abcdefghijklmnopqrstuvwxyz1234567890",
            "glpat-abcdefghijklmnopqrstuvwxyz1234567890",
            "AIzaabcdefghijklmnopqrstuvwxyz1234567890",
            "AKIAABCDEFGHIJKLMNOPQRSTUVWXYZ1234",
        ):
            with self.subTest(model_reported_id=model_reported_id):
                inputs = complete_owner_inputs(self.verification)
                inputs["TP-01"]["model_reported_id"] = model_reported_id

                with self.assertRaisesRegex(evaluation.EvaluationError, "model_reported_id"):
                    evaluation.evidence_from_owner_inputs(self.verification, inputs)

    def test_owner_input_accepts_the_reference_model_identifier(self) -> None:
        inputs = complete_owner_inputs(self.verification)

        evidence = evaluation.evidence_from_owner_inputs(self.verification, inputs)

        self.assertEqual(evidence["results"][0]["model_reported_id"], evaluation.REFERENCE_MODEL_REPORTED_ID)

    def test_owner_input_preserves_a_versioned_provider_model_identifier(self) -> None:
        inputs = complete_owner_inputs(self.verification)
        provider_model_id = "gpt-5.6-terra-2026-08-19-deployment-42"
        inputs["TP-01"]["model_reported_id"] = provider_model_id

        evidence = evaluation.evidence_from_owner_inputs(self.verification, inputs)

        self.assertEqual(evidence["results"][0]["model_reported_id"], provider_model_id)

    def test_scored_ids_must_be_fixed_for_the_same_artifact(self) -> None:
        inputs = complete_owner_inputs(self.verification)
        inputs["TP-01"]["surfaced_item_ids"] = ["TP-01-HI-99"]

        with self.assertRaisesRegex(evaluation.EvaluationError, "not fixed for this artifact"):
            evaluation.evidence_from_owner_inputs(self.verification, inputs)

    def test_surfaced_item_cannot_be_unsupported(self) -> None:
        inputs = complete_owner_inputs(self.verification)
        inputs["TP-01"]["unsupported_claim_ids"] = ["TP-01-HI-01"]
        inputs["TP-01"]["unsupported_claim_count"] = 1

        with self.assertRaisesRegex(evaluation.EvaluationError, "cannot also be unsupported"):
            evaluation.evidence_from_owner_inputs(self.verification, inputs)

        inputs = complete_owner_inputs(self.verification)
        inputs["TP-01"]["surfaced_item_ids"] = ["TP-02-HI-01"]

        with self.assertRaisesRegex(evaluation.EvaluationError, "not fixed for this artifact"):
            evaluation.evidence_from_owner_inputs(self.verification, inputs)

    def test_fixed_contract_thresholds_determine_eligibility(self) -> None:
        cases: list[tuple[str, Callable[[dict[str, dict[str, object]]], object]]] = [
            ("incomplete decode", lambda inputs: inputs["TP-01"].update(decoded_completely=False)),
            (
                "fewer than ten useful",
                lambda inputs: [
                    inputs[artifact_id].update(usefulness="not_useful")
                    for artifact_id in ("TP-01", "TP-02", "TP-03")
                ],
            ),
            (
                "fewer than forty-five hits",
                lambda inputs: [
                    inputs[artifact_id].update(surfaced_item_ids=[])
                    for artifact_id in ("SP-01", "SP-02", "SP-03", "SP-04")
                ],
            ),
            (
                "false source anchor",
                lambda inputs: inputs["TP-01"].update(false_source_anchor_count=1),
            ),
            (
                "more than one misleading artifact",
                lambda inputs: [
                    inputs[artifact_id].update(materially_misleading=True)
                    for artifact_id in ("TP-01", "TP-02")
                ],
            ),
        ]
        for name, mutate in cases:
            with self.subTest(name=name):
                inputs = complete_owner_inputs(self.verification)
                mutate(inputs)
                evidence = evaluation.evidence_from_owner_inputs(self.verification, inputs)
                self.assertEqual(evidence["evaluation"]["status"], "recorded")
                self.assertFalse(evidence["evaluation"]["eligible_for_threshold"])

    def test_question_limit_rejects_an_owner_record_before_eligibility(self) -> None:
        inputs = complete_owner_inputs(self.verification)
        inputs["TP-01"]["question_count"] = 6

        with self.assertRaisesRegex(evaluation.EvaluationError, "0 through 5"):
            evaluation.evidence_from_owner_inputs(self.verification, inputs)

    def test_fresh_manifest_is_required_before_recording_and_writing(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            copy_corpus(root)
            verification = evaluation.verify_manifest(root)
            inputs = complete_owner_inputs(verification)
            corpus_file = root / "evaluation" / "goal-01" / "CORPUS.md"
            corpus_file.write_bytes(corpus_file.read_bytes() + b"\n")

            with self.assertRaisesRegex(evaluation.EvaluationError, "hash mismatch"):
                evaluation.evidence_from_owner_inputs(verification, inputs)

        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            copy_corpus(root)
            verification = evaluation.verify_manifest(root)
            evidence = evaluation.evidence_from_owner_inputs(
                verification,
                complete_owner_inputs(verification),
            )
            corpus_file = root / "evaluation" / "goal-01" / "CORPUS.md"
            corpus_file.write_bytes(corpus_file.read_bytes() + b"\n")

            with self.assertRaisesRegex(evaluation.EvaluationError, "hash mismatch"):
                evaluation.write_evidence(root / "goal-06.json", evidence, root=root)

    def test_false_source_anchors_can_be_recorded_without_a_scoring_registry_id(self) -> None:
        inputs = complete_owner_inputs(self.verification)
        inputs["TP-01"]["false_source_anchor_count"] = 1

        evidence = evaluation.evidence_from_owner_inputs(self.verification, inputs)

        self.assertEqual(evidence["evaluation"]["false_source_anchor_count"], 1)
        self.assertFalse(evidence["evaluation"]["eligible_for_threshold"])

    def test_owner_counts_must_cover_scoring_observations(self) -> None:
        inputs = complete_owner_inputs(self.verification)
        inputs["TP-01"]["unsupported_claim_ids"] = ["TP-01-HI-01"]

        with self.assertRaisesRegex(evaluation.EvaluationError, "cannot be below"):
            evaluation.evidence_from_owner_inputs(self.verification, inputs)

    def test_evidence_cannot_overwrite_the_immutable_corpus(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            copy_corpus(root)
            verification = evaluation.verify_manifest(root)
            evidence = evaluation.evidence_from_owner_inputs(
                verification,
                complete_owner_inputs(verification),
            )

            with self.assertRaisesRegex(evaluation.EvaluationError, "immutable evaluation corpus"):
                evaluation.write_evidence(
                    root / "evaluation" / "goal-01" / "CORPUS.md",
                    evidence,
                    root=root,
                )

    def test_invalid_rfc3339_timestamp_is_rejected(self) -> None:
        with self.assertRaisesRegex(evaluation.EvaluationError, "RFC 3339"):
            evaluation.scaffold_evidence(
                self.verification,
                created_at="2026-99-99T99:99:99Z",
            )

    def test_cli_record_without_owner_inputs_writes_blocked_scaffold(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            evidence_path = Path(directory) / "goal-06.json"
            result = subprocess.run(
                [
                    sys.executable,
                    "scripts/mt.py",
                    "evaluation",
                    "record",
                    "--evidence",
                    str(evidence_path),
                ],
                cwd=ROOT,
                check=False,
                capture_output=True,
                text=True,
            )

            self.assertEqual(result.returncode, 2, result.stderr)
            evidence = json.loads(evidence_path.read_text(encoding="utf-8"))
            self.assertEqual(evidence["evaluation"]["status"], "not_evaluated")
            self.assertIn("owner-local response inputs are required", result.stderr)

    def test_cli_record_with_complete_valid_owner_inputs_writes_eligible_evidence(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            owner_input_dir = Path(directory) / "owner-inputs"
            owner_input_dir.mkdir()
            for artifact_id, value in complete_owner_inputs(self.verification).items():
                (owner_input_dir / f"{artifact_id}.json").write_text(
                    json.dumps(value),
                    encoding="utf-8",
                )
            evidence_path = Path(directory) / "goal-06.json"
            result = subprocess.run(
                [
                    sys.executable,
                    "scripts/mt.py",
                    "evaluation",
                    "record",
                    "--owner-input-dir",
                    str(owner_input_dir),
                    "--evidence",
                    str(evidence_path),
                ],
                cwd=ROOT,
                check=False,
                capture_output=True,
                text=True,
            )

            self.assertEqual(result.returncode, 0, result.stderr)
            evidence = json.loads(evidence_path.read_text(encoding="utf-8"))
            self.assertEqual(evidence["evaluation"]["status"], "recorded")
            self.assertTrue(evidence["evaluation"]["eligible_for_threshold"])

    def test_cli_help_exposes_offline_commands(self) -> None:
        result = subprocess.run(
            [sys.executable, "scripts/mt.py", "evaluation", "--help"],
            cwd=ROOT,
            check=False,
            capture_output=True,
            text=True,
        )

        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn("verify-manifest", result.stdout)
        self.assertIn("scaffold", result.stdout)
        self.assertIn("record", result.stdout)
        self.assertIn("never contacts", result.stdout)


if __name__ == "__main__":
    unittest.main()
