//! Document-aware translation.
//!
//! Translating raw Markdown through an LLM corrupts structure: code gets
//! "helpfully" rewritten, link targets get localized, YAML keys get translated.
//! So this module splits a document into translatable prose segments and
//! verbatim segments, sends only the former to a [`TranslationService`], and
//! reassembles by span. The provider never sees the document as one string.

use std::ops::Range;

use crate::block::{Block, BlockKind};
use crate::doc::Document;

/// A slice of the document, classified for translation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Segment {
    pub range: Range<usize>,
    pub text: String,
    /// False for code, URLs, diagram source, math, frontmatter keys, …
    pub translatable: bool,
}

/// What part of a document to translate.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Scope {
    /// An explicit byte range, e.g. the editor selection.
    Selection(Range<usize>),
    /// The block containing this offset.
    Block(usize),
    /// Everything.
    Document,
}

/// A translation backend. Kept as a trait so the document model never names a
/// vendor; a provider is chosen at the application edge.
pub trait TranslationService: Send + Sync {
    /// Translate each string into `target_lang`, preserving order and length of
    /// the slice. Implementations must return exactly `texts.len()` items.
    fn translate(&self, texts: &[String], target_lang: &str) -> anyhow::Result<Vec<String>>;
}

/// The result of a translation pass: the rewritten document text plus what was
/// left untouched, so a caller can show `Original | Translation` without
/// re-deriving the split.
#[derive(Debug, Clone)]
pub struct Translation {
    pub text: String,
    pub segments: Vec<Segment>,
}

/// Resolve a scope to a byte range in `doc`.
pub fn resolve_scope(doc: &Document, scope: &Scope) -> Range<usize> {
    match scope {
        Scope::Selection(range) => clamp(range.clone(), doc.source().len()),
        Scope::Block(offset) => doc
            .block_at(*offset)
            .map(|b| b.range.clone())
            .unwrap_or(0..doc.source().len()),
        Scope::Document => 0..doc.source().len(),
    }
}

/// Split the requested scope into translatable and verbatim segments.
///
/// Segments tile the range exactly: concatenating every `text` in order
/// reproduces the source slice byte-for-byte. That invariant is what makes
/// reassembly lossless.
pub fn segment(doc: &Document, scope: &Scope) -> Vec<Segment> {
    let range = resolve_scope(doc, scope);
    let source = doc.source();
    let mut segments = Vec::new();

    // Blocks that overlap the scope, in order. Anything between them (blank
    // lines, frontmatter) is emitted verbatim so the tiling stays exact.
    let mut cursor = range.start;
    for block in doc.blocks() {
        if block.range.end <= range.start {
            continue;
        }
        if block.range.start >= range.end {
            break;
        }
        let start = block.range.start.max(range.start);
        let end = block.range.end.min(range.end);
        if start > cursor {
            push_verbatim(&mut segments, source, cursor..start);
        }
        segment_block(block, source, start..end, &mut segments);
        cursor = end;
    }
    if cursor < range.end {
        push_verbatim(&mut segments, source, cursor..range.end);
    }

    segments.retain(|s| !s.text.is_empty());
    segments
}

/// Split one block. Whole-block verbatim for code/math/diagrams/MDX/HTML;
/// inline-aware for prose.
fn segment_block(block: &Block, source: &str, range: Range<usize>, out: &mut Vec<Segment>) {
    let verbatim_block = matches!(
        block.kind,
        BlockKind::Code { .. } | BlockKind::Math | BlockKind::Diagram(_) | BlockKind::Html | BlockKind::Mdx(_)
    );
    if verbatim_block {
        push_verbatim(out, source, range);
        return;
    }
    segment_prose(source, range, out);
}

