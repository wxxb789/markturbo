//! The application's asset source.
//!
//! GPUI resolves every asset — icons, and the fonts its SVG renderer needs —
//! through one [`AssetSource`]. `gpui-component` ships most icons; the fonts and
//! the handful of icons it lacks are ours to supply, so this composes the two
//! rather than replacing either.

use std::borrow::Cow;

use gpui::{AssetSource, Result, SharedString};

/// Fonts GPUI's SVG renderer asks for by name.
///
/// `gpui::svg_renderer::load_bundled_fonts` requests exactly these two paths on
/// first SVG render, and `fix_generic_font_families` then points `sans-serif`
/// and `monospace` at them when the system has no match. Every diagram this app
/// renders reaches text through that path — `mermaid-svg` emits
/// `font-family="sans-serif"` on the root `<svg>` — so without these, diagram
/// labels come out blank.
#[derive(rust_embed::RustEmbed)]
#[folder = "assets"]
#[include = "fonts/**/*.ttf"]
struct Fonts;

/// Icons `gpui-component` does not ship.
///
/// Ours take priority over the delegate's, so a name that exists in both
/// resolves here — which is how an upstream icon can be replaced without
/// forking the icon set.
#[derive(rust_embed::RustEmbed)]
#[folder = "assets"]
#[include = "icons/**/*.svg"]
struct Icons;

/// KaTeX faces used by the native math renderer.
///
/// These are separate from GPUI's fonts: they are parsed lazily when a math
/// block is rendered, rather than registered with the UI font database.
#[derive(rust_embed::RustEmbed)]
#[folder = "../../fonts/katex"]
#[include = "KaTeX_*.ttf"]
#[exclude = "KaTeX_Caligraphic-Bold.ttf"]
struct MathFonts;

/// Distribution notices retained inside the single-file release artifact.
#[derive(rust_embed::RustEmbed)]
#[folder = "../.."]
#[include = "LICENSE"]
#[include = "fonts/katex/LICENSE"]
struct Licenses;

/// Notices for the UI fonts compiled into the executable.
#[derive(rust_embed::RustEmbed)]
#[folder = "assets"]
#[include = "fonts/ibm-plex-sans/license.txt"]
#[include = "fonts/lilex/OFL.txt"]
struct FontNotices;

/// The complete first-use workspace.
///
/// Release binaries materialize this into the application's data directory on
/// demand. Keeping it here means the executable is the only release artifact
/// required for a working Welcome sample.
#[cfg(any(not(debug_assertions), test))]
#[derive(rust_embed::RustEmbed)]
#[folder = "../../sample"]
struct Sample;

/// KaTeX faces RaTeX's layout can ask for, and the embedded file each uses.
///
/// Mirrors `ratex-font-loader`'s own `FONT_MAP` and is the single inventory
/// shared by the asset and renderer paths.
pub(crate) const MATH_FONT_FILES: &[(ratex_font::FontId, &str)] = {
    use ratex_font::FontId as F;
    &[
        (F::MainRegular, "KaTeX_Main-Regular.ttf"),
        (F::MainBold, "KaTeX_Main-Bold.ttf"),
        (F::MainItalic, "KaTeX_Main-Italic.ttf"),
        (F::MainBoldItalic, "KaTeX_Main-BoldItalic.ttf"),
        (F::MathItalic, "KaTeX_Math-Italic.ttf"),
        (F::MathBoldItalic, "KaTeX_Math-BoldItalic.ttf"),
        (F::AmsRegular, "KaTeX_AMS-Regular.ttf"),
        (F::CaligraphicRegular, "KaTeX_Caligraphic-Regular.ttf"),
        (F::FrakturRegular, "KaTeX_Fraktur-Regular.ttf"),
        (F::FrakturBold, "KaTeX_Fraktur-Bold.ttf"),
        (F::SansSerifRegular, "KaTeX_SansSerif-Regular.ttf"),
        (F::SansSerifBold, "KaTeX_SansSerif-Bold.ttf"),
        (F::SansSerifItalic, "KaTeX_SansSerif-Italic.ttf"),
        (F::ScriptRegular, "KaTeX_Script-Regular.ttf"),
        (F::TypewriterRegular, "KaTeX_Typewriter-Regular.ttf"),
        (F::Size1Regular, "KaTeX_Size1-Regular.ttf"),
        (F::Size2Regular, "KaTeX_Size2-Regular.ttf"),
        (F::Size3Regular, "KaTeX_Size3-Regular.ttf"),
        (F::Size4Regular, "KaTeX_Size4-Regular.ttf"),
    ]
};

