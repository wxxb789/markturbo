//! Shared walk policy: which directories to skip, and what counts as binary.
//!
//! Five walkers grew five ignore lists. They drifted — the folder search's had
//! ten names, the skill and instruction walkers five, and a comment in
//! `search.rs` claimed all three were the same list. A directory that is noise
//! to one walker is noise to all of them, so there is one list here and the
//! walkers spell out only what they legitimately differ on: how deep they go
//! and what they collect.
//!
//! This lives in `mt-doc` rather than `mt-app` because both crates walk. What
//! stays in `mt-app` is `read_dir`, which returns `FileNode` — a UI-tree type
//! with children and an expansion contract. Moving that here to reunite the
//! walkers would pull a view model into the document engine for one caller,
//! which is the wrong direction: `mt-doc` must never depend on the UI.

use std::path::Path;

/// Directory names never worth walking, watching, or showing.
///
/// The union of what the five callers separately maintained. Erring toward
/// the longer list is deliberate: every name here is machine-owned build
/// output or a package cache, so a walker that skips one it did not
/// previously skip loses nothing a person was looking for, while one that
/// descends into `node_modules` on a real repository loses the search.
///
/// Matched on the directory's own name, not its path, so a nested
/// `crates/x/target` is caught the same as a top-level one.
pub const SKIP_DIRS: &[&str] = &[
    ".git",
    "node_modules",
    "target",
    "dist",
    "build",
    "__pycache__",
    ".venv",
    "venv",
    ".next",
    ".turbo",
    ".svelte-kit",
    ".parcel-cache",
    ".pytest_cache",
];

/// True when a directory of this name should never be descended into.
pub fn is_noise_dir(name: &str) -> bool {
    SKIP_DIRS.contains(&name)
}

/// True when any component of `path` below `root` is a skipped directory.
///
/// For callers handed a whole path rather than one entry at a time — the
/// filesystem watcher gets absolute paths from the OS and has no walk of its
/// own to prune. `root` is stripped first so a workspace that itself lives
/// under a directory named `build` is not filtered out entirely.
pub fn is_noise_path(path: &Path, root: &Path) -> bool {
    path.strip_prefix(root)
        .unwrap_or(path)
        .components()
        .filter_map(|c| c.as_os_str().to_str())
        .any(is_noise_dir)
}

/// How much of a file is examined before deciding it is binary.
const SNIFF_BYTES: usize = 8 * 1024;

/// True when `bytes` contain a NUL within the sniff window.
///
/// The heuristic `git` uses, and it is one comparison rather than a table of
/// magic numbers. A NUL cannot appear in valid UTF-8 text that anyone edits,
/// and every common binary format emits one within its first few hundred
/// bytes.
///
/// Takes bytes rather than a path so a caller that has already read the file —
/// the folder search reads every candidate anyway — pays nothing for the
/// check. [`looks_binary`] is the variant for a caller holding only a path.
pub fn bytes_look_binary(bytes: &[u8]) -> bool {
    bytes[..bytes.len().min(SNIFF_BYTES)].contains(&0)
}

/// True when the first 8 KB of `path` contain a NUL byte.
///
/// **This reads the file.** Call it when the user picks one thing, never while
/// listing a directory: over a flat 5,000-file folder the sniff measures 4.36s
/// against 4.0ms for the `read_dir` alone — ~1000x — and per-entry is exactly
/// how it was first written. A caller that already holds the bytes wants
/// [`bytes_look_binary`].
///
/// An unreadable file is reported as *not* binary: the open will fail again
/// when the user acts on it, and that failure carries its own message. Hiding
/// the entry here instead would make a permission problem look like a missing
/// file.
pub fn looks_binary(path: &Path) -> bool {
    use std::io::Read as _;

    let Ok(mut file) = std::fs::File::open(path) else {
        return false;
    };
    let mut head = [0u8; SNIFF_BYTES];
    let Ok(read) = file.read(&mut head) else {
        return false;
    };
    head[..read].contains(&0)
}

