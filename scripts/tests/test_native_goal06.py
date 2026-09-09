"""Unit tests for the Goal 06 native harness without launching a UI."""

from __future__ import annotations

import argparse
import copy
import hashlib
import tempfile
import tomllib
import unittest
from pathlib import Path
from unittest import mock

from scripts.markturbo_tools.native import goal06 as HARNESS
from scripts.markturbo_tools.native import runtime


SCRIPT = Path(HARNESS.__file__)
CASE_DOCUMENT = HARNESS.CASE_DOCUMENT
CASE_SELECTION = HARNESS.CASE_SELECTION
COMPLETE_EVIDENCE = HARNESS.complete_evidence
NEW_EVIDENCE = HARNESS.new_evidence
PARSE_ARGS = HARNESS.parse_args
REQUIRED_CASE_IDS = HARNESS.REQUIRED_CASE_IDS
REVIEW_SETTINGS_DOCUMENT = HARNESS.review_settings_document
RUN = HARNESS.run
SOURCE_CONTRACT_FAILURE = HARNESS.source_contract_failure
VALIDATE_EVIDENCE = HARNESS.validate_evidence

HASH = "a" * 64


def fingerprint(value: bytes) -> dict[str, int | str]:
    return {"byte_count": len(value), "sha256": hashlib.sha256(value).hexdigest()}


def process_context() -> dict[str, int | str]:
    return {"session_id": 1, "integrity_rid": 0x2000, "integrity": "medium"}


def runtime_scan(configured_provider: bool = False) -> dict[str, int | bool]:
    scan = {
        "files_scanned": 3,
        "app_logs_scanned": 1,
        "config_files_scanned": 1,
        "utf8_sentinel_absent": True,
        "utf16le_sentinel_absent": True,
    }
    if configured_provider:
        scan.update(
            ephemeral_credential_utf8_absent=True,
            ephemeral_credential_utf16le_absent=True,
        )
    return scan


def valid_evidence(configured_provider: bool = False) -> dict:
    evidence = NEW_EVIDENCE(HASH, configured_provider)
    evidence["executable"].update(
        {
            "sha256": HASH,
            "byte_count": 1234,
            "hash_verified": True,
            "copied_sha256": HASH,
            "copy_hash_verified": True,
            "format": "PE32+",
            "machine": "x86_64",
            "machine_code": 0x8664,
            "optional_magic": 0x20B,
        }
    )
    evidence["environment"] = {
        "platform": "Windows 11",
        "windows_major": 10,
        "windows_build": 22631,
        "architecture": "x86_64",
        "native_machine_code": 0x8664,
        "python_pointer_bits": 64,
        "wts_state": "WTSActive",
        "input_desktop": "Default",
        "thread_desktop": "Default",
        "harness_process": process_context(),
    }
    original = fingerprint(b"review source")
    for case in evidence["cases"]:
        case["status"] = "PASS"
        case["duration_ms"] = 1.0
        case["observations"] = {
            "editor_before": original,
            "editor_after": original,
            "source_before": original,
            "source_after": original,
            "no_dirty_interlock": True,
            "flow": (
                HARNESS.CONFIGURED_PROVIDER_CASE_FLOWS[case["id"]]
                if configured_provider
                else HARNESS.CASE_FLOWS[case["id"]]
            ),
            "process_context": process_context(),
            "foreground_verified": True,
            "runtime_scan": runtime_scan(configured_provider),
        }
        if configured_provider:
            case["observations"].update(
                review_consent_approved=True,
                structured_success=True,
            )
        else:
            case["observations"]["missing_credential_diagnostic"] = True
        if case["id"] == CASE_SELECTION:
            case["observations"]["selection_shortcut"] = True
        else:
            case["observations"]["document_shortcut"] = True
    COMPLETE_EVIDENCE(evidence, "PASS")
    return evidence


