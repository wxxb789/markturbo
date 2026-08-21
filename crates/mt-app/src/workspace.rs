//! Workspace: an opened directory, its file tree, and filesystem watching.
//!
//! The filesystem is the source of truth. This module never caches file
//! contents — it caches only the tree shape, and re-reads on open.

use std::path::{Path, PathBuf};

use mt_doc::DocType;

/// A node in the workspace file tree.
#[derive(Debug, Clone)]
pub struct FileNode {
    pub path: PathBuf,
    pub name: String,
    pub is_dir: bool,
    /// Populated for directories. Empty until the node is expanded.
    pub children: Vec<FileNode>,
    /// `None` for directories.
    pub doc_type: Option<DocType>,
}

impl FileNode {
    /// True when opening this file in the document view makes sense.
    ///
    /// Delegates to [`is_openable`], which reads the file — see its note on
    /// where this may be called from.
    pub fn is_openable(&self) -> bool {
        !self.is_dir && is_openable(&self.path)
    }
}

/// True when opening `path` in the document view makes sense.
///
/// Two gates, cheapest first: [`DocType`] is an allowlist, so `.png`, `.zip`
/// and every other extension nobody edits here are rejected on the name alone,
/// and [`looks_binary`] then settles what a name cannot.
///
/// **This reads the file.** Call it when the user picks one thing, never while
/// listing a directory. Over a flat 5,000-file folder the sniff measures 4.36s
/// against 4.0ms for the `read_dir` alone — ~1000x — and per-entry is exactly
/// how it was first written, one screenful below the `file_type` call that
/// exists to avoid a per-entry `stat`.
pub fn is_openable(path: &Path) -> bool {
    DocType::of(path).is_document() && !looks_binary(path)
}

/// True when the first 8 KB of `path` contain a NUL byte.
///
/// The heuristic `git` uses, and it is one read rather than a table of magic
/// numbers. A NUL cannot appear in valid UTF-8 text that anyone edits, and
/// every common binary format emits one within its first few hundred bytes;
/// bounding the sniff keeps it a fixed cost on a file that may be gigabytes.
///
/// What this prevents is data loss, not mojibake. [`crate::fs::load`] reads
/// through `String::from_utf8_lossy`, so every byte that is not valid UTF-8
/// becomes U+FFFD in the buffer, and `Save` writes that buffer back with no
/// dirty check — one stray Ctrl+S on a wrongly-opened binary overwrites the
/// original bytes irrecoverably. Refusing the open is the only point at which
/// that is still cheap to stop.
///
/// An unreadable file is reported as *not* binary: the open will fail again
/// when the user clicks it, and that failure carries its own message. Hiding
/// the entry here instead would make a permission problem look like a missing
/// file.
fn looks_binary(path: &Path) -> bool {
    use std::io::Read as _;

    let Ok(mut file) = std::fs::File::open(path) else {
        return false;
    };
    let mut head = [0u8; 8 * 1024];
    let Ok(read) = file.read(&mut head) else {
        return false;
    };
    head[..read].contains(&0)
}

/// Directory names never worth showing: they are large, machine-owned, and
/// contain no documents a user edits here.
const IGNORED_DIRS: &[&str] = &[
    ".git",
    "node_modules",
    "target",
    ".venv",
    "venv",
    "__pycache__",
    ".next",
    ".turbo",
    "dist",
    "build",
    ".DS_Store",
];

fn is_ignored(name: &str) -> bool {
    IGNORED_DIRS.contains(&name)
}

