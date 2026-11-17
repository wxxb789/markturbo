"""Unit tests for the Goal 06 native harness without launching a UI."""

from __future__ import annotations

import ctypes
import hashlib
import tempfile
import tomllib
import unittest
from pathlib import Path
from typing import Any
from unittest import mock

from scripts.markturbo_tools.native import evidence as shared_evidence
from scripts.markturbo_tools.native import goal06 as HARNESS
from scripts.markturbo_tools.native import runtime

from ._native_support import HASH, case_observations, fingerprint, passed_evidence

CASE_NOT_SET_UP = HARNESS.CASE_NOT_SET_UP
CASE_DOCUMENT = HARNESS.CASE_DOCUMENT
CASE_SELECTION = HARNESS.CASE_SELECTION
NEW_EVIDENCE = HARNESS.new_evidence
PARSE_ARGS = HARNESS.parse_args
REVIEW_SETTINGS_DOCUMENT = HARNESS.review_settings_document
VALIDATE_EVIDENCE = HARNESS.validate_evidence
CONFIGURED_IDENTITY = shared_evidence.credential_target(
    HARNESS.CONFIGURED_PROVIDER_ENDPOINT, "openai-responses"
)
DOCUMENT_PRINT = fingerprint(HARNESS.DOCUMENT_TEXT.encode("utf-8"))


def mode_of(configured_provider: bool) -> str:
    return HARNESS.CONFIGURED_PROVIDER_MODE if configured_provider else HARNESS.NOT_SET_UP_MODE


def complete(evidence: dict, status: str) -> None:
    runtime.complete_evidence(evidence, status, HARNESS.CASE_IDS[evidence["mode"]])


def valid_evidence(configured_provider: bool = False) -> dict:
    flows = HARNESS.CASE_FLOWS[mode_of(configured_provider)]

    def observations(case_id: str) -> dict:
        if configured_provider:
            own = {
                "editor_after": DOCUMENT_PRINT,
                "source_after": DOCUMENT_PRINT,
                "consent_ms": 2100.5,
                "result_ms": 900.0,
                "clean_close_ms": 300.0,
            }
            if case_id == CASE_SELECTION:
                own["consent_selection_scope"] = True
        else:
            own = {
                "shortcut_diagnostic_before": False,
                "shortcut_diagnostic_ms": 120.0,
                "shortcut_editor_after": DOCUMENT_PRINT,
                "shortcut_source_after": DOCUMENT_PRINT,
                "close_diagnostic_gone_ms": 40.0,
                "menu_diagnostic_ms": 80,
                "menu_editor_after": DOCUMENT_PRINT,
                "menu_source_after": DOCUMENT_PRINT,
                "clean_close_ms": 250.0,
            }
        return case_observations(
            flows[case_id], editor_before=DOCUMENT_PRINT, source_before=DOCUMENT_PRINT, **own
        )

    return passed_evidence(NEW_EVIDENCE(HASH, configured_provider), observations, complete)


