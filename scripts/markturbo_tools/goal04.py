"""Goal 04 startup/modularity evidence and controlled-build helpers.

This module is deliberately independent from ``probe`` so Windows process and
window measurement can remain a compact CLI harness.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import math
import os
import platform
import re
import shutil
import subprocess
import sys
import tomllib
from collections.abc import Callable
from dataclasses import dataclass
from datetime import UTC, datetime, timedelta
from pathlib import Path
from statistics import median
from typing import TypeVar

from .metrics import inclusive_p95, nearest_rank_percentile

REPO = Path(__file__).resolve().parents[2]
STARTUP_TRACE_SCHEMA = "markturbo-startup-v1"
STARTUP_QUIET_SCHEMA = "markturbo-goal-04-quiet-v1"
STARTUP_BUILD_SCHEMA = "markturbo-goal-04-build-v1"
STARTUP_THRESHOLD_SCHEMA = "markturbo-goal-04-threshold-v1"
STARTUP_EVIDENCE_SCHEMA = "markturbo-goal-04-startup-v1"
MODEL_FIRST_USE_EVIDENCE_SCHEMA = "markturbo-goal-04-model-first-use-v1"
MODEL_TRANSPORT_DECISION_SCHEMA = "markturbo-goal-04-decision-v1"
GOAL04_TARGET = "x86_64-pc-windows-msvc"
QUIET_EVIDENCE_MAX_AGE = timedelta(minutes=5)
MAX_INPUT_EVIDENCE_BYTES = 1024 * 1024
SHA256_PATTERN = re.compile(r"[0-9a-f]{64}")
STARTUP_TRACE_EVENTS = (
    "process_started",
    "initial_state_ready",
    "first_frame_painted",
    "first_input_handled",
)
EVIDENCE_VARIANT_LABELS = {"full", "no-model", "bare", "opt-3", "opt-s"}
PATH_ARGUMENTS = {
    "--app-build-evidence",
    "--app-exe",
    "--compare",
    "--compare-build-evidence",
    "--build-evidence",
    "--evidence",
    "--exe",
    "--fresh-profile-evidence",
    "--font-dir",
    "--log",
    "--open",
    "--out",
    "--quiet-evidence",
    "--target-dir",
    "--threshold-evidence",
    "--warm-evidence",
}
MODEL_TRANSPORT_DECISIONS = (
    "keep in-process",
    "isolate in a worker",
    "investigate upstream",
    "reject",
)
MODEL_ATTRIBUTION_PACKAGES = (
    "genai",
    "reqwest",
    "rustls",
    "tokio",
    "hyper",
    "tokio-rustls",
)
STARTUP_MILESTONE_FIELDS = (
    "process_created_ms",
    "process_started_ms",
    "initial_state_ready_ms",
    "window_visible_ms",
    "first_frame_painted_ms",
    "first_input_handled_ms",
)
STARTUP_IDLE_FIELDS = (
    "idle_working_set_mb",
    "idle_private_mb",
    "peak_working_set_mb",
    "page_faults",
    "threads",
)
STARTUP_COMPARISON_FIELDS = STARTUP_MILESTONE_FIELDS + STARTUP_IDLE_FIELDS


@dataclass(frozen=True)
class StartupTraceEvent:
    counter: int
    frequency: int
    detail: str | None = None


@dataclass(frozen=True)
class StartupSample:
    process_created_ms: float
    process_started_ms: float
    initial_state_ready_ms: float
    window_visible_ms: float
    first_frame_painted_ms: float
    first_input_handled_ms: float
    initial_state: str
    idle_working_set_mb: float
    idle_private_mb: float
    peak_working_set_mb: float
    page_faults: int
    threads: int

    def evidence(self) -> dict[str, float | str]:
        return {
            "process_created_ms": self.process_created_ms,
            "process_started_ms": self.process_started_ms,
            "initial_state_ready_ms": self.initial_state_ready_ms,
            "window_visible_ms": self.window_visible_ms,
            "first_frame_painted_ms": self.first_frame_painted_ms,
            "first_input_handled_ms": self.first_input_handled_ms,
            "initial_state": self.initial_state,
            "idle_working_set_mb": self.idle_working_set_mb,
            "idle_private_mb": self.idle_private_mb,
            "peak_working_set_mb": self.peak_working_set_mb,
            "page_faults": self.page_faults,
            "threads": self.threads,
        }


@dataclass(frozen=True)
class Goal04BuildVariant:
    artifact_kind: str
    target_name: str
    no_default_features: bool
    opt_level: str
    model_transport: bool
    role: str


GOAL04_BUILD_VARIANTS = {
    "full": Goal04BuildVariant("application", "markturbo", False, "3", True, "decision"),
    "no-model": Goal04BuildVariant(
        "application", "markturbo", True, "3", False, "decision"
    ),
    "bare": Goal04BuildVariant(
        "application", "markturbo-gpui-shell", True, "3", False, "diagnostic"
    ),
    "opt-3": Goal04BuildVariant(
        "application", "markturbo", False, "3", True, "diagnostic"
    ),
    "opt-s": Goal04BuildVariant(
        "application", "markturbo", False, "s", True, "diagnostic"
    ),
    "model-first-use": Goal04BuildVariant(
        "test", "model_first_use_cost", False, "3", True, "baseline"
    ),
}


def parse_startup_trace(
    text: str,
    *,
    nonce: str,
    pid: int,
    frequency: int,
) -> dict[str, StartupTraceEvent]:
    """Parse the app's content-free startup trace, rejecting mixed runs."""
    events: dict[str, StartupTraceEvent] = {}
    for line_number, line in enumerate(text.splitlines(), start=1):
        if not line.strip():
            continue
        try:
            row = json.loads(line)
        except json.JSONDecodeError as error:
            raise ValueError(f"invalid startup trace JSON on line {line_number}") from error
        if not isinstance(row, dict) or row.get("schema") != STARTUP_TRACE_SCHEMA:
            raise ValueError(f"invalid startup trace schema on line {line_number}")
        if row.get("nonce") != nonce:
            raise ValueError(f"startup trace nonce mismatch on line {line_number}")
        if row.get("pid") != pid:
            raise ValueError(f"startup trace pid mismatch on line {line_number}")
        if row.get("frequency") != frequency:
            raise ValueError(f"startup trace frequency mismatch on line {line_number}")
        name = row.get("event")
        if name not in STARTUP_TRACE_EVENTS:
            raise ValueError(f"unknown startup event on line {line_number}: {name!r}")
        if name in events:
            raise ValueError(f"duplicate startup event: {name}")
        counter = row.get("counter")
        detail = row.get("detail")
        if not isinstance(counter, int) or counter < 0:
            raise ValueError(f"invalid startup counter on line {line_number}")
        if detail is not None and not isinstance(detail, str):
            raise ValueError(f"invalid startup event detail on line {line_number}")
        events[name] = StartupTraceEvent(counter, frequency, detail)
    return events


class StartupTraceReader:
    """Incrementally read complete trace rows without perturbing every poll."""

    def __init__(self, path: Path, *, nonce: str, pid: int, frequency: int) -> None:
        self.path = path
        self.nonce = nonce
        self.pid = pid
        self.frequency = frequency
        self.stream = None
        self.partial = ""
        self.events: dict[str, StartupTraceEvent] = {}

    def read(self) -> dict[str, StartupTraceEvent]:
        if self.stream is None:
            try:
                self.stream = self.path.open("r", encoding="utf-8", errors="strict", newline="")
            except FileNotFoundError:
                return dict(self.events)
        chunk = self.stream.read()
        if not chunk:
            return dict(self.events)
        lines = (self.partial + chunk).splitlines(keepends=True)
        self.partial = ""
        if lines and not lines[-1].endswith("\n"):
            self.partial = lines.pop()
        if lines:
            parsed = parse_startup_trace(
                "".join(lines),
                nonce=self.nonce,
                pid=self.pid,
                frequency=self.frequency,
            )
            duplicate = self.events.keys() & parsed.keys()
            if duplicate:
                raise ValueError(f"duplicate startup event: {min(duplicate)}")
            self.events.update(parsed)
        return dict(self.events)

    def close(self) -> None:
        if self.stream is not None:
            self.stream.close()
            self.stream = None


def trace_milestones(
    events: dict[str, StartupTraceEvent],
    *,
    start_counter: int,
    frequency: int,
) -> dict[str, float]:
    """Convert a complete ordered trace to milliseconds from harness launch."""
    missing = [name for name in STARTUP_TRACE_EVENTS if name not in events]
    if missing:
        raise ValueError(f"missing startup event(s): {', '.join(missing)}")
    counters = [events[name].counter for name in STARTUP_TRACE_EVENTS]
    if counters != sorted(counters):
        raise ValueError("startup event order is invalid")
    if counters[0] < start_counter:
        raise ValueError("startup event precedes the harness launch counter")
    return {
        f"{name}_ms": (events[name].counter - start_counter) / frequency * 1000
        for name in STARTUP_TRACE_EVENTS
    }


T = TypeVar("T")


def measure_startup_abba(
    rounds: int,
    measure_a: Callable[[], T],
    measure_b: Callable[[], T],
) -> tuple[tuple[T, ...], tuple[T, ...]]:
    """Collect structured samples in strict A-B-B-A order."""
    if rounds < 1:
        raise ValueError("rounds must be at least 1")
    samples_a: list[T] = []
    samples_b: list[T] = []
    for _ in range(rounds):
        samples_a.append(measure_a())
        samples_b.append(measure_b())
        samples_b.append(measure_b())
        samples_a.append(measure_a())
    return tuple(samples_a), tuple(samples_b)


def quiet_gate_failures(
    cpu: list[float],
    disk: list[float],
    max_cpu_median: float,
    max_cpu_p95: float,
    max_disk_median: float,
    max_disk_p95: float,
) -> list[str]:
    checks = [
        ("CPU median", median(cpu), max_cpu_median),
        ("CPU p95", nearest_rank_percentile(cpu, 0.95), max_cpu_p95),
        ("disk median", median(disk), max_disk_median),
        ("disk p95", nearest_rank_percentile(disk, 0.95), max_disk_p95),
    ]
    return [f"{name} {value:.2f}% > {limit:.2f}%" for name, value, limit in checks
            if value > limit]


def summarize_startup_milestones(label: str, samples: tuple[StartupSample, ...]) -> None:
    print(f"{label}: {len(samples)} milestone samples")
    for index, sample in enumerate(samples, start=1):
        print(f"  sample {index:>2}: {json.dumps(sample.evidence(), sort_keys=True)}")
    for field in STARTUP_MILESTONE_FIELDS:
        values = [float(getattr(sample, field)) for sample in samples]
        p95 = values[0] if len(values) == 1 else inclusive_p95(values)
        print(
            f"  {field.removesuffix('_ms'):<27} median {median(values):7.1f} ms  "
            f"p95 {p95:7.1f} ms"
        )
    for field in STARTUP_IDLE_FIELDS:
        values = [float(getattr(sample, field)) for sample in samples]
        unit = "MB" if field.endswith("_mb") else ""
        p95 = values[0] if len(values) == 1 else inclusive_p95(values)
        print(
            f"  {field:<27} median {median(values):7.1f} {unit:<2}  "
            f"p95 {p95:7.1f} {unit}"
        )


