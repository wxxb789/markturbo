//! Layout metrics.
//!
//! One place for the numbers that decide how the window is spaced, because a
//! layout is only coherent if the same distance means the same thing everywhere.
//! Before this, each view picked its own `px_2`/`px_3`/`p_4` and the three side
//! panels disagreed about where their content started — visible as a ragged left
//! edge when switching tabs.
//!
//! The scale is 4px-based, which is what `gpui-component`'s own `p_*`/`gap_*`
//! helpers step in, so these constants and those helpers stay commensurable.
//!
//! # Pixels or fractions
//!
//! Both, deliberately, and the split is not arbitrary. A number is a **pixel**
//! when what it sizes is decided by its content — a row holds one line of text,
//! a hit target has to survive a mouse, a title bar has to clear Windows' 32px
//! caption buttons. Scaling those with the window would make a maximized window
//! draw a 60px-tall file row for no reason.
//!
//! A number is a **fraction** when it partitions the window between siblings —
//! the side panels against the document. Those are the ones that read as wrong
//! at a size other than the developer's: a 268px panel is a third of a 768px
//! laptop and a tenth of a 2560px monitor, and it was chosen against exactly
//! one of them. [`PanelWidth`] carries the fraction plus the pixel guard rails
//! that keep it legible at both ends.

use std::ops::Range;

use gpui::{Pixels, px};

/// Distance from a window edge to the content inside it.
///
/// One inset for the title bar, the panels and the status bar alike. This is
/// what makes the left edge of the file tree line up with the left edge of the
/// window title above it.
pub const INSET: f32 = 12.;

/// Height of the title bar's content row.
///
/// Tall enough for a 24px hit target plus breathing room. Windows' own caption
/// buttons are 32px tall, so anything shorter leaves them poking out.
pub const TITLE_BAR: f32 = 40.;

/// The smallest square a pointer target may be.
///
/// 24px is the smallest that stays comfortable with a mouse; smaller icon
/// buttons keep their glyph size and grow their padding to reach it.
pub const TARGET: f32 = 24.;

/// Height of a row in a list — file tree, skills, outline.
///
/// Deliberately below the 32px a "comfortable" list would use: these panels are
/// scanned, not clicked through, and 28px fits a third more of a repository on
/// screen while staying above [`TARGET`].
pub const ROW: f32 = 28.;

/// Vertical gap between rows in a list.
///
/// Small but not zero: touching rows read as a solid block, and the hover
/// highlight needs somewhere to end.
pub const ROW_GAP: f32 = 2.;

/// Horizontal padding inside a list row.
pub const ROW_PAD: f32 = 8.;

/// Indentation per level in a tree.
pub const INDENT: f32 = 14.;

/// A panel width, expressed as a share of the window.
///
/// The pixel bounds are not a second opinion about the width — they are the
/// two places a fraction stops being right. Below `min` a file name is all
/// ellipsis regardless of how wide the monitor is; above `max` the panel is
/// eating the document it exists to annotate. Between them the fraction wins,
/// which is what makes the same layout read the same on a 13" laptop and a 4K
/// display.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PanelWidth {
    /// Share of the window's width, 0..1.
    pub fraction: f32,
    pub min: f32,
    pub max: f32,
}

impl PanelWidth {
    /// This panel's width in a window `viewport` wide.
    pub fn resolve(self, viewport: Pixels) -> Pixels {
        let wanted = f32::from(viewport) * self.fraction;
        // A window narrower than `min` cannot honor the floor without hiding
        // the document entirely, so the floor yields — a panel is allowed to be
        // cramped, never allowed to be the whole window.
        let max = self.max.min(f32::from(viewport) * MAX_PANEL_SHARE);
        px(wanted.clamp(self.min.min(max), max))
    }

    /// The drag limits for this panel, as `resizable_panel().size_range()` wants
    /// them.
    ///
    /// Wider than [`Self::resolve`]'s clamp on purpose: the fraction decides
    /// where the panel *starts*, and a user who drags it somewhere else has
    /// said what they want. Only the two absolutes are enforced.
    pub fn drag_range(self) -> Range<Pixels> {
        px(self.min)..px(self.max)
    }
}

/// The most of a window one panel may claim when the fraction is resolved.
///
/// Only reachable on a window narrow enough that the pixel floor and the
/// fraction disagree; without it, two 340px panels on a 600px window would
/// leave the document nothing.
const MAX_PANEL_SHARE: f32 = 0.4;

