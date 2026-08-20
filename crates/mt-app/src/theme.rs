//! Preset themes.
//!
//! One preset drives two renderers. GPUI's chrome reads `gpui_component::Theme`
//! and the Web preview renders in its own browser context, so a preset that only
//! set one of them would put Split mode in two different themes side by side.
//! [`Preset::tokens`] is therefore the single source: [`apply`] projects it onto
//! GPUI, and [`crate::web`] formats the same values into CSS custom properties.
//!
//! The palettes are transcribed from `marswaveai/ColaMD`'s `themes/*.css`
//! (MIT), which authors twelve of them against a small, stable set of variables
//! — the same set a Markdown document actually needs. Anything a window chrome
//! needs beyond that (hover states, button fills, scrollbar thumbs) is *derived*
//! by mixing towards the foreground or background rather than authored, so
//! adding a preset stays a single row here.

use gpui::{App, Hsla, Window, px, rgb};
use gpui_component::{Theme, ThemeMode};

/// The document colors a preset authors.
///
/// Deliberately the ColaMD variable set: it is what a Markdown document is made
/// of, and every chrome color below is derived from it. `u32` rather than `Hsla`
/// so the table stays a diffable list of hex literals.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Tokens {
    pub bg: u32,
    pub text: u32,
    pub text_secondary: u32,
    pub text_muted: u32,
    pub border: u32,
    pub link: u32,
    /// Inline `code` background.
    pub code_bg: u32,
    /// Fenced code block background.
    pub code_block_bg: u32,
    pub blockquote_border: u32,
    pub table_header_bg: u32,
    pub selection: u32,
    pub highlight: u32,
    /// Bold text and inline code ink. ColaMD sets these per element; folding
    /// them into one accent is what lets the chrome share a preset's identity.
    pub accent: u32,
}

/// Which body font a preset asks for.
///
/// Three of ColaMD's twelve are defined as much by their typeface as their
/// palette — Writer is monospace, Sepia and Elegant are serif — and dropping
/// that would make them near-duplicates of Light.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum BodyFont {
    #[default]
    Sans,
    Serif,
    Mono,
}

impl BodyFont {
    /// The CSS `font-family` stack for the Web preview.
    ///
    /// CJK faces are named ahead of the Latin fallbacks in the serif and mono
    /// stacks: a document mixing Chinese and English otherwise renders the CJK
    /// run in whatever the browser's default is, which is rarely the same face.
    pub fn css(self) -> &'static str {
        match self {
            BodyFont::Sans => {
                "-apple-system, \"Segoe UI\", system-ui, \"PingFang SC\", \
                 \"Noto Sans SC\", sans-serif"
            }
            BodyFont::Serif => {
                "Georgia, \"Source Han Serif SC\", \"Noto Serif SC\", \
                 \"Songti SC\", serif"
            }
            BodyFont::Mono => {
                "\"SF Mono\", \"JetBrains Mono\", \"Cascadia Code\", Consolas, \
                 Menlo, \"PingFang SC\", \"Noto Sans SC\", monospace"
            }
        }
    }

    /// Body line height. Serif and mono presets are reading-first layouts and
    /// ColaMD gives both more leading than its default.
    pub fn line_height(self) -> f32 {
        match self {
            BodyFont::Sans => 1.6,
            BodyFont::Serif => 1.85,
            BodyFont::Mono => 1.8,
        }
    }
}

/// One preset theme.
#[derive(Debug, Clone, Copy)]
pub struct Preset {
    /// Stable id, which is what the settings file stores.
    pub id: &'static str,
    pub name: &'static str,
    pub dark: bool,
    pub font: BodyFont,
    pub tokens: Tokens,
}

const fn p(
    id: &'static str,
    name: &'static str,
    dark: bool,
    font: BodyFont,
    tokens: Tokens,
) -> Preset {
    Preset {
        id,
        name,
        dark,
        font,
        tokens,
    }
}