def milestone_comparison(
    samples_a: tuple[StartupSample, ...],
    samples_b: tuple[StartupSample, ...],
) -> dict[str, dict[str, object]]:
    """Summarize structured A-B-B-A pairs for every named milestone."""
    if len(samples_a) != len(samples_b) or len(samples_a) % 2:
        raise ValueError("startup A-B-B-A samples must have matching even lengths")
    result: dict[str, dict[str, object]] = {}
    for field in STARTUP_COMPARISON_FIELDS:
        values_a = [float(getattr(sample, field)) for sample in samples_a]
        values_b = [float(getattr(sample, field)) for sample in samples_b]
        paired_a = [sum(values_a[ix : ix + 2]) / 2 for ix in range(0, len(values_a), 2)]
        paired_b = [sum(values_b[ix : ix + 2]) / 2 for ix in range(0, len(values_b), 2)]
        deltas = [b - a for a, b in zip(paired_a, paired_b, strict=True)]
        percentages = [delta / a * 100 for a, delta in zip(paired_a, deltas, strict=True)]
        result[field] = {
            "paired_a": paired_a,
            "paired_b": paired_b,
            "b_minus_a": deltas,
            "b_minus_a_percent": percentages,
            "median_b_minus_a": median(deltas),
            "median_b_minus_a_percent": median(percentages),
        }
    return result


def startup_summary(samples: tuple[StartupSample, ...]) -> dict[str, dict[str, float]]:
    result: dict[str, dict[str, float]] = {}
    for field in STARTUP_COMPARISON_FIELDS:
        values = [float(getattr(sample, field)) for sample in samples]
        result[field] = {
            "median": median(values),
            "p95": inclusive_p95(values),
        }
    return result


def evaluate_model_threshold(
    full: tuple[StartupSample, ...],
    no_model: tuple[StartupSample, ...],
    threshold: dict[str, object],
) -> dict[str, object]:
    record = threshold["record"]
    assert isinstance(record, dict)
    materiality = record["materiality"]
    assert isinstance(materiality, dict)
    launch_rule = materiality["launch"]
    idle_rule = materiality["idle_private"]
    assert isinstance(launch_rule, dict)
    assert isinstance(idle_rule, dict)
    comparison = milestone_comparison(full, no_model)

    launch: dict[str, object] = {}
    launch_met = False
    for field in launch_rule["milestones"]:
        summary = comparison[field]
        full_median = median(summary["paired_a"])
        no_model_median = median(summary["paired_b"])
        improvement = full_median - no_model_median
        relative = improvement / full_median * 100
        full_p95 = inclusive_p95([float(getattr(sample, field)) for sample in full])
        no_model_p95 = inclusive_p95(
            [float(getattr(sample, field)) for sample in no_model]
        )
        p95_regression = (no_model_p95 - full_p95) / full_p95 * 100
        met = (
            improvement >= launch_rule["required_absolute_improvement_ms"]
            and relative >= launch_rule["required_relative_improvement_percent"]
            and p95_regression <= launch_rule["max_p95_regression_percent"]
        )
        launch[field] = {
            "full_paired_median_ms": full_median,
            "no_model_paired_median_ms": no_model_median,
            "improvement_ms": improvement,
            "improvement_percent": relative,
            "full_raw_p95_ms": full_p95,
            "no_model_raw_p95_ms": no_model_p95,
            "p95_regression_percent": p95_regression,
            "material": met,
        }
        launch_met = launch_met or met

    idle_summary = comparison["idle_private_mb"]
    full_idle = median(idle_summary["paired_a"])
    no_model_idle = median(idle_summary["paired_b"])
    idle_improvement = full_idle - no_model_idle
    idle_relative = idle_improvement / full_idle * 100
    idle_met = (
        idle_improvement >= idle_rule["required_absolute_improvement_mb"]
        and idle_relative >= idle_rule["required_relative_improvement_percent"]
    )
    material = launch_met or idle_met
    return {
        "statistics": {
            "median": "median of per-round A-B-B-A pair means",
            "p95": "inclusive p95 of raw samples",
            "direction": "positive values favor no-model over full",
        },
        "launch": launch,
        "idle_private": {
            "full_paired_median_mb": full_idle,
            "no_model_paired_median_mb": no_model_idle,
            "improvement_mb": idle_improvement,
            "improvement_percent": idle_relative,
            "material": idle_met,
        },
        "materiality_met": material,
        "automatic_result": None if material else record["fallback_decision"],
    }


def startup_sample_from_evidence(value: object) -> StartupSample:
    if not isinstance(value, dict) or set(value) != {
        "process_created_ms",
        "process_started_ms",
        "initial_state_ready_ms",
        "window_visible_ms",
        "first_frame_painted_ms",
        "first_input_handled_ms",
        "initial_state",
        "idle_working_set_mb",
        "idle_private_mb",
        "peak_working_set_mb",
        "page_faults",
        "threads",
    }:
        raise ValueError("startup evidence contains an invalid sample shape")
    for field in STARTUP_MILESTONE_FIELDS + (
        "idle_working_set_mb",
        "idle_private_mb",
        "peak_working_set_mb",
    ):
        if not finite_number(value[field]) or value[field] <= 0:
            raise ValueError("startup evidence contains an invalid sample value")
    for field in ("page_faults", "threads"):
        if (
            not isinstance(value[field], int)
            or isinstance(value[field], bool)
            or value[field] < 0
        ):
            raise ValueError("startup evidence contains an invalid sample value")
    if value["threads"] == 0 or value["peak_working_set_mb"] < value["idle_working_set_mb"]:
        raise ValueError("startup evidence contains an invalid idle sample")
    state = value["initial_state"]
    if state not in {"welcome", "workspace", "bare"}:
        raise ValueError("startup evidence contains an invalid initial state")
    ordered = [
        value["process_started_ms"],
        value["initial_state_ready_ms"],
        value["first_frame_painted_ms"],
        value["first_input_handled_ms"],
    ]
    if ordered != sorted(ordered):
        raise ValueError("startup evidence milestone order is invalid")
    return StartupSample(**value)


def _git_value(*args: str) -> str:
    result = subprocess.run(
        ["git", *args], cwd=REPO, capture_output=True, text=True, check=True
    )
    return result.stdout.strip()


def source_state() -> dict[str, object]:
    digest = hashlib.sha256()
    diff = subprocess.run(
        ["git", "diff", "--binary", "HEAD"],
        cwd=REPO,
        capture_output=True,
        check=True,
    ).stdout
    digest.update(diff)
    untracked = subprocess.run(
        ["git", "ls-files", "--others", "--exclude-standard", "-z"],
        cwd=REPO,
        capture_output=True,
        check=True,
    ).stdout.split(b"\0")
    untracked = sorted(path for path in untracked if path)
    for raw_path in untracked:
        path = REPO / os.fsdecode(raw_path)
        digest.update(raw_path)
        digest.update(b"\0")
        digest.update(path.read_bytes())
    dirty = bool(diff or untracked)
    return {
        "revision": _git_value("rev-parse", "HEAD"),
        "dirty": dirty,
        "worktree_sha256": digest.hexdigest() if dirty else None,
    }


def read_evidence_object(path: Path) -> dict[str, object]:
    try:
        if path.stat().st_size > MAX_INPUT_EVIDENCE_BYTES:
            raise ValueError("evidence file exceeds the 1 MiB limit")
        value = json.loads(path.read_text(encoding="utf-8"))
    except OSError as error:
        raise ValueError("could not read evidence file") from error
    except json.JSONDecodeError as error:
        raise ValueError("evidence file is not valid JSON") from error
    if not isinstance(value, dict):
        raise ValueError("evidence root must be a JSON object")
    return value


def require_distinct_output_path(output: Path, *inputs: Path | None) -> None:
    output = output.resolve()
    if any(path is not None and path.resolve() == output for path in inputs):
        raise ValueError("evidence output path must differ from every input and executable")


def validate_source_state(value: object) -> None:
    if not isinstance(value, dict) or set(value) != {
        "revision",
        "dirty",
        "worktree_sha256",
    }:
        raise ValueError("invalid source state")
    revision = value.get("revision")
    dirty = value.get("dirty")
    worktree = value.get("worktree_sha256")
    if (
        not isinstance(revision, str)
        or re.fullmatch(r"[0-9a-f]{40}", revision) is None
        or not isinstance(dirty, bool)
        or (dirty and (not isinstance(worktree, str) or SHA256_PATTERN.fullmatch(worktree) is None))
        or (not dirty and worktree is not None)
    ):
        raise ValueError("invalid source state")


def validate_fingerprint(value: object, field: str = "executable") -> None:
    if not isinstance(value, dict) or set(value) != {"byte_count", "sha256"}:
        raise ValueError(f"invalid {field} fingerprint")
    byte_count = value.get("byte_count")
    sha256 = value.get("sha256")
    if (
        not isinstance(byte_count, int)
        or isinstance(byte_count, bool)
        or byte_count <= 0
        or not isinstance(sha256, str)
        or SHA256_PATTERN.fullmatch(sha256) is None
    ):
        raise ValueError(f"invalid {field} fingerprint")

def validate_pe_sections(value: object) -> None:
    if not isinstance(value, list) or not 1 <= len(value) <= 96:
        raise ValueError("invalid PE section evidence")
    for section in value:
        if (
            not isinstance(section, dict)
            or set(section) != {"name", "virtual_size", "raw_size", "characteristics"}
            or not isinstance(section.get("name"), str)
            or re.fullmatch(r"[.A-Za-z0-9_$-]{1,8}", section["name"]) is None
            or any(
                not isinstance(section.get(field), int)
                or isinstance(section.get(field), bool)
                or section[field] < 0
                for field in ("virtual_size", "raw_size", "characteristics")
            )
        ):
            raise ValueError("invalid PE section evidence")

def goal04_build_command(variant: Goal04BuildVariant) -> list[str]:
    if variant.artifact_kind == "application":
        command = [
            "cargo",
            "build",
            "--release",
            "--locked",
            "--target",
            GOAL04_TARGET,
            "-p",
            "mt-app",
            "--bin",
            variant.target_name,
        ]
    else:
        command = [
            "cargo",
            "test",
            "--release",
            "--locked",
            "--target",
            GOAL04_TARGET,
            "-p",
            "mt-app",
            "--test",
            variant.target_name,
            "--no-run",
            "--message-format=json-render-diagnostics",
        ]
    if variant.no_default_features:
        command.append("--no-default-features")
    return command


def goal04_behavior_verification_command(variant_name: str) -> list[str] | None:
    if variant_name != "no-model":
        return None
    return [
        "cargo",
        "test",
        "--release",
        "--locked",
        "--target",
        GOAL04_TARGET,
        "-p",
        "mt-app",
        "--lib",
        "--no-default-features",
        "translate::ablation_tests::measurement_build_reports_the_removed_transport",
        "--",
        "--exact",
    ]


def goal04_bloat_command(variant: Goal04BuildVariant) -> list[str]:
    command = [
        "cargo",
        "bloat",
        "--release",
        "--locked",
        "--target",
        GOAL04_TARGET,
        "-p",
        "mt-app",
        "--bin",
        variant.target_name,
        "--crates",
        "-n",
        "0",
        "--message-format",
        "json",
    ]
    if variant.no_default_features:
        command.append("--no-default-features")
    return command


def goal04_tree_command(variant: Goal04BuildVariant) -> list[str]:
    command = [
        "cargo",
        "tree",
        "--locked",
        "--target",
        GOAL04_TARGET,
        "-p",
        "mt-app",
        "--edges",
        "normal",
        "--prefix",
        "none",
        "--format",
        "{p}|{f}",
    ]
    if variant.no_default_features:
        command.append("--no-default-features")
    return command


