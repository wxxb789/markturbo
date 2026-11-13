//! markturbo: a native workspace for Markdown as the interface between humans
//! and AI agents.
//!
//! The desktop depends on `mt-core`; headless workflows never depend on the
//! presentation layer.
//!
//! ```text
//! mt-core    document, workspace, Agent Skills, Review/Translation,
//!            file safety, recovery, settings, credentials, rendering
//!     ↑
//! mt-app     nsh application: shell, document, review, recovery, settings,
//!            web presentation, i18n, embedded UI/sample assets
//! ```

pub mod app;
pub mod app_paths;
pub mod assets;
pub mod i18n;
pub mod logging;
pub mod state;
pub mod web;

pub use app::App;

/// The crate's UI set: what a TSX entry may import from `markturbo/native`.
#[nsh_core::prelude::native_module("markturbo/native")]
fn native(registry: &mut nsh_core::prelude::Registry) {
    registry.register::<App>();
}