/// Every preset, light first then dark, each group in the order they are
/// offered.
///
/// Transcribed from ColaMD's `themes/*.css`. The per-element rules that file
/// sets (`strong`, `code`, `h1..h3`) collapse into `accent` here; where a preset
/// sets no such rule, `accent` takes its link color, which is what its own
/// stylesheet falls back to.
// One row per preset: the value of this table is that it can be diffed against
// upstream's CSS at a glance, which rustfmt's four-lines-per-field expansion
// would destroy.
#[rustfmt::skip]
pub const PRESETS: &[Preset] = &[
    // --- Light ---------------------------------------------------------
    p("light", "Light", false, BodyFont::Sans, Tokens {
        bg: 0xffffff, text: 0x24292f, text_secondary: 0x656d76, text_muted: 0x656d76,
        border: 0xd0d7de, link: 0x0969da, code_bg: 0xeef1f4, code_block_bg: 0xf6f8fa,
        blockquote_border: 0xd0d7de, table_header_bg: 0xf6f8fa, selection: 0xcfe3fb,
        highlight: 0xfff3bf, accent: 0x0969da,
    }),
    p("notion", "Notion", false, BodyFont::Sans, Tokens {
        bg: 0xffffff, text: 0x37352f, text_secondary: 0x777674, text_muted: 0x9b9a97,
        border: 0xe9e9e7, link: 0x37352f, code_bg: 0xf7f6f3, code_block_bg: 0xf7f6f3,
        blockquote_border: 0x37352f, table_header_bg: 0xf7f6f3, selection: 0xd5e6f8,
        highlight: 0xfdecc8, accent: 0xeb5757,
    }),
    p("bear", "Bear", false, BodyFont::Sans, Tokens {
        bg: 0xffffff, text: 0x262626, text_secondary: 0x767676, text_muted: 0x8a8a8a,
        border: 0xe8e6e3, link: 0xd43d2a, code_bg: 0xfaeae7, code_block_bg: 0x2e2a28,
        blockquote_border: 0xd43d2a, table_header_bg: 0xf7f5f3, selection: 0xf7dcd8,
        highlight: 0xffe9a8, accent: 0xd43d2a,
    }),
    p("elegant", "Elegant", false, BodyFont::Serif, Tokens {
        bg: 0xf0edea, text: 0x2c2c2c, text_secondary: 0x6c6c6c, text_muted: 0x777777,
        border: 0xd8d3ce, link: 0xbc4424, code_bg: 0xe8e4df, code_block_bg: 0x2c2c2c,
        blockquote_border: 0xc44b2b, table_header_bg: 0xeae6e1, selection: 0xf0cec3,
        highlight: 0xf0d9a8, accent: 0xc44b2b,
    }),
    p("sepia", "Sepia", false, BodyFont::Serif, Tokens {
        bg: 0xf6efe0, text: 0x4f4032, text_secondary: 0x7b6b54, text_muted: 0x9a8a72,
        border: 0xe2d7bf, link: 0x9c5e29, code_bg: 0xe9dcc5, code_block_bg: 0xece2cb,
        blockquote_border: 0xc9a86a, table_header_bg: 0xece2cb, selection: 0xecd6c0,
        highlight: 0xf2dfa8, accent: 0x7a5a33,
    }),
    p("writer", "Writer", false, BodyFont::Mono, Tokens {
        bg: 0xfcfcfa, text: 0x1a1a1a, text_secondary: 0x757571, text_muted: 0x8e8e8a,
        border: 0xe3e3de, link: 0x3b6ec4, code_bg: 0xf0f0ec, code_block_bg: 0xf2f2ee,
        blockquote_border: 0x1a1a1a, table_header_bg: 0xf2f2ee, selection: 0xd8e2f4,
        highlight: 0xfff3a3, accent: 0x3b6ec4,
    }),
    // --- Dark ----------------------------------------------------------
    p("dark", "Dark", true, BodyFont::Sans, Tokens {
        bg: 0x0d1117, text: 0xe6edf3, text_secondary: 0xaab3bd, text_muted: 0x8b949e,
        border: 0x30363d, link: 0x58a6ff, code_bg: 0x2c333c, code_block_bg: 0x161b22,
        blockquote_border: 0x30363d, table_header_bg: 0x161b22, selection: 0x1f3f66,
        highlight: 0x4a3a12, accent: 0x58a6ff,
    }),
    p("midnight", "Midnight", true, BodyFont::Sans, Tokens {
        bg: 0x000000, text: 0xd6d6d6, text_secondary: 0xb1b1b1, text_muted: 0x7a7a7a,
        border: 0x262626, link: 0x0a84ff, code_bg: 0x151515, code_block_bg: 0x111111,
        blockquote_border: 0x3a3a3a, table_header_bg: 0x111111, selection: 0x0a3a72,
        highlight: 0x453a0a, accent: 0x0a84ff,
    }),
    p("nord", "Nord", true, BodyFont::Sans, Tokens {
        bg: 0x2e3440, text: 0xd8dee9, text_secondary: 0xaebdd4, text_muted: 0x7d8ba1,
        border: 0x3b4252, link: 0x88c0d0, code_bg: 0x36404d, code_block_bg: 0x262c37,
        blockquote_border: 0x81a1c1, table_header_bg: 0x3b4252, selection: 0x3f5866,
        highlight: 0x4d4636, accent: 0x8fbcbb,
    }),
    p("gruvbox", "Gruvbox", true, BodyFont::Sans, Tokens {
        bg: 0x282828, text: 0xebdbb2, text_secondary: 0xc5b5a5, text_muted: 0x928374,
        border: 0x3c3836, link: 0x83a598, code_bg: 0x3a3733, code_block_bg: 0x1d2021,
        blockquote_border: 0xfe8019, table_header_bg: 0x3c3836, selection: 0x50382b,
        highlight: 0x4b3f18, accent: 0xfe8019,
    }),
    p("solarized-dark", "Solarized Dark", true, BodyFont::Sans, Tokens {
        bg: 0x002b36, text: 0xc2d5d7, text_secondary: 0xa4bbc3, text_muted: 0x586e75,
        border: 0x0d3d4a, link: 0x3093da, code_bg: 0x12414c, code_block_bg: 0x073642,
        blockquote_border: 0x2aa198, table_header_bg: 0x073642, selection: 0x0d4a70,
        highlight: 0x3a3512, accent: 0x2aa198,
    }),
    p("dracula", "Dracula", true, BodyFont::Sans, Tokens {
        bg: 0x282a36, text: 0xf8f8f2, text_secondary: 0xafb7db, text_muted: 0x7f86a8,
        border: 0x44475a, link: 0xbd93f9, code_bg: 0x363a4b, code_block_bg: 0x21222c,
        blockquote_border: 0xff79c6, table_header_bg: 0x343746, selection: 0x453d5e,
        highlight: 0x45482f, accent: 0xff79c6,
    }),
];