class EvidenceSchemaTests(unittest.TestCase):
    def test_default_mode_is_one_hash_bound_not_set_up_case(self) -> None:
        evidence = valid_evidence()

        VALIDATE_EVIDENCE(evidence)

        self.assertEqual(evidence["status"], "PASS")
        self.assertEqual(HARNESS.REQUIRED_CASE_IDS, (CASE_NOT_SET_UP,))
        self.assertEqual([case["id"] for case in evidence["cases"]], [CASE_NOT_SET_UP])

    def test_configured_mode_keeps_document_and_selection_cases(self) -> None:
        evidence = valid_evidence(configured_provider=True)

        VALIDATE_EVIDENCE(evidence)

        self.assertEqual(
            [case["id"] for case in evidence["cases"]], [CASE_DOCUMENT, CASE_SELECTION]
        )

    def test_selection_pass_requires_the_measured_selection_scope(self) -> None:
        for value in (False, None):
            with self.subTest(value=value):
                evidence = valid_evidence(configured_provider=True)
                evidence["cases"][1]["observations"]["consent_selection_scope"] = value
                with self.assertRaisesRegex(ValueError, "consent_selection_scope"):
                    VALIDATE_EVIDENCE(evidence)

        evidence = valid_evidence(configured_provider=True)
        del evidence["cases"][1]["observations"]["consent_selection_scope"]
        with self.assertRaisesRegex(ValueError, "incomplete"):
            VALIDATE_EVIDENCE(evidence)

    def test_mode_selects_its_own_cases_and_observations(self) -> None:
        evidence = valid_evidence(configured_provider=True)
        evidence["mode"] = HARNESS.NOT_SET_UP_MODE
        with self.assertRaises(ValueError):
            VALIDATE_EVIDENCE(evidence)

        evidence = valid_evidence()
        evidence["cases"][0]["observations"]["consent_ms"] = 1.0
        with self.assertRaisesRegex(ValueError, "unknown observation field"):
            VALIDATE_EVIDENCE(evidence)

        for mode in ("unknown", ["unhashable"]):
            evidence["mode"] = mode
            with self.assertRaisesRegex(ValueError, "invalid native Review mode"):
                VALIDATE_EVIDENCE(evidence)

    def test_each_phase_fails_on_its_own_change(self) -> None:
        for configured, key, message in (
            (False, "shortcut_editor_after", r"changed editor text \(shortcut_editor_after\)"),
            (False, "menu_source_after", r"changed source text \(menu_source_after\)"),
            (True, "editor_after", r"changed editor text \(editor_after\)"),
            (False, "source_before", "differ before Review"),
        ):
            with self.subTest(key=key):
                evidence = valid_evidence(configured)
                evidence["cases"][0]["observations"][key] = fingerprint(b"changed")
                with self.assertRaisesRegex(ValueError, message):
                    VALIDATE_EVIDENCE(evidence)

    def test_measured_booleans_and_durations_must_hold(self) -> None:
        for configured, key, value, message in (
            (False, "shortcut_diagnostic_before", True, "false shortcut_diagnostic_before"),
            (False, "shortcut_diagnostic_before", None, "false shortcut_diagnostic_before"),
            (True, "clean_close_ms", None, "measured clean_close_ms"),
            (False, "close_diagnostic_gone_ms", -1.0, "measured close_diagnostic_gone_ms"),
            (False, "menu_diagnostic_ms", True, "measured menu_diagnostic_ms"),
            (True, "result_ms", float("nan"), "measured result_ms"),
        ):
            with self.subTest(key=key, value=value):
                evidence = valid_evidence(configured)
                evidence["cases"][0]["observations"][key] = value
                with self.assertRaisesRegex(ValueError, message):
                    VALIDATE_EVIDENCE(evidence)

    def test_blocked_foreground_diagnostics_are_retained_as_content_free_evidence(self) -> None:
        evidence = NEW_EVIDENCE(HASH, configured_provider=True)
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


class FakeApp:
    """The few app facts the Review flow reads: the diagnostic card and the
    title menu, with switches that break one step at a time."""

    hwnd = 42

    def __init__(
        self,
        *,
        shown_at_start: bool = False,
        close_works: bool = True,
        close_exposed: bool = True,
        menu_works: bool = True,
        breaks: tuple[str, str] | None = None,
    ) -> None:
        self.card = shown_at_start
        self.close_works = close_works
        self.close_exposed = close_exposed
        self.menu_works = menu_works
        # (phase, what): after `phase`'s Review, change the "editor" or the
        # "source", or make the editor's re-read time out ("read").
        self.breaks = breaks
        self.phase = "start"
        self.events: list[tuple[str, object]] = []