/// Within prose, hold inline code, autolinks, link *targets*, and math verbatim
/// while letting the surrounding words through.
///
/// This is a scanner rather than an AST walk on purpose: it operates on the raw
/// slice, so the reassembled output preserves the author's exact markup
/// (indentation, list markers, emphasis characters) instead of re-serializing
/// an AST.
fn segment_prose(source: &str, range: Range<usize>, out: &mut Vec<Segment>) {
    let text = &source[range.clone()];
    let bytes = text.as_bytes();
    let mut i = 0;
    let mut prose_start = 0;

    /// Emit `[prose_start, end)` as a translatable segment.
    fn flush_prose(
        out: &mut Vec<Segment>,
        source: &str,
        base: usize,
        prose_start: usize,
        end: usize,
    ) {
        if end > prose_start {
            let abs = base + prose_start..base + end;
            out.push(Segment {
                text: source[abs.clone()].to_string(),
                range: abs,
                translatable: true,
            });
        }
    }
    let base = range.start;

    while i < bytes.len() {
        // Line-leading block markup: `#`, `-`, `*`, `>`, `1.`, and the
        // indentation before it. A translator handed `# Title` may drop or
        // move the `#`; handing it only `Title` cannot break the document.
        if i == 0 || bytes[i - 1] == b'\n' {
            let marker_len = leading_marker(&text[i..]);
            if marker_len > 0 {
                flush_prose(out, source, base, prose_start, i);
                push_verbatim(out, source, base + i..base + i + marker_len);
                i += marker_len;
                prose_start = i;
                continue;
            }
        }

        // Table cell delimiter. Keeping the pipes verbatim splits each cell
        // into its own prose segment, so a translator cannot merge columns.
        if bytes[i] == b'|' {
            flush_prose(out, source, base, prose_start, i);
            push_verbatim(out, source, base + i..base + i + 1);
            i += 1;
            prose_start = i;
            continue;
        }

        // Inline code: `code` / ``co`de``. Match the opening run length.
        if bytes[i] == b'`' {
            let tick_len = bytes[i..].iter().take_while(|&&b| b == b'`').count();
            let fence = "`".repeat(tick_len);
            if let Some(rel_end) = text[i + tick_len..].find(&fence) {
                let end = i + tick_len + rel_end + tick_len;
                flush_prose(out, source, base, prose_start, i);
                push_verbatim(out, source, base + i..base + end);
                i = end;
                prose_start = i;
                continue;
            }
        }

        // Inline math: $…$ (single-line only, matching CommonMark math ext).
        if bytes[i] == b'$' && i + 1 < bytes.len() && bytes[i + 1] != b' ' {
            if let Some(rel_end) = text[i + 1..].find('$') {
                let candidate = &text[i + 1..i + 1 + rel_end];
                if !candidate.contains('\n') && !candidate.is_empty() {
                    let end = i + 1 + rel_end + 1;
                    flush_prose(out, source, base, prose_start, i);
                    push_verbatim(out, source, base + i..base + end);
                    i = end;
                    prose_start = i;
                    continue;
                }
            }
        }

        // Link/image target: `](target)` — the label before it stays
        // translatable, the target does not.
        if bytes[i] == b']' && text[i + 1..].starts_with('(') {
            if let Some(rel_end) = text[i + 1..].find(')') {
                let end = i + 1 + rel_end + 1;
                flush_prose(out, source, base, prose_start, i + 1);
                push_verbatim(out, source, base + i + 1..base + end);
                i = end;
                prose_start = i;
                continue;
            }
        }

        // Autolink: <https://…> or <mailto:…>.
        if bytes[i] == b'<' {
            if let Some(rel_end) = text[i..].find('>') {
                let inner = &text[i + 1..i + rel_end];
                if is_uri_like(inner) {
                    let end = i + rel_end + 1;
                    flush_prose(out, source, base, prose_start, i);
                    push_verbatim(out, source, base + i..base + end);
                    i = end;
                    prose_start = i;
                    continue;
                }
            }
        }

        // Advance by one char, never one byte: CJK and emoji must not split.
        i += char_len(bytes[i]);
    }
    flush_prose(out, source, base, prose_start, bytes.len());
}