/// True when `path` is worth opening in an editor at all.
///
/// Two gates, cheapest first: [`crate::DocType`] is an allowlist, so `.png`,
/// `.zip` and every other extension nobody edits here are rejected on the name
/// alone, and the content sniff then settles what a name cannot — a NUL-filled
/// `.log` is on the text allowlist and is still not a document.
///
/// One answer for the whole codebase. The file tree, the drop handler and the
/// folder search each used to decide this for themselves, and the search's
/// answer was the extension alone, so a binary wearing a text extension became
/// a search result whose "matching line" was a run of raw bytes.
///
/// Reads the file; see [`looks_binary`] on where that is affordable.
pub fn is_openable(path: &Path) -> bool {
    crate::DocType::of(path).is_document() && !looks_binary(path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn the_skip_list_covers_every_name_the_five_walkers_maintained() {
        // The defect this catches: consolidation that silently drops a name.
        // Before this module there were five lists — the file tree's eleven,
        // the folder search's ten, the watcher's six, and the skill and
        // instruction walkers' five each — and every name in any of them must
        // survive here or some walker regressed into a `node_modules`.
        for name in [
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
        ] {
            assert!(is_noise_dir(name), "{name} was on a walker's list");
        }
        // …and an ordinary directory is not swept up with them.
        assert!(!is_noise_dir("docs"));
        assert!(!is_noise_dir("src"));
        // A name that merely starts with a skipped one is a different
        // directory: `target-audience/` is prose, not build output.
        assert!(!is_noise_dir("target-audience"));
    }

    #[test]
    fn the_list_has_no_duplicates() {
        // Five lists merged by hand is exactly how a name lands here twice.
        let mut names: Vec<&str> = SKIP_DIRS.to_vec();
        names.sort_unstable();
        let before = names.len();
        names.dedup();
        assert_eq!(before, names.len(), "duplicate in {SKIP_DIRS:?}");
    }

    #[test]
    fn a_file_name_never_belongs_on_a_directory_list() {
        // `.DS_Store` sat in the file tree's directory list, where it could
        // never match: it is a file, and the list is consulted per directory
        // name. It is excluded where it always actually was — `DocType::of`
        // classifies it `Other`, so it is listed but not openable, which is
        // what the tree wants for any file the user can see in Finder.
        assert!(!is_noise_dir(".DS_Store"));
        assert!(!crate::DocType::of(Path::new(".DS_Store")).is_document());
    }

    #[test]
    fn a_nested_skipped_directory_is_caught_by_path() {
        // The watcher gets absolute paths from the OS with no walk to prune,
        // so the check has to look at every component or a `cargo build` deep
        // in a monorepo floods the UI with reload prompts.
        let root = Path::new("/w");
        assert!(is_noise_path(Path::new("/w/a/node_modules/b/c.md"), root));
        assert!(is_noise_path(Path::new("/w/.git/HEAD"), root));
        assert!(!is_noise_path(
            Path::new("/w/docs/target-audience.md"),
            root
        ));
    }

    #[test]
    fn the_root_is_stripped_before_the_components_are_judged() {
        // A workspace whose own path contains `build` would otherwise have
        // every file in it filtered out — the whole tree would look empty.
        let root = Path::new("/home/me/build");
        assert!(!is_noise_path(Path::new("/home/me/build/notes.md"), root));
        assert!(is_noise_path(
            Path::new("/home/me/build/app/dist/bundle.js"),
            root
        ));
    }

    #[test]
    fn a_nul_byte_is_what_separates_binary_from_text() {
        assert!(bytes_look_binary(b"text\0more"));
        assert!(bytes_look_binary(b"\0"));
        assert!(!bytes_look_binary(b"fn main() {}\n"));
        assert!(!bytes_look_binary(b""));
        // Multibyte UTF-8 never produces a NUL byte, so CJK prose must not be
        // mistaken for binary — the failure would hide a user's own notes.
        assert!(!bytes_look_binary("标题\n中文正文。\n".as_bytes()));
        assert!(!bytes_look_binary("🚀 ship it\n".as_bytes()));
        // A legacy-encoded document is text too: GBK bytes are not valid UTF-8
        // but contain no NUL, and `mt_app::fs::load` detects and decodes them
        // properly — refusing to open one would be a regression, not safety.
        assert!(!bytes_look_binary(b"\xD6\xD0\xCE\xC4\r\n"));
    }

    #[test]
    fn a_nul_past_the_sniff_window_does_not_count() {
        // Bounding the sniff is what keeps it a fixed cost on a file that may
        // be gigabytes. Every format that matters declares itself in its first
        // bytes, so a longer read buys nothing.
        let mut late = vec![b'a'; SNIFF_BYTES + 1024];
        late.push(0);
        assert!(!bytes_look_binary(&late));
    }

    #[test]
    fn the_path_and_byte_sniffs_agree() {
        // Two spellings of one heuristic drift apart the moment only one is
        // maintained. The file tree uses the path form and the folder search
        // the byte form, and a file admitted by one and refused by the other
        // is the inconsistency this whole module exists to remove.
        let dir = tempfile::tempdir().unwrap();
        for (name, bytes) in [
            ("text.md", &b"# Hi\n"[..]),
            ("blob.log", &b"gz\0\x01\x02binary"[..]),
            ("empty", &b""[..]),
        ] {
            let path = dir.path().join(name);
            std::fs::write(&path, bytes).unwrap();
            assert_eq!(
                looks_binary(&path),
                bytes_look_binary(bytes),
                "{name} disagrees"
            );
        }
    }

    #[test]
    fn an_unreadable_file_is_reported_as_text_not_binary() {
        // Hiding a permission problem behind "binary" makes it look like a
        // missing file; letting the open proceed surfaces the real error.
        let dir = tempfile::tempdir().unwrap();
        assert!(!looks_binary(&dir.path().join("absent")));
    }

    #[test]
    fn the_open_gate_needs_both_the_extension_and_the_contents() {
        // An extension-only gate admits the last one; a sniff-only gate admits
        // a `.png` full of ASCII. Both halves are load-bearing.
        let dir = tempfile::tempdir().unwrap();
        let write = |name: &str, bytes: &[u8]| -> PathBuf {
            let path = dir.path().join(name);
            std::fs::write(&path, bytes).unwrap();
            path
        };

        assert!(is_openable(&write("README.md", b"# Hi\n")));
        assert!(is_openable(&write("main.rs", b"fn main() {}\n")));
        assert!(!is_openable(&write("logo.png", b"\x89PNG\r\n\x1a\n\0")));
        assert!(!is_openable(&write("blob.log", b"gz\0\x01\x02binary")));
    }
}
