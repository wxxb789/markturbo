//! Assets and the bundled first-use workspace.
//!
//! Release builds embed `sample/` in the executable and materialize it under
//! the application data directory on demand. Debug builds use the repository
//! fixture directly, so the sample is never copied during local iteration.

use std::borrow::Cow;

/// The complete first-use workspace.
#[cfg(any(not(debug_assertions), test))]
#[derive(rust_embed::RustEmbed)]
#[folder = "../../sample"]
struct Sample;

/// Files of the bundled sample, embedded for release builds.
pub fn embedded_sample_files() -> impl Iterator<Item = (Cow<'static, str>, Cow<'static, [u8]>)> {
    #[cfg(any(not(debug_assertions), test))]
    {
        Sample::iter().filter_map(|path| Sample::get(&path).map(|file| (path, file.data)))
    }
    #[cfg(all(debug_assertions, not(test)))]
    {
        std::iter::empty()
    }
}
