"""Source loading and lexical views for fail-closed Rust source guards.

This module recognizes lexical and cfg-gated item boundaries. It does not parse
full Rust syntax or validate control flow.
"""

from __future__ import annotations

import re
from pathlib import Path

_SIMPLE_ESCAPES = frozenset({"\\", "'", '"', "n", "r", "t", "0"})


def _cfg_requires_test(predicate: list[str]) -> bool | None:
    """Recognize positive test requirements without evaluating platform flags."""
    if predicate == ["test"]:
        return True
    if predicate == ["not", "(", "test", ")"]:
        return False
    if not predicate:
        return None
    if predicate[0] in {"all", "any"}:
        if len(predicate) < 3 or predicate[1] != "(" or predicate[-1] != ")":
            return None
        arguments: list[bool] = []
        depth = 0
        start = 2
        for index in range(2, len(predicate) - 1):
            token = predicate[index]
            if token == "(":
                depth += 1
            elif token == ")":
                depth -= 1
                if depth < 0:
                    return None
            elif token == "," and depth == 0:
                required = _cfg_requires_test(predicate[start:index])
                if required is None:
                    return None
                arguments.append(required)
                start = index + 1
        if depth:
            return None
        if start < len(predicate) - 1:
            required = _cfg_requires_test(predicate[start:-1])
            if required is None:
                return None
            arguments.append(required)
        if predicate[0] == "all":
            return any(arguments)
        return bool(arguments) and all(arguments)
    if "test" in predicate:
        return None
    return False


def _cfg_item_end(
    tokens: list[str], closing: dict[int, int], start: int
) -> int | None:
    """Find an annotated item's boundary using balanced token trees."""
    cursor = start
    while cursor < len(tokens):
        token = tokens[cursor]
        if token in {"pub", "unsafe", "async", "default", "auto"}:
            cursor += 1
            if token == "pub" and cursor < len(tokens) and tokens[cursor] == "(":
                cursor = closing[cursor] + 1
        elif (
            token in {"const", "extern"}
            and cursor + 1 < len(tokens)
            and tokens[cursor + 1] in {"fn", "unsafe", "async", "extern", "static"}
        ):
            cursor += 1
        else:
            break
    if cursor >= len(tokens):
        return None

    head = tokens[cursor]
    if cursor + 1 < len(tokens) and tokens[cursor + 1] == ":":
        boundary = "field"
    elif head in {"use", "const", "static", "type", "let"} or (
        head == "extern" and tokens[cursor + 1 : cursor + 2] == ["crate"]
    ):
        boundary = "semicolon"
    elif head in {
        "fn", "mod", "struct", "enum", "union", "trait", "impl",
        "extern", "macro_rules", "macro", "{",
    }:
        boundary = "body"
    else:
        bang = cursor + 1
        while bang + 1 < len(tokens) and tokens[bang] == "::":
            bang += 2
        if (
            bang + 1 >= len(tokens)
            or tokens[bang] != "!"
            or tokens[bang + 1] not in {"(", "[", "{"}
        ):
            return None
        end = closing[bang + 1] + 1
        if end < len(tokens) and tokens[end] == ";":
            return end + 1
        return end if tokens[bang + 1] == "{" else None

    angles = 0
    while cursor < len(tokens):
        token = tokens[cursor]
        if token in {"(", "["}:
            cursor = closing[cursor] + 1
            continue
        if token == "{":
            if boundary == "body" and angles == 0:
                return closing[cursor] + 1
            cursor = closing[cursor] + 1
            continue
        if token in {")", "]", "}"}:
            if token == "}" and boundary == "field" and angles == 0:
                return cursor
            return None
        if token == ";":
            return cursor + 1 if angles == 0 and boundary != "field" else None
        if token == "," and angles == 0 and boundary == "field":
            return cursor + 1
        if boundary != "semicolon":
            if token == "<":
                angles += 1
            elif token == ">" and angles:
                angles -= 1
        cursor += 1
    return None