/// The preset used when a setting names one that no longer exists.
pub const DEFAULT_LIGHT: &str = "light";
pub const DEFAULT_DARK: &str = "dark";

/// Look a preset up by id, falling back to the default for `dark`.
///
/// Never panics and never returns `None`: a settings file naming a preset that
/// was renamed must still open a window.
pub fn by_id(id: &str, dark: bool) -> &'static Preset {
    PRESETS
        .iter()
        .find(|p| p.id == id && p.dark == dark)
        .or_else(|| {
            let fallback = if dark { DEFAULT_DARK } else { DEFAULT_LIGHT };
            PRESETS.iter().find(|p| p.id == fallback)
        })
        // The two defaults are in the table and the table is a const, so this is
        // unreachable — but a panic in theme resolution would take the window
        // with it, so fall back to whatever is first for the requested mode.
        .or_else(|| PRESETS.iter().find(|p| p.dark == dark))
        .unwrap_or(&PRESETS[0])
}

/// Presets for one mode, in table order.
pub fn for_mode(dark: bool) -> impl Iterator<Item = &'static Preset> {
    PRESETS.iter().filter(move |p| p.dark == dark)
}

/// Blend two colors channel-wise, `t` of the way from `a` to `b`.
///
/// Chrome states are derived rather than authored, so a preset is one row of
/// document colors instead of forty. sRGB mixing is deliberate: the inputs are
/// hex literals from a stylesheet, so matching what a browser does with the same
/// values matters more than perceptual uniformity.
fn mix(a: u32, b: u32, t: f32) -> u32 {
    let t = t.clamp(0., 1.);
    let ch = |shift: u32| {
        let a = ((a >> shift) & 0xff) as f32;
        let b = ((b >> shift) & 0xff) as f32;
        (a + (b - a) * t).round().clamp(0., 255.) as u32
    };
    (ch(16) << 16) | (ch(8) << 8) | ch(0)
}

