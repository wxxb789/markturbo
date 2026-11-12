"""Behavioral tests for the Goal 09 capture matrix's portable contract."""

from __future__ import annotations

import tempfile
import unittest
from pathlib import Path

from scripts.markturbo_tools.native import visual


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

    def test_physical_viewports_convert_dip_to_the_window_dpi(self) -> None:
        self.assertEqual(visual.physical_viewport("minimum", 120), (900, 600))
        self.assertEqual(visual.physical_viewport("default", 120), (1750, 1125))
        self.assertEqual(visual.physical_viewport("wide", 120), (2400, 1350))
        self.assertEqual(visual.physical_viewport("default", 96), (1400, 900))

    def test_credential_identity_is_endpoint_bound(self) -> None:
        first = visual.credential_identity(43123)
        second = visual.credential_identity(43124)
        self.assertNotEqual(first, second)
        self.assertIn("wire=openai-chat", first)
        self.assertIn("host=127.0.0.1", first)
        self.assertRegex(first, r"identity-sha256=[0-9a-f]{64}$")

    def test_theme_check_accepts_the_shipped_backgrounds(self) -> None:
        self.assertEqual(
            visual.color_distance((255, 255, 255), (255, 255, 255)), 0
        )
        self.assertLessEqual(
            visual.color_distance((250, 252, 251), visual.THEME_BACKGROUNDS["light"]),
            10,
        )
        self.assertGreater(
            visual.color_distance(visual.THEME_BACKGROUNDS["light"], visual.THEME_BACKGROUNDS["dark"]),
            10,
        )

    def test_settings_write_requested_identity_without_a_credential(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            path = Path(temporary) / "settings.toml"
            visual.write_settings(path, "dark", "zh-cn", None)
            text = path.read_text(encoding="utf-8")
            self.assertIn('theme = "dark"', text)
            self.assertIn('language = "zh-cn"', text)
            self.assertNotIn("api", text.lower())

    def test_web_preview_rejects_a_blank_pane_and_accepts_content(self) -> None:
        from PIL import Image, ImageDraw

        blank = Image.new("RGB", (900, 600), (255, 255, 255))
        self.assertFalse(visual.web_content_present(blank, "light", (270, 250, 890, 550)))
        painted = blank.copy()
        ImageDraw.Draw(painted).rectangle((300, 300, 500, 400), fill=(20, 20, 20))
        self.assertTrue(visual.web_content_present(painted, "light", (270, 250, 890, 550)))

    def test_web_preview_accepts_small_text_in_the_wide_pane(self) -> None:
        from PIL import Image, ImageDraw

        wide = Image.new("RGB", (2400, 1350), (255, 255, 255))
        ImageDraw.Draw(wide).rectangle((340, 290, 620, 460), fill=(20, 20, 20))
        self.assertTrue(
            visual.web_content_present(wide, "light", (325, 271, 2400, 1321))
        )

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


if __name__ == "__main__":
    unittest.main()
