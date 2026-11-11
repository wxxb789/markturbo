//! Welcome's bundled-sample adapter.
//!
//! Runtime data paths and sample installation live in `mt-core`; this module
//! keeps only the app-owned asset source and the debug fixture behavior.

use std::io;
#[cfg(any(debug_assertions, test))]
use std::path::Path;
use std::path::PathBuf;

/// Whether the sample workspace can be offered from Welcome.
pub fn bundled_sample_available() -> bool {
    #[cfg(any(debug_assertions, test))]
    {
        source_sample_dir().is_dir()
    }

    #[cfg(not(any(debug_assertions, test)))]
    {
        crate::assets::embedded_sample_files().next().is_some()
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
}