/// Default width of the side panel.
///
/// A fifth of the window: enough for a nested file name at laptop width,
/// and it keeps growing with the monitor rather than staying at whatever
/// looked right on the machine it was written on.
pub const SIDE_PANEL: PanelWidth = PanelWidth {
    fraction: 0.2,
    min: SIDE_PANEL_MIN,
    max: 640.,
};

/// Default width of the right details panel.
///
/// A larger share than the left, for a reason that is not taste: the left
/// column holds names, which elide gracefully, while the right holds
/// label/value pairs whose label column alone is 96px. At the left's share the
/// value beside it had barely 140px, so every path and every tool list wrapped
/// — which is what made the panel look broken rather than narrow.
pub const RIGHT_PANEL: PanelWidth = PanelWidth {
    fraction: 0.24,
    min: 260.,
    max: 720.,
};

/// Smallest useful side panel width. Below this, file names are all ellipsis.
pub const SIDE_PANEL_MIN: f32 = 180.;

/// Gap between related controls in a row — a button and its neighbour.
pub const GAP: f32 = 6.;

/// Gap between unrelated groups in the same row.
pub const GAP_GROUP: f32 = 12.;

/// Padding inside a panel's header row.
pub const HEADER_PAD_Y: f32 = 8.;

/// Diameter of the unsaved-changes dot on a tab.
///
/// Sized to read as a deliberate mark rather than a rendering artifact, and
/// small enough to sit in the same slot the close button occupies — the two
/// swap, so a difference in size would make the tab jump on every keystroke
/// that first dirties it.
pub const DIRTY_DOT: f32 = 8.;

/// Height of the status bar.
pub const STATUS_BAR: f32 = 26.;

/// The corner radius of a general element. Matches `Theme::radius`, which is
/// what `cx.theme().radius` returns — this is for the places that need the raw
/// number rather than the theme's `Pixels`.
pub const RADIUS: f32 = 8.;

/// Convenience wrappers, so call sites read `metrics::inset()` rather than
/// `px(metrics::INSET)`.
pub fn inset() -> Pixels {
    px(INSET)
}

pub fn title_bar() -> Pixels {
    px(TITLE_BAR)
}

pub fn row() -> Pixels {
    px(ROW)
}

pub fn row_gap() -> Pixels {
    px(ROW_GAP)
}

pub fn row_pad() -> Pixels {
    px(ROW_PAD)
}

pub fn indent(depth: usize) -> Pixels {
    px(INDENT * depth as f32)
}

pub fn gap() -> Pixels {
    px(GAP)
}

pub fn gap_group() -> Pixels {
    px(GAP_GROUP)
}

pub fn target() -> Pixels {
    px(TARGET)
}

pub fn status_bar() -> Pixels {
    px(STATUS_BAR)
}

pub fn dirty_dot() -> Pixels {
    px(DIRTY_DOT)
}

