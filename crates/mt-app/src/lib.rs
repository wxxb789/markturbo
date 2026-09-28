//! markturbo: a native GPUI workspace for Markdown as the interface between
//! humans and AI agents.
//!
//! The desktop depends on `mt-core`; headless workflows never depend on GPUI.
//!
//! ```text
//! mt-core    document, workspace, Agent Skills, Review/Translation,
//!            file safety, recovery, settings, credentials, rendering
//!     ↑
//! mt-app     main/startup + GPUI views, web and theme presentation,
//!            i18n, embedded UI/sample assets, settings/credential globals
//! ```

pub mod app_paths;
pub mod assets;
pub mod credentials;
pub mod i18n;
pub mod metrics;
pub mod settings;
pub mod startup;
pub mod theme;
pub mod views;
pub mod web;
