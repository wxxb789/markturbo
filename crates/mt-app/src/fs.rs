//! Reading and writing workspace files safely.
//!
//! Two invariants:
//!
//! * A save never clobbers a change made outside the app. We record the
//!   modification time and size seen when the file was loaded, and refuse to
//!   write if the file on disk no longer matches.
//! * Files stay ordinary files. No proprietary format, no reformatting, no
//!   forced newline conversion — a document written back unchanged is
//!   byte-identical to what was read.

use std::path::{Path, PathBuf};
use std::time::SystemTime;

/// What we knew about a file when we last read or wrote it.
///
/// mtime + size is not a cryptographic guarantee, but it is what every editor
/// uses and it catches every realistic case: an agent rewriting the file, a git
/// checkout, another editor saving.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileStamp {
    pub modified: Option<SystemTime>,
    pub len: u64,
}

impl FileStamp {
    pub fn of(path: &Path) -> std::io::Result<Self> {
        let meta = std::fs::metadata(path)?;
        Ok(Self {
            modified: meta.modified().ok(),
            len: meta.len(),
        })
    }

    /// True when the file on disk still matches this stamp.
    pub fn matches(&self, path: &Path) -> bool {
        Self::of(path).is_ok_and(|current| current == *self)
    }
}

/// A file loaded into the editor.
#[derive(Debug, Clone)]
pub struct LoadedFile {
    pub path: PathBuf,
    pub text: String,
    pub stamp: FileStamp,
    /// The newline convention found in the file, so saving preserves it.
    pub newline: Newline,
    /// True when the file began with a UTF-8 BOM, which must be written back.
    pub had_bom: bool,
}

/// Which line ending the file uses.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Newline {
    Lf,
    Crlf,
}

impl Newline {
    pub fn as_str(self) -> &'static str {
        match self {
            Newline::Lf => "\n",
            Newline::Crlf => "\r\n",
        }
    }

    /// Detect the dominant convention.
    ///
    /// Mixed files exist; we pick the majority so a save does not rewrite every
    /// line of a file the user only touched in one place.
    pub fn detect(text: &str) -> Self {
        let crlf = text.matches("\r\n").count();
        let lf = text.matches('\n').count() - crlf;
        if crlf > lf { Newline::Crlf } else { Newline::Lf }
    }
}

/// Read a file for editing.
///
/// Line endings are normalized to `\n` in memory — the editor and every parser
/// want one convention — and restored on save. Invalid UTF-8 is replaced rather
/// than rejected so a file with one bad byte still opens.
pub fn load(path: &Path) -> std::io::Result<LoadedFile> {
    let bytes = std::fs::read(path)?;
    let stamp = FileStamp::of(path)?;

    let had_bom = bytes.starts_with(&[0xEF, 0xBB, 0xBF]);
    let body = if had_bom { &bytes[3..] } else { &bytes[..] };
    let raw = String::from_utf8_lossy(body).into_owned();

    let newline = Newline::detect(&raw);
    let text = if newline == Newline::Crlf {
        raw.replace("\r\n", "\n")
    } else {
        raw
    };

    Ok(LoadedFile {
        path: path.to_path_buf(),
        text,
        stamp,
        newline,
        had_bom,
    })
}

/// Why a save did not happen.
#[derive(Debug)]
pub enum SaveError {
    /// The file changed on disk since it was loaded. The caller must resolve
    /// this with the user before overwriting.
    Conflict,
    Io(std::io::Error),
}

