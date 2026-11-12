"""Run native test-ablation mutations in isolated Python interpreters.

This experiment mutates imported objects only. It never edits production source.
"""

from __future__ import annotations

import dataclasses
import importlib.util
import json
import shutil
import sys
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT))

from scripts.markturbo_tools.native import goal03, runtime

BASELINE_PATH = ROOT / ".scratch" / "test-ablation" / "native-baseline-goal03.py"


def load_goal03_module(suite: str):
    if suite == "ablated":
        import scripts.tests.test_native_goal03 as module

        return module
    spec = importlib.util.spec_from_file_location("native_baseline_goal03", BASELINE_PATH)
    if spec is None or spec.loader is None:
        raise RuntimeError("could not load baseline Goal03 test module")
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    return module


def install_mutation(name: str) -> None:
    if name == "partial_pass":
        original = runtime.run_native_acceptance

        def partial_pass(args, plan):
            returncode, evidence, code = original(args, plan)
            if code == "PARTIAL_CASE_RUN":
                return 0, evidence, "ALL_REQUIRED_CASES"
            return returncode, evidence, code

        runtime.run_native_acceptance = partial_pass
        return

    if name == "cleanup_noop":
        runtime.remove_tree_with_retry = lambda *_args, **_kwargs: None
        return

    if name == "retention_delete":
        original = runtime.run_native_acceptance

        def retention_delete(args, plan):
            result = original(args, plan)
            debug_workdir = getattr(args, "debug_workdir", None)
            if debug_workdir is not None:
                shutil.rmtree(debug_workdir, ignore_errors=True)
            return result

        runtime.run_native_acceptance = retention_delete
        return

    if name in {"scenario_swap", "scenario_duplicate"}:
        original = goal03.native_run_plan

        def mutated_plan():
            plan = original()

            def scenarios(harness):
                values = list(plan.scenarios(harness))
                if name == "scenario_swap":
                    values[0], values[1] = values[1], values[0]
                else:
                    values[-1] = values[0]
                return tuple(values)

            return dataclasses.replace(plan, scenarios=scenarios)

        goal03.native_run_plan = mutated_plan
        return

    raise ValueError(f"unknown mutation: {name}")


def selected_suite(suite: str, mutation: str) -> unittest.TestSuite:
    loader = unittest.TestLoader()
    result = unittest.TestSuite()
    goal03_tests = load_goal03_module(suite)

    if mutation == "partial_pass":
        result.addTests(
            loader.loadTestsFromName(
                "scripts.tests.test_native_goal02_execution.PrivacyAndCliTests."
                "test_single_case_run_never_claims_acceptance_pass"
            )
        )
        result.addTests(
            loader.loadTestsFromName(
                "scripts.tests.test_native_goal07.EvidenceFinalizationTests."
                "test_partial_case_counts_only_the_selected_observation"
            )
        )
        if suite == "baseline":
            result.addTests(
                loader.loadTestsFromName(
                    "ParserAndIsolationTests."
                    "test_single_case_run_never_claims_acceptance_pass",
                    goal03_tests,
                )
            )
    elif mutation in {"cleanup_noop", "retention_delete"}:
        result.addTests(
            loader.loadTestsFromName(
                "scripts.tests.test_native_goal02_execution.PrivacyAndCliTests."
                "test_run_lifecycle_cleans_success_and_keeps_failure_or_partial_workdirs"
            )
        )
        if suite == "baseline":
            result.addTests(
                loader.loadTestsFromName(
                    "ParserAndIsolationTests."
                    "test_run_lifecycle_cleans_success_and_keeps_failure_or_partial_workdirs",
                    goal03_tests,
                )
            )
    else:
        test_name = (
            "test_native_run_plan_wires_each_required_case_to_its_scenario_in_order"
            if suite == "ablated"
            else "test_single_case_run_never_claims_acceptance_pass"
        )
        result.addTests(
            loader.loadTestsFromName(f"ParserAndIsolationTests.{test_name}", goal03_tests)
        )
        if suite == "baseline":
            result.addTests(
                loader.loadTestsFromName(
                    "ParserAndIsolationTests."
                    "test_run_lifecycle_cleans_success_and_keeps_failure_or_partial_workdirs",
                    goal03_tests,
                )
            )
    return result


def main() -> int:
    if len(sys.argv) != 3:
        raise SystemExit("usage: native-mutation-runner.py baseline|ablated MUTATION")
    suite, mutation = sys.argv[1:]
    if suite not in {"baseline", "ablated"}:
        raise SystemExit("suite must be baseline or ablated")
    install_mutation(mutation)
    tests = selected_suite(suite, mutation)
    outcome = unittest.TextTestRunner(stream=sys.stderr, verbosity=0).run(tests)
    record = {
        "suite": suite,
        "mutation": mutation,
        "tests_run": outcome.testsRun,
        "failures": len(outcome.failures),
        "errors": len(outcome.errors),
        "survived": outcome.wasSuccessful(),
    }
    print(json.dumps(record, sort_keys=True))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
