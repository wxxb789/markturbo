"""Compare the old and replacement fixture detectors under isolated faults."""

from __future__ import annotations

import importlib.util
import json
import sys
import unittest
from pathlib import Path
from unittest import mock

sys.path.insert(0, str(Path(__file__).resolve().parents[2]))

from scripts.markturbo_tools import perf_fixtures
from scripts.tests.test_perf_fixtures import PerformanceFixtureTests


def result_for(test_class, method):
    result = unittest.TestResult()
    test_class(method).run(result)
    return {
        "run": result.testsRun,
        "failures": len(result.failures),
        "errors": len(result.errors),
    }


spec = importlib.util.spec_from_file_location(
    "perf_baseline", Path(__file__).with_name("perf-baseline.py")
)
baseline = importlib.util.module_from_spec(spec)
spec.loader.exec_module(baseline)
original_make = perf_fixtures.make
original_diagrams = perf_fixtures.make_diagram_heavy
faults = (
    ("numbering", "make", lambda n: original_make(n).replace("## Section 1\n", "## Section 0\n")),
    ("unicode", "make", lambda n: original_make(n).replace("\u4e2d\u6587", "ascii")),
    ("fence_kind", "make", lambda n: original_make(n).replace("```rust", "```text")),
    ("truncation", "make", lambda n: original_make(n)[:-2] + "\n"),
    ("line_count", "make", lambda n: original_make(n - 1 if n > 40 else n)),
    ("newlines", "make", lambda n: original_make(n).replace("\n", "\r\n")),
    ("diagram_ids", "make_diagram_heavy", lambda count=60: original_diagrams(count).replace("Node 0", "Changed 0")),
    ("diagram_default_count", "make_diagram_heavy", lambda count=60: original_diagrams(59 if count == 60 else count)),
)
results = []
for name, target, replacement in faults:
    with mock.patch.object(perf_fixtures, target, replacement):
        old = result_for(
            baseline.PerformanceFixtureTests, "test_document_generation_is_deterministic"
        )
        new = result_for(
            PerformanceFixtureTests,
            "test_generated_bytes_match_the_committed_benchmark_inputs",
        )
    results.append({"fault": name, "original": old, "replacement": new})
    if old != {"run": 1, "failures": 0, "errors": 0}:
        raise AssertionError(f"unexpected original detector result: {name}")
    if new["run"] != 1 or new["failures"] == 0 or new["errors"] != 0:
        raise AssertionError(f"replacement did not detect the fault: {name}")
print(json.dumps(results, indent=2))
