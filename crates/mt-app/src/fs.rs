//! Reading and writing workspace files safely.
//!
//! Three invariants:
//!
//! * A save never clobbers a change made outside the app. We record the
//!   modification time and size seen when the file was loaded, and refuse to
//!   write if the file on disk no longer matches.
//! * Files stay ordinary files. No proprietary format, no reformatting, no
//!   forced newline conversion — a document written back unchanged is
//!   byte-identical to what was read.
//! * A file this app cannot decode is never silently rewritten as something
//!   else. Text is decoded through its detected encoding and re-encoded in the
//!   same one on save, so a GBK document opened and saved untouched stays GBK.

use std::path::{Path, PathBuf};
use std::time::SystemTime;

use encoding_rs::{Encoding, UTF_8};

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
    /// The encoding the bytes were decoded from, so a save re-encodes in it.
    ///
    /// Almost always UTF-8. The exception is what this field exists for: a
    /// legacy GBK or Shift-JIS document decoded as UTF-8 becomes a wall of
    /// U+FFFD, and saving that back destroys the file. Carrying the encoding
    /// makes the round trip lossless instead.
    pub encoding: &'static Encoding,
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
        if crlf > lf {
            Newline::Crlf
        } else {
            Newline::Lf
        }
    }
}

/// Decide which encoding a file's bytes are in.
///
/// A BOM wins outright — it is a declaration, not a guess. Otherwise, valid
/// UTF-8 is taken at face value, because on a developer's machine that is what
/// almost every file is and running a detector over it can only introduce
/// error. Only bytes that are *not* valid UTF-8 reach the detector, which is
/// exactly the population it was built for: legacy content with no label.
///
/// Returns the encoding, the body with any BOM removed, and whether there was
/// one.
fn sniff_encoding(bytes: &[u8]) -> (&'static Encoding, &[u8], bool) {
    if let Some((encoding, bom_len)) = Encoding::for_bom(bytes) {
        return (encoding, &bytes[bom_len..], true);
    }
    if std::str::from_utf8(bytes).is_ok() {
        return (UTF_8, bytes, false);
    }
    // `Iso2022JpDetection::Allow`: the security reason for denying it is that a
    // browser can be tricked into running script from a mis-detected page. This
    // is a text editor with no script engine, and denying it would misdecode
    // exactly the Japanese mail archives the encoding still appears in.
    let mut detector = chardetng::EncodingDetector::new(chardetng::Iso2022JpDetection::Allow);
    detector.feed(bytes, true);
    // `Deny` — UTF-8 was already ruled out above, and letting the detector
    // return it anyway would put us back to lossy decoding.
    (
        detector.guess(None, chardetng::Utf8Detection::Deny),
        bytes,
        false,
    )
}

/// Read a file for editing.
///
/// Line endings are normalized to `\n` in memory — the editor and every parser
/// want one convention — and restored on save. Undecodable bytes are replaced
/// rather than rejected so a file with one bad byte still opens.
pub fn load(path: &Path) -> std::io::Result<LoadedFile> {
    let bytes = std::fs::read(path)?;
    let stamp = FileStamp::of(path)?;

    let (encoding, body, had_bom) = sniff_encoding(&bytes);
    let (raw, _) = encoding.decode_without_bom_handling(body);
    let raw = raw.into_owned();

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
        encoding,
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
            SaveError::Conflict => {
                f.write_str("the file changed on disk since it was opened; reload or save a copy")
            }
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
    write(&file.path, text, file.newline, file.had_bom, file.encoding).map_err(SaveError::Io)
}

/// Write to a new path (Save As). No conflict check: the caller chose the path.
pub fn save_as(
    path: &Path,
    text: &str,
    newline: Newline,
    had_bom: bool,
) -> std::io::Result<FileStamp> {
    write(path, text, newline, had_bom, UTF_8)
}

