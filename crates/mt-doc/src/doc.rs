//! The central document model.
//!
//! A `Document` owns the authoritative source text and a parsed view of it. The
//! source is never rewritten by parsing: everything derived carries byte spans
//! back into the original, so "open and preview" is guaranteed lossless and
//! saving writes back exactly what the editor holds.

use std::path::{Path, PathBuf};

use markdown::{ParseOptions, mdast};

use crate::block::{Block, BlockKind, DiagramKind, MdxKind};
use crate::diagnostic::Diagnostic;
use crate::doctype::DocType;
use crate::frontmatter;
use crate::outline::{Heading, Outline};

/// A parsed document: source of truth plus derived structure.
#[derive(Debug, Clone)]
pub struct Document {
    path: Option<PathBuf>,
    doc_type: DocType,
    source: String,
    /// Raw frontmatter YAML, if the document opens with one.
    frontmatter_raw: Option<String>,
    /// Parsed frontmatter. `None` when absent or malformed.
    frontmatter: Option<serde_yaml::Value>,
    blocks: Vec<Block>,
    outline: Outline,
    diagnostics: Vec<Diagnostic>,
}

impl Document {
    /// Parse `source` as the document at `path`.
    ///
    /// Never fails: parse problems become diagnostics so a broken file still
    /// opens and can be repaired in the editor.
    pub fn new(path: Option<PathBuf>, source: String) -> Self {
        let doc_type = path
            .as_deref()
            .map(DocType::of)
            .unwrap_or(DocType::Markdown);
        let mut doc = Self {
            path,
            doc_type,
            source,
            frontmatter_raw: None,
            frontmatter: None,
            blocks: Vec::new(),
            outline: Outline::default(),
            diagnostics: Vec::new(),
        };
        doc.reparse();
        doc
    }

    /// Parse an in-memory document with an explicit type (no path).
    pub fn with_type(doc_type: DocType, source: String) -> Self {
        let mut doc = Self {
            path: None,
            doc_type,
            source,
            frontmatter_raw: None,
            frontmatter: None,
            blocks: Vec::new(),
            outline: Outline::default(),
            diagnostics: Vec::new(),
        };
        doc.reparse();
        doc
    }

    pub fn path(&self) -> Option<&Path> {
        self.path.as_deref()
    }

    pub fn doc_type(&self) -> DocType {
        self.doc_type
    }

    /// The authoritative source text.
    pub fn source(&self) -> &str {
        &self.source
    }

    pub fn blocks(&self) -> &[Block] {
        &self.blocks
    }

    pub fn outline(&self) -> &Outline {
        &self.outline
    }

    pub fn diagnostics(&self) -> &[Diagnostic] {
        &self.diagnostics
    }

    pub fn frontmatter(&self) -> Option<&serde_yaml::Value> {
        self.frontmatter.as_ref()
    }

    pub fn frontmatter_raw(&self) -> Option<&str> {
        self.frontmatter_raw.as_deref()
    }

    /// Replace the source and reparse. This is the only mutation path, so the
    /// derived state can never drift from the text.
    pub fn set_source(&mut self, source: String) {
        if self.source == source {
            return;
        }
        self.source = source;
        self.reparse();
    }

    /// Blocks that need an out-of-band renderer (diagram or math).
    pub fn renderable_blocks(&self) -> impl Iterator<Item = &Block> {
        self.blocks.iter().filter(|b| b.needs_renderer())
    }

    /// The block containing `offset`, for block-scoped operations such as
    /// translating "the block the cursor is in".
    pub fn block_at(&self, offset: usize) -> Option<&Block> {
        self.blocks
            .iter()
            .find(|b| b.range.start <= offset && offset < b.range.end)
    }

