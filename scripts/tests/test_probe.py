"""Synthetic tests for probe geometry and measurement contracts."""

from __future__ import annotations

import unittest
from pathlib import Path
from unittest import mock

from scripts.markturbo_tools import metrics, probe

RECT = probe.RECT
EXPECTED_CHILD_FAILURES = probe.expected_child_failures
DURATION_US = probe.duration_us
PERCENTILE = metrics.nearest_rank_percentile
QUIET_GATE_FAILURES = probe.quiet_gate_failures


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


class FormulaProbeTests(unittest.TestCase):
    @staticmethod
    def completed_probe() -> mock.Mock:
        return mock.Mock(
            returncode=0,
            stdout="first 1ms  subsequent 250us",
            stderr="",
        )

    def test_default_formula_probe_clears_an_ambient_font_override(self) -> None:
        with (
            mock.patch.dict(probe.os.environ, {"MT_MATH_FONT_DIR": "ambient"}),
            mock.patch.object(
                probe.subprocess,
                "run",
                return_value=self.completed_probe(),
            ) as run,
        ):
            self.assertEqual(probe.formula_once(Path("test.exe"), None, 5.0), 1000.0)

        self.assertNotIn("MT_MATH_FONT_DIR", run.call_args.kwargs["env"])

    def test_formula_probe_sets_only_an_explicit_font_override(self) -> None:
        override = Path("override-fonts")
        with mock.patch.object(
            probe.subprocess,
            "run",
            return_value=self.completed_probe(),
        ) as run:
            probe.formula_once(Path("test.exe"), override, 5.0)

        self.assertEqual(run.call_args.kwargs["env"]["MT_MATH_FONT_DIR"], str(override))


class AbbaMeasurementTests(unittest.TestCase):
    def test_interleaves_samples_and_calculates_paired_deltas(self) -> None:
        a_samples = iter((10.0, 14.0))
        b_samples = iter((20.0, 22.0))

        comparison = metrics.measure_abba(1, lambda: next(a_samples), lambda: next(b_samples))

        self.assertEqual(comparison.samples_a, (10.0, 14.0))
        self.assertEqual(comparison.samples_b, (20.0, 22.0))
        self.assertEqual(comparison.paired_a, (12.0,))
        self.assertEqual(comparison.paired_b, (21.0,))
        self.assertEqual(comparison.deltas, (9.0,))
        self.assertEqual(comparison.percentages, (75.0,))

    def test_runs_each_round_in_abba_order(self) -> None:
        order: list[str] = []

        def sample(label: str, value: float):
            def measure() -> float:
                order.append(label)
                return value

            return measure

        metrics.measure_abba(2, sample("A", 10.0), sample("B", 12.0))

        self.assertEqual(order, ["A", "B", "B", "A", "A", "B", "B", "A"])

    def test_rejects_invalid_percentile_inputs(self) -> None:
        with self.assertRaisesRegex(ValueError, "zero samples"):
            metrics.nearest_rank_percentile([], 0.95)
        with self.assertRaisesRegex(ValueError, "range"):
            metrics.nearest_rank_percentile([1.0], 0.0)


if __name__ == "__main__":
    unittest.main()
