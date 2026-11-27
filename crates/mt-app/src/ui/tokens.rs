//! Owner lane: foundation.
//!
//! The Afterglow design tokens: the palette of both themes, the spacing and
//! type scale, the layout numbers and the family names.
//!
//! Everything visual reads from here, so a surface never spells a colour or a
//! distance out again. [`palette`] follows the effective appearance: nsh
//! resolves `system` itself, and the settings lane applies an explicit
//! `light` / `dark` choice with `nsh_core::element::set_color_scheme`, which
//! this reads back as [`nsh_core::element::dark`].
//!
//! The colour values are the spec's, including their alphas: `ink` at .84 is
//! one colour with that alpha, not a pre-blended grey, so it sits correctly on
//! both the room and the column ground.

use nsh_core::{
    Color,
    element::{AdaptiveColor, FontWeight},
    prelude::*,
};

pub fn mono_count<T: Styled>(element: T) -> T {
    element.font_family(FONT_SOURCE)
}

/// `0xRRGGBB`, opaque.
const fn hex(rgb: u32) -> Color {
    Color::rgba((rgb >> 16) as u8, (rgb >> 8) as u8, rgb as u8, 0xff)
}

/// `0xRRGGBB` at `alpha` (0..=255).
const fn hexa(rgb: u32, alpha: u8) -> Color {
    Color::rgba((rgb >> 16) as u8, (rgb >> 8) as u8, rgb as u8, alpha)
}

// The spec's alphas, as 8-bit.
const A_84: u8 = 214;
const A_88: u8 = 224;
const A_62: u8 = 158;
const A_70: u8 = 179;
const A_56: u8 = 143;
const A_34: u8 = 87;
const A_38: u8 = 97;
const A_08: u8 = 20;
const A_10: u8 = 26;
const A_30: u8 = 77;
const A_66: u8 = 168;
const A_54: u8 = 138;
const A_18: u8 = 46;
const A_22: u8 = 56;
const A_16: u8 = 41;
const A_12: u8 = 31;
const A_60: u8 = 153;
const A_06: u8 = 15;
const A_08P: u8 = 20;
const A_04: u8 = 10;
const A_03: u8 = 8;

/// The Afterglow palette of one appearance.
///
/// Field names say what the colour is for; a surface that needs "the model's
/// current words" asks for [`Palette::hot`] and never for a hex.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Palette {
    /// The room: the window ground behind everything.
    pub room: Color,
    /// The text column's ground: the room itself in both appearances, so no
    /// sheet shows under the text.
    pub column_ground: Color,
    /// A floating card's ground (the consent card): the room in light, lifted
    /// a hair off it in dark, where a shadow alone does not show on graphite.
    pub card_ground: Color,
    /// Body ink: your text, labels at full attention.
    pub ink: Color,
    /// Resting ink: quiet local actions, captions (steps to [`Palette::ink`]
    /// on hover).
    pub ink_rest: Color,
    /// Caption ink: the quietest text that must still be read (hints,
    /// disclosures, secondary lines), at least 4.5:1 on every ground. Also a
    /// control's resting border, which needs 3:1.
    pub ink_caption: Color,
    /// Non-text ink: rules, dots, disabled states. Never text: it is about
    /// 2.2:1 (light) and 2.6:1 (dark) on the room.
    pub ink_faint: Color,
    /// The 1px hairline.
    pub hairline: Color,
    /// The one HOT item's glyphs (model output).
    pub hot: Color,
    /// Its tight under-bloom plate.
    pub hot_bloom: Color,
    /// Its halo plate, capped at 6%, 24px.
    pub halo: Color,
    /// Ember: model output that has cooled.
    pub ember: Color,
    /// The ember's under-bloom.
    pub ember_bloom: Color,
    /// Inference phosphor: the haze beside words the model read into it.
    pub haze: Color,
    /// The 2px gutter edge of an inference.
    pub haze_edge: Color,
    /// Added words, while they are a hot proposal.
    pub added: Color,
    /// The emitter plate under added words (light theme: brighter than the
    /// room; dark theme: the bloom carries it, same as [`Palette::room`]).
    pub added_plate: Color,
    /// Removed words: the cold strike.
    pub removed: Color,
    /// Lines that differ from disk: the dashed warm-grey underline.
    pub conflict: Color,
    /// Outbound data: white-hot, always with the filament.
    pub leaving: Color,
    /// The filament's ramp, dull to white-hot.
    pub filament: [Color; 3],
    /// A loopback endpoint's filament: plain grey, no heat claim.
    pub loopback: Color,
    /// Stale emission: colourless ink, still readable.
    pub afterimage_ink: Color,
    /// The ghost plate under stale emission.
    pub afterimage_plate: Color,
    /// Destructive intent: a failed save, an irreversible loss.
    pub danger: Color,
    /// The stale label's text.
    pub stale_label: Color,
    /// The ground the stale label sits on.
    pub stale_ground: Color,
    /// An emitter's plate in light theme (`#FFFDF7`); dark theme has none —
    /// the bloom is the light — so this is the room there.
    pub emitter_plate: Color,
    /// Whether this is the dark appearance (for the few places that must
    /// branch, e.g. under-bloom geometry).
    pub is_dark: bool,
}