def goal04_cargo_config_context(env: dict[str, str]) -> dict[str, object]:
    candidates: list[tuple[str, Path]] = []
    roots = [REPO, *REPO.parents]
    for index, root in enumerate(roots):
        scope = "workspace" if index == 0 else f"ancestor-{index}"
        for name in ("config.toml", "config"):
            candidates.append((scope, root / ".cargo" / name))
    cargo_home = Path(env.get("CARGO_HOME", Path.home() / ".cargo"))
    for name in ("config.toml", "config"):
        candidates.append(("cargo-home", cargo_home / name))

    files: list[dict[str, object]] = []
    seen: set[Path] = set()
    for scope, path in candidates:
        try:
            resolved = path.resolve()
            if resolved in seen or not resolved.is_file():
                continue
            data = resolved.read_bytes()
        except OSError as error:
            raise ValueError("could not fingerprint Cargo configuration") from error
        seen.add(resolved)
        files.append(
            {
                "scope": scope,
                "name": path.name,
                "byte_count": len(data),
                "sha256": hashlib.sha256(data).hexdigest(),
            }
        )
    files.sort(key=lambda item: (str(item["scope"]), str(item["name"])))
    return {
        "cargo_home_overridden": "CARGO_HOME" in env,
        "files": files,
    }


def goal04_toolchain_context(cargo: str, env: dict[str, str]) -> dict[str, object]:
    cargo_path = Path(cargo)
    rustc_name = "rustc.exe" if sys.platform == "win32" else "rustc"
    rustc_path = cargo_path.with_name(rustc_name)
    if not rustc_path.is_file():
        discovered = shutil.which("rustc", path=env.get("PATH"))
        if discovered is None:
            raise ValueError("rustc not found")
        rustc_path = Path(discovered)

    def version_output(executable: Path, arguments: list[str]) -> str:
        try:
            result = subprocess.run(
                [str(executable), *arguments],
                cwd=REPO,
                env=env,
                capture_output=True,
                text=True,
                encoding="utf-8",
                errors="replace",
            )
        except OSError as error:
            raise ValueError("could not execute the Rust toolchain") from error
        if result.returncode:
            raise ValueError("Rust toolchain version output is invalid")
        return result.stdout

    return {
        "cargo": parse_goal04_cargo_version(version_output(cargo_path, ["-Vv"])),
        "rustc": parse_goal04_rustc_version(version_output(rustc_path, ["-vV"])),
        "cargo_config": goal04_cargo_config_context(env),
    }


def parse_goal04_cargo_version(output: str) -> dict[str, str | list[str]]:
    lines = [line.strip() for line in output.splitlines() if line.strip()]
    if len(lines) != 8:
        raise ValueError("Cargo version output is invalid")
    headline = re.fullmatch(
        r"cargo ([0-9]+\.[0-9]+\.[0-9]+(?:-[A-Za-z0-9.-]+)?) "
        r"\(([0-9a-f]{7,40}) ([0-9]{4}-[0-9]{2}-[0-9]{2})\)",
        lines[0],
    )
    fields: dict[str, str] = {}
    for line, name, pattern in (
        (lines[1], "release", r"[0-9]+\.[0-9]+\.[0-9]+(?:-[A-Za-z0-9.-]+)?"),
        (lines[2], "commit_hash", r"[0-9a-f]{40}"),
        (lines[3], "commit_date", r"[0-9]{4}-[0-9]{2}-[0-9]{2}"),
        (lines[4], "host", re.escape(GOAL04_TARGET)),
    ):
        match = re.fullmatch(rf"{name.replace('_', '-')}: ({pattern})", line)
        if match is None:
            raise ValueError("Cargo version output is invalid")
        fields[name] = match.group(1)
    if (
        headline is None
        or headline.group(1) != fields["release"]
        or not fields["commit_hash"].startswith(headline.group(2))
        or headline.group(3) != fields["commit_date"]
        or re.fullmatch(r"libgit2: [A-Za-z0-9 .():+\-]+", lines[5]) is None
        or re.fullmatch(r"libcurl: [A-Za-z0-9 .():+\-\[\]]+", lines[6]) is None
        or re.fullmatch(r"os: [A-Za-z0-9 .()\[\]\-]+", lines[7]) is None
    ):
        raise ValueError("Cargo version output is invalid")
    return {"command": ["cargo", "-Vv"], **fields}


def parse_goal04_rustc_version(output: str) -> dict[str, str | list[str]]:
    lines = [line.strip() for line in output.splitlines() if line.strip()]
    if len(lines) != 7:
        raise ValueError("rustc version output is invalid")
    headline = re.fullmatch(
        r"rustc ([0-9]+\.[0-9]+\.[0-9]+(?:-[A-Za-z0-9.-]+)?) "
        r"\(([0-9a-f]{7,40}) ([0-9]{4}-[0-9]{2}-[0-9]{2})\)",
        lines[0],
    )
    if lines[1] != "binary: rustc":
        raise ValueError("rustc version output is invalid")
    fields: dict[str, str] = {}
    for line, label, name, pattern in (
        (lines[2], "commit-hash", "commit_hash", r"[0-9a-f]{40}"),
        (lines[3], "commit-date", "commit_date", r"[0-9]{4}-[0-9]{2}-[0-9]{2}"),
        (lines[4], "host", "host", re.escape(GOAL04_TARGET)),
        (lines[5], "release", "release", r"[0-9]+\.[0-9]+\.[0-9]+(?:-[A-Za-z0-9.-]+)?"),
        (lines[6], "LLVM version", "llvm_version", r"[0-9]+(?:\.[0-9]+)+"),
    ):
        match = re.fullmatch(rf"{label}: ({pattern})", line)
        if match is None:
            raise ValueError("rustc version output is invalid")
        fields[name] = match.group(1)
    if (
        headline is None
        or headline.group(1) != fields["release"]
        or not fields["commit_hash"].startswith(headline.group(2))
        or headline.group(3) != fields["commit_date"]
    ):
        raise ValueError("rustc version output is invalid")
    return {"command": ["rustc", "-vV"], **fields}


def goal04_build_environment(target_dir: Path, variant_name: str) -> dict[str, str]:
    env = os.environ.copy()
    for name in tuple(env):
        if name.startswith(
            ("CARGO_BUILD_", "CARGO_PROFILE_", "CARGO_TARGET_")
        ) or name in {
            "CARGO_ENCODED_RUSTFLAGS",
            "RUSTC",
            "RUSTC_BOOTSTRAP",
            "RUSTDOC",
            "RUSTC_WRAPPER",
            "RUSTC_WORKSPACE_WRAPPER",
            "RUSTDOCFLAGS",
            "RUSTFLAGS",
            "RUSTUP_TOOLCHAIN",
        }:
            env.pop(name, None)
    env["CARGO_TARGET_DIR"] = str(target_dir)
    env["CARGO_INCREMENTAL"] = "0"
    if variant_name in {"opt-3", "opt-s"}:
        env["CARGO_PROFILE_RELEASE_OPT_LEVEL"] = GOAL04_BUILD_VARIANTS[
            variant_name
        ].opt_level
    return env


def parse_goal04_dependency_graph(
    output: str,
) -> tuple[list[str], dict[str, list[str]]]:
    packages_set: set[str] = set()
    feature_sets: dict[str, set[str]] = {}
    for line in output.splitlines():
        if not line.strip():
            continue
        package, feature_text = line.rsplit("|", 1)
        name = package.split(maxsplit=1)[0]
        packages_set.add(name)
        feature_text = feature_text.removesuffix(" (*)")
        feature_sets.setdefault(name, set()).update(
            feature.strip()
            for feature in feature_text.split(",")
            if feature.strip()
        )
    packages = sorted(packages_set)
    selected_features = {
        name: sorted(feature_sets.get(name, set()))
        for name in MODEL_ATTRIBUTION_PACKAGES
        if name in packages_set
    }
    return packages, selected_features


def goal04_release_profile(variant: Goal04BuildVariant) -> dict[str, object]:
    manifest = tomllib.loads((REPO / "Cargo.toml").read_text(encoding="utf-8"))
    release = manifest.get("profile", {}).get("release", {})
    return {
        "name": "release",
        "opt_level": variant.opt_level,
        "codegen_units": release.get("codegen-units", 16),
        "lto": release.get("lto", False),
        "strip": release.get("strip", False),
        "panic": release.get("panic", "unwind"),
    }


def goal04_behavior(variant_name: str) -> dict[str, object]:
    if variant_name == "no-model":
        return {
            "translation_scopes": ["selection", "block", "document"],
            "model_transport": "compile-time removed",
            "lost_behavior": (
                "provider-backed translation for every scope; invocation returns the "
                "Goal 04 measurement-build unavailable diagnostic"
            ),
        }
    if variant_name == "bare":
        return {
            "translation_scopes": [],
            "model_transport": "not part of the bare shell",
            "lost_behavior": "all product behavior; this variant is a GPUI platform diagnostic",
        }
    return {
        "translation_scopes": ["selection", "block", "document"],
        "model_transport": "compiled in and available when configured",
        "lost_behavior": None,
    }


def goal04_platform_setup(variant: Goal04BuildVariant) -> dict[str, object] | None:
    if variant.artifact_kind != "application":
        return None
    return {
        "app_identity": "io.github.wxxb789.markturbo",
        "assets": "embedded Assets provider",
        "direct_composition": "disabled on Windows",
        "gpui_component": "initialized",
        "window": "same title, bounds, minimum size, kind and TitleBar options",
    }


def goal04_tokio_disposition(variant_name: str) -> str:
    if variant_name == "full":
        return "present with the unified application and model-transport feature set"
    if variant_name == "no-model":
        return (
            "may remain through non-model dependencies; this ablation claims removal only "
            "of model-added features and the genai/reqwest/rustls closure"
        )
    return "recorded as diagnostic dependency context"


def validate_bloat_evidence(
    value: object, variant_name: str, dependency_packages: set[str]
) -> None:
    if variant_name not in {"full", "no-model"}:
        if value is not None:
            raise ValueError("unexpected cargo-bloat evidence")
        return
    if not isinstance(value, dict) or set(value) != {
        "version",
        "command",
        "file_size",
        "text_section_size",
        "crates",
    }:
        raise ValueError("invalid cargo-bloat evidence")
    if not safe_metadata_text(value.get("version")):
        raise ValueError("invalid cargo-bloat version")
    variant = GOAL04_BUILD_VARIANTS[variant_name]
    if value.get("command") != goal04_bloat_command(variant):
        raise ValueError("invalid cargo-bloat command")
    if any(
        not isinstance(value.get(field), int)
        or isinstance(value.get(field), bool)
        or value[field] <= 0
        for field in ("file_size", "text_section_size")
    ):
        raise ValueError("invalid cargo-bloat sizes")
    crates = value.get("crates")
    if not isinstance(crates, list) or not crates:
        raise ValueError("invalid cargo-bloat crate list")
    for crate in crates:
        name = crate.get("name") if isinstance(crate, dict) else None
        if (
            not isinstance(crate, dict)
            or set(crate) != {"name", "size"}
            or not isinstance(name, str)
            or (
                name not in {"[Unknown]", "[Other MSVC attribution]"}
                and (
                    re.fullmatch(r"[A-Za-z0-9_.?-]+", name) is None
                    or name.removesuffix("?") not in dependency_packages
                )
            )
            or not isinstance(crate.get("size"), int)
            or isinstance(crate.get("size"), bool)
            or crate["size"] < 0
        ):
            raise ValueError("invalid cargo-bloat crate record")
    crate_names = {crate["name"].removesuffix("?") for crate in crates}
    model_crates = {"genai", "reqwest", "rustls"}
    if variant_name == "full" and not {"genai", "reqwest"}.issubset(crate_names):
        raise ValueError("full cargo-bloat evidence is missing model transport")
    if variant_name == "no-model" and model_crates.intersection(crate_names):
        raise ValueError("no-model cargo-bloat evidence still contains model transport")


