from __future__ import annotations

import contextlib
import io
import json
import sys
import types
import unittest
from pathlib import Path
from unittest import mock

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT))

from scripts.markturbo_tools import evaluation, revision_evaluation as revision
from scripts.markturbo_tools.native import goal07 as native_goal07
from scripts.tests import test_evaluation, test_revision_evaluation


def run_case(case: unittest.TestCase) -> dict[str, object]:
    stream = io.StringIO()
    result = unittest.TextTestRunner(stream=stream, verbosity=2).run(unittest.TestSuite([case]))
    return {
        "tests_run": result.testsRun,
        "failures": len(result.failures),
        "errors": len(result.errors),
        "skipped": len(result.skipped),
        "successful": result.wasSuccessful(),
        "output": stream.getvalue(),
    }


def mutant_evaluation(label: str, old: str, new: str) -> types.ModuleType:
    source_path = ROOT / "scripts" / "markturbo_tools" / "evaluation.py"
    source = source_path.read_text(encoding="utf-8")
    if source.count(old) != 1:
        raise AssertionError(f"{label}: replacement count was {source.count(old)}")
    name = f"scripts.markturbo_tools._evaluation_mutant_{label}"
    module = types.ModuleType(name)
    module.__file__ = str(source_path)
    module.__package__ = "scripts.markturbo_tools"
    sys.modules[name] = module
    exec(compile(source.replace(old, new), str(source_path), "exec"), module.__dict__)
    return module


def main() -> int:
    matrix: dict[str, dict[str, object]] = {}

    mutations = [
        (
            "approved_manifest_anchor_removed",
            "if manifest_sha256 != APPROVED_MANIFEST_SHA256:",
            "if False and manifest_sha256 != APPROVED_MANIFEST_SHA256:",
            test_evaluation.ManifestTests("test_recomputed_tampered_manifest_is_rejected_by_approved_trust_anchor"),
        ),
        (
            "per_file_hash_comparison_removed",
            "if digest != parsed.sha256:",
            "if False and digest != parsed.sha256:",
            test_evaluation.ManifestTests("test_manifest_hash_mismatch_is_rejected"),
        ),
    ]
    for label, old, new, case in mutations:
        mutant = mutant_evaluation(label, old, new)
        with mock.patch.object(test_evaluation, "evaluation", mutant):
            outcome = run_case(case)
        outcome["mutation_killed"] = not outcome["successful"]
        matrix[label] = outcome

    exact_mutant = mutant_evaluation(
        "exact_file_set_removed",
        "if actual_paths != expected_paths:",
        "if False and actual_paths != expected_paths:",
    )
    with (
        mock.patch.object(test_revision_evaluation, "evaluation", exact_mutant),
        mock.patch.object(revision, "evaluation", exact_mutant),
    ):
        outcome = run_case(
            test_revision_evaluation.RevisionEvaluationV2Tests(
                "test_scaffold_rechecks_manifest_exact_file_set"
            )
        )
    outcome["mutation_killed"] = not outcome["successful"]
    matrix["exact_file_set_comparison_removed"] = outcome

    def stale_verification(value):
        return evaluation.verify_manifest(revision.REPO) if value is None else value

    with mock.patch.object(revision, "_fresh_verification", stale_verification):
        outcome = run_case(
            test_revision_evaluation.RevisionEvaluationV2Tests(
                "test_scaffold_rechecks_manifest_exact_file_set"
            )
        )
    outcome["mutation_killed"] = not outcome["successful"]
    matrix["revision_scaffold_fresh_verifier_call_removed"] = outcome

    outcome = run_case(
        test_revision_evaluation.RevisionEvaluationV2Tests(
            "test_native_acceptance_requires_the_exact_goal07_case_set"
        )
    )
    outcome["receipt_variants"] = ["missing", "duplicate", "invented"]
    matrix["native_receipt_case_matrix_baseline"] = outcome

    outcome = run_case(
        test_revision_evaluation.RevisionEvaluationV2Tests(
            "test_native_schema_version_and_unhashable_case_id_fail_closed"
        )
    )
    outcome["receipt_variants"] = ["wrong_schema_version", "altered_required_observation", "unhashable_id"]
    matrix["native_receipt_schema_observation_baseline"] = outcome

    reduced = frozenset(sorted(revision.NATIVE_REQUIRED_CASE_IDS)[:-1])
    with mock.patch.object(revision, "NATIVE_REQUIRED_CASE_IDS", reduced):
        outcome = run_case(
            test_revision_evaluation.RevisionEvaluationV2Tests(
                "test_native_acceptance_requires_the_exact_goal07_case_set"
            )
        )
    outcome["mutation_killed"] = not outcome["successful"]
    outcome["removed_case"] = sorted(set(native_goal07.REQUIRED_CASE_IDS) - set(reduced))
    matrix["accepted_production_case_set_reduced"] = outcome

    with mock.patch.object(native_goal07, "validate_evidence", lambda value: None):
        outcome = run_case(
            test_revision_evaluation.RevisionEvaluationV2Tests(
                "test_native_acceptance_runs_full_validator_without_legacy_request_key"
            )
        )
    outcome["mutation_killed"] = not outcome["successful"]
    matrix["full_native_validator_bypassed"] = outcome

    output = ROOT / ".scratch" / "test-ablation" / "revision-mutation-results.json"
    output.write_text(json.dumps(matrix, indent=2, sort_keys=True) + "\n", encoding="utf-8")
    print(json.dumps(matrix, indent=2, sort_keys=True))
    required_kills = [
        "approved_manifest_anchor_removed",
        "per_file_hash_comparison_removed",
        "exact_file_set_comparison_removed",
        "revision_scaffold_fresh_verifier_call_removed",
        "accepted_production_case_set_reduced",
        "full_native_validator_bypassed",
    ]
    baselines = [
        "native_receipt_case_matrix_baseline",
        "native_receipt_schema_observation_baseline",
    ]
    return 0 if (
        all(matrix[name].get("mutation_killed") is True for name in required_kills)
        and all(matrix[name]["successful"] is True for name in baselines)
    ) else 1


if __name__ == "__main__":
    raise SystemExit(main())
