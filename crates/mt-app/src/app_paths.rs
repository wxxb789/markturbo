//! Welcome's embedded sample adapter.
//!
//! Runtime data paths and sample installation live in `mt-core`; this module
//! keeps only the app-owned asset source and the debug fixture behavior.

use std::io;
#[cfg(any(debug_assertions, test))]
use std::path::Path;
use std::path::PathBuf;

/// Whether the sample workspace can be offered from Welcome.
///
/// Release builds query the embedded asset source without resolving or writing
/// a runtime data directory, so a bad data root does not disable the open action.
pub fn bundled_sample_available() -> bool {
    #[cfg(any(debug_assertions, test))]
    {
        source_sample_dir().is_dir()
    }

    #[cfg(not(any(debug_assertions, test)))]
    {
        release_sample_is_available()
    }
}

/// Opens the Welcome sample, using the repository fixture in debug builds and
/// atomically installing embedded assets into app-owned data in release builds.
pub fn bundled_sample_dir() -> io::Result<PathBuf> {
    #[cfg(any(debug_assertions, test))]
    {
        let source_sample = source_sample_dir();
        if source_sample.is_dir() {
            return Ok(source_sample);
        }
        Err(io::Error::new(
            io::ErrorKind::NotFound,
            "repository sample fixture is unavailable",
        ))
    }

    #[cfg(not(any(debug_assertions, test)))]
    {
        let data_root = mt_core::runtime_paths::data_dir().ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::NotFound,
                "application data directory is unavailable",
            )
        })?;
        static SAMPLE_INSTALLER: mt_core::runtime_paths::EmbeddedSampleInstaller =
            mt_core::runtime_paths::EmbeddedSampleInstaller::new();
        SAMPLE_INSTALLER.materialize_in(&data_root, crate::assets::embedded_sample_files)
    }
}

#[cfg(any(debug_assertions, test))]
fn source_sample_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../sample")
}

#[cfg(not(any(debug_assertions, test)))]
fn release_sample_is_available() -> bool {
    crate::assets::embedded_sample_files().next().is_some()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn debug_welcome_sample_uses_repository_fixture_without_installing_assets() {
        let source = source_sample_dir();
        assert!(source.is_dir());
        assert!(bundled_sample_available());
        assert_eq!(bundled_sample_dir().unwrap(), source);
    }

    #[test]
    fn embedded_welcome_sample_installs_opens_and_preserves_user_edits() {
        let data_root = tempfile::tempdir().unwrap();
        let installer = mt_core::runtime_paths::EmbeddedSampleInstaller::new();
        let sample = installer
            .materialize_in(data_root.path(), crate::assets::embedded_sample_files)
            .unwrap();
        let readme = sample.join("README.md");
        let loaded = mt_core::document::io::load(&readme).unwrap();
        let document = mt_core::Document::new(Some(loaded.path), loaded.text);
        assert_eq!(document.outline().headings[0].text, "Welcome to markturbo");

        std::fs::write(&readme, "# My edited sample\n").unwrap();
        assert_eq!(
            installer
                .materialize_in(data_root.path(), crate::assets::embedded_sample_files)
                .unwrap(),
            sample
        );
        assert_eq!(
            std::fs::read_to_string(readme).unwrap(),
            "# My edited sample\n"
        );
    }
}
