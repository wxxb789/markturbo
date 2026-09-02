"""Unit tests for deterministic performance fixture generation."""

from __future__ import annotations

import contextlib
import io
import tempfile
import unittest
from pathlib import Path
from unittest import mock

from scripts.markturbo_tools import perf_fixtures


class PerformanceFixtureTests(unittest.TestCase):
    def test_document_generation_is_deterministic(self) -> None:
        self.assertEqual(perf_fixtures.make(40), perf_fixtures.make(40))
        self.assertTrue(perf_fixtures.make(40).endswith("\n"))

    def test_diagram_fixture_has_unique_diagrams(self) -> None:
        document = perf_fixtures.make_diagram_heavy(3)

        self.assertIn("A0[Node 0] --> B0[Next];", document)
        self.assertIn("A2[Node 2] --> B2[Next];", document)
        self.assertEqual(document.count("```mermaid"), 3)
        self.assertEqual(document.count("```d2"), 3)

    def test_write_uses_lf_on_windows_and_unix(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            destination = root / "fixture.md"
            with mock.patch.object(perf_fixtures, "ROOT", root):
                with contextlib.redirect_stdout(io.StringIO()):
                    perf_fixtures.write(destination, "one\ntwo\n")

            self.assertEqual(destination.read_bytes(), b"one\ntwo\n")
