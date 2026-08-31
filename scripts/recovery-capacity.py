#!/usr/bin/env -S uv run --script
# /// script
# requires-python = ">=3.11"
# dependencies = []
# ///
"""Measure Windows DPAPI recovery capacity with fresh Rust test processes.

The ignored Rust test performs three near-capacity checkpoint commits. This
harness runs that test in fresh cargo processes, validates its printed evidence,
and prints the raw per-round measurements plus an aggregate summary.
"""

from __future__ import annotations

import argparse
import re
import statistics
import subprocess
import sys
from dataclasses import dataclass
from pathlib import Path


REPO = Path(__file__).resolve().parent.parent
DEFAULT_INVOCATIONS = 3
RUST_TEST = "recovery::tests::recovery_capacity_batch_commits_within_budget"
EXPECTED_ROUNDS = 3
CHECKPOINT_COMMIT_BUDGET_SECONDS = 8.0

ROUND_RE = re.compile(
    r"^recovery DPAPI capacity batch round (?P<round>\d+): (?P<duration>\S+)$",
    re.MULTILINE,
)
CIPHERTEXT_RE = re.compile(
    r"^recovery DPAPI capacity ciphertext total: (?P<bytes>\d+)$",
    re.MULTILINE,
)
SUMMARY_RE = re.compile(
    r"^recovery DPAPI capacity batch median: (?P<median>\S+); max: (?P<maximum>\S+)$",
    re.MULTILINE,
)
DURATION_RE = re.compile(r"^(?P<value>\d+(?:\.\d+)?)(?P<unit>ns|µs|us|ms|s)$")


@dataclass(frozen=True)
class CapacityRound:
    number: int
    elapsed_seconds: float
    ciphertext_bytes: int


@dataclass(frozen=True)
class CapacityMeasurement:
    rounds: tuple[CapacityRound, ...]
    median_seconds: float
    max_seconds: float


@dataclass(frozen=True)
class CapacitySummary:
    round_count: int
    duration_median_seconds: float
    duration_max_seconds: float
    ciphertext_median_bytes: float
    ciphertext_max_bytes: int


def duration_seconds(value: str) -> float:
    """Normalize Rust Debug duration output to seconds."""
    match = DURATION_RE.fullmatch(value)
    if match is None:
        raise ValueError(f"unrecognized Rust duration: {value!r}")

    factor = {
        "ns": 1e-9,
        "µs": 1e-6,
        "us": 1e-6,
        "ms": 1e-3,
        "s": 1.0,
    }[match["unit"]]
    return float(match["value"]) * factor


def parse_capacity_output(output: str) -> CapacityMeasurement:
    """Parse and cross-check the evidence emitted by the ignored Rust test."""
    output = output.replace("\r\n", "\n")
    parsed_rounds = [
        (int(match["round"]), duration_seconds(match["duration"]))
        for match in ROUND_RE.finditer(output)
    ]
    ciphertext_totals = [int(match["bytes"]) for match in CIPHERTEXT_RE.finditer(output)]
    summaries = list(SUMMARY_RE.finditer(output))

    if len(parsed_rounds) != EXPECTED_ROUNDS:
        raise ValueError(
            f"expected {EXPECTED_ROUNDS} capacity round durations, found {len(parsed_rounds)}"
        )
    if [number for number, _ in parsed_rounds] != list(range(EXPECTED_ROUNDS)):
        raise ValueError(
            "capacity rounds must appear once in order as "
            f"0..{EXPECTED_ROUNDS - 1}, found {[number for number, _ in parsed_rounds]}"
        )
    if len(ciphertext_totals) != EXPECTED_ROUNDS:
        raise ValueError(
            f"expected {EXPECTED_ROUNDS} ciphertext totals, found {len(ciphertext_totals)}"
        )
    if any(total <= 0 for total in ciphertext_totals):
        raise ValueError("capacity ciphertext totals must be positive")
    if len(summaries) != 1:
        raise ValueError(f"expected one capacity median/max summary, found {len(summaries)}")

    expected_median = statistics.median(seconds for _, seconds in parsed_rounds)
    expected_max = max(seconds for _, seconds in parsed_rounds)
    reported_median = duration_seconds(summaries[0]["median"])
    reported_max = duration_seconds(summaries[0]["maximum"])
    if not durations_match(reported_median, expected_median):
        raise ValueError(
            "reported capacity median does not match its rounds: "
            f"reported {reported_median:.9f}s, expected {expected_median:.9f}s"
        )
    if not durations_match(reported_max, expected_max):
        raise ValueError(
            "reported capacity max does not match its rounds: "
            f"reported {reported_max:.9f}s, expected {expected_max:.9f}s"
        )

    rounds = tuple(
        CapacityRound(number, elapsed_seconds, ciphertext_totals[number])
        for number, elapsed_seconds in parsed_rounds
    )
    return CapacityMeasurement(rounds, reported_median, reported_max)


