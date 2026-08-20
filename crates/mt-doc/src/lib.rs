//! markturbo document engine.
//!
//! This crate deliberately has no GPUI dependency: it owns document semantics
//! (source text, parsed structure, metadata, blocks, diagnostics) so the same
//! model can drive the native renderer, the WebView renderer, an editor, a CLI,
//! or headless tooling.

pub mod block;
pub mod diagnostic;
pub mod doc;
pub mod doctype;
pub mod frontmatter;
pub mod outline;
pub mod skill;
pub mod translate;

pub use block::{Block, BlockKind, DiagramKind};
pub use diagnostic::{Diagnostic, Severity};
pub use doc::Document;
pub use doctype::DocType;
pub use outline::{Heading, Outline};
pub use skill::{Skill, SkillMeta};
