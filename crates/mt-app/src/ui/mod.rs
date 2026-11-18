//! The Afterglow UI.
//!
//! Owner lane: `foundation` owns this tree, [`shell`], [`tokens`], [`fonts`],
//! [`prims`] and [`motion`]. Every other file names its owner lane in its own
//! header and only that lane edits it.
//!
//! One window, one room. The shell composes four regions — the 32px title row,
//! the 44px rail, the text column and the field — plus the overlay layer the
//! consent card and the notices use, and routes the field to exactly one
//! surface at a time:
//!
//! ```text
//! revision  >  review  >  context  >  preview (split)  >  nothing
//! ```
//!
//! A surface says whether it is on screen with its own `is_open(state)` and
//! draws itself with `field(state, language)`; [`settings`] replaces the whole
//! room with `page(state, language)`; [`notices`] contributes banners to the
//! column, the bottom status line and the overlay's guard.
//!
//! Only model output and outbound data emit light (see [`motion::Heat`] and
//! the emitters in [`prims`]); everything else is ambient ink. Local actions
//! never glow.

pub mod consent;
pub mod context;
pub mod document;
pub mod fonts;
pub mod glow;
pub mod motion;
pub mod notices;
pub mod preview;
pub mod prims;
pub mod rail;
pub mod review;
pub mod revision;
pub mod settings;
pub mod shell;
pub mod titlebar;
pub mod tokens;
pub mod welcome;

pub use shell::shell;