impl std::fmt::Display for SaveError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SaveError::Conflict => f.write_str(
                "the file changed on disk since it was opened; reload or save a copy",
            ),
            SaveError::Io(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for SaveError {}

/// Write `text` back to `file.path`, refusing if the file changed externally.
///
/// Returns the new stamp on success. `force` skips the check — only pass it
/// after the user has explicitly chosen to overwrite.
pub fn save(file: &LoadedFile, text: &str, force: bool) -> Result<FileStamp, SaveError> {
    if !force && file.path.exists() && !file.stamp.matches(&file.path) {
        return Err(SaveError::Conflict);
    }
    write(&file.path, text, file.newline, file.had_bom).map_err(SaveError::Io)
}

/// Write to a new path (Save As). No conflict check: the caller chose the path.
pub fn save_as(
    path: &Path,
    text: &str,
    newline: Newline,
    had_bom: bool,
) -> std::io::Result<FileStamp> {
    write(path, text, newline, had_bom)
}

fn write(
    path: &Path,
    text: &str,
    newline: Newline,
    had_bom: bool,
) -> std::io::Result<FileStamp> {
    let mut bytes = Vec::with_capacity(text.len() + 3);
    if had_bom {
        bytes.extend_from_slice(&[0xEF, 0xBB, 0xBF]);
    }
    if newline == Newline::Crlf {
        // The in-memory text uses `\n`; restore the file's convention. Guard
        // against a stray `\r\n` already present so we never emit `\r\r\n`.
        bytes.extend_from_slice(text.replace("\r\n", "\n").replace('\n', "\r\n").as_bytes());
    } else {
        bytes.extend_from_slice(text.as_bytes());
    }

    // Write to a sibling temp file then rename, so a crash mid-write cannot
    // truncate the user's document. Same directory keeps the rename atomic.
    let temp = path.with_extension(format!(
        "{}.markturbo-tmp",
        path.extension().and_then(|e| e.to_str()).unwrap_or("")
    ));
    std::fs::write(&temp, &bytes)?;
    if let Err(err) = std::fs::rename(&temp, path) {
        // Windows rename fails if the destination exists and is locked; clean
        // up rather than leaving a stray temp file behind.
        let _ = std::fs::remove_file(&temp);
        return Err(err);
    }
    FileStamp::of(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_file(dir: &Path, name: &str, content: &[u8]) -> PathBuf {
        let path = dir.join(name);
        std::fs::write(&path, content).unwrap();
        path
    }

    #[test]
    fn round_trips_lf_files_byte_identically() {
        let dir = tempfile::tempdir().unwrap();
        let original = "# Title\n\nBody with trailing spaces  \n\n\n";
        let path = write_file(dir.path(), "a.md", original.as_bytes());

        let file = load(&path).unwrap();
        assert_eq!(file.text, original);
        assert_eq!(file.newline, Newline::Lf);

        save(&file, &file.text, false).unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), original.as_bytes());
    }

    #[test]
    fn round_trips_crlf_files_byte_identically() {
        let dir = tempfile::tempdir().unwrap();
        let original = b"# Title\r\n\r\nBody\r\n";
        let path = write_file(dir.path(), "a.md", original);

        let file = load(&path).unwrap();
        assert_eq!(file.newline, Newline::Crlf);
        assert_eq!(file.text, "# Title\n\nBody\n", "normalized in memory");

        save(&file, &file.text, false).unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), original, "CRLF restored");
    }

    #[test]
    fn preserves_a_bom() {
        let dir = tempfile::tempdir().unwrap();
        let original = b"\xEF\xBB\xBF# Title\n";
        let path = write_file(dir.path(), "a.md", original);

        let file = load(&path).unwrap();
        assert!(file.had_bom);
        assert_eq!(file.text, "# Title\n", "BOM stripped in memory");

        save(&file, &file.text, false).unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), original);
    }

    #[test]
    fn external_change_blocks_a_save() {
        let dir = tempfile::tempdir().unwrap();
        let path = write_file(dir.path(), "a.md", b"original\n");
        let file = load(&path).unwrap();

        // An agent rewrites the file behind our back.
        std::thread::sleep(std::time::Duration::from_millis(20));
        std::fs::write(&path, b"rewritten by an agent, much longer than before\n").unwrap();

        let err = save(&file, "my edit\n", false).unwrap_err();
        assert!(matches!(err, SaveError::Conflict));
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            "rewritten by an agent, much longer than before\n",
            "the external change must survive"
        );
    }

    #[test]
    fn forced_save_overwrites_after_the_user_decides() {
        let dir = tempfile::tempdir().unwrap();
        let path = write_file(dir.path(), "a.md", b"original\n");
        let file = load(&path).unwrap();
        std::fs::write(&path, b"external change\n").unwrap();

        save(&file, "my edit\n", true).unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "my edit\n");
    }

    #[test]
    fn saving_an_unchanged_file_succeeds_and_updates_the_stamp() {
        let dir = tempfile::tempdir().unwrap();
        let path = write_file(dir.path(), "a.md", b"one\n");
        let mut file = load(&path).unwrap();

        let new_stamp = save(&file, "one\ntwo\n", false).unwrap();
        file.stamp = new_stamp;
        // A second save with the refreshed stamp must not report a conflict.
        save(&file, "one\ntwo\nthree\n", false).unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "one\ntwo\nthree\n");
    }

    #[test]
    fn save_as_writes_a_new_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("new.md");
        save_as(&path, "content\n", Newline::Lf, false).unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "content\n");
    }

    #[test]
    fn no_temp_file_is_left_behind() {
        let dir = tempfile::tempdir().unwrap();
        let path = write_file(dir.path(), "a.md", b"x\n");
        let file = load(&path).unwrap();
        save(&file, "y\n", false).unwrap();

        let leftovers: Vec<_> = std::fs::read_dir(dir.path())
            .unwrap()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|n| n.contains("markturbo-tmp"))
            .collect();
        assert!(leftovers.is_empty(), "found {leftovers:?}");
    }

    #[test]
    fn detects_the_majority_newline_in_mixed_files() {
        assert_eq!(Newline::detect("a\r\nb\r\nc\n"), Newline::Crlf);
        assert_eq!(Newline::detect("a\nb\nc\r\n"), Newline::Lf);
        assert_eq!(Newline::detect("no newlines"), Newline::Lf);
    }

    #[test]
    fn invalid_utf8_still_opens() {
        let dir = tempfile::tempdir().unwrap();
        let path = write_file(dir.path(), "a.md", b"valid \xFF\xFE invalid\n");
        let file = load(&path).unwrap();
        assert!(file.text.contains("valid"), "must not fail to open");
    }

    #[test]
    fn cjk_content_round_trips() {
        let dir = tempfile::tempdir().unwrap();
        let original = "# 中文标题\n\n段落 🎉\n";
        let path = write_file(dir.path(), "a.md", original.as_bytes());
        let file = load(&path).unwrap();
        save(&file, &file.text, false).unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), original);
    }

    #[test]
    fn deleted_file_can_still_be_saved() {
        // Deleting externally then saving must recreate rather than conflict:
        // there is nothing to clobber.
        let dir = tempfile::tempdir().unwrap();
        let path = write_file(dir.path(), "a.md", b"x\n");
        let file = load(&path).unwrap();
        std::fs::remove_file(&path).unwrap();
        save(&file, "restored\n", false).unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "restored\n");
    }
}