def normalize_goal04_bloat_crates(
    value: object, dependency_packages: set[str]
) -> list[dict[str, object]]:
    if not isinstance(value, list):
        raise ValueError("cargo-bloat crate list is invalid")
    aliases: dict[str, str | None] = {}
    for package in dependency_packages:
        alias = package.replace("-", "_")
        aliases[alias] = package if alias not in aliases else None
    totals: dict[str, int] = {}
    for item in value:
        if (
            not isinstance(item, dict)
            or set(item) != {"name", "size"}
            or not isinstance(item.get("name"), str)
            or not isinstance(item.get("size"), int)
            or isinstance(item.get("size"), bool)
            or item["size"] < 0
        ):
            raise ValueError("cargo-bloat crate list is invalid")
        raw_name = item["name"]
        if raw_name in {"", "[Unknown]"}:
            name = "[Unknown]"
        else:
            optional = raw_name.endswith("?")
            package = aliases.get(raw_name.removesuffix("?"))
            name = (
                f"{package}?"
                if optional and package is not None
                else package or "[Other MSVC attribution]"
            )
        totals[name] = totals.get(name, 0) + item["size"]
    return [
        {"name": name, "size": size}
        for name, size in sorted(totals.items(), key=lambda item: (-item[1], item[0]))
    ]


def validate_dependency_graph(value: object, variant_name: str) -> None:
    if not isinstance(value, dict) or set(value) != {
        "command",
        "packages",
        "selected_features",
        "tokio_disposition",
    }:
        raise ValueError("invalid dependency graph evidence")
    variant = GOAL04_BUILD_VARIANTS[variant_name]
    if value.get("command") != goal04_tree_command(variant):
        raise ValueError("invalid dependency graph command")
    packages = value.get("packages")
    if (
        not isinstance(packages, list)
        or not all(
            isinstance(package, str)
            and re.fullmatch(r"[A-Za-z0-9_.-]+", package) is not None
            for package in packages
        )
    ):
        raise ValueError("invalid dependency graph package list")
    if packages != sorted(set(packages)):
        raise ValueError("invalid dependency graph package list")
    selected = value.get("selected_features")
    if not isinstance(selected, dict):
        raise ValueError("invalid dependency graph feature attribution")
    for name, features in selected.items():
        if (
            name not in MODEL_ATTRIBUTION_PACKAGES
            or not isinstance(features, list)
            or not all(
                isinstance(feature, str)
                and re.fullmatch(r"[A-Za-z0-9_.-]+", feature) is not None
                for feature in features
            )
            or features != sorted(set(features))
        ):
            raise ValueError("invalid dependency graph feature attribution")
    expected_selected = set(packages).intersection(MODEL_ATTRIBUTION_PACKAGES)
    if set(selected) != expected_selected:
        raise ValueError("dependency graph feature attribution is incomplete")
    if value.get("tokio_disposition") != goal04_tokio_disposition(variant_name):
        raise ValueError("dependency graph Tokio disposition is invalid")
    model_packages = {"genai", "reqwest", "rustls"}
    if variant_name == "full" and not model_packages.issubset(packages):
        raise ValueError("full build dependency graph is missing model transport")
    if variant_name == "no-model" and model_packages.intersection(packages):
        raise ValueError("no-model dependency graph still contains model transport")


def canonical_build_evidence(evidence: dict[str, object]) -> dict[str, object]:
    validate_build_evidence(evidence)
    return {
        "schema": STARTUP_BUILD_SCHEMA,
        "created_at": evidence["created_at"],
        "variant": evidence["variant"],
        "role": evidence["role"],
        "artifact_kind": evidence["artifact_kind"],
        "target_name": evidence["target_name"],
        "source": evidence["source"],
        "cargo_lock": evidence["cargo_lock"],
        "target": evidence["target"],
        "profile": evidence["profile"],
        "toolchain": evidence["toolchain"],
        "features": evidence["features"],
        "behavior": evidence["behavior"],
        "behavior_verification": evidence["behavior_verification"],
        "platform_setup": evidence["platform_setup"],
        "build": evidence["build"],
        "dependency_graph": evidence["dependency_graph"],
        "cargo_bloat": evidence["cargo_bloat"],
        "executable": evidence["executable"],
        "pe_sections": evidence["pe_sections"],
    }


def validate_build_evidence(evidence: dict[str, object]) -> None:
    if evidence.get("schema") != STARTUP_BUILD_SCHEMA:
        raise ValueError("invalid Goal 04 build evidence schema")
    parse_evidence_time(evidence.get("created_at"), "build evidence created_at")
    variant_name = evidence.get("variant")
    if not isinstance(variant_name, str):
        raise ValueError("unknown Goal 04 build variant")
    variant = GOAL04_BUILD_VARIANTS.get(variant_name)
    if variant is None:
        raise ValueError("unknown Goal 04 build variant")
    if (
        evidence.get("role") != variant.role
        or evidence.get("artifact_kind") != variant.artifact_kind
        or evidence.get("target_name") != variant.target_name
    ):
        raise ValueError("build evidence variant contract does not match")
    validate_source_state(evidence.get("source"))
    validate_fingerprint(evidence.get("cargo_lock"), "Cargo.lock")
    validate_fingerprint(evidence.get("executable"))
    validate_pe_sections(evidence.get("pe_sections"))
    if evidence.get("target") != GOAL04_TARGET or rust_target() != GOAL04_TARGET:
        raise ValueError("build evidence target does not match this toolchain")
    if evidence.get("profile") != goal04_release_profile(variant):
        raise ValueError("build evidence release profile does not match")
    validate_goal04_toolchain_context(evidence.get("toolchain"))
    features = evidence.get("features")
    if features != {
        "default_features": not variant.no_default_features,
        "model_transport": variant.model_transport,
    }:
        raise ValueError("build evidence feature set does not match")
    if evidence.get("behavior") != goal04_behavior(variant_name):
        raise ValueError("build evidence behavior contract does not match")
    verification_command = goal04_behavior_verification_command(variant_name)
    expected_verification = (
        None
        if verification_command is None
        else {"command": verification_command, "passed": 1, "failed": 0}
    )
    if evidence.get("behavior_verification") != expected_verification:
        raise ValueError("build evidence behavior verification does not match")
    if evidence.get("platform_setup") != goal04_platform_setup(variant):
        raise ValueError("build evidence platform setup does not match")
    if evidence.get("build") != {
        "command": goal04_build_command(variant),
        "environment": {
            "CARGO_TARGET_DIR": "<redacted-path>",
            "CARGO_INCREMENTAL": "0",
            "CARGO_PROFILE_RELEASE_OPT_LEVEL": (
                variant.opt_level if variant_name in {"opt-3", "opt-s"} else None
            ),
        },
    }:
        raise ValueError("build evidence command does not match")
    dependency_graph = evidence.get("dependency_graph")
    validate_dependency_graph(dependency_graph, variant_name)
    assert isinstance(dependency_graph, dict)
    packages = dependency_graph["packages"]
    assert isinstance(packages, list)
    validate_bloat_evidence(evidence.get("cargo_bloat"), variant_name, set(packages))
    if isinstance(evidence.get("cargo_bloat"), dict):
        bloat = evidence["cargo_bloat"]
        executable = evidence["executable"]
        assert isinstance(bloat, dict)
        assert isinstance(executable, dict)
        if (
            bloat["file_size"] != executable["byte_count"]
            or bloat["text_section_size"] > bloat["file_size"]
        ):
            raise ValueError("cargo-bloat sizes do not match the executable")


def validate_goal04_toolchain_context(value: object) -> None:
    if not isinstance(value, dict) or set(value) != {"cargo", "rustc", "cargo_config"}:
        raise ValueError("build evidence toolchain is invalid")
    cargo = value.get("cargo")
    rustc = value.get("rustc")
    if (
        not isinstance(cargo, dict)
        or set(cargo) != {"command", "release", "commit_hash", "commit_date", "host"}
        or cargo.get("command") != ["cargo", "-Vv"]
        or not isinstance(rustc, dict)
        or set(rustc)
        != {
            "command",
            "release",
            "commit_hash",
            "commit_date",
            "host",
            "llvm_version",
        }
        or rustc.get("command") != ["rustc", "-vV"]
    ):
        raise ValueError("build evidence toolchain is invalid")
    for record in (cargo, rustc):
        if (
            not isinstance(record.get("release"), str)
            or re.fullmatch(
                r"[0-9]+\.[0-9]+\.[0-9]+(?:-[A-Za-z0-9.-]+)?",
                record["release"],
            )
            is None
            or not isinstance(record.get("commit_hash"), str)
            or re.fullmatch(r"[0-9a-f]{40}", record["commit_hash"]) is None
            or not isinstance(record.get("commit_date"), str)
            or re.fullmatch(r"[0-9]{4}-[0-9]{2}-[0-9]{2}", record["commit_date"])
            is None
            or record.get("host") != GOAL04_TARGET
        ):
            raise ValueError("build evidence toolchain is invalid")
    llvm_version = rustc.get("llvm_version")
    if not isinstance(llvm_version, str) or re.fullmatch(
        r"[0-9]+(?:\.[0-9]+)+", llvm_version
    ) is None:
        raise ValueError("build evidence toolchain is invalid")
    config = value.get("cargo_config")
    if (
        not isinstance(config, dict)
        or set(config) != {"cargo_home_overridden", "files"}
        or not isinstance(config.get("cargo_home_overridden"), bool)
        or not isinstance(config.get("files"), list)
    ):
        raise ValueError("build evidence Cargo configuration is invalid")
    files = config["files"]
    seen: set[tuple[str, str]] = set()
    for record in files:
        if not isinstance(record, dict) or set(record) != {
            "scope",
            "name",
            "byte_count",
            "sha256",
        }:
            raise ValueError("build evidence Cargo configuration is invalid")
        scope = record.get("scope")
        name = record.get("name")
        byte_count = record.get("byte_count")
        sha256 = record.get("sha256")
        if (
            not isinstance(scope, str)
            or re.fullmatch(r"(?:workspace|cargo-home|ancestor-[1-9][0-9]*)", scope) is None
            or name not in {"config", "config.toml"}
            or not isinstance(byte_count, int)
            or isinstance(byte_count, bool)
            or byte_count < 0
            or not isinstance(sha256, str)
            or SHA256_PATTERN.fullmatch(sha256) is None
            or (scope, name) in seen
        ):
            raise ValueError("build evidence Cargo configuration is invalid")
        seen.add((scope, name))
    if files != sorted(files, key=lambda item: (str(item["scope"]), str(item["name"]))):
        raise ValueError("build evidence Cargo configuration is not canonical")


def load_build_evidence(
    path: Path,
    *,
    variant_name: str,
    source: dict[str, object],
    executable: Path,
) -> dict[str, object]:
    from .native.runtime import sha256_file

    evidence = read_evidence_object(path)
    normalized = canonical_build_evidence(evidence)
    if normalized["variant"] != variant_name:
        raise ValueError("build evidence variant does not match its label")
    if normalized["source"] != source:
        raise ValueError("build evidence was captured from a different source state")
    lock = sha256_file(REPO / "Cargo.lock").evidence()
    if normalized["cargo_lock"] != lock:
        raise ValueError("build evidence Cargo.lock does not match")
    fingerprint = sha256_file(executable).evidence()
    if normalized["executable"] != fingerprint:
        raise ValueError("build evidence executable fingerprint does not match")
    return normalized