class FakeWin32:
    def __init__(self, app: FakeApp, owns_keyboard: bool = True, focus_lag: int = 0) -> None:
        self.app = app
        self.owns_keyboard = owns_keyboard
        # Samples that miss the focus before activation hands it over.
        self.focus_lag = focus_lag

    def foreground_focus(self, hwnd: int) -> int:
        if self.focus_lag:
            self.focus_lag -= 1
            return 0
        return hwnd if self.owns_keyboard else 0

    def send_shortcut(self, hwnd: int, key: int) -> None:
        self.app.events.append(("shortcut", key))
        if key == runtime.VK_RETURN:
            self.app.card = True
            self.app.phase = "shortcut"


def fake_harness(app: FakeApp, source: Path, **win32: Any) -> HARNESS.Goal06Harness:
    """A real `Goal06Harness` whose UI Automation reads the fake app."""
    harness = object.__new__(HARNESS.Goal06Harness)
    harness.ui_timeout = 0.2
    harness.configured_provider = False
    harness.win32 = FakeWin32(app, **win32)
    document = runtime.fingerprint_text(HARNESS.DOCUMENT_TEXT)
    close_button = object()

    def find_control(_app, automation_id, control_type, timeout_code, _mismatch):
        app.events.append(("find", automation_id))
        if automation_id == HARNESS.REVIEW_CLOSE_AUTOMATION_ID and app.card and app.close_exposed:
            return close_button
        raise runtime.HarnessFailure(timeout_code)

    def invoke_control(control, _code):
        app.events.append(("invoke", "close" if control is close_button else control))
        if control is close_button and app.close_works:
            app.card = False

    def menu_command(_app, automation_id):
        app.events.append(("menu", automation_id))
        if automation_id == HARNESS.REVIEW_MENU_AUTOMATION_ID and app.menu_works:
            app.card = True
            app.phase = "menu"

    harness.activate_source_layout = lambda _app: app.events.append(("layout", None))
    harness.focus_editor = lambda _app: app.events.append(("focus", None))
    def editor_fingerprint(_app):
        if app.breaks == (app.phase, "read"):
            raise runtime.HarnessFailure("EDITOR_TEXT_TIMEOUT")
        if app.breaks == (app.phase, "editor"):
            return runtime.fingerprint_text(HARNESS.DOCUMENT_TEXT + "x")
        if app.breaks == (app.phase, "source"):
            source.write_text(HARNESS.DOCUMENT_TEXT + "x", encoding="utf-8", newline="")
        return document

    harness.editor_fingerprint = editor_fingerprint
    harness.name_present = lambda _app, name, _type: (
        name == HARNESS.NOT_SET_UP_MESSAGE and app.card
    )
    harness.control_absent = lambda _app, automation_id, *_codes: not (
        automation_id == HARNESS.REVIEW_CLOSE_AUTOMATION_ID and app.card
    )
    harness.find_control = find_control
    harness.invoke_control = invoke_control
    harness.menu_command = menu_command
    source.write_text(HARNESS.DOCUMENT_TEXT, encoding="utf-8", newline="")
    return harness


