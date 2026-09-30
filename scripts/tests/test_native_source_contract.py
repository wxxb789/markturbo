"""Focused tests for the native harness Rust source lexical views."""

from __future__ import annotations

import tempfile
import unittest
from pathlib import Path

from scripts.markturbo_tools.native.source_contract import production_source, rust_source_views


class ProductionSourceTests(unittest.TestCase):
    @staticmethod
    def project(text: str) -> str | None:
        with tempfile.TemporaryDirectory() as directory:
            source = Path(directory) / "source.rs"
            source.write_bytes(text.encode("utf-8"))
            return production_source(source)

    @staticmethod
    def masked(text: str) -> str:
        return "".join(character if character in "\r\n" else " " for character in text)

    def test_test_only_items_are_masked_without_shifting_later_production(self) -> None:
        items = (
            "#[cfg(test)] mod checks { fn test_only() {} }\n",
            "# /* attr */ [ cfg /* condition */ (\n test \n) ]\n"
            "pub(crate) mod arbitrary_name\n{ fn test_only() {} }\n",
            "#[inline]\n#[cfg(test)]\n#[allow(dead_code)]\n"
            "pub(crate) async fn test_only() { nested({ 1 }); }\n",
            "#[cfg(test)] use crate::fixtures::{First, Second};\n",
            "#[cfg(test)] extern crate fixtures;\n",
            "#[cfg(test)] const TEST_ONLY: usize = { let value = 1; value };\n",
            "#[cfg(test)] static TEST_ONLY: [u8; 1] = [1];\n",
            "#[cfg(test)] type TestOnly = Result<First, Second>;\n",
            "#[cfg(test)] struct TestOnly(usize);\n",
            "#[cfg(test)] struct TestOnly { field: usize }\n",
            "#[cfg(test)] enum TestOnly { First, Second }\n",
            "#[cfg(test)] trait TestOnly { fn check(); }\n",
            "#[cfg(test)] impl TestOnly { fn check() {} }\n",
            '#[cfg(test)] unsafe extern "C" { fn test_only(); }\n',
            "#[cfg(test)] macro_rules! test_only { () => { check(); } }\n",
            "#[cfg(test)] fixtures::test_only! { check(); }\n",
            "#[cfg(test)] fn test_only<const N: usize>() -> Sized<{ N }> {}\n",
        )
        before = "fn production_before() {}\n"
        after = "fn production_after() {}\n"
        for item in items:
            for newline in ("\n", "\r\n"):
                with self.subTest(item=item, newline=newline):
                    text = (before + item + after).replace("\n", newline)
                    expected = (before + self.masked(item) + after).replace("\n", newline)
                    self.assertEqual(self.project(text), expected)

    def test_interleaved_and_nested_test_items_leave_production_in_place(self) -> None:
        first = "#[cfg(test)] mod checks { fn decoy() {} }\n"
        second = "#[cfg(test)] fn another_decoy() {}\n"
        before = "mod shipping {\nfn before() {}\n"
        middle = "fn between() {}\n"
        after = "fn after() {}\n}\nfn outside() {}\n"
        self.assertEqual(
            self.project(before + first + middle + second + after),
            before + self.masked(first) + middle + self.masked(second) + after,
        )

    def test_test_only_fields_do_not_hide_later_fields_or_functions(self) -> None:
        item = "#[cfg(test)] fixture: Result<First, Second>,\n"
        before = "struct Shipping {\n"
        after = "production: usize,\n}\nfn production() {}\n"
        self.assertEqual(
            self.project(before + item + after),
            before + self.masked(item) + after,
        )

    def test_positive_test_predicates_are_excluded_but_shipping_alternatives_remain(
        self,
    ) -> None:
        for predicate in (
            "test",
            'all(test, feature = "fixture")',
            "all(windows, all(test, unix))",
            "any(all(test, windows), test)",
        ):
            with self.subTest(predicate=predicate):
                item = f"#[cfg({predicate})] fn decoy() {{}}\n"
                after = "fn production() {}\n"
                self.assertEqual(self.project(item + after), self.masked(item) + after)
        for predicate in (
            "any(test, windows)",
            'any(test, feature = "test")',
            "not(test)",
            'all(windows, feature = "fixture")',
        ):
            with self.subTest(predicate=predicate):
                source = f"#[cfg({predicate})] fn potentially_shipping() {{}}\n"
                self.assertEqual(self.project(source), source)

    def test_comments_and_literals_cannot_introduce_test_boundaries(self) -> None:
        source = (
            'const RAW: &str = r###"\n#[cfg(test)]\nmod tests {}\n"###;\n'
            'const TEXT: &str = "#[cfg(test)] mod checks {}";\n'
            'const BYTES: &[u8] = br#"#[cfg(test)] mod checks {}"#;\n'
            "// #[cfg(test)] mod checks { unmatched comment brace\n"
            "/* #[cfg(test)] mod checks { /* nested */ */\n"
            "fn production() {}\n"
        )
        self.assertEqual(self.project(source), source)

    def test_crlf_multiline_literals_preserve_offsets(self) -> None:
        source = 'const TEXT: &str = "first\r\nsecond";\r\nfn production() {}\r\n'
        self.assertEqual(self.project(source), source)

    def test_unsafe_lexical_attribute_and_item_boundaries_fail_closed(self) -> None:
        malformed = (
            'const TEXT: &str = "unfinished',
            'const TEXT: &str = "bare\rcarriage return";',
            "/* unfinished comment",
            "#[cfg(test)] mod checks {",
            "#[cfg(test)] fn checks(] {}",
            "#[cfg(test)]",
            "#[cfg(test)] use fixtures::OnlyForTests",
            "#[cfg(test)] unsupported_item",
            "#[cfg()] fn checks() {}",
            "#[cfg(all(test,,windows))] fn checks() {}",
            "#[cfg(not(not(test)))] fn checks() {}",
            "#![cfg(test)] fn checks() {}",
            "#[cfg_attr(test, cfg(test))] fn checks() {}",
        )
        for source in malformed:
            with self.subTest(source=source):
                self.assertIsNone(self.project("fn production() {}\n" + source))