pub(crate) fn embedded_math_font(name: &str) -> Option<Cow<'static, [u8]>> {
    MathFonts::get(name).map(|file| file.data)
}

fn embedded_license(path: &str) -> Option<Cow<'static, [u8]>> {
    match path {
        "licenses/markturbo.txt" => Licenses::get("LICENSE").map(|file| file.data),
        "licenses/katex.md" => Licenses::get("fonts/katex/LICENSE").map(|file| file.data),
        "licenses/ibm-plex-sans.txt" => {
            FontNotices::get("fonts/ibm-plex-sans/license.txt").map(|file| file.data)
        }
        "licenses/lilex.txt" => FontNotices::get("fonts/lilex/OFL.txt").map(|file| file.data),
        _ => None,
    }
}

#[cfg(any(not(debug_assertions), test))]
pub(crate) fn embedded_sample_files()
-> impl Iterator<Item = (Cow<'static, str>, Cow<'static, [u8]>)> {
    Sample::iter().filter_map(|path| Sample::get(&path).map(|file| (path, file.data)))
}

/// Icons plus fonts.
///
/// Order matters only in that the two sets are disjoint: `gpui-component`
/// serves `icons/**`, this serves `fonts/**`. Anything else is genuinely
/// missing and is reported as such.
pub struct Assets;

