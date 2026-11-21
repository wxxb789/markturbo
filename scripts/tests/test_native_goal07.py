"""Unit tests for the Goal 07 native harness without launching a UI."""

from __future__ import annotations

import argparse
import shutil
import socket
import subprocess
import tempfile
import unittest
from pathlib import Path
from typing import Any, Callable
from unittest import mock

from scripts.markturbo_tools.native import evidence as shared_evidence
from scripts.markturbo_tools.native import goal07 as HARNESS
from scripts.markturbo_tools.native import goal07_provider as PROVIDER
from scripts.markturbo_tools.native import runtime

from ._native_support import (
    HASH,
    case_observations,
    fingerprint,
    passed_evidence,
    process_context,
)
from .test_native_runtime_process import OwnedKernel, launch_owned, owned_harness

CASE = HARNESS.CASE_REVISION_SESSION
EDITS = PROVIDER.fixture_edits(HARNESS.EDITOR_SOURCE_TEXT)
REVIEWED = runtime.fingerprint_bytes(HARNESS.EDITOR_SOURCE_BYTES)
ONE_ACCEPTED = runtime.fingerprint_bytes(HARNESS.apply_edits(HARNESS.EDITOR_SOURCE_BYTES, EDITS[:1]))
ALL_ACCEPTED = runtime.fingerprint_bytes(HARNESS.apply_edits(HARNESS.EDITOR_SOURCE_BYTES, EDITS))
DISK = runtime.fingerprint_bytes(HARNESS.SOURCE_BYTES)
STALE = runtime.fingerprint_text(HARNESS.STALE_EDIT)
PROPOSAL_AFTER = 'goal07 = "new"'


def credential_target(endpoint: str) -> str:
    return shared_evidence.credential_target(endpoint, "openai-responses")


def passing_observations(case_id: str = CASE) -> dict[str, Any]:
    """A passed session's observations: every phase's measurements as expected."""
    return case_observations(
        HARNESS.CASE_FLOWS[case_id],
        **HARNESS.expected_observations(),
        provider_request_count=HARNESS.EXPECTED_REQUEST_COUNT,
    )


def valid_evidence() -> dict:
    return passed_evidence(HARNESS.new_evidence(HASH), passing_observations, HARNESS.complete_evidence)


def bare_harness(**attributes: Any) -> HARNESS.Goal07Harness:
    harness = object.__new__(HARNESS.Goal07Harness)
    harness.ui_timeout = 1.0
    for name, value in attributes.items():
        setattr(harness, name, value)
    return harness


def wait_once(check: Callable[[], Any], _timeout: float, code: str, **_kwargs: Any) -> Any:
    value = check()
    if not value:
        raise runtime.HarnessFailure(code)
    return value


class EvidenceTests(unittest.TestCase):
    def test_complete_pass_is_hash_bound_and_counts_the_one_chain(self) -> None:
        evidence = valid_evidence()
        HARNESS.validate_evidence(evidence)
        self.assertEqual(HARNESS.REQUIRED_CASE_IDS, (CASE,))
        self.assertEqual(evidence["status"], "PASS")
        self.assertEqual(evidence["transport"]["request_count"], 2)
        self.assertEqual(evidence["summary"]["passed_case_count"], 1)

    def test_every_fingerprint_has_one_exact_expected_value(self) -> None:
        expected = HARNESS.expected_fingerprints()
        self.assertEqual(set(expected), HARNESS.FINGERPRINT_OBSERVATION_KEYS)
        # The steps are distinguishable: an Accept that wrote nothing, a
        # Reject that wrote, or a Done that saved cannot pass for the
        # expected bytes.
        self.assertEqual(len(EDITS), 2)
        self.assertNotEqual(ONE_ACCEPTED, REVIEWED)
        self.assertNotEqual(ALL_ACCEPTED, ONE_ACCEPTED)
        self.assertNotEqual(DISK, REVIEWED)
        self.assertEqual(expected["decisions_editor_one_accepted"], ONE_ACCEPTED.evidence())
        # Copy exports the approved result: h1 in, h2 left open by Undo.
        self.assertEqual(expected["decisions_clipboard_copied"], ONE_ACCEPTED.evidence())
        self.assertEqual(expected["decisions_editor_stale"], STALE.evidence())
        # The field shows the docked h1's replacement as the model sent it (the
        # edit's span, not its whole line): RevisionHunkView.after.
        self.assertEqual(
            expected["decisions_proposal_after"], fingerprint(PROPOSAL_AFTER.encode("utf-8"))
        )

    def test_the_accepted_hunk_is_the_executable_one_after_the_multibyte_line(self) -> None:
        script, owner = EDITS
        self.assertEqual(script["replacement"], PROPOSAL_AFTER)
        multibyte_line_end = HARNESS.EDITOR_SOURCE_BYTES.index("🚀".encode("utf-8"))
        self.assertGreater(script["range"]["start"], multibyte_line_end)
        self.assertLess(script["range"]["end"], owner["range"]["start"])
        accepted = HARNESS.apply_edits(HARNESS.SOURCE_BYTES, PROVIDER.fixture_edits(HARNESS.SOURCE_TEXT)[:1])
        self.assertIn(b'window.goal07 = "new";\r\n', accepted)
        self.assertIn(b"<p>owner: TBD</p>\r\n", accepted)
        self.assertIn("<p>fixture 文档 🚀</p>\r\n".encode("utf-8"), accepted)

    def test_each_fingerprint_must_hold_its_expected_bytes(self) -> None:
        for key in sorted(HARNESS.FINGERPRINT_OBSERVATION_KEYS):
            with self.subTest(key=key):
                evidence = valid_evidence()
                evidence["cases"][0]["observations"][key] = fingerprint(b"other bytes")
                with self.assertRaisesRegex(ValueError, f"{key} does not hold the expected bytes"):
                    HARNESS.validate_evidence(evidence)

    def test_script_effect_and_window_count_are_required_and_typed(self) -> None:
        for key, value in (
            ("web_security_script_effect", False),
            ("web_security_window_count", 2),
            ("web_security_window_count", True),
        ):
            with self.subTest(key=key, value=value):
                evidence = valid_evidence()
                evidence["cases"][0]["observations"][key] = value
                with self.assertRaisesRegex(ValueError, f"{key} does not hold the expected value"):
                    HARNESS.validate_evidence(evidence)

    def test_each_new_web_observation_is_required_on_its_own(self) -> None:
        for key in (
            "web_security_navigation_attempt_count",
            "web_security_network_attempt_count",
            "web_security_tcp_contact_count",
            "web_security_udp_contact_count",
        ):
            with self.subTest(key=key):
                evidence = valid_evidence()
                observations = evidence["cases"][0]["observations"]
                self.assertIn(key, observations)
                del observations[key]
                with self.assertRaisesRegex(ValueError, "incomplete"):
                    HARNESS.validate_evidence(evidence)

    def test_network_observations_require_all_attempts_and_zero_contacts(self) -> None:
        for key, value in (
            ("web_security_navigation_attempt_count", HARNESS.NAVIGATION_ATTEMPT_COUNT - 1),
            ("web_security_network_attempt_count", HARNESS.NETWORK_PROBE_COUNT - 1),
            ("web_security_tcp_contact_count", 1),
            ("web_security_udp_contact_count", 1),
            ("web_security_tcp_contact_count", False),
        ):
            with self.subTest(key=key, value=value):
                evidence = valid_evidence()
                evidence["cases"][0]["observations"][key] = value
                with self.assertRaisesRegex(ValueError, f"{key} does not hold the expected value"):
                    HARNESS.validate_evidence(evidence)

    def test_the_session_needs_exactly_one_review_and_one_revision(self) -> None:
        for count in (1, 4):
            with self.subTest(count=count):
                evidence = valid_evidence()
                evidence["cases"][0]["observations"]["provider_request_count"] = count
                evidence["transport"]["request_count"] = count
                with self.assertRaisesRegex(ValueError, "one Review and one Revision"):
                    HARNESS.validate_evidence(evidence)

    def test_transport_count_must_match_case_observations(self) -> None:
        evidence = valid_evidence()
        evidence["transport"]["request_count"] -= 1
        with self.assertRaisesRegex(ValueError, "transport request count"):
            HARNESS.validate_evidence(evidence)

    def test_every_phase_observation_is_required(self) -> None:
        for phase in HARNESS.PHASES:
            key = next(key for key in sorted(HARNESS.OBSERVATION_KEYS) if key.startswith(phase))
            with self.subTest(key=key):
                evidence = valid_evidence()
                del evidence["cases"][0]["observations"][key]
                with self.assertRaisesRegex(ValueError, "incomplete"):
                    HARNESS.validate_evidence(evidence)

    def test_blocked_evidence_is_valid_without_windows_environment(self) -> None:
        evidence = HARNESS.new_evidence(HASH)
        evidence["cases"][0].update(
            status="BLOCKED",
            reason_code="WINDOWS_REQUIRED",
            duration_ms=0.0,
            observations={},
        )
        HARNESS.complete_evidence(evidence, "BLOCKED")
        HARNESS.validate_evidence(evidence)
        self.assertEqual(evidence["summary"]["blocked_case_count"], 1)

    def test_document_answer_and_raw_response_sentinels_are_private(self) -> None:
        for sentinel in (
            HARNESS.DOCUMENT_SENTINEL,
            PROVIDER.ANSWER_SENTINEL,
            PROVIDER.RAW_RESPONSE_SENTINEL,
        ):
            evidence = valid_evidence()
            evidence["cases"][0]["observations"]["flow"] = sentinel
            with self.subTest(sentinel=sentinel), self.assertRaisesRegex(
                ValueError, "free-form observation strings"
            ):
                HARNESS.validate_evidence(evidence)

    def test_runtime_scan_must_cover_the_review_settings(self) -> None:
        evidence = valid_evidence()
        evidence["cases"][0]["observations"]["runtime_scan"]["config_files_scanned"] = 0
        with self.assertRaisesRegex(ValueError, "configuration file"):
            HARNESS.validate_evidence(evidence)

    def test_evidence_does_not_allow_request_body_or_credentials(self) -> None:
        evidence = HARNESS.new_evidence(HASH)
        evidence["transport"]["request_body"] = "forbidden"
        with self.assertRaisesRegex(ValueError, "private content"):
            HARNESS.validate_evidence(evidence)


