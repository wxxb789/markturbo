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
            assert!(data.len() > 10_000, "{path} looks truncated");
            // TrueType magic, so a placeholder or an LFS pointer fails here.
            assert_eq!(&data[..4], &[0x00, 0x01, 0x00, 0x00], "{path} is not a TTF");
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