class RustSourceViewsTests(unittest.TestCase):
    def assert_position_preserved(self, source: str, *views: str) -> None:
        newline_offsets = tuple(
            index for index, character in enumerate(source) if character in "\r\n"
        )
        for view in views:
            self.assertEqual(len(source), len(view))
            self.assertEqual(
                newline_offsets,
                tuple(
                    index
                    for index, character in enumerate(view)
                    if character in "\r\n"
                ),
            )

    def test_line_and_nested_block_comments_are_masked(self) -> None:
        source = (
            "fn sample() {\n"
            "    let value = 3; // trailing comment\n"
            "    /* outer comment\n"
            "       /* nested comment */\n"
            "       after inner close\n"
            "    */\n"
            "    keep();\n"
            "}\n"
        )

        views = rust_source_views(source)
        self.assertIsNotNone(views)
        code_only, comment_free = views
        self.assert_position_preserved(source, code_only, comment_free)
        self.assertEqual(code_only, comment_free)
        self.assertIn("let value = 3;", code_only)
        self.assertIn("keep();", code_only)
        for comment_text in (
            "trailing comment",
            "outer comment",
            "nested comment",
            "after inner close",
        ):
            self.assertNotIn(comment_text, code_only)

    def test_line_comment_may_end_at_end_of_source(self) -> None:
        source = "let value = 7; // comment to eof"

        views = rust_source_views(source)
        self.assertIsNotNone(views)
        code_only, comment_free = views
        self.assert_position_preserved(source, code_only, comment_free)
        self.assertIn("let value = 7;", code_only)
        self.assertNotIn("comment to eof", comment_free)

    def test_carriage_return_line_endings_keep_offsets(self) -> None:
        source = "let value = 1; // comment\r\nlet next = 2;"

        views = rust_source_views(source)
        self.assertIsNotNone(views)
        code_only, comment_free = views
        self.assert_position_preserved(source, code_only, comment_free)
        self.assertTrue(code_only.endswith("\r\nlet next = 2;"))
        self.assertTrue(comment_free.endswith("\r\nlet next = 2;"))

    def test_escaped_ordinary_string_hides_comment_markers(self) -> None:
        literal = r'let text = "quote: \" slash: \\ // fake /* still data */";'
        source = literal + " // real comment\nnext();"

        views = rust_source_views(source)
        self.assertIsNotNone(views)
        code_only, comment_free = views
        self.assert_position_preserved(source, code_only, comment_free)
        self.assertTrue(comment_free.startswith(literal))
        self.assertIn("// fake", comment_free)
        self.assertIn("/* still data */", comment_free)
        self.assertNotIn("real comment", comment_free)
        self.assertNotIn("// fake", code_only)
        self.assertNotIn("still data", code_only)
        self.assertIn("next();", code_only)

    def test_unescaped_linefeed_inside_string_preserves_valid_rust_source(self) -> None:
        source = 'let help = "first line\nsecond line";\nuse_help();'

        views = rust_source_views(source)
        self.assertIsNotNone(views)
        code_only, comment_free = views
        self.assert_position_preserved(source, code_only, comment_free)
        self.assertEqual(comment_free, source)
        self.assertNotIn("second line", code_only)
        self.assertIn("use_help();", code_only)

    def test_raw_hash_and_byte_strings_keep_their_full_content(self) -> None:
        source = (
            'let no_hash = r"// raw body /* still raw */"; '
            'let one_hash = r#"contains " quote and // text"#; '
            'let bytes = b"/* byte text */ //"; '
            'let raw_bytes = br##"contains "# and // /* text"##;'
        )

        views = rust_source_views(source)
        self.assertIsNotNone(views)
        code_only, comment_free = views
        self.assert_position_preserved(source, code_only, comment_free)
        self.assertEqual(comment_free, source)
        for literal_text in (
            "// raw body",
            "/* still raw */",
            "contains \" quote and // text",
            "/* byte text */ //",
            'contains "# and // /* text',
        ):
            self.assertNotIn(literal_text, code_only)

    def test_raw_terminator_requires_the_opening_hash_count(self) -> None:
        source = 'let value = r##"body "# and // remains"##;'

        views = rust_source_views(source)
        self.assertIsNotNone(views)
        code_only, comment_free = views
        self.assert_position_preserved(source, code_only, comment_free)
        self.assertEqual(comment_free, source)
        self.assertNotIn("// remains", code_only)

    def test_character_literals_are_masked_but_lifetimes_remain_code(self) -> None:
        source = r"""fn f<'a>(x: &'a str, y: &'static str) {
    let brace = '{';
    let apostrophe = '\'';
    let double_quote = '"';
}"""

        views = rust_source_views(source)
        self.assertIsNotNone(views)
        code_only, comment_free = views
        self.assert_position_preserved(source, code_only, comment_free)
        self.assertEqual(comment_free, source)
        self.assertIn("fn f<'a>", code_only)
        self.assertIn("x: &'a str", code_only)
        self.assertIn("&'static str", code_only)
        self.assertIn("let brace =    ;", code_only)
        self.assertIn("let apostrophe =     ;", code_only)
        self.assertIn("let double_quote =    ;", code_only)

    def test_malformed_or_ambiguous_lexical_constructs_fail_closed(self) -> None:
        malformed_sources = (
            ("unclosed nested block comment", "/* outer /* nested */"),
            ("unclosed ordinary string", 'let value = "unfinished'),
            ("escaped quote without terminator", r'let value = "escaped \"'),
            (
                "raw string with too few closing hashes",
                'let value = r##"content "#;',
            ),
            (
                "raw byte string with too few closing hashes",
                'let value = br###"content"##;',
            ),
            ("unclosed character literal", "let value = '{;"),
            ("invalid string escape", r'let value = "bad \q";'),
            ("invalid character escape", r"let value = '\q';"),
        )

        for name, source in malformed_sources:
            with self.subTest(name=name):
                self.assertIsNone(rust_source_views(source))


if __name__ == "__main__":
    unittest.main()