def production_source(path: Path) -> str | None:
    """Mask test-only items, preserving offsets and newlines; fail closed."""
    with path.open(encoding="utf-8", newline="") as handle:
        source = handle.read()
    views = rust_source_views(source)
    if views is None:
        return None
    code_only, _ = views
    matches = list(re.finditer(r"(?:r#)?[^\W\d]\w*|::|->|[^\s]", code_only))
    tokens = [match.group().removeprefix("r#") for match in matches]
    closing: dict[int, int] = {}
    stack: list[int] = []
    openers = {")": "(", "]": "[", "}": "{"}
    for index, token in enumerate(tokens):
        if token in {"(", "[", "{"}:
            stack.append(index)
        elif token in openers:
            if not stack or tokens[stack[-1]] != openers[token]:
                return None
            closing[stack.pop()] = index
    if stack:
        return None

    production = list(source)
    index = 0
    while index < len(tokens):
        if tokens[index] != "#":
            index += 1
            continue
        start = index
        test_only = False
        while index < len(tokens) and tokens[index] == "#":
            opening = index + 1
            inner = opening < len(tokens) and tokens[opening] == "!"
            if inner:
                if index != start:
                    return None
                opening += 1
            if opening >= len(tokens) or tokens[opening] != "[":
                return None
            end = closing[opening]
            attribute = tokens[opening + 1 : end]
            if attribute and attribute[0] == "cfg":
                if (
                    len(attribute) < 3
                    or attribute[1] != "("
                    or closing.get(opening + 2) != end - 1
                ):
                    return None
                try:
                    required = _cfg_requires_test(attribute[2:-1])
                except RecursionError:
                    return None
                if required is None:
                    return None
                test_only |= required
            elif attribute and attribute[0] == "cfg_attr" and "test" in attribute:
                return None
            index = end + 1
            if inner:
                if test_only:
                    return None
                break
        if not test_only:
            continue
        end = _cfg_item_end(tokens, closing, index)
        if end is None or end <= index:
            return None
        _mask(production, source, matches[start].start(), matches[end - 1].end())
        index = end
    return "".join(production)


def _is_identifier_start(character: str) -> bool:
    return character == "_" or character.isidentifier()


def _is_identifier_continue(character: str) -> bool:
    return ("a" + character).isidentifier()


def _identifier_continues_before(source: str, index: int) -> bool:
    return index > 0 and _is_identifier_continue(source[index - 1])


def _hex_value(character: str) -> int | None:
    if "0" <= character <= "9":
        return ord(character) - ord("0")
    if "a" <= character <= "f":
        return ord(character) - ord("a") + 10
    if "A" <= character <= "F":
        return ord(character) - ord("A") + 10
    return None


def _escape_end(
    source: str,
    slash_index: int,
    *,
    byte: bool,
    allow_line_continuation: bool,
) -> int | None:
    """Return the position after a valid Rust escape, or None."""
    escaped_index = slash_index + 1
    if escaped_index >= len(source):
        return None

    escaped = source[escaped_index]
    if escaped in _SIMPLE_ESCAPES:
        return escaped_index + 1
    if allow_line_continuation and escaped == "\n":
        return escaped_index + 1
    if (
        allow_line_continuation
        and escaped == "\r"
        and escaped_index + 1 < len(source)
        and source[escaped_index + 1] == "\n"
    ):
        return escaped_index + 2

    if escaped == "x":
        first_index = escaped_index + 1
        second_index = escaped_index + 2
        if second_index >= len(source):
            return None
        first = _hex_value(source[first_index])
        second = _hex_value(source[second_index])
        if first is None or second is None:
            return None
        value = first * 16 + second
        if not byte and value > 0x7F:
            return None
        return second_index + 1

    if not byte and escaped == "u":
        brace_index = escaped_index + 1
        if brace_index >= len(source) or source[brace_index] != "{":
            return None

        cursor = brace_index + 1
        digits = 0
        value = 0
        while cursor < len(source) and source[cursor] != "}":
            character = source[cursor]
            if character == "_":
                cursor += 1
                continue
            digit = _hex_value(character)
            if digit is None:
                return None
            digits += 1
            if digits > 6:
                return None
            value = value * 16 + digit
            cursor += 1

        if cursor >= len(source) or digits == 0:
            return None
        if value > 0x10FFFF or 0xD800 <= value <= 0xDFFF:
            return None
        return cursor + 1

    return None


def _ordinary_string_end(source: str, quote_index: int, *, byte: bool) -> int | None:
    cursor = quote_index + 1
    while cursor < len(source):
        character = source[cursor]
        if character == '"':
            return cursor + 1
        if character == "\\":
            cursor = _escape_end(
                source,
                cursor,
                byte=byte,
                allow_line_continuation=True,
            )
            if cursor is None:
                return None
            continue
        if character == "\r":
            if cursor + 1 >= len(source) or source[cursor + 1] != "\n":
                return None
            cursor += 2
            continue
        codepoint = ord(character)
        if 0xD800 <= codepoint <= 0xDFFF:
            return None
        cursor += 1
    return None


def _raw_string_opening(source: str, r_index: int) -> tuple[int, int] | None:
    cursor = r_index + 1
    while cursor < len(source) and source[cursor] == "#":
        cursor += 1
    if cursor < len(source) and source[cursor] == '"':
        return cursor, cursor - r_index - 1
    return None


