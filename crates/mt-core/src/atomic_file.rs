//! Staged, synced, then persisted replacement of small state files.
//!
//! Writing a state file in place can be observed torn: `settings.toml` holds
//! consent and credential authorization, and a torn file loads as defaults
//! while every later save fails as malformed. One sibling temporary, an
//! fsync, and a rename make the replacement all-or-nothing, and dropping the
//! stage removes it when any step fails.
//!
//! Callers keep their own staging prefixes and error surfaces. The document
//! commit protocol in [`crate::document::io::commit`] stays separate: it
//! races external writers and works through recovery links, which this module
//! deliberately does not.

use std::fs::File;
use std::io::{self, Write as _};
use std::path::Path;

/// A replacement staged next to its destination, removed on drop unless it is
/// published.
pub(crate) struct StagedFile {
    temp: tempfile::NamedTempFile,
}

impl StagedFile {
    /// Stage an empty file in `parent` under `prefix` and `suffix`.
    pub(crate) fn create_in(parent: &Path, prefix: &str, suffix: &str) -> io::Result<Self> {
        let temp = tempfile::Builder::new()
            .prefix(prefix)
            .suffix(suffix)
            .tempfile_in(parent)?;
        Ok(Self { temp })
    }

    pub(crate) fn write_all(&mut self, bytes: &[u8]) -> io::Result<()> {
        self.temp.write_all(bytes)
    }

    /// The staged file, for callers that stream into it before publishing.
    #[cfg(feature = "model-transport")]
    pub(crate) fn as_file(&self) -> &File {
        self.temp.as_file()
    }

    pub(crate) fn as_file_mut(&mut self) -> &mut File {
        self.temp.as_file_mut()
    }

    /// Flush the staged bytes to the device before they are published.
    pub(crate) fn sync_all(&self) -> io::Result<()> {
        self.temp.as_file().sync_all()
    }

    /// Publish as `path`, replacing whatever entry is there.
    pub(crate) fn persist(self, path: &Path) -> io::Result<File> {
        self.temp.persist(path).map_err(|error| error.error)
    }

    /// Publish as `path` only when nothing is there. An existing entry is left
    /// untouched and reported as `AlreadyExists`.
    pub(crate) fn persist_noclobber(self, path: &Path) -> io::Result<File> {
        self.temp
            .persist_noclobber(path)
            .map_err(|error| error.error)
    }

    /// The underlying temporary, for callers whose commit protocol owns it.
    pub(crate) fn into_tempfile(self) -> tempfile::NamedTempFile {
        self.temp
    }
}

/// Replace `path` with `bytes`: create the parent directory, stage a sibling,
/// fsync it, and rename it over `path`. On any failure `path` keeps its
/// previous contents and the stage is removed.
pub(crate) fn replace(path: &Path, prefix: &str, suffix: &str, bytes: &[u8]) -> io::Result<()> {
    let parent = parent_directory(path);
    std::fs::create_dir_all(parent)?;
    stage_and_publish(path, prefix, suffix, bytes, false)
}

/// Like [`replace`], but refuse to overwrite an existing entry. The parent
/// directory must already exist: an export destination chosen by the user is
/// not a directory to create.
pub(crate) fn replace_noclobber(
    path: &Path,
    prefix: &str,
    suffix: &str,
    bytes: &[u8],
) -> io::Result<()> {
    stage_and_publish(path, prefix, suffix, bytes, true)
}

fn stage_and_publish(
    path: &Path,
    prefix: &str,
    suffix: &str,
    bytes: &[u8],
    noclobber: bool,
) -> io::Result<()> {
    let mut staged = StagedFile::create_in(parent_directory(path), prefix, suffix)?;
    staged.write_all(bytes)?;
    staged.sync_all()?;
    if noclobber {
        staged.persist_noclobber(path)?;
    } else {
        staged.persist(path)?;
    }
    Ok(())
}

fn parent_directory(path: &Path) -> &Path {
    path.parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or(Path::new("."))
}

#[cfg(test)]
mod tests {
    use super::*;

    const PREFIX: &str = ".markturbo-atomic-test-";

    fn staging_files(parent: &Path) -> Vec<std::path::PathBuf> {
        std::fs::read_dir(parent)
            .unwrap()
            .filter_map(|entry| {
                let entry = entry.unwrap();
                let name = entry.file_name();
                name.to_str()
                    .filter(|name| name.starts_with(PREFIX))
                    .map(|_| entry.path())
            })
            .collect()
    }

    #[test]
    fn replace_leaves_no_stage_and_publishes_the_complete_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state.toml");
        std::fs::write(&path, "old").unwrap();

        replace(&path, PREFIX, ".tmp", b"new").unwrap();

        assert_eq!(std::fs::read_to_string(&path).unwrap(), "new");
        assert!(staging_files(dir.path()).is_empty(), "no stage left behind");
    }

    #[test]
    fn replace_recreates_a_missing_parent_directory() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nested/dir/state.toml");

        replace(&path, PREFIX, ".tmp", b"new").unwrap();

        assert_eq!(std::fs::read_to_string(&path).unwrap(), "new");
        assert!(staging_files(path.parent().unwrap()).is_empty());
    }

    #[test]
    fn a_failed_publish_leaves_the_destination_and_no_stage() {
        // A directory cannot be replaced by a rename on any platform, so the
        // publish step fails after the bytes were staged. The stage must not
        // survive, and neither must the entry the failure was aimed at.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("occupied");
        std::fs::create_dir(&path).unwrap();

        let error = replace(&path, PREFIX, ".tmp", b"new").unwrap_err();

        assert_ne!(
            error.kind(),
            io::ErrorKind::NotFound,
            "the failure is the publish, not the stage"
        );
        assert!(staging_files(dir.path()).is_empty(), "no stage left behind");
        assert!(path.is_dir(), "the previous entry is untouched");
    }

    #[test]
    fn replace_noclobber_keeps_an_existing_file() {
        // The settings/export no-overwrite rule: an existing file survives the
        // failed persist byte-for-byte, and the stage is removed.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("kept.json");
        std::fs::write(&path, "original").unwrap();

        let error = replace_noclobber(&path, PREFIX, ".tmp", b"replacement").unwrap_err();

        assert_eq!(error.kind(), io::ErrorKind::AlreadyExists);
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "original");
        assert!(staging_files(dir.path()).is_empty(), "no stage left behind");
    }

    #[test]
    fn replace_noclobber_does_not_create_a_missing_parent() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("missing/kept.json");

        let error = replace_noclobber(&path, PREFIX, ".tmp", b"new").unwrap_err();

        assert_eq!(error.kind(), io::ErrorKind::NotFound);
    }

    #[test]
    fn a_staged_file_dropped_without_persist_leaves_nothing() {
        let dir = tempfile::tempdir().unwrap();
        {
            let mut staged = StagedFile::create_in(dir.path(), PREFIX, ".tmp").unwrap();
            staged.write_all(b"unpublished").unwrap();
            staged.sync_all().unwrap();
            assert_eq!(staging_files(dir.path()).len(), 1);
        }
        assert!(staging_files(dir.path()).is_empty());
    }
}