/// The dark appearance: warm graphite, amber model light.
pub const DARK: Palette = Palette {
    room: hex(0x101113),
    column_ground: hex(0x101113),
    card_ground: hex(0x15161A),
    ink: hexa(0xD9DCE1, A_84),
    ink_rest: hexa(0xD9DCE1, A_56),
    ink_caption: hexa(0xD9DCE1, A_54),
    ink_faint: hexa(0xD9DCE1, A_34),
    hairline: hexa(0xD9DCE1, A_08),
    hot: hex(0xFFB36B),
    hot_bloom: hexa(0xFF8A3D, A_30),
    halo: hexa(0xFF8A3D, A_06),
    ember: hex(0xC98A5A),
    ember_bloom: hexa(0x7A4A2A, A_18),
    haze: hexa(0x8FBFB6, A_16),
    haze_edge: hexa(0x8FBFB6, A_60),
    added: hex(0xFFD9A8),
    added_plate: hex(0x101113),
    removed: hex(0x8FA9C8),
    conflict: hex(0xA39B8E),
    leaving: hex(0xFFF1D6),
    filament: [hex(0x5A1E10), hex(0xFF6A2A), hex(0xFFF1D6)],
    loopback: hex(0x6B6F76),
    afterimage_ink: hexa(0xD9DCE1, A_62),
    afterimage_plate: hexa(0xFFFFFF, A_04),
    danger: hex(0xFF6B5E),
    stale_label: hex(0xD9DCE1),
    stale_ground: hex(0x101113),
    emitter_plate: hex(0x101113),
    is_dark: true,
};

/// The light appearance: warm bone, an emitter is a plate brighter than the
/// room plus saturated glyphs.
pub const LIGHT: Palette = Palette {
    room: hex(0xF2F0EB),
    column_ground: hex(0xF2F0EB),
    card_ground: hex(0xF2F0EB),
    ink: hexa(0x23262B, A_88),
    ink_rest: hexa(0x23262B, A_70),
    ink_caption: hexa(0x23262B, A_66),
    ink_faint: hexa(0x23262B, A_38),
    hairline: hexa(0x23262B, A_10),
    hot: hex(0xB04E00),
    // The prototype's bloom peaks at .30 inside a gradient; a stated finding
    // draws it as one flat tint under the glyphs (`glow::stated_decoration`),
    // where .30 would leave HOT at 4.41:1 on the plate. .22 keeps it above
    // 4.5:1.
    hot_bloom: hexa(0xFFB04D, A_22),
    halo: hexa(0xFFB04D, A_06),
    ember: hex(0x8D6240),
    ember_bloom: hexa(0xB88458, A_08P),
    haze: hexa(0x4E8C84, A_12),
    haze_edge: hex(0x4E8C84),
    added: hex(0x7A3A00),
    added_plate: hex(0xFFF3E0),
    removed: hex(0x4F6E93),
    conflict: hex(0x8A8174),
    leaving: hex(0xB8370C),
    filament: [hex(0xE9D7C9), hex(0xF07A2B), hex(0xC13A0E)],
    loopback: hex(0x6B6F76),
    afterimage_ink: hexa(0x23262B, A_70),
    afterimage_plate: hexa(0x000000, A_03),
    danger: hex(0xC2372B),
    stale_label: hex(0x23262B),
    stale_ground: hex(0xF7F5F1),
    emitter_plate: hex(0xFFFDF7),
    is_dark: false,
};

/// The palette of the effective appearance (system, or what the settings
/// lane set with `set_color_scheme`). Reading it subscribes, so a surface
/// that calls it inside a style closure repaints on a theme change.
pub fn palette() -> &'static Palette {
    if dark() { &DARK } else { &LIGHT }
}

