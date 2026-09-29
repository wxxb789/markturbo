"""Position-preserving lexical views for fail-closed Rust source guards.

This module recognizes comments and literal boundaries only. It does not parse
Rust syntax or validate control flow.
"""

from __future__ import annotations

_SIMPLE_ESCAPES = frozenset({"\\", "'", '"', "n", "r", "t", "0"})


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
            return None
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