fn is_uri_like(s: &str) -> bool {
    s.contains("://") || s.starts_with("mailto:") || (s.contains('@') && !s.contains(' '))
}

/// Length of the block-level markup at the start of a line, including the
/// indentation and the trailing space, or 0 when the line starts with prose.
///
/// Covers ATX headings, list bullets, ordered-list numbers, blockquote markers,
/// and task-list checkboxes. Table delimiter rows (`|---|`) are handled by the
/// pipe rule plus this function's hyphen-run case.
fn leading_marker(line: &str) -> usize {
    let indent = line.len() - line.trim_start_matches([' ', '\t']).len();
    let rest = &line[indent..];
    let bytes = rest.as_bytes();
    if bytes.is_empty() {
        return 0;
    }

    let marker = match bytes[0] {
        // ATX heading: one to six `#` followed by a space.
        b'#' => {
            let hashes = bytes.iter().take_while(|&&b| b == b'#').count();
            if (1..=6).contains(&hashes) && bytes.get(hashes) == Some(&b' ') {
                hashes + 1
            } else {
                0
            }
        }
        // Bullet or blockquote: the character plus a space.
        b'-' | b'*' | b'+' | b'>' => {
            // A run of three or more is a thematic break or a table delimiter
            // row; treat the whole run as markup.
            let run = bytes.iter().take_while(|&&b| b == bytes[0]).count();
            if run >= 3 {
                run
            } else if bytes.get(1) == Some(&b' ') {
                2
            } else {
                0
            }
        }
        // Ordered list: digits then `.` or `)` then a space.
        b'0'..=b'9' => {
            let digits = bytes.iter().take_while(|b| b.is_ascii_digit()).count();
            if matches!(bytes.get(digits), Some(b'.') | Some(b')'))
                && bytes.get(digits + 1) == Some(&b' ')
            {
                digits + 2
            } else {
                0
            }
        }
        _ => 0,
    };

    if marker == 0 {
        return 0;
    }

    // Task-list checkbox directly after a bullet: `- [ ] ` / `- [x] `.
    let after = &rest[marker..];
    let checkbox = if after.starts_with("[ ] ")
        || after.starts_with("[x] ")
        || after.starts_with("[X] ")
    {
        4
    } else {
        0
    };

    indent + marker + checkbox
}

/// UTF-8 length from the leading byte.
fn char_len(b: u8) -> usize {
    match b {
        0x00..=0x7f => 1,
        0xc0..=0xdf => 2,
        0xe0..=0xef => 3,
        _ => 4,
    }
}

fn push_verbatim(out: &mut Vec<Segment>, source: &str, range: Range<usize>) {
    if range.is_empty() {
        return;
    }
    out.push(Segment {
        text: source[range.clone()].to_string(),
        range,
        translatable: false,
    });
}

fn clamp(range: Range<usize>, len: usize) -> Range<usize> {
    let start = range.start.min(len);
    let end = range.end.clamp(start, len);
    start..end
}

