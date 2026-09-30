"""Small, dependency-free helpers shared by measurement tools."""

from __future__ import annotations

import math
import statistics
from collections.abc import Callable
from dataclasses import dataclass
from typing import TypeVar


T = TypeVar("T")


def nearest_rank_percentile(values: list[float], quantile: float) -> float:
    """Return a nearest-rank percentile for a non-empty sample."""
    if not values:
        raise ValueError("cannot calculate a percentile of zero samples")
    if not 0 < quantile <= 1:
        raise ValueError("quantile must be in the range (0, 1]")
    ordered = sorted(values)
    index = math.ceil(quantile * len(ordered)) - 1
    return ordered[index]


def inclusive_p95(values: list[float]) -> float:
    """Return p95 using the inclusive method used by startup measurements."""
    if len(values) < 2:
        raise ValueError("inclusive p95 requires at least two samples")
    return statistics.quantiles(values, n=20, method="inclusive")[-1]


@dataclass(frozen=True)
class AbbaComparison:
    """Samples and paired results from an A-B-B-A measurement sequence."""

    samples_a: tuple[float, ...]
    samples_b: tuple[float, ...]
    paired_a: tuple[float, ...]
    paired_b: tuple[float, ...]
    deltas: tuple[float, ...]
    percentages: tuple[float, ...]


def measure_abba_samples(
    rounds: int,
    measure_a: Callable[[], T],
    measure_b: Callable[[], T],
) -> tuple[tuple[T, ...], tuple[T, ...]]:
    """Collect samples by variant in strict A-B-B-A order."""
    if rounds < 1:
        raise ValueError("rounds must be at least 1")

    samples_a: list[T] = []
    samples_b: list[T] = []
    for _ in range(rounds):
        a_first = measure_a()
        b_first = measure_b()
        b_second = measure_b()
        a_second = measure_a()
        samples_a.extend((a_first, a_second))
        samples_b.extend((b_first, b_second))
    return tuple(samples_a), tuple(samples_b)


def summarize_abba_samples(
    samples_a: tuple[float, ...],
    samples_b: tuple[float, ...],
    pair_mean: Callable[[tuple[float, ...]], float] = statistics.fmean,
) -> AbbaComparison:
    """Summarize adjacent pairs from already collected A-B-B-A samples."""
    paired_a = tuple(
        pair_mean(samples_a[index : index + 2])
        for index in range(0, len(samples_a), 2)
    )
    paired_b = tuple(
        pair_mean(samples_b[index : index + 2])
        for index in range(0, len(samples_b), 2)
    )
    deltas = tuple(b - a for a, b in zip(paired_a, paired_b, strict=True))
    percentages = tuple(
        delta / a * 100 for a, delta in zip(paired_a, deltas, strict=True)
    )
    return AbbaComparison(
        samples_a,
        samples_b,
        paired_a,
        paired_b,
        deltas,
        percentages,
    )


def measure_abba(
    rounds: int,
    measure_a: Callable[[], float],
    measure_b: Callable[[], float],
) -> AbbaComparison:
    """Measure two variants in A-B-B-A order to reduce drift bias."""
    samples_a, samples_b = measure_abba_samples(rounds, measure_a, measure_b)
    try:
        return summarize_abba_samples(samples_a, samples_b)
    except ZeroDivisionError as error:
        raise ValueError("A-B-B-A baseline samples must be non-zero") from error
