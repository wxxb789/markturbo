//! Block model: the unit the renderer registry dispatches on.
//!
//! The Markdown parser produces a flat list of top-level blocks. Fenced blocks
//! whose info string names a diagram/math technology become `Diagram`/`Math`,
//! so adding a new renderer means adding a `DiagramKind` mapping — not touching
//! the parser or the renderer core.

use std::fmt;

/// Diagram technologies recognized from a fence info string.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum DiagramKind {
    Mermaid,
    D2,
    PlantUml,
    /// A fence tagged with something diagram-shaped we don't render yet. Kept
    /// as data so a future renderer registers without a parser change.
    Other(String),
}

impl DiagramKind {
    /// Stable identifier used as the renderer-registry key.
    pub fn id(&self) -> &str {
        match self {
            DiagramKind::Mermaid => "mermaid",
            DiagramKind::D2 => "d2",
            DiagramKind::PlantUml => "plantuml",
            DiagramKind::Other(s) => s,
        }
    }

    /// Map a fence language token to a diagram kind, if it names one.
    pub fn from_lang(lang: &str) -> Option<Self> {
        match lang.to_ascii_lowercase().as_str() {
            "mermaid" => Some(DiagramKind::Mermaid),
            "d2" => Some(DiagramKind::D2),
            "plantuml" | "puml" | "uml" => Some(DiagramKind::PlantUml),
            // Known-but-unimplemented technologies. Listing them here means the
            // block is classified as a diagram (and gets a "no renderer"
            // diagnostic) instead of silently rendering as a code block.
            "graphviz" | "dot" | "vega" | "vega-lite" | "pikchr" | "svgbob" | "nomnoml"
            | "typst" => Some(DiagramKind::Other(lang.to_ascii_lowercase())),
            _ => None,
        }
    }
}

impl fmt::Display for DiagramKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            DiagramKind::Mermaid => "Mermaid",
            DiagramKind::D2 => "D2",
            DiagramKind::PlantUml => "PlantUML",
            DiagramKind::Other(s) => s,
        })
    }
}

/// What a top-level block is, for renderer dispatch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BlockKind {
    /// Prose, headings, lists, tables, quotes — anything the Markdown renderer
    /// handles directly.
    Markdown,
    /// A fenced code block with an ordinary language.
    Code {
        lang: Option<String>,
    },
    /// Display math: a `$$…$$` block or a ```math fence.
    Math,
    Diagram(DiagramKind),
    /// A raw HTML block.
    Html,
    /// An MDX construct: JSX element, `import`/`export`, or `{expression}`.
    Mdx(MdxKind),
}

/// The MDX construct a block represents. Native mode needs to tell these apart
/// to build an outline and to render sensible placeholders without corrupting
/// the document.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MdxKind {
    /// `import x from 'y'` / `export const z = …`
    EsmStatement,
    /// `<Component />` at block level.
    JsxElement,
    /// `{someExpression}` at block level.
    Expression,
}

impl MdxKind {
    pub fn label(self) -> &'static str {
        match self {
            MdxKind::EsmStatement => "import/export",
            MdxKind::JsxElement => "JSX",
            MdxKind::Expression => "expression",
        }
    }
}

/// A top-level block with its exact source span, so editing and preview stay in
/// sync and the original text is always recoverable.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Block {
    pub kind: BlockKind,
    /// Byte range in the document source.
    pub range: std::ops::Range<usize>,
    /// 1-based start line, for diagnostics and source<->preview sync.
    pub line: usize,
    /// For fenced blocks, the fence body without the fences. For everything
    /// else, the full source slice.
    pub content: String,
}

impl Block {
    /// The renderer-registry key for this block.
    ///
    /// Blocks that the Markdown renderer handles natively return `None`.
    pub fn renderer_id(&self) -> Option<&str> {
        match &self.kind {
            BlockKind::Diagram(kind) => Some(kind.id()),
            BlockKind::Math => Some("math"),
            _ => None,
        }
    }

    /// True when this block needs an out-of-band renderer (diagram or math).
    pub fn needs_renderer(&self) -> bool {
        self.renderer_id().is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_fence_languages() {
        assert_eq!(
            DiagramKind::from_lang("mermaid"),
            Some(DiagramKind::Mermaid)
        );
        assert_eq!(
            DiagramKind::from_lang("MERMAID"),
            Some(DiagramKind::Mermaid)
        );
        assert_eq!(DiagramKind::from_lang("puml"), Some(DiagramKind::PlantUml));
        assert_eq!(DiagramKind::from_lang("d2"), Some(DiagramKind::D2));
        assert_eq!(DiagramKind::from_lang("rust"), None);
        assert_eq!(DiagramKind::from_lang(""), None);
    }

    #[test]
    fn unimplemented_diagrams_are_still_diagrams() {
        // So they get a diagnostic rather than rendering as code.
        let kind = DiagramKind::from_lang("graphviz").expect("recognized");
        assert_eq!(kind.id(), "graphviz");
    }

    #[test]
    fn renderer_id_only_for_out_of_band_blocks() {
        let at = |kind| Block {
            kind,
            range: 0..0,
            line: 1,
            content: String::new(),
        };
        assert_eq!(at(BlockKind::Markdown).renderer_id(), None);
        assert_eq!(
            at(BlockKind::Code {
                lang: Some("rust".into())
            })
            .renderer_id(),
            None
        );
        assert_eq!(at(BlockKind::Math).renderer_id(), Some("math"));
        assert_eq!(
            at(BlockKind::Diagram(DiagramKind::D2)).renderer_id(),
            Some("d2")
        );
    }
}
