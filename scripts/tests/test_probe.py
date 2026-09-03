"""Synthetic tests for probe geometry and measurement contracts."""

from __future__ import annotations

import argparse
import copy
import json
import subprocess
import tempfile
import unittest
from collections.abc import Iterator
from contextlib import contextmanager
from datetime import UTC, datetime, timedelta
from pathlib import Path
from unittest import mock

from scripts.markturbo_tools import goal04, metrics, probe

RECT = probe.RECT
EXPECTED_CHILD_FAILURES = probe.expected_child_failures
DURATION_US = probe.duration_us
PERCENTILE = metrics.nearest_rank_percentile
QUIET_GATE_FAILURES = probe.quiet_gate_failures


def fixture_source_state(*, dirty: bool = False) -> dict[str, object]:
    return {
        "revision": "a" * 40,
        "dirty": dirty,
        "worktree_sha256": ("b" * 64) if dirty else None,
    }


def fixture_host_context(*, active_console_session_id: int = 1) -> dict[str, object]:
    return {
        "environment": {
            "platform": "Windows 11",
            "windows_major": 10,
            "windows_minor": 0,
            "windows_build": 22631,
            "architecture": "x86_64",
            "native_machine_code": 34404,
            "python_pointer_bits": 64,
            "wts_state": "WTSActive",
            "active_console_session_id": active_console_session_id,
            "harness_is_console_session": True,
            "input_desktop": "Default",
            "thread_desktop": "Default",
            "harness_process": {
                "session_id": 1,
                "integrity_rid": 8192,
                "integrity": "medium",
            },
        },
        "hardware": {
            "processor": "Test CPU",
            "logical_processors": 8,
            "gpu": ["Test GPU"],
        },
    }


def fixture_fingerprint(seed: int) -> dict[str, object]:
    return {"byte_count": 1000 + seed, "sha256": f"{seed:064x}"}


def fixture_quiet_evidence(
    *,
    created_at: str | None = None,
    status: str | None = None,
    source: dict[str, object] | None = None,
    host: dict[str, object] | None = None,
    cpu: list[float] | None = None,
    disk: list[float] | None = None,
) -> dict[str, object]:
    cpu_values = list(cpu or [1.0, 2.0, 3.0])
    disk_values = list(disk or [0.1, 0.2, 0.3])
    thresholds = {
        "max_cpu_median_percent": 5.0,
        "max_cpu_p95_percent": 10.0,
        "max_disk_median_percent": 2.0,
        "max_disk_p95_percent": 10.0,
    }
    failures = probe.quiet_gate_failures(
        cpu_values,
        disk_values,
        thresholds["max_cpu_median_percent"],
        thresholds["max_cpu_p95_percent"],
        thresholds["max_disk_median_percent"],
        thresholds["max_disk_p95_percent"],
    )
    return {
        "schema": probe.STARTUP_QUIET_SCHEMA,
        "created_at": created_at or datetime(2026, 9, 2, 11, 59, tzinfo=UTC).isoformat(),
        "status": status or ("FAIL" if failures else "PASS"),
        "source": copy.deepcopy(source or fixture_source_state()),
        "host": copy.deepcopy(host or fixture_host_context()),
        "window": {"samples": len(cpu_values), "interval_seconds": 1.0},
        "waited_seconds": float(len(cpu_values)),
        "thresholds": thresholds,
        "samples": {"cpu_percent": cpu_values, "disk_percent": disk_values},
        "summary": {
            "cpu_median_percent": probe.median(cpu_values),
            "cpu_p95_percent": metrics.nearest_rank_percentile(cpu_values, 0.95),
            "disk_median_percent": probe.median(disk_values),
            "disk_p95_percent": metrics.nearest_rank_percentile(disk_values, 0.95),
        },
        "failures": failures,
    }


def fixture_build_evidence(
    variant_name: str,
    *,
    source: dict[str, object] | None = None,
    executable: dict[str, object] | None = None,
    created_at: str | None = None,
) -> dict[str, object]:
    variant = probe.GOAL04_BUILD_VARIANTS[variant_name]
    packages = (
        ["genai", "mt-app", "reqwest", "rustls", "tokio"]
        if variant_name == "full"
        else (
            ["gpui", "mt-app", "serde", "tokio"]
            if variant_name == "no-model"
            else ["gpui"]
        )
    )
    selected_features = {
        name: (
            ["default"]
            if name != "tokio"
            else (["net", "rt-multi-thread", "time"] if variant_name == "full" else ["rt", "time"])
        )
        for name in packages
        if name in {"genai", "reqwest", "rustls", "tokio", "hyper", "tokio-rustls"}
    }
    executable_value = copy.deepcopy(executable or fixture_fingerprint(100 + len(variant_name)))
    cargo_bloat = None
    if variant_name in {"full", "no-model"}:
        cargo_bloat = {
            "version": "cargo-bloat 0.12.1",
            "command": probe.goal04_bloat_command(variant),
            "file_size": executable_value["byte_count"],
            "text_section_size": min(65432, executable_value["byte_count"]),
            "crates": ([{"name": "genai", "size": 6000}, {"name": "reqwest", "size": 5000}, {"name": "rustls?", "size": 4000}] if variant_name == "full" else [{"name": "mt-app", "size": 1234}]),
        }
    return {
        "schema": probe.STARTUP_BUILD_SCHEMA,
        "created_at": created_at or datetime(2026, 9, 2, 11, 0, tzinfo=UTC).isoformat(),
        "variant": variant_name,
        "role": variant.role,
        "artifact_kind": variant.artifact_kind,
        "target_name": variant.target_name,
        "source": copy.deepcopy(source or fixture_source_state()),
        "cargo_lock": fixture_fingerprint(900),
        "target": probe.GOAL04_TARGET,
        "profile": probe.goal04_release_profile(variant),
        "toolchain": {
            "cargo": {
                "command": ["cargo", "-Vv"],
                "release": "1.90.0",
                "commit_hash": "1" * 40,
                "commit_date": "2026-08-01",
                "host": probe.GOAL04_TARGET,
            },
            "rustc": {
                "command": ["rustc", "-vV"],
                "release": "1.90.0",
                "commit_hash": "2" * 40,
                "commit_date": "2026-08-01",
                "host": probe.GOAL04_TARGET,
                "llvm_version": "22.1.0",
            },
            "cargo_config": {"cargo_home_overridden": False, "files": []},
        },
        "features": {
            "default_features": not variant.no_default_features,
            "model_transport": variant.model_transport,
        },
        "behavior": probe.goal04_behavior(variant_name),
        "behavior_verification": (
            {
                "command": probe.goal04_behavior_verification_command(variant_name),
                "passed": 1,
                "failed": 0,
            }
            if variant_name == "no-model"
            else None
        ),
        "platform_setup": probe.goal04_platform_setup(variant),
        "build": {
            "command": probe.goal04_build_command(variant),
            "environment": {
                "CARGO_TARGET_DIR": "<redacted-path>",
                "CARGO_INCREMENTAL": "0",
                "CARGO_PROFILE_RELEASE_OPT_LEVEL": (
                    variant.opt_level if variant_name in {"opt-3", "opt-s"} else None
                ),
            },
        },
        "dependency_graph": {
            "command": probe.goal04_tree_command(variant),
            "packages": packages,
            "selected_features": selected_features,
            "tokio_disposition": probe.goal04_tokio_disposition(variant_name),
        },
        "cargo_bloat": cargo_bloat,
        "executable": executable_value,
        "pe_sections": [
            {
                "name": ".text",
                "virtual_size": 4096,
                "raw_size": 4096,
                "characteristics": 1610612768,
            }
        ],
    }