def _raw_string_end(source: str, quote_index: int, hash_count: int) -> int | None:
    closing = '"' + ("#" * hash_count)
    close_index = source.find(closing, quote_index + 1)
    if close_index < 0:
        return None
    return close_index + len(closing)


def _character_literal_end(source: str, quote_index: int, *, byte: bool) -> int | None:
    cursor = quote_index + 1
    if cursor >= len(source):
        return None

    character = source[cursor]
    if character == "\\":
        cursor = _escape_end(
            source,
            cursor,
            byte=byte,
            allow_line_continuation=False,
        )
        if cursor is None:
            return None
    else:
        if character in "\r\n'":
            return None
        codepoint = ord(character)
        if 0xD800 <= codepoint <= 0xDFFF or (byte and codepoint > 0x7F):
            return None
        cursor += 1

    if cursor < len(source) and source[cursor] == "'":
        return cursor + 1
    return None


def _lifetime_end(source: str, quote_index: int) -> int | None:
    cursor = quote_index + 1
    if cursor >= len(source) or not _is_identifier_start(source[cursor]):
        return None
    cursor += 1
    while cursor < len(source) and _is_identifier_continue(source[cursor]):
        cursor += 1
    return cursor


def _mask(chars: list[str], source: str, start: int, end: int) -> None:
    for index in range(start, end):
        if source[index] not in "\r\n":
            chars[index] = " "


def rust_source_views(source: str) -> tuple[str, str] | None:
    """Return code-only and comment-free source views, or None if unsafe.

    Both views preserve every character offset and line break. ``code_only``
    masks comments and literals; ``comment_free`` masks comments only. A
    malformed or ambiguous comment, string, or apostrophe construct fails
    closed instead of returning a potentially misleading view.
    """
    code_only = list(source)
    comment_free = list(source)
    length = len(source)
    index = 0

    while index < length:
        character = source[index]
        if character == "/" and index + 1 < length:
            following = source[index + 1]
            if following == "/":
                end = index + 2
                while end < length and source[end] not in "\r\n":
                    end += 1
                _mask(code_only, source, index, end)
                _mask(comment_free, source, index, end)
                index = end
                continue

            if following == "*":
                depth = 1
                end = index + 2
                while end < length and depth:
                    if (
                        source[end] == "/"
                        and end + 1 < length
                        and source[end + 1] == "*"
                    ):
                        depth += 1
                        end += 2
                    elif (
                        source[end] == "*"
                        and end + 1 < length
                        and source[end + 1] == "/"
                    ):
                        depth -= 1
                        end += 2
                    else:
                        end += 1
                if depth:
                    return None
                _mask(code_only, source, index, end)
                _mask(comment_free, source, index, end)
                index = end
                continue

        if character in "bc" and not _identifier_continues_before(source, index):
            if character == "b" and index + 1 < length and source[index + 1] == "'":
                byte_char_end = _character_literal_end(source, index + 1, byte=True)
                if byte_char_end is not None:
                    _mask(code_only, source, index, byte_char_end)
                    index = byte_char_end
                    continue
                if _character_literal_end(source, index + 1, byte=False) is not None:
                    return None

            if index + 1 < length and source[index + 1] == "r":
                opening = _raw_string_opening(source, index + 1)
                if opening is not None:
                    quote_index, hash_count = opening
                    end = _raw_string_end(source, quote_index, hash_count)
                    if end is None:
                        return None
                    _mask(code_only, source, index, end)
                    index = end
                    continue

            if index + 1 < length and source[index + 1] == '"':
                end = _ordinary_string_end(
                    source,
                    index + 1,
                    byte=character == "b",
                )
                if end is None:
                    return None
                _mask(code_only, source, index, end)
                index = end
                continue

        if character == "r" and not _identifier_continues_before(source, index):
            opening = _raw_string_opening(source, index)
            if opening is not None:
                quote_index, hash_count = opening
                end = _raw_string_end(source, quote_index, hash_count)
                if end is None:
                    return None
                _mask(code_only, source, index, end)
                index = end
                continue

        if character == '"':
            end = _ordinary_string_end(source, index, byte=False)
            if end is None:
                return None
            _mask(code_only, source, index, end)
            index = end
            continue

        if character == "'":
            char_end = _character_literal_end(source, index, byte=False)
            if char_end is not None:
                _mask(code_only, source, index, char_end)
                index = char_end
                continue
            lifetime_end = _lifetime_end(source, index)
            if lifetime_end is None:
                return None
            index = lifetime_end
            continue

        index += 1

    return "".join(code_only), "".join(comment_free)
