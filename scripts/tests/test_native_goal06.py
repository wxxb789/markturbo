"""Unit tests for the Goal 06 native harness without launching a UI."""

from __future__ import annotations

import ctypes
import hashlib
import tempfile
import tomllib
import unittest
from pathlib import Path
from unittest import mock

from scripts.markturbo_tools.native import evidence as shared_evidence
from scripts.markturbo_tools.native import goal06 as HARNESS
from scripts.markturbo_tools.native import runtime

from ._native_support import HASH, case_observations, fingerprint, passed_evidence

CASE_DOCUMENT = HARNESS.CASE_DOCUMENT
CASE_SELECTION = HARNESS.CASE_SELECTION
NEW_EVIDENCE = HARNESS.new_evidence
PARSE_ARGS = HARNESS.parse_args
REQUIRED_CASE_IDS = HARNESS.REQUIRED_CASE_IDS
REVIEW_SETTINGS_DOCUMENT = HARNESS.review_settings_document
VALIDATE_EVIDENCE = HARNESS.validate_evidence
CONFIGURED_IDENTITY = shared_evidence.credential_target(
    HARNESS.CONFIGURED_PROVIDER_ENDPOINT, "openai-responses"
)


def complete(evidence: dict, status: str) -> None:
    runtime.complete_evidence(evidence, status, REQUIRED_CASE_IDS)


def valid_evidence(configured_provider: bool = False) -> dict:
    flows = HARNESS.CONFIGURED_PROVIDER_CASE_FLOWS if configured_provider else HARNESS.CASE_FLOWS
    original = fingerprint(b"review source")

    def observations(case_id: str) -> dict:
        mode = (
            {"review_consent_approved": True, "structured_success": True}
            if configured_provider
            else {"missing_credential_diagnostic": True}
        )
        return case_observations(
            flows[case_id],
            editor_before=original,
            editor_after=original,
            source_before=original,
            source_after=original,
            no_dirty_interlock=True,
            document_shortcut=case_id == CASE_DOCUMENT,
            selection_shortcut=case_id == CASE_SELECTION,
            **mode,
        )

    return passed_evidence(NEW_EVIDENCE(HASH, configured_provider), observations, complete)


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

    def test_mode_selects_its_own_flows_and_observations(self) -> None:
        evidence = valid_evidence(configured_provider=True)
        evidence["mode"] = HARNESS.MISSING_CREDENTIAL_MODE
        with self.assertRaisesRegex(ValueError, "unknown observation field"):
            VALIDATE_EVIDENCE(evidence)

        evidence["mode"] = "unknown"
        with self.assertRaisesRegex(ValueError, "invalid native Review mode"):
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
        complete(evidence, "BLOCKED")

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
        identity = CONFIGURED_IDENTITY
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
        self.assertEqual(calls, [CONFIGURED_IDENTITY])

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

    def test_persistent_credential_block_does_not_expose_credential_content(self) -> None:
        target = CONFIGURED_IDENTITY
        secret = b"UNIQUE-CREDENTIAL-BLOB"
        target_buffer = ctypes.create_unicode_buffer(target)
        blob = (ctypes.c_ubyte * len(secret)).from_buffer_copy(secret)
        credential = runtime.CREDENTIALW()
        credential.TargetName = ctypes.cast(target_buffer, runtime.wt.LPWSTR)
        credential.CredentialBlobSize = len(secret)
        credential.CredentialBlob = ctypes.cast(blob, ctypes.POINTER(ctypes.c_ubyte))
        records = (ctypes.POINTER(runtime.CREDENTIALW) * 1)(ctypes.pointer(credential))
        records_pointer = ctypes.cast(
            records, ctypes.POINTER(ctypes.POINTER(runtime.CREDENTIALW))
        )
        enumerated: list[str] = []
        freed: list[object] = []

        class FakeAdvapi32:
            def CredEnumerateW(
                self,
                requested_target: str,
                _flags: int,
                count_pointer: object,
                credentials_pointer: object,
            ) -> bool:
                enumerated.append(requested_target)
                ctypes.cast(count_pointer, ctypes.POINTER(runtime.wt.DWORD)).contents.value = 1
                output = ctypes.cast(
                    credentials_pointer,
                    ctypes.POINTER(
                        ctypes.POINTER(ctypes.POINTER(runtime.CREDENTIALW))
                    ),
                )
                output[0] = records_pointer
                return True

            def CredFree(self, credentials: object) -> None:
                freed.append(credentials)

        win32 = object.__new__(runtime.Win32)
        win32.advapi32 = FakeAdvapi32()
        evidence: dict = {}

        with mock.patch.object(HARNESS, "preflight", return_value=(win32, object())):
            with self.assertRaises(runtime.HarnessBlocked) as raised:
                HARNESS.goal06_preflight(True)(
                    Path("C:/release/markturbo.exe"), HASH, evidence
                )

        self.assertEqual(raised.exception.code, "PERSISTENT_CREDENTIAL_PRESENT")
        self.assertNotIn(secret.decode("utf-8"), str(raised.exception))
        self.assertEqual(enumerated, [target])
        self.assertEqual(len(freed), 1)


