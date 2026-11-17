"""Unit tests for the Goal 03 native harness without launching a UI."""

from __future__ import annotations

import hashlib
import json
import shutil
import tempfile
import tomllib
import unittest
from pathlib import Path
from unittest import mock

from scripts.markturbo_tools.native import goal03 as HARNESS
from scripts.markturbo_tools.native import runtime

from ._native_support import (
    HASH,
    case_observations,
    fingerprint,
    passed_evidence,
    process_context,
)

BUILD_LAUNCH_SPEC = runtime.build_launch_spec
CASE_FIRST_USE = HARNESS.CASE_FIRST_USE
DOCUMENT_SENTINEL = HARNESS.DOCUMENT_SENTINEL
HARNESS_FAILURE = runtime.HarnessFailure
HAS_UNICODE_CLIPBOARD_TEXT = runtime.has_unicode_clipboard_text
REQUIRED_CASE_IDS = HARNESS.REQUIRED_CASE_IDS
VALIDATE_EVIDENCE = HARNESS.validate_evidence
GOAL_03_HARNESS = HARNESS.Goal03Harness


def complete(evidence: dict, status: str) -> None:
    runtime.complete_evidence(evidence, status, REQUIRED_CASE_IDS)


def sample_observation() -> dict[str, int | str | dict[str, int | str]]:
    content = b"README.md\0sample\n\0"
    content_hash = hashlib.sha256(content).hexdigest()
    return {
        "sample_file_count": 1,
        "sample_home_rows": 1,
        "sample_manifest": fingerprint(b"README.md\0"),
        "sample_content": {"byte_count": len(b"sample\n"), "sha256": content_hash},
        "sample_version": content_hash[:24],
    }


def observations(case_id: str) -> dict:
    original = fingerprint(b"original")
    new = fingerprint(b"new")
    resaved = fingerprint(b"resaved")
    pasted = fingerprint(HARNESS.PASTE_TEXT.encode("utf-8"))
    return case_observations(
        HARNESS.CASE_FLOWS[case_id],
        welcome_recent_rows=0,
        new_editor=new,
        save_as_cancel_editor=new,
        save_as_cancel_destination_before=original,
        save_as_cancel_destination_after=original,
        overwrite_no_editor=new,
        overwrite_no_destination_before=original,
        overwrite_no_destination_after=original,
        overwrite_yes_destination=new,
        save_as_create_destination=new,
        subsequent_save_destination=resaved,
        subsequent_save_previous_destination=new,
        saved_close_recent_rows=2,
        paste_clipboard=pasted,
        paste_editor=pasted,
        paste_discard_recent_rows=2,
        reopen_editor=resaved,
        reopen_recent_count=10,
        recents_rows=5,
        recents_disabled_rows=1,
        recents_reopen_editor=resaved,
        recents_count_after_restart=10,
        process_contexts=[process_context()] * HARNESS.LAUNCH_COUNT,
        **sample_observation(),
    )


def valid_evidence() -> dict:
    return passed_evidence(HARNESS.new_evidence(HASH), observations, complete)