class NotSetUpSessionTests(unittest.TestCase):
    def run_session(self, app: FakeApp, **win32: Any) -> dict:
        with tempfile.TemporaryDirectory() as temporary:
            source = Path(temporary) / "review.md"
            harness = fake_harness(app, source, **win32)
            return harness.not_set_up_session(app, source)

    def assert_fails(self, code: str, app: FakeApp, **win32: Any) -> None:
        with self.assertRaises(runtime.HarnessFailure) as raised:
            self.run_session(app, **win32)
        self.assertEqual(raised.exception.code, code)

    def test_one_session_proves_the_shortcut_close_and_menu_phases(self) -> None:
        app = FakeApp()

        observations = self.run_session(app)

        self.assertEqual(
            [event for event in app.events if event[0] in {"shortcut", "invoke", "menu"}],
            [
                ("shortcut", runtime.VK_RETURN),
                ("invoke", "close"),
                ("menu", HARNESS.REVIEW_MENU_AUTOMATION_ID),
            ],
        )
        self.assertIs(observations["shortcut_diagnostic_before"], False)
        for phase in ("shortcut", "menu"):
            self.assertEqual(observations[f"{phase}_editor_after"], observations["editor_before"])
            self.assertEqual(observations[f"{phase}_source_after"], observations["source_before"])
        # With the clean close the scenario adds, the session's own keys are
        # exactly what a PASS case requires.
        contract = HARNESS.CONTRACTS[HARNESS.NOT_SET_UP_MODE]
        self.assertEqual(
            set(observations) | {"clean_close_ms"},
            set(contract.required_observations[CASE_NOT_SET_UP]),
        )

    def test_a_card_already_shown_cannot_stand_in_for_the_shortcut(self) -> None:
        self.assert_fails("SHORTCUT_DIAGNOSTIC_PRESENT_BEFORE_REVIEW", FakeApp(shown_at_start=True))

    def test_the_shortcut_needs_the_keyboard_on_the_app_window(self) -> None:
        self.assert_fails("SHORTCUT_NATIVE_FOCUS_LOST", FakeApp(), owns_keyboard=False)

    def test_a_card_that_does_not_close_cannot_stand_in_for_the_menu_row(self) -> None:
        app = FakeApp(close_works=False)

        self.assert_fails("CLOSE_DIAGNOSTIC_STILL_SHOWN", app)
        self.assertNotIn("menu", [event[0] for event in app.events])

    def test_a_menu_row_that_shows_nothing_fails_its_own_phase(self) -> None:
        self.assert_fails("MENU_DIAGNOSTIC_UIA_TIMEOUT", FakeApp(menu_works=False))

    def test_every_failure_names_the_phase_it_happened_in(self) -> None:
        # The same helper failure, or the same change, in two phases of the
        # one session must yield two different, locatable codes.
        for code, app in (
            ("SHORTCUT_REVIEW_CHANGED_EDITOR", FakeApp(breaks=("shortcut", "editor"))),
            ("SHORTCUT_REVIEW_CHANGED_SOURCE", FakeApp(breaks=("shortcut", "source"))),
            ("SHORTCUT_EDITOR_TEXT_TIMEOUT", FakeApp(breaks=("shortcut", "read"))),
            ("MENU_REVIEW_CHANGED_EDITOR", FakeApp(breaks=("menu", "editor"))),
            ("MENU_REVIEW_CHANGED_SOURCE", FakeApp(breaks=("menu", "source"))),
            ("MENU_EDITOR_TEXT_TIMEOUT", FakeApp(breaks=("menu", "read"))),
            ("CLOSE_REVIEW_UIA_TIMEOUT", FakeApp(close_exposed=False)),
        ):
            with self.subTest(code=code):
                self.assert_fails(code, app)

    def test_the_shortcut_waits_for_activation_to_hand_over_the_keyboard(self) -> None:
        observations = self.run_session(FakeApp(), focus_lag=2)

        self.assertIs(observations["shortcut_diagnostic_before"], False)


