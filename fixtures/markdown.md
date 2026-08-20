---
title: Markdown feature sweep
tags: [fixture, markdown]
---

# Heading 1

Ordinary paragraph with **bold**, *italic*, ~~strikethrough~~, `inline code`,
and a [link](https://example.com/path?a=1&b=2).

## Heading 2 with `code`

### Heading 3

Paragraph followed by a hard break.
Second line of the same paragraph.

## Lists

- First bullet
- Second bullet
  - Nested one
  - Nested two
    - Deeply nested
- Third bullet

1. Ordered first
2. Ordered second
   1. Nested ordered
3. Ordered third

- [ ] Unchecked task
- [x] Checked task

## Blockquotes

> A single-level quote.
>
> > A nested quote, which some parsers get wrong.

## Table

| Left | Center | Right | Notes |
|:-----|:------:|------:|-------|
| a    | b      | c     | plain |
| `x`  | **y**  | [z](https://example.com) | mixed inline |
| 中文 | 日本語 | 한국어 | CJK columns |

## Fenced code

```rust
fn main() {
    let value = String::new();
    println!("{value}");
}
```

```
Fence with no language.
```

    Indented code block.

## Images

![Alt text](https://example.com/image.png "Title")

## Unicode and CJK

Emoji: 🎉 👩‍💻 🇯🇵 (ZWJ sequences and flags)

中文段落：这是一段包含 `代码`、**粗体** 和 [链接](https://例え.jp/道) 的文字。

日本語の段落。ひらがな、カタカナ、漢字が混在します。

Right-to-left: مرحبا بالعالم

Combining marks: é (e + U+0301) vs é (U+00E9)

## Horizontal rules

---

***

## HTML

<div align="center">
  <strong>Raw HTML block</strong>
</div>

Inline <kbd>Ctrl</kbd>+<kbd>S</kbd> HTML.

## Autolinks and footnotes

See <https://example.com> and <mailto:someone@example.com>.

A footnote reference[^1].

[^1]: The footnote body.

## Edge cases

Escaped characters: \*not italic\*, \# not a heading, \` not code.

Empty link text: [](https://example.com)

A line ending in two spaces for a hard break.  
Next line.