impl AssetSource for Assets {
    fn load(&self, path: &str) -> Result<Option<Cow<'static, [u8]>>> {
        if let Some(file) = Fonts::get(path) {
            return Ok(Some(file.data));
        }
        if let Some(file) = Icons::get(path) {
            return Ok(Some(file.data));
        }
        if let Some(file) = embedded_license(path) {
            return Ok(Some(file));
        }
        // Delegate rather than replace: this is what supplies every `IconName`.
        gpui_component_assets::Assets.load(path)
    }

    fn list(&self, path: &str) -> Result<Vec<SharedString>> {
        let mut items = gpui_component_assets::Assets.list(path)?;
        let own = Fonts::iter()
            .chain(Icons::iter())
            .filter(|p| p.starts_with(path));
        for path in own {
            let path: SharedString = path.into();
            // An icon of ours that shadows an upstream one is still one entry.
            if !items.contains(&path) {
                items.push(path);
            }
        }
        Ok(items)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The exact paths `gpui` requests. Hard-coded on purpose: if an upstream
    /// bump renames them, this test is the thing that says so — the app itself
    /// only logs a warning and renders text with no glyphs.
    const REQUIRED: &[&str] = &[
        "fonts/ibm-plex-sans/IBMPlexSans-Regular.ttf",
        "fonts/lilex/Lilex-Regular.ttf",
    ];

    #[test]
    fn serves_the_fonts_gpui_asks_for() {
        for path in REQUIRED {
            let data = Assets
                .load(path)
                .unwrap_or_else(|e| panic!("{path}: {e}"))
                .unwrap_or_else(|| panic!("{path} is missing"));
            assert!(data.len() > 7_000, "{path} looks truncated");
            // TrueType magic, so a placeholder or an LFS pointer fails here.
            assert_eq!(&data[..4], &[0x00, 0x01, 0x00, 0x00], "{path} is not a TTF");
        }
    }

    #[test]
    fn embeds_every_katex_face_used_by_the_native_renderer() {
        let embedded: Vec<_> = MathFonts::iter()
            .filter(|path| path.ends_with(".ttf"))
            .collect();
        assert_eq!(embedded.len(), MATH_FONT_FILES.len());
        for (_, path) in MATH_FONT_FILES {
            let data = embedded_math_font(path).unwrap_or_else(|| panic!("{path} is missing"));
            assert!(data.len() > 7_000, "{path} looks truncated");
            assert_eq!(&data[..4], &[0x00, 0x01, 0x00, 0x00], "{path} is not a TTF");
        }
    }

    #[test]
    fn embeds_the_complete_sample_workspace() {
        let mut paths: Vec<_> = embedded_sample_files()
            .map(|(path, data)| {
                assert!(!data.is_empty(), "{path} is empty");
                path.into_owned()
            })
            .collect();
        paths.sort();
        assert_eq!(
            paths,
            [
                ".claude/skills/broken-example/SKILL.md",
                ".claude/skills/hello-diagrams/SKILL.md",
                ".claude/skills/hello-diagrams/references/guide.md",
                ".claude/skills/hello-diagrams/scripts/render.sh",
                "AGENTS.md",
                "README.md",
                "docs/diagrams.md",
            ]
        );
    }

    #[test]
    fn embeds_every_distribution_notice() {
        for (path, expected) in [
            ("licenses/markturbo.txt", "Apache License"),
            ("licenses/katex.md", "WITHOUT WARRANTY"),
            ("licenses/ibm-plex-sans.txt", "IBM Corp."),
            ("licenses/lilex.txt", "Lilex Project Authors"),
        ] {
            let notice = Assets
                .load(path)
                .expect("notice lookup must succeed")
                .unwrap_or_else(|| panic!("{path} must be embedded"));
            let text = std::str::from_utf8(&notice).expect("notice must be UTF-8");
            assert!(text.contains(expected));
        }
    }

    #[test]
    fn still_serves_component_icons() {
        let icon = Assets
            .load("icons/folder.svg")
            .expect("icons must still resolve through the delegate");
        assert!(icon.is_some_and(|d| !d.is_empty()));
    }

    #[test]
    fn an_unknown_path_is_an_error_not_a_silent_none() {
        // gpui-component's source reports a missing asset as `Err`; composing
        // must not quietly turn that into `Ok(None)`.
        assert!(Assets.load("nope/missing.svg").is_err());
    }

    #[test]
    fn listing_covers_every_set() {
        assert_eq!(Assets.list("fonts").unwrap().len(), 2);
        let icons = Assets.list("icons").unwrap();
        assert!(!icons.is_empty());
        // Ours appear alongside the delegate's rather than replacing the list.
        assert!(
            icons.iter().any(|p| p.ends_with("refresh-cw.svg")),
            "own icons must be listed"
        );
        assert!(icons.iter().any(|p| p.ends_with("folder.svg")));
    }

    #[test]
    fn serves_the_icons_upstream_lacks() {
        // `refresh-cw` is not in gpui-component's set; the Harness panel used
        // `redo` for rescanning, which is a single curved arrow and reads as
        // "undo" rather than "reload".
        let icon = Assets
            .load("icons/refresh-cw.svg")
            .expect("must resolve")
            .expect("must exist");
        let text = std::str::from_utf8(&icon).expect("svg is text");
        assert!(text.contains("<svg"), "not an SVG");
        assert!(
            text.contains("stroke=\"currentColor\""),
            "must take its color from the theme like every other icon"
        );
    }

    #[test]
    fn listing_does_not_duplicate_a_shadowed_icon() {
        // Ours take priority on load, so listing both copies would report an
        // icon set larger than what can actually be resolved.
        let icons = Assets.list("icons").unwrap();
        let mut sorted: Vec<&SharedString> = icons.iter().collect();
        sorted.sort();
        let before = sorted.len();
        sorted.dedup();
        assert_eq!(before, sorted.len(), "duplicate entries in the icon list");
    }
}