class EvidenceSchemaTests(unittest.TestCase):
    def test_accepts_complete_hash_bound_evidence(self) -> None:
        evidence = valid_evidence()

        VALIDATE_EVIDENCE(evidence)

        self.assertEqual(evidence["status"], "PASS")
        self.assertEqual([case["id"] for case in evidence["cases"]], list(REQUIRED_CASE_IDS))

    def test_accepts_a_failed_case_with_its_phase_code(self) -> None:
        evidence = valid_evidence()
        evidence["cases"][0].update(
            status="FAIL",
            reason_code="OVERWRITE_NO_NATIVE_OVERWRITE_DIALOG_TIMEOUT",
            failure_type="HarnessFailure",
            observations={},
        )
        complete(evidence, "FAIL")

        VALIDATE_EVIDENCE(evidence)

    def test_pass_requires_every_phase_fact(self) -> None:
        changed = fingerprint(b"changed")
        for key, value, message in (
            ("welcome_recent_rows", 1, "clean profile"),
            ("save_as_cancel_editor", changed, "unsaved editor"),
            ("overwrite_no_editor", changed, "unsaved editor"),
            ("save_as_cancel_destination_after", changed, "Save As picker cancellation"),
            ("overwrite_no_destination_after", changed, "overwrite cancellation"),
            ("overwrite_yes_destination", changed, "confirmed overwrite"),
            ("save_as_create_destination", changed, "menu Save As"),
            ("subsequent_save_previous_destination", changed, "only the new path"),
            ("saved_close_recent_rows", 3, "both recorded"),
            ("paste_discard_recent_rows", 3, "pasted buffer"),
            ("paste_editor", changed, "exact clipboard text"),
            ("paste_clipboard", changed, "CJK and emoji paste text"),
            ("sample_home_rows", 0, "sample file inventory"),
            ("sample_version", "b" * 24, "sample version does not match"),
            ("reopen_editor", changed, "direct reopen"),
            ("recents_reopen_editor", changed, "recent list"),
            ("reopen_recent_count", 11, "exactly ten"),
            ("recents_count_after_restart", 9, "exactly ten"),
            ("recents_rows", 10, "exactly five"),
            ("recents_disabled_rows", 0, "stale recent row"),
            ("recents_rows", True, "invalid recents_rows"),
            ("process_contexts", [process_context()], "one process context per launch"),
        ):
            with self.subTest(key=key, value=value):
                evidence = valid_evidence()
                evidence["cases"][0]["observations"][key] = value
                with self.assertRaisesRegex(ValueError, message):
                    VALIDATE_EVIDENCE(evidence)

    def test_pass_requires_every_phase_observation(self) -> None:
        for key in sorted(HARNESS.REQUIRED_OBSERVATIONS[CASE_FIRST_USE]):
            with self.subTest(key=key):
                evidence = valid_evidence()
                evidence["cases"][0]["observations"].pop(key)
                with self.assertRaisesRegex(ValueError, "passed case evidence is incomplete"):
                    VALIDATE_EVIDENCE(evidence)