fn write(
    path: &Path,
    text: &str,
    newline: Newline,
    had_bom: bool,
    encoding: &'static Encoding,
) -> std::io::Result<FileStamp> {
    let restored;
    let text = if newline == Newline::Crlf {
        // The in-memory text uses `\n`; restore the file's convention. Guard
        // against a stray `\r\n` already present so we never emit `\r\r\n`.
        restored = text.replace("\r\n", "\n").replace('\n', "\r\n");
        restored.as_str()
    } else {
        text
    };

    let mut bytes = Vec::with_capacity(text.len() + 3);
    if had_bom {
        // The BOM that belongs to *this* encoding, not always UTF-8's. A
        // UTF-16 file written back with a UTF-8 BOM would be undecodable by
        // whatever reads it next.
        bytes.extend_from_slice(match encoding.name() {
            "UTF-16LE" => &[0xFF, 0xFE][..],
            "UTF-16BE" => &[0xFE, 0xFF][..],
            _ => &[0xEF, 0xBB, 0xBF][..],
        });
    }

    match encoding.name() {
        // `encoding_rs` has no UTF-16 *encoder*: `Encoding::encode` silently
        // falls back to UTF-8 output for these two, which would write a
        // UTF-8 body under a UTF-16 BOM — a file nothing can read. Encoding
        // the code units directly is the whole of what the encoder would do.
        name @ ("UTF-16LE" | "UTF-16BE") => {
            let big_endian = name == "UTF-16BE";
            for unit in text.encode_utf16() {
                bytes.extend_from_slice(&if big_endian {
                    unit.to_be_bytes()
                } else {
                    unit.to_le_bytes()
                });
            }
        }
        // Never fails: an unmappable character becomes a numeric character
        // reference. Lossy, but lossy in a way the target encoding can
        // represent, which beats refusing to save.
        _ => {
            let (encoded, _, _) = encoding.encode(text);
            bytes.extend_from_slice(&encoded);
        }
    }

    // Write to a sibling temp file then rename, so a crash mid-write cannot
    // truncate the user's document. Same directory keeps the rename atomic.
    //
    // `NamedTempFile` rather than a name derived from the target: the derived
    // name was deterministic, so two saves of the same document raced on one
    // temp path, and a file with no extension produced `README..markturbo-tmp`.
    // `persist` also handles the Windows case where the destination exists.
    let dir = path.parent().unwrap_or(Path::new("."));
    let mut temp = tempfile::Builder::new()
        .prefix(".markturbo-")
        .tempfile_in(dir)?;
    std::io::Write::write_all(&mut temp, &bytes)?;
    temp.persist(path).map_err(|e| e.error)?;
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
            .filter(|n| n != "a.md")
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
    fn a_gbk_document_survives_an_open_and_save() {
        // The data-loss path this closes: decoding through `from_utf8_lossy`
        // turned every GBK byte into U+FFFD, and `save` wrote that back — so
        // opening a legacy Chinese document and pressing Ctrl+S destroyed it.
        let dir = tempfile::tempdir().unwrap();
        // "中文" in GBK. Not valid UTF-8, which is what routes it to the detector.
        let original = b"\xD6\xD0\xCE\xC4\r\n";
        let path = write_file(dir.path(), "legacy.txt", original);

        let file = load(&path).unwrap();
        assert_eq!(file.encoding.name(), "GBK", "detected as GBK, not UTF-8");
        assert_eq!(file.text, "中文\n", "decoded, not replaced");
        assert!(
            !file.text.contains('\u{FFFD}'),
            "no replacement characters: {:?}",
            file.text
        );

        save(&file, &file.text, false).unwrap();
        assert_eq!(
            std::fs::read(&path).unwrap(),
            original,
            "an untouched save is byte-identical, still GBK"
        );
    }

    #[test]
    fn a_utf16_document_keeps_its_encoding_and_its_bom() {
        // `encoding_rs` has no UTF-16 encoder — `Encoding::encode` silently
        // emits UTF-8 for it. Writing a UTF-8 body under a UTF-16 BOM produces
        // a file nothing can read, so `write` encodes the code units itself.
        let dir = tempfile::tempdir().unwrap();
        let original = b"\xFF\xFEh\x00i\x00\n\x00";
        let path = write_file(dir.path(), "a.txt", original);

        let file = load(&path).unwrap();
        assert_eq!(file.encoding.name(), "UTF-16LE");
        assert!(file.had_bom);
        assert_eq!(file.text, "hi\n");

        save(&file, &file.text, false).unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), original);
    }

    #[test]
    fn a_utf8_file_is_never_handed_to_the_detector() {
        // Valid UTF-8 is taken at face value. A detector run over it can only
        // introduce error, and on a developer's machine nearly every file is
        // UTF-8 — including ones whose bytes a detector would happily read as
        // some legacy single-byte encoding.
        let dir = tempfile::tempdir().unwrap();
        let path = write_file(dir.path(), "a.md", "café ünicode ①②③\n".as_bytes());
        let file = load(&path).unwrap();
        assert_eq!(file.encoding.name(), "UTF-8");
        assert_eq!(file.text, "café ünicode ①②③\n");
    }

    #[test]
    fn two_saves_of_one_file_cannot_collide_on_a_temp_path() {
        // The temp name used to be derived from the target, so it was the same
        // on every save: two concurrent writers truncated each other's temp
        // file and one of them renamed a half-written document into place.
        let dir = tempfile::tempdir().unwrap();
        let path = write_file(dir.path(), "a.md", b"start\n");
        let file = load(&path).unwrap();

        let source = include_str!("fs.rs");
        let body = source
            .split_once("fn write(")
            .expect("write must exist")
            .1
            .split_once("\n#[cfg(test)]")
            .map(|(body, _)| body)
            .unwrap_or(source);
        assert!(
            !body.contains("with_extension"),
            "the temp path must not be derived from the target name: a \
             deterministic name is the same on every save, so two saves of one \
             document race on it"
        );
        assert!(
            body.contains("tempfile::Builder"),
            "the temp file must come from `tempfile`, which randomizes the name"
        );

        // And a file with no extension no longer produces `README..markturbo-tmp`.
        let plain = write_file(dir.path(), "README", b"x\n");
        let plain_file = load(&plain).unwrap();
        save(&plain_file, "y\n", false).unwrap();
        assert_eq!(std::fs::read_to_string(&plain).unwrap(), "y\n");
        assert!(file.path.exists());
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