pub(crate) fn adaptive_color(pick: impl Fn(&Palette) -> Color) -> AdaptiveColor {
    AdaptiveColor::new(pick(&LIGHT), pick(&DARK))
}

impl Palette {
    /// The room at 70%: what dims everything under the consent card, because
    /// the room dims to 30% while something is about to leave the device.
    pub fn scrim(self) -> Color {
        with_alpha(self.room, 0.70)
    }

    /// The same ink at another alpha, for the few steps the spec leaves to the
    /// surface (a disabled label, a hairline under a plate).
    pub fn ink_at(self, alpha: f32) -> Color {
        with_alpha(self.ink, alpha)
    }

    /// The Shoji paper: the toned panel ground the sidebar and the drop
    /// screen share, opaque so it covers what it slides over. It is the
    /// prototype's `--shoji-paper-solid` — the dark room's card tone, and the
    /// light room under one [`Palette::afterimage_plate`] layer.
    pub fn shoji_paper(self) -> Color {
        if self.is_dark {
            hex(0x15161A)
        } else {
            hex(0xEAE8E3)
        }
    }
}

/// `color` at `alpha` (0..=1).
pub fn with_alpha(color: Color, alpha: f32) -> Color {
    let (r, g, b, _) = color.to_f64();
    Color::rgba(
        (r * 255.0).round() as u8,
        (g * 255.0).round() as u8,
        (b * 255.0).round() as u8,
        (alpha.clamp(0.0, 1.0) * 255.0).round() as u8,
    )
}

// ─── layout ───────────────────────────────────────────────────────────────

// Distances are plain points, as `px(…)` and `Points` both take them; nothing
// here is a `Points` until a surface asks for one.

/// The 4px scale: 4, 8, 16, 24, 40, 64, 88.
pub const SPACE_1: f32 = 4.0;
pub const SPACE_2: f32 = 8.0;
pub const SPACE_3: f32 = 16.0;
pub const SPACE_4: f32 = 24.0;
pub const SPACE_XL: f32 = SPACE_4;
pub const SELECTION_UNDERLINE: f32 = SPACE_1 / 2.0;
pub const SPACE_5: f32 = 40.0;
pub const SPACE_6: f32 = 64.0;
pub const SPACE_7: f32 = 88.0;

/// The opaque title row.
pub const TITLE_ROW_H: f32 = 32.0;
pub const SETTINGS_HEADER_H: f32 = SPACE_4 + SPACE_2 + SPACE_1;
/// The rail: a 44px strip that widens to 240px on hover.
pub const RAIL_W: f32 = 44.0;
pub const RAIL_HOVER_W: f32 = 240.0;
/// The rail's one-dot-per-file mark.
pub const RAIL_DOT: f32 = 6.0;
/// The gap between the rail and the text column.
pub const COLUMN_GAP: f32 = 40.0;
/// The gap between the text column and the field.
pub const FIELD_GAP: f32 = 40.0;
/// The client width below which the field slides over the column.
pub const NARROW_MAX_W: f32 = 1040.0;
/// The client width at and above which the measure is the wide one.
pub const WIDE_MIN_W: f32 = 1280.0;
/// The text measure: 680 wide, 560 from 1040 to 1279.
pub const MEASURE_WIDE: f32 = 680.0;
pub const MEASURE_NARROW: f32 = 560.0;
/// The field's width at the 1068 client budget (44+40+560+40+352+32).
pub const FIELD_MIN_W: f32 = 352.0;
/// The field's inset from the window's right edge.
pub const FIELD_RIGHT_INSET: f32 = 32.0;
/// The docked reading position's widest column: model text is measured, not
/// stretched across a wide window.
pub const FIELD_READING_MAX_W: f32 = 440.0;
/// The consent card's width, and the range it may use.
pub const CONSENT_CARD_MIN_W: f32 = 520.0;
pub const CONSENT_CARD_MAX_W: f32 = 640.0;
/// The consent card's controls.
pub const CONSENT_BUTTON_W: f32 = 164.0;
pub const CONSENT_BUTTON_H: f32 = 36.0;
/// The context strip that expands the title row's path.
pub const CONTEXT_STRIP_H: f32 = 56.0;
/// Hairlines are 1px; the radius is 0 everywhere in this direction.
pub const HAIRLINE: f32 = 1.0;
/// The focus ring: a designed 1px ink ring, offset from the control.
pub const FOCUS_RING: f32 = 1.0;
pub const FOCUS_RING_OFFSET: f32 = 2.0;