class CleanCloseTests(unittest.TestCase):
    """The document's clean state, measured by how a close request ends."""

    @staticmethod
    def harness(
        exits_with: int | None,
        guard: bool,
        *,
        exit_after_polls: int = 0,
        window_destroyed: bool = False,
        query_fails: bool = False,
    ) -> HARNESS.Goal06Harness:
        """`exit_after_polls` keeps the process alive for that many polls after
        the close; meanwhile its window may already be destroyed, or a UIA
        query against it may fail (nsh's DestroyWindow precedes the exit)."""

        class Process:
            def __init__(self) -> None:
                self.closed = False
                self.polls = 0

            def poll(self) -> int | None:
                if not self.closed:
                    return None
                self.polls += 1
                return exits_with if self.polls > exit_after_polls else None

            def wait(self, timeout: float) -> int:
                return exits_with

        process = Process()

        class User32:
            @staticmethod
            def IsWindow(hwnd: int) -> bool:
                return not (window_destroyed and process.closed)

        class Win32:
            user32 = User32()

            def post_close(self, hwnd: int) -> None:
                process.closed = True

        def name_present(_app, name, _type):
            if query_fails and process.closed:
                raise runtime.HarnessFailure("UIA_NAME_QUERY_FAILED")
            return guard and process.closed and name == runtime.GUARD_HEADING

        harness = object.__new__(HARNESS.Goal06Harness)
        harness.ui_timeout = 0.2
        harness.win32 = Win32()
        harness.name_present = name_present
        harness.app = type("App", (), {"hwnd": 42, "process": process})()
        return harness

    def test_a_clean_document_closes_without_the_guard(self) -> None:
        for window_destroyed, query_fails in ((False, False), (True, False), (False, True)):
            with self.subTest(window_destroyed=window_destroyed, query_fails=query_fails):
                harness = self.harness(
                    exits_with=0,
                    guard=False,
                    exit_after_polls=3,
                    window_destroyed=window_destroyed,
                    query_fails=query_fails,
                )

                self.assertTrue(runtime.finite_nonnegative(harness.close_clean(harness.app)))

    def test_a_dirty_document_fails_on_the_guard_it_raises(self) -> None:
        for exits_with, guard, code in (
            (None, True, "REVIEW_LEFT_DOCUMENT_DIRTY"),
            (None, False, "PROCESS_EXIT_TIMEOUT"),
            (1, False, "PROCESS_EXIT_NONZERO"),
        ):
            with self.subTest(code=code):
                harness = self.harness(exits_with=exits_with, guard=guard)
                with self.assertRaises(runtime.HarnessFailure) as raised:
                    harness.close_clean(harness.app)
                self.assertEqual(raised.exception.code, code)

    def test_queries_that_keep_failing_never_pass_as_a_clean_close(self) -> None:
        harness = self.harness(exits_with=None, guard=False, query_fails=True)

        with self.assertRaises(runtime.HarnessFailure) as raised:
            harness.close_clean(harness.app)

        self.assertEqual(raised.exception.code, "PROCESS_EXIT_TIMEOUT")


class ConfiguredReviewTests(unittest.TestCase):
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

    def test_consent_is_approved_only_by_a_full_pointer_hold(self) -> None:
        events: list[tuple[str, object]] = []
        harness, button = self.consent_harness(events)
        app = type("App", (), {"hwnd": 42})()

        scope = HARNESS.Goal06Harness.approve_review_consent(harness, app, CASE_DOCUMENT)

        self.assertIsNone(scope)
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

    def test_selection_consent_measures_its_scope_before_holding(self) -> None:
        events: list[tuple[str, object]] = []
        harness, _button = self.consent_harness(events)
        app = type("App", (), {"hwnd": 42})()

        self.assertIs(HARNESS.Goal06Harness.approve_review_consent(harness, app, CASE_SELECTION), True)

        events.clear()
        harness, _button = self.consent_harness(events, scope_shown=False)
        with self.assertRaises(runtime.HarnessFailure) as raised:
            HARNESS.Goal06Harness.approve_review_consent(harness, app, CASE_SELECTION)

        self.assertEqual(raised.exception.code, "REVIEW_CONSENT_SELECTION_SCOPE_MISSING")
        self.assertIn(("scope", (HARNESS.CONSENT_SELECTION_SCOPE, "Text")), events)
        self.assertNotIn("hold", [event[0] for event in events])

    def test_review_requests_use_the_editor_shortcut_or_the_menu_row(self) -> None:
        for case_id, expected in (
            (CASE_DOCUMENT, [("focus", None), ("shortcut", runtime.VK_RETURN)]),
            (
                CASE_SELECTION,
                [
                    ("focus", None),
                    ("shortcut", runtime.VK_A),
                    ("menu", HARNESS.REVIEW_MENU_AUTOMATION_ID),
                ],
            ),
        ):
            with self.subTest(case=case_id), tempfile.TemporaryDirectory() as temporary:
                app = FakeApp()
                harness = fake_harness(app, Path(temporary) / "review.md")

                harness.request_review(app, case_id)
                self.assertEqual(app.events, expected)

    def test_each_review_request_needs_the_keyboard_on_the_app_window(self) -> None:
        for case_id, code in (
            (CASE_DOCUMENT, "REVIEW_SHORTCUT_NATIVE_FOCUS_LOST"),
            (CASE_SELECTION, "REVIEW_SELECT_ALL_NATIVE_FOCUS_LOST"),
        ):
            with self.subTest(case=case_id), tempfile.TemporaryDirectory() as temporary:
                app = FakeApp()
                harness = fake_harness(app, Path(temporary) / "review.md", owns_keyboard=False)

                with self.assertRaises(runtime.HarnessFailure) as raised:
                    harness.request_review(app, case_id)

                self.assertEqual(raised.exception.code, code)
                self.assertNotIn("shortcut", [event[0] for event in app.events])

    def test_a_review_that_changes_the_editor_or_file_fails_its_case(self) -> None:
        for what, code in (("editor", "REVIEW_CHANGED_EDITOR"), ("source", "REVIEW_CHANGED_SOURCE")):
            with self.subTest(what=what), tempfile.TemporaryDirectory() as temporary:
                source = Path(temporary) / "review.md"
                app = FakeApp(breaks=("result", what))
                harness = fake_harness(app, source)
                before = {"editor_before": DOCUMENT_PRINT, "source_before": DOCUMENT_PRINT}
                app.phase = "result"

                with self.assertRaises(runtime.HarnessFailure) as raised:
                    harness.read_only_phase(app, source, before, None)

                self.assertEqual(raised.exception.code, code)


