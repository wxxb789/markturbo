"""The native selector module against the Rust UI source it drives."""

from __future__ import annotations

import re
import tempfile
import unittest
from pathlib import Path
from typing import Any

from scripts.markturbo_tools.native import selectors

UI = Path(__file__).resolve().parents[2] / "crates" / "mt-app" / "src" / "ui"
TEST_MODULE = re.compile(r"\n#\[cfg\(test\)\]\s*\nmod tests\b")
TEST_ID = re.compile(r'\.test_id\("([^"]+)"\)')


def production_sources(root: Path) -> tuple[str, list[str]]:
    """Every UI source with its inline test module stripped, plus the files
    that were stripped.

    A file that mentions `#[cfg(test)]` but has no `mod tests` after it is an
    error: the strip must never silently stop working.
    """
    production: list[str] = []
    stripped: list[str] = []
    for path in sorted(root.glob("*.rs")):
        text = path.read_text(encoding="utf-8")
        match = TEST_MODULE.search(text)
        if match is not None:
            text = text[: match.start()]
            stripped.append(path.name)
        elif "#[cfg(test)]" in text:
            raise AssertionError(
                f"{path.name} mentions cfg(test) without an inline tests module"
            )
        production.append(text)
    return "\n".join(production), stripped


def selector_ids(module: Any) -> dict[str, str]:
    """Every string this module sends to the UI, keyed by its constant."""
    ids: dict[str, str] = {}
    for name, value in vars(module).items():
        if name.startswith("_"):
            continue
        if name == "EDITOR_NAME" and isinstance(value, str):
            ids[name] = value
        elif name.endswith("_AUTOMATION_ID") and isinstance(value, str):
            ids[name] = value
        elif name.endswith("_AUTOMATION_IDS"):
            values = value.values() if isinstance(value, dict) else value
            for nested in values:
                ids[f"{name}:{nested}"] = nested
        elif name == "CJK_OPTICS_VARIANTS":
            for variant_id, _ in value:
                ids[f"{name}:{variant_id}"] = variant_id
    return ids


def production_mentions(production: str, automation_id: str) -> bool:
    """Whether the production UI source carries the selector's literal, or
    the format template for a per-row id such as `recovery-restore-{index}`."""
    if f'"{automation_id}"' in production:
        return True
    prefix = automation_id.rpartition("-")[0]
    return bool(prefix) and f'"{prefix}-{{index}}"' in production


class SelectorGuardTests(unittest.TestCase):
    def test_every_selector_exists_in_production_ui_source(self) -> None:
        production, stripped = production_sources(UI)

        self.assertTrue(stripped, "the guard must strip at least one inline test module")
        self.assertGreater(len(TEST_ID.findall(production)), 0)
        for name, value in sorted(selector_ids(selectors).items()):
            with self.subTest(selector=name):
                self.assertTrue(
                    production_mentions(production, value),
                    f"{name} ({value!r}) is not in the production UI source",
                )

    def test_the_strip_removes_test_only_ids(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            (root / "view.rs").write_text(
                'fn view() { widget.test_id("production-id"); }\n\n'
                "#[cfg(test)]\nmod tests {\n"
                '    fn t() { widget.test_id("test-only-id"); }\n'
                "}\n",
                encoding="utf-8",
            )

            production, stripped = production_sources(root)

        self.assertEqual(stripped, ["view.rs"])
        self.assertIn('"production-id"', production)
        self.assertNotIn('"test-only-id"', production)

    def test_a_marker_without_an_inline_tests_module_fails_closed(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            (root / "view.rs").write_text(
                '#[cfg(test)]\nfn helper() {}\n'
                'fn view() { widget.test_id("production-id"); }\n',
                encoding="utf-8",
            )

            with self.assertRaisesRegex(AssertionError, "without an inline tests module"):
                production_sources(root)

    def test_consent_hold_outlasts_the_rust_charge(self) -> None:
        motion = (UI / "motion.rs").read_text(encoding="utf-8")
        match = re.search(r"FILAMENT_CHARGE_MS: f32 = ([\d.]+)", motion)
        self.assertIsNotNone(match)
        charge_seconds = float(match.group(1)) / 1000.0
        self.assertGreater(selectors.CONSENT_HOLD_SECONDS, charge_seconds)


if __name__ == "__main__":
    unittest.main()