class HarnessContractTests(unittest.TestCase):
    @staticmethod
    def consent_harness(events: list[tuple[str, object]], *, scope_shown: bool = True):
        button = object()

        class FakeHarness:
            ui_timeout = 1.0

            def find_control(self, app, automation_id, control_type, timeout_code, mismatch_code):
                events.append(("control", (automation_id, control_type, timeout_code)))
                return button

            def name_present(self, app, name, control_type):
                events.append(("scope", (name, control_type)))
                return scope_shown

            def hold_control(self, app, control, seconds, failure_code):
                events.append(("hold", (control, seconds, failure_code)))

            def click_control(self, *_args):
                events.append(("click", None))

            def invoke_control(self, *_args):
                events.append(("invoke", None))

        return FakeHarness(), button

    def test_configured_mode_approves_consent_only_by_a_full_pointer_hold(self) -> None:
        events: list[tuple[str, object]] = []
        harness, button = self.consent_harness(events)
        app = type("App", (), {"hwnd": 42})()

        HARNESS.Goal06Harness.approve_review_consent(harness, app, CASE_DOCUMENT)

        self.assertEqual(
            events[0],
            ("control", (HARNESS.CONSENT_SEND_AUTOMATION_ID, "Button", "REVIEW_CONSENT_UIA_TIMEOUT")),
        )
        self.assertEqual(
            events[-1],
            ("hold", (button, HARNESS.CONSENT_HOLD_SECONDS, "REVIEW_CONSENT_HOLD_FAILED")),
        )
        self.assertNotIn("click", [event[0] for event in events])
        self.assertNotIn("invoke", [event[0] for event in events])
        # The control's own hold threshold is 1.2s; the harness must exceed it.
        self.assertGreater(HARNESS.CONSENT_HOLD_SECONDS, 1.2)

    def test_selection_consent_requires_the_selection_scope_before_holding(self) -> None:
        events: list[tuple[str, object]] = []
        harness, _button = self.consent_harness(events, scope_shown=False)
        app = type("App", (), {"hwnd": 42})()

        with self.assertRaises(runtime.HarnessFailure) as raised:
            HARNESS.Goal06Harness.approve_review_consent(harness, app, CASE_SELECTION)

        self.assertEqual(raised.exception.code, "REVIEW_CONSENT_SELECTION_SCOPE_MISSING")
        self.assertIn(("scope", (HARNESS.CONSENT_SELECTION_SCOPE, "Text")), events)
        self.assertNotIn("hold", [event[0] for event in events])

    def test_review_requests_use_the_editor_shortcut_or_the_selection_menu_row(self) -> None:
        for case_id, expected in (
            (CASE_DOCUMENT, [("focus", 42), ("shortcut", runtime.VK_RETURN)]),
            (
                CASE_SELECTION,
                [
                    ("focus", 42),
                    ("shortcut", runtime.VK_A),
                    ("menu", HARNESS.REVIEW_SELECTION_MENU_AUTOMATION_ID),
                ],
            ),
        ):
            with self.subTest(case=case_id):
                events: list[tuple[str, object]] = []

                class FakeWin32:
                    def send_shortcut(self, hwnd, key):
                        events.append(("shortcut", key))

                class FakeHarness:
                    win32 = FakeWin32()

                    def focus_editor(self, app):
                        events.append(("focus", app.hwnd))

                    def menu_command(self, app, automation_id):
                        events.append(("menu", automation_id))

                HARNESS.Goal06Harness.request_review(
                    FakeHarness(), type("App", (), {"hwnd": 42})(), case_id
                )
                self.assertEqual(events, expected)

    def test_cli_arguments_require_a_hash_and_accept_each_case(self) -> None:
        for case_id in REQUIRED_CASE_IDS:
            parsed = PARSE_ARGS(["--expect-exe-sha256", HASH, "--case", case_id])
            self.assertEqual(parsed.case, case_id)
            self.assertFalse(parsed.configured_provider)

        configured = PARSE_ARGS(["--expect-exe-sha256", HASH, "--configured-provider"])
        self.assertTrue(configured.configured_provider)

    def test_close_app_posts_close_and_waits_for_clean_exit(self) -> None:
        for returncode in (0, 1):
            with self.subTest(returncode=returncode):
                events: list[tuple[str, object]] = []

                class FakeWin32:
                    def post_close(self, hwnd: int) -> None:
                        events.append(("close", hwnd))

                class FakeProcess:
                    def wait(self, timeout: float) -> int:
                        events.append(("wait", timeout))
                        return returncode

                harness = object.__new__(runtime.NativeHarness)
                harness.win32 = FakeWin32()
                harness.ui_timeout = 1.0
                app = type(
                    "App", (), {"hwnd": 42, "process": FakeProcess()}
                )()

                if returncode == 0:
                    harness.close_app(app)
                else:
                    with self.assertRaisesRegex(
                        runtime.HarnessFailure, "PROCESS_EXIT_NONZERO"
                    ):
                        harness.close_app(app)

                self.assertEqual(events, [("close", 42), ("wait", 1.0)])