/// The type scale: 12.5, 14, 16, 22 (with their line heights).
pub const TYPE_UI: f32 = 12.5;
pub const TYPE_UI_LINE: f32 = 18.0;
pub const TYPE_SOURCE: f32 = 14.0;
pub const TYPE_SOURCE_LINE: f32 = 24.0;
pub const TYPE_PREVIEW: f32 = 16.0;
pub const TYPE_PREVIEW_LINE: f32 = 27.0;
pub const TYPE_MODEL: f32 = 22.0;
pub const TYPE_MODEL_LINE: f32 = 30.0;
/// zh-CN sets at 1.75 line height with no letter spacing.
pub const CJK_LINE_FACTOR: f32 = 1.75;
/// Markdown punctuation in source mode sits at 34% ink.
pub const PUNCTUATION_INK: Color = hexa(0xD9DCE1, A_34);

/// How the window is laid out at one client width: the spec's width
/// contract, resolved once by the shell.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Layout {
    /// Client width 1280 and above: measure 680, field in the row.
    Wide,
    /// 1040 to 1279: measure 560, field in the row.
    Medium,
    /// Below 1040: measure 560, the field slides over the column.
    Narrow,
}

impl Layout {
    /// The layout at `client_width`.
    pub fn of(client_width: f32) -> Layout {
        if client_width >= WIDE_MIN_W {
            Layout::Wide
        } else if client_width >= NARROW_MAX_W {
            Layout::Medium
        } else {
            Layout::Narrow
        }
    }

    /// The text column's measure.
    pub fn measure(self) -> f32 {
        match self {
            Layout::Wide => MEASURE_WIDE,
            Layout::Medium | Layout::Narrow => MEASURE_NARROW,
        }
    }

    /// Whether the field is an overlay over the column (below 1040).
    pub fn field_overlays(self) -> bool {
        matches!(self, Layout::Narrow)
    }

    /// The field's left edge at `client_width`: where the overlay begins
    /// below 1040, and the field's own column above it.
    pub fn field_left(self, client_width: f32) -> f32 {
        if self.field_overlays() {
            client_width * (1.0 - Self::OVERLAY_FRACTION)
        } else {
            RAIL_W + COLUMN_GAP + self.measure() + FIELD_GAP
        }
    }

    /// How wide the column draws at `client_width`: the band the overlay
    /// leaves visible below 1040, its whole box when the field sits beside it.
    pub fn column_width(self, client_width: f32) -> f32 {
        if self.field_overlays() {
            (self.field_left(client_width) - RAIL_W).max(0.0)
        } else {
            self.measure() + 2.0 * COLUMN_GAP
        }
    }

    /// How much the field takes of the width when it overlays the column.
    pub const OVERLAY_FRACTION: f32 = 0.46;

    /// What the column's text dims to while the field overlays it.
    pub const DIMMED: f32 = 0.4;

    /// What the room dims to behind the consent card.
    pub const SCRIM: f32 = 0.3;
}

// ─── type ─────────────────────────────────────────────────────────────────

// DirectWrite — the engine every surface on Windows draws through — exposes
// each embedded face under its file's *legacy* family name (name table ID 1,
// the name `FindFamilyName` matches), and the three Grotesk instances carry
// one each (`Martian Grotesk Std Lt` / `Std Rg` / `Std Md`), so a family
// names the weight it draws and `FontWeight` alone cannot pick an instance.
// The typographic names (nameID 16: `Martian Grotesk`, `Martian Mono`) follow
// in every list as the second family, for platforms that register the faces
// under those; they are fallbacks, never the first entry.