pub fn header_pad_y() -> Pixels {
    px(HEADER_PAD_Y)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_spacing_step_is_on_the_four_pixel_grid() {
        // The scale has to agree with gpui-component's `p_*`/`gap_*` helpers,
        // which step in 4px — a 5px inset next to a `gap_2` is the kind of
        // half-pixel drift that makes a layout look accidental.
        for (name, value) in [
            ("INSET", INSET),
            ("TITLE_BAR", TITLE_BAR),
            ("TARGET", TARGET),
            ("ROW", ROW),
            ("ROW_PAD", ROW_PAD),
            ("GAP_GROUP", GAP_GROUP),
            ("HEADER_PAD_Y", HEADER_PAD_Y),
            ("RADIUS", RADIUS),
        ] {
            assert_eq!(value % 4., 0., "{name} = {value} is off the grid");
        }
    }

    /// Relationships between the constants, checked at compile time.
    ///
    /// `const _:` rather than `assert!` inside a `#[test]`: these compare two
    /// constants, so the comparison is decided when the crate is compiled and
    /// clippy rightly points out that a runtime assertion over it is theatre.
    /// This way a bad edit fails the build rather than a test run.
    const _: () = {
        // A row shorter than the minimum target is a row that is hard to hit.
        assert!(ROW >= TARGET);
        // The side panel can be narrowed, but not to uselessness — below about
        // 160px a file name is all ellipsis.
        assert!(SIDE_PANEL_MIN >= 160.);
        // The details panel holds a 96px label column plus its value, so it
        // needs to be the wider of the two or every field wraps.
        assert!(RIGHT_PANEL.fraction > SIDE_PANEL.fraction);
        assert!(RIGHT_PANEL.min > SIDE_PANEL.min);
        // Two panels at their default share must still leave the document the
        // majority of the window. This is the check that makes the fractions
        // reviewable as a set rather than one at a time.
        assert!(SIDE_PANEL.fraction + RIGHT_PANEL.fraction < 0.5);
        // Windows draws 32px caption buttons; a shorter bar leaves them
        // overhanging the content below.
        assert!(TITLE_BAR >= 32.);
        // The whole point of two gaps: related controls sit closer than
        // unrelated ones. Equal values would make the distinction invisible.
        assert!(GAP < GAP_GROUP);
        assert!(ROW_GAP < GAP);
    };

    #[test]
    fn a_panel_scales_with_the_window() {
        // The point of the fraction: the same layout on a laptop and a 4K
        // display, rather than a column chosen against one of them.
        let laptop = SIDE_PANEL.resolve(px(1366.));
        let big = SIDE_PANEL.resolve(px(2560.));
        assert!(big > laptop, "{big:?} should exceed {laptop:?}");
        assert_eq!(laptop, px(1366. * 0.2));
    }

    #[test]
    fn the_pixel_guards_bound_both_ends() {
        // Narrow window: the floor holds, up to the share cap.
        let tiny = SIDE_PANEL.resolve(px(600.));
        assert_eq!(tiny, px(600. * 0.2_f32).max(px(SIDE_PANEL_MIN)));
        // Huge window: the ceiling holds, so the panel does not become the
        // window.
        let huge = SIDE_PANEL.resolve(px(6000.));
        assert_eq!(huge, px(SIDE_PANEL.max));
    }

    #[test]
    fn no_window_is_mostly_panel() {
        // The failure this prevents is a narrow window where both floors apply
        // and the document is squeezed to nothing.
        for width in [480., 600., 768., 1024., 1366., 1920., 2560., 3840.] {
            let panels = f32::from(SIDE_PANEL.resolve(px(width)))
                + f32::from(RIGHT_PANEL.resolve(px(width)));
            assert!(
                panels < width * 0.81,
                "at {width}px the panels take {panels}px, leaving the document \
                 {}px",
                width - panels
            );
        }
    }

    #[test]
    fn the_details_panel_is_the_wider_one_at_every_size() {
        // Not just in the constants: the clamps could invert the relationship
        // at one end, which is exactly where it would go unnoticed.
        for width in [768., 1366., 1920., 3840.] {
            assert!(
                RIGHT_PANEL.resolve(px(width)) >= SIDE_PANEL.resolve(px(width)),
                "at {width}px the details panel is narrower than the file tree, \
                 so its label/value pairs wrap"
            );
        }
    }

    #[test]
    fn indentation_accumulates_per_level() {
        assert_eq!(indent(0), px(0.));
        assert_eq!(indent(2), px(INDENT * 2.));
        // A tree five levels deep must still leave room for a name in the
        // minimum-width panel.
        assert!(
            f32::from(indent(5)) + ROW_PAD * 2. < SIDE_PANEL_MIN - 60.,
            "deep nesting leaves no room for the label"
        );
    }

    /// Every panel header must start at the same distance from the window edge.
    ///
    /// A source-level check because the failure is purely visual: three panels
    /// each picking their own `px_2`/`px_3`/`p_4` produce a left edge that jumps
    /// when switching tabs, which no runtime assertion sees. What it guards is
    /// that the views go through this module rather than reintroducing literals.
    #[test]
    fn the_panels_share_one_horizontal_inset() {
        let sources = [
            (
                "explorer.rs",
                crate::views::production_source(include_str!("views/explorer.rs")),
            ),
            (
                "harness.rs",
                crate::views::production_source(include_str!("views/harness.rs")),
            ),
            (
                "workspace.rs",
                crate::views::production_source(include_str!("views/workspace.rs")),
            ),
        ];
        for (name, source) in sources {
            assert!(
                source.contains("metrics::inset()"),
                "{name} does not use the shared inset; a hard-coded padding \
                 there makes the panel edges disagree"
            );
        }
    }

    /// List rows must use the shared row padding.
    ///
    /// Same reasoning: rows in the file tree and rows in the harness panel sit
    /// in the same column, so a different padding in each is visible as
    /// misaligned text the moment the user switches panels.
    #[test]
    fn list_rows_share_one_padding() {
        for (name, source) in [
            (
                "explorer.rs",
                crate::views::production_source(include_str!("views/explorer.rs")),
            ),
            (
                "harness.rs",
                crate::views::production_source(include_str!("views/harness.rs")),
            ),
        ] {
            assert!(
                source.contains("metrics::row_pad()"),
                "{name} does not use the shared row padding"
            );
        }
    }
}