/// Read one directory level.
///
/// Deliberately shallow: a workspace can be a monorepo, and eagerly walking it
/// would block startup. The tree fills in as the user expands.
pub fn read_dir(path: &Path) -> std::io::Result<Vec<FileNode>> {
    let mut dirs = Vec::new();
    let mut files = Vec::new();

    for entry in std::fs::read_dir(path)? {
        let Ok(entry) = entry else { continue };
        let entry_path = entry.path();
        let Some(name) = entry_path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        if is_ignored(name) {
            continue;
        }
        // `file_type` avoids a stat per entry on Windows; fall back to metadata
        // only when it fails (broken symlink, permission).
        let is_dir = match entry.file_type() {
            Ok(ft) if ft.is_symlink() => entry_path.is_dir(),
            Ok(ft) => ft.is_dir(),
            Err(_) => continue,
        };

        let doc_type = (!is_dir).then(|| DocType::of(&entry_path));
        let node = FileNode {
            name: name.to_string(),
            is_dir,
            doc_type,
            children: Vec::new(),
            path: entry_path,
        };
        if is_dir {
            dirs.push(node)
        } else {
            files.push(node)
        }
    }

    // Directories first, then files, each case-insensitively by name — the
    // ordering every file explorer uses, and stable across platforms whose
    // `read_dir` order differs.
    let by_name = |a: &FileNode, b: &FileNode| {
        a.name
            .to_lowercase()
            .cmp(&b.name.to_lowercase())
            .then_with(|| a.name.cmp(&b.name))
    };
    dirs.sort_by(by_name);
    files.sort_by(by_name);
    dirs.append(&mut files);
    Ok(dirs)
}

/// Read a directory recursively up to `depth` levels.
///
/// Used for the initial tree so the first screen is not empty, and for
/// expanding a node.
pub fn read_dir_deep(path: &Path, depth: usize) -> std::io::Result<Vec<FileNode>> {
    let mut nodes = read_dir(path)?;
    if depth > 0 {
        for node in nodes.iter_mut().filter(|n| n.is_dir) {
            node.children = read_dir_deep(&node.path, depth - 1).unwrap_or_default();
        }
    }
    Ok(nodes)
}

