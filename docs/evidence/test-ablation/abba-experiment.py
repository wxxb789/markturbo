"""Serialize the A3 differential cases and compare them with the original result."""

from __future__ import annotations

import json
import sys
from dataclasses import asdict
from pathlib import Path
from typing import Callable

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT))

from scripts.markturbo_tools import goal04, metrics

ARTIFACTS = ROOT / ".scratch" / "test-ablation"


def outcome(call: Callable[[], object]) -> dict[str, object]:
    try:
        value = call()
        if hasattr(value, "__dataclass_fields__"):
            value = asdict(value)
        return {"ok": value}
    except Exception as error:
        return {"error": {"type": type(error).__name__, "message": str(error)}}


def sample(base: float) -> goal04.StartupSample:
    return goal04.StartupSample(
        process_created_ms=base,
        process_started_ms=base + 1,
        initial_state_ready_ms=base + 2,
        window_visible_ms=base + 2.5,
        first_frame_painted_ms=base + 3,
        first_input_handled_ms=base + 4,
        initial_state="welcome",
        idle_working_set_mb=base + 5,
        idle_private_mb=base + 6,
        peak_working_set_mb=base + 7,
        page_faults=int(base + 8),
        threads=int(base + 9),
    )


def measured(a: tuple[float, ...], b: tuple[float, ...]) -> metrics.AbbaComparison:
    samples_a = iter(a)
    samples_b = iter(b)
    return metrics.measure_abba(
        len(a) // 2,
        lambda: next(samples_a),
        lambda: next(samples_b),
    )


def results() -> dict[str, object]:
    return {
        "numeric_fixed": outcome(
            lambda: measured(
                (10.0, 14.0, 100.0, 104.0),
                (20.0, 22.0, 80.0, 82.0),
            )
        ),
        "numeric_nonzero_negative": outcome(
            lambda: measured((-4.0, -2.0), (3.0, 5.0))
        ),
        "numeric_zero_a": outcome(
            lambda: measured((0.0, 0.0), (1.0, 1.0))
        ),
        "numeric_zero_rounds": outcome(
            lambda: metrics.measure_abba(0, lambda: 1.0, lambda: 2.0)
        ),
        "startup_fixed": outcome(
            lambda: goal04.milestone_comparison(
                tuple(map(sample, (10, 14, 100, 104, 1000, 1004))),
                tuple(map(sample, (20, 22, 90, 92, 1100, 1104))),
            )
        ),
        "startup_unequal": outcome(
            lambda: goal04.milestone_comparison(
                tuple(map(sample, (10, 14))),
                tuple(map(sample, (20, 22, 24, 26))),
            )
        ),
        "startup_odd": outcome(
            lambda: goal04.milestone_comparison(
                tuple(map(sample, (10, 14, 18))),
                tuple(map(sample, (20, 22, 24))),
            )
        ),
        "startup_zero_a": outcome(
            lambda: goal04.milestone_comparison(
                tuple(map(sample, (-1, 1))),
                tuple(map(sample, (2, 4))),
            )
        ),
    }


def main() -> int:
    encoded = json.dumps(
        results(), sort_keys=True, separators=(",", ":"), allow_nan=False
    ) + "\n"
    refactored = ARTIFACTS / "abba-refactored.json"
    refactored.write_text(encoded, encoding="utf-8", newline="\n")
    original = (ARTIFACTS / "abba-original.json").read_bytes()
    matches = original == encoded.encode("utf-8")
    print(f"byte_identical={str(matches).lower()}")
    print(f"original_bytes={len(original)}")
    print(f"refactored_bytes={len(encoded.encode('utf-8'))}")
    return 0 if matches else 1


if __name__ == "__main__":
    raise SystemExit(main())