class PlanAndArgumentTests(unittest.TestCase):
    def test_each_mode_runs_one_scenario_per_case(self) -> None:
        for configured in (False, True):
            with self.subTest(configured=configured):
                plan = HARNESS.native_run_plan(configured)
                harness = mock.Mock()
                harness.scenario.side_effect = lambda case_id: case_id

                ran = [scenario() for scenario in plan.scenarios(harness)]

                self.assertEqual(tuple(ran), HARNESS.CASE_IDS[mode_of(configured)])
                self.assertEqual(plan.required_case_ids, HARNESS.CASE_IDS[mode_of(configured)])

    def test_cli_accepts_only_the_cases_of_the_selected_mode(self) -> None:
        parsed = PARSE_ARGS(["--expect-exe-sha256", HASH, "--case", CASE_NOT_SET_UP])
        self.assertEqual(parsed.case, CASE_NOT_SET_UP)
        self.assertFalse(parsed.configured_provider)
        for case_id in (CASE_DOCUMENT, CASE_SELECTION):
            configured = PARSE_ARGS(
                ["--expect-exe-sha256", HASH, "--configured-provider", "--case", case_id]
            )
            self.assertTrue(configured.configured_provider)

        for argv in (
            ["--case", CASE_DOCUMENT],
            ["--configured-provider", "--case", CASE_NOT_SET_UP],
        ):
            with self.subTest(argv=argv), mock.patch("sys.stderr"):
                with self.assertRaises(SystemExit):
                    PARSE_ARGS(["--expect-exe-sha256", HASH, *argv])

    def test_close_app_posts_close_and_waits_for_clean_exit(self) -> None:
        for returncode in (0, 1):
            with self.subTest(returncode=returncode):
                events: list[tuple[str, object]] = []

                class PostCloseWin32:
                    def post_close(self, hwnd: int) -> None:
                        events.append(("close", hwnd))

                class FakeProcess:
                    def wait(self, timeout: float) -> int:
                        events.append(("wait", timeout))
                        return returncode

                harness = object.__new__(runtime.NativeHarness)
                harness.win32 = PostCloseWin32()
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


if __name__ == "__main__":
    unittest.main()
