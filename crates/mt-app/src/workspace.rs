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
    pub fn is_openable(&self) -> bool {
        self.doc_type.is_some_and(DocType::is_document)
    }
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

        let node = FileNode {
            name: name.to_string(),
            is_dir,
            doc_type: (!is_dir).then(|| DocType::of(&entry_path)),
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
        std::fs::create_dir(root.join("docs")).unwrap();
        std::fs::write(root.join("docs/guide.md"), "# Guide\n").unwrap();
        std::fs::create_dir(root.join("node_modules")).unwrap();
        std::fs::write(root.join("node_modules/pkg.md"), "x").unwrap();
        std::fs::create_dir(root.join(".git")).unwrap();
        dir
    }

    #[test]
    fn lists_directories_before_files_and_skips_noise() {
        let dir = fixture();
        let nodes = read_dir(dir.path()).unwrap();
        let names: Vec<_> = nodes.iter().map(|n| n.name.as_str()).collect();
        // Directories first, then files sorted case-insensitively — so
        // `notes.txt` precedes `README.md`, as in every file explorer.
        assert_eq!(names, vec!["docs", "notes.txt", "README.md"]);
        // And the noise directories never appear.
        assert!(!names.contains(&"node_modules"));
        assert!(!names.contains(&".git"));
    }

    #[test]
    fn marks_openable_documents() {
        let dir = fixture();
        let nodes = read_dir(dir.path()).unwrap();
        let readme = nodes.iter().find(|n| n.name == "README.md").unwrap();
        let txt = nodes.iter().find(|n| n.name == "notes.txt").unwrap();
        assert!(readme.is_openable());
        assert!(
            !txt.is_openable(),
            "plain text is not a document view target"
        );
    }

    #[test]
    fn deep_read_fills_children() {
        let dir = fixture();
        let nodes = read_dir_deep(dir.path(), 1).unwrap();
        let docs = nodes.iter().find(|n| n.name == "docs").unwrap();
        assert_eq!(docs.children.len(), 1);
        assert_eq!(docs.children[0].name, "guide.md");
    }

    #[test]
    fn depth_zero_does_not_descend() {
        let dir = fixture();
        let nodes = read_dir_deep(dir.path(), 0).unwrap();
        let docs = nodes.iter().find(|n| n.name == "docs").unwrap();
        assert!(docs.children.is_empty());
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
