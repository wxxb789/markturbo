//! Workspace directory tree.
//!
//! The filesystem is the source of truth. This module never caches file
//! contents — it caches only the tree shape, and re-reads on open.

use std::path::{Path, PathBuf};

use crate::DocType;

use super::walk;

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
    /// True when the entry is a symlink or junction, whatever it points at.
    ///
    /// Recorded during the read because [`read_dir`] already holds the
    /// [`std::fs::FileType`]; deriving it later would cost a
    /// `symlink_metadata` per directory, which is the per-entry syscall this
    /// module is built to avoid. [`read_dir_deep`] is what consults it.
    pub is_symlink: bool,
}

impl FileNode {
    /// True when opening this file in the document view makes sense.
    ///
    /// **This reads the file** — see [`is_openable`] on where
    /// that is affordable. Never call it while listing a directory.
    pub fn is_openable(&self) -> bool {
        !self.is_dir && is_openable(&self.path)
    }
}

/// Re-exported so the workspace tree and its consumers share the same gate as
/// folder search and file opening.
pub use super::walk::is_openable;

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
        if walk::is_noise_dir(name) {
            continue;
        }
        // `file_type` avoids a stat per entry on Windows; fall back to metadata
        // only when it fails (broken symlink, permission).
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        let is_symlink = file_type.is_symlink();
        // A symlink's own type says only that it is a link; `is_dir` follows it
        // to answer what the tree must show.
        let is_dir = if is_symlink {
            entry_path.is_dir()
        } else {
            file_type.is_dir()
        };

        let doc_type = (!is_dir).then(|| DocType::of(&entry_path));
        let node = FileNode {
            name: name.to_string(),
            is_dir,
            is_symlink,
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
///
/// Symlinked directories are listed but never descended into, which is what
/// [`crate::workspace::search::document_paths`] already does and this did not.
/// A junction pointing at an ancestor — such as `node_modules/.bin` on Windows
/// or `latest -> .` in a build tree — otherwise makes every branch below it
/// `depth` levels deep again, and the read the explorer runs on a background
/// task never returns.
/// `depth` alone bounds the *recursion*, not the *work*: a cycle at depth 1 of
/// a depth-3 walk still multiplies out.
pub fn read_dir_deep(path: &Path, depth: usize) -> std::io::Result<Vec<FileNode>> {
    let mut nodes = read_dir(path)?;
    if depth > 0 {
        for node in nodes.iter_mut().filter(|n| n.is_dir && !n.is_symlink) {
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
        // own file manager looks broken. Not openable, because the round trip
        // is not lossless for a binary: `fs::load` detects an encoding for it
        // (a detector fed executable bytes returns *something*), decodes, and
        // `fs::save` re-encodes from the editor's `String` — so every byte the
        // decoder could not map is gone. The stamp check in `save` catches an
        // external rewrite, not this: nothing changed on disk, so the write is
        // authorized and destroys the file it was authorized to write.
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

    /// Every walker in the repo skips the same directories.
    ///
    /// The defect this catches: five separate lists, four distinct contents.
    /// The file tree's had `.venv` and `.next`; the skill and instruction
    /// walkers' did not, so a `.venv` full of vendored packages next to a
    /// skills root was walked in full. Consolidation only holds if the tree
    /// keeps consulting the shared list rather than growing a private one.
    #[test]
    fn the_tree_skips_exactly_what_every_other_walker_skips() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::write(root.join("keep.md"), "x").unwrap();
        for noise in crate::workspace::walk::SKIP_DIRS {
            std::fs::create_dir(root.join(noise)).unwrap();
        }

        let names: Vec<String> = read_dir(root)
            .unwrap()
            .iter()
            .map(|n| n.name.clone())
            .collect();
        assert_eq!(names, vec!["keep.md".to_string()], "got {names:?}");
    }

    #[test]
    fn a_symlinked_directory_is_listed_but_never_descended() {
        // The defect this catches: `read_dir_deep` had no cycle guard, while
        // `crate::workspace::search`'s walk did. A junction pointing at an
        // ancestor makes every branch below it `depth` levels deep again:
        // `depth` bounds recursion, not work, and the explorer's background
        // read never returns. Listed rather than hidden, so the user can see the
        // link exists in a tree that matches their file manager.
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::create_dir(root.join("real")).unwrap();
        std::fs::write(root.join("real/a.md"), "x").unwrap();

        // Windows needs Developer Mode or elevation for `symlink_dir`, so this
        // reports and returns rather than failing on an unprivileged machine.
        #[cfg(windows)]
        let made = std::os::windows::fs::symlink_dir(root.join("real"), root.join("link")).is_ok();
        #[cfg(unix)]
        let made = std::os::unix::fs::symlink(root.join("real"), root.join("link")).is_ok();
        #[cfg(not(any(windows, unix)))]
        let made = false;
        if !made {
            eprintln!("skipping: this session cannot create directory symlinks");
            return;
        }

        let nodes = read_dir_deep(root, 2).unwrap();
        let link = node(&nodes, "link");
        assert!(link.is_dir, "a link to a directory shows as a directory");
        assert!(link.is_symlink);
        assert!(link.children.is_empty(), "a link must not be walked");
        // …and the real directory still is, or the guard went too far.
        assert_eq!(node(&nodes, "real").children.len(), 1);
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