def validate_threshold_evidence(evidence: dict[str, object]) -> None:
    if evidence.get("schema") != STARTUP_THRESHOLD_SCHEMA:
        raise ValueError("invalid Goal 04 threshold evidence schema")
    parse_evidence_time(evidence.get("created_at"), "threshold evidence created_at")
    if evidence.get("status") != "APPROVED" or evidence.get("approved_by") != "project-owner":
        raise ValueError("model-transport threshold is not owner-approved")
    if evidence.get("scope") != "model-transport":
        raise ValueError("threshold evidence has the wrong decision scope")
    validate_source_state(evidence.get("source"))
    materiality = evidence.get("materiality")
    if not isinstance(materiality, dict):
        raise ValueError("threshold evidence is missing materiality rules")
    launch = materiality.get("launch")
    idle = materiality.get("idle_private")
    if not isinstance(launch, dict) or not isinstance(idle, dict):
        raise ValueError("threshold evidence is missing launch or idle rules")
    if launch.get("milestones") != [
        "first_frame_painted_ms",
        "first_input_handled_ms",
    ]:
        raise ValueError("threshold evidence launch milestones do not match Goal 04")
    positive = (
        launch.get("required_absolute_improvement_ms"),
        launch.get("required_relative_improvement_percent"),
        idle.get("required_absolute_improvement_mb"),
        idle.get("required_relative_improvement_percent"),
    )
    if not all(finite_number(value) and value > 0 for value in positive):
        raise ValueError("threshold evidence improvements must be positive")
    p95 = launch.get("max_p95_regression_percent")
    if not finite_number(p95) or p95 < 0:
        raise ValueError("threshold evidence p95 limit is invalid")
    if materiality.get("decision_rule") != (
        "launch or idle_private; every selected path must satisfy both its absolute "
        "and relative threshold"
    ):
        raise ValueError("threshold evidence decision rule does not match")
    if materiality.get("cache_rule") != (
        "warm and fresh-profile evidence must each meet the materiality rule"
    ):
        raise ValueError("threshold evidence cache rule does not match")
    if evidence.get("fallback_decision") != "keep in-process":
        raise ValueError("threshold evidence fallback decision does not match")


def canonical_threshold_evidence(evidence: dict[str, object]) -> dict[str, object]:
    validate_threshold_evidence(evidence)
    materiality = evidence["materiality"]
    assert isinstance(materiality, dict)
    launch = materiality["launch"]
    idle = materiality["idle_private"]
    assert isinstance(launch, dict)
    assert isinstance(idle, dict)
    return {
        "schema": STARTUP_THRESHOLD_SCHEMA,
        "created_at": evidence["created_at"],
        "status": "APPROVED",
        "approved_by": "project-owner",
        "scope": "model-transport",
        "source": evidence["source"],
        "materiality": {
            "launch": {
                "milestones": list(launch["milestones"]),
                "required_absolute_improvement_ms": launch[
                    "required_absolute_improvement_ms"
                ],
                "required_relative_improvement_percent": launch[
                    "required_relative_improvement_percent"
                ],
                "max_p95_regression_percent": launch[
                    "max_p95_regression_percent"
                ],
            },
            "idle_private": {
                "required_absolute_improvement_mb": idle[
                    "required_absolute_improvement_mb"
                ],
                "required_relative_improvement_percent": idle[
                    "required_relative_improvement_percent"
                ],
            },
            "decision_rule": materiality["decision_rule"],
            "cache_rule": materiality["cache_rule"],
        },
        "fallback_decision": "keep in-process",
    }


def load_threshold_evidence(
    path: Path,
    *,
    source: dict[str, object],
    checked_at: datetime,
) -> dict[str, object]:
    from .native.runtime import sha256_file

    evidence = canonical_threshold_evidence(read_evidence_object(path))
    if evidence["source"] != source:
        raise ValueError("threshold evidence was approved for a different source state")
    approved_at = parse_evidence_time(
        evidence["created_at"], "threshold evidence created_at"
    )
    if approved_at > checked_at.astimezone(UTC):
        raise ValueError("threshold evidence was created after measurement began")
    return {
        "artifact": sha256_file(path).evidence(),
        "record": evidence,
    }


def safe_command() -> list[str]:
    command: list[str] = []
    redact_next = False
    for index, arg in enumerate(sys.argv):
        if redact_next:
            command.append("<redacted-path>")
            redact_next = False
            continue
        matching = next((flag for flag in PATH_ARGUMENTS if arg.startswith(f"{flag}=")), None)
        if matching is not None:
            command.append(f"{matching}=<redacted-path>")
            continue
        command.append(Path(arg).name if index == 0 else arg)
        redact_next = arg in PATH_ARGUMENTS
    return command


def validate_startup_evidence(evidence: dict[str, object]) -> None:
    if evidence.get("schema") != STARTUP_EVIDENCE_SCHEMA:
        raise ValueError("invalid Goal 04 startup evidence schema")
    created_at = parse_evidence_time(evidence.get("created_at"), "created_at")
    source = evidence.get("source")
    validate_source_state(source)
    assert isinstance(source, dict)
    host = evidence.get("host")
    validate_goal04_host_context(host)
    assert isinstance(host, dict)
    checked_at = parse_evidence_time(
        evidence.get("measurement_started_at"), "measurement_started_at"
    )
    if created_at < checked_at:
        raise ValueError("startup evidence predates its measurement")
    quiet = evidence.get("quiet_gate")
    if not isinstance(quiet, dict):
        raise ValueError("startup evidence requires a passing quiet gate")
    validate_startup_quiet_evidence(
        quiet,
        source=source,
        host=host,
        checked_at=checked_at,
    )
    if normalized_quiet_evidence(quiet) != quiet:
        raise ValueError("startup quiet evidence is not canonical")
    rounds = evidence.get("rounds")
    if not isinstance(rounds, int) or rounds < 10:
        raise ValueError("startup evidence requires at least 10 rounds")
    cache_state = evidence.get("cache_state")
    warmup = evidence.get("warmup_launches_per_variant")
    idle_settle = evidence.get("idle_settle_seconds")
    if (
        not isinstance(warmup, int)
        or isinstance(warmup, bool)
        or warmup < 0
        or (cache_state == "warm" and warmup < 1)
        or not finite_number(idle_settle)
        or idle_settle < 0
    ):
        raise ValueError("startup evidence measurement controls are invalid")
    if not isinstance(cache_state, str) or cache_state not in {
        "warm",
        "fresh-profile",
    } or evidence.get("cache_control") != {
        "data_and_config_roots": (
            "reused per variant across warmup and measured launches"
            if cache_state == "warm"
            else "fresh isolated roots for every launch"
        ),
        "windows_file_cache": "not flushed",
        "cold_start_claim": False,
    }:
        raise ValueError("startup evidence cache contract is invalid")
    if evidence.get("target") != rust_target() or evidence.get("profile") != "release":
        raise ValueError("startup evidence target or profile does not match")
    command = evidence.get("command")
    if (
        not isinstance(command, list)
        or not command
        or not all(safe_metadata_text(part) for part in command)
    ):
        raise ValueError("startup evidence command is not content-free")
    if evidence.get("instrumentation") != {
        "schema": STARTUP_TRACE_SCHEMA,
        "input": "Win32 SendInput VK_F24 -> GPUI action acknowledgement",
        "first_frame": (
            "GPUI post-render callback for the first application frame; "
            "not a DWM presentation timestamp"
        ),
        "comparison_scope": "enabled identically for every compared variant",
    }:
        raise ValueError("startup evidence instrumentation contract is invalid")
    variants = evidence.get("variants")
    if not isinstance(variants, list) or not variants:
        raise ValueError("startup evidence requires at least one variant")
    comparison = evidence.get("comparison")
    if len(variants) not in {1, 2} or (len(variants) == 2) != isinstance(comparison, dict):
        raise ValueError("startup evidence comparison shape is invalid")
    expected_samples = rounds * (2 if len(variants) == 2 else 1)
    labels = [variant.get("label") for variant in variants if isinstance(variant, dict)]
    if (
        len(labels) != len(variants)
        or len(set(labels)) != len(labels)
        or any(label not in EVIDENCE_VARIANT_LABELS for label in labels)
    ):
        raise ValueError("startup evidence requires distinct variant labels")
    decision_pair = labels == ["full", "no-model"]
    if "no-model" in labels and not decision_pair:
        raise ValueError("no-model startup evidence requires the full/no-model decision pair")
    if evidence.get("decision_scope") != (
        "model-transport" if decision_pair else "diagnostic"
    ):
        raise ValueError("startup evidence decision scope is invalid")
    threshold = evidence.get("threshold")
    if decision_pair:
        if not isinstance(threshold, dict) or set(threshold) != {"artifact", "record"}:
            raise ValueError("model-transport evidence requires an approved threshold")
        validate_fingerprint(threshold.get("artifact"), "threshold artifact")
        record = threshold.get("record")
        if not isinstance(record, dict) or canonical_threshold_evidence(record) != record:
            raise ValueError("startup threshold record is invalid")
        if record.get("source") != source or parse_evidence_time(
            record.get("created_at"), "threshold evidence created_at"
        ) > checked_at:
            raise ValueError("startup threshold record is not pre-registered")
    elif threshold is not None:
        raise ValueError("diagnostic startup evidence cannot carry a decision threshold")
    preflight = evidence.get("preflight")
    if not isinstance(preflight, dict) or set(preflight) != set(labels):
        raise ValueError("startup evidence preflight map is invalid")
    parsed_samples: list[tuple[StartupSample, ...]] = []
    builds: list[dict[str, object]] = []
    for variant in variants:
        assert isinstance(variant, dict)
        label = variant["label"]
        executable = variant.get("executable")
        build = variant.get("build")
        samples = variant.get("samples")
        if (
            not isinstance(executable, dict)
            or not isinstance(executable.get("byte_count"), int)
            or not isinstance(executable.get("sha256"), str)
            or not isinstance(samples, list)
            or len(samples) != expected_samples
        ):
            raise ValueError("startup evidence contains an invalid variant")
        validate_fingerprint(
            {
                "byte_count": executable.get("byte_count"),
                "sha256": executable.get("sha256"),
            }
        )
        if not isinstance(build, dict) or canonical_build_evidence(build) != build:
            raise ValueError("startup evidence contains invalid build provenance")
        if (
            build.get("variant") != label
            or build.get("artifact_kind") != "application"
            or build.get("source") != source
            or build.get("executable")
            != {
                "byte_count": executable.get("byte_count"),
                "sha256": executable.get("sha256"),
            }
        ):
            raise ValueError("startup build provenance does not match the variant")
        builds.append(build)
        preflight_entry = preflight.get(label)
        if not isinstance(preflight_entry, dict):
            raise ValueError("startup evidence preflight entry is invalid")
        preflight_executable = preflight_entry.get("executable")
        if (
            not isinstance(preflight_executable, dict)
            or preflight_executable.get("byte_count") != executable.get("byte_count")
            or preflight_executable.get("sha256") != executable.get("sha256")
            or preflight_executable.get("format") != "PE32+"
            or preflight_executable.get("machine") != "x86_64"
            or preflight_executable.get("hash_verified") is not True
            or preflight_entry.get("environment") != host.get("environment")
        ):
            raise ValueError("startup evidence preflight does not match the variant")
        parsed = tuple(startup_sample_from_evidence(sample) for sample in samples)
        if label == "bare":
            if any(sample.initial_state != "bare" for sample in parsed):
                raise ValueError("bare startup evidence has the wrong initial state")
        elif any(sample.initial_state == "bare" for sample in parsed):
            raise ValueError("application startup evidence has a bare initial state")
        if variant.get("summary") != startup_summary(parsed):
            raise ValueError("startup evidence summary does not match its samples")
        parsed_samples.append(parsed)
    if len(builds) == 2 and builds[0].get("toolchain") != builds[1].get("toolchain"):
        raise ValueError("startup comparison builds use different Rust toolchains")
    if len(parsed_samples) == 2 and comparison != milestone_comparison(
        parsed_samples[0], parsed_samples[1]
    ):
        raise ValueError("startup evidence comparison does not match its samples")
    expected_evaluation = (
        evaluate_model_threshold(parsed_samples[0], parsed_samples[1], threshold)
        if decision_pair
        else None
    )
    if evidence.get("threshold_evaluation") != expected_evaluation:
        raise ValueError("startup threshold evaluation does not match its samples")