/// Display path of `path` relative to the workspace root, with forward slashes
/// so the UI reads the same on every platform.
pub fn display_relative(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .components()
        .filter_map(|c| c.as_os_str().to_str())
        .collect::<Vec<_>>()
        .join("/")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::write(root.join("README.md"), "# Hi\n").unwrap();
        std::fs::write(root.join("notes.txt"), "x").unwrap();
        std::fs::write(root.join("main.rs"), "fn main() {}\n").unwrap();
        // A real PNG header: the extension is not on the text allowlist, so
        // this must never reach the sniff at all.
        std::fs::write(root.join("logo.png"), b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR").unwrap();
        // …and a file that lies about itself, which is the only case the
        // extension cannot decide.
        std::fs::write(root.join("blob.log"), b"gz\0\x01\x02binary").unwrap();
        std::fs::create_dir(root.join("docs")).unwrap();
        std::fs::write(root.join("docs/guide.md"), "# Guide\n").unwrap();
        std::fs::create_dir(root.join("node_modules")).unwrap();
        std::fs::write(root.join("node_modules/pkg.md"), "x").unwrap();
        std::fs::create_dir(root.join(".git")).unwrap();
        dir
    }

    fn node<'a>(nodes: &'a [FileNode], name: &str) -> &'a FileNode {
        nodes.iter().find(|n| n.name == name).unwrap()
    }

    #[test]
    fn lists_directories_before_files_and_skips_noise() {
        let dir = fixture();
        let nodes = read_dir(dir.path()).unwrap();
        let names: Vec<_> = nodes.iter().map(|n| n.name.as_str()).collect();
        // Directories first, then files sorted case-insensitively — so
        // `notes.txt` precedes `README.md`, as in every file explorer.
        assert_eq!(
            names,
            vec![
                "docs",
                "blob.log",
                "logo.png",
                "main.rs",
                "notes.txt",
                "README.md"
            ]
        );
        // And the noise directories never appear.
        assert!(!names.contains(&"node_modules"));
        assert!(!names.contains(&".git"));
    }

    #[test]
    fn marks_openable_documents() {
        let dir = fixture();
        let nodes = read_dir(dir.path()).unwrap();
        assert!(node(&nodes, "README.md").is_openable());
        // Text files open in Source, which is what the tree has always shown
        // them for.
        assert!(node(&nodes, "notes.txt").is_openable());
        assert!(node(&nodes, "main.rs").is_openable());
    }

    #[test]
    fn binaries_are_listed_but_never_openable() {
        // Listed, because a tree that hides a file the user can see in their
        // own file manager looks broken. Not openable, because `fs::load`
        // decodes lossily and `Save` writes the buffer back unconditionally —
        // opening one and pressing Ctrl+S replaces its bytes with U+FFFD.
        let dir = fixture();
        let nodes = read_dir(dir.path()).unwrap();
        let png = node(&nodes, "logo.png");
        assert!(!png.is_openable(), "an image is not an editable document");
        let blob = node(&nodes, "blob.log");
        assert!(
            !blob.is_openable(),
            "`.log` is on the text allowlist, so only the content sniff can \
             catch this one"
        );
    }

    #[test]
    fn listing_a_directory_never_reads_a_file() {
        // The regression this guards: the sniff used to run per entry, one
        // screenful below the `file_type` call that exists to avoid a per-entry
        // `stat`. Measured over a flat 5,000-file folder it cost 4.36s against
        // 4.0ms for the `read_dir` alone. Asserted against the source because
        // the cost is a syscall count, which no return value exposes.
        let source = include_str!("workspace.rs");
        let body = source
            .split_once("pub fn read_dir(")
            .expect("read_dir must exist")
            .1;
        let body = body
            .split_once("\npub fn read_dir_deep(")
            .map_or(body, |(b, _)| b);
        assert!(
            !body.contains("looks_binary") && !body.contains("is_openable"),
            "`read_dir` must not sniff file contents: it runs per entry, and \
             the check belongs at the click that opens one file"
        );
    }

    #[test]
    fn the_open_gate_sniffs_contents_not_just_the_extension() {
        // `is_openable` is what the explorer's click handler calls, so this is
        // the path a real open takes. An extension-only gate would admit both
        // of the last two.
        let dir = tempfile::tempdir().unwrap();
        let write = |name: &str, bytes: &[u8]| {
            let path = dir.path().join(name);
            std::fs::write(&path, bytes).unwrap();
            path
        };

        assert!(is_openable(&write("README.md", b"# Hi\n")));
        assert!(is_openable(&write("main.rs", b"fn main() {}\n")));
        assert!(!is_openable(&write("logo.png", b"\x89PNG\r\n\x1a\n\0")));
        assert!(!is_openable(&write("blob.log", b"gz\0\x01\x02binary")));
    }

    #[test]
    fn a_nul_byte_is_what_separates_binary_from_text() {
        let dir = tempfile::tempdir().unwrap();
        let write = |name: &str, bytes: &[u8]| {
            let path = dir.path().join(name);
            std::fs::write(&path, bytes).unwrap();
            path
        };

        assert!(looks_binary(&write("has-nul", b"text\0more")));
        assert!(looks_binary(&write("leading-nul", b"\0")));
        assert!(!looks_binary(&write("ascii", b"fn main() {}\n")));
        // Multibyte UTF-8 never produces a NUL byte, so CJK prose must not be
        // mistaken for binary — the failure would hide a user's own notes.
        assert!(!looks_binary(&write(
            "cjk",
            "标题\n中文正文。\n".as_bytes()
        )));
        assert!(!looks_binary(&write("emoji", "🚀 ship it\n".as_bytes())));
        assert!(!looks_binary(&write("empty", b"")));
        // A NUL past the sniff window is not worth a longer read: every format
        // that matters declares itself in its first bytes.
        let mut late = vec![b'a'; 9 * 1024];
        late.push(0);
        assert!(!looks_binary(&write("late-nul", &late)));
        // A missing file is not binary — the open reports its own error.
        assert!(!looks_binary(&dir.path().join("absent")));
    }

    #[test]
    fn deep_read_fills_children() {
        let dir = fixture();
        let nodes = read_dir_deep(dir.path(), 1).unwrap();
        let docs = node(&nodes, "docs");
        assert_eq!(docs.children.len(), 1);
        assert_eq!(docs.children[0].name, "guide.md");
    }

    #[test]
    fn depth_zero_does_not_descend() {
        let dir = fixture();
        let nodes = read_dir_deep(dir.path(), 0).unwrap();
        assert!(node(&nodes, "docs").children.is_empty());
    }

    #[test]
    fn relative_display_uses_forward_slashes() {
        let root = Path::new("/a/b");
        let path = Path::new("/a/b/c/d.md");
        assert_eq!(display_relative(root, path), "c/d.md");
    }

    #[test]
    fn unreadable_directory_is_an_error_not_a_panic() {
        assert!(read_dir(Path::new("/definitely/not/here/xyz")).is_err());
        // …and the deep variant degrades to empty rather than propagating.
        let dir = tempfile::tempdir().unwrap();
        assert!(read_dir_deep(dir.path(), 3).unwrap().is_empty());
    }
}