impl Preset {
    /// This preset's colors, with chrome states derived.
    ///
    /// Toward the foreground for anything that should stand *out* of the page
    /// (hover, selection, borders) and toward the background for anything that
    /// should recede. Deriving both directions from the same pair is what keeps
    /// contrast sane on Midnight (`#000`) and Sepia alike.
    fn chrome(&self) -> Chrome {
        let t = self.tokens;
        Chrome {
            hover: mix(t.bg, t.text, 0.07),
            active: mix(t.bg, t.text, 0.12),
            subtle: mix(t.bg, t.text, 0.04),
            scrollbar_thumb: mix(t.bg, t.text, 0.18),
            scrollbar_thumb_hover: mix(t.bg, t.text, 0.34),
            accent_hover: mix(t.accent, t.text, 0.15),
            accent_active: mix(t.accent, t.bg, 0.15),
            // Ink that reads on the accent fill. The page background is the one
            // color guaranteed to contrast with the accent in *this* preset —
            // hard-coding white would give Sepia white-on-ochre and Gruvbox
            // white-on-orange.
            on_accent: t.bg,
        }
    }
}

struct Chrome {
    hover: u32,
    active: u32,
    subtle: u32,
    scrollbar_thumb: u32,
    scrollbar_thumb_hover: u32,
    accent_hover: u32,
    accent_active: u32,
    on_accent: u32,
}