/// Translate `scope` of `doc` via `service`, preserving everything that must
/// not change.
pub fn translate(
    doc: &Document,
    scope: &Scope,
    target_lang: &str,
    service: &dyn TranslationService,
) -> anyhow::Result<Translation> {
    let mut segments = segment(doc, scope);

    // Only send segments with actual words. Whitespace-only prose (blank lines
    // between paragraphs) would waste a round-trip and risk being "corrected".
    let indices: Vec<usize> = segments
        .iter()
        .enumerate()
        .filter(|(_, s)| s.translatable && s.text.chars().any(|c| c.is_alphanumeric()))
        .map(|(i, _)| i)
        .collect();

    if !indices.is_empty() {
        let inputs: Vec<String> = indices.iter().map(|&i| segments[i].text.clone()).collect();
        let outputs = service.translate(&inputs, target_lang)?;
        anyhow::ensure!(
            outputs.len() == inputs.len(),
            "translation service returned {} results for {} inputs",
            outputs.len(),
            inputs.len()
        );
        for (&i, out) in indices.iter().zip(outputs) {
            // Preserve the segment's leading/trailing whitespace: it carries
            // Markdown structure (indentation, line breaks) that a translator
            // has no reason to reproduce.
            let original = &segments[i].text;
            let lead: String = original.chars().take_while(|c| c.is_whitespace()).collect();
            let trail: String = original
                .chars()
                .rev()
                .take_while(|c| c.is_whitespace())
                .collect::<Vec<_>>()
                .into_iter()
                .rev()
                .collect();
            segments[i].text = format!("{lead}{}{trail}", out.trim());
        }
    }

    let range = resolve_scope(doc, scope);
    let mut text = String::with_capacity(doc.source().len());
    text.push_str(&doc.source()[..range.start]);
    for segment in &segments {
        text.push_str(&segment.text);
    }
    text.push_str(&doc.source()[range.end..]);

    Ok(Translation { text, segments })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::doctype::DocType;

    /// Marks translated prose unmistakably, so a test can assert exactly what
    /// reached the provider and what did not.
    struct Upper;
    impl TranslationService for Upper {
        fn translate(&self, texts: &[String], _: &str) -> anyhow::Result<Vec<String>> {
            Ok(texts.iter().map(|t| t.to_uppercase()).collect())
        }
    }

    struct Recording(std::sync::Mutex<Vec<String>>);
    impl TranslationService for Recording {
        fn translate(&self, texts: &[String], _: &str) -> anyhow::Result<Vec<String>> {
            self.0.lock().unwrap().extend(texts.iter().cloned());
            Ok(texts.to_vec())
        }
    }

    fn doc(src: &str) -> Document {
        Document::with_type(DocType::Markdown, src.to_string())
    }

    /// The invariant everything else rests on.
    fn assert_tiles(d: &Document, scope: &Scope) {
        let range = resolve_scope(d, scope);
        let joined: String = segment(d, scope).iter().map(|s| s.text.as_str()).collect();
        assert_eq!(joined, &d.source()[range], "segments must tile the scope");
    }

    #[test]
    fn segments_tile_the_document_exactly() {
        let src = "# Title\n\nSome *prose* with `code` and [a link](https://example.com).\n\n```rust\nlet value = String::new();\n```\n\n> quote\n\n| a | b |\n|---|---|\n| 1 | 2 |\n";
        assert_tiles(&doc(src), &Scope::Document);
    }

    #[test]
    fn fenced_code_is_never_translated() {
        let src = "Intro text.\n\n```rust\nlet value = String::new();\n```\n";
        let out = translate(&doc(src), &Scope::Document, "zh", &Upper).unwrap();
        assert!(out.text.contains("let value = String::new();"));
        assert!(out.text.contains("INTRO TEXT."));
    }

    #[test]
    fn diagram_and_math_source_are_never_translated() {
        let src = "Before.\n\n```mermaid\ngraph TD;\nA[Start]-->B[End];\n```\n\n$$\n\\frac{a}{b}\n$$\n";
        let out = translate(&doc(src), &Scope::Document, "zh", &Upper).unwrap();
        assert!(out.text.contains("A[Start]-->B[End];"));
        assert!(out.text.contains("\\frac{a}{b}"));
        assert!(out.text.contains("BEFORE."));
    }

    #[test]
    fn inline_code_urls_and_link_targets_survive() {
        let src = "Use `String::new()` here, see [docs](https://example.com/a_b) and <https://x.dev>.\n";
        let out = translate(&doc(src), &Scope::Document, "zh", &Upper).unwrap();
        assert!(out.text.contains("`String::new()`"), "got {}", out.text);
        assert!(out.text.contains("(https://example.com/a_b)"));
        assert!(out.text.contains("<https://x.dev>"));
        assert!(out.text.contains("DOCS"), "link label is prose");
    }

    #[test]
    fn inline_math_survives() {
        let src = "The value $x_i^2$ grows.\n";
        let out = translate(&doc(src), &Scope::Document, "zh", &Upper).unwrap();
        assert!(out.text.contains("$x_i^2$"), "got {}", out.text);
    }

    #[test]
    fn frontmatter_keys_are_not_translated() {
        let src = "---\nname: demo\ndescription: A demo skill.\n---\n\nBody text.\n";
        let out = translate(&doc(src), &Scope::Document, "zh", &Upper).unwrap();
        assert!(out.text.starts_with("---\nname: demo\n"), "got {}", out.text);
        assert!(out.text.contains("BODY TEXT."));
    }

    #[test]
    fn selection_scope_leaves_the_rest_byte_identical() {
        let src = "First paragraph.\n\nSecond paragraph.\n";
        let d = doc(src);
        let start = src.find("Second").unwrap();
        let scope = Scope::Selection(start..start + "Second paragraph.".len());
        let out = translate(&d, &scope, "zh", &Upper).unwrap();
        assert_eq!(out.text, "First paragraph.\n\nSECOND PARAGRAPH.\n");
    }

    #[test]
    fn block_scope_translates_only_the_containing_block() {
        let src = "Alpha.\n\nBeta.\n\nGamma.\n";
        let d = doc(src);
        let offset = src.find("Beta").unwrap();
        let out = translate(&d, &Scope::Block(offset), "zh", &Upper).unwrap();
        assert_eq!(out.text, "Alpha.\n\nBETA.\n\nGamma.\n");
    }

    #[test]
    fn only_word_bearing_prose_reaches_the_provider() {
        let src = "# Title\n\n```rust\nfn main() {}\n```\n\nText.\n";
        let rec = Recording(Default::default());
        translate(&doc(src), &Scope::Document, "zh", &rec).unwrap();
        let sent = rec.0.into_inner().unwrap();
        assert!(sent.iter().all(|s| !s.contains("fn main")), "sent {sent:?}");
        assert!(sent.iter().any(|s| s.contains("Title")));
        assert!(sent.iter().any(|s| s.contains("Text.")));
    }

    #[test]
    fn cjk_prose_round_trips() {
        let src = "这是一段中文，包含 `代码` 与[链接](https://例え.jp)。\n";
        let d = doc(src);
        assert_tiles(&d, &Scope::Document);
        let out = translate(&d, &Scope::Document, "en", &Upper).unwrap();
        assert!(out.text.contains("`代码`"));
        assert!(out.text.contains("(https://例え.jp)"));
    }

    #[test]
    fn structure_whitespace_is_preserved_around_translations() {
        // A list item's `- ` marker and indentation must survive even if the
        // provider trims.
        struct Trimming;
        impl TranslationService for Trimming {
            fn translate(&self, texts: &[String], _: &str) -> anyhow::Result<Vec<String>> {
                Ok(texts.iter().map(|t| format!("  {}  ", t.trim())).collect())
            }
        }
        let src = "- item one\n- item two\n";
        let out = translate(&doc(src), &Scope::Document, "zh", &Trimming).unwrap();
        assert_eq!(out.text, src, "no net change when the text is unchanged");
    }

    #[test]
    fn block_markup_is_never_sent_to_the_provider() {
        // A translator handed `# Title` may move or drop the `#`. It must only
        // ever see the words.
        let src = "# Heading\n\n- bullet item\n- [ ] task item\n\n1. first\n\n> quoted line\n\n### Deeper\n";
        let rec = Recording(Default::default());
        translate(&doc(src), &Scope::Document, "zh", &rec).unwrap();
        let sent = rec.0.into_inner().unwrap();
        for text in &sent {
            let t = text.trim();
            assert!(
                !t.starts_with('#')
                    && !t.starts_with("- ")
                    && !t.starts_with("> ")
                    && !t.starts_with("1."),
                "markup leaked to the provider: {text:?}"
            );
        }
        assert!(sent.iter().any(|s| s.trim() == "Heading"), "got {sent:?}");
        assert!(sent.iter().any(|s| s.trim() == "task item"), "got {sent:?}");
    }

    #[test]
    fn markup_survives_a_provider_that_rewrites_everything() {
        // The strongest form of the guarantee: even a provider that returns
        // unrelated text must not be able to damage the document's structure.
        struct Replacing;
        impl TranslationService for Replacing {
            fn translate(&self, texts: &[String], _: &str) -> anyhow::Result<Vec<String>> {
                Ok(texts.iter().map(|_| "X".to_string()).collect())
            }
        }
        let src = "# Heading\n\n- one\n- two\n\n> quote\n\n| a | b |\n|---|---|\n| 1 | 2 |\n";
        let out = translate(&doc(src), &Scope::Document, "zh", &Replacing).unwrap();
        for marker in ["# ", "- ", "> ", "|---|"] {
            assert!(
                out.text.contains(marker),
                "{marker:?} lost from:\n{}",
                out.text
            );
        }
    }

    #[test]
    fn table_cells_stay_in_their_columns() {
        let src = "| Name | Description |\n|---|---|\n| alpha | first thing |\n";
        let d = doc(src);
        assert_tiles(&d, &Scope::Document);
        let out = translate(&d, &Scope::Document, "zh", &Upper).unwrap();
        // Same number of pipes in, same number out.
        assert_eq!(
            out.text.matches('|').count(),
            src.matches('|').count(),
            "column structure changed:\n{}",
            out.text
        );
    }

    #[test]
    fn leading_marker_recognizes_block_syntax() {
        assert_eq!(leading_marker("# Title"), 2);
        assert_eq!(leading_marker("### Deep"), 4);
        assert_eq!(leading_marker("####### too many"), 0, "7 hashes is not ATX");
        assert_eq!(leading_marker("- item"), 2);
        assert_eq!(leading_marker("  - nested"), 4);
        assert_eq!(leading_marker("- [ ] task"), 6);
        assert_eq!(leading_marker("1. first"), 3);
        assert_eq!(leading_marker("12) twelfth"), 4);
        assert_eq!(leading_marker("> quote"), 2);
        assert_eq!(leading_marker("---"), 3, "thematic break");
        assert_eq!(leading_marker("plain prose"), 0);
        assert_eq!(leading_marker("#nospace"), 0);
        assert_eq!(leading_marker(""), 0);
    }

    #[test]
    fn provider_returning_wrong_count_is_an_error_not_a_corruption() {
        struct Broken;
        impl TranslationService for Broken {
            fn translate(&self, _: &[String], _: &str) -> anyhow::Result<Vec<String>> {
                Ok(vec![])
            }
        }
        let err = translate(&doc("Text.\n"), &Scope::Document, "zh", &Broken).unwrap_err();
        assert!(err.to_string().contains("returned 0"));
    }

    #[test]
    fn mdx_blocks_are_verbatim() {
        let src = "import X from 'y'\n\nProse here.\n\n<Chart data={rows} />\n";
        let d = Document::with_type(DocType::Mdx, src.to_string());
        let out = translate(&d, &Scope::Document, "zh", &Upper).unwrap();
        assert!(out.text.contains("import X from 'y'"));
        assert!(out.text.contains("<Chart data={rows} />"));
        assert!(out.text.contains("PROSE HERE."));
    }

    #[test]
    fn out_of_bounds_selection_is_clamped() {
        let d = doc("short\n");
        let out = translate(&d, &Scope::Selection(0..9999), "zh", &Upper).unwrap();
        assert_eq!(out.text, "SHORT\n");
    }
}