class EvidenceSchemaTests(unittest.TestCase):
    def test_accepts_complete_hash_bound_evidence(self) -> None:
        evidence = valid_evidence()

        VALIDATE_EVIDENCE(evidence)

        self.assertEqual(evidence["status"], "PASS")
        self.assertEqual([case["id"] for case in evidence["cases"]], list(REQUIRED_CASE_IDS))

    def test_accepts_complete_configured_provider_success_evidence(self) -> None:
        evidence = valid_evidence(configured_provider=True)

        VALIDATE_EVIDENCE(evidence)

        self.assertEqual(evidence["mode"], HARNESS.CONFIGURED_PROVIDER_MODE)

    def test_configured_provider_success_requires_consent_and_structured_result(self) -> None:
        evidence = valid_evidence(configured_provider=True)
        evidence["cases"][0]["observations"]["structured_success"] = False

        with self.assertRaisesRegex(ValueError, "structured_success"):
            VALIDATE_EVIDENCE(evidence)

        evidence = valid_evidence(configured_provider=True)
        evidence["cases"][1]["observations"]["review_consent_approved"] = False

        with self.assertRaisesRegex(ValueError, "review_consent_approved"):
            VALIDATE_EVIDENCE(evidence)

    def test_configured_provider_success_requires_content_free_credential_scan(self) -> None:
        evidence = valid_evidence(configured_provider=True)
        evidence["cases"][0]["observations"]["runtime_scan"].pop(
            "ephemeral_credential_utf8_absent"
        )

        with self.assertRaisesRegex(ValueError, "ephemeral_credential_utf8_absent"):
            VALIDATE_EVIDENCE(evidence)

    def test_pass_requires_identical_editor_and_file_fingerprints(self) -> None:
        evidence = valid_evidence()
        evidence["cases"][0]["observations"]["editor_after"] = fingerprint(b"changed")

        with self.assertRaisesRegex(ValueError, "changed editor"):
            VALIDATE_EVIDENCE(evidence)

        evidence = valid_evidence()
        evidence["cases"][1]["observations"]["source_after"] = fingerprint(b"changed")

        with self.assertRaisesRegex(ValueError, "changed source"):
            VALIDATE_EVIDENCE(evidence)

    def test_pass_requires_each_shortcut_and_missing_credential_diagnostic(self) -> None:
        evidence = valid_evidence()
        evidence["cases"][0]["observations"]["document_shortcut"] = False

        with self.assertRaisesRegex(ValueError, "document_shortcut"):
            VALIDATE_EVIDENCE(evidence)

        evidence = valid_evidence()
        evidence["cases"][1]["observations"]["missing_credential_diagnostic"] = False

        with self.assertRaisesRegex(ValueError, "missing_credential_diagnostic"):
            VALIDATE_EVIDENCE(evidence)

    def test_blocked_foreground_diagnostics_are_retained_as_content_free_evidence(self) -> None:
        evidence = NEW_EVIDENCE(HASH)
        evidence["cases"][0].update(
            status="BLOCKED",
            reason_code="FOREGROUND_PERMISSION_DENIED",
            duration_ms=1.0,
            observations={
                "requested_hwnd": 42,
                "foreground_hwnd": 0,
                "show_window_return": True,
                "bring_to_top_return": False,
                "set_foreground_return": False,
                "foreground_attempts": 80,
            },
        )
        evidence["cases"][1].update(
            status="BLOCKED",
            reason_code="SKIPPED_AFTER_BLOCKED",
            duration_ms=0.0,
        )
        COMPLETE_EVIDENCE(evidence, "BLOCKED")

        VALIDATE_EVIDENCE(evidence)


