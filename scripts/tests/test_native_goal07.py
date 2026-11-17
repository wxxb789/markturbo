"""Unit tests for the Goal 07 native harness without launching a UI."""

from __future__ import annotations

import argparse
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
        **HARNESS.expected_fingerprints(),
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
        self.assertIn(b'<script>window.goal07 = "new";</script>\r\n<p>owner: TBD</p>', accepted)
        self.assertIn("<p>trusted 文档 🚀</p>\r\n".encode("utf-8"), accepted)

    def test_each_fingerprint_must_hold_its_expected_bytes(self) -> None:
        for key in sorted(HARNESS.FINGERPRINT_OBSERVATION_KEYS):
            with self.subTest(key=key):
                evidence = valid_evidence()
                evidence["cases"][0]["observations"][key] = fingerprint(b"other bytes")
                with self.assertRaisesRegex(ValueError, f"{key} does not hold the expected bytes"):
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

    def test_profile_writes_only_the_trusted_document_and_never_touches_credential_manager(
        self,
    ) -> None:
        win32 = mock.Mock()
        harness = bare_harness(win32=win32)
        with tempfile.TemporaryDirectory() as directory:
            harness.root = Path(directory)
            case_root, data, config, workspace, stderr = harness.profile(
                "http://127.0.0.1:43123/v1/"
            )
            self.assertEqual(case_root, data.parent)
            self.assertEqual(
                {path.name: path.read_bytes() for path in workspace.iterdir()},
                {HARNESS.TRUST_FILE: HARNESS.SOURCE_BYTES},
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
            phase_trust=order.phase_trust,
            phase_decisions=order.phase_decisions,
            quit_via_menu=order.quit_via_menu,
            click_lifecycle_decision=order.click_lifecycle_decision,
            wait_process_exit=order.wait_process_exit,
            reap=order.reap,
            _finalize_observations=lambda _provider, _root, _app, observations: observations,
        )
        order.launch_app.return_value = app
        order.phase_trust.return_value = {"trust_disk_before": DISK.evidence()}
        order.phase_decisions.return_value = {"decisions_editor_stale": STALE.evidence()}
        return harness, order, app, provider

    def test_one_launch_on_the_document_runs_both_phases_then_quits_through_the_guard(self) -> None:
        harness, order, app, provider = self.session_harness()
        with mock.patch.object(PROVIDER, "LoopbackRevisionServer", return_value=provider):
            observations = harness.scenario_revision_session()

        source = Path("workspace") / HARNESS.TRUST_FILE
        self.assertEqual(
            # The app is the launch's return value: its own calls are not steps.
            [call for call in order.mock_calls if not call[0].startswith("launch_app()")],
            [
                mock.call.launch_app(
                    source, Path("data"), Path("config"), Path("workspace"), Path("stderr"),
                ),
                mock.call.phase_trust(app, provider, source),
                mock.call.phase_decisions(app, provider, source),
                mock.call.quit_via_menu(app),
                mock.call.click_lifecycle_decision(app, "Discard"),
                mock.call.wait_process_exit(app),
                mock.call.reap(app),
            ],
        )
        self.assertEqual(observations["trust_disk_before"], DISK.evidence())
        self.assertEqual(observations["decisions_editor_stale"], STALE.evidence())
        self.assertEqual(observations["flow"], HARNESS.CASE_FLOWS[CASE])
        provider.close.assert_called_once_with()

    def test_a_phase_failure_names_its_phase_and_stops_the_session(self) -> None:
        harness, order, app, provider = self.session_harness()
        order.phase_trust.side_effect = runtime.HarnessFailure("SCRIPTS_TRUSTED_TIMEOUT")
        with (
            mock.patch.object(PROVIDER, "LoopbackRevisionServer", return_value=provider),
            self.assertRaisesRegex(runtime.HarnessFailure, "^TRUST_SCRIPTS_TRUSTED_TIMEOUT$"),
        ):
            harness.scenario_revision_session()
        order.phase_decisions.assert_not_called()
        order.quit_via_menu.assert_not_called()
        order.reap.assert_called_once_with(app)
        provider.close.assert_called_once_with()

    def test_an_unexpected_measurement_fails_with_its_phase_key(self) -> None:
        for key, value, code in (
            ("trust_disk_before", REVIEWED, "TRUST_DISK_BEFORE_UNEXPECTED"),
            ("decisions_editor_one_accepted", ALL_ACCEPTED, "DECISIONS_EDITOR_ONE_ACCEPTED_UNEXPECTED"),
            ("decisions_clipboard_copied", ALL_ACCEPTED, "DECISIONS_CLIPBOARD_COPIED_UNEXPECTED"),
            ("decisions_disk_after", ONE_ACCEPTED, "DECISIONS_DISK_AFTER_UNEXPECTED"),
        ):
            with self.subTest(code=code):
                observed: dict[str, Any] = {}
                with self.assertRaisesRegex(runtime.HarnessFailure, f"^{code}$"):
                    with HARNESS.phase(key.partition("_")[0].upper()):
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

    def trust_harness(self, focus_returns: bool) -> tuple[HARNESS.Goal07Harness, mock.Mock, mock.Mock]:
        order = mock.Mock()
        app = mock.Mock(hwnd=73)
        harness = bare_harness(win32=mock.Mock())
        for name in (
            "menu_command", "_press", "confirm_system_alert", "wait_name", "editor_control",
        ):
            setattr(harness, name, getattr(order, name))
        harness.name_present = mock.Mock(return_value=False)
        harness.win32.foreground_focus.return_value = app.hwnd if focus_returns else 0
        return harness, order, app

    def test_trust_phase_trusts_scripts_then_hands_the_keyboard_back(self) -> None:
        harness, order, app = self.trust_harness(focus_returns=True)
        with (
            mock.patch.object(HARNESS, "sha256_file", return_value=DISK),
            mock.patch.object(HARNESS, "wait_until", side_effect=wait_once),
        ):
            observed = harness.phase_trust(app, order.provider, Path("trusted.html"))

        self.assertEqual(
            order.mock_calls,
            [
                mock.call.menu_command(app, runtime.MENU_WEB_AUTOMATION_ID),
                mock.call._press(app, HARNESS.TRUST_SCRIPTS_AUTOMATION_ID, "WEB_TRUST"),
                mock.call.confirm_system_alert(app, mock.ANY, "SCRIPTS_TRUSTED_BEFORE_CONFIRMATION"),
                mock.call.wait_name(app, HARNESS.TRUSTED_BANNER, "Text", "SCRIPTS_TRUSTED_TIMEOUT"),
                mock.call.menu_command(app, runtime.MENU_EDIT_AUTOMATION_ID),
                mock.call.editor_control(app),
            ],
        )
        self.assertEqual(observed, {"trust_disk_before": DISK.evidence()})
        # The Run scripts alert is outside UIA: Enter answers it only while
        # trust is still off, then the banner proves it.
        still_undecided = order.confirm_system_alert.call_args.args[1]
        self.assertTrue(still_undecided())
        harness.name_present.return_value = True
        self.assertFalse(still_undecided())
        harness.name_present.assert_called_with(app, HARNESS.TRUSTED_BANNER, "Text")

    def test_trust_phase_fails_while_the_web_view_keeps_the_keyboard(self) -> None:
        harness, order, app = self.trust_harness(focus_returns=False)
        with (
            mock.patch.object(HARNESS, "sha256_file", return_value=DISK),
            mock.patch.object(HARNESS, "wait_until", side_effect=wait_once),
            self.assertRaisesRegex(runtime.HarnessFailure, "WEB_TO_SOURCE_NATIVE_FOCUS_TIMEOUT"),
        ):
            harness.phase_trust(app, order.provider, Path("trusted.html"))

    def decisions_harness(self, accept_offered: bool = False) -> tuple[HARNESS.Goal07Harness, mock.Mock]:
        order = mock.Mock()
        harness = bare_harness()
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

    def test_decisions_phase_walks_accept_reject_undo_copy_stale_done_restricted(self) -> None:
        harness, order = self.decisions_harness()
        app, provider = mock.Mock(hwnd=73), order.provider
        with mock.patch.object(HARNESS, "sha256_file", return_value=DISK):
            observed = harness.phase_decisions(app, provider, Path("trusted.html"))

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
                mock.call.menu_command(app, runtime.MENU_WEB_AUTOMATION_ID),
                mock.call.wait_name(
                    app, HARNESS.RESTRICTED_BANNER, "Text", "SCRIPTS_RESTRICTED_TIMEOUT"
                ),
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

    def test_a_wrong_measurement_stops_the_phase_before_its_next_step(self) -> None:
        # Accept wrote nothing: the read fails at once, before Reject could
        # time out first.
        harness, order = self.decisions_harness()
        order.editor_fingerprint.side_effect = [REVIEWED, REVIEWED]
        with (
            mock.patch.object(HARNESS, "sha256_file", return_value=DISK),
            self.assertRaisesRegex(runtime.HarnessFailure, "^EDITOR_ONE_ACCEPTED_UNEXPECTED$"),
        ):
            harness.phase_decisions(mock.Mock(hwnd=73), order.provider, Path("trusted.html"))
        self.assertEqual(order.decide_hunks.call_count, 1)
        order.copy_approved.assert_not_called()

    def test_decisions_phase_fails_when_the_changed_part_still_offers_accept(self) -> None:
        harness, order = self.decisions_harness(accept_offered=True)
        with (
            mock.patch.object(HARNESS, "sha256_file", return_value=DISK),
            self.assertRaisesRegex(runtime.HarnessFailure, "^REVISION_OFFERS_ACCEPT$"),
        ):
            harness.phase_decisions(mock.Mock(hwnd=73), order.provider, Path("trusted.html"))
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
        harness = bare_harness(run_review=order.run_review, request_revision=order.request_revision)
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