// After Martian, each platform falls back to the faces it ships that sit
// closest to it: a clean, geometric-leaning CJK sans first — so CJK
// characters missing from Martian, such as `―`, are drawn by the CJK face,
// as Chinese text sets them — then the platform's own neo-grotesk UI face for
// what neither has, then the generic family. Displayed text takes a character
// the first family lacks from the next family that has it. Windows text inputs
// resolve only the first family from system fonts; see `FONT_INPUT`.
//
// * Windows: DengXian — geometric, low-contrast and open, the system's
//   closest Simplified Chinese match to Martian — then Microsoft YaHei UI.
//   For Latin, Bahnschrift: its DIN skeleton shares Martian's engineered,
//   squared rounds, where Segoe UI's humanist strokes do not; Segoe UI
//   Variable follows. Code falls back to Cascadia Mono, then Consolas.
// * macOS: PingFang SC, cut in SF's neo-grotesk spirit, then Hiragino Sans
//   GB; for Latin the system face (`system-ui`, SF Pro), whose large x-height
//   sits with Martian's. Code falls back to the system monospace
//   (`ui-monospace`, SF Mono, squared and technical like Martian Mono), then
//   Menlo.
// * Linux: Noto Sans CJK SC (or Source Han Sans SC, WenQuanYi Micro Hei),
//   then Inter / Adwaita Sans where installed and Noto Sans; code falls back
//   to Adwaita Mono, then DejaVu Sans Mono.
//
// Ahead of every platform's CJK face sits the companion the settings page
// downloads on request (`ui::fonts::Companion`): Source Han Sans CN VF. It is
// one variable file without a typographic name, so both its legacy family
// (nameID 1) and what `register_font` resolves are `Source Han Sans CN VF`;
// DirectWrite picks its named instance by weight (300 / 400 / 500 draw the
// Light / Regular / Medium cuts), so the list's weight selects the cut. Until
// it is installed the name matches nothing and the platform's face follows.
macro_rules! cjk_companion {
    () => {
        "Source Han Sans CN VF"
    };
}
#[cfg(target_os = "windows")]
macro_rules! sans_fallback {
    () => {
        "DengXian, Microsoft YaHei UI, Bahnschrift, Segoe UI Variable Text, sans-serif"
    };
}
#[cfg(target_os = "windows")]
macro_rules! light_fallback {
    () => {
        "DengXian Light, DengXian, Microsoft YaHei UI, Bahnschrift, Segoe UI Variable Display, sans-serif"
    };
}
#[cfg(target_os = "windows")]
macro_rules! mono_fallback {
    () => {
        "DengXian, Microsoft YaHei UI, Cascadia Mono, Consolas, monospace"
    };
}
#[cfg(target_os = "macos")]
macro_rules! sans_fallback {
    () => {
        "PingFang SC, Hiragino Sans GB, system-ui, sans-serif"
    };
}
#[cfg(not(target_os = "windows"))]
macro_rules! light_fallback {
    () => {
        sans_fallback!()
    };
}
#[cfg(target_os = "macos")]
macro_rules! mono_fallback {
    () => {
        "PingFang SC, Hiragino Sans GB, ui-monospace, Menlo, monospace"
    };
}
#[cfg(not(any(target_os = "windows", target_os = "macos")))]
macro_rules! sans_fallback {
    () => {
        "Noto Sans CJK SC, Source Han Sans SC, WenQuanYi Micro Hei, Inter, Adwaita Sans, Noto Sans, sans-serif"
    };
}
#[cfg(not(any(target_os = "windows", target_os = "macos")))]
macro_rules! mono_fallback {
    () => {
        "Noto Sans Mono CJK SC, Noto Sans CJK SC, Adwaita Mono, DejaVu Sans Mono, monospace"
    };
}

/// Martian Grotesk 400 (the `Std Rg` instance): UI and preview body.
pub const FONT_UI: &str = concat!(
    "Martian Grotesk Std Rg, Martian Grotesk, ",
    cjk_companion!(),
    ", ",
    sans_fallback!()
);
/// Martian Grotesk 300 (the `Std Lt` instance): the model's voice, the only
/// large type in the app.
pub const FONT_MODEL: &str = concat!(
    "Martian Grotesk Std Lt, Martian Grotesk, ",
    cjk_companion!(),
    ", ",
    light_fallback!()
);
/// Martian Grotesk 500 (the `Std Md` instance): the few labels that must not
/// be quiet. Pair it with [`WEIGHT_MEDIUM`].
pub const FONT_MEDIUM: &str = concat!(
    "Martian Grotesk Std Md, Martian Grotesk, ",
    cjk_companion!(),
    ", ",
    sans_fallback!()
);
/// Martian Mono 400 (the condensed `Cn Rg` instance): source mode. CJK falls
/// back to the companion, then the platform's sans, without forcing
/// monospace widths.
pub const FONT_SOURCE: &str = concat!(
    "Martian Mono Cn Rg, Martian Mono, ",
    cjk_companion!(),
    ", ",
    mono_fallback!()
);
/// The downloadable companion CJK face's family, first in the CJK position of
/// every list above.
pub const FONT_CJK_COMPANION: &str = cjk_companion!();
/// Text inputs: the UI list. Every platform's input resolves the whole list;
/// nsh 0.7.1's Windows input finds registered faces too (its
/// `text::resolve_family` walks the list and the registered collection).
pub const FONT_INPUT: &str = FONT_UI;