def fixture_threshold_evidence(
    *,
    created_at: str | None = None,
    source: dict[str, object] | None = None,
    status: str = "APPROVED",
    approved_by: str = "project-owner",
) -> dict[str, object]:
    return {
        "schema": probe.STARTUP_THRESHOLD_SCHEMA,
        "created_at": created_at or datetime(2026, 9, 2, 11, 50, tzinfo=UTC).isoformat(),
        "status": status,
        "approved_by": approved_by,
        "scope": "model-transport",
        "source": copy.deepcopy(source or fixture_source_state()),
        "materiality": {
            "launch": {
                "milestones": ["first_frame_painted_ms", "first_input_handled_ms"],
                "required_absolute_improvement_ms": 50.0,
                "required_relative_improvement_percent": 10.0,
                "max_p95_regression_percent": 5.0,
            },
            "idle_private": {
                "required_absolute_improvement_mb": 20.0,
                "required_relative_improvement_percent": 10.0,
            },
            "decision_rule": (
                "launch or idle_private; every selected path must satisfy both its absolute "
                "and relative threshold"
            ),
            "cache_rule": (
                "warm and fresh-profile evidence must each meet the materiality rule"
            ),
        },
        "fallback_decision": "keep in-process",
    }


def fixture_startup_sample(initial_state: str, *, base: float = 1.0) -> probe.StartupSample:
    return probe.StartupSample(
        process_created_ms=base,
        process_started_ms=base + 1.0,
        initial_state_ready_ms=base + 2.0,
        window_visible_ms=base + 2.5,
        first_frame_painted_ms=base + 3.0,
        first_input_handled_ms=base + 4.0,
        initial_state=initial_state,
        idle_working_set_mb=base + 5.0,
        idle_private_mb=base + 6.0,
        peak_working_set_mb=base + 7.0,
        page_faults=int(base + 8),
        threads=int(base + 9),
    )


def fixture_preflight(executable: dict[str, object], host: dict[str, object]) -> dict[str, object]:
    return {
        "executable": {
            "byte_count": executable["byte_count"],
            "sha256": executable["sha256"],
            "format": "PE32+",
            "machine": "x86_64",
            "hash_verified": True,
        },
        "environment": copy.deepcopy(host["environment"]),
    }


def fixture_startup_evidence(
    labels: tuple[str, ...] = ("bare",),
    *,
    rounds: int = 10,
    source: dict[str, object] | None = None,
    host: dict[str, object] | None = None,
    measurement_started_at: datetime | None = None,
    cache_state: str = "warm",
) -> dict[str, object]:
    source_value = copy.deepcopy(source or fixture_source_state())
    host_value = copy.deepcopy(host or fixture_host_context())
    started_at = measurement_started_at or datetime(2026, 9, 2, 12, 0, tzinfo=UTC)
    quiet = fixture_quiet_evidence(
        created_at=(started_at - timedelta(minutes=1)).isoformat(),
        source=source_value,
        host=host_value,
    )
    state_map = {
        "bare": "bare",
        "full": "welcome",
        "no-model": "welcome",
        "opt-3": "welcome",
        "opt-s": "welcome",
    }
    variants: list[dict[str, object]] = []
    preflight: dict[str, object] = {}
    parsed_samples: list[tuple[probe.StartupSample, ...]] = []
    sample_count = rounds * (2 if len(labels) == 2 else 1)
    for index, label in enumerate(labels, start=1):
        executable = fixture_fingerprint(200 + index)
        build = fixture_build_evidence(label, source=source_value, executable=executable)
        samples = tuple(
            fixture_startup_sample(state_map[label], base=10.0 + index) for _ in range(sample_count)
        )
        variants.append(
            {
                "label": label,
                "executable": copy.deepcopy(executable),
                "build": build,
                "samples": [sample.evidence() for sample in samples],
                "summary": probe.startup_summary(samples),
            }
        )
        preflight[label] = fixture_preflight(executable, host_value)
        parsed_samples.append(samples)

    evidence: dict[str, object] = {
        "schema": probe.STARTUP_EVIDENCE_SCHEMA,
        "created_at": (started_at + timedelta(minutes=1)).isoformat(),
        "measurement_started_at": started_at.isoformat(),
        "command": ["probe.py", "startup", "--milestones"],
        "source": source_value,
        "target": probe.GOAL04_TARGET,
        "profile": "release",
        "host": host_value,
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
        "warmup_launches_per_variant": 2,
        "idle_settle_seconds": 1.0,
        "instrumentation": {
            "schema": probe.STARTUP_TRACE_SCHEMA,
            "input": "Win32 SendInput VK_F24 -> GPUI action acknowledgement",
            "first_frame": (
                "GPUI post-render callback for the first application frame; "
                "not a DWM presentation timestamp"
            ),
            "comparison_scope": "enabled identically for every compared variant",
        },
        "decision_scope": "model-transport" if labels == ("full", "no-model") else "diagnostic",
        "threshold": None,
        "threshold_evaluation": None,
        "quiet_gate": quiet,
        "preflight": preflight,
        "variants": variants,
    }
    if labels == ("full", "no-model"):
        threshold = fixture_threshold_evidence(
            created_at=(started_at - timedelta(minutes=2)).isoformat(),
            source=source_value,
        )
        evidence["threshold"] = {
            "artifact": fixture_fingerprint(700),
            "record": probe.canonical_threshold_evidence(threshold),
        }
    if len(parsed_samples) == 2:
        evidence["comparison"] = probe.milestone_comparison(parsed_samples[0], parsed_samples[1])
    if labels == ("full", "no-model"):
        evidence["threshold_evaluation"] = probe.evaluate_model_threshold(
            parsed_samples[0], parsed_samples[1], evidence["threshold"]
        )
    return evidence