def durations_match(reported: float, calculated: float) -> bool:
    """Allow only the rounding loss introduced by Rust's compact Debug format."""
    return abs(reported - calculated) <= max(1e-9, calculated * 1e-6)


def cargo_command() -> list[str]:
    return [
        "cargo",
        "test",
        "-p",
        "mt-app",
        "--release",
        "--locked",
        "--lib",
        RUST_TEST,
        "--",
        "--ignored",
        "--exact",
        "--nocapture",
    ]


def run_cargo_invocation() -> CapacityMeasurement:
    try:
        completed = subprocess.run(
            cargo_command(),
            cwd=REPO,
            text=True,
            capture_output=True,
            check=False,
        )
    except OSError as error:
        raise RuntimeError(f"could not start capacity cargo invocation: {error}") from error
    output = completed.stdout + completed.stderr
    if completed.returncode != 0:
        raise RuntimeError(
            f"capacity cargo invocation failed with exit code {completed.returncode}:\n{output}"
        )
    try:
        return parse_capacity_output(output)
    except ValueError as error:
        raise RuntimeError(f"capacity cargo invocation emitted invalid evidence: {error}\n{output}") from error


def print_measurement(invocation: int, measurement: CapacityMeasurement) -> None:
    print(f"invocation {invocation}:")
    for round_result in measurement.rounds:
        print(
            f"  round {round_result.number}: {round_result.elapsed_seconds:.9f} s; "
            f"ciphertext {round_result.ciphertext_bytes} bytes"
        )
    print(f"  Rust median: {measurement.median_seconds:.9f} s")
    print(f"  Rust max: {measurement.max_seconds:.9f} s")


def summarize_capacity(measurements: list[CapacityMeasurement]) -> CapacitySummary:
    all_rounds = [round_result for measurement in measurements for round_result in measurement.rounds]
    if not all_rounds:
        raise ValueError("cannot summarize zero capacity rounds")
    durations = [round_result.elapsed_seconds for round_result in all_rounds]
    ciphertexts = [round_result.ciphertext_bytes for round_result in all_rounds]
    return CapacitySummary(
        round_count=len(all_rounds),
        duration_median_seconds=statistics.median(durations),
        duration_max_seconds=max(durations),
        ciphertext_median_bytes=statistics.median(ciphertexts),
        ciphertext_max_bytes=max(ciphertexts),
    )


def capacity_budget_failure(summary: CapacitySummary) -> str | None:
    if summary.duration_max_seconds > CHECKPOINT_COMMIT_BUDGET_SECONDS:
        return (
            "recovery capacity maximum "
            f"{summary.duration_max_seconds:.9f}s exceeds the "
            f"{CHECKPOINT_COMMIT_BUDGET_SECONDS:.0f}-second post-dispatch budget"
        )
    return None


def print_global_summary(measurements: list[CapacityMeasurement]) -> CapacitySummary:
    summary = summarize_capacity(measurements)
    print("global summary:")
    print(f"  cargo invocations: {len(measurements)}")
    print(f"  capacity rounds: {summary.round_count}")
    print(f"  duration median: {summary.duration_median_seconds:.9f} s")
    print(f"  duration max: {summary.duration_max_seconds:.9f} s")
    print(f"  ciphertext median: {summary.ciphertext_median_bytes:.0f} bytes")
    print(f"  ciphertext max: {summary.ciphertext_max_bytes} bytes")
    print(f"  post-dispatch budget: {CHECKPOINT_COMMIT_BUDGET_SECONDS:.0f} s")
    return summary


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--invocations",
        type=int,
        default=DEFAULT_INVOCATIONS,
        help=f"fresh cargo test processes to run (default: {DEFAULT_INVOCATIONS})",
    )
    args = parser.parse_args()
    if args.invocations <= 0:
        parser.error("--invocations must be positive")
    return args


def main() -> int:
    if sys.platform != "win32":
        print("recovery capacity harness requires Windows current-user DPAPI", file=sys.stderr)
        return 2

    args = parse_args()
    print(f"recovery capacity harness: {args.invocations} fresh cargo invocation(s)")
    print("command: " + " ".join(cargo_command()))
    measurements: list[CapacityMeasurement] = []
    for invocation in range(1, args.invocations + 1):
        try:
            measurement = run_cargo_invocation()
        except RuntimeError as error:
            print(f"invocation {invocation} failed: {error}", file=sys.stderr)
            return 1
        measurements.append(measurement)
        print_measurement(invocation, measurement)

    summary = print_global_summary(measurements)
    if failure := capacity_budget_failure(summary):
        print(f"FAIL: {failure}", file=sys.stderr)
        return 1
    print(
        "PASS: all capacity rounds stayed within the "
        f"{CHECKPOINT_COMMIT_BUDGET_SECONDS:.0f}-second post-dispatch budget"
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