/// The model's voice: Martian Grotesk 300, the only large type in the app.
pub const WEIGHT_MODEL: FontWeight = FontWeight(300);
/// UI and preview body.
pub const WEIGHT_UI: FontWeight = FontWeight(400);
/// The few labels that must not be quiet.
pub const WEIGHT_MEDIUM: FontWeight = FontWeight(500);

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CjkOptics {
    size_step: f32,
    weight_step: u16,
}

impl CjkOptics {
    pub const fn size(self, base: f32, cjk: bool) -> f32 {
        if cjk { base + self.size_step } else { base }
    }

    pub const fn weight(self, base: FontWeight, cjk: bool) -> FontWeight {
        if cjk {
            FontWeight(base.0 + self.weight_step)
        } else {
            base
        }
    }

    pub const fn changes_weight(self) -> bool {
        self.weight_step != 0
    }

    pub const fn is_unchanged(self) -> bool {
        self.size_step == 0.0 && self.weight_step == 0
    }
}

const CJK_OPTICS_BASELINE: CjkOptics = CjkOptics {
    size_step: 0.0,
    weight_step: 0,
};

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CjkOpticsVariant {
    pub label: &'static str,
    pub id: &'static str,
    pub optics: CjkOptics,
}

/// The owner's pick from the 2026-11-23 Goal 17 capture: the comparison's
/// third column, "+1px / next companion weight".
pub const CJK_OPTICS: CjkOptics = CJK_OPTICS_CAPTURE_VARIANTS[2].optics;

pub const CJK_OPTICS_CAPTURE_VARIANTS: [CjkOpticsVariant; 3] = [
    CjkOpticsVariant {
        label: "+0",
        id: "cjk-optics-plus-0",
        optics: CJK_OPTICS_BASELINE,
    },
    CjkOpticsVariant {
        label: "+1px",
        id: "cjk-optics-plus-1px",
        optics: CjkOptics {
            size_step: 1.0,
            weight_step: 0,
        },
    },
    CjkOpticsVariant {
        label: "+1px / next companion weight",
        id: "cjk-optics-plus-1px-next-weight",
        optics: CjkOptics {
            size_step: 1.0,
            weight_step: 100,
        },
    },
];