/// Apply a preset to GPUI's global theme.
///
/// Called after `Theme::change`, which resets every color to the built-in
/// light/dark config — so this must run *after* it, never before.
pub fn apply(preset: &Preset, window: Option<&mut Window>, cx: &mut App) {
    // Establishing the mode first is what makes every field this does not touch
    // (charts, skeletons) sane rather than a light value on a dark page.
    Theme::change(
        if preset.dark {
            ThemeMode::Dark
        } else {
            ThemeMode::Light
        },
        window,
        cx,
    );

    let t = preset.tokens;
    let c = preset.chrome();
    let h = |value: u32| -> Hsla { rgb(value).into() };

    let theme = Theme::global_mut(cx);

    theme.background = h(t.bg);
    theme.foreground = h(t.text);
    theme.border = h(t.border);
    theme.muted = h(c.subtle);
    theme.muted_foreground = h(t.text_muted);
    theme.secondary = h(c.subtle);
    theme.secondary_foreground = h(t.text);
    theme.secondary_hover = h(c.hover);
    theme.secondary_active = h(c.active);

    theme.accent = h(c.hover);
    theme.accent_foreground = h(t.text);
    theme.primary = h(t.accent);
    theme.primary_hover = h(c.accent_hover);
    theme.primary_active = h(c.accent_active);
    theme.primary_foreground = h(c.on_accent);

    theme.button = h(c.subtle);
    theme.button_foreground = h(t.text);
    theme.button_hover = h(c.hover);
    theme.button_active = h(c.active);
    theme.button_primary = h(t.accent);
    theme.button_primary_hover = h(c.accent_hover);
    theme.button_primary_active = h(c.accent_active);
    theme.button_primary_foreground = h(c.on_accent);

    theme.link = h(t.link);
    theme.link_hover = h(mix(t.link, t.text, 0.25));
    theme.link_active = h(mix(t.link, t.bg, 0.2));

    theme.input = h(t.border);
    theme.ring = h(t.accent);
    theme.caret = h(t.text);
    theme.selection = h(t.selection);

    theme.popover = h(mix(t.bg, t.text, 0.03));
    theme.popover_foreground = h(t.text);

    // Note: no `theme.list` — `Theme` has its own `list: ListSettings` field
    // which shadows `ThemeColor::list` through the `Deref`, so assigning a color
    // there does not compile. The list *surface* is the page background anyway;
    // the states below are what actually read.
    theme.list_hover = h(c.hover);
    theme.list_active = h(c.active);
    theme.list_active_border = h(t.accent);
    theme.list_head = h(t.table_header_bg);
    theme.list_even = h(c.subtle);

    theme.table = h(t.bg);
    theme.table_head = h(t.table_header_bg);
    theme.table_head_foreground = h(t.text_secondary);
    theme.table_hover = h(c.hover);
    theme.table_active = h(c.active);
    theme.table_row_border = h(t.border);
    theme.table_even = h(c.subtle);

    theme.tab = gpui::transparent_black();
    theme.tab_bar = h(c.subtle);
    theme.tab_bar_segmented = h(c.subtle);
    theme.tab_active = h(t.bg);
    theme.tab_foreground = h(t.text_muted);
    theme.tab_active_foreground = h(t.text);

    theme.title_bar = h(c.subtle);
    theme.title_bar_border = h(t.border);
    theme.status_bar = h(c.subtle);

    theme.sidebar = h(c.subtle);
    theme.sidebar_foreground = h(t.text);
    theme.sidebar_border = h(t.border);
    theme.sidebar_accent = h(c.active);
    theme.sidebar_accent_foreground = h(t.text);
    theme.sidebar_primary = h(t.accent);
    theme.sidebar_primary_foreground = h(c.on_accent);

    theme.scrollbar = gpui::transparent_black();
    theme.scrollbar_thumb = h(c.scrollbar_thumb);
    theme.scrollbar_thumb_hover = h(c.scrollbar_thumb_hover);

    theme.switch = h(c.active);
    theme.switch_thumb = h(t.bg);
    theme.slider_bar = h(t.accent);
    theme.slider_thumb = h(t.bg);

    // Softer than gpui-component's default 6px: at the density this app runs
    // (28px rows, xsmall buttons) a 6px corner reads as a chamfer rather than a
    // rounded one. 8px is what tty7 settles on for the same widget sizes.
    theme.radius = px(crate::metrics::RADIUS);
    theme.radius_lg = px(crate::metrics::RADIUS * 1.5);

    // 15px rather than the stock 16: `text_sm` and `text_xs` are relative to
    // this, and at 16 the panel labels came out larger than the document text
    // they annotate. The editor keeps its own mono size.
    theme.font_size = px(15.);

    // Note: `scrollbar_mode` is left alone. gpui-component already defaults to
    // `Scrolling` — overlay bars that fade — which is what these narrow panels
    // want; a permanent gutter would cost a visible slice of every file name.

    // The theme's colors reach the scrollbar and resize handles only through the
    // Base layer, which caches them — without this the scrollbar keeps the
    // colors of whichever theme was applied last.
    Theme::sync_base(cx);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_preset_has_a_unique_id_within_its_mode() {
        let mut seen: Vec<(&str, bool)> = Vec::new();
        for preset in PRESETS {
            let key = (preset.id, preset.dark);
            assert!(!seen.contains(&key), "duplicate preset {}", preset.id);
            seen.push(key);
        }
    }

    #[test]
    fn both_modes_are_offered() {
        assert!(for_mode(false).count() >= 6, "light presets");
        assert!(for_mode(true).count() >= 6, "dark presets");
    }

    #[test]
    fn the_named_defaults_exist() {
        assert!(PRESETS.iter().any(|p| p.id == DEFAULT_LIGHT && !p.dark));
        assert!(PRESETS.iter().any(|p| p.id == DEFAULT_DARK && p.dark));
    }

    #[test]
    fn an_unknown_id_falls_back_within_the_requested_mode() {
        // A settings file naming a renamed preset must still open a window, and
        // must not open it in the wrong mode.
        assert_eq!(by_id("no-such-theme", true).id, DEFAULT_DARK);
        assert_eq!(by_id("no-such-theme", false).id, DEFAULT_LIGHT);
        // Asking for a light preset in dark mode is the same mistake.
        assert!(by_id("sepia", true).dark, "must stay in the requested mode");
    }

    #[test]
    fn a_known_id_resolves_to_itself() {
        for preset in PRESETS {
            assert_eq!(by_id(preset.id, preset.dark).id, preset.id);
        }
    }

    #[test]
    fn mixing_interpolates_per_channel_and_clamps() {
        assert_eq!(mix(0x000000, 0xffffff, 0.0), 0x000000);
        assert_eq!(mix(0x000000, 0xffffff, 1.0), 0xffffff);
        assert_eq!(mix(0x000000, 0xffffff, 0.5), 0x808080);
        // Out-of-range t must not wrap around into another channel.
        assert_eq!(mix(0x102030, 0x405060, -1.0), 0x102030);
        assert_eq!(mix(0x102030, 0x405060, 2.0), 0x405060);
    }

    #[test]
    fn derived_chrome_stays_between_the_background_and_the_foreground() {
        // The property that makes deriving safe on both `#000000` (Midnight)
        // and `#f6efe0` (Sepia): a hover state is always a step from the page
        // towards the ink, never past either.
        let luma = |c: u32| {
            let r = ((c >> 16) & 0xff) as f32;
            let g = ((c >> 8) & 0xff) as f32;
            let b = (c & 0xff) as f32;
            0.2126 * r + 0.7152 * g + 0.0722 * b
        };
        for preset in PRESETS {
            let c = preset.chrome();
            let (bg, fg) = (luma(preset.tokens.bg), luma(preset.tokens.text));
            let (lo, hi) = if bg <= fg { (bg, fg) } else { (fg, bg) };
            for (name, value) in [
                ("hover", c.hover),
                ("active", c.active),
                ("subtle", c.subtle),
                ("scrollbar_thumb", c.scrollbar_thumb),
                ("scrollbar_thumb_hover", c.scrollbar_thumb_hover),
            ] {
                let l = luma(value);
                assert!(
                    l >= lo - 0.5 && l <= hi + 0.5,
                    "{}'s {name} ({l}) escaped [{lo}, {hi}]",
                    preset.id
                );
            }
        }
    }

    #[test]
    fn hover_is_a_visible_step_from_the_page() {
        // A hover that lands on the background is a row that does not respond.
        for preset in PRESETS {
            let c = preset.chrome();
            assert_ne!(c.hover, preset.tokens.bg, "{} hover", preset.id);
            assert_ne!(c.active, c.hover, "{} active vs hover", preset.id);
        }
    }

    #[test]
    fn reading_presets_use_a_reading_typeface() {
        // The three ColaMD presets that are as much typography as palette.
        let font = |id: &str| PRESETS.iter().find(|p| p.id == id).unwrap().font;
        assert_eq!(font("writer"), BodyFont::Mono);
        assert_eq!(font("sepia"), BodyFont::Serif);
        assert_eq!(font("elegant"), BodyFont::Serif);
        assert_eq!(font("light"), BodyFont::Sans);
    }

    #[test]
    fn every_font_stack_names_a_generic_family_last() {
        // Without a generic fallback a machine missing every named face gets the
        // browser default, which may not even be the right category.
        for font in [BodyFont::Sans, BodyFont::Serif, BodyFont::Mono] {
            let css = font.css();
            assert!(
                css.ends_with("sans-serif") || css.ends_with("serif") || css.ends_with("monospace"),
                "{css}"
            );
            assert!(font.line_height() >= 1.5, "{css} is too tight to read");
        }
    }
}
