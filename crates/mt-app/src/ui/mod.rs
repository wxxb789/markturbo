//! The Afterglow UI.
//!
//! Owner lane: `foundation` owns this tree, [`shell`], [`tokens`], [`fonts`],
//! [`prims`] and [`motion`]. Every other file names its owner lane in its own
//! header and only that lane edits it.
//!
//! One window, one room. The shell composes the title row, the Shoji sidebar,
//! and the two-screen stage, plus the overlay layer for consent and notices.
//! The source editor stays mounted on one screen; the other holds one docked
//! surface at a time:
//!
//! ```text
//! revision  >  review  >  context  >  preview  >  nothing
//! ```
//!
//! A surface says whether it is on screen with its own `is_open(state)` and
//! draws itself with `field(state, language)`; [`settings`] replaces the whole
//! room with its page; [`notices`] contributes banners to the
//! column, the bottom status line and the overlay's guard.
//!
//! Only model output and outbound data emit light (see [`motion::Heat`] and
//! the emitters in [`prims`]); everything else is ambient ink. Local actions
//! never glow.

pub mod cjk_optics;
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