def validate_quiet_evidence(evidence: dict[str, object]) -> None:
    if evidence.get("schema") != STARTUP_QUIET_SCHEMA:
        raise ValueError("invalid Goal 04 quiet evidence schema")
    if evidence.get("status") not in {"PASS", "FAIL"}:
        raise ValueError("invalid Goal 04 quiet evidence status")
    parse_evidence_time(evidence.get("created_at"), "quiet evidence created_at")
    validate_source_state(evidence.get("source"))
    validate_goal04_host_context(evidence.get("host"))
    window = evidence.get("window")
    thresholds = evidence.get("thresholds")
    samples = evidence.get("samples")
    if (
        not isinstance(window, dict)
        or not isinstance(thresholds, dict)
        or not isinstance(samples, dict)
    ):
        raise ValueError("quiet evidence is missing its sample window")
    expected = window.get("samples")
    interval = window.get("interval_seconds")
    waited = evidence.get("waited_seconds")
    cpu = samples.get("cpu_percent")
    disk = samples.get("disk_percent")
    if (
        not isinstance(expected, int)
        or expected < 2
        or not finite_number(interval)
        or interval <= 0
        or not finite_number(waited)
        or waited < 0
        or not isinstance(cpu, list)
        or not isinstance(disk, list)
        or len(cpu) != expected
        or len(disk) != expected
        or not all(finite_number(value) and 0 <= value <= 100 for value in cpu + disk)
    ):
        raise ValueError("quiet evidence sample counts do not match")
    threshold_names = (
        "max_cpu_median_percent",
        "max_cpu_p95_percent",
        "max_disk_median_percent",
        "max_disk_p95_percent",
    )
    if not all(
        finite_number(thresholds.get(name)) and thresholds[name] >= 0
        for name in threshold_names
    ):
        raise ValueError("quiet evidence thresholds are invalid")
    failures = quiet_gate_failures(
        cpu,
        disk,
        thresholds["max_cpu_median_percent"],
        thresholds["max_cpu_p95_percent"],
        thresholds["max_disk_median_percent"],
        thresholds["max_disk_p95_percent"],
    )
    expected_status = "FAIL" if failures else "PASS"
    if evidence.get("status") != expected_status:
        raise ValueError("quiet evidence status does not match its samples")


def finite_number(value: object) -> bool:
    return (
        isinstance(value, (int, float))
        and not isinstance(value, bool)
        and math.isfinite(value)
    )


def safe_metadata_text(value: object) -> bool:
    return (
        isinstance(value, str)
        and 0 < len(value) <= 256
        and value.isprintable()
        and "\\" not in value
        and "/" not in value
    )


def validate_goal04_host_context(value: object) -> None:
    if not isinstance(value, dict) or set(value) != {"environment", "hardware"}:
        raise ValueError("invalid Goal 04 host context")
    environment = value.get("environment")
    hardware = value.get("hardware")
    if not isinstance(environment, dict) or set(environment) != {
        "platform",
        "windows_major",
        "windows_minor",
        "windows_build",
        "architecture",
        "native_machine_code",
        "python_pointer_bits",
        "wts_state",
        "active_console_session_id",
        "harness_is_console_session",
        "input_desktop",
        "thread_desktop",
        "harness_process",
    }:
        raise ValueError("invalid Goal 04 environment context")
    if (
        environment.get("platform") != "Windows 11"
        or environment.get("architecture") != "x86_64"
        or environment.get("python_pointer_bits") != 64
        or environment.get("wts_state") != "WTSActive"
        or not all(
            isinstance(environment.get(name), int)
            and not isinstance(environment.get(name), bool)
            for name in (
                "windows_major",
                "windows_minor",
                "windows_build",
                "native_machine_code",
            )
        )
        or not isinstance(environment.get("harness_is_console_session"), bool)
        or not safe_metadata_text(environment.get("input_desktop"))
        or not safe_metadata_text(environment.get("thread_desktop"))
    ):
        raise ValueError("invalid Goal 04 environment context")
    active_console = environment.get("active_console_session_id")
    if active_console is not None and (
        not isinstance(active_console, int) or isinstance(active_console, bool)
    ):
        raise ValueError("invalid Goal 04 environment context")
    process = environment.get("harness_process")
    if not isinstance(process, dict) or set(process) != {
        "session_id",
        "integrity_rid",
        "integrity",
    }:
        raise ValueError("invalid Goal 04 process context")
    if (
        not isinstance(process.get("session_id"), int)
        or isinstance(process.get("session_id"), bool)
        or not isinstance(process.get("integrity_rid"), int)
        or isinstance(process.get("integrity_rid"), bool)
        or not safe_metadata_text(process.get("integrity"))
    ):
        raise ValueError("invalid Goal 04 process context")
    if not isinstance(hardware, dict) or set(hardware) != {
        "processor",
        "logical_processors",
        "gpu",
    }:
        raise ValueError("invalid Goal 04 hardware context")
    gpu = hardware.get("gpu")
    if (
        not safe_metadata_text(hardware.get("processor"))
        or not isinstance(hardware.get("logical_processors"), int)
        or isinstance(hardware.get("logical_processors"), bool)
        or hardware["logical_processors"] <= 0
        or not isinstance(gpu, list)
        or not 1 <= len(gpu) <= 8
        or not all(safe_metadata_text(name) for name in gpu)
    ):
        raise ValueError("invalid Goal 04 hardware context")


def normalized_quiet_evidence(evidence: dict[str, object]) -> dict[str, object]:
    """Keep only content-free fields and recompute all derived quiet results."""
    validate_quiet_evidence(evidence)
    window = evidence["window"]
    thresholds = evidence["thresholds"]
    samples = evidence["samples"]
    assert isinstance(window, dict)
    assert isinstance(thresholds, dict)
    assert isinstance(samples, dict)
    cpu = list(samples["cpu_percent"])
    disk = list(samples["disk_percent"])
    failures = quiet_gate_failures(
        cpu,
        disk,
        thresholds["max_cpu_median_percent"],
        thresholds["max_cpu_p95_percent"],
        thresholds["max_disk_median_percent"],
        thresholds["max_disk_p95_percent"],
    )
    return {
        "schema": STARTUP_QUIET_SCHEMA,
        "created_at": evidence["created_at"],
        "status": "FAIL" if failures else "PASS",
        "source": evidence["source"],
        "host": evidence["host"],
        "window": {
            "samples": window["samples"],
            "interval_seconds": window["interval_seconds"],
        },
        "waited_seconds": evidence["waited_seconds"],
        "thresholds": {name: thresholds[name] for name in (
            "max_cpu_median_percent",
            "max_cpu_p95_percent",
            "max_disk_median_percent",
            "max_disk_p95_percent",
        )},
        "samples": {"cpu_percent": cpu, "disk_percent": disk},
        "summary": {
            "cpu_median_percent": median(cpu),
            "cpu_p95_percent": nearest_rank_percentile(cpu, 0.95),
            "disk_median_percent": median(disk),
            "disk_p95_percent": nearest_rank_percentile(disk, 0.95),
        },
        "failures": failures,
    }


def parse_evidence_time(value: object, field: str) -> datetime:
    if not isinstance(value, str):
        raise ValueError(f"{field} must be an ISO 8601 timestamp")
    try:
        parsed = datetime.fromisoformat(value.replace("Z", "+00:00"))
    except ValueError:
        raise ValueError(f"{field} must be an ISO 8601 timestamp") from None
    if parsed.tzinfo is None:
        raise ValueError(f"{field} must include a UTC offset")
    return parsed.astimezone(UTC)


def validate_startup_quiet_evidence(
    evidence: dict[str, object],
    *,
    source: dict[str, object],
    host: dict[str, object],
    checked_at: datetime,
) -> None:
    validate_quiet_evidence(evidence)
    if evidence.get("status") != "PASS":
        raise ValueError("startup measurement requires a passing quiet gate")
    if evidence.get("source") != source:
        raise ValueError("quiet evidence was captured from a different source state")
    if evidence.get("host") != host:
        raise ValueError("quiet evidence was captured from a different host or session")
    age = checked_at.astimezone(UTC) - parse_evidence_time(
        evidence.get("created_at"), "quiet evidence created_at"
    )
    if age < timedelta(0) or age > QUIET_EVIDENCE_MAX_AGE:
        raise ValueError("quiet evidence is stale; capture a new gate immediately before startup")


def rust_target() -> str:
    rustc = shutil.which("rustc")
    if rustc is None:
        fallback = Path.home() / ".cargo" / "bin" / "rustc.exe"
        rustc = str(fallback) if fallback.is_file() else None
    if rustc is None:
        return "unknown"
    result = subprocess.run(
        [rustc, "-vV"], capture_output=True, text=True, check=False
    )
    for line in result.stdout.splitlines():
        if line.startswith("host: "):
            return line.removeprefix("host: ")
    return "unknown"


def gpu_names() -> list[str]:
    pwsh = shutil.which("pwsh")
    if pwsh is None:
        return ["unavailable"]
    result = subprocess.run(
        [
            pwsh,
            "-NoProfile",
            "-Command",
            "Get-CimInstance Win32_VideoController | Select-Object -ExpandProperty Name",
        ],
        capture_output=True,
        text=True,
        timeout=15,
        check=False,
    )
    names = [line.strip() for line in result.stdout.splitlines() if line.strip()]
    return names or ["unavailable"]


def goal04_host_context() -> dict[str, object]:
    from .native.runtime import environment_preflight

    _, _, environment = environment_preflight()
    return {
        "environment": environment,
        "hardware": {
            "processor": os.environ.get("PROCESSOR_IDENTIFIER", platform.processor()),
            "logical_processors": os.cpu_count(),
            "gpu": gpu_names(),
        },
    }


