//! Owner lane: foundation.
//!
//! The typefaces the Afterglow UI is set in, embedded in the executable and
//! registered with nsh at startup.
//!
//! Martian Grotesk (300 / 400 / 500) and Martian Mono (400, condensed) are
//! by Evil Martians, licensed under the SIL Open Font License 1.1; the licence
//! texts ship beside the files in `crates/mt-app/fonts/martian/`, and
//! `SOURCE.md` there records the release, URL and hash each `.ttf` came from.
//!
//! nsh resolves each face to its family — the three Grotesk instances register
//! as `Martian Grotesk` and are told apart by weight, the mono as
//! `Martian Mono`. [`crate::ui::tokens`] holds those names, the weights and
//! the CJK fallback lists.
//!
//! Registration is asynchronous (`register_font`'s answer resolves to the
//! family name). Text laid out before it lands is measured again by the
//! engine when the faces arrive, so nothing needs to block the first frame.

use nsh_core::element::{register_font, spawn};
use nsh_core::prelude::stored;

/// One embedded face: the family name it must resolve to, and its bytes.
struct Face {
    /// The family nsh must resolve it to; a mismatch is logged, not fatal.
    family: &'static str,
    /// The `.ttf` as shipped by the release.
    data: &'static [u8],
}

/// The four faces, with the family names `ui::tokens` refers to. A `static`,
/// not a `const`: a `const` is copied into each crate that uses it, which put
/// a second copy of every face in the executable.
static FACES: [Face; 4] = [
    Face {
        family: "Martian Grotesk",
        data: include_bytes!("../../fonts/martian/MartianGrotesk-StdLt.ttf"),
    },
    Face {
        family: "Martian Grotesk",
        data: include_bytes!("../../fonts/martian/MartianGrotesk-StdRg.ttf"),
    },
    Face {
        family: "Martian Grotesk",
        data: include_bytes!("../../fonts/martian/MartianGrotesk-StdMd.ttf"),
    },
    Face {
        family: "Martian Mono",
        data: include_bytes!("../../fonts/martian/MartianMono-CnRg.ttf"),
    },
];

/// The embedded faces' bytes, in registration order: what `main.rs` hands to
/// the root so the first frame is already measured with them.
pub fn faces() -> impl Iterator<Item = &'static [u8]> {
    FACES.iter().map(|face| face.data)
}

/// The family name of each face, in the same order as [`faces`].
pub fn family_names() -> impl Iterator<Item = &'static str> {
    FACES.iter().map(|face| face.family)
}

/// Registers every embedded face, in order. Call once, from the app root;
/// the registration lives as long as the app does.
///
/// Idempotent per process in practice (nsh keeps the faces), but it is only
/// called from [`crate::app::App`].
pub fn register() {
    stored(spawn(async {
        for face in &FACES {
            match register_font(face.data).await {
                Ok(family) if family == face.family => {
                    log::debug!("fonts: registered {family}");
                }
                Ok(family) => {
                    log::warn!(
                        "fonts: {} registered as {family:?}, not {:?}",
                        face.family,
                        face.family
                    );
                }
                Err(why) => log::error!("fonts: {} is no font: {why}", face.family),
            }
        }
    }));
}