    fn reparse(&mut self) {
        self.diagnostics.clear();
        self.blocks.clear();
        self.outline = Outline::default();

        let (fm, _) = frontmatter::split(&self.source);
        self.frontmatter_raw = fm.as_ref().map(|f| f.raw.clone());
        self.frontmatter = match fm.as_ref() {
            Some(f) => match frontmatter::parse_yaml(&f.raw) {
                Ok(value) => Some(value),
                Err(diag) => {
                    self.diagnostics.push(diag);
                    None
                }
            },
            None => None,
        };
        if frontmatter::has_unterminated_fence(&self.source) {
            self.diagnostics.push(
                Diagnostic::warning("frontmatter", "unterminated `---` block; treated as content")
                    .at_line(1),
            );
        }

        let options = parse_options(self.doc_type);
        // markdown-rs parses the whole document including frontmatter, so
        // offsets are already document-absolute.
        let tree = match markdown::to_mdast(&self.source, &options) {
            Ok(tree) => tree,
            Err(message) => {
                let mut diag = Diagnostic::error(
                    if self.doc_type.is_mdx() { "mdx" } else { "markdown" },
                    message.reason.clone(),
                );
                if let Some(place) = message.place.as_ref() {
                    diag = diag.at_line(place_line(place));
                }
                self.diagnostics.push(diag);
                // A document that fails to parse still opens as source-only:
                // one block covering everything, so nothing is lost.
                self.blocks.push(Block {
                    kind: BlockKind::Markdown,
                    range: 0..self.source.len(),
                    line: 1,
                    content: self.source.clone(),
                });
                return;
            }
        };

        let mdast::Node::Root(root) = tree else {
            return;
        };
        for node in &root.children {
            if let Some(block) = self.to_block(node) {
                self.blocks.push(block);
            }
            if let mdast::Node::Heading(h) = node {
                self.outline.headings.push(Heading {
                    depth: h.depth,
                    text: node_text(node),
                    offset: node.position().map(|p| p.start.offset).unwrap_or(0),
                    line: node.position().map(|p| p.start.line).unwrap_or(1),
                });
            }
        }
        self.outline.structural = Outline::collect_structural(&self.blocks);

        for block in &self.blocks {
            if let BlockKind::Diagram(DiagramKind::Other(name)) = &block.kind {
                self.diagnostics.push(
                    Diagnostic::info(
                        name.clone(),
                        format!("no renderer registered for `{name}`; showing source"),
                    )
                    .at_line(block.line),
                );
            }
        }
    }

    fn to_block(&self, node: &mdast::Node) -> Option<Block> {
        let position = node.position()?;
        let range = position.start.offset..position.end.offset;
        let line = position.start.line;
        let source_slice = self.source.get(range.clone()).unwrap_or_default();

        let (kind, content) = match node {
            mdast::Node::Code(code) => {
                let lang = code.lang.as_deref().unwrap_or("").trim();
                // The fence body, not the fence: renderers want the diagram
                // source verbatim.
                let content = code.value.clone();
                match DiagramKind::from_lang(lang) {
                    Some(diagram) => (BlockKind::Diagram(diagram), content),
                    None if lang.eq_ignore_ascii_case("math")
                        || lang.eq_ignore_ascii_case("latex")
                        || lang.eq_ignore_ascii_case("tex") =>
                    {
                        (BlockKind::Math, content)
                    }
                    None => (
                        BlockKind::Code {
                            lang: (!lang.is_empty()).then(|| lang.to_string()),
                        },
                        content,
                    ),
                }
            }
            mdast::Node::Math(math) => (BlockKind::Math, math.value.clone()),
            mdast::Node::Html(_) => (BlockKind::Html, source_slice.to_string()),
            mdast::Node::MdxjsEsm(_) => (
                BlockKind::Mdx(MdxKind::EsmStatement),
                source_slice.to_string(),
            ),
            mdast::Node::MdxJsxFlowElement(_) => (
                BlockKind::Mdx(MdxKind::JsxElement),
                source_slice.to_string(),
            ),
            mdast::Node::MdxFlowExpression(_) => (
                BlockKind::Mdx(MdxKind::Expression),
                source_slice.to_string(),
            ),
            // Frontmatter is modeled separately; skip the AST node so it does
            // not show up as a content block.
            mdast::Node::Yaml(_) | mdast::Node::Toml(_) => return None,
            _ => (BlockKind::Markdown, source_slice.to_string()),
        };

        Some(Block {
            kind,
            range,
            line,
            content,
        })
    }
}

