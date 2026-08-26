"""Synthetic tests for probe.py's geometry contracts."""

from __future__ import annotations

import runpy
import unittest
from pathlib import Path


PROBE = runpy.run_path(Path(__file__).with_name("probe.py"))
RECT = PROBE["RECT"]
EXPECTED_CHILD_FAILURES = PROBE["expected_child_failures"]
DURATION_US = PROBE["duration_us"]
PERCENTILE = PROBE["percentile"]
QUIET_GATE_FAILURES = PROBE["quiet_gate_failures"]


class NativeChromeGeometryTests(unittest.TestCase):
    def setUp(self) -> None:
        self.client = RECT(100, 200, 1300, 1000)

    def failures(self, child, require_insets: bool):
        return EXPECTED_CHILD_FAILURES(
            [(1, "WRY_WEBVIEW", child, True)],
            ["WRY_WEBVIEW"],
            self.client,
            require_insets,
        )

    def test_full_client_child_only_fails_opt_in_chrome_contract(self) -> None:
        child = RECT(100, 200, 1300, 1000)

        self.assertEqual(self.failures(child, False), [])
        self.assertRegex(
            self.failures(child, True)[0],
            r"top=0,bottom=0",
        )

    def test_positive_top_and_bottom_insets_pass(self) -> None:
        child = RECT(100, 272, 1300, 972)

        self.assertEqual(self.failures(child, True), [])


class QuietGateTests(unittest.TestCase):
    def test_nearest_rank_percentile_matches_the_gate_contract(self) -> None:
        self.assertEqual(PERCENTILE([1, 2, 3, 4, 100], 0.95), 100)

    def test_quiet_gate_reports_only_exceeded_limits(self) -> None:
        failures = QUIET_GATE_FAILURES(
            [2.0, 4.0, 6.0],
            [0.5, 1.0, 3.0],
            5.0,
            10.0,
            2.0,
            2.0,
        )

        self.assertEqual(failures, ["disk p95 3.00% > 2.00%"])

    def test_rust_duration_units_are_normalized_to_microseconds(self) -> None:
        self.assertEqual(DURATION_US("first 8.6ms  subsequent 1.4ms"), 8600.0)
        self.assertEqual(DURATION_US("first 750µs  subsequent 200µs"), 750.0)


if __name__ == "__main__":
    unittest.main()
