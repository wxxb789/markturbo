"""Inject A3 arithmetic faults one at a time and record surviving-test outcomes."""

from __future__ import annotations

import hashlib
import json
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
METRICS = ROOT / "scripts" / "markturbo_tools" / "metrics.py"
GOAL04 = ROOT / "scripts" / "markturbo_tools" / "goal04.py"
OUTPUT = ROOT / ".scratch" / "test-ablation" / "abba-fault-controls.json"

FAULTS = (
    (
        "wrong_schedule_abab",
        METRICS,
        "        b_second = measure_b()\n        a_second = measure_a()\n",
        "        a_second = measure_a()\n        b_second = measure_b()\n",
        "scripts.tests.test_probe.AbbaMeasurementTests.test_shared_scheduler_keeps_structured_samples_separated_in_abba_order",
    ),
    (
        "cross_pair_indexes",
        METRICS,
        "        pair_mean(samples_a[index : index + 2])\n",
        "        pair_mean((samples_a[index], samples_a[(index + 2) % len(samples_a)]))\n",
        "scripts.tests.test_probe.AbbaMeasurementTests.test_structured_comparison_uses_adjacent_pairs_and_pair_medians",
    ),
    (
        "reversed_delta",
        METRICS,
        "    deltas = tuple(b - a for a, b in zip(paired_a, paired_b, strict=True))\n",
        "    deltas = tuple(a - b for a, b in zip(paired_a, paired_b, strict=True))\n",
        "scripts.tests.test_probe.AbbaMeasurementTests.test_metrics_keeps_paired_statistics_for_abba_samples",
    ),
    (
        "allow_zero_baseline",
        METRICS,
        "        delta / a * 100 for a, delta in zip(paired_a, deltas, strict=True)\n",
        "        0.0 if a == 0 else delta / a * 100\n        for a, delta in zip(paired_a, deltas, strict=True)\n",
        "scripts.tests.test_probe.AbbaMeasurementTests.test_metrics_rejects_a_zero_paired_baseline",
    ),
    (
        "raw_median_delta",
        GOAL04,
        '            "median_b_minus_a": median(comparison.deltas),\n',
        '            "median_b_minus_a": median(values_b) - median(values_a),\n',
        "scripts.tests.test_probe.AbbaMeasurementTests.test_structured_comparison_uses_adjacent_pairs_and_pair_medians",
    ),
)


def main() -> int:
    originals = {path: path.read_text(encoding="utf-8") for _, path, _, _, _ in FAULTS}
    results: list[dict[str, object]] = []
    try:
        for name, path, old, new, test_id in FAULTS:
            source = originals[path]
            if source.count(old) != 1:
                raise RuntimeError(f"{name}: mutation target count was {source.count(old)}")
            path.write_text(source.replace(old, new), encoding="utf-8", newline="\n")
            completed = subprocess.run(
                [sys.executable, "-m", "unittest", "-v", test_id],
                cwd=ROOT,
                text=True,
                stdout=subprocess.PIPE,
                stderr=subprocess.STDOUT,
                check=False,
            )
            encoded = completed.stdout.encode("utf-8")
            results.append(
                {
                    "fault": name,
                    "test": test_id,
                    "exit_code": completed.returncode,
                    "killed": completed.returncode != 0,
                    "output_sha256": hashlib.sha256(encoded).hexdigest(),
                    "output": completed.stdout,
                }
            )
            path.write_text(source, encoding="utf-8", newline="\n")
    finally:
        for path, source in originals.items():
            path.write_text(source, encoding="utf-8", newline="\n")
    OUTPUT.write_text(
        json.dumps(results, indent=2, sort_keys=True) + "\n",
        encoding="utf-8",
        newline="\n",
    )
    for result in results:
        print(f"{result['fault']}: exit={result['exit_code']} killed={result['killed']}")
    return 0 if len(results) == len(FAULTS) and all(row["killed"] for row in results) else 1


if __name__ == "__main__":
    raise SystemExit(main())