class EvidenceFinalizationTests(unittest.TestCase):
    def run_native_cases(
        self,
        root: Path,
        scenario: Callable[[], dict],
        *,
        case: str | None = None,
        preflight: object | None = None,
    ) -> tuple[int, dict, str]:
        executable = root / "markturbo.exe"
        executable.write_bytes(b"native evidence finalization fixture")
        goal07_plan = HARNESS.native_run_plan()

        class FakeHarness:
            def cleanup(self) -> None:
                pass

        plan = runtime.NativeRunPlan(
            required_case_ids=HARNESS.REQUIRED_CASE_IDS,
            workdir_prefix="markturbo-goal07-evidence-test-",
            new_evidence=goal07_plan.new_evidence,
            validate_evidence=goal07_plan.validate_evidence,
            preflight=preflight or (lambda *_args: (object(), object())),
            ui_types_loader=lambda: (),
            harness_factory=lambda *_args: FakeHarness(),
            scenarios=lambda _harness: (scenario,),
            finalize_evidence=goal07_plan.finalize_evidence,
        )
        args = argparse.Namespace(
            exe=executable,
            expect_exe_sha256=runtime.sha256_file(executable).sha256,
            ui_timeout=1.0,
            case=case,
            keep_workdir_on_failure=False,
        )
        return runtime.run_native_acceptance(args, plan)

    def test_interleaved_evidence_instances_keep_transport_counts_separate(self) -> None:
        first = HARNESS.new_evidence(HASH)
        second = HARNESS.new_evidence(HASH)
        first["cases"][0]["observations"]["provider_request_count"] = 6
        second["cases"][0]["observations"]["provider_request_count"] = 4

        HARNESS.complete_evidence(first, "FAIL")
        HARNESS.complete_evidence(second, "BLOCKED")
        self.assertEqual(first["transport"]["request_count"], 6)
        self.assertEqual(second["transport"]["request_count"], 4)

    def test_request_count_rejects_boolean_case_observations(self) -> None:
        evidence = valid_evidence()
        evidence["cases"][0]["observations"]["provider_request_count"] = True
        with self.assertRaisesRegex(ValueError, "invalid provider request count"):
            HARNESS.validate_evidence(evidence)

        with self.assertRaisesRegex(ValueError, "invalid provider request count"):
            HARNESS.complete_evidence(evidence, "PASS")

    def test_failed_privacy_scan_fails_the_session_without_its_observations(self) -> None:
        def failed_scan() -> dict:
            with tempfile.TemporaryDirectory() as temporary:
                case_root = Path(temporary)
                logs = case_root / "data" / "logs"
                logs.mkdir(parents=True)
                (case_root / "config").mkdir()
                (logs / "markturbo.log").write_bytes(HARNESS.DOCUMENT_SENTINEL.encode("utf-8"))
                return HARNESS.scan_case_artifacts(case_root, "synthetic-key")

        with tempfile.TemporaryDirectory() as temporary:
            returncode, evidence, reason = self.run_native_cases(Path(temporary), failed_scan)

        self.assertEqual((returncode, evidence["status"], reason), (1, "FAIL", "UTF8_DOCUMENT_SENTINEL_LEAKED"))
        self.assertEqual(evidence["cases"][0]["observations"], {})
        self.assertEqual(evidence["transport"]["request_count"], 0)

    def test_selected_case_run_is_partial(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            returncode, evidence, reason = self.run_native_cases(
                Path(temporary), lambda: {"provider_request_count": 2}, case=CASE
            )

        self.assertEqual((returncode, evidence["status"], reason), (1, "FAIL", "PARTIAL_CASE_RUN"))
        self.assertEqual(evidence["transport"]["request_count"], 2)
        self.assertEqual(evidence["cases"][0]["status"], "PASS")

    def test_early_blocked_plan_keeps_request_count_zero(self) -> None:
        def blocked(*_args: object) -> tuple[object, object]:
            raise runtime.HarnessBlocked("WTS_SESSION_NOT_ACTIVE")

        with tempfile.TemporaryDirectory() as temporary:
            returncode, evidence, reason = self.run_native_cases(
                Path(temporary), lambda: {}, preflight=blocked
            )

        self.assertEqual((returncode, evidence["status"], reason), (2, "BLOCKED", "WTS_SESSION_NOT_ACTIVE"))
        self.assertEqual(evidence["transport"]["request_count"], 0)


class PrivacyAndRuntimeTests(unittest.TestCase):
    def test_runtime_scan_detects_split_boundary_private_bytes(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / "data" / "logs").mkdir(parents=True)
            (root / "config").mkdir()
            split = 1024 * 1024 - 2
            (root / "data" / "logs" / "app.log").write_bytes(
                b"x" * split + HARNESS.DOCUMENT_SENTINEL.encode("utf-8")
            )
            with self.assertRaisesRegex(runtime.HarnessFailure, "UTF8_DOCUMENT_SENTINEL_LEAKED"):
                HARNESS.scan_case_artifacts(root, "synthetic-key")

    def test_runtime_scan_rejects_private_values_without_recording_them(self) -> None:
        for value, code in (
            ("synthetic-key", "UTF8_EPHEMERAL_CREDENTIAL_LEAKED"),
            (PROVIDER.ANSWER_SENTINEL, "UTF8_ANSWER_SENTINEL_LEAKED"),
            (PROVIDER.RAW_RESPONSE_SENTINEL, "UTF8_RAW_RESPONSE_SENTINEL_LEAKED"),
        ):
            with self.subTest(code=code), tempfile.TemporaryDirectory() as directory:
                root = Path(directory)
                (root / "data" / "logs").mkdir(parents=True)
                (root / "config").mkdir()
                (root / "data" / "logs" / "app.log").write_text(value, encoding="utf-8")
                with self.assertRaisesRegex(runtime.HarnessFailure, code):
                    HARNESS.scan_case_artifacts(root, "synthetic-key")

    def test_privacy_scan_waits_for_owned_descendants_even_after_parent_exit(self) -> None:
        for parent_exited in (False, True):
            with self.subTest(parent_exited=parent_exited), tempfile.TemporaryDirectory() as directory:
                harness, kernel = owned_harness(Path(directory), HARNESS.Goal07Harness)
                app = launch_owned(harness)
                kernel.parent_exited = parent_exited
                case_root = harness.root / "cases" / "owned"
                logs = case_root / "data" / "logs"
                logs.mkdir()
                (logs / "app.log").write_bytes(b"content-free log")
                (case_root / "config" / "settings.toml").write_bytes(b"keyless")
                (case_root / "stderr.log").write_bytes(b"")
                provider = mock.Mock(request_count=2)
                harness.network_probes = mock.Mock(
                    snapshot=mock.Mock(return_value={"tcp_contacts": 0, "udp_contacts": 0})
                )
                real_scan = HARNESS.scan_case_artifacts

                def scan(*args):
                    self.assertFalse(kernel.descendant_alive)
                    self.assertIn("completion-4", kernel.events)
                    kernel.events.append("scan")
                    return real_scan(*args)

                with mock.patch.object(HARNESS, "scan_case_artifacts", side_effect=scan):
                    result = harness._finalize_observations(provider, case_root, app, {})
                self.assertEqual(kernel.events[-2:], ["completion-4", "scan"])
                provider.verify_chain.assert_called_once_with()
                self.assertEqual(result["provider_request_count"], 2)
                self.assertEqual(result["runtime_scan"]["files_scanned"], 3)
                self.assertCountEqual(kernel.closed, kernel.opened)
                harness.network_probes.close.assert_called_once_with()

    def test_quiescence_failure_never_reaches_provider_finalization_or_privacy_scan(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            harness, kernel = owned_harness(
                Path(directory), HARNESS.Goal07Harness, OwnedKernel("completion")
            )
            app = launch_owned(harness)
            kernel.parent_exited = True
            provider = mock.Mock()
            observations: dict = {}
            with (
                mock.patch.object(runtime.ctypes, "get_last_error", return_value=258, create=True),
                mock.patch.object(HARNESS, "scan_case_artifacts") as scan,
                self.assertRaisesRegex(runtime.HarnessFailure, "PROCESS_JOB_QUIESCENCE_TIMEOUT"),
            ):
                harness._finalize_observations(provider, Path(directory), app, observations)
            scan.assert_not_called()
            provider.verify_chain.assert_not_called()
            self.assertEqual(observations, {})
            self.assertCountEqual(kernel.closed, kernel.opened)

    def test_isolation_removal_requires_successful_owned_job_quiescence(self) -> None:
        for fail in ("", "completion"):
            with self.subTest(fail=fail), tempfile.TemporaryDirectory() as directory:
                root = Path(directory)
                executable = root / "app.exe"
                executable.write_bytes(b"synthetic fixture")
                kernel = OwnedKernel(fail)
                harnesses = []

                def factory(_exe, workdir, *_args):
                    harness, _kernel = owned_harness(workdir, HARNESS.Goal07Harness, kernel)
                    harnesses.append(harness)
                    return harness

                def scenario():
                    launch_owned(harnesses[0])
                    kernel.parent_exited = True
                    raise runtime.HarnessBlocked("FOREGROUND_PERMISSION_DENIED")

                base = HARNESS.native_run_plan()
                plan = runtime.NativeRunPlan(
                    required_case_ids=HARNESS.REQUIRED_CASE_IDS,
                    workdir_prefix="markturbo-owned-job-test-",
                    new_evidence=base.new_evidence,
                    validate_evidence=base.validate_evidence,
                    preflight=lambda *_args: (None, None),
                    ui_types_loader=lambda: (),
                    harness_factory=factory,
                    scenarios=lambda _harness: (scenario,),
                    finalize_evidence=base.finalize_evidence,
                )
                args = argparse.Namespace(
                    exe=executable, expect_exe_sha256=runtime.sha256_file(executable).sha256,
                    ui_timeout=1.0, case=None, keep_workdir_on_failure=False,
                )
                real_remove = runtime.remove_tree_with_retry

                def remove(workdir):
                    self.assertFalse(kernel.descendant_alive)
                    self.assertIn("completion-4", kernel.events)
                    kernel.events.append("remove")
                    real_remove(workdir)

                try:
                    with (
                        mock.patch.object(runtime.ctypes, "get_last_error", return_value=258, create=True),
                        mock.patch.object(runtime, "remove_tree_with_retry", side_effect=remove) as removal,
                    ):
                        result, evidence, reason = runtime.run_native_acceptance(args, plan)
                    self.assertCountEqual(kernel.closed, kernel.opened)
                    if fail:
                        self.assertEqual((result, reason), (1, "PROCESS_JOB_QUIESCENCE_TIMEOUT"))
                        self.assertEqual(evidence["cases"][0]["status"], "FAIL")
                        removal.assert_not_called()
                        self.assertTrue(args.debug_workdir.is_dir())
                    else:
                        self.assertEqual((result, reason), (2, "FOREGROUND_PERMISSION_DENIED"))
                        self.assertEqual(kernel.events[-2:], ["completion-4", "remove"])
                        self.assertIsNone(args.debug_workdir)
                        self.assertFalse(harnesses[0].root.exists())
                finally:
                    if args.debug_workdir is not None:
                        # The fake owns no OS processes; remove the deliberately retained fixture.
                        real_remove(args.debug_workdir)


class SessionTests(unittest.TestCase):
    """The single launch: its profile, its phases in order and their failures."""

    def test_profile_writes_only_the_security_fixture_and_never_touches_credential_manager(
        self,
    ) -> None:
        win32 = mock.Mock()
        harness = bare_harness(win32=win32)
        with tempfile.TemporaryDirectory() as directory, HARNESS.NetworkProbeListeners() as probes:
            harness.root = Path(directory)
            case_root, data, config, workspace, stderr = harness.profile(
                "http://127.0.0.1:43123/v1/", probes
            )
            self.assertEqual(case_root, data.parent)
            fixture = HARNESS.fixture_source(probes).encode("utf-8")
            self.assertEqual(
                {path.name: path.read_bytes() for path in workspace.iterdir()},
                {HARNESS.FIXTURE_FILE: fixture},
            )
            self.assertEqual(
                HARNESS.canonicalize_fixture_source(fixture, probes), HARNESS.SOURCE_BYTES
            )
            self.assertEqual(
                (config / "settings.toml").read_bytes(),
                HARNESS.review_settings_document("http://127.0.0.1:43123/v1/"),
            )
            self.assertEqual(stderr.name, "stderr.log")
        self.assertEqual(win32.mock_calls, [])

    def session_harness(self) -> tuple[HARNESS.Goal07Harness, mock.Mock, mock.Mock, mock.Mock]:
        order = mock.Mock()
        app, provider = mock.Mock(hwnd=73), mock.Mock()
        app.security_context.evidence.return_value = process_context()
        app.process.poll.return_value = None
        provider.start.return_value = provider
        provider.base_url = "http://127.0.0.1:43123/v1/"
        workspace = Path("workspace")
        harness = bare_harness(
            profile=mock.Mock(return_value=(Path("case"), Path("data"), Path("config"), workspace, Path("stderr"))),
            launch_app=order.launch_app,
            phase_web_security=order.phase_web_security,
            phase_decisions=order.phase_decisions,
            quit_via_menu=order.quit_via_menu,
            click_lifecycle_decision=order.click_lifecycle_decision,
            wait_process_exit=order.wait_process_exit,
            reap=order.reap,
            _finalize_observations=lambda _provider, _root, _app, observations: observations,
        )
        order.launch_app.return_value = app
        order.phase_web_security.return_value = {
            "web_security_disk_before": DISK.evidence(),
            "web_security_script_effect": True,
            "web_security_window_count": 1,
        }
        order.phase_decisions.return_value = {"decisions_editor_stale": STALE.evidence()}
        return harness, order, app, provider

    def test_one_launch_on_the_document_runs_both_phases_then_quits_through_the_guard(self) -> None:
        harness, order, app, provider = self.session_harness()
        with mock.patch.object(PROVIDER, "LoopbackRevisionServer", return_value=provider):
            observations = harness.scenario_revision_session()

        source = Path("workspace") / HARNESS.FIXTURE_FILE
        self.assertEqual(
            # The app is the launch's return value: its own calls are not steps.
            [call for call in order.mock_calls if not call[0].startswith("launch_app()")],
            [
                mock.call.launch_app(
                    source, Path("data"), Path("config"), Path("workspace"), Path("stderr"),
                ),
                mock.call.phase_web_security(app, provider, source),
                mock.call.phase_decisions(app, provider, source),
                mock.call.quit_via_menu(app),
                mock.call.click_lifecycle_decision(app, "Discard"),
                mock.call.wait_process_exit(app),
                mock.call.reap(app),
            ],
        )
        self.assertEqual(observations["web_security_disk_before"], DISK.evidence())
        self.assertEqual(observations["decisions_editor_stale"], STALE.evidence())
        self.assertEqual(observations["flow"], HARNESS.CASE_FLOWS[CASE])
        provider.close.assert_called_once_with()

    def test_a_phase_failure_names_its_phase_and_stops_the_session(self) -> None:
        harness, order, app, provider = self.session_harness()
        order.phase_web_security.side_effect = runtime.HarnessFailure("SCRIPT_EFFECT_TIMEOUT")
        with (
            mock.patch.object(PROVIDER, "LoopbackRevisionServer", return_value=provider),
            self.assertRaisesRegex(
                runtime.HarnessFailure, "^WEB_SECURITY_SCRIPT_EFFECT_TIMEOUT$"
            ),
        ):
            harness.scenario_revision_session()
        order.phase_decisions.assert_not_called()
        order.quit_via_menu.assert_not_called()
        order.reap.assert_called_once_with(app)
        provider.close.assert_called_once_with()

    def test_an_unexpected_measurement_fails_with_its_phase_key(self) -> None:
        for key, value, code in (
            ("web_security_disk_before", REVIEWED, "WEB_SECURITY_DISK_BEFORE_UNEXPECTED"),
            ("web_security_script_effect", False, "WEB_SECURITY_SCRIPT_EFFECT_UNEXPECTED"),
            ("web_security_window_count", 2, "WEB_SECURITY_WINDOW_COUNT_UNEXPECTED"),
            ("web_security_window_count", True, "WEB_SECURITY_WINDOW_COUNT_UNEXPECTED"),
            ("decisions_editor_one_accepted", ALL_ACCEPTED, "DECISIONS_EDITOR_ONE_ACCEPTED_UNEXPECTED"),
            ("decisions_clipboard_copied", ALL_ACCEPTED, "DECISIONS_CLIPBOARD_COPIED_UNEXPECTED"),
            ("decisions_disk_after", ONE_ACCEPTED, "DECISIONS_DISK_AFTER_UNEXPECTED"),
        ):
            with self.subTest(code=code):
                observed: dict[str, Any] = {}
                with self.assertRaisesRegex(runtime.HarnessFailure, f"^{code}$"):
                    phase_name = "WEB_SECURITY" if key.startswith("web_security_") else "DECISIONS"
                    with HARNESS.phase(phase_name):
                        HARNESS.record(observed, key, value)
                self.assertEqual(observed, {})
        observed = {}
        HARNESS.record(observed, "decisions_editor_one_accepted", ONE_ACCEPTED)
        self.assertEqual(observed, {"decisions_editor_one_accepted": ONE_ACCEPTED.evidence()})

    def test_scenario_contract_failure_is_raised_before_shared_pass_status(self) -> None:
        observations = passing_observations()
        observations["decisions_clipboard_copied"] = ALL_ACCEPTED.evidence()
        harness = bare_harness(parent_context=mock.Mock(), scenario_revision_session=lambda: observations)
        harness.parent_context.evidence.return_value = process_context()
        with self.assertRaisesRegex(runtime.HarnessFailure, "CASE_CONTRACT_FAILED"):
            harness.scenario(CASE)


class PhaseTests(unittest.TestCase):
    """Each phase's UI steps, in order, against fakes."""

    def web_security_harness(
        self, focus_returns: bool
    ) -> tuple[HARNESS.Goal07Harness, mock.Mock, mock.Mock]:
        order = mock.Mock()
        app = mock.Mock(hwnd=73)
        win32 = mock.Mock()
        win32.foreground_focus.return_value = app.hwnd if focus_returns else 0
        win32.foreground_window.return_value = app.hwnd
        win32.visible_top_level_windows.return_value = (app.hwnd,)
        app.process.poll.return_value = None
        app.process.active_process_ids.return_value = (73, 74)
        probes = mock.Mock()
        probes.snapshot.return_value = {"tcp_contacts": 0, "udp_contacts": 0}
        harness = bare_harness(win32=win32, network_probes=probes)
        for name in ("menu_command", "wait_name", "find_named", "click_control", "editor_control"):
            setattr(harness, name, getattr(order, name))
        return harness, order, app

    def test_web_phase_checks_script_navigation_and_window_then_hands_keyboard_back(self) -> None:
        harness, order, app = self.web_security_harness(focus_returns=True)
        with (
            mock.patch.object(HARNESS, "fixture_fingerprint", return_value=DISK),
            mock.patch.object(HARNESS, "wait_until", side_effect=wait_once),
        ):
            observed = harness.phase_web_security(app, order.provider, Path(HARNESS.FIXTURE_FILE))

        self.assertEqual(
            order.mock_calls,
            [
                mock.call.menu_command(app, runtime.MENU_WEB_AUTOMATION_ID),
                mock.call.wait_name(
                    app, HARNESS.SCRIPT_EFFECT_TEXT, "Text", "SCRIPT_EFFECT_TIMEOUT"
                ),
                mock.call.wait_name(
                    app,
                    HARNESS.USER_NAVIGATION_BLOCKED_TEXT,
                    "Text",
                    "USER_NAVIGATION_NOT_BLOCKED",
                ),
                mock.call.wait_name(
                    app,
                    HARNESS.NETWORK_ATTEMPTS_COMPLETE_TEXT,
                    "Text",
                    "NETWORK_ATTEMPTS_TIMEOUT",
                ),
                mock.call.wait_name(
                    app,
                    HARNESS.SCRIPT_NAVIGATION_READY_TEXT,
                    "Text",
                    "SCRIPT_NAVIGATION_READY_TIMEOUT",
                ),
                mock.call.find_named(
                    app,
                    HARNESS.SCRIPT_NAVIGATION_RELEASE_NAME,
                    "Button",
                    "SCRIPT_NAVIGATION_RELEASE_UIA_TIMEOUT",
                    "SCRIPT_NAVIGATION_RELEASE_UIA_MISMATCH",
                    anonymous=False,
                ),
                mock.call.click_control(
                    order.find_named.return_value, "SCRIPT_NAVIGATION_RELEASE_CLICK_FAILED"
                ),
                mock.call.menu_command(app, runtime.MENU_EDIT_AUTOMATION_ID),
                mock.call.editor_control(app),
            ],
        )
        self.assertEqual(
            observed,
            {
                "web_security_disk_before": DISK.evidence(),
                "web_security_script_effect": True,
                "web_security_window_count": 1,
                "web_security_navigation_attempt_count": HARNESS.NAVIGATION_ATTEMPT_COUNT,
                "web_security_network_attempt_count": HARNESS.NETWORK_PROBE_COUNT,
                "web_security_tcp_contact_count": 0,
                "web_security_udp_contact_count": 0,
            },
        )
        harness.win32.sleep.assert_has_calls(
            [
                mock.call(HARNESS.SCRIPT_NAVIGATION_SETTLE_SECONDS),
                mock.call(HARNESS.NETWORK_PROBE_SETTLE_SECONDS),
            ]
        )
        harness.win32.visible_top_level_windows.assert_called_once_with((73, 74))
        harness.win32.foreground_window.assert_called_once_with()
        harness.network_probes.snapshot.assert_called_once_with()

    def test_interrupted_navigation_order_fails_before_verdict(self) -> None:
        harness, order, app = self.web_security_harness(focus_returns=True)
        harness.network_probes = mock.Mock(
            snapshot=mock.Mock(return_value={"tcp_contacts": 0, "udp_contacts": 0})
        )

        def wait_for_marker(_app: Any, _name: str, _kind: str, failure_code: str) -> None:
            if failure_code == "SCRIPT_NAVIGATION_READY_TIMEOUT":
                raise runtime.HarnessFailure(failure_code)

        order.wait_name.side_effect = wait_for_marker
        with (
            mock.patch.object(HARNESS, "fixture_fingerprint", return_value=DISK),
            self.assertRaisesRegex(runtime.HarnessFailure, "SCRIPT_NAVIGATION_READY_TIMEOUT"),
        ):
            harness.phase_web_security(app, order.provider, Path(HARNESS.FIXTURE_FILE))

        harness.network_probes.snapshot.assert_not_called()
        harness.win32.visible_top_level_windows.assert_not_called()
        order.find_named.assert_not_called()
        order.click_control.assert_not_called()
        order.menu_command.assert_called_once_with(app, runtime.MENU_WEB_AUTOMATION_ID)

    def test_web_phase_fails_closed_when_a_listener_counts_a_contact(self) -> None:
        harness, order, app = self.web_security_harness(focus_returns=True)
        harness.network_probes.snapshot.return_value = {"tcp_contacts": 1, "udp_contacts": 0}
        with (
            mock.patch.object(HARNESS, "fixture_fingerprint", return_value=DISK),
            self.assertRaisesRegex(runtime.HarnessFailure, "TCP_CONTACT_COUNT_UNEXPECTED"),
        ):
            harness.phase_web_security(app, order.provider, Path(HARNESS.FIXTURE_FILE))

        harness.win32.visible_top_level_windows.assert_called_once_with((73, 74))
        order.menu_command.assert_has_calls(
            [
                mock.call(app, runtime.MENU_WEB_AUTOMATION_ID),
                mock.call(app, runtime.MENU_EDIT_AUTOMATION_ID),
            ]
        )

    def test_fixture_attempt_markers_precede_calls_and_use_only_run_targets(self) -> None:
        with HARNESS.NetworkProbeListeners() as probes:
            source = HARNESS.fixture_source(probes)
            self.assertIn(f'const tcpPort = "{probes.tcp_port}";', source)
            self.assertIn(f'const udpPort = "{probes.udp_port}";', source)
            self.assertIn(HARNESS.NETWORK_PROBE_HOST, source)
            self.assertEqual(
                source.count("{ run:"),
                HARNESS.NETWORK_PROBE_COUNT + HARNESS.NAVIGATION_ATTEMPT_COUNT,
            )
            self.assertEqual(source.count("navigation: true"), HARNESS.NAVIGATION_ATTEMPT_COUNT)
            self.assertEqual(
                source.count("scriptNavigation: true"), HARNESS.SCRIPT_NAVIGATION_ATTEMPT_COUNT
            )
            for attempt in (
                "function fetchProbe",
                "function imageProbe",
                "navigator.sendBeacon",
                "new WebSocket",
                "new RTCPeerConnection",
                "stun:127.0.0.1:${udpPort}",
                "turn:127.0.0.1:${tcpPort}",
                "dns-prefetch",
                "preconnect",
                'form.method = "post"',
                "link.click()",
                "form.requestSubmit()",
                "new SubmitEvent(\"submit\", { bubbles: true, cancelable: true })",
                "submitEvent.defaultPrevented",
                'target="_self"',
                'target="_blank"',
                "frame.src = `https://127.0.0.1:${tcpPort}/subframe`",
            ):
                with self.subTest(attempt=attempt):
                    self.assertIn(attempt, source)
            self.assertIn('`https://127.0.0.1:${tcpPort}`', source)
            self_location = source.index("try { location = `${tcpLoopback}/frame`")
            for attempt in (
                'window.open(`${tcpLoopback}/popup`',
                "top.location = `https://${probeHost}/top`",
                "frame.src = `https://127.0.0.1:${tcpPort}/subframe`",
                "submitForm(`${tcpLoopback}/form`)",
                "submitForm(`${tcpHostname}/form`)",
            ):
                with self.subTest(script_navigation_before_self_location=attempt):
                    self.assertLess(source.index(attempt), self_location)
            run_attempts = source[source.index("async function runAttempts()") :]
            release_handler = run_attempts.index("navigationRelease.addEventListener(\"click\"")
            deferred_loop = run_attempts.index("for (const attempt of scriptNavigationAttempts)")
            self.assertLess(
                run_attempts.index(
                    "networkStatus.textContent = `Network probes complete ${networkIndex}/${networkProbeCount}`"
                ),
                release_handler,
            )
            self.assertLess(
                run_attempts.index("userNavigationStatus.textContent = `User navigation blocked"),
                release_handler,
            )
            self.assertLess(
                run_attempts.index("navigationReleaseStatus.textContent = `Ready to run script navigation probes"),
                run_attempts.index("navigationRelease.disabled = false"),
            )
            handler_end = run_attempts.index("}, { once: true });", release_handler)
            self.assertGreater(deferred_loop, release_handler)
            for marker in (
                "networkStatus",
                "navigationStatus",
                "userNavigationStatus",
                "navigationReleaseStatus",
            ):
                self.assertNotIn(f"{marker}.textContent =", run_attempts[release_handler:handler_end])

    def test_equal_dynamic_ports_canonicalize_at_their_declarations_only(self) -> None:
        probes = mock.Mock(tcp_port=43123, udp_port=43123)
        source = HARNESS.fixture_source(probes).encode("utf-8")
        self.assertEqual(HARNESS.canonicalize_fixture_source(source, probes), HARNESS.SOURCE_BYTES)

    def test_fixture_script_runs_all_attempts_in_order_with_fake_browser_apis(self) -> None:
        node = shutil.which("node")
        if node is None:
            self.skipTest("Node.js is unavailable")
        with HARNESS.NetworkProbeListeners() as probes:
            source = HARNESS.fixture_source(probes)
        start = source.index("<script>\r\n") + len("<script>\r\n")
        end = source.index("</script>", start)
        script = source[start:end]
        node_harness = r"""
const fs = require("node:fs");
const vm = require("node:vm");
const source = fs.readFileSync(0, "utf8");
const runCall = "runAttempts();";
const callIndex = source.lastIndexOf(runCall);
if (callIndex < 0 || source.slice(callIndex + runCall.length).trim()) throw new Error("fixture runner call was not found");
const code = source.slice(0, callIndex) + "globalThis.__run = runAttempts();" + source.slice(callIndex + runCall.length);
const expectedNetworkCount = Number(process.argv[1]);
const expectedNavigationCount = Number(process.argv[2]);
const expectedUserNavigationCount = Number(process.argv[3]);
const expectedScriptNavigationCount = Number(process.argv[4]);
const calls = [];
const effect = { textContent: "" };
const networkWrites = [];
const navigationWrites = [];
const userNavigationWrites = [];
const navigationReleaseWrites = [];
const statusNode = writes => ({
  value: "",
  get textContent() { return this.value; },
  set textContent(value) { this.value = value; writes.push([value, calls.length]); },
});
const network = statusNode(networkWrites);
const navigation = statusNode(navigationWrites);
const userNavigation = statusNode(userNavigationWrites);
const navigationReleaseStatus = statusNode(navigationReleaseWrites);
const navigationRelease = {
  disabled: true,
  listeners: {},
  addEventListener(name, callback) { this.listeners[name] = callback; },
  click() {
    if (this.disabled) throw new Error("navigation release is disabled");
    if (navigationReleaseStatus.textContent !== `Ready to run script navigation probes ${expectedScriptNavigationCount}/${expectedScriptNavigationCount}`) throw new Error("ready marker missing before release");
    this.listeners.click?.();
  },
};
const nodes = new Map([
  ["goal07-effect", effect],
  ["goal07-network", network],
  ["goal07-navigation", navigation],
  ["goal07-user-link", {
    href: "",
    target: "_self",
    listeners: {},
    addEventListener(name, callback) { this.listeners[name] = callback; },
    click() {
      assertReady(true);
      calls.push(["user-link", this.href, this.target]);
      this.listeners.click?.({ defaultPrevented: true });
    },
  }],
  ["goal07-user-form", {
    action: "",
    target: "_blank",
    syntheticSubmitPrevented: false,
    dispatchEvent(event) {
      if (event.type !== "submit" || !event.bubbles || !event.cancelable) throw new Error("submit event shape mismatch");
      event.defaultPrevented = true;
      this.syntheticSubmitPrevented = event.defaultPrevented;
      return false;
    },
    requestSubmit() {
      assertReady(true);
      if (!this.syntheticSubmitPrevented) throw new Error("capture-phase submit cancellation missing");
      calls.push(["user-form", this.action, this.target]);
    },
  }],
  ["goal07-user-navigation", userNavigation],
  ["goal07-navigation-release-status", navigationReleaseStatus],
  ["goal07-navigation-release", navigationRelease],
]);
function assertReady(isNavigation = false, isScriptNavigation = false) {
  if (isScriptNavigation) {
    if (network.textContent !== `Network probes complete ${expectedNetworkCount}/${expectedNetworkCount}`) throw new Error("network marker missing before script navigation");
    if (navigationReleaseStatus.textContent !== `Ready to run script navigation probes ${expectedScriptNavigationCount}/${expectedScriptNavigationCount}`) throw new Error("script navigation marker missing");
    return;
  }
  if (!network.textContent.startsWith("Network probe attempted ") && !network.textContent.startsWith("Network probes complete ")) throw new Error("network marker missing");
  if (isNavigation && !navigation.textContent.startsWith("Navigation attempt ")) throw new Error("navigation marker missing");
}
function element(tag) {
  const value = { tag, action: "", rel: "" };
  if (tag === "img") Object.defineProperty(value, "src", { set(url) { assertReady(); calls.push(["img", url]); } });
  if (tag === "link") Object.defineProperty(value, "href", { set(url) { assertReady(); calls.push(["link", value.rel, url]); } });
  if (tag === "form") value.submit = () => { assertReady(true, true); calls.push(["form", value.action]); throw new Error("blocked"); };
  if (tag === "iframe") Object.defineProperty(value, "src", { set(url) { assertReady(true, true); calls.push(["iframe", url]); } });
  return value;
}
const context = {
  document: {
    getElementById: id => nodes.get(id),
    createElement: element,
    body: { appendChild() {} },
    head: { appendChild() {} },
  },
  window: {
    open(url) { assertReady(true, true); calls.push(["open", url]); return null; },
  },
  navigator: {
    sendBeacon(url) { assertReady(); calls.push(["beacon", url]); return true; },
  },
  fetch(url) { assertReady(); calls.push(["fetch", url]); return Promise.resolve(); },
  WebSocket: function (url) { assertReady(); calls.push(["websocket", url]); },
  SubmitEvent: function (type, options) {
    this.type = type;
    this.bubbles = options.bubbles;
    this.cancelable = options.cancelable;
    this.defaultPrevented = false;
  },
  RTCPeerConnection: function (configuration) {
    assertReady();
    calls.push(["rtc"]);
    context.__iceServers = configuration.iceServers;
    this.createDataChannel = () => { context.__dataChannel = true; };
    this.createOffer = () => Promise.resolve({ type: "offer" });
    this.setLocalDescription = () => Promise.resolve();
  },
  setTimeout(callback) { callback(); return 0; },
};
context.top = {};
Object.defineProperty(context.top, "location", {
  set(url) { assertReady(true, true); calls.push(["top", url]); throw new Error("blocked"); },
});
Object.defineProperty(context, "location", {
  set(url) { assertReady(true, true); calls.push(["location", url]); throw new Error("blocked"); },
});
async function main() {
  await vm.runInNewContext(code, context);
  if (effect.textContent !== "Script ran automatically") throw new Error("script effect missing");
  if (navigationRelease.disabled) throw new Error("navigation release was not enabled");
  if (calls.length !== expectedNetworkCount + expectedUserNavigationCount) throw new Error("script navigation ran before release");
  if (navigationReleaseStatus.textContent !== `Ready to run script navigation probes ${expectedScriptNavigationCount}/${expectedScriptNavigationCount}`) throw new Error("script navigation ready marker missing");
  navigationRelease.click();
  const scriptNavigationKinds = new Set(["open", "top", "location", "form", "iframe"]);
  const userNavigationKinds = new Set(["user-link", "user-form"]);
  const networkCallCount = calls.filter(([kind]) => !scriptNavigationKinds.has(kind) && !userNavigationKinds.has(kind)).length;
  const scriptNavigationCalls = calls.filter(([kind]) => scriptNavigationKinds.has(kind));
  const userNavigationCalls = calls.filter(([kind]) => userNavigationKinds.has(kind));
  if (networkCallCount !== expectedNetworkCount) throw new Error("not every network probe ran");
  if (userNavigationCalls.length !== expectedUserNavigationCount) throw new Error("not every user navigation ran");
  if (scriptNavigationCalls.length !== expectedScriptNavigationCount) throw new Error("not every script navigation ran");
  if (calls.length !== expectedNetworkCount + expectedNavigationCount) throw new Error("attempt count mismatch");
  if (network.textContent !== `Network probes complete ${expectedNetworkCount}/${expectedNetworkCount}`) throw new Error("network attempts incomplete");
  if (navigationWrites.length !== expectedUserNavigationCount) throw new Error("user navigation attempts are not marked");
  for (let index = 1; index <= expectedUserNavigationCount; index += 1) {
    if (navigationWrites[index - 1][0] !== `Navigation attempt ${index}/${expectedNavigationCount} attempted` || navigationWrites[index - 1][1] !== expectedNetworkCount + index - 1) throw new Error("user navigation marker ordering invalid");
  }
  if (userNavigation.textContent !== `User navigation blocked ${expectedUserNavigationCount}/${expectedUserNavigationCount}`) throw new Error("user navigation was not blocked");
  if (navigationReleaseWrites.length !== 1) throw new Error("navigation ready marker missing");
  if (navigationReleaseWrites[0][0] !== `Ready to run script navigation probes ${expectedScriptNavigationCount}/${expectedScriptNavigationCount}`) throw new Error("navigation ready marker changed");
  const networkCompletions = networkWrites.filter(([value]) => value.startsWith("Network probes complete "));
  if (networkCompletions.length !== expectedNetworkCount) throw new Error("network completion markers missing");
  for (let index = 1; index <= expectedNetworkCount; index += 1) {
    if (networkCompletions[index - 1][0] !== `Network probes complete ${index}/${expectedNetworkCount}` || networkCompletions[index - 1][1] !== index) throw new Error("network completion ordering invalid");
  }
  const firstScriptNavigationCall = calls.findIndex(([kind]) => scriptNavigationKinds.has(kind));
  if (networkWrites.some(([, callCount]) => callCount > firstScriptNavigationCall) || navigationWrites.some(([, callCount]) => callCount > firstScriptNavigationCall) || userNavigationWrites.some(([, callCount]) => callCount > firstScriptNavigationCall) || navigationReleaseWrites.some(([, callCount]) => callCount > firstScriptNavigationCall)) throw new Error("a marker was written after script navigation began");
  if (context.__iceServers.length !== 2 || !context.__dataChannel) throw new Error("WebRTC setup incomplete");
}
main().catch(error => { console.error(error.message); process.exitCode = 1; });
"""
        result = subprocess.run(
            [
                node,
                "-e",
                node_harness,
                str(HARNESS.NETWORK_PROBE_COUNT),
                str(HARNESS.NAVIGATION_ATTEMPT_COUNT),
                str(HARNESS.USER_NAVIGATION_ATTEMPT_COUNT),
                str(HARNESS.SCRIPT_NAVIGATION_ATTEMPT_COUNT),
            ],
            input=script,
            text=True,
            capture_output=True,
            timeout=10,
            check=False,
        )
        self.assertEqual(result.returncode, 0, result.stderr.strip())

    def test_probe_listeners_bind_ephemeral_loopback_ports_and_count_only_contacts(self) -> None:
        with HARNESS.NetworkProbeListeners() as probes:
            self.assertGreater(probes.tcp_port, 0)
            self.assertGreater(probes.udp_port, 0)
            self.assertEqual(probes._tcp_socket.getsockname()[0], "127.0.0.1")
            self.assertEqual(probes._udp_socket.getsockname()[0], "127.0.0.1")
            with socket.create_connection(("127.0.0.1", probes.tcp_port), timeout=1.0):
                pass
            udp = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
            try:
                udp.sendto(b"probe", ("127.0.0.1", probes.udp_port))
            finally:
                udp.close()
            self.assertEqual(probes.snapshot(), {"tcp_contacts": 1, "udp_contacts": 1})

    def test_listener_failure_cannot_be_reported_as_zero_contacts(self) -> None:
        class FailingListener:
            def accept(self):
                raise OSError("fixture socket failure")

        probes = HARNESS.NetworkProbeListeners()
        probes._started = True
        probes._tcp_socket = FailingListener()
        with self.assertRaisesRegex(runtime.HarnessFailure, "NETWORK_PROBE_LISTENER_FAILED"):
            probes.snapshot()

    def test_web_phase_does_not_release_script_navigation_without_the_ready_marker(self) -> None:
        harness, order, app = self.web_security_harness(focus_returns=True)
        def fail_ready_check(
            _app: Any, _name: str, _kind: str, failure_code: str, *_args: Any
        ) -> None:
            if failure_code == "SCRIPT_NAVIGATION_READY_TIMEOUT":
                raise runtime.HarnessFailure(failure_code)

        order.wait_name.side_effect = fail_ready_check
        with (
            mock.patch.object(HARNESS, "fixture_fingerprint", return_value=DISK),
            self.assertRaisesRegex(runtime.HarnessFailure, "SCRIPT_NAVIGATION_READY_TIMEOUT"),
        ):
            harness.phase_web_security(app, order.provider, Path(HARNESS.FIXTURE_FILE))
        order.menu_command.assert_called_once_with(app, runtime.MENU_WEB_AUTOMATION_ID)
        order.find_named.assert_not_called()
        order.click_control.assert_not_called()
        harness.win32.visible_top_level_windows.assert_not_called()

    def test_web_phase_fails_if_a_popup_creates_an_extra_owned_window(self) -> None:
        harness, order, app = self.web_security_harness(focus_returns=True)
        harness.win32.visible_top_level_windows.return_value = (app.hwnd, 93)
        with (
            mock.patch.object(HARNESS, "fixture_fingerprint", return_value=DISK),
            self.assertRaisesRegex(runtime.HarnessFailure, "APP_WINDOW_COUNT_UNEXPECTED"),
        ):
            harness.phase_web_security(app, order.provider, Path(HARNESS.FIXTURE_FILE))
        order.menu_command.assert_called_once_with(app, runtime.MENU_WEB_AUTOMATION_ID)
        order.editor_control.assert_not_called()

    def test_web_phase_fails_when_a_new_window_leaves_the_app_foreground(self) -> None:
        harness, order, app = self.web_security_harness(focus_returns=True)
        harness.win32.foreground_window.return_value = 93
        with (
            mock.patch.object(HARNESS, "fixture_fingerprint", return_value=DISK),
            self.assertRaisesRegex(runtime.HarnessFailure, "APP_WINDOW_FOREGROUND_LOST"),
        ):
            harness.phase_web_security(app, order.provider, Path(HARNESS.FIXTURE_FILE))
        harness.win32.visible_top_level_windows.assert_called_once_with((73, 74))

    def test_web_phase_fails_while_the_web_view_keeps_the_keyboard(self) -> None:
        harness, order, app = self.web_security_harness(focus_returns=False)
        with (
            mock.patch.object(HARNESS, "fixture_fingerprint", return_value=DISK),
            mock.patch.object(HARNESS, "wait_until", side_effect=wait_once),
            self.assertRaisesRegex(runtime.HarnessFailure, "WEB_TO_SOURCE_NATIVE_FOCUS_TIMEOUT"),
        ):
            harness.phase_web_security(app, order.provider, Path(HARNESS.FIXTURE_FILE))

    def decisions_harness(self, accept_offered: bool = False) -> tuple[HARNESS.Goal07Harness, mock.Mock]:
        order = mock.Mock()
        harness = bare_harness(network_probes=mock.Mock())
        for name in (
            "review_and_revise", "_named_texts", "editor_fingerprint", "decide_hunks",
            "undo_decision", "copy_approved", "replace_editor", "wait_name", "control_absent",
            "done", "menu_command",
        ):
            setattr(harness, name, getattr(order, name))
        order.review_and_revise.return_value = len(EDITS)
        order._named_texts.return_value = PROPOSAL_AFTER
        order.editor_fingerprint.side_effect = [REVIEWED, ONE_ACCEPTED]
        order.copy_approved.return_value = HARNESS.apply_edits(
            HARNESS.EDITOR_SOURCE_BYTES, EDITS[:1]
        ).decode("utf-8")
        order.replace_editor.return_value = STALE
        order.control_absent.return_value = not accept_offered
        return harness, order

    def test_decisions_phase_walks_accept_reject_undo_copy_stale_and_done(self) -> None:
        harness, order = self.decisions_harness()
        app, provider = mock.Mock(hwnd=73), order.provider
        with mock.patch.object(HARNESS, "fixture_fingerprint", return_value=DISK):
            observed = harness.phase_decisions(app, provider, Path(HARNESS.FIXTURE_FILE))

        self.assertEqual(
            order.mock_calls,
            [
                mock.call.review_and_revise(app, provider),
                mock.call._named_texts(
                    app, HARNESS.REVISION_PROPOSAL_AFTER_AUTOMATION_ID,
                    HARNESS.REVISION_PROPOSAL_AFTER_NAME, "REVISION_PROPOSAL_AFTER",
                ),
                mock.call.editor_fingerprint(app),
                mock.call.decide_hunks(app, [True], 2),
                mock.call.editor_fingerprint(app),
                mock.call.decide_hunks(app, [False], 2, decided=1),
                mock.call.undo_decision(app, 1, 2),
                mock.call.copy_approved(app),
                mock.call.replace_editor(app, HARNESS.STALE_EDIT),
                mock.call.wait_name(
                    app, HARNESS.REVISION_PART_CHANGED_TEXT, "Text",
                    "REVISION_PART_CHANGED_UIA_TIMEOUT",
                ),
                mock.call.control_absent(
                    app, HARNESS.REVISION_ACCEPT_AUTOMATION_ID, "Button",
                    "REVISION_ACCEPT_UIA_QUERY_FAILED", "REVISION_ACCEPT_UIA_CONTRACT_MISMATCH",
                ),
                mock.call.done(app, HARNESS.REVISION_APPLIED),
                mock.call.provider.verify_chain(),
            ],
        )
        shared_evidence.validate_case(
            HARNESS.CONTRACT,
            CASE,
            {**passing_observations(), **observed},
            process_context(),
        )
        self.assertEqual(
            {key for key in HARNESS.OBSERVATION_KEYS if key.startswith("decisions_")},
            set(observed),
        )

    def test_decisions_phase_canonicalizes_dynamic_ports_in_the_clipboard(self) -> None:
        probes = mock.Mock(tcp_port=43123, udp_port=54234)
        harness, order = self.decisions_harness()
        harness.network_probes = probes
        order.copy_approved.return_value = HARNESS.fixture_source(probes).replace(
            "\r\n", "\n"
        ).replace('window.goal07 = "old";', 'window.goal07 = "new";')

        with mock.patch.object(HARNESS, "fixture_fingerprint", return_value=DISK):
            observed = harness.phase_decisions(
                mock.Mock(hwnd=73), order.provider, Path(HARNESS.FIXTURE_FILE)
            )

        self.assertEqual(observed["decisions_clipboard_copied"], ONE_ACCEPTED.evidence())

    def test_a_wrong_measurement_stops_the_phase_before_its_next_step(self) -> None:
        # Accept wrote nothing: the read fails at once, before Reject could
        # time out first.
        harness, order = self.decisions_harness()
        order.editor_fingerprint.side_effect = [REVIEWED, REVIEWED]
        with (
            mock.patch.object(HARNESS, "fixture_fingerprint", return_value=DISK),
            self.assertRaisesRegex(runtime.HarnessFailure, "^EDITOR_ONE_ACCEPTED_UNEXPECTED$"),
        ):
            harness.phase_decisions(mock.Mock(hwnd=73), order.provider, Path(HARNESS.FIXTURE_FILE))
        self.assertEqual(order.decide_hunks.call_count, 1)
        order.copy_approved.assert_not_called()

    def test_decisions_phase_fails_when_the_changed_part_still_offers_accept(self) -> None:
        harness, order = self.decisions_harness(accept_offered=True)
        with (
            mock.patch.object(HARNESS, "fixture_fingerprint", return_value=DISK),
            self.assertRaisesRegex(runtime.HarnessFailure, "^REVISION_OFFERS_ACCEPT$"),
        ):
            harness.phase_decisions(mock.Mock(hwnd=73), order.provider, Path(HARNESS.FIXTURE_FILE))
        order.done.assert_not_called()


class StepTests(unittest.TestCase):
    """The UI steps the phases share."""

    def test_review_sends_its_shortcut_to_the_handed_back_keyboard_without_a_refocus(self) -> None:
        harness = bare_harness(
            focus_editor=mock.Mock(),
            win32=mock.Mock(),
            _approve_consent=mock.Mock(),
            wait_name=mock.Mock(),
        )
        app, provider = mock.Mock(hwnd=73), mock.Mock()
        harness.win32.foreground_focus.return_value = app.hwnd

        harness.run_review(app, provider)

        harness.focus_editor.assert_not_called()
        harness.win32.send_shortcut.assert_called_once_with(73, runtime.VK_RETURN)
        harness._approve_consent.assert_called_once_with(
            app, "REVIEW_CONSENT_HOLD_FAILED", provider=provider,
            expected_request_count=0, open_gate=provider.grant_review_consent,
        )
        harness.wait_name.assert_called_once_with(
            app, HARNESS.REVIEW_RESULT_MARKER, "Text", "REVIEW_RESULT_UIA_TIMEOUT", 60.0
        )

    def test_review_never_sends_keys_to_a_different_native_owner(self) -> None:
        for focused in (0, 74):
            with self.subTest(focused=focused):
                harness = bare_harness(win32=mock.Mock(), focus_editor=mock.Mock())
                harness.win32.foreground_focus.return_value = focused

                with self.assertRaisesRegex(runtime.HarnessFailure, "WEB_TO_SOURCE_NATIVE_FOCUS_LOST"):
                    harness.run_review(mock.Mock(hwnd=73), mock.Mock())

                harness.win32.send_shortcut.assert_not_called()
                harness.win32.send_inputs.assert_not_called()
                harness.focus_editor.assert_not_called()

    def test_the_chain_is_bound_to_the_document_before_the_review(self) -> None:
        order = mock.Mock()
        harness = bare_harness(
            run_review=order.run_review,
            request_revision=order.request_revision,
            editor_source_text=HARNESS.EDITOR_SOURCE_TEXT,
        )
        app, provider = mock.Mock(), order.provider

        hunks = harness.review_and_revise(app, provider)

        self.assertEqual(hunks, 2)
        self.assertEqual(
            order.mock_calls,
            [
                mock.call.provider.begin_chain(HARNESS.EDITOR_SOURCE_TEXT),
                mock.call.run_review(app, provider),
                mock.call.request_revision(app, provider=provider, hunks=2),
            ],
        )

    def test_revision_request_submits_answer_prepares_and_waits_for_hunks(self) -> None:
        order = mock.Mock()
        harness = bare_harness()
        for name in ("_set_answer", "_press", "_approve_consent", "wait_name"):
            setattr(harness, name, getattr(order, name))
        app, provider = mock.Mock(), mock.Mock()

        harness.request_revision(app, provider=provider, hunks=2)

        self.assertEqual(
            order.mock_calls,
            [
                mock.call._set_answer(app, PROVIDER.ANSWER_TEXT),
                mock.call._press(app, HARNESS.PREPARE_REVISION_AUTOMATION_ID, "REVISION_PREPARE"),
                mock.call._approve_consent(
                    app,
                    "REVISION_CONSENT_HOLD_FAILED",
                    provider=provider,
                    expected_request_count=1,
                    open_gate=provider.grant_revision_consent,
                ),
                mock.call.wait_name(
                    app, "0 of 2 decided", "Text", "REVISION_PROPOSAL_UIA_TIMEOUT", 60.0
                ),
            ],
        )

    def typing_harness(self, accepted_value: Callable[[str], str]) -> tuple[HARNESS.Goal07Harness, list]:
        events: list[tuple[str, object]] = []
        value = mock.Mock(CurrentValue="")

        class Control:
            iface_value = value

            def set_focus(self) -> None:
                events.append(("focus", None))

        control = Control()
        harness = bare_harness(win32=mock.Mock())
        harness.ui_timeout = 0.05
        harness.find_control = lambda _app, automation_id, *_args: (
            events.append(("find", automation_id)) or control
        )
        harness.control_by_id = lambda *_args, **_kwargs: control

        def type_text(hwnd: int, text: str) -> None:
            events.append(("send-unicode", (hwnd, text)))
            value.CurrentValue = accepted_value(text)

        harness.win32.send_unicode.side_effect = type_text
        harness._press = lambda _app, name, code: events.append(("press", (name, code)))
        return harness, events

    def test_answer_input_uses_the_focused_unicode_keyboard_path(self) -> None:
        harness, events = self.typing_harness(lambda text: text)

        harness._set_answer(mock.Mock(hwnd=41), "kept intent")

        self.assertEqual(
            events,
            [
                ("find", HARNESS.REVIEW_ANSWER_INPUT_AUTOMATION_ID),
                ("focus", None),
                ("send-unicode", (41, "kept intent")),
                ("press", (HARNESS.REVIEW_ANSWER_AUTOMATION_ID, "REVIEW_ANSWER_SUBMIT")),
            ],
        )

    def test_answer_input_times_out_before_submitting_a_wrong_value(self) -> None:
        harness, events = self.typing_harness(lambda _text: "other")

        with self.assertRaisesRegex(runtime.HarnessFailure, "REVIEW_ANSWER_INPUT_TIMEOUT"):
            harness._set_answer(mock.Mock(hwnd=41), "kept intent")
        self.assertNotIn("press", [event for event, _ in events])

    def test_decisions_wait_for_each_count_and_the_next_docked_hunk(self) -> None:
        harness = bare_harness()
        events: list[tuple[str, object]] = []
        # The decided hunk stays docked with its word for one poll, then the
        # next undecided hunk comes forward.
        docked_word = iter([True, False])
        harness._press = lambda _app, name, _code: events.append(("press", name))
        harness.wait_name = lambda _app, name, *_args: events.append(("wait", name))

        def name_present(_app, name, control_type):
            self.assertIn(name, HARNESS.REVISION_DECISION_WORDS)
            self.assertEqual(control_type, "Text")
            present = next(docked_word, False) if name == "Accepted" else False
            events.append(("docked", name)) if present else None
            return present

        harness.name_present = name_present

        harness.decide_hunks(mock.Mock(), [True, False], 2)
        harness.undo_decision(mock.Mock(), 1, 2)
        harness.decide_hunks(mock.Mock(), [True], 2, decided=1)
        harness.done(mock.Mock(), HARNESS.REVISION_APPLIED)

        self.assertEqual(
            events,
            [
                ("press", HARNESS.REVISION_ACCEPT_AUTOMATION_ID),
                ("wait", "1 of 2 decided"),
                ("docked", "Accepted"),
                ("press", HARNESS.REVISION_REJECT_AUTOMATION_ID),
                ("wait", "2 of 2 decided"),
                ("press", HARNESS.REVISION_UNDO_AUTOMATION_ID),
                ("wait", "1 of 2 decided"),
                ("press", HARNESS.REVISION_ACCEPT_AUTOMATION_ID),
                ("wait", "2 of 2 decided"),
                ("press", HARNESS.REVISION_DONE_AUTOMATION_ID),
                ("wait", HARNESS.REVISION_APPLIED),
            ],
        )

    def test_copy_reads_the_os_clipboard_after_clearing_it(self) -> None:
        order = mock.Mock()
        harness = bare_harness()
        for name in (
            "read_text_clipboard", "write_text_clipboard", "find_control", "click_control",
            "wait_clipboard_text",
        ):
            setattr(harness, name, getattr(order, name))
        order.wait_clipboard_text.return_value = "approved"
        app = mock.Mock()

        self.assertEqual(harness.copy_approved(app), "approved")
        self.assertEqual(
            order.mock_calls,
            [
                mock.call.write_text_clipboard(None),
                mock.call.find_control(
                    app, HARNESS.REVISION_COPY_AUTOMATION_ID, "Button",
                    "REVISION_COPY_UIA_TIMEOUT", "REVISION_COPY_UIA_CONTRACT_MISMATCH",
                ),
                mock.call.click_control(order.find_control.return_value, "REVISION_COPY_CLICK_FAILED"),
                mock.call.wait_clipboard_text("REVISION_COPY_CLIPBOARD_TIMEOUT"),
            ],
        )

    def test_proposal_text_joins_only_the_exposed_group_text_lines(self) -> None:
        def child(name: object, control_type: str = "Text") -> object:
            item = mock.Mock()
            item.element_info.name = name
            item.element_info.control_type = control_type
            return item

        group = mock.Mock()
        group.element_info.name = HARNESS.REVISION_PROPOSAL_AFTER_NAME
        group.children.return_value = [
            child("approved"), child("Accept", "Button"), child(None), child("preview"),
        ]
        harness = bare_harness(find_control=mock.Mock(return_value=group))

        def texts() -> str:
            return harness._named_texts(
                mock.Mock(),
                HARNESS.REVISION_PROPOSAL_AFTER_AUTOMATION_ID,
                HARNESS.REVISION_PROPOSAL_AFTER_NAME,
                "P",
            )

        self.assertEqual(texts(), "approved\npreview")
        self.assertEqual(harness.find_control.call_args.args[2], "Group")

        group.children.return_value = [child("Accept", "Button")]
        with self.assertRaisesRegex(runtime.HarnessFailure, "P_TEXT_UNAVAILABLE"):
            texts()

        group.children.return_value = [child("approved")]
        group.element_info.name = "Something else"
        with self.assertRaisesRegex(runtime.HarnessFailure, "P_UIA_CONTRACT_MISMATCH"):
            texts()

    def test_press_requires_foreground_first_and_finds_the_control_by_id(self) -> None:
        events: list[str] = []
        harness = bare_harness(win32=mock.Mock())
        harness.win32.require_foreground.side_effect = lambda *_args: events.append("foreground")
        harness.find_control = lambda _app, automation_id, control_type, *_args: (
            events.append(f"find-{automation_id}-{control_type}") or object()
        )
        harness.click_control = lambda _control, failure: events.append(f"click-{failure}")

        harness._press(mock.Mock(hwnd=41), HARNESS.REVISION_ACCEPT_AUTOMATION_ID, "REVISION_ACCEPT")

        self.assertEqual(
            events,
            ["foreground", "find-revision-accept-Button", "click-REVISION_ACCEPT_CLICK_FAILED"],
        )


class ConsentTests(unittest.TestCase):
    def test_consent_gate_rejects_any_pre_click_request_snapshot(self) -> None:
        harness = bare_harness(win32=mock.Mock(), find_control=lambda *_args, **_kwargs: object())
        events: list[str] = []

        class FakeProvider:
            def request_snapshot(self) -> dict[str, int | bool]:
                return {"request_count": 1, "invalid_request": False}

            def dispatch_consent_click(self, _count: int, click: Callable[[], None]) -> None:
                events.append("marker")
                click()

        with self.assertRaisesRegex(runtime.HarnessFailure, "REVIEW_REQUEST_BEFORE_CONSENT_CLICK"):
            harness._approve_consent(
                mock.Mock(process=mock.Mock(pid=7), hwnd=8),
                "REVIEW_CONSENT_HOLD_FAILED",
                provider=FakeProvider(),
                expected_request_count=0,
                open_gate=lambda: events.append("gate"),
            )
        self.assertEqual(events, [])

    def test_consent_gate_records_rejected_request_before_click(self) -> None:
        harness = bare_harness()
        provider = mock.Mock()
        provider.request_snapshot.return_value = {"request_count": 0, "invalid_request": True}
        with self.assertRaisesRegex(runtime.HarnessFailure, "REVISION_REQUEST_BEFORE_CONSENT_CLICK"):
            harness._open_consent_gate(provider, 1, lambda: None)

    def test_consent_dispatch_requires_request_count_and_gate(self) -> None:
        harness = bare_harness(
            win32=mock.Mock(),
            find_control=lambda *_args, **_kwargs: object(),
            hold_control=mock.Mock(),
        )
        provider = mock.Mock()
        app = mock.Mock(process=mock.Mock(pid=7), hwnd=8)

        for expected_request_count, open_gate in ((None, lambda: None), (0, None)):
            with self.subTest(expected_request_count=expected_request_count), self.assertRaisesRegex(
                runtime.HarnessFailure, "CONSENT_GATE_CONFIGURATION_INVALID"
            ):
                harness._approve_consent(
                    app,
                    "REVIEW_CONSENT_CLICK_FAILED",
                    provider=provider,
                    expected_request_count=expected_request_count,
                    open_gate=open_gate,
                )
        provider.dispatch_consent_click.assert_not_called()
        harness.hold_control.assert_not_called()

    def test_consent_gate_opens_before_the_consent_hold(self) -> None:
        events: list[str] = []
        send = object()
        harness = bare_harness(
            find_control=lambda _app, automation_id, *_args: (
                send if automation_id == HARNESS.CONSENT_SEND_AUTOMATION_ID else None
            ),
            hold_control=lambda _app, control, seconds, failure: events.append(
                f"hold-{control is send}-{seconds}-{failure}"
            ),
        )

        class FakeProvider:
            requests = 0

            def request_snapshot(self) -> dict[str, int | bool]:
                events.append(f"snapshot-{self.requests}")
                return {"request_count": self.requests, "invalid_request": False}

            def dispatch_consent_click(self, count: int, click: Callable[[], None]) -> None:
                events.append(f"marker-{count}")
                click()
                # The held send reaches the loopback after the hold.
                self.requests += 1

        harness._approve_consent(
            mock.Mock(process=mock.Mock(pid=7), hwnd=8),
            "REVIEW_CONSENT_HOLD_FAILED",
            provider=FakeProvider(),
            expected_request_count=0,
            open_gate=lambda: events.append("gate"),
        )

        self.assertEqual(
            events,
            [
                "snapshot-0",
                "gate",
                "marker-0",
                f"hold-True-{HARNESS.CONSENT_HOLD_SECONDS}-REVIEW_CONSENT_HOLD_FAILED",
                "snapshot-1",
            ],
        )
        self.assertEqual(HARNESS.CONSENT_HOLD_SECONDS, 1.8)

    def test_a_hold_whose_request_never_arrives_fails_fast_with_its_operation(self) -> None:
        # A real loopback that never receives the Review request.
        harness = bare_harness(
            find_control=lambda *_args: object(), hold_control=lambda *_args: None
        )
        with (
            PROVIDER.LoopbackRevisionServer() as server,
            mock.patch.object(HARNESS, "REQUEST_SENT_TIMEOUT_SECONDS", 0.2),
            self.assertRaisesRegex(runtime.HarnessFailure, "^REVIEW_REQUEST_NOT_SENT$"),
        ):
            server.begin_chain(HARNESS.EDITOR_SOURCE_TEXT)
            harness._approve_consent(
                mock.Mock(hwnd=8),
                "REVIEW_CONSENT_HOLD_FAILED",
                provider=server,
                expected_request_count=0,
                open_gate=server.grant_review_consent,
            )

        # The Revision hold counts from the Review already received.
        provider = mock.Mock()
        provider.request_snapshot.return_value = {"request_count": 1, "invalid_request": False}
        with (
            mock.patch.object(HARNESS, "REQUEST_SENT_TIMEOUT_SECONDS", 0.2),
            self.assertRaisesRegex(runtime.HarnessFailure, "^REVISION_REQUEST_NOT_SENT$"),
        ):
            harness._wait_request_sent(provider, 1)
        self.assertEqual(HARNESS.REQUEST_SENT_TIMEOUT_SECONDS, 5.0)

    def test_a_refused_request_fails_with_the_loopback_code_not_as_unsent(self) -> None:
        harness = bare_harness()
        provider = mock.Mock()
        provider.request_snapshot.return_value = {"request_count": 0, "invalid_request": True}
        provider.verify_chain.side_effect = runtime.HarnessFailure("LOOPBACK_STREAM_MODE_MISMATCH")
        with self.assertRaisesRegex(runtime.HarnessFailure, "^LOOPBACK_STREAM_MODE_MISMATCH$"):
            harness._wait_request_sent(provider, 0)


class ProfileAndFixtureTests(unittest.TestCase):
    def test_loopback_profile_is_keyless_and_uses_only_a_process_synthetic_key(self) -> None:
        settings = HARNESS.review_settings_document("http://127.0.0.1:4141/v1/").decode("utf-8")
        self.assertIn('model-provider = "openai-responses"', settings)
        self.assertNotIn("api-key", settings.casefold())
        self.assertIn("model-environment-key-identity", settings)
        self.assertIn(credential_target("http://127.0.0.1:4141/v1/"), settings)
        harness = bare_harness(_credential="markturbo-goal07-loopback-key")
        self.assertEqual(harness.openai_api_key_for_child(), "markturbo-goal07-loopback-key")

    def test_harness_credentials_are_fresh_uuid_values(self) -> None:
        with mock.patch.object(runtime.NativeHarness, "__init__", return_value=None):
            first = HARNESS.Goal07Harness()
            second = HARNESS.Goal07Harness()
        self.assertRegex(first.openai_api_key_for_child(), r"^markturbo-goal07-[0-9a-f]{32}$")
        self.assertRegex(second.openai_api_key_for_child(), r"^markturbo-goal07-[0-9a-f]{32}$")
        self.assertNotEqual(first.openai_api_key_for_child(), second.openai_api_key_for_child())

    def test_apply_edits_rejects_invalid_ranges_duplicate_offsets_and_oversized_output(self) -> None:
        invalid = [
            {"range": {"start": -1, "end": 0}, "expected_source": "", "replacement": "x"},
            {"range": {"start": 0, "end": 4}, "expected_source": "abc", "replacement": "x"},
        ]
        for edit in invalid:
            with self.subTest(edit=edit), self.assertRaisesRegex(ValueError, "invalid native fixture edit"):
                HARNESS.apply_edits(b"abc", [edit])

        split_character = {"range": {"start": 1, "end": 1}, "expected_source": "", "replacement": "x"}
        with self.assertRaisesRegex(ValueError, "invalid native fixture edit"):
            HARNESS.apply_edits("é".encode("utf-8"), [split_character])

        duplicate_insertions = [
            {"range": {"start": 1, "end": 1}, "expected_source": "", "replacement": "x"},
            {"range": {"start": 1, "end": 1}, "expected_source": "", "replacement": "y"},
        ]
        with self.assertRaisesRegex(ValueError, "invalid native fixture edit"):
            HARNESS.apply_edits(b"abc", duplicate_insertions)

        overlapping = [
            {"range": {"start": 0, "end": 2}, "expected_source": "ab", "replacement": "x"},
            {"range": {"start": 1, "end": 3}, "expected_source": "bc", "replacement": "y"},
        ]
        with self.assertRaisesRegex(ValueError, "invalid native fixture edit"):
            HARNESS.apply_edits(b"abc", overlapping)

        with mock.patch.object(HARNESS, "MAX_EDIT_OUTPUT_BYTES", 3):
            oversized = {"range": {"start": 0, "end": 1}, "expected_source": "a", "replacement": "xxxx"}
            with self.assertRaisesRegex(ValueError, "invalid native fixture edit"):
                HARNESS.apply_edits(b"abc", [oversized])

        no_op = {"range": {"start": 0, "end": 1}, "expected_source": "a", "replacement": "a"}
        with self.assertRaisesRegex(ValueError, "invalid native fixture edit"):
            HARNESS.apply_edits(b"abc", [no_op])

        with mock.patch.object(HARNESS, "MAX_EDIT_OUTPUT_BYTES", 4):
            subset_only_oversized = [
                {"range": {"start": 0, "end": 3}, "expected_source": "abc", "replacement": ""},
                {"range": {"start": 4, "end": 4}, "expected_source": "", "replacement": "xyz"},
            ]
            with self.assertRaisesRegex(ValueError, "invalid native fixture edit"):
                HARNESS.apply_edits(b"abcd", subset_only_oversized)


if __name__ == "__main__":
    unittest.main()
