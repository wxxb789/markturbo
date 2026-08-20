//! markturbo: a native GPUI workspace for Markdown as the interface between
//! humans and AI agents.
//!
//! Layering:
//!
//! ```text
//! mt-doc     document engine — no GPUI, reusable headless
//!   ↓
//! assets     icons + the fonts GPUI's SVG renderer needs
//! fs         load/save with conflict protection
//! workspace  directory tree
//! renderer   block renderer registry (diagrams, math)
//! web        WebView compatibility path
//!   ↓
//! views      GPUI views
//! ```

pub mod assets;
pub mod fs;
pub mod renderer;
pub mod translate;
pub mod views;
pub mod watcher;
pub mod web;
pub mod workspace;
