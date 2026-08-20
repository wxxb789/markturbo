//! Document outline: headings, plus MDX structural entries.
//!
//! Used by the Outline panel and by translation (to scope a "block" the user
//! selected). Pure data — no UI types.

use crate::block::{Block, BlockKind, MdxKind};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Heading {
    /// 1-6 for `#`..`######`.
    pub depth: u8,
    pub text: String,
    /// Byte offset of the heading in the source.
    pub offset: usize,
    /// 1-based line.
    pub line: usize,
}

/// A structural entry that is not a Markdown heading, e.g. an MDX component.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StructuralEntry {
    pub label: String,
    pub kind: MdxKind,
    pub offset: usize,
    pub line: usize,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Outline {
    pub headings: Vec<Heading>,
    /// MDX imports/exports/JSX, so an MDX file with no headings still has a
    /// usable outline.
    pub structural: Vec<StructuralEntry>,
}

impl Outline {
    pub fn is_empty(&self) -> bool {
        self.headings.is_empty() && self.structural.is_empty()
    }

    /// Build the non-heading part of the outline from parsed blocks.
    pub(crate) fn collect_structural(blocks: &[Block]) -> Vec<StructuralEntry> {
        blocks
            .iter()
            .filter_map(|b| {
                let BlockKind::Mdx(kind) = b.kind else {
                    return None;
                };
                Some(StructuralEntry {
                    label: summarize_mdx(&b.content, kind),
                    kind,
                    offset: b.range.start,
                    line: b.line,
                })
            })
            .collect()
    }
}

/// One-line label for an MDX block, for the outline and native placeholders.
fn summarize_mdx(content: &str, kind: MdxKind) -> String {
    let first = content.lines().find(|l| !l.trim().is_empty()).unwrap_or("").trim();
    match kind {
        // `<RevenueChart data={x} />` -> `<RevenueChart />`
        MdxKind::JsxElement => jsx_tag_name(first)
            .map(|name| format!("<{name} />"))
            .unwrap_or_else(|| truncate(first, 48)),
        _ => truncate(first, 60),
    }
}

/// Extract the component/tag name from an opening JSX tag.
pub fn jsx_tag_name(source: &str) -> Option<String> {
    let rest = source.trim_start().strip_prefix('<')?;
    let rest = rest.strip_prefix('/').unwrap_or(rest);
    let name: String = rest
        .chars()
        .take_while(|c| c.is_alphanumeric() || *c == '.' || *c == '_' || *c == '-' || *c == ':')
        .collect();
    (!name.is_empty()).then_some(name)
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    // Truncate on a char boundary; CJK and emoji must not be split.
    let head: String = s.chars().take(max).collect();
    format!("{head}…")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn jsx_tag_names() {
        assert_eq!(jsx_tag_name("<RevenueChart />"), Some("RevenueChart".into()));
        assert_eq!(jsx_tag_name("  <Foo.Bar a={1}>"), Some("Foo.Bar".into()));
        assert_eq!(jsx_tag_name("</Closing>"), Some("Closing".into()));
        assert_eq!(jsx_tag_name("not jsx"), None);
        assert_eq!(jsx_tag_name("<>"), None, "fragments have no name");
    }

    #[test]
    fn summarizes_jsx_to_a_bare_tag() {
        let s = summarize_mdx("<RevenueChart data={rows} height={300} />", MdxKind::JsxElement);
        assert_eq!(s, "<RevenueChart />");
    }

    #[test]
    fn truncation_respects_char_boundaries() {
        let cjk = "中文".repeat(50);
        let out = truncate(&cjk, 5);
        assert_eq!(out.chars().count(), 6, "5 chars + ellipsis");
        assert!(out.ends_with('…'));
    }

    #[test]
    fn structural_entries_come_only_from_mdx_blocks() {
        let blocks = vec![
            Block {
                kind: BlockKind::Markdown,
                range: 0..5,
                line: 1,
                content: "hello".into(),
            },
            Block {
                kind: BlockKind::Mdx(MdxKind::EsmStatement),
                range: 6..30,
                line: 3,
                content: "import X from 'y'".into(),
            },
        ];
        let s = Outline::collect_structural(&blocks);
        assert_eq!(s.len(), 1);
        assert_eq!(s[0].label, "import X from 'y'");
        assert_eq!(s[0].line, 3);
    }
}