def write_startup_evidence(
    path: Path,
    *,
    label_a: str,
    samples_a: tuple[StartupSample, ...],
    cache_state: str,
    rounds: int,
    warmup: int,
    idle_settle: float,
    measurement_started_at: datetime,
    source: dict[str, object],
    host: dict[str, object],
    build_a: dict[str, object],
    preflight_a: dict[str, object],
    quiet_gate: dict[str, object],
    threshold: dict[str, object] | None = None,
    label_b: str | None = None,
    samples_b: tuple[StartupSample, ...] = (),
    build_b: dict[str, object] | None = None,
    preflight_b: dict[str, object] | None = None,
) -> None:
    def variant(
        label: str,
        samples: tuple[StartupSample, ...],
        build: dict[str, object],
        preflight: dict[str, object],
    ) -> dict[str, object]:
        executable = preflight.get("executable")
        if not isinstance(executable, dict):
            raise ValueError("preflight did not record executable evidence")
        return {
            "label": label,
            "executable": dict(executable),
            "build": build,
            "samples": [sample.evidence() for sample in samples],
            "summary": startup_summary(samples),
        }

    evidence: dict[str, object] = {
        "schema": STARTUP_EVIDENCE_SCHEMA,
        "created_at": datetime.now(UTC).isoformat(),
        "measurement_started_at": measurement_started_at.isoformat(),
        "command": safe_command(),
        "source": source,
        "target": rust_target(),
        "profile": "release",
        "host": host,
        "cache_state": cache_state,
        "cache_control": {
            "data_and_config_roots": (
                "reused per variant across warmup and measured launches"
                if cache_state == "warm"
                else "fresh isolated roots for every launch"
            ),
            "windows_file_cache": "not flushed",
            "cold_start_claim": False,
        },
        "rounds": rounds,
        "warmup_launches_per_variant": warmup,
        "idle_settle_seconds": idle_settle,
        "instrumentation": {
            "schema": STARTUP_TRACE_SCHEMA,
            "input": "Win32 SendInput VK_F24 -> GPUI action acknowledgement",
            "first_frame": (
                "GPUI post-render callback for the first application frame; "
                "not a DWM presentation timestamp"
            ),
            "comparison_scope": "enabled identically for every compared variant",
        },
        "decision_scope": (
            "model-transport" if [label_a, label_b] == ["full", "no-model"] else "diagnostic"
        ),
        "threshold": threshold,
        "threshold_evaluation": None,
        "quiet_gate": quiet_gate,
        "preflight": {label_a: preflight_a},
        "variants": [variant(label_a, samples_a, build_a, preflight_a)],
    }
    if label_b is not None and build_b is not None and preflight_b is not None:
        evidence["preflight"][label_b] = preflight_b
        evidence["variants"].append(variant(label_b, samples_b, build_b, preflight_b))
        evidence["comparison"] = milestone_comparison(samples_a, samples_b)
        if [label_a, label_b] == ["full", "no-model"]:
            assert threshold is not None
            evidence["threshold_evaluation"] = evaluate_model_threshold(
                samples_a,
                samples_b,
                threshold,
            )
    from .native.runtime import write_evidence

    write_evidence(path, evidence, validate_startup_evidence)


def model_first_use_cache_state(warmup: int) -> dict[str, str]:
    return {
        "process": "fresh process per sample",
        "transport": "cold initialization inside each process",
        "windows_file_cache": (
            f"warmed by {warmup} discarded process run(s); not flushed"
            if warmup
            else "not warmed by the harness; not flushed"
        ),
    }


def validate_model_first_use_evidence(evidence: dict[str, object]) -> None:
    if evidence.get("schema") != MODEL_FIRST_USE_EVIDENCE_SCHEMA:
        raise ValueError("invalid Goal 04 model first-use evidence schema")
    created_at = parse_evidence_time(evidence.get("created_at"), "created_at")
    checked_at = parse_evidence_time(
        evidence.get("measurement_started_at"), "measurement_started_at"
    )
    if created_at < checked_at:
        raise ValueError("model first-use evidence predates its measurement")
    source = evidence.get("source")
    validate_source_state(source)
    assert isinstance(source, dict)
    host = evidence.get("host")
    validate_goal04_host_context(host)
    assert isinstance(host, dict)
    quiet = evidence.get("quiet_gate")
    if not isinstance(quiet, dict):
        raise ValueError("model first-use evidence requires a passing quiet gate")
    validate_startup_quiet_evidence(
        quiet,
        source=source,
        host=host,
        checked_at=checked_at,
    )
    if normalized_quiet_evidence(quiet) != quiet:
        raise ValueError("model first-use quiet evidence is not canonical")
    rounds = evidence.get("rounds")
    warmup = evidence.get("warmup_runs")
    samples = evidence.get("samples_us")
    if (
        not isinstance(rounds, int)
        or isinstance(rounds, bool)
        or rounds < 10
        or not isinstance(warmup, int)
        or isinstance(warmup, bool)
        or warmup < 0
        or not isinstance(samples, list)
        or len(samples) != rounds
        or not all(finite_number(value) and value >= 0 for value in samples)
    ):
        raise ValueError("model first-use samples are invalid")
    expected_summary = {
        "median_us": median(samples),
        "p95_us": nearest_rank_percentile(samples, 0.95),
    }
    if evidence.get("summary") != expected_summary:
        raise ValueError("model first-use summary does not match its samples")
    if evidence.get("cache_state") != model_first_use_cache_state(warmup):
        raise ValueError("model first-use cache state does not match its runs")
    full_build = evidence.get("full_application_build")
    test_build = evidence.get("test_build")
    if (
        not isinstance(full_build, dict)
        or canonical_build_evidence(full_build) != full_build
        or full_build.get("variant") != "full"
        or full_build.get("source") != source
        or not isinstance(test_build, dict)
        or canonical_build_evidence(test_build) != test_build
        or test_build.get("variant") != "model-first-use"
        or test_build.get("source") != source
        or full_build.get("cargo_lock") != test_build.get("cargo_lock")
        or full_build.get("target") != test_build.get("target")
        or full_build.get("profile") != test_build.get("profile")
        or full_build.get("toolchain") != test_build.get("toolchain")
    ):
        raise ValueError("model first-use build provenance is invalid")
    validate_fingerprint(evidence.get("executable"))
    if evidence.get("executable") != test_build.get("executable"):
        raise ValueError("model first-use executable does not match its build")
    if evidence.get("target") != test_build.get("target") or evidence.get(
        "profile"
    ) != test_build.get("profile"):
        raise ValueError("model first-use target or profile does not match")
    command = evidence.get("command")
    if (
        not isinstance(command, list)
        or not command
        or not all(safe_metadata_text(part) for part in command)
    ):
        raise ValueError("model first-use command is not content-free")
    if evidence.get("measurement") != {
        "protocol": "first transport initialization plus first loopback HTTP request",
        "subsequent_request": "executed only to preserve the test contract; not reported",
        "ablated_result": "not applicable; model transport is absent",
    }:
        raise ValueError("model first-use measurement contract is invalid")


def write_model_first_use_evidence(
    path: Path,
    *,
    measurement_started_at: datetime,
    source: dict[str, object],
    host: dict[str, object],
    quiet_gate: dict[str, object],
    full_build: dict[str, object],
    test_build: dict[str, object],
    samples: list[float],
    rounds: int,
    warmup: int,
) -> None:
    from .native.runtime import write_evidence

    evidence: dict[str, object] = {
        "schema": MODEL_FIRST_USE_EVIDENCE_SCHEMA,
        "created_at": datetime.now(UTC).isoformat(),
        "measurement_started_at": measurement_started_at.isoformat(),
        "command": safe_command(),
        "source": source,
        "host": host,
        "target": test_build["target"],
        "profile": test_build["profile"],
        "rounds": rounds,
        "warmup_runs": warmup,
        "cache_state": model_first_use_cache_state(warmup),
        "quiet_gate": quiet_gate,
        "full_application_build": full_build,
        "test_build": test_build,
        "executable": test_build["executable"],
        "samples_us": samples,
        "summary": {
            "median_us": median(samples),
            "p95_us": nearest_rank_percentile(samples, 0.95),
        },
        "measurement": {
            "protocol": "first transport initialization plus first loopback HTTP request",
            "subsequent_request": "executed only to preserve the test contract; not reported",
            "ablated_result": "not applicable; model transport is absent",
        },
    }
    write_evidence(path, evidence, validate_model_first_use_evidence)


def startup_decision_input(path: Path, cache_state: str) -> tuple[dict[str, object], dict[str, object]]:
    from .native.runtime import sha256_file

    evidence = read_evidence_object(path)
    validate_startup_evidence(evidence)
    variants = evidence.get("variants")
    if (
        evidence.get("decision_scope") != "model-transport"
        or evidence.get("cache_state") != cache_state
        or not isinstance(variants, list)
        or [variant.get("label") for variant in variants if isinstance(variant, dict)]
        != ["full", "no-model"]
    ):
        raise ValueError(f"{cache_state} input is not model-transport startup evidence")
    evaluation = evidence.get("threshold_evaluation")
    quiet = evidence.get("quiet_gate")
    if not isinstance(evaluation, dict) or not isinstance(
        evaluation.get("materiality_met"), bool
    ) or not isinstance(quiet, dict):
        raise ValueError(f"{cache_state} input is missing its threshold evaluation")
    return evidence, {
        "artifact": sha256_file(path).evidence(),
        "cache_state": cache_state,
        "startup_created_at": evidence["created_at"],
        "quiet_gate_created_at": quiet["created_at"],
        "materiality_met": evaluation["materiality_met"],
    }


def validate_model_transport_decision_evidence(evidence: dict[str, object]) -> None:
    if evidence.get("schema") != MODEL_TRANSPORT_DECISION_SCHEMA:
        raise ValueError("invalid Goal 04 model-transport decision schema")
    parse_evidence_time(evidence.get("created_at"), "decision created_at")
    if evidence.get("status") != "APPROVED" or evidence.get("approved_by") != "project-owner":
        raise ValueError("model-transport decision is not owner-approved")
    source = evidence.get("source")
    validate_source_state(source)
    host = evidence.get("host")
    validate_goal04_host_context(host)
    threshold = evidence.get("threshold")
    if not isinstance(threshold, dict) or set(threshold) != {"artifact", "record"}:
        raise ValueError("model-transport decision is missing its threshold")
    validate_fingerprint(threshold.get("artifact"), "threshold artifact")
    record = threshold.get("record")
    if (
        not isinstance(record, dict)
        or canonical_threshold_evidence(record) != record
        or record.get("source") != source
    ):
        raise ValueError("model-transport decision threshold is invalid")
    inputs = evidence.get("inputs")
    if not isinstance(inputs, dict) or set(inputs) != {"warm", "fresh_profile"}:
        raise ValueError("model-transport decision inputs are invalid")
    materiality: dict[str, bool] = {}
    for key, cache_state in (("warm", "warm"), ("fresh_profile", "fresh-profile")):
        value = inputs.get(key)
        if not isinstance(value, dict) or set(value) != {
            "artifact",
            "cache_state",
            "startup_created_at",
            "quiet_gate_created_at",
            "materiality_met",
        }:
            raise ValueError("model-transport decision input is invalid")
        validate_fingerprint(value.get("artifact"), "startup evidence artifact")
        parse_evidence_time(value.get("startup_created_at"), "startup created_at")
        parse_evidence_time(value.get("quiet_gate_created_at"), "quiet gate created_at")
        if value.get("cache_state") != cache_state or not isinstance(
            value.get("materiality_met"), bool
        ):
            raise ValueError("model-transport decision input is invalid")
        materiality[key] = value["materiality_met"]
    expected_materiality = {
        "warm": materiality["warm"],
        "fresh_profile": materiality["fresh_profile"],
        "both": materiality["warm"] and materiality["fresh_profile"],
    }
    if evidence.get("materiality") != expected_materiality:
        raise ValueError("model-transport decision materiality is invalid")
    fingerprints = evidence.get("variant_fingerprints")
    if not isinstance(fingerprints, dict) or set(fingerprints) != {"full", "no-model"}:
        raise ValueError("model-transport decision variants are invalid")
    validate_fingerprint(fingerprints.get("full"), "full executable")
    validate_fingerprint(fingerprints.get("no-model"), "no-model executable")
    decision = evidence.get("decision")
    if decision not in MODEL_TRANSPORT_DECISIONS:
        raise ValueError("model-transport decision is invalid")
    if not expected_materiality["both"] and decision != "keep in-process":
        raise ValueError("below-threshold evidence must keep model transport in-process")
    authorization = (
        "model transport extraction authorized"
        if decision == "isolate in a worker"
        else "no model transport extraction authorized"
    )
    if evidence.get("authorization") != authorization:
        raise ValueError("model-transport decision authorization is invalid")
    command = evidence.get("command")
    if (
        not isinstance(command, list)
        or not command
        or not all(safe_metadata_text(part) for part in command)
    ):
        raise ValueError("model-transport decision command is not content-free")