class ConfigurationTests(unittest.TestCase):
    def test_review_profile_configures_responses_without_a_credential(self) -> None:
        document = REVIEW_SETTINGS_DOCUMENT("https://run-unique.invalid/v1/")

        settings = tomllib.loads(document.decode("utf-8"))

        self.assertEqual(settings["model-provider"], "openai-responses")
        self.assertEqual(settings["model-name"], "gpt-5.6-terra")
        self.assertEqual(settings["model-base-url"], "https://run-unique.invalid/v1/")
        self.assertNotIn("credential", document.decode("utf-8").casefold())
        self.assertNotIn("api-key", document.decode("utf-8").casefold())

    def test_configured_provider_profile_uses_only_fixed_loopback_identity(self) -> None:
        identity = HARNESS.configured_provider_environment_key_identity()
        document = REVIEW_SETTINGS_DOCUMENT(HARNESS.CONFIGURED_PROVIDER_ENDPOINT, identity)

        settings = tomllib.loads(document.decode("utf-8"))
        expected_hash = hashlib.sha256(
            b"".join(
                component + b"\0"
                for component in (
                    b"io.github.wxxb789.markturbo",
                    b"openai-responses",
                    b"http",
                    b"127.0.0.1",
                    b"4141",
                    b"/v1/",
                )
            )
        ).hexdigest()
        expected_identity = (
            "io.github.wxxb789.markturbo:model-credential:v2|wire=openai-responses"
            f"|host=127.0.0.1|identity-sha256={expected_hash}"
        )

        self.assertEqual(settings["model-base-url"], HARNESS.CONFIGURED_PROVIDER_ENDPOINT)
        self.assertEqual(settings["model-environment-key-identity"], expected_identity)
        self.assertNotIn("api-key", document.decode("utf-8").casefold())

    def test_opt_in_environment_contains_only_a_fresh_process_only_openai_key(self) -> None:
        harness = object.__new__(HARNESS.Goal06Harness)
        harness.configured_provider = False
        self.assertIsNone(harness.openai_api_key_for_child())

        harness.configured_provider = True
        first = harness.openai_api_key_for_child()
        second = harness.openai_api_key_for_child()

        self.assertTrue(first)
        self.assertNotEqual(
            hashlib.sha256(first.encode()).digest(),
            hashlib.sha256(second.encode()).digest(),
        )

    def test_launch_spec_scrubs_inherited_key_before_applying_opt_in_child_key(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary).resolve()
            spec = runtime.build_launch_spec(
                root / "markturbo.exe",
                None,
                root / "data",
                root / "config",
                root / "workspace",
                root / "stderr.log",
                {"PATH": "path", "OPENAI_API_KEY": "inherited"},
                ephemeral_openai_api_key="process-only",
            )

        self.assertEqual(spec.env["OPENAI_API_KEY"], "process-only")
        self.assertNotEqual(spec.env["OPENAI_API_KEY"], "inherited")

    def test_runtime_scan_rejects_an_ephemeral_credential_leak_without_recording_its_value(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            case_root = Path(temporary)
            logs = case_root / "data" / "logs"
            logs.mkdir(parents=True)
            (case_root / "config").mkdir()
            (logs / "markturbo.log").write_text("process-only", encoding="utf-8")

            with self.assertRaisesRegex(runtime.HarnessFailure, "UTF8_EPHEMERAL_CREDENTIAL_LEAKED"):
                HARNESS.scan_case_artifacts(case_root, "process-only")


class PersistentCredentialPreflightTests(unittest.TestCase):
    def test_configured_mode_blocks_before_launch_when_its_target_has_a_persistent_credential(self) -> None:
        calls: list[str] = []
        win32 = mock.Mock()
        win32.persistent_credential_target_exists.side_effect = (
            lambda target: calls.append(target) or True
        )
        parent = object()
        evidence: dict = {}

        with mock.patch.object(HARNESS, "preflight", return_value=(win32, parent)) as preflight:
            with self.assertRaisesRegex(runtime.HarnessBlocked, "PERSISTENT_CREDENTIAL_PRESENT"):
                HARNESS.goal06_preflight(True)(Path("C:/release/markturbo.exe"), HASH, evidence)

        preflight.assert_called_once_with(Path("C:/release/markturbo.exe"), HASH, evidence)
        self.assertEqual(calls, [HARNESS.configured_provider_environment_key_identity()])

    def test_default_mode_does_not_query_persistent_credentials(self) -> None:
        win32 = mock.Mock()
        parent = object()
        evidence: dict = {}

        with mock.patch.object(HARNESS, "preflight", return_value=(win32, parent)):
            actual = HARNESS.goal06_preflight(False)(
                Path("C:/release/markturbo.exe"), HASH, evidence
            )

        self.assertEqual(actual, (win32, parent))
        win32.persistent_credential_target_exists.assert_not_called()

    def test_windows_metadata_preflight_never_dereferences_a_credential_blob(self) -> None:
        source = Path(runtime.__file__).read_text(encoding="utf-8")
        method = source.split("    def persistent_credential_target_exists", 1)[1].split(
            "    def send_inputs", 1
        )[0]

        self.assertIn("CredEnumerateW", method)
        self.assertNotIn("CredentialBlob", method)


class HarnessContractTests(unittest.TestCase):
    def test_native_harness_requires_stable_review_controls_and_shortcuts(self) -> None:
        self.assertIsNone(SOURCE_CONTRACT_FAILURE())

    def test_diagnostic_accessibility_value_exposes_its_content_free_message(self) -> None:
        workspace = (HARNESS.REPO / "crates" / "mt-app" / "src" / "views" / "workspace.rs").read_text(
            encoding="utf-8"
        )
        production = workspace.split("\n#[cfg(test)]", 1)[0]

        self.assertIn("aria_value(diagnostic.diagnostic.message.as_str())", production)

    def test_scenario_activates_the_source_layout_before_reading_editor_state(self) -> None:
        source = SCRIPT.read_text(encoding="utf-8")
        scenario = source.split("def scenario(", 1)[1].split("def native_run_plan", 1)[0]

        self.assertLess(
            scenario.index("self.activate_source_layout(app)"),
            scenario.index("self.editor_fingerprint(app)"),
        )

    def test_structured_success_uses_the_stable_result_label(self) -> None:
        calls: list[tuple[str, str, str, str]] = []
        result = type("Result", (), {"element_info": type("Info", (), {"name": "Review ready"})()})()

        class FakeHarness:
            def find_control(
                self,
                app: object,
                accessibility_id: str,
                control_type: str,
                timeout_code: str,
                mismatch_code: str,
            ) -> object:
                calls.append((accessibility_id, control_type, timeout_code, mismatch_code))
                return result

        HARNESS.Goal06Harness.require_structured_success_result(FakeHarness(), object())

        self.assertEqual(
            calls,
            [
                (
                    HARNESS.REVIEW_RESULT_ACCESSIBILITY_ID,
                    "Text",
                    "REVIEW_RESULT_UIA_TIMEOUT",
                    "REVIEW_RESULT_UIA_CONTRACT_MISMATCH",
                )
            ],
        )

    def test_configured_mode_approves_the_send_task_dialog(self) -> None:
        events: list[tuple[str, object]] = []
        button = object()
        app = type("App", (), {"process": type("Process", (), {"pid": 7})(), "hwnd": 42})()

        class FakeWin32:
            def owned_task_dialogs(self, process_id: int, owner_hwnd: int) -> list[int]:
                events.append(("dialogs", (process_id, owner_hwnd)))
                return [99]

        class FakeHarness:
            win32 = FakeWin32()
            ui_timeout = 1.0

            def control_by_id(self, *args: object) -> object:
                events.append(("control", args))
                return button

            def click_control(self, value: object, failure_code: str) -> None:
                events.append(("click", (value, failure_code)))

        HARNESS.Goal06Harness.approve_review_consent(FakeHarness(), app)

        self.assertEqual(events[0], ("dialogs", (7, 42)))
        self.assertEqual(events[-1], ("click", (button, "REVIEW_CONSENT_CLICK_FAILED")))

    def test_cli_arguments_require_a_hash_and_accept_each_case(self) -> None:
        for case_id in REQUIRED_CASE_IDS:
            parsed = PARSE_ARGS(["--expect-exe-sha256", HASH, "--case", case_id])
            self.assertEqual(parsed.case, case_id)
            self.assertFalse(parsed.configured_provider)

        configured = PARSE_ARGS(["--expect-exe-sha256", HASH, "--configured-provider"])
        self.assertTrue(configured.configured_provider)

    def test_run_delegates_to_the_shared_hash_bound_runtime(self) -> None:
        args = argparse.Namespace(expect_exe_sha256=HASH)
        expected = (0, {"status": "PASS"}, "evidence")
        with mock.patch.object(HARNESS, "run_native_acceptance", return_value=expected) as run:
            self.assertEqual(RUN(args), expected)

        run.assert_called_once()

    def test_close_app_posts_close_and_waits_for_clean_exit(self) -> None:
        events: list[tuple[str, int]] = []

        class FakeWin32:
            def post_close(self, hwnd: int) -> None:
                events.append(("close", hwnd))

        class FakeHarness:
            win32 = FakeWin32()

            def wait_process_exit(self, app: object) -> None:
                events.append(("wait", app.hwnd))

        app = type("App", (), {"hwnd": 42})()

        HARNESS.Goal06Harness.close_app(FakeHarness(), app)

        self.assertEqual(events, [("close", 42), ("wait", 42)])

    def test_source_contract_cannot_be_satisfied_by_test_only_controls(self) -> None:
        source = SCRIPT.read_text(encoding="utf-8")
        self.assertIn("production_source", source)
        self.assertIn("REVIEW_RUN_ACCESSIBILITY_ID", source)
        self.assertIn("REVIEW_DIAGNOSTIC_ACCESSIBILITY_ID", source)