def fixture_model_first_use_evidence() -> dict[str, object]:
    source = fixture_source_state()
    host = fixture_host_context()
    started_at = datetime(2026, 9, 2, 12, 0, tzinfo=UTC)
    quiet = fixture_quiet_evidence(
        created_at=(started_at - timedelta(minutes=1)).isoformat(),
        source=source,
        host=host,
    )
    full_build = fixture_build_evidence("full", source=source, executable=fixture_fingerprint(801))
    test_build = fixture_build_evidence(
        "model-first-use",
        source=source,
        executable=fixture_fingerprint(802),
    )
    samples = [100.0 + index for index in range(10)]
    return {
        "schema": probe.MODEL_FIRST_USE_EVIDENCE_SCHEMA,
        "created_at": (started_at + timedelta(minutes=1)).isoformat(),
        "measurement_started_at": started_at.isoformat(),
        "command": ["probe.py", "model-first-use"],
        "source": source,
        "host": host,
        "target": probe.GOAL04_TARGET,
        "profile": "release",
        "rounds": 10,
        "warmup_runs": 2,
        "cache_state": probe.model_first_use_cache_state(2),
        "quiet_gate": quiet,
        "full_application_build": full_build,
        "test_build": test_build,
        "executable": copy.deepcopy(test_build["executable"]),
        "samples_us": samples,
        "summary": {
            "median_us": probe.median(samples),
            "p95_us": metrics.nearest_rank_percentile(samples, 0.95),
        },
        "measurement": {
            "protocol": "first transport initialization plus first loopback HTTP request",
            "subsequent_request": "executed only to preserve the test contract; not reported",
            "ablated_result": "not applicable; model transport is absent",
        },
    }


def fixture_model_transport_decision_evidence() -> dict[str, object]:
    warm = fixture_startup_evidence(("full", "no-model"), cache_state="warm")
    fresh = fixture_startup_evidence(
        ("full", "no-model"), cache_state="fresh-profile"
    )
    variants = warm["variants"]
    assert isinstance(variants, list)

    def input_summary(
        evidence: dict[str, object], cache_state: str, seed: int
    ) -> dict[str, object]:
        quiet = evidence["quiet_gate"]
        evaluation = evidence["threshold_evaluation"]
        assert isinstance(quiet, dict)
        assert isinstance(evaluation, dict)
        return {
            "artifact": fixture_fingerprint(seed),
            "cache_state": cache_state,
            "startup_created_at": evidence["created_at"],
            "quiet_gate_created_at": quiet["created_at"],
            "materiality_met": evaluation["materiality_met"],
        }

    warm_input = input_summary(warm, "warm", 901)
    fresh_input = input_summary(fresh, "fresh-profile", 902)
    return {
        "schema": probe.MODEL_TRANSPORT_DECISION_SCHEMA,
        "created_at": datetime(2026, 9, 2, 13, 0, tzinfo=UTC).isoformat(),
        "status": "APPROVED",
        "approved_by": "project-owner",
        "command": ["probe.py", "decide-goal04", "--owner-approved"],
        "source": warm["source"],
        "host": warm["host"],
        "threshold": warm["threshold"],
        "inputs": {"warm": warm_input, "fresh_profile": fresh_input},
        "variant_fingerprints": {
            variant["label"]: {
                "byte_count": variant["executable"]["byte_count"],
                "sha256": variant["executable"]["sha256"],
            }
            for variant in variants
        },
        "materiality": {
            "warm": warm_input["materiality_met"],
            "fresh_profile": fresh_input["materiality_met"],
            "both": False,
        },
        "decision": "keep in-process",
        "authorization": "no model transport extraction authorized",
    }


def fingerprint_mock(value: dict[str, object]) -> mock.Mock:
    return mock.Mock(evidence=mock.Mock(return_value=value))


@contextmanager
def goal04_contract() -> Iterator[None]:
    with mock.patch.object(
        goal04, "rust_target", return_value=probe.GOAL04_TARGET
    ):
        yield


@contextmanager
def build_manifest(seed: int) -> Iterator[tuple[dict[str, object], dict[str, object], Path, Path]]:
    source = fixture_source_state()
    evidence = fixture_build_evidence(
        "full", source=source, executable=fixture_fingerprint(seed)
    )
    with tempfile.TemporaryDirectory() as directory:
        manifest = Path(directory) / "full.json"
        executable = Path(directory) / "markturbo.exe"
        manifest.write_text(json.dumps(evidence), encoding="utf-8")
        executable.write_bytes(b"exe")
        yield source, evidence, manifest, executable


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


