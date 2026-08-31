#!/usr/bin/env -S uv run --script
# /// script
# requires-python = ">=3.11"
# dependencies = []
# ///
"""Unit tests for recovery-capacity.py output parsing."""

from __future__ import annotations

import runpy
import unittest
from pathlib import Path


HARNESS = runpy.run_path(Path(__file__).with_name("recovery-capacity.py"))
DURATION_SECONDS = HARNESS["duration_seconds"]
PARSE_CAPACITY_OUTPUT = HARNESS["parse_capacity_output"]
CAPACITY_BUDGET_FAILURE = HARNESS["capacity_budget_failure"]
CAPACITY_MEASUREMENT = HARNESS["CapacityMeasurement"]
CAPACITY_ROUND = HARNESS["CapacityRound"]
SUMMARIZE_CAPACITY = HARNESS["summarize_capacity"]


VALID_OUTPUT = """\
recovery DPAPI capacity ciphertext total: 124000000
recovery DPAPI capacity batch round 0: 1.2s
recovery DPAPI capacity ciphertext total: 125000000
recovery DPAPI capacity batch round 1: 900ms
recovery DPAPI capacity ciphertext total: 126000000
recovery DPAPI capacity batch round 2: 1.1s
recovery DPAPI capacity batch median: 1.1s; max: 1.2s
"""


class DurationParsingTests(unittest.TestCase):
    def test_normalizes_rust_duration_units(self) -> None:
        self.assertEqual(DURATION_SECONDS("5ns"), 5e-9)
        self.assertEqual(DURATION_SECONDS("7µs"), 7e-6)
        self.assertEqual(DURATION_SECONDS("8us"), 8e-6)
        self.assertEqual(DURATION_SECONDS("9.5ms"), 0.0095)
        self.assertEqual(DURATION_SECONDS("1.25s"), 1.25)

    def test_rejects_unknown_duration(self) -> None:
        with self.assertRaisesRegex(ValueError, "unrecognized Rust duration"):
            DURATION_SECONDS("one second")


class CapacityOutputTests(unittest.TestCase):
    def test_parses_complete_consistent_rust_output(self) -> None:
        measurement = PARSE_CAPACITY_OUTPUT(VALID_OUTPUT)

        self.assertEqual([round_result.number for round_result in measurement.rounds], [0, 1, 2])
        self.assertEqual(
            [round_result.ciphertext_bytes for round_result in measurement.rounds],
            [124000000, 125000000, 126000000],
        )
        self.assertEqual(measurement.median_seconds, 1.1)
        self.assertEqual(measurement.max_seconds, 1.2)

    def test_parses_windows_line_endings(self) -> None:
        measurement = PARSE_CAPACITY_OUTPUT(VALID_OUTPUT.replace("\n", "\r\n"))

        self.assertEqual(measurement.median_seconds, 1.1)

    def test_rejects_missing_ciphertext_evidence(self) -> None:
        output = VALID_OUTPUT.replace(
            "recovery DPAPI capacity ciphertext total: 126000000\n", ""
        )

        with self.assertRaisesRegex(ValueError, "expected 3 ciphertext totals, found 2"):
            PARSE_CAPACITY_OUTPUT(output)

    def test_rejects_inconsistent_reported_median(self) -> None:
        output = VALID_OUTPUT.replace("median: 1.1s", "median: 900ms")

        with self.assertRaisesRegex(ValueError, "reported capacity median does not match"):
            PARSE_CAPACITY_OUTPUT(output)

    def test_rejects_inconsistent_reported_max(self) -> None:
        output = VALID_OUTPUT.replace("max: 1.2s", "max: 1.1s")

        with self.assertRaisesRegex(ValueError, "reported capacity max does not match"):
            PARSE_CAPACITY_OUTPUT(output)

    def test_rejects_out_of_order_rounds(self) -> None:
        output = VALID_OUTPUT.replace("round 1: 900ms", "round 3: 900ms")

        with self.assertRaisesRegex(ValueError, "capacity rounds must appear once in order"):
            PARSE_CAPACITY_OUTPUT(output)


class CapacitySummaryTests(unittest.TestCase):
    def test_budget_failure_uses_global_round_maximum(self) -> None:
        measurement = CAPACITY_MEASUREMENT(
            rounds=(
                CAPACITY_ROUND(0, 7.9, 124000000),
                CAPACITY_ROUND(1, 8.01, 125000000),
                CAPACITY_ROUND(2, 7.8, 126000000),
            ),
            median_seconds=7.9,
            max_seconds=8.01,
        )

        summary = SUMMARIZE_CAPACITY([measurement])

        self.assertEqual(summary.round_count, 3)
        self.assertIn("8.010000000s exceeds", CAPACITY_BUDGET_FAILURE(summary))


if __name__ == "__main__":
    unittest.main()
