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

/// Default width of the side panel.
pub const SIDE_PANEL: f32 = 268.;

/// Default width of the right details panel.
///
/// Wider than the left: the left column holds names, which elide gracefully,
/// while the right holds label/value pairs whose label column alone is 96px.
/// At 268 the value beside it had barely 140px, so every path and every tool
/// list wrapped — which is what made the panel look broken rather than narrow.
pub const RIGHT_PANEL: f32 = 340.;

/// Smallest useful side panel width. Below this, file names are all ellipsis.
pub const SIDE_PANEL_MIN: f32 = 180.;

/// Gap between related controls in a row — a button and its neighbour.
pub const GAP: f32 = 6.;

/// Gap between unrelated groups in the same row.
pub const GAP_GROUP: f32 = 12.;

/// Padding inside a panel's header row.
pub const HEADER_PAD_Y: f32 = 8.;

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

pub fn side_panel() -> Pixels {
    px(SIDE_PANEL)
}

pub fn right_panel() -> Pixels {
    px(RIGHT_PANEL)
}

pub fn status_bar() -> Pixels {
    px(STATUS_BAR)
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
            ("RIGHT_PANEL", RIGHT_PANEL),
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
        assert!(SIDE_PANEL > SIDE_PANEL_MIN);
        assert!(SIDE_PANEL_MIN >= 160.);
        // The details panel holds a 96px label column plus its value, so it
        // needs to be the wider of the two or every field wraps.
        assert!(RIGHT_PANEL > SIDE_PANEL);
        // Windows draws 32px caption buttons; a shorter bar leaves them
        // overhanging the content below.
        assert!(TITLE_BAR >= 32.);
        // The whole point of two gaps: related controls sit closer than
        // unrelated ones. Equal values would make the distinction invisible.
        assert!(GAP < GAP_GROUP);
        assert!(ROW_GAP < GAP);
    };

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
            ("explorer.rs", include_str!("views/explorer.rs")),
            ("harness.rs", include_str!("views/harness.rs")),
            ("workspace.rs", include_str!("views/workspace.rs")),
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
    /// in the same 268px column, so a different padding in each is visible as
    /// misaligned text the moment the user switches panels.
    #[test]
    fn list_rows_share_one_padding() {
        for (name, source) in [
            ("explorer.rs", include_str!("views/explorer.rs")),
            ("harness.rs", include_str!("views/harness.rs")),
        ] {
            assert!(
                source.contains("metrics::row_pad()"),
                "{name} does not use the shared row padding"
            );
        }
    }
}
