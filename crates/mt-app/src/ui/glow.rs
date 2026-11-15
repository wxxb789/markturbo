//! Owner lane: review.
//!
//! The document's light: the decorations `document.rs` puts on the source
//! editor so findings move with the text and cannot drift.
//!
//! Contract: [`document_decorations`] is what `document.rs` hands to the
//! editor's `decorations` prop, in the shape `nsh_base::EditorDecoration`
//! takes:
//!
//! * a STATED finding lights the anchored glyphs — an inline range style with
//!   the hot or ember glyph colour, plus the tight under-bloom;
//! * an INFERRED finding lights the air — a `line_style` haze in the
//!   sea-glass phosphor with a 2px gutter edge, while the glyphs stay ink;
//! * every finding gets a 2px heat tick as a gutter mark.
//!
//! The function is called from a reactive binding, so it may read signals; it
//! returns nothing when no review is live or the document has moved on. It must
//! never mutate state.
//!
//! The review lane owns everything in this file. What is here now returns no
//! decorations, which is exactly "no findings are lit".

use nsh_base::editor::EditorDecoration;

use crate::state::{AppState, Tab};

/// The live decorations for `tab`'s source editor: stated glyphs, inferred
/// haze and one gutter tick per finding. Empty when there is nothing live to
/// show (no review, a stale review, or a document whose text has moved on).
pub fn document_decorations(state: &AppState, tab: &Tab) -> Vec<EditorDecoration> {
    let _ = (state, tab);
    Vec::new()
}