/// Parse options for a document type.
///
/// MDX and raw HTML are mutually exclusive in markdown-rs (HTML wins when both
/// are on), which is exactly why the document type — not a global setting —
/// selects them.
pub fn parse_options(doc_type: DocType) -> ParseOptions {
    let mut options = ParseOptions::gfm();
    options.constructs.frontmatter = true;
    options.constructs.math_flow = true;
    options.constructs.math_text = true;

    if doc_type.is_mdx() {
        options.constructs.html_flow = false;
        options.constructs.html_text = false;
        options.constructs.autolink = false;
        options.constructs.code_indented = false;
        options.constructs.mdx_esm = true;
        options.constructs.mdx_expression_flow = true;
        options.constructs.mdx_expression_text = true;
        options.constructs.mdx_jsx_flow = true;
        options.constructs.mdx_jsx_text = true;
        // markdown-rs only enables ESM when given a parse callback. We have no
        // JS engine, and we don't need one: the structural boundary is a blank
        // line, which markdown-rs already finds. Accepting every statement means
        // native mode sees the block (for the outline and for not corrupting the
        // file), while the WebView path — which runs a real MDX compiler —
        // remains the authority on JS validity.
        options.mdx_esm_parse = Some(Box::new(|_: &str| markdown::MdxSignal::Ok));
    }
    options
}

fn place_line(place: &markdown::message::Place) -> usize {
    match place {
        markdown::message::Place::Point(p) => p.line,
        markdown::message::Place::Position(p) => p.start.line,
    }
}