/// The app's nsh theme (`theme.json`): the root carries it in the app and in
/// any probe that must draw with the app's own fonts and colours.
pub fn theme() -> nsh_core::element::Tokens {
    nsh_base::tokens_of(Some(include_str!("../../theme.json")))
        .expect("theme.json is a valid nsh theme")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// nsh's own controls (inputs, inline code) draw with the theme's
    /// `font-sans` / `font-mono`; they must lead with the app's Martian faces.
    #[test]
    fn the_theme_fonts_are_the_app_lists() {
        use nsh_core::element::{FONT_MONO, FONT_SANS};
        let theme = theme();
        // nsh keeps a font token's first family only.
        let first = |list: &'static str| list.split(',').next().map(str::trim);
        assert_eq!(
            theme.font(FONT_SANS).map(|font| font.as_ref()),
            first(FONT_UI)
        );
        assert_eq!(
            theme.font(FONT_MONO).map(|font| font.as_ref()),
            first(FONT_SOURCE)
        );
    }

    #[test]
    fn layout_breakpoints_follow_the_spec() {
        assert_eq!(Layout::of(1440.0), Layout::Wide);
        assert_eq!(Layout::of(1280.0), Layout::Wide);
        assert_eq!(Layout::of(1279.0), Layout::Medium);
        assert_eq!(Layout::of(1040.0), Layout::Medium);
        assert_eq!(Layout::of(1039.0), Layout::Narrow);
        assert_eq!(Layout::of(1100.0).measure(), MEASURE_NARROW);
        assert!(!Layout::of(1100.0).field_overlays());
        assert!(Layout::of(1000.0).field_overlays());
    }

    #[test]
    fn the_1068_budget_adds_up() {
        let used = RAIL_W + COLUMN_GAP + MEASURE_NARROW + FIELD_GAP + FIELD_MIN_W + 2.0 * SPACE_3;
        assert_eq!(used, 1068.0);
    }

    #[test]
    fn the_family_lists_name_the_legacy_families_first() {
        // What the text engine resolves on Windows: the file's legacy name
        // (name table ID 1), then the typographic one, then the platform's
        // own fallbacks. The resolution itself is proven through the real
        // engine in `document::tests::the_ui_and_model_lists_resolve_to_martian`.
        let cjk = if cfg!(target_os = "windows") {
            "DengXian"
        } else if cfg!(target_os = "macos") {
            "PingFang SC"
        } else {
            "Noto Sans CJK SC"
        };
        for (list, legacy, typographic) in [
            (FONT_UI, "Martian Grotesk Std Rg", "Martian Grotesk"),
            (FONT_MODEL, "Martian Grotesk Std Lt", "Martian Grotesk"),
            (FONT_MEDIUM, "Martian Grotesk Std Md", "Martian Grotesk"),
            (FONT_SOURCE, "Martian Mono Cn Rg", "Martian Mono"),
        ] {
            let families: Vec<&str> = list.split(',').map(str::trim).collect();
            assert_eq!(families[0], legacy, "{list}");
            assert_eq!(families[1], typographic, "{list}");
            assert!(families.contains(&cjk), "{list}");
            // The downloadable companion follows Martian directly, then the
            // platform's own CJK face: they draw the CJK punctuation Martian
            // lacks, before any Latin system face.
            assert_eq!(families[2], FONT_CJK_COMPANION, "{list}");
            assert_eq!(families[2], "Source Han Sans CN VF", "{list}");
            assert_eq!(
                families
                    .iter()
                    .filter(|family| **family == FONT_CJK_COMPANION)
                    .count(),
                1,
                "{list}"
            );
            let is_cjk = |family: &str| {
                [
                    "DengXian",
                    "PingFang SC",
                    "Noto Sans CJK SC",
                    "Noto Sans Mono CJK SC",
                ]
                .iter()
                .any(|face| family.starts_with(face))
            };
            assert!(is_cjk(families[3]), "{list}");
        }
        // The model's 22px CJK follows the light cut before the body's.
        if cfg!(target_os = "windows") {
            assert!(
                FONT_MODEL.contains("DengXian Light, DengXian"),
                "{FONT_MODEL}"
            );
        }
    }

    #[test]
    fn the_font_roles_have_exact_ordered_fallbacks() {
        #[cfg(target_os = "windows")]
        let (sans, model, mono) = (
            "Source Han Sans CN VF, DengXian, Microsoft YaHei UI, Bahnschrift, Segoe UI Variable Text, sans-serif",
            "Source Han Sans CN VF, DengXian Light, DengXian, Microsoft YaHei UI, Bahnschrift, Segoe UI Variable Display, sans-serif",
            "Source Han Sans CN VF, DengXian, Microsoft YaHei UI, Cascadia Mono, Consolas, monospace",
        );
        #[cfg(target_os = "macos")]
        let (sans, model, mono) = (
            "Source Han Sans CN VF, PingFang SC, Hiragino Sans GB, system-ui, sans-serif",
            "Source Han Sans CN VF, PingFang SC, Hiragino Sans GB, system-ui, sans-serif",
            "Source Han Sans CN VF, PingFang SC, Hiragino Sans GB, ui-monospace, Menlo, monospace",
        );
        #[cfg(not(any(target_os = "windows", target_os = "macos")))]
        let (sans, model, mono) = (
            "Source Han Sans CN VF, Noto Sans CJK SC, Source Han Sans SC, WenQuanYi Micro Hei, Inter, Adwaita Sans, Noto Sans, sans-serif",
            "Source Han Sans CN VF, Noto Sans CJK SC, Source Han Sans SC, WenQuanYi Micro Hei, Inter, Adwaita Sans, Noto Sans, sans-serif",
            "Source Han Sans CN VF, Noto Sans Mono CJK SC, Noto Sans CJK SC, Adwaita Mono, DejaVu Sans Mono, monospace",
        );

        for (role, list, expected) in [
            ("FONT_UI", FONT_UI, sans),
            ("FONT_MODEL", FONT_MODEL, model),
            ("FONT_MEDIUM", FONT_MEDIUM, sans),
            ("FONT_SOURCE", FONT_SOURCE, mono),
        ] {
            assert_eq!(list.splitn(3, ", ").nth(2), Some(expected), "{role}");
        }
    }

    /// nsh 0.7.1's Windows input resolves the whole family list and finds
    /// registered faces (nsh-platform-windows `text::resolve_family`), so the
    /// input list is the UI list on every platform, Martian first.
    #[test]
    fn the_input_list_is_the_ui_list() {
        assert_eq!(FONT_INPUT, FONT_UI);
        assert_eq!(
            FONT_INPUT.split(',').next().map(str::trim),
            Some("Martian Grotesk Std Rg"),
            "{FONT_INPUT}"
        );
    }

    /// WCAG contrast of `ink` drawn over the opaque `ground` (its alpha
    /// composited first, as the screen shows it).
    fn contrast(ink: Color, ground: Color) -> f32 {
        let ground = Hsla::from(ground);
        ground.blend(Hsla::from(ink)).contrast_ratio(ground)
    }

    #[test]
    fn light_ember_keeps_its_lightness_contrast_and_warmth() {
        let (r, g, b, _) = LIGHT.ember.to_f64();
        let linear = |v: f64| {
            if v <= 0.04045 {
                v / 12.92
            } else {
                ((v + 0.055) / 1.055).powf(2.4)
            }
        };
        let (r, g, b) = (linear(r), linear(g), linear(b));
        let l = (0.4122214708 * r + 0.5363325363 * g + 0.0514459929 * b).cbrt();
        let m = (0.2119034982 * r + 0.6806995451 * g + 0.1073969566 * b).cbrt();
        let s = (0.0883024619 * r + 0.2817188376 * g + 0.6299787005 * b).cbrt();
        let lightness = 0.2104542553 * l + 0.7936177850 * m - 0.0040720468 * s;
        let a = 1.9779984951 * l - 2.4285922050 * m + 0.4505937099 * s;
        let b = 0.0259040371 * l + 0.7827717662 * m - 0.8086757660 * s;
        let chroma = a.hypot(b);
        let ratio = contrast(LIGHT.ember, LIGHT.room);
        assert!(
            (lightness - 0.5354).abs() <= 0.02 && chroma >= 0.06 && ratio >= 4.5,
            "lightness {lightness}, chroma {chroma}, contrast {ratio}"
        );
    }

    /// `layer` composited over the opaque `ground`.
    fn over(ground: Color, layer: Color) -> Color {
        Hsla::from(ground).blend(Hsla::from(layer)).into()
    }

    /// The grounds text sits on: the room, the column, the cards (room) and
    /// the light emitter plate.
    fn grounds(p: &Palette) -> [(&'static str, Color); 3] {
        [
            ("room", p.room),
            ("column", p.column_ground),
            ("emitter plate", p.emitter_plate),
        ]
    }

    #[test]
    fn every_text_ink_reads_at_4_5_to_1_on_every_ground() {
        for p in [&LIGHT, &DARK] {
            for (ink_name, ink) in [
                ("ink", p.ink),
                ("ink_rest", p.ink_rest),
                ("ink_caption", p.ink_caption),
            ] {
                for (ground_name, ground) in grounds(p) {
                    let ratio = contrast(ink, ground);
                    assert!(
                        ratio >= 4.5,
                        "{ink_name} on {ground_name} (dark: {}) is {ratio:.2}:1",
                        p.is_dark
                    );
                }
            }
        }
    }

    #[test]
    fn a_controls_resting_border_reaches_3_to_1() {
        // `prims::primary_action` rests on `ink_caption` (WCAG 1.4.11);
        // `ink_faint` does not reach it and stays for non-text marks.
        for p in [&LIGHT, &DARK] {
            for (ground_name, ground) in grounds(p) {
                let ratio = contrast(p.ink_caption, ground);
                assert!(
                    ratio >= 3.0,
                    "border on {ground_name} (dark: {}) is {ratio:.2}:1",
                    p.is_dark
                );
                assert!(
                    contrast(p.ink_faint, ground) < 3.0,
                    "ink_faint is the non-text step below the border"
                );
            }
        }
    }

    #[test]
    fn model_light_reads_at_4_5_to_1_on_its_plate_and_bloom() {
        for p in [&LIGHT, &DARK] {
            // A stated finding draws its bloom as one flat tint over the
            // plate (the room in dark), the worst ground HOT and ember meet.
            let plate = p.emitter_plate;
            for (name, glyph, ground) in [
                ("hot on room", p.hot, p.room),
                ("hot on plate", p.hot, plate),
                ("hot on bloom", p.hot, over(plate, p.hot_bloom)),
                ("ember on room", p.ember, p.room),
                ("ember on plate", p.ember, plate),
                ("ember on bloom", p.ember, over(plate, p.ember_bloom)),
            ] {
                let ratio = contrast(glyph, ground);
                assert!(ratio >= 4.5, "{name} (dark: {}) is {ratio:.2}:1", p.is_dark);
            }
        }
    }

    #[test]
    fn the_column_has_no_sheet() {
        assert_eq!(LIGHT.column_ground, LIGHT.room);
        assert_eq!(DARK.column_ground, DARK.room);
    }
}