class ParserAndIsolationTests(unittest.TestCase):
    def test_recent_settings_seed_is_valid_input_without_document_content(self) -> None:
        documents = [Path(f"C:/work/recent-{index:02}.md") for index in range(11)]
        text = HARNESS.recent_settings_document(documents).decode("utf-8")
        parsed = tomllib.loads(text)

        self.assertTrue(parsed["show-welcome-on-startup"])
        self.assertEqual(len(parsed["recent-targets"]), 11)
        self.assertEqual(parsed["recent-targets"][1]["display-name"], "recent-01.md")
        self.assertNotIn(DOCUMENT_SENTINEL, text)

    def test_seeded_history_holds_eleven_targets_with_one_missing(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            config = Path(temporary) / "config"
            workspace = Path(temporary) / "workspace"
            documents = HARNESS.seed_recent_history(config, workspace)

            self.assertEqual(HARNESS.persisted_recent_count(config), 11)
            self.assertEqual(
                [path.exists() for path in documents],
                [index != HARNESS.STALE_SEED_INDEX for index in range(11)],
            )
            self.assertTrue(all(workspace.resolve() in path.parents for path in documents))

    def test_unicode_text_clipboard_allows_additional_application_formats(self) -> None:
        self.assertTrue(HAS_UNICODE_CLIPBOARD_TEXT({13, 49161, 49282}))
        self.assertFalse(HAS_UNICODE_CLIPBOARD_TEXT({49161, 49282}))

    def test_materialized_sample_inventory_requires_a_nonempty_self_consistent_tree(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            template = root / "template"
            template.mkdir()
            (template / "README.md").write_bytes(b"sample\n")
            expected_content_hash = hashlib.sha256(b"README.md\0sample\n\0").hexdigest()
            data = root / "data"
            materialized = data / "sample" / expected_content_hash[:24]
            materialized.parent.mkdir(parents=True)
            shutil.copytree(template, materialized)

            evidence = HARNESS.materialized_sample_inventory(data)

            self.assertEqual(
                set(evidence),
                {"sample_file_count", "sample_manifest", "sample_content", "sample_version"},
            )
            self.assertNotIn("README.md", json.dumps(evidence))
            self.assertEqual(evidence["sample_content"]["sha256"], expected_content_hash)
            self.assertEqual(evidence["sample_version"], expected_content_hash[:24])
            (data / "sample" / ("a" * 24)).mkdir()
            with self.assertRaises(HARNESS_FAILURE) as raised:
                HARNESS.materialized_sample_inventory(data)
            self.assertEqual(raised.exception.code, "SAMPLE_MATERIALIZATION_INCOMPLETE")
            shutil.rmtree(data / "sample" / ("a" * 24))
            (materialized / "README.md").unlink()
            with self.assertRaises(HARNESS_FAILURE) as raised:
                HARNESS.materialized_sample_inventory(data)
            self.assertEqual(raised.exception.code, "SAMPLE_MATERIALIZATION_INCOMPLETE")

    def test_welcome_actions_are_found_by_their_own_automation_ids(self) -> None:
        # The nsh welcome's actions are Buttons that carry their test ids, so
        # an anonymous by-name lookup would never see them.
        harness = mock.Mock()
        app = object()
        GOAL_03_HARNESS.welcome_entry_points(harness, app)

        self.assertEqual(
            [call.args[1] for call in harness.welcome_action.call_args_list],
            ["welcome-new-file", "welcome-paste", "welcome-open-sample"],
        )
        GOAL_03_HARNESS.welcome_action(harness, app, HARNESS.WELCOME_SAMPLE_AUTOMATION_ID)
        harness.find_control.assert_called_once_with(
            app,
            "welcome-open-sample",
            "Button",
            "WELCOME_UIA_TIMEOUT",
            "WELCOME_UIA_CONTRACT_MISMATCH",
        )
        harness.find_named.assert_not_called()

    def test_save_as_shortcut_requires_editor_focus_and_sends_six_key_events(self) -> None:
        events = []

        class FakeWin32:
            def require_foreground(self, hwnd, timeout):
                events.append(("foreground", hwnd, timeout))

            def send_inputs(self, inputs):
                events.append(("inputs", len(inputs)))

        class FakeHarness:
            ui_timeout = 3.0
            win32 = FakeWin32()

            def require_source_editor_focus(self, app, failure_code):
                events.append(("focus", app.hwnd, failure_code))

        app = type("App", (), {"hwnd": 42})()
        GOAL_03_HARNESS.request_save_as_shortcut(FakeHarness(), app)

        self.assertEqual(events[0], ("focus", 42, "SAVE_AS_SHORTCUT_EDITOR_NOT_FOCUSED"))
        self.assertEqual(events[1], ("foreground", 42, 3.0))
        self.assertEqual(events[2], ("inputs", 6))

    def test_native_run_plan_wires_the_one_case_to_its_scenario(self) -> None:
        harness = object.__new__(GOAL_03_HARNESS)
        plan = HARNESS.native_run_plan()

        scenarios = plan.scenarios(harness)

        self.assertEqual(plan.required_case_ids, (CASE_FIRST_USE,))
        self.assertEqual(
            tuple(scenario.__func__ for scenario in scenarios),
            (GOAL_03_HARNESS.scenario_first_use,),
        )
        self.assertIs(scenarios[0].__self__, harness)

    def test_constructs_isolated_no_argument_and_explicit_launches(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary).resolve()
            exe = root / "bin" / "markturbo.exe"
            data = root / "data"
            config = root / "config"
            workspace = root / "workspace"
            stderr = root / "stderr.log"
            env = {"PATH": "path", "OPENAI_API_KEY": "secret", "ANTHROPIC_API_KEY": "secret"}
            no_argument = BUILD_LAUNCH_SPEC(exe, None, data, config, workspace, stderr, env)
            explicit = BUILD_LAUNCH_SPEC(exe, workspace / "document.md", data, config, workspace, stderr, env)

        self.assertEqual(no_argument.args, (str(exe),))
        self.assertEqual(explicit.args, (str(exe), str(workspace / "document.md")))
        self.assertEqual(no_argument.env["MARKTURBO_DATA_DIR"], str(data))
        self.assertEqual(no_argument.env["MARKTURBO_CONFIG_DIR"], str(config))
        self.assertNotIn("OPENAI_API_KEY", no_argument.env)
        self.assertNotIn("ANTHROPIC_API_KEY", no_argument.env)

    def test_rejects_relative_isolation_paths(self) -> None:
        with self.assertRaisesRegex(ValueError, "launch paths must be absolute"):
            BUILD_LAUNCH_SPEC(Path("markturbo.exe"), None, Path("data"), Path("config"), Path("workspace"), Path("stderr.log"))


def fake_session_harness() -> mock.Mock:
    """A harness whose UI primitives are mocks over a tiny editor/clipboard
    model; the phase methods are real. Saves write real files."""
    harness = mock.Mock()
    for name in ("first_session", "cold_reopen", "restart_recents"):
        setattr(harness, name, getattr(GOAL_03_HARNESS, name).__get__(harness))
    state = {"editor": HARNESS.RESAVE_TEXT, "clipboard": None}
    texts = {}
    harness.state = state

    def paste_editor(_app, value):
        state["editor"] = value
        texts[HARNESS.fingerprint_text(value)] = value
        return HARNESS.fingerprint_text(value)

    def wait_file(path, expected, _code):
        runtime.write_durable(path, texts[expected].encode("utf-8"))
        return runtime.sha256_file(path)

    def welcome_action(_app, automation_id):
        return automation_id

    def click_control(control, _code):
        if control == HARNESS.WELCOME_PASTE_AUTOMATION_ID:
            state["editor"] = state["clipboard"]

    harness.paste_editor.side_effect = paste_editor
    harness.wait_file.side_effect = wait_file
    harness.welcome_action.side_effect = welcome_action
    harness.click_control.side_effect = click_control
    harness.editor_fingerprint.side_effect = lambda _app: HARNESS.fingerprint_text(
        state["editor"]
    )
    harness.write_text_clipboard.side_effect = lambda value: state.update(clipboard=value)
    harness.read_text_clipboard.side_effect = lambda: state["clipboard"]
    created_row = mock.Mock()
    created_row.element_info.name = "File created.md"
    harness.wait_recent_controls.return_value = [created_row, mock.Mock()]
    harness.recent_controls.return_value = []
    harness.wait_workspace_home.return_value = [mock.Mock()]
    harness.welcome_absent.return_value = True
    harness.editor_absent_while_running.return_value = True
    return harness


class FirstUseScenarioTests(unittest.TestCase):
    def test_three_launches_cover_every_required_observation(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary).resolve()
            harness = fake_session_harness()
            harness.case_roots.return_value = tuple(
                root / "case" / name for name in ("data", "config", "workspace", "stderr.log")
            )
            for path in harness.case_roots.return_value[:3]:
                path.mkdir(parents=True)
            config = root / "case" / "config"
            launches = []

            def launch(target, *_roots):
                settings = config / "settings.toml"
                seeded = (
                    len(tomllib.loads(settings.read_text(encoding="utf-8"))["recent-targets"])
                    if settings.exists()
                    else None
                )
                launches.append((target, seeded))
                return mock.Mock()

            def launch(target, *_roots, _launch=launch):
                if target is not None:
                    # The argv reopen shows the saved file.
                    harness.state["editor"] = HARNESS.RESAVE_TEXT
                return _launch(target, *_roots)

            harness.launch_app.side_effect = launch
            stale_row = mock.Mock()
            stale_row.element_info.name = "File recent-01.md — Missing"
            stale_row.is_enabled.return_value = False
            created_row = mock.Mock()
            created_row.element_info.name = "File created.md"
            harness.wait_recent_controls.side_effect = [
                [created_row, mock.Mock()],
                [created_row, mock.Mock()],
                [created_row, *(RestartRecentsTests.row(name) for name in (
                    "Workspace workspace", "File recent-00.md")), stale_row,
                 RestartRecentsTests.row("File recent-02.md")],
            ]
            with (
                mock.patch.object(HARNESS, "STALE_RECENT_OBSERVE_SECONDS", 0),
                mock.patch.object(HARNESS, "persisted_recent_count", return_value=10),
                mock.patch.object(
                    HARNESS, "materialized_sample_inventory", return_value=sample_observation()
                ),
                mock.patch.object(HARNESS, "scan_case_artifacts", return_value={}),
            ):
                observations = GOAL_03_HARNESS.scenario_first_use(harness)

        created = (root / "case" / "workspace" / "created.md").resolve()
        # A clean profile first; the seeded eleven before the argv reopen, and
        # nothing rewritten by the harness before the restart.
        self.assertEqual(launches, [(None, None), (created, 11), (None, 11)])
        self.assertEqual(harness.reap.call_count, HARNESS.LAUNCH_COUNT)
        required = HARNESS.REQUIRED_OBSERVATIONS[CASE_FIRST_USE] | {"flow", "runtime_scan"}
        self.assertEqual(required - set(observations), set())

    def test_the_first_session_passes_on_the_fake_product(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            workspace = Path(temporary)
            harness = fake_session_harness()
            with mock.patch.object(
                HARNESS, "materialized_sample_inventory", return_value=sample_observation()
            ):
                observed = harness.first_session(mock.Mock(), workspace, workspace)
        self.assertEqual(observed["paste_editor"], observed["paste_clipboard"])
        self.assertEqual(observed["saved_close_recent_rows"], 2)

    def test_each_behaviour_failure_names_its_phase(self) -> None:
        """A regression fails inside its phase with its own code, before any
        evidence validation runs."""

        def tamper(name: str, data: bytes):
            def write(*_args, **_kwargs):
                runtime.write_durable(workspace / name, data)

            return write

        def fail_focus_on_call(number: int):
            calls = []

            def focus(_app, code):
                if code == "FOCUS_NOT_RESTORED":
                    calls.append(code)
                    if len(calls) == number:
                        raise HARNESS_FAILURE(code)

            return focus

        def other_row_first(harness):
            row = mock.Mock()
            row.element_info.name = "File other.md"
            harness.wait_recent_controls.return_value = [row, mock.Mock()]

        def three_rows(harness):
            harness.wait_recent_controls.return_value = (
                harness.wait_recent_controls.return_value + [mock.Mock()]
            )

        def discard_drops_a_row(harness):
            rows = harness.wait_recent_controls.return_value
            harness.wait_recent_controls.side_effect = [rows, rows[:1]]

        def paste_other_text(harness):
            harness.click_control.side_effect = lambda *_: harness.state.update(editor="other")

        cases = (
            ("WELCOME_PROFILE_NOT_CLEAN",
             lambda h: setattr(h.recent_controls, "return_value", [mock.Mock()])),
            ("SAVE_AS_CANCEL_CHANGED_STATE",
             lambda h: setattr(h.cancel_save_picker, "side_effect", tamper("cancelled.md", b"x"))),
            ("SAVE_AS_CANCEL_FOCUS_NOT_RESTORED",
             lambda h: setattr(h.require_source_editor_focus, "side_effect", fail_focus_on_call(1))),
            ("OVERWRITE_NO_CHANGED_STATE",
             lambda h: setattr(h.select_existing_save_path, "side_effect", tamper("replace.md", b"x"))),
            ("OVERWRITE_NO_FOCUS_NOT_RESTORED",
             lambda h: setattr(h.require_source_editor_focus, "side_effect", fail_focus_on_call(2))),
            ("OVERWRITE_NO_NATIVE_OVERWRITE_DIALOG_TIMEOUT",
             lambda h: setattr(h.select_existing_save_path, "side_effect",
                               HARNESS_FAILURE("NATIVE_OVERWRITE_DIALOG_TIMEOUT"))),
            ("SUBSEQUENT_SAVE_PREVIOUS_PATH_CHANGED",
             lambda h: setattr(h.save_via_menu, "side_effect", tamper("replace.md", b"x"))),
            ("SUBSEQUENT_SAVE_CANCELLED_PATH_CHANGED",
             lambda h: setattr(h.save_via_menu, "side_effect", tamper("cancelled.md", b"x"))),
            ("SAVED_CLOSE_RECENT_NOT_NEW_PATH", other_row_first),
            ("SAVED_CLOSE_RECENT_COUNT", three_rows),
            ("PASTE_EXACT_TEXT_MISMATCH", paste_other_text),
            ("PASTE_DISCARD_RECENT_CHANGED", discard_drops_a_row),
        )
        for code, arrange in cases:
            with self.subTest(code=code), tempfile.TemporaryDirectory() as temporary:
                workspace = Path(temporary)
                harness = fake_session_harness()
                arrange(harness)
                with (
                    mock.patch.object(
                        HARNESS, "materialized_sample_inventory", return_value=sample_observation()
                    ),
                    self.assertRaises(HARNESS_FAILURE) as raised,
                ):
                    harness.first_session(mock.Mock(), workspace, workspace)
                self.assertEqual(raised.exception.code, code)

    def test_a_clipboard_that_does_not_hold_the_paste_text_blocks(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            workspace = Path(temporary)
            harness = fake_session_harness()
            harness.read_text_clipboard.side_effect = lambda: "other"
            with self.assertRaises(runtime.HarnessBlocked) as raised:
                harness.first_session(mock.Mock(), workspace, workspace)
        self.assertEqual(raised.exception.code, "CLIPBOARD_TEXT_MISMATCH")

    def test_each_cold_reopen_failure_names_its_phase(self) -> None:
        saved = HARNESS.fingerprint_text(HARNESS.RESAVE_TEXT)
        for code, attribute, value, count in (
            ("REOPEN_EXACT_TEXT_MISMATCH", "editor_fingerprint",
             HARNESS.fingerprint_text("other"), 10),
            ("REOPEN_WELCOME_SHOWN", "welcome_absent", False, 10),
            ("REOPEN_RECENT_BOUND_NOT_TEN", None, None, 11),
        ):
            with self.subTest(code=code):
                harness = fake_session_harness()
                if attribute:
                    getattr(harness, attribute).side_effect = None
                    getattr(harness, attribute).return_value = value
                with (
                    mock.patch.object(HARNESS, "persisted_recent_count", return_value=count),
                    self.assertRaises(HARNESS_FAILURE) as raised,
                ):
                    harness.cold_reopen(mock.Mock(), Path("config"), saved)
                self.assertEqual(raised.exception.code, code)


class RecentControlBehaviorTests(unittest.TestCase):
    @staticmethod
    def harness(elements: list[object], query_error: BaseException | None = None) -> object:
        class ElementArray:
            def __init__(self, values: list[object]) -> None:
                self.values = values
                self.Length = len(values)

            def GetElement(self, index: int) -> object:
                return self.values[index]

        class RootElement:
            def FindAll(self, _scope: object, _condition: object) -> ElementArray:
                if query_error is not None:
                    raise query_error
                return ElementArray(elements)

        class FakeIUIA:
            @staticmethod
            def CreatePropertyCondition(property_id: object, value: object) -> object:
                return (property_id, value)

        class FakeUIA:
            UIA_dll = type("UIADll", (), {"UIA_AutomationIdPropertyId": "automation-id"})()
            iuia = FakeIUIA()
            tree_scope = {"descendants": object()}
            known_control_types = {"Button": 1}

        class ElementInfo:
            def __init__(self, element: object) -> None:
                self.automation_id = element.CurrentAutomationId
                self.control_type = "Button" if element.CurrentControlType == 1 else "Text"
                self.visible = element.visible

        class Control:
            def __init__(self, element_info: ElementInfo) -> None:
                self.element_info = element_info

            def is_visible(self) -> bool:
                return self.element_info.visible

        harness = object.__new__(HARNESS.Goal03Harness)
        harness.iuia_class = FakeUIA
        harness.fresh_uia_root = lambda _hwnd: type(
            "Root", (), {"element": RootElement()}
        )()
        harness.uia_wrapper_class = Control
        harness.uia_element_info_class = ElementInfo
        return harness

    @staticmethod
    def element(automation_id: str, control_type: int, *, visible: bool = True) -> object:
        return type(
            "Element",
            (),
            {
                "CurrentAutomationId": automation_id,
                "CurrentControlType": control_type,
                "visible": visible,
            },
        )()

    def test_recent_query_returns_every_recent_row_button(self) -> None:
        elements = [
            self.element(HARNESS.WELCOME_RECENT_AUTOMATION_ID, 1),
            self.element(HARNESS.WELCOME_RECENT_AUTOMATION_ID, 1),
        ]
        harness = self.harness(elements)

        controls = harness.recent_controls(type("App", (), {"hwnd": 42})())

        self.assertEqual(
            [control.element_info.automation_id for control in controls],
            [HARNESS.WELCOME_RECENT_AUTOMATION_ID] * 2,
        )

    def test_recent_query_rejects_a_hidden_or_mistyped_row(self) -> None:
        for element in (
            self.element(HARNESS.WELCOME_RECENT_AUTOMATION_ID, 2),
            self.element(HARNESS.WELCOME_RECENT_AUTOMATION_ID, 1, visible=False),
        ):
            with self.subTest(element=element), self.assertRaises(HARNESS_FAILURE) as raised:
                self.harness([element]).recent_controls(type("App", (), {"hwnd": 42})())
            self.assertEqual(raised.exception.code, "RECENT_UIA_CONTRACT_MISMATCH")

    def test_workspace_home_rows_share_their_own_id_and_failure_codes(self) -> None:
        harness = self.harness([self.element(HARNESS.WORKSPACE_HOME_FILE_AUTOMATION_ID, 1)])
        controls = harness.workspace_home_files(type("App", (), {"hwnd": 42})())
        self.assertEqual(len(controls), 1)

        harness = self.harness([self.element(HARNESS.WORKSPACE_HOME_FILE_AUTOMATION_ID, 2)])
        with self.assertRaises(HARNESS_FAILURE) as raised:
            harness.workspace_home_files(type("App", (), {"hwnd": 42})())
        self.assertEqual(raised.exception.code, "WORKSPACE_HOME_UIA_CONTRACT_MISMATCH")

    def test_recent_query_preserves_uia_contract_failures(self) -> None:
        failure = HARNESS_FAILURE("RECENT_UIA_CONTRACT_MISMATCH")
        harness = self.harness([], failure)

        with self.assertRaises(HARNESS_FAILURE) as raised:
            harness.recent_controls(type("App", (), {"hwnd": 42})())

        self.assertIs(raised.exception, failure)


class RestartRecentsTests(unittest.TestCase):
    """The restart phase: five rows, the stale one disabled and named
    `File <name> — Missing`, a real press on it opens nothing, and the saved
    file reopens from its row."""

    @staticmethod
    def row(name: str, *, enabled: bool = True) -> mock.Mock:
        control = mock.Mock()
        control.element_info.name = name
        control.is_enabled.return_value = enabled
        return control

    def run_phase(
        self, stale: mock.Mock, arrange=lambda harness, rows: None, count: int = 10
    ) -> tuple[mock.Mock, dict, mock.Mock]:
        harness = fake_session_harness()
        saved_row = self.row("File created.md")
        rows = [saved_row, self.row("Workspace workspace"), self.row("File recent-00.md"), stale,
                self.row("File recent-02.md")]
        harness.wait_recent_controls.return_value = rows
        arrange(harness, rows)
        with (
            mock.patch.object(HARNESS, "STALE_RECENT_OBSERVE_SECONDS", 0),
            mock.patch.object(HARNESS, "persisted_recent_count", return_value=count),
        ):
            observed = harness.restart_recents(
                mock.Mock(),
                Path("config"),
                Path("recent-01.md"),
                Path("created.md"),
                HARNESS.fingerprint_text(HARNESS.RESAVE_TEXT),
            )
        return harness, observed, saved_row

    def test_a_disabled_missing_row_is_pressed_and_opens_nothing(self) -> None:
        stale = self.row("File recent-01.md \u2014 Missing", enabled=False)
        harness, observed, saved_row = self.run_phase(stale)
        harness.hold_control.assert_called_once_with(
            mock.ANY, stale, HARNESS.STALE_RECENT_PRESS_SECONDS, "STALE_PRESS_FAILED"
        )
        harness.click_control.assert_called_once_with(saved_row, "SAVED_ROW_CLICK_FAILED")
        self.assertEqual(
            {key: observed[key] for key in ("recents_rows", "recents_disabled_rows", "recents_count_after_restart")},
            {"recents_rows": 5, "recents_disabled_rows": 1, "recents_count_after_restart": 10},
        )

    def test_an_enabled_or_unmarked_stale_row_fails(self) -> None:
        for stale in (
            self.row("File recent-01.md \u2014 Missing", enabled=True),
            self.row("File recent-01.md", enabled=False),
        ):
            with self.subTest(name=stale.element_info.name), self.assertRaises(
                HARNESS_FAILURE
            ) as raised:
                self.run_phase(stale)
            self.assertEqual(raised.exception.code, "RECENTS_STALE_NOT_DEGRADED")

    def test_each_restart_failure_names_its_phase(self) -> None:
        stale = self.row("File recent-01.md — Missing", enabled=False)

        def set_editor(text):
            return lambda harness, _rows: harness.state.update(editor=text)

        for code, arrange, count in (
            ("RECENTS_ROW_COUNT", lambda _h, rows: rows.append(self.row("File more.md")), 10),
            ("RECENTS_DISABLED_COUNT", lambda _h, rows: rows[2].is_enabled.configure_mock(
                return_value=False), 10),
            ("RECENTS_STALE_UIA_CONTRACT_MISMATCH",
             lambda _h, rows: rows.__setitem__(4, self.row("File recent-01.md copy")), 10),
            ("RECENTS_STALE_OPENED_DOCUMENT",
             lambda h, _rows: setattr(h.editor_absent_while_running, "return_value", False), 10),
            ("RECENTS_SAVED_ROW_UIA_CONTRACT_MISMATCH",
             lambda _h, rows: rows.__setitem__(0, self.row("File renamed.md")), 10),
            ("RECENTS_EXACT_TEXT_MISMATCH", set_editor("other"), 10),
            ("RECENTS_RECENT_BOUND_NOT_TEN", lambda _h, _rows: None, 11),
        ):
            with self.subTest(code=code), self.assertRaises(HARNESS_FAILURE) as raised:
                self.run_phase(stale, arrange, count)
            self.assertEqual(raised.exception.code, code)

    def test_the_stale_press_must_keep_opening_nothing(self) -> None:
        polls = iter([True, True, False])
        with self.assertRaises(HARNESS_FAILURE) as raised:
            HARNESS.require_for(lambda: next(polls), 5.0, "STALE_OPENED_DOCUMENT")
        self.assertEqual(raised.exception.code, "STALE_OPENED_DOCUMENT")
        HARNESS.require_for(lambda: True, 0.1, "STALE_OPENED_DOCUMENT")


class RuntimeArtifactPrivacyTests(unittest.TestCase):
    def test_runtime_scan_rejects_document_text_in_data_or_config_but_not_workspace(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            (root / "data" / "logs").mkdir(parents=True)
            (root / "config").mkdir()
            (root / "workspace").mkdir()
            (root / "data" / "logs" / "markturbo.log").write_text("startup", encoding="utf-8")
            (root / "stderr.log").write_text("", encoding="utf-8")
            (root / "workspace" / "saved.md").write_text(DOCUMENT_SENTINEL, encoding="utf-8")
            HARNESS.scan_case_artifacts(root)

            for leak in ("data/leak.log", "config/settings.toml"):
                with self.subTest(leak=leak):
                    (root / leak).write_text(DOCUMENT_SENTINEL, encoding="utf-8")
                    with self.assertRaises(HARNESS_FAILURE) as raised:
                        HARNESS.scan_case_artifacts(root)
                    self.assertEqual(raised.exception.code, "UTF8_DOCUMENT_SENTINEL_LEAKED")
                    (root / leak).unlink()


if __name__ == "__main__":
    unittest.main()
