"""Unit tests for the Goal 06 native harness without launching a UI."""

from __future__ import annotations

import ctypes
import copy
import hashlib
import tempfile
import tomllib
import unittest
from pathlib import Path
from unittest import mock

from scripts.markturbo_tools.native import goal06 as HARNESS
from scripts.markturbo_tools.native import runtime
from scripts.markturbo_tools.native import source_contract


CASE_DOCUMENT = HARNESS.CASE_DOCUMENT
CASE_SELECTION = HARNESS.CASE_SELECTION
COMPLETE_EVIDENCE = HARNESS.complete_evidence
NEW_EVIDENCE = HARNESS.new_evidence
PARSE_ARGS = HARNESS.parse_args
REQUIRED_CASE_IDS = HARNESS.REQUIRED_CASE_IDS
REVIEW_SETTINGS_DOCUMENT = HARNESS.review_settings_document
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

    def test_artifact_read_errors_keep_the_goal_runtime_scan_code(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            case_root = Path(temporary)
            logs = case_root / "data" / "logs"
            logs.mkdir(parents=True)
            (case_root / "config").mkdir()
            (logs / "markturbo.log").write_bytes(b"startup")
            with (
                mock.patch.object(
                    HARNESS,
                    "artifact_contains",
                    side_effect=PermissionError("document content"),
                ),
                self.assertRaises(runtime.HarnessFailure) as raised,
            ):
                HARNESS.scan_case_artifacts(case_root, "process-only")

        self.assertEqual(raised.exception.code, "RUNTIME_ARTIFACT_SCAN_FAILED")
        self.assertEqual(raised.exception.detail, "PermissionError")


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

    def test_persistent_credential_block_does_not_expose_credential_content(self) -> None:
        target = HARNESS.configured_provider_environment_key_identity()
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

    def test_production_source_removes_test_imports_without_losing_later_code(self) -> None:
        source_text = (
            "pub fn before_test_import() {}\n"
            "#[cfg(test)]\nuse crate::test_support::TestDependency;\n"
            "pub fn after_test_import() {}\n"
            "\n#[cfg(test)]\nmod tests {\n    fn test_only() {}\n}\n"
        )
        with tempfile.TemporaryDirectory() as directory:
            source = Path(directory) / "workspace.rs"
            source.write_text(source_text, encoding="utf-8")

            production = source_contract.production_source(source)

        self.assertIsNotNone(production)
        self.assertNotIn("use crate::test_support::TestDependency;", production)
        self.assertIn("before_test_import", production)
        self.assertIn("after_test_import", production)
        self.assertNotIn("mod tests", production)


class SourceContractTests(unittest.TestCase):
    """The review shortcut, diagnostic, result and consent selectors must live
    in production nsh source; tests and comments cannot supply them."""

    REPO = Path(__file__).resolve().parents[2]
    TEST_SCOPES = (
        "\n#[cfg(test)]\nmod tests {\n    // test-only contract decoy\n}\n",
        "\n#[cfg(test)] mod checks {\nfn decoy() {\n// test-only contract decoy\n}\n}\n",
        "\n# [ cfg (\n test \n) ]\npub(crate) mod checks\n{\n"
        "fn decoy() {\n// test-only contract decoy\n}\n}\n",
        "\n#[cfg(test)] fn decoy() {\n// test-only contract decoy\n}\n",
        "\n#[cfg(all(test, windows))] fn decoy() {\n// test-only contract decoy\n}\n",
        "\nmod helpers {\n#[cfg(test)] mod checks {\n"
        "fn decoy() {\n// test-only contract decoy\n}\n}\n}\n",
    )

    def failure_after(self, relative: str, edit) -> str | None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            for name in HARNESS.UI_SOURCE_REQUIREMENTS:
                target = root / name
                target.parent.mkdir(parents=True, exist_ok=True)
                target.write_bytes((self.REPO / name).read_bytes())
            target = root / relative
            target.write_text(edit(target.read_text(encoding="utf-8")), encoding="utf-8")
            with mock.patch.object(HARNESS, "REPO", root):
                return HARNESS.source_contract_failure()

    def test_real_repository_satisfies_source_contract(self) -> None:
        self.assertIsNone(HARNESS.source_contract_failure())

    def test_unsafe_projection_returns_source_contract_failure(self) -> None:
        for relative in HARNESS.UI_SOURCE_REQUIREMENTS:
            with self.subTest(file=relative):
                self.assertEqual(
                    self.failure_after(relative, lambda text: text + "\n#[cfg(test)] mod checks {\n"),
                    "REVIEW_SOURCE_CONTRACT_MISSING",
                )

    def test_each_missing_production_marker_returns_source_contract_failure(self) -> None:
        for relative, markers in HARNESS.UI_SOURCE_REQUIREMENTS.items():
            for marker in markers:
                with self.subTest(file=relative, marker=marker):
                    self.assertEqual(
                        self.failure_after(relative, lambda text: text.replace(marker, "")),
                        "REVIEW_SOURCE_CONTRACT_MISSING",
                    )

    def test_cfg_test_decoys_do_not_supply_missing_production_contracts(self) -> None:
        # Each file's parse is exercised once per test scope; the marker
        # check itself is shared by every marker.
        for relative, markers in HARNESS.UI_SOURCE_REQUIREMENTS.items():
            for marker in markers[:1]:
                for scope in self.TEST_SCOPES:
                    decoy = scope.replace(
                        "// test-only contract decoy", f"let _ = {marker};"
                    )
                    with self.subTest(file=relative, marker=marker, scope=scope):
                        self.assertEqual(
                            self.failure_after(
                                relative,
                                lambda text: decoy + text.replace(marker, ""),
                            ),
                            "REVIEW_SOURCE_CONTRACT_MISSING",
                        )

    def test_comment_decoys_do_not_preserve_a_removed_selector(self) -> None:
        relative = "crates/mt-app/src/ui/review.rs"
        marker = '"No model set up"'
        for decoy in (f"// {marker}\n", f"/* {marker} */\n", f"/// {marker}\nfn f() {{}}\n"):
            with self.subTest(decoy=decoy):
                self.assertEqual(
                    self.failure_after(relative, lambda text: text.replace(marker, '""') + decoy),
                    "REVIEW_SOURCE_CONTRACT_MISSING",
                )
