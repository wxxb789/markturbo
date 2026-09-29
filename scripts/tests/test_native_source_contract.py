"""Focused tests for the native harness Rust source lexical views."""

from __future__ import annotations

import unittest

from scripts.markturbo_tools.native.source_contract import rust_source_views


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
