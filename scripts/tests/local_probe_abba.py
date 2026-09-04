"""Local-only tests for A-B-B-A scheduling and arithmetic."""

from __future__ import annotations

import argparse
import tempfile
import unittest
from pathlib import Path
from unittest import mock

from scripts.markturbo_tools import metrics, probe
from scripts.tests.test_probe import fixture_startup_sample


class StartupAbbaTests(unittest.TestCase):
    def test_startup_samples_follow_abba_order_without_losing_records(self) -> None:
        order: list[str] = []

        def sample(label: str):
            def measure() -> str:
                order.append(label)
                return f"{label}-{len(order)}"

            return measure

        a, b = probe.measure_startup_abba(2, sample("A"), sample("B"))

        self.assertEqual(order, ["A", "B", "B", "A", "A", "B", "B", "A"])
        self.assertEqual(a, ("A-1", "A-4", "A-5", "A-8"))
        self.assertEqual(b, ("B-2", "B-3", "B-6", "B-7"))


class ScalarAbbaTests(unittest.TestCase):
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


class StartupProfileAbbaTests(unittest.TestCase):
    @staticmethod
    def args(exe: Path, compare: Path, cache_state: str) -> argparse.Namespace:
        return argparse.Namespace(
            exe=exe,
            compare=compare,
            open=None,
            rounds=2,
            warmup=1,
            timeout=30.0,
            milestones=True,
            label=None,
            compare_label=None,
            cache_state=cache_state,
            evidence=None,
            quiet_evidence=None,
            threshold_evidence=None,
            idle_settle=0.0,
        )

    def profile_calls(self, cache_state: str) -> list[tuple[str, Path | None]]:
        from scripts.markturbo_tools.native import runtime as native_runtime

        with tempfile.TemporaryDirectory() as directory:
            exe_a = Path(directory) / "a.exe"
            exe_b = Path(directory) / "b.exe"
            exe_a.write_bytes(b"")
            exe_b.write_bytes(b"")
            calls: list[tuple[str, Path | None]] = []

            def fake_measure(
                exe: Path,
                target: str | None,
                timeout: float,
                *,
                win32: object,
                parent_context: object,
                idle_settle: float,
                profile_root: Path | None = None,
            ) -> probe.StartupSample:
                calls.append((exe.name, profile_root))
                return fixture_startup_sample("welcome")

            with (
                mock.patch.object(
                    native_runtime,
                    "sha256_file",
                    return_value=mock.Mock(sha256="hash"),
                ),
                mock.patch.object(
                    native_runtime,
                    "preflight",
                    side_effect=lambda *args, **kwargs: (object(), object()),
                ),
                mock.patch.object(
                    probe, "startup_milestones_once", side_effect=fake_measure
                ),
                mock.patch.object(probe, "summarize_startup_milestones"),
                mock.patch.object(probe, "milestone_comparison", return_value={}),
            ):
                probe.cmd_startup_milestones(
                    self.args(exe_a, exe_b, cache_state), [exe_a, exe_b]
                )

        return calls

    def test_warm_mode_reuses_distinct_profile_roots_per_variant(self) -> None:
        calls = self.profile_calls("warm")

        a_roots = [profile_root for name, profile_root in calls if name == "a.exe"]
        b_roots = [profile_root for name, profile_root in calls if name == "b.exe"]
        self.assertEqual(len(a_roots), 5)
        self.assertEqual(len(b_roots), 5)
        self.assertTrue(all(root == a_roots[0] for root in a_roots))
        self.assertTrue(all(root == b_roots[0] for root in b_roots))
        self.assertIsNotNone(a_roots[0])
        self.assertIsNotNone(b_roots[0])
        self.assertNotEqual(a_roots[0], b_roots[0])

    def test_fresh_profile_mode_passes_no_profile_root(self) -> None:
        roots = [profile_root for _, profile_root in self.profile_calls("fresh-profile")]
        self.assertEqual(roots, [None] * 10)


if __name__ == "__main__":
    unittest.main()