/// Concatenate the text content of a node, for headings and outline labels.
fn node_text(node: &mdast::Node) -> String {
    match node {
        mdast::Node::Text(t) => t.value.clone(),
        mdast::Node::InlineCode(c) => c.value.clone(),
        mdast::Node::InlineMath(m) => m.value.clone(),
        _ => node
            .children()
            .map(|children| children.iter().map(node_text).collect::<String>())
            .unwrap_or_default(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn md(source: &str) -> Document {
        Document::with_type(DocType::Markdown, source.to_string())
    }

    #[test]
    fn source_is_preserved_verbatim() {
        // Deliberately non-normalized: extra blank lines, trailing spaces,
        // setext heading, tabs. Opening must not touch any of it.
        let src = "Title\n=====\n\n\n-   item  \n\t- nested\n";
        let doc = md(src);
        assert_eq!(doc.source(), src);
    }

    #[test]
    fn extracts_headings_in_order() {
        let doc = md("# One\n\ntext\n\n## Two `code`\n\n### 三级标题\n");
        let texts: Vec<_> = doc.outline().headings.iter().map(|h| h.text.as_str()).collect();
        assert_eq!(texts, vec!["One", "Two code", "三级标题"]);
        let depths: Vec<_> = doc.outline().headings.iter().map(|h| h.depth).collect();
        assert_eq!(depths, vec![1, 2, 3]);
    }

    #[test]
    fn classifies_fenced_blocks() {
        let doc = md(
            "```rust\nlet x = 1;\n```\n\n```mermaid\ngraph TD;\nA-->B;\n```\n\n```d2\na -> b\n```\n\n```plantuml\n@startuml\n@enduml\n```\n",
        );
        let kinds: Vec<_> = doc
            .blocks()
            .iter()
            .map(|b| b.kind.clone())
            .collect();
        assert_eq!(
            kinds,
            vec![
                BlockKind::Code { lang: Some("rust".into()) },
                BlockKind::Diagram(DiagramKind::Mermaid),
                BlockKind::Diagram(DiagramKind::D2),
                BlockKind::Diagram(DiagramKind::PlantUml),
            ]
        );
        assert_eq!(doc.renderable_blocks().count(), 3);
    }

    #[test]
    fn fence_content_excludes_the_fence() {
        let doc = md("```mermaid\ngraph TD;\nA-->B;\n```\n");
        assert_eq!(doc.blocks()[0].content, "graph TD;\nA-->B;");
    }

    #[test]
    fn display_math_is_a_math_block() {
        let doc = md("$$\n\\frac{a}{b}\n$$\n");
        assert_eq!(doc.blocks()[0].kind, BlockKind::Math);
        assert_eq!(doc.blocks()[0].content.trim(), "\\frac{a}{b}");
    }

    #[test]
    fn frontmatter_is_parsed_and_not_a_content_block() {
        let doc = md("---\nname: demo\n---\n\n# Body\n");
        assert_eq!(
            doc.frontmatter()
                .and_then(|v| v.get("name"))
                .and_then(|v| v.as_str()),
            Some("demo")
        );
        assert!(
            doc.blocks().iter().all(|b| b.kind == BlockKind::Markdown),
            "frontmatter must not appear as a content block"
        );
        assert_eq!(doc.outline().headings.len(), 1);
    }

    #[test]
    fn malformed_frontmatter_yields_a_diagnostic_and_still_opens() {
        let doc = md("---\nname: [unclosed\n---\n\n# Body\n");
        assert!(doc.frontmatter().is_none());
        assert_eq!(doc.diagnostics().len(), 1);
        assert_eq!(doc.diagnostics()[0].source, "frontmatter");
        assert_eq!(doc.outline().headings.len(), 1, "body still parses");
    }

    #[test]
    fn unknown_diagram_language_reports_info_not_error() {
        let doc = md("```graphviz\ndigraph {}\n```\n");
        assert_eq!(
            doc.blocks()[0].kind,
            BlockKind::Diagram(DiagramKind::Other("graphviz".into()))
        );
        let diag = &doc.diagnostics()[0];
        assert_eq!(diag.severity, crate::diagnostic::Severity::Info);
        assert!(diag.message.contains("no renderer"));
    }

    #[test]
    fn mdx_constructs_are_recognized() {
        let src = "import Chart from './c'\n\n# Title\n\n<RevenueChart data={rows} />\n\n{1 + 2}\n";
        let doc = Document::with_type(DocType::Mdx, src.to_string());
        let kinds: Vec<_> = doc.blocks().iter().map(|b| b.kind.clone()).collect();
        assert!(kinds.contains(&BlockKind::Mdx(MdxKind::EsmStatement)));
        assert!(kinds.contains(&BlockKind::Mdx(MdxKind::JsxElement)));
        assert!(kinds.contains(&BlockKind::Mdx(MdxKind::Expression)));
        // And the outline has structural entries even though there is 1 heading.
        let labels: Vec<_> = doc
            .outline()
            .structural
            .iter()
            .map(|e| e.label.as_str())
            .collect();
        assert!(labels.contains(&"<RevenueChart />"), "got {labels:?}");
    }

    #[test]
    fn markdown_mode_does_not_produce_mdx_blocks() {
        // Same source as plain Markdown: `<RevenueChart />` is HTML, not JSX.
        let src = "<RevenueChart />\n";
        let doc = md(src);
        assert_eq!(doc.blocks()[0].kind, BlockKind::Html);
    }

    #[test]
    fn invalid_mdx_produces_a_diagnostic_and_preserves_source() {
        let src = "# Title\n\n<Unclosed>\n";
        let doc = Document::with_type(DocType::Mdx, src.to_string());
        assert!(
            doc.diagnostics().iter().any(|d| d.source == "mdx"),
            "expected an mdx diagnostic, got {:?}",
            doc.diagnostics()
        );
        assert_eq!(doc.source(), src, "source must survive a parse failure");
        assert!(!doc.blocks().is_empty(), "must still be viewable");
    }

    #[test]
    fn set_source_reparses() {
        let mut doc = md("# One\n");
        assert_eq!(doc.outline().headings.len(), 1);
        doc.set_source("# One\n\n## Two\n".to_string());
        assert_eq!(doc.outline().headings.len(), 2);
    }

    #[test]
    fn block_at_offset_finds_the_containing_block() {
        let src = "# Title\n\n```rust\nlet x = 1;\n```\n";
        let doc = md(src);
        let code_offset = src.find("let x").unwrap();
        let block = doc.block_at(code_offset).expect("block");
        assert!(matches!(block.kind, BlockKind::Code { .. }));
    }

    #[test]
    fn handles_unicode_and_cjk_without_panicking() {
        let src = "# 标题 🎉\n\n段落文字，包含 `代码` 和 [链接](https://例え.jp)。\n\n| 列一 | 列二 |\n|---|---|\n| 值 | 值 |\n";
        let doc = md(src);
        assert_eq!(doc.source(), src);
        assert_eq!(doc.outline().headings[0].text, "标题 🎉");
        // All block ranges must be valid char boundaries into the source.
        for b in doc.blocks() {
            assert!(src.get(b.range.clone()).is_some(), "bad range {:?}", b.range);
        }
    }

    #[test]
    fn ranges_never_overlap_and_stay_in_bounds() {
        let src = "# A\n\ntext\n\n- a\n- b\n\n> quote\n\n```sh\nls\n```\n\n| x |\n|---|\n| 1 |\n";
        let doc = md(src);
        let mut prev_end = 0;
        for b in doc.blocks() {
            assert!(b.range.start >= prev_end, "overlap at {:?}", b.range);
            assert!(b.range.end <= src.len());
            prev_end = b.range.end;
        }
    }
}