class StartupTraceTests(unittest.TestCase):
    @staticmethod
    def trace(*rows: dict[str, object]) -> str:
        return "\n".join(json.dumps(row) for row in rows) + "\n"

    @staticmethod
    def event(name: str, counter: int, *, detail: str | None = None) -> dict[str, object]:
        row: dict[str, object] = {
            "schema": "markturbo-startup-v1",
            "nonce": "run-1",
            "pid": 42,
            "event": name,
            "counter": counter,
            "frequency": 1_000,
        }
        if detail is not None:
            row["detail"] = detail
        return row

    def test_parses_complete_ordered_content_free_trace(self) -> None:
        events = probe.parse_startup_trace(
            self.trace(
                self.event("process_started", 1_010),
                self.event("initial_state_ready", 1_020, detail="welcome"),
                self.event("first_frame_painted", 1_040),
                self.event("first_input_handled", 1_070),
            ),
            nonce="run-1",
            pid=42,
            frequency=1_000,
        )

        self.assertEqual(
            probe.trace_milestones(events, start_counter=1_000, frequency=1_000),
            {
                "process_started_ms": 10.0,
                "initial_state_ready_ms": 20.0,
                "first_frame_painted_ms": 40.0,
                "first_input_handled_ms": 70.0,
            },
        )
        self.assertEqual(events["initial_state_ready"].detail, "welcome")

    def test_rejects_duplicate_missing_or_foreign_events(self) -> None:
        duplicate = self.trace(
            self.event("process_started", 1),
            self.event("process_started", 2),
        )
        with self.assertRaisesRegex(ValueError, "duplicate startup event"):
            probe.parse_startup_trace(
                duplicate, nonce="run-1", pid=42, frequency=1_000
            )

        foreign = self.event("process_started", 1)
        foreign["nonce"] = "other"
        with self.assertRaisesRegex(ValueError, "nonce"):
            probe.parse_startup_trace(
                self.trace(foreign), nonce="run-1", pid=42, frequency=1_000
            )

        incomplete = probe.parse_startup_trace(
            self.trace(self.event("process_started", 1)),
            nonce="run-1",
            pid=42,
            frequency=1_000,
        )
        with self.assertRaisesRegex(ValueError, "missing startup event"):
            probe.trace_milestones(incomplete, start_counter=0, frequency=1_000)

    def test_rejects_out_of_order_or_mismatched_counter_frequency(self) -> None:
        out_of_order = probe.parse_startup_trace(
            self.trace(
                self.event("process_started", 10),
                self.event("initial_state_ready", 30),
                self.event("first_frame_painted", 20),
                self.event("first_input_handled", 40),
            ),
            nonce="run-1",
            pid=42,
            frequency=1_000,
        )
        with self.assertRaisesRegex(ValueError, "event order"):
            probe.trace_milestones(out_of_order, start_counter=0, frequency=1_000)

        wrong_frequency = self.event("process_started", 10)
        wrong_frequency["frequency"] = 10_000
        with self.assertRaisesRegex(ValueError, "frequency"):
            probe.parse_startup_trace(
                self.trace(wrong_frequency),
                nonce="run-1",
                pid=42,
                frequency=1_000,
            )

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

    def test_trace_reader_keeps_partial_rows_and_only_parses_new_bytes(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "trace.jsonl"
            reader = probe.StartupTraceReader(
                path, nonce="run-1", pid=42, frequency=1_000
            )
            self.assertEqual(reader.read(), {})

            first = json.dumps(self.event("process_started", 10))
            path.write_text(first, encoding="utf-8")
            self.assertEqual(reader.read(), {})
            with path.open("a", encoding="utf-8") as stream:
                stream.write("\n" + json.dumps(self.event("initial_state_ready", 20)) + "\n")

            events = reader.read()
            reader.close()
            self.assertEqual(set(events), {"process_started", "initial_state_ready"})

    def test_evidence_command_redacts_every_path_argument(self) -> None:
        argv = [
            "Q:/repo/scripts/markturbo_tools/probe.py",
            "startup",
            "--exe",
            "private/full.exe",
            "--compare=private/no-model.exe",
            "--open",
            "secrets/brief.md",
            "--label",
            "full",
        ]
        with mock.patch.object(probe.sys, "argv", argv):
            command = probe.safe_command()

        self.assertEqual(command[0], "probe.py")
        self.assertNotIn("private", " ".join(command))
        self.assertNotIn("secrets", " ".join(command))
        self.assertIn("full", command)


class StartupQuietEvidenceTests(unittest.TestCase):
    def test_validate_startup_quiet_evidence_rejects_invalid_states(self) -> None:
        checked_at = datetime(2026, 9, 2, 12, 0, tzinfo=UTC)
        source = fixture_source_state()
        host = fixture_host_context()
        cases = [
            (
                "fail status",
                fixture_quiet_evidence(status="FAIL", source=source, host=host),
                "status does not match its samples",
            ),
            (
                "source mismatch",
                fixture_quiet_evidence(
                    source=fixture_source_state(dirty=True),
                    host=host,
                ),
                "different source state",
            ),
            (
                "host mismatch",
                fixture_quiet_evidence(
                    source=source,
                    host=fixture_host_context(active_console_session_id=2),
                ),
                "different host or session",
            ),
            (
                "stale",
                fixture_quiet_evidence(
                    created_at=(checked_at - probe.QUIET_EVIDENCE_MAX_AGE - timedelta(seconds=1)).isoformat(),
                    source=source,
                    host=host,
                ),
                "stale",
            ),
            (
                "future",
                fixture_quiet_evidence(
                    created_at=(checked_at + timedelta(seconds=1)).isoformat(),
                    source=source,
                    host=host,
                ),
                "stale",
            ),
        ]

        for label, evidence, message in cases:
            with self.subTest(label=label):
                with self.assertRaisesRegex(ValueError, message):
                    probe.validate_startup_quiet_evidence(
                        evidence,
                        source=source,
                        host=host,
                        checked_at=checked_at,
                    )

    def test_normalized_quiet_evidence_discards_extra_fields(self) -> None:
        evidence = fixture_quiet_evidence()
        evidence["secret"] = "token"
        evidence["path"] = "C:/private/run.json"
        evidence["window"]["path"] = "C:/private/window"
        evidence["thresholds"]["secret"] = "threshold-token"
        evidence["samples"]["path"] = "C:/private/samples"
        evidence["summary"]["secret"] = "summary-token"
        evidence["failures"] = []

        normalized = probe.normalized_quiet_evidence(evidence)

        self.assertEqual(
            normalized,
            fixture_quiet_evidence(
                created_at=evidence["created_at"],
                source=evidence["source"],
                host=evidence["host"],
            ),
        )
        serialized = json.dumps(normalized, sort_keys=True)
        self.assertNotIn("token", serialized)
        self.assertNotIn("private", serialized)


class StartupCommandTests(unittest.TestCase):
    @staticmethod
    def sample() -> probe.StartupSample:
        return fixture_startup_sample("welcome")

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
                return self.sample()

            with (
                mock.patch.object(native_runtime, "sha256_file", return_value=mock.Mock(sha256="hash")),
                mock.patch.object(native_runtime, "preflight", side_effect=lambda *args, **kwargs: (object(), object())),
                mock.patch.object(probe, "startup_milestones_once", side_effect=fake_measure),
                mock.patch.object(probe, "summarize_startup_milestones"),
                mock.patch.object(probe, "milestone_comparison", return_value={}),
            ):
                probe.cmd_startup_milestones(
                    self.args(exe_a, exe_b, cache_state), [exe_a, exe_b]
                )

        return calls

    def test_cmd_startup_warm_mode_reuses_distinct_profile_roots_per_variant(self) -> None:
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

    def test_cmd_startup_fresh_profile_mode_passes_no_profile_root(self) -> None:
        roots = [profile_root for _, profile_root in self.profile_calls("fresh-profile")]
        self.assertEqual(roots, [None] * 10)

    def test_paired_startup_cannot_bypass_formal_labels_or_milestones(self) -> None:
        args = self.args(Path("a.exe"), Path("b.exe"), "warm")
        with self.assertRaisesRegex(SystemExit, "recognized variant labels"):
            probe.cmd_startup(args)

        args.milestones = False
        with self.assertRaisesRegex(SystemExit, "require --milestones"):
            probe.cmd_startup(args)

    def test_no_model_comparison_cannot_use_the_opt_3_alias(self) -> None:
        args = self.args(Path("opt-3.exe"), Path("no-model.exe"), "warm")
        args.label = "opt-3"
        args.compare_label = "no-model"

        with self.assertRaisesRegex(SystemExit, "requires --label full"):
            probe.cmd_startup(args)

    def test_startup_rejects_toolchain_mismatch_before_measurement(self) -> None:
        args = self.args(Path("full.exe"), Path("no-model.exe"), "warm")
        args.label = "full"
        args.compare_label = "no-model"
        args.build_evidence = Path("full-build.json")
        args.compare_build_evidence = Path("no-model-build.json")
        full = fixture_build_evidence("full")
        no_model = fixture_build_evidence("no-model")
        no_model["toolchain"]["rustc"]["release"] = "1.91.0"

        with (
            mock.patch.object(probe, "source_state", return_value=fixture_source_state()),
            mock.patch.object(
                probe, "load_build_evidence", side_effect=[full, no_model]
            ),
            mock.patch.object(probe, "startup_milestones_once") as measure,
        ):
            with self.assertRaisesRegex(SystemExit, "different Rust toolchains"):
                probe.cmd_startup_milestones(args, [args.exe, args.compare])

        measure.assert_not_called()

    def test_startup_evidence_cannot_overwrite_the_open_document(self) -> None:
        args = self.args(Path("full.exe"), Path("no-model.exe"), "warm")
        args.rounds = 10
        args.label = "full"
        args.compare_label = "no-model"
        args.build_evidence = Path("full-build.json")
        args.compare_build_evidence = Path("no-model-build.json")
        args.quiet_evidence = Path("quiet.json")
        args.threshold_evidence = Path("threshold.json")
        args.open = "document.md"
        args.evidence = Path("document.md")

        with self.assertRaisesRegex(SystemExit, "must differ"):
            probe.cmd_startup(args)


class ProcessCleanupTests(unittest.TestCase):
    def test_kill_and_wait_converts_timeout_to_runtime_error(self) -> None:
        process = mock.Mock()
        process.poll.return_value = None
        process.wait.side_effect = subprocess.TimeoutExpired(cmd="probe", timeout=1.5)

        with self.assertRaisesRegex(RuntimeError, "probe process did not exit after termination"):
            probe.kill_and_wait(process, timeout=1.5)

        process.kill.assert_called_once_with()

    def test_evidence_output_cannot_overwrite_an_input(self) -> None:
        path = Path("same.json")
        with self.assertRaisesRegex(ValueError, "must differ"):
            probe.require_distinct_output_path(path, path)


class Goal04BuildEvidenceTests(unittest.TestCase):
    @staticmethod
    def load(
        manifest: Path,
        *,
        variant_name: str,
        source: dict[str, object],
        executable: Path,
        evidence: dict[str, object],
        actual_executable: dict[str, object] | None = None,
    ) -> dict[str, object]:
        from scripts.markturbo_tools.native import runtime as native_runtime

        def sha256_file(path: Path) -> mock.Mock:
            if Path(path).name == "Cargo.lock":
                return fingerprint_mock(evidence["cargo_lock"])
            if Path(path) == executable:
                return fingerprint_mock(actual_executable or evidence["executable"])
            raise AssertionError(path)

        with (
            goal04_contract(),
            mock.patch.object(native_runtime, "sha256_file", side_effect=sha256_file),
        ):
            return probe.load_build_evidence(
                manifest,
                variant_name=variant_name,
                source=source,
                executable=executable,
            )

    def test_validate_build_evidence_rejects_feature_mismatch(self) -> None:
        evidence = fixture_build_evidence("no-model")
        evidence["features"]["default_features"] = True

        with goal04_contract():
            with self.assertRaisesRegex(ValueError, "feature set"):
                probe.validate_build_evidence(evidence)

    def test_validate_build_evidence_requires_no_model_behavior_verification(self) -> None:
        evidence = fixture_build_evidence("no-model")
        evidence["behavior_verification"] = None

        with goal04_contract():
            with self.assertRaisesRegex(ValueError, "behavior verification"):
                probe.validate_build_evidence(evidence)

    def test_cargo_config_context_records_only_content_free_fingerprints(self) -> None:
        from scripts.markturbo_tools import goal04

        with tempfile.TemporaryDirectory() as directory:
            cargo_home = Path(directory)
            config = cargo_home / "config.toml"
            config.write_text(
                '[target.x86_64-pc-windows-msvc]\nlinker = "C:/secret/tool.exe"\n',
                encoding="utf-8",
            )
            context = goal04.goal04_cargo_config_context({"CARGO_HOME": directory})

        self.assertTrue(context["cargo_home_overridden"])
        self.assertEqual(len(context["files"]), 1)
        self.assertEqual(context["files"][0]["scope"], "cargo-home")
        self.assertNotIn("secret", repr(context))
        self.assertNotIn(directory, repr(context))

    def test_toolchain_parsers_reject_unknown_output_lines(self) -> None:
        from scripts.markturbo_tools import goal04

        cargo_output = "\n".join(
            [
                "cargo 1.98.0 (797e8a9bc 2026-08-05)",
                "release: 1.98.0",
                f"commit-hash: {'7' * 40}",
                "commit-date: 2026-08-05",
                f"host: {probe.GOAL04_TARGET}",
                "libgit2: 1.9.4 (sys:0.21.0 vendored)",
                "libcurl: 8.21.0-DEV (sys:0.4.90+curl-8.21.0 vendored ssl:Schannel)",
                "os: Windows 10.0.26200 (Windows 11 Enterprise) [64-bit]",
            ]
        )
        cargo_output = cargo_output.replace("7" * 9, "797e8a9bc", 1)
        parsed = goal04.parse_goal04_cargo_version(cargo_output)
        self.assertEqual(parsed["release"], "1.98.0")

        with self.assertRaisesRegex(ValueError, "Cargo version output"):
            goal04.parse_goal04_cargo_version(
                cargo_output + "\nOPENAI_API_KEY=private-token\n"
            )

    def test_build_environment_removes_toolchain_and_target_overrides(self) -> None:
        from scripts.markturbo_tools import goal04

        overrides = {
            "CARGO_BUILD_RUSTC": "private-rustc",
            "CARGO_BUILD_RUSTC_WRAPPER": "private-wrapper",
            "CARGO_BUILD_RUSTFLAGS": "private-flags",
            "CARGO_TARGET_X86_64_PC_WINDOWS_MSVC_LINKER": "private-linker",
            "CARGO_PROFILE_RELEASE_OPT_LEVEL": "0",
            "RUSTC": "private-rustc",
            "RUSTFLAGS": "private-flags",
            "RUSTUP_TOOLCHAIN": "private-toolchain",
            "CARGO_HOME": "C:/cargo-home",
        }
        with mock.patch.dict(goal04.os.environ, overrides, clear=True):
            env = goal04.goal04_build_environment(Path("target-dir"), "opt-s")

        for name in overrides:
            if name not in {"CARGO_HOME", "CARGO_PROFILE_RELEASE_OPT_LEVEL"}:
                self.assertNotIn(name, env)
        self.assertEqual(env["CARGO_HOME"], "C:/cargo-home")
        self.assertEqual(env["CARGO_INCREMENTAL"], "0")
        self.assertEqual(env["CARGO_PROFILE_RELEASE_OPT_LEVEL"], "s")

    def test_dependency_graph_parser_ignores_cargo_dedupe_markers(self) -> None:
        packages, features = probe.parse_goal04_dependency_graph(
            "reqwest v0.13.4|__rustls,http2\n"
            "tokio v1.53.1|net,rt-multi-thread,time (*)\n"
            "tokio v1.53.1|net,rt-multi-thread,time\n"
        )

        self.assertEqual(packages, ["reqwest", "tokio"])
        self.assertEqual(features["reqwest"], ["__rustls", "http2"])
        self.assertEqual(features["tokio"], ["net", "rt-multi-thread", "time"])

    def test_bloat_normalization_redacts_non_crate_msvc_attribution_labels(self) -> None:
        crates = probe.normalize_goal04_bloat_crates(
            [
                {"name": "genai", "size": 6000},
                {"name": "", "size": 582291},
                {"name": "enum2$<gpui_component", "size": 98244},
                {"name": "sk-proj-private-token", "size": 30},
                {"name": "genai?", "size": 20},
                {"name": "gpui_component", "size": 100},
            ],
            {"genai", "gpui-component"},
        )

        self.assertIn({"name": "[Unknown]", "size": 582291}, crates)
        self.assertIn(
            {"name": "[Other MSVC attribution]", "size": 98274}, crates
        )
        self.assertIn({"name": "genai?", "size": 20}, crates)
        self.assertIn({"name": "gpui-component", "size": 100}, crates)
        self.assertNotIn("enum2$<gpui_component", repr(crates))
        self.assertNotIn("private-token", repr(crates))

        no_model = probe.normalize_goal04_bloat_crates(
            [{"name": "genai?", "size": 20}], {"gpui", "mt_app"}
        )
        self.assertEqual(
            no_model, [{"name": "[Other MSVC attribution]", "size": 20}]
        )

    def test_validate_bloat_accepts_only_normalized_attribution_labels(self) -> None:
        evidence = fixture_build_evidence("full")
        evidence["cargo_bloat"]["crates"].extend(
            [
                {"name": "[Unknown]", "size": 582291},
                {"name": "[Other MSVC attribution]", "size": 98274},
            ]
        )

        with goal04_contract():
            probe.validate_build_evidence(evidence)

            evidence["cargo_bloat"]["crates"][-1]["name"] = "sk-proj-private-token"
            with self.assertRaisesRegex(ValueError, "invalid cargo-bloat crate record"):
                probe.validate_build_evidence(evidence)

    def test_load_build_evidence_accepts_canonical_manifest(self) -> None:
        with build_manifest(301) as (source, canonical, manifest, executable):
            loaded = self.load(
                manifest,
                variant_name="full",
                source=source,
                executable=executable,
                evidence=canonical,
            )

        self.assertEqual(loaded, canonical)

    def test_load_build_evidence_rejects_variant_source_and_hash_mismatch(self) -> None:
        with build_manifest(302) as (source, canonical, manifest, executable):
            with self.assertRaisesRegex(ValueError, "variant does not match its label"):
                self.load(
                    manifest,
                    variant_name="no-model",
                    source=source,
                    executable=executable,
                    evidence=canonical,
                )
            with self.assertRaisesRegex(ValueError, "different source state"):
                self.load(
                    manifest,
                    variant_name="full",
                    source=fixture_source_state(dirty=True),
                    executable=executable,
                    evidence=canonical,
                )
            with self.assertRaisesRegex(ValueError, "executable fingerprint does not match"):
                self.load(
                    manifest,
                    variant_name="full",
                    source=source,
                    executable=executable,
                    evidence=canonical,
                    actual_executable=fixture_fingerprint(999),
                )


class StartupEvidenceContractTests(unittest.TestCase):
    def test_validate_startup_evidence_rejects_warm_mode_without_warmup(self) -> None:
        evidence = fixture_startup_evidence(("full", "no-model"), cache_state="warm")
        evidence["warmup_launches_per_variant"] = 0

        with goal04_contract():
            with self.assertRaisesRegex(ValueError, "measurement controls"):
                probe.validate_startup_evidence(evidence)

    def test_validate_startup_evidence_rejects_non_string_cache_state(self) -> None:
        evidence = fixture_startup_evidence(("full", "no-model"), cache_state="warm")
        evidence["cache_state"] = []

        with goal04_contract():
            with self.assertRaisesRegex(ValueError, "cache contract"):
                probe.validate_startup_evidence(evidence)

    def test_validate_startup_evidence_rejects_no_model_alias_pair(self) -> None:
        evidence = fixture_startup_evidence(("opt-3", "no-model"))

        with goal04_contract():
            with self.assertRaisesRegex(ValueError, "full/no-model decision pair"):
                probe.validate_startup_evidence(evidence)

    def test_validate_startup_evidence_requires_matching_toolchains(self) -> None:
        evidence = fixture_startup_evidence(("full", "no-model"))
        evidence["variants"][1]["build"]["toolchain"]["rustc"]["release"] = "1.91.0"

        with goal04_contract():
            with self.assertRaisesRegex(ValueError, "different Rust toolchains"):
                probe.validate_startup_evidence(evidence)

    def test_validate_startup_evidence_rejects_unknown_label(self) -> None:
        evidence = fixture_startup_evidence(("bare",))
        evidence["variants"][0]["label"] = "mystery"
        evidence["preflight"] = {"mystery": evidence["preflight"]["bare"]}

        with goal04_contract():
            with self.assertRaisesRegex(ValueError, "distinct variant labels"):
                probe.validate_startup_evidence(evidence)

    def test_validate_startup_evidence_rejects_bad_digest_and_negative_bytes(self) -> None:
        with goal04_contract():
            for label, mutator in [
                (
                    "negative bytes",
                    lambda evidence: evidence["threshold"]["artifact"].update(byte_count=-1),
                ),
                (
                    "bad digest",
                    lambda evidence: evidence["threshold"]["artifact"].update(sha256="not-a-digest"),
                ),
            ]:
                with self.subTest(label=label):
                    evidence = fixture_startup_evidence(("full", "no-model"))
                    mutator(evidence)
                    with self.assertRaisesRegex(ValueError, "invalid threshold artifact fingerprint"):
                        probe.validate_startup_evidence(evidence)

    def test_validate_startup_evidence_rejects_invalid_sample_shape_or_order(self) -> None:
        with goal04_contract():
            evidence = fixture_startup_evidence(("bare",))
            invalid_shape = copy.deepcopy(evidence)
            invalid_shape["variants"][0]["samples"][0].pop("threads")
            with self.assertRaisesRegex(ValueError, "invalid sample shape"):
                probe.validate_startup_evidence(invalid_shape)

            invalid_order = copy.deepcopy(evidence)
            invalid_order["variants"][0]["samples"][0]["first_frame_painted_ms"] = 12.5
            with self.assertRaisesRegex(ValueError, "milestone order"):
                probe.validate_startup_evidence(invalid_order)

    def test_validate_startup_evidence_rejects_forged_summary_or_comparison(self) -> None:
        with goal04_contract():
            evidence = fixture_startup_evidence(("bare", "opt-s"))
            forged_summary = copy.deepcopy(evidence)
            forged_summary["variants"][0]["summary"] = {"process_created_ms": {"median": 0.0, "p95": 0.0}}
            with self.assertRaisesRegex(ValueError, "summary does not match"):
                probe.validate_startup_evidence(forged_summary)

            forged_comparison = copy.deepcopy(evidence)
            forged_comparison["comparison"] = {}
            with self.assertRaisesRegex(ValueError, "comparison does not match"):
                probe.validate_startup_evidence(forged_comparison)


class ThresholdEvidenceTests(unittest.TestCase):
    def test_validate_threshold_evidence_rejects_unapproved(self) -> None:
        evidence = fixture_threshold_evidence(status="PENDING")
        with self.assertRaisesRegex(ValueError, "not owner-approved"):
            probe.validate_threshold_evidence(evidence)

    def test_load_threshold_evidence_rejects_post_measure_and_source_mismatch(self) -> None:
        from scripts.markturbo_tools.native import runtime as native_runtime

        source = fixture_source_state()
        canonical = probe.canonical_threshold_evidence(fixture_threshold_evidence(source=source))

        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "threshold.json"
            path.write_text(json.dumps(canonical), encoding="utf-8")

            with mock.patch.object(native_runtime, "sha256_file", return_value=fingerprint_mock(fixture_fingerprint(701))):
                with self.assertRaisesRegex(ValueError, "different source state"):
                    probe.load_threshold_evidence(
                        path,
                        source=fixture_source_state(dirty=True),
                        checked_at=datetime(2026, 9, 2, 12, 0, tzinfo=UTC),
                    )
                with self.assertRaisesRegex(ValueError, "created after measurement began"):
                    probe.load_threshold_evidence(
                        path,
                        source=source,
                        checked_at=datetime(2026, 9, 2, 11, 49, tzinfo=UTC),
                    )


class ModelFirstUseEvidenceTests(unittest.TestCase):
    def test_command_rejects_toolchain_mismatch_before_measurement(self) -> None:
        source = fixture_source_state()
        test_build = fixture_build_evidence("model-first-use", source=source)
        full_build = fixture_build_evidence("full", source=source)
        test_build["toolchain"]["cargo"]["release"] = "1.91.0"

        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            executable = root / "model-first-use.exe"
            app_executable = root / "markturbo.exe"
            executable.write_bytes(b"test")
            app_executable.write_bytes(b"app")
            args = argparse.Namespace(
                rounds=10,
                warmup=0,
                timeout=30.0,
                exe=executable,
                evidence=root / "model-first-use.json",
                quiet_evidence=root / "quiet.json",
                build_evidence=root / "model-first-use-build.json",
                app_exe=app_executable,
                app_build_evidence=root / "full-build.json",
            )
            with (
                mock.patch.object(probe, "source_state", return_value=source),
                mock.patch.object(
                    probe, "goal04_host_context", return_value=fixture_host_context()
                ),
                mock.patch.object(probe, "read_evidence_object", return_value={}),
                mock.patch.object(probe, "validate_startup_quiet_evidence"),
                mock.patch.object(probe, "normalized_quiet_evidence", return_value={}),
                mock.patch.object(
                    probe,
                    "load_build_evidence",
                    side_effect=[test_build, full_build],
                ),
                mock.patch.object(probe, "model_first_use_once") as measure,
            ):
                with self.assertRaisesRegex(SystemExit, "different Rust toolchains"):
                    probe.cmd_model_first_use(args)

            measure.assert_not_called()

    def test_validate_model_first_use_evidence_requires_matching_toolchains(self) -> None:
        evidence = fixture_model_first_use_evidence()
        evidence["test_build"]["toolchain"]["cargo"]["release"] = "1.91.0"

        with goal04_contract():
            with self.assertRaisesRegex(ValueError, "build provenance is invalid"):
                probe.validate_model_first_use_evidence(evidence)

    def test_validate_model_first_use_evidence_rejects_wrong_build_role(self) -> None:
        evidence = fixture_model_first_use_evidence()
        evidence["test_build"] = fixture_build_evidence(
            "full",
            source=evidence["source"],
            executable=copy.deepcopy(evidence["executable"]),
        )

        with goal04_contract():
            with self.assertRaisesRegex(ValueError, "build provenance is invalid"):
                probe.validate_model_first_use_evidence(evidence)

    def test_validate_model_first_use_evidence_rejects_forged_samples(self) -> None:
        with goal04_contract():
            evidence = fixture_model_first_use_evidence()
            invalid_summary = copy.deepcopy(evidence)
            invalid_summary["summary"] = {"median_us": 0.0, "p95_us": 0.0}
            with self.assertRaisesRegex(ValueError, "summary does not match"):
                probe.validate_model_first_use_evidence(invalid_summary)

            invalid_samples = copy.deepcopy(evidence)
            invalid_samples["samples_us"][0] = -1.0
            with self.assertRaisesRegex(ValueError, "samples are invalid"):
                probe.validate_model_first_use_evidence(invalid_samples)

            invalid_cache = copy.deepcopy(evidence)
            invalid_cache["cache_state"]["process"] = "reused process"
            with self.assertRaisesRegex(ValueError, "cache state"):
                probe.validate_model_first_use_evidence(invalid_cache)


class ModelTransportDecisionEvidenceTests(unittest.TestCase):
    def test_below_threshold_decision_is_bound_to_keep_in_process(self) -> None:
        evidence = fixture_model_transport_decision_evidence()
        with goal04_contract():
            probe.validate_model_transport_decision_evidence(evidence)

            invalid = copy.deepcopy(evidence)
            invalid["decision"] = "isolate in a worker"
            invalid["authorization"] = "model transport extraction authorized"
            with self.assertRaisesRegex(ValueError, "below-threshold"):
                probe.validate_model_transport_decision_evidence(invalid)

    def test_decision_requires_one_warm_and_one_fresh_profile_input(self) -> None:
        evidence = fixture_model_transport_decision_evidence()
        evidence["inputs"]["fresh_profile"]["cache_state"] = "warm"
        with goal04_contract():
            with self.assertRaisesRegex(ValueError, "input is invalid"):
                probe.validate_model_transport_decision_evidence(evidence)

    def test_decision_rejects_startup_evidence_from_an_old_source_state(self) -> None:
        from scripts.markturbo_tools import goal04

        warm = fixture_startup_evidence(("full", "no-model"), cache_state="warm")
        fresh = fixture_startup_evidence(
            ("full", "no-model"), cache_state="fresh-profile"
        )
        args = argparse.Namespace(
            owner_approved=True,
            evidence=Path("decision.json"),
            warm_evidence=Path("warm.json"),
            fresh_profile_evidence=Path("fresh.json"),
            decision="keep in-process",
        )

        with (
            mock.patch.object(
                goal04,
                "startup_decision_input",
                side_effect=[(warm, {}), (fresh, {})],
            ),
            mock.patch.object(
                goal04, "source_state", return_value=fixture_source_state(dirty=True)
            ),
        ):
            with self.assertRaisesRegex(SystemExit, "current source state"):
                probe.cmd_decide_goal04(args)


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
