"""Behavioral tests for the Goal 09 capture matrix's portable contract."""

from __future__ import annotations

import http.client
import json
import os
import shlex
import tempfile
import threading
import unittest
from pathlib import Path
from typing import Any
from unittest import mock

from scripts.markturbo_tools import cli
from scripts.markturbo_tools.native import runtime, visual


class MatrixTests(unittest.TestCase):
    def test_default_matrix_has_every_state_language_viewport_and_mode(self) -> None:
        cells = visual.matrix_cells()
        self.assertEqual(len(cells), 132)
        self.assertEqual(cells[0].cell_id, "en-us-clean-profile-welcome-minimum-light")
        self.assertEqual(cells[-1].cell_id, "zh-cn-settings-wide-dark")
        self.assertEqual(
            {cell.state for cell in cells},
            set(visual.STATES),
        )
        self.assertEqual(
            {cell.language for cell in cells},
            set(visual.LANGUAGES),
        )

    def test_goal17_cjk_recipe_is_opt_in_and_separate_from_the_default_matrix(self) -> None:
        cell = visual.cjk_optics_cell()
        self.assertEqual(cell.cell_id, "zh-cn-cjk-optics-default-light")
        self.assertNotIn(cell.state, visual.STATES)
        self.assertEqual(
            visual.CJK_OPTICS_VARIANTS,
            (
                ("cjk-optics-plus-0", "+0"),
                ("cjk-optics-plus-1px", "+1px"),
                ("cjk-optics-plus-1px-next-weight", "+1px / next companion weight"),
            ),
        )
        arguments = visual.parser().parse_args(
            ["--exe", "markturbo.exe", "--out", "captures", "--goal17-cjk-optics"]
        )
        self.assertTrue(arguments.goal17_cjk_optics)

    def test_goal17_capture_seeds_only_the_font_before_app_launch(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            user_data = root / "user-data"
            source = user_data / "fonts" / visual.CJK_COMPANION_FONT_NAME
            source.parent.mkdir(parents=True)
            source.write_bytes(b"companion font")
            (user_data / "settings.toml").write_text("theme = 'dark'", encoding="utf-8")
            harness = mock.Mock()
            observed: dict[str, Any] = {}

            def stop_before_launch(
                _target: Path | None,
                data: Path,
                _config: Path,
                _workspace: Path,
                _stderr: Path,
            ) -> None:
                observed["data_entries"] = sorted(path.name for path in data.iterdir())
                font_dir = data / "fonts"
                observed["font_entries"] = sorted(path.name for path in font_dir.iterdir())
                observed["font_bytes"] = (font_dir / visual.CJK_COMPANION_FONT_NAME).read_bytes()
                raise runtime.HarnessFailure("CJK_TEST_STOP")

            harness.launch_app.side_effect = stop_before_launch
            with (
                mock.patch.object(visual, "VisualHarness", return_value=harness),
                mock.patch.dict(
                    os.environ, {"MARKTURBO_DATA_DIR": str(user_data)}, clear=False
                ),
            ):
                receipt = visual.capture_cell(
                    mock.Mock(),
                    Path("markturbo.exe"),
                    root / "out",
                    visual.cjk_optics_cell(),
                    visual.SourceBinding("head", "tree", False, None),
                    {},
                    (),
                    mock.Mock(),
                )

            self.assertEqual(observed["data_entries"], ["fonts"])
            self.assertEqual(observed["font_entries"], [visual.CJK_COMPANION_FONT_NAME])
            self.assertEqual(observed["font_bytes"], b"companion font")
            self.assertEqual(receipt["reason_code"], "CJK_TEST_STOP")
            harness.cleanup.assert_called_once()

    def test_missing_goal17_companion_font_blocks_capture_before_launch(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            user_data = root / "user-data"
            harness = mock.Mock()
            with (
                mock.patch.object(visual, "VisualHarness", return_value=harness),
                mock.patch.dict(
                    os.environ, {"MARKTURBO_DATA_DIR": str(user_data)}, clear=False
                ),
            ):
                receipt = visual.capture_cell(
                    mock.Mock(),
                    Path("markturbo.exe"),
                    root / "out",
                    visual.cjk_optics_cell(),
                    visual.SourceBinding("head", "tree", False, None),
                    {},
                    (),
                    mock.Mock(),
                )

            harness.launch_app.assert_not_called()
            stored = json.loads(
                (
                    root
                    / "out"
                    / "cells"
                    / visual.cjk_optics_cell().cell_id
                    / "receipt.json"
                ).read_text(encoding="utf-8")
            )
            self.assertEqual(receipt["status"], "BLOCKED")
            self.assertEqual(stored["status"], "BLOCKED")
            self.assertEqual(stored["reason_code"], "CJK_COMPANION_FONT_MISSING")
            self.assertNotIn(str(user_data), json.dumps(stored))

    def test_goal17_capture_requires_the_companion_font_to_be_loaded(self) -> None:
        harness = mock.Mock()
        harness.shows_text.side_effect = lambda _app, text: (
            text == visual.CJK_COMPANION_FONT_LOADED
        )
        visual.require_companion_font_loaded(harness, mock.Mock())

        harness.shows_text.side_effect = lambda _app, text: (
            text == visual.CJK_COMPANION_FONT_UNAVAILABLE
        )
        with self.assertRaises(runtime.HarnessBlocked) as raised:
            visual.require_companion_font_loaded(harness, mock.Mock())
        self.assertEqual(raised.exception.code, "CJK_COMPANION_FONT_NOT_LOADED")

        with mock.patch.object(
            visual,
            "wait_until",
            side_effect=runtime.HarnessFailure("CJK_COMPANION_FONT_STATE_MISSING"),
        ):
            with self.assertRaises(runtime.HarnessBlocked) as missing:
                visual.require_companion_font_loaded(harness, mock.Mock())
        self.assertEqual(missing.exception.code, "CJK_COMPANION_FONT_STATE_MISSING")

    def test_goal17_capture_cell_blocks_when_the_companion_font_is_unavailable(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            user_data = root / "user-data"
            font = user_data / "fonts" / visual.CJK_COMPANION_FONT_NAME
            font.parent.mkdir(parents=True)
            font.write_bytes(b"companion font")
            app = mock.Mock(window=mock.Mock(handle=1))
            harness = mock.Mock()
            harness.shows_text.side_effect = lambda _app, text: (
                text != visual.CJK_COMPANION_FONT_LOADED
            )
            window = mock.Mock(hwnd=1)
            window.foreground.return_value = True
            window.accessibility.return_value = {"unnamed": []}
            window.obstruction.return_value = {"visible": False}
            win32 = mock.Mock()
            win32.user32.GetDpiForWindow.return_value = 96

            harness.launch_app.return_value = app
            with (
                mock.patch.object(visual, "VisualHarness", return_value=harness),
                mock.patch.object(visual, "AppWindow", return_value=window),
                mock.patch.object(
                    visual, "dpi_awareness", return_value=(0, 0, (0, 0, 500, 500))
                ),
                mock.patch.object(visual, "capture_window", side_effect=RuntimeError),
                mock.patch.object(visual, "SETTLE_SECONDS", 0),
                mock.patch.dict(
                    os.environ, {"MARKTURBO_DATA_DIR": str(user_data)}, clear=False
                ),
            ):
                receipt = visual.capture_cell(
                    win32,
                    root / "markturbo.exe",
                    root / "out",
                    visual.cjk_optics_cell(),
                    visual.SourceBinding("head", "tree", False, None),
                    {"name": "markturbo.exe", "byte_count": 1, "sha256": "a" * 64},
                    (None,) * 5,
                    None,
                )

        self.assertEqual(
            (receipt["status"], receipt["reason_code"]),
            ("BLOCKED", "CJK_COMPANION_FONT_NOT_LOADED"),
        )

    def test_documented_goal17_visual_command_parses_through_mt_cli(self) -> None:
        acceptance = Path(__file__).resolve().parents[2] / "docs" / "goal-17-acceptance.md"
        command = next(
            line.strip()
            for line in acceptance.read_text(encoding="utf-8").splitlines()
            if line.strip().startswith("uv run --locked --project scripts scripts/mt.py visual")
        )
        tokens = shlex.split(command)
        entrypoint = next(index for index, token in enumerate(tokens) if token.endswith("mt.py"))
        arguments = cli.parser().parse_args(tokens[entrypoint + 1 :])
        self.assertEqual(arguments.command, "visual")
        visual_arguments = visual.parser().parse_args(cli.forwarded(arguments.args))
        self.assertTrue(visual_arguments.goal17_cjk_optics)
        self.assertEqual(visual_arguments.exe, Path("<release-exe>"))
        self.assertEqual(visual_arguments.out, Path(".scratch/goal-17-cjk"))

    def test_physical_viewports_convert_dip_to_the_window_dpi(self) -> None:
        self.assertEqual(visual.physical_viewport("minimum", 120), (900, 600))
        self.assertEqual(visual.physical_viewport("default", 120), (1750, 1125))
        self.assertEqual(visual.physical_viewport("wide", 120), (2400, 1350))
        self.assertEqual(visual.physical_viewport("default", 96), (1400, 900))

    def test_credential_identity_is_endpoint_bound(self) -> None:
        first = visual.credential_identity(43123)
        second = visual.credential_identity(43124)
        self.assertNotEqual(first, second)
        self.assertIn("wire=openai-responses", first)
        self.assertIn("host=127.0.0.1", first)
        self.assertRegex(first, r"identity-sha256=[0-9a-f]{64}$")

    def test_theme_check_tells_the_two_rooms_apart(self) -> None:
        light = visual.THEME_BACKGROUNDS["light"]
        dark = visual.THEME_BACKGROUNDS["dark"]
        self.assertEqual(visual.color_distance(light, visual.expected_background("light")), 0)
        self.assertLessEqual(visual.color_distance((16, 18, 20), dark), 10)
        self.assertGreater(visual.color_distance(light, dark), 10)

    def test_settings_write_requested_identity_without_a_credential(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            path = Path(temporary) / "settings.toml"
            visual.write_settings(path, "dark", "zh-cn", None)
            text = path.read_text(encoding="utf-8")
            self.assertIn('theme = "dark"', text)
            self.assertIn('language = "zh-cn"', text)
            self.assertNotIn("model", text)

            server = visual.ReviewServer(hold=False)
            try:
                visual.write_settings(path, "light", "en-us", server)
            finally:
                server.server.server_close()
            text = path.read_text(encoding="utf-8")
            self.assertIn('model-provider = "openai-responses"', text)
            self.assertIn(f'model-base-url = "{server.base_url}"', text)
            self.assertIn(visual.credential_identity(server.port), text)
            self.assertNotIn("api-key", text)

    def test_web_preview_rejects_a_blank_pane_and_accepts_content(self) -> None:
        from PIL import Image, ImageDraw

        for ground in ((255, 255, 255), (16, 17, 19)):
            blank = Image.new("RGB", (900, 600), ground)
            self.assertFalse(visual.web_content_present(blank, (270, 250, 890, 550)))
            painted = blank.copy()
            ink = (128, 128, 128)
            ImageDraw.Draw(painted).rectangle((300, 300, 500, 400), fill=ink)
            self.assertTrue(visual.web_content_present(painted, (270, 250, 890, 550)))

    def test_web_preview_accepts_small_text_in_the_wide_pane(self) -> None:
        from PIL import Image, ImageDraw

        wide = Image.new("RGB", (2400, 1350), (255, 255, 255))
        ImageDraw.Draw(wide).rectangle((340, 290, 620, 460), fill=(20, 20, 20))
        self.assertTrue(visual.web_content_present(wide, (325, 271, 2400, 1321)))

    def test_composited_page_lands_on_the_reported_pane_rectangle(self) -> None:
        import io

        from PIL import Image

        background = Image.new("RGB", (300, 200), (255, 255, 255))
        page = Image.new("RGB", (100, 100), (10, 20, 30))
        buffer = io.BytesIO()
        page.save(buffer, format="PNG")
        composed = visual.composite_web(
            background, buffer.getvalue(), (50, 40, 150, 140)
        )
        self.assertEqual(composed.getpixel((60, 50)), (10, 20, 30))
        self.assertEqual(composed.getpixel((10, 10)), (255, 255, 255))

    def test_csv_parser_rejects_unknown_values(self) -> None:
        with self.assertRaises(Exception):
            visual.parse_csv("minimum,enormous", tuple(visual.VIEWPORTS))

    def test_child_environment_is_restored_after_the_launch(self) -> None:
        name = "MARKTURBO_GOAL09_TEST_VARIABLE"
        self.assertNotIn(name, os.environ)
        with visual.child_environment({name: "1"}):
            self.assertEqual(os.environ[name], "1")
        self.assertNotIn(name, os.environ)


def post(server: visual.ReviewServer, path: str = "/v1/responses") -> tuple[int, bytes]:
    connection = http.client.HTTPConnection("127.0.0.1", server.port, timeout=5)
    try:
        body = json.dumps({"metadata": {"operation": "read_only_review"}})
        connection.request("POST", path, body, {"content-type": "application/json"})
        response = connection.getresponse()
        return response.status, response.read()
    finally:
        connection.close()


class ReviewServerTests(unittest.TestCase):
    def setUp(self) -> None:
        self.server = visual.ReviewServer(hold=False)
        self.server.start()
        self.addCleanup(self.server.close)

    def test_a_request_before_the_consent_hold_is_refused(self) -> None:
        status, _ = post(self.server)
        self.assertEqual(status, 409)
        self.assertEqual(self.server.request_count, 0)
        self.assertEqual(self.server.refusal, "REVIEW_REQUEST_BEFORE_CONSENT")

    def test_a_consented_review_gets_the_fixed_questions(self) -> None:
        self.server.allow()
        status, body = post(self.server)
        self.assertEqual(status, 200)
        self.assertEqual(self.server.request_count, 1)
        self.assertIsNone(self.server.refusal)
        response = json.loads(body)
        self.assertEqual(response["object"], "response")
        review = json.loads(response["output"][0]["content"][0]["text"])
        self.assertEqual(
            [question["question"] for question in review["clarification_questions"]],
            [question for question, _, _ in visual.REVIEW_QUESTIONS],
        )

    def test_another_path_is_refused(self) -> None:
        self.server.allow()
        status, _ = post(self.server, "/v1/chat/completions")
        self.assertEqual(status, 404)
        self.assertEqual(self.server.refusal, "REVIEW_REQUEST_PATH_MISMATCH")

    def test_a_held_review_answers_only_when_released(self) -> None:
        held = visual.ReviewServer(hold=True)
        held.start()
        held.allow()
        result: dict[str, int] = {}
        thread = threading.Thread(target=lambda: result.update(status=post(held)[0]))
        thread.start()
        try:
            runtime.wait_until(lambda: held.request_count == 1, 5.0, "NOT_SEEN")
            thread.join(0.3)
            self.assertTrue(thread.is_alive())
        finally:
            held.close()
        thread.join(5)
        self.assertEqual(result.get("status"), 200)


class FakeControl:
    def __init__(self, ui: FakeUi, automation_id: str) -> None:
        self.ui = ui
        self.automation_id = automation_id
        self.element_info = mock.Mock(name=automation_id)
        self.element_info.name = ui.names.get(automation_id, "")

    def is_visible(self) -> bool:
        return True

    def is_enabled(self) -> bool:
        return True

    def invoke(self) -> None:
        self.ui.pressed.append(self.automation_id)
        self.ui.on_press(self.automation_id)


class FakeUi:
    """The controls a fake app shows, keyed by AutomationId, and what each
    press reveals next."""

    def __init__(self, shown: set[str], reveals: dict[str, set[str]]) -> None:
        self.shown = set(shown)
        self.reveals = reveals
        self.names: dict[str, str] = {}
        self.texts: set[str] = set()
        self.pressed: list[str] = []

    def on_press(self, automation_id: str) -> None:
        self.shown |= self.reveals.get(automation_id, set())


def fake_harness(ui: FakeUi) -> visual.VisualHarness:
    harness = object.__new__(visual.VisualHarness)
    harness.ui_timeout = 0.2
    harness._credential = None

    def control_by_id(_hwnd: int, automation_id: str, _type: str, *_args: Any) -> Any:
        return FakeControl(ui, automation_id) if automation_id in ui.shown else None

    def menu_command(_app: Any, automation_id: str) -> None:
        FakeControl(ui, automation_id).invoke()

    harness.control_by_id = control_by_id
    harness.menu_command = menu_command
    harness.name_present = lambda _app, name, _type: name in ui.texts
    harness.editor_control = lambda *_args: object()
    return harness


APP = mock.Mock(hwnd=1)


def cell(state: str, language: str = "en-us") -> visual.Cell:
    return visual.Cell(state, language, "default", "light")


class StatePlanTests(unittest.TestCase):
    def test_every_state_has_a_plan(self) -> None:
        harness = fake_harness(FakeUi(set(), {}))
        self.assertEqual(set(harness._plans()), set(visual.STATES))

    def test_settings_is_proven_by_its_page_not_by_the_menu_press(self) -> None:
        ui = FakeUi({"title-settings"}, {"title-menu-settings": {"settings-save"}})
        markers = fake_harness(ui).prepare(APP, cell("settings"), Path("plan.md"), None)
        self.assertEqual(ui.pressed, ["title-menu-settings"])
        self.assertIn("settings-save", markers)

        unopened = FakeUi({"title-settings"}, {})
        with self.assertRaises(runtime.HarnessFailure) as raised:
            fake_harness(unopened).prepare(APP, cell("settings"), Path("plan.md"), None)
        self.assertEqual(raised.exception.code, "SETTINGS_PAGE_MISSING")

    def test_welcome_fails_when_a_document_is_already_open(self) -> None:
        ui = FakeUi({"welcome-new-file", "title-file"}, {})
        with self.assertRaises(runtime.HarnessFailure) as raised:
            fake_harness(ui).prepare(APP, cell("clean-profile-welcome"), Path("p"), None)
        self.assertEqual(raised.exception.code, "CLEAN_PROFILE_WELCOME_DOCUMENT_OPEN")

    def test_a_document_state_waits_for_the_fixture_tab(self) -> None:
        ui = FakeUi({"title-file"}, {"title-menu-web": {"web-trust"}})
        ui.names["title-file"] = "other.md"
        with self.assertRaises(runtime.HarnessFailure) as raised:
            fake_harness(ui).prepare(APP, cell("web-compatibility-preview"), Path("p"), None)
        self.assertEqual(raised.exception.code, "WEB_COMPATIBILITY_PREVIEW_DOCUMENT_NOT_OPEN")

        ui.names["title-file"] = visual.PLAN_NAME
        markers = fake_harness(ui).prepare(APP, cell("web-compatibility-preview"), Path("p"), None)
        self.assertEqual(markers[-1], "web-trust")

    def test_review_sends_only_through_the_consent_hold(self) -> None:
        ui = FakeUi(
            {"title-file", "title-review"},
            {"title-review": {"consent-send"}},
        )
        ui.names["title-file"] = visual.PLAN_NAME
        server = visual.ReviewServer(hold=True)
        server.start()
        self.addCleanup(server.close)
        harness = fake_harness(ui)
        holds: list[bool] = []

        def hold(_app: Any, control: Any, _seconds: float, _code: str) -> None:
            # The real hold's effect: the app sends, then shows its wait.
            self.assertEqual(control.automation_id, "consent-send")
            holds.append(server.consented.is_set())
            threading.Thread(target=post, args=(server,), daemon=True).start()
            ui.shown.add("review-cancel")

        harness.hold_control = hold
        markers = harness.prepare(APP, cell("review-loading"), Path("p"), server)
        self.assertEqual(holds, [True])
        self.assertEqual(server.request_count, 1)
        self.assertEqual(ui.pressed, ["title-review"])
        self.assertEqual(markers[-1], "review-cancel")

    def test_context_is_proven_by_its_source_toggle_in_the_cell_language(self) -> None:
        for language in visual.LANGUAGES:
            ui = FakeUi({"title-file"}, {"title-menu-context": {"context-close"}})
            ui.names["title-file"] = visual.PLAN_NAME
            ui.texts.add(visual.CONTEXT_SOURCE_TOGGLE[language])
            markers = fake_harness(ui).prepare(
                APP, cell("effective-agent-context", language), Path("p"), None
            )
            self.assertEqual(markers[-1], "context-source-toggle")

        empty = FakeUi({"title-file"}, {"title-menu-context": {"context-close"}})
        empty.names["title-file"] = visual.PLAN_NAME
        with self.assertRaises(runtime.HarnessFailure) as raised:
            fake_harness(empty).prepare(APP, cell("effective-agent-context"), Path("p"), None)
        self.assertEqual(raised.exception.code, "EFFECTIVE_AGENT_CONTEXT_CONTEXT_SOURCE_MISSING")

    def review_ui(self) -> tuple[FakeUi, visual.ReviewServer]:
        ui = FakeUi({"title-file", "title-review"}, {"title-review": {"consent-send"}})
        ui.names["title-file"] = visual.PLAN_NAME
        server = visual.ReviewServer(hold=True)
        server.start()
        self.addCleanup(server.close)
        return ui, server

    @mock.patch.object(visual, "REQUEST_SENT_TIMEOUT_SECONDS", 0.2)
    def test_an_unseen_hold_is_repeated_once_while_the_card_is_open(self) -> None:
        ui, server = self.review_ui()
        harness = fake_harness(ui)

        def second_hold_sends(_app: Any, _control: Any, _seconds: float, _code: str) -> None:
            if harness.consent_holds == 2:
                threading.Thread(target=post, args=(server,), daemon=True).start()
                ui.shown.add("review-cancel")

        harness.hold_control = second_hold_sends
        harness.prepare(APP, cell("review-loading"), Path("p"), server)
        self.assertEqual(harness.consent_holds, 2)
        self.assertEqual(server.request_count, 1)

        ui, server = self.review_ui()
        harness = fake_harness(ui)
        harness.hold_control = lambda *_args: ui.shown.discard("consent-send")
        with self.assertRaises(runtime.HarnessFailure) as raised:
            harness.prepare(APP, cell("review-loading"), Path("p"), server)
        self.assertEqual(raised.exception.code, "REVIEW_LOADING_REVIEW_REQUEST_NOT_SENT")
        self.assertEqual(harness.consent_holds, 1)

    def test_a_review_sent_before_the_consent_card_fails(self) -> None:
        ui = FakeUi({"title-file", "title-review"}, {"title-review": {"consent-send"}})
        ui.names["title-file"] = visual.PLAN_NAME
        server = visual.ReviewServer(hold=False)
        server.start()
        self.addCleanup(server.close)
        post(server)
        harness = fake_harness(ui)
        harness.hold_control = mock.Mock()
        with self.assertRaises(runtime.HarnessFailure) as raised:
            harness.prepare(APP, cell("review-loading"), Path("p"), server)
        self.assertEqual(raised.exception.code, "REVIEW_LOADING_REVIEW_REQUEST_BEFORE_CONSENT")
        harness.hold_control.assert_not_called()


class CaptureCellTests(unittest.TestCase):
    def test_a_profile_a_browser_holds_is_removed_after_ending_that_browser(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            profile = Path(temporary) / "profile"
            (profile / "local-app-data").mkdir(parents=True)
            held = {"browser": True}
            real_rmtree = visual.shutil.rmtree

            def rmtree(path: Path, ignore_errors: bool = False) -> None:
                if not held["browser"]:
                    real_rmtree(path, ignore_errors=ignore_errors)

            def stop(path: Path) -> None:
                self.assertEqual(path, profile)
                held["browser"] = False

            with (
                mock.patch.object(visual.shutil, "rmtree", rmtree),
                mock.patch.object(visual, "stop_profile_browsers", side_effect=stop) as stopped,
            ):
                visual.remove_profile(profile, 0.1)
            self.assertFalse(profile.exists())
            stopped.assert_called_once()

    def test_a_failed_launch_writes_a_content_free_receipt_and_no_profile(self) -> None:
        profiles: list[Path] = []
        real_mkdtemp = tempfile.mkdtemp

        def mkdtemp(**kwargs: Any) -> str:
            path = real_mkdtemp(**kwargs)
            profiles.append(Path(path))
            return path

        def launch(_self: Any, target: Path | None, *_args: Any) -> None:
            self.assertEqual(target.name, visual.PLAN_NAME)
            # The WebView2 profile (`%LOCALAPPDATA%`) is the cell's own.
            self.assertTrue(Path(os.environ["LOCALAPPDATA"]).is_relative_to(profiles[0]))
            raise runtime.HarnessFailure("MAIN_WINDOW_UIA_TIMEOUT")

        source = visual.SourceBinding("h" * 40, "t" * 40, False, None)
        with (
            tempfile.TemporaryDirectory() as temporary,
            mock.patch.object(visual.tempfile, "mkdtemp", mkdtemp),
            mock.patch.object(visual.VisualHarness, "launch_app", launch),
        ):
            out = Path(temporary)
            receipt = visual.capture_cell(
                mock.Mock(),
                out / "markturbo.exe",
                out,
                cell("review-result-questions"),
                source,
                {"name": "markturbo.exe", "byte_count": 1, "sha256": "a" * 64},
                (None,) * 5,
                None,
            )
            cell_root = out / "cells" / "en-us-review-result-questions-default-light"
            written = sorted(path.name for path in cell_root.iterdir())
            text = (cell_root / "receipt.json").read_text(encoding="utf-8")
        self.assertEqual(receipt["status"], "FAILED")
        self.assertEqual(receipt["reason_code"], "MAIN_WINDOW_UIA_TIMEOUT")
        self.assertEqual(written, ["receipt.json"])
        self.assertEqual(len(profiles), 1)
        self.assertFalse(profiles[0].exists())
        self.assertNotIn(str(profiles[0]), text)
        self.assertNotIn(temporary, text)
        self.assertNotIn("Release plan", text)


if __name__ == "__main__":
    unittest.main()