def cmd_decide_goal04(a: argparse.Namespace) -> None:
    from .native.runtime import write_evidence

    if not a.owner_approved:
        sys.exit("--owner-approved is required to write the Goal 04 decision")
    try:
        require_distinct_output_path(
            a.evidence,
            a.warm_evidence,
            a.fresh_profile_evidence,
        )
        warm, warm_summary = startup_decision_input(a.warm_evidence, "warm")
        fresh, fresh_summary = startup_decision_input(
            a.fresh_profile_evidence, "fresh-profile"
        )
    except ValueError as error:
        sys.exit(str(error))
    if (
        warm.get("source") != fresh.get("source")
        or warm.get("host") != fresh.get("host")
        or warm.get("threshold") != fresh.get("threshold")
    ):
        sys.exit("warm and fresh-profile evidence do not share source, host and threshold")
    if warm.get("source") != source_state():
        sys.exit("startup evidence does not match the current source state; rerun measurement")
    warm_variants = warm["variants"]
    fresh_variants = fresh["variants"]
    assert isinstance(warm_variants, list)
    assert isinstance(fresh_variants, list)
    if any(
        warm_variant.get("build") != fresh_variant.get("build")
        for warm_variant, fresh_variant in zip(warm_variants, fresh_variants, strict=True)
    ):
        sys.exit("warm and fresh-profile evidence do not use the same builds")
    both_material = bool(
        warm_summary["materiality_met"] and fresh_summary["materiality_met"]
    )
    if not both_material and a.decision != "keep in-process":
        sys.exit("below-threshold evidence requires the keep in-process decision")
    variant_fingerprints = {
        variant["label"]: {
            "byte_count": variant["executable"]["byte_count"],
            "sha256": variant["executable"]["sha256"],
        }
        for variant in warm_variants
    }
    evidence: dict[str, object] = {
        "schema": MODEL_TRANSPORT_DECISION_SCHEMA,
        "created_at": datetime.now(UTC).isoformat(),
        "status": "APPROVED",
        "approved_by": "project-owner",
        "command": safe_command(),
        "source": warm["source"],
        "host": warm["host"],
        "threshold": warm["threshold"],
        "inputs": {
            "warm": warm_summary,
            "fresh_profile": fresh_summary,
        },
        "variant_fingerprints": variant_fingerprints,
        "materiality": {
            "warm": warm_summary["materiality_met"],
            "fresh_profile": fresh_summary["materiality_met"],
            "both": both_material,
        },
        "decision": a.decision,
        "authorization": (
            "model transport extraction authorized"
            if a.decision == "isolate in a worker"
            else "no model transport extraction authorized"
        ),
    }
    write_evidence(a.evidence, evidence, validate_model_transport_decision_evidence)
    print(
        f"Goal 04 decision: {a.decision}; "
        f"materiality in both cache modes={both_material}"
    )


def goal04_test_executable(output: str, target_name: str) -> Path:
    candidates: list[Path] = []
    for line in output.splitlines():
        try:
            message = json.loads(line)
        except json.JSONDecodeError:
            continue
        if (
            isinstance(message, dict)
            and message.get("reason") == "compiler-artifact"
            and isinstance(message.get("target"), dict)
            and message["target"].get("name") == target_name
            and isinstance(message.get("executable"), str)
        ):
            candidates.append(Path(message["executable"]))
    if len(candidates) != 1:
        raise ValueError("cargo did not report exactly one Goal 04 test executable")
    return candidates[0]


def cmd_build_goal04(a: argparse.Namespace) -> None:
    """Create one fresh, source-bound Goal 04 executable and build manifest."""
    from .native.runtime import inspect_pe_sections, sha256_file, write_evidence

    variant = GOAL04_BUILD_VARIANTS[a.variant]
    target_dir = a.target_dir.resolve()
    suffix = ".exe" if sys.platform == "win32" else ""
    expected_executable = (
        target_dir / GOAL04_TARGET / "release" / f"{variant.target_name}{suffix}"
        if variant.artifact_kind == "application"
        else target_dir / "goal04-artifacts" / "model-first-use.exe"
    )
    try:
        require_distinct_output_path(a.evidence, expected_executable)
    except ValueError as error:
        sys.exit(str(error))
    if target_dir.exists() and any(target_dir.iterdir()):
        sys.exit("--target-dir must be absent or empty for a fresh Goal 04 build")
    target_dir.mkdir(parents=True, exist_ok=True)

    cargo = shutil.which("cargo")
    if cargo is None:
        fallback = Path.home() / ".cargo" / "bin" / "cargo.exe"
        cargo = str(fallback) if fallback.is_file() else None
    if cargo is None:
        sys.exit("cargo not found")

    before = source_state()
    command = goal04_build_command(variant)
    actual_command = [cargo, *command[1:]]
    env = goal04_build_environment(target_dir, a.variant)
    try:
        toolchain = goal04_toolchain_context(cargo, env)
        validate_goal04_toolchain_context(toolchain)
    except ValueError as error:
        sys.exit(f"Goal 04 toolchain capture failed: {error}")
    result = subprocess.run(
        actual_command,
        cwd=REPO,
        env=env,
        capture_output=True,
        text=True,
        encoding="utf-8",
        errors="replace",
    )
    if result.returncode:
        output = (result.stdout + result.stderr)[-8000:]
        sys.exit(f"Goal 04 build failed with {result.returncode}:\n{output}")

    if variant.artifact_kind == "application":
        executable = expected_executable
    else:
        try:
            built_test = goal04_test_executable(result.stdout, variant.target_name)
        except ValueError as error:
            sys.exit(str(error))
        artifact_dir = target_dir / "goal04-artifacts"
        artifact_dir.mkdir()
        executable = expected_executable
        shutil.copy2(built_test, executable)
    if not executable.is_file():
        sys.exit("Goal 04 build did not produce its expected executable")

    behavior_verification: dict[str, object] | None = None
    verification_command = goal04_behavior_verification_command(a.variant)
    if verification_command is not None:
        verification_result = subprocess.run(
            [cargo, *verification_command[1:]],
            cwd=REPO,
            env=env,
            capture_output=True,
            text=True,
            encoding="utf-8",
            errors="replace",
        )
        verification_output = verification_result.stdout + verification_result.stderr
        if verification_result.returncode:
            sys.exit(
                "Goal 04 behavior verification failed with "
                f"{verification_result.returncode}:\n{verification_output[-8000:]}"
            )
        results = re.findall(
            r"test result: ok\. (\d+) passed; (\d+) failed;",
            verification_output,
        )
        if results != [("1", "0")]:
            sys.exit("Goal 04 behavior verification did not run exactly one passing test")
        behavior_verification = {
            "command": verification_command,
            "passed": 1,
            "failed": 0,
        }

    tree_command = goal04_tree_command(variant)
    tree_result = subprocess.run(
        [cargo, *tree_command[1:]],
        cwd=REPO,
        env=env,
        capture_output=True,
        text=True,
        encoding="utf-8",
        errors="replace",
    )
    if tree_result.returncode:
        output = (tree_result.stdout + tree_result.stderr)[-8000:]
        sys.exit(f"Goal 04 dependency graph failed with {tree_result.returncode}:\n{output}")
    try:
        packages, selected_features = parse_goal04_dependency_graph(tree_result.stdout)
    except (IndexError, ValueError):
        sys.exit("Goal 04 dependency graph returned invalid output")

    cargo_bloat: dict[str, object] | None = None
    if a.variant in {"full", "no-model"}:
        bloat_command = goal04_bloat_command(variant)
        bloat_result = subprocess.run(
            [cargo, *bloat_command[1:]],
            cwd=REPO,
            env=env,
            capture_output=True,
            text=True,
            encoding="utf-8",
            errors="replace",
        )
        if bloat_result.returncode:
            output = (bloat_result.stdout + bloat_result.stderr)[-8000:]
            sys.exit(f"Goal 04 cargo bloat failed with {bloat_result.returncode}:\n{output}")
        try:
            bloat = json.loads(bloat_result.stdout.splitlines()[-1])
            crates = normalize_goal04_bloat_crates(bloat["crates"], set(packages))
        except (IndexError, KeyError, TypeError, ValueError, json.JSONDecodeError):
            sys.exit("Goal 04 cargo bloat returned invalid JSON")
        version_result = subprocess.run(
            [cargo, "bloat", "--version"],
            capture_output=True,
            text=True,
            encoding="utf-8",
            errors="replace",
            check=False,
        )
        version = version_result.stdout.strip()
        if version_result.returncode or not safe_metadata_text(version):
            sys.exit("Goal 04 could not identify the cargo-bloat version")
        if any(
            not isinstance(bloat.get(field), int)
            or isinstance(bloat.get(field), bool)
            or bloat[field] <= 0
            for field in ("file-size", "text-section-size")
        ):
            sys.exit("Goal 04 cargo bloat returned invalid size fields")
        cargo_bloat = {
            "version": version,
            "command": bloat_command,
            "file_size": bloat["file-size"],
            "text_section_size": bloat["text-section-size"],
            "crates": crates,
        }
    after = source_state()
    if after != before:
        sys.exit("source state changed during Goal 04 build")
    try:
        if goal04_toolchain_context(cargo, env) != toolchain:
            sys.exit("Rust toolchain or Cargo configuration changed during Goal 04 build")
    except ValueError as error:
        sys.exit(f"Goal 04 toolchain recheck failed: {error}")

    executable_fingerprint = sha256_file(executable).evidence()
    if cargo_bloat is not None and cargo_bloat["file_size"] != executable_fingerprint[
        "byte_count"
    ]:
        sys.exit("Goal 04 cargo bloat file size does not match the executable")
    evidence: dict[str, object] = {
        "schema": STARTUP_BUILD_SCHEMA,
        "created_at": datetime.now(UTC).isoformat(),
        "variant": a.variant,
        "role": variant.role,
        "artifact_kind": variant.artifact_kind,
        "target_name": variant.target_name,
        "source": before,
        "cargo_lock": sha256_file(REPO / "Cargo.lock").evidence(),
        "target": GOAL04_TARGET,
        "profile": goal04_release_profile(variant),
        "toolchain": toolchain,
        "features": {
            "default_features": not variant.no_default_features,
            "model_transport": variant.model_transport,
        },
        "behavior": goal04_behavior(a.variant),
        "behavior_verification": behavior_verification,
        "platform_setup": goal04_platform_setup(variant),
        "build": {
            "command": command,
            "environment": {
                "CARGO_TARGET_DIR": "<redacted-path>",
                "CARGO_INCREMENTAL": "0",
                "CARGO_PROFILE_RELEASE_OPT_LEVEL": (
                    variant.opt_level if a.variant in {"opt-3", "opt-s"} else None
                ),
            },
        },
        "dependency_graph": {
            "command": tree_command,
            "packages": packages,
            "selected_features": selected_features,
            "tokio_disposition": goal04_tokio_disposition(a.variant),
        },
        "cargo_bloat": cargo_bloat,
        "executable": executable_fingerprint,
        "pe_sections": inspect_pe_sections(executable),
    }
    write_evidence(a.evidence, evidence, validate_build_evidence)
    print(
        f"Goal 04 {a.variant} build: {evidence['executable']['byte_count']} bytes  "
        f"sha256 {evidence['executable']['sha256']}  artifact {executable}"
    )
