//! Full-text search over documents.
//!
//! Pure text and filesystem work, no GPUI: the view decides *which* documents
//! are in scope — the open tabs, a folder, the discovered harness — and this
//! decides what matches inside them.
//!
//! Two entry points rather than one, because the two sources differ in a way
//! that matters. An open tab's authoritative text is the editor's buffer, which
//! may hold unsaved edits; everything else has to be read from disk. Searching
//! the file for a tab the user has been typing in would report matches that are
//! no longer there and miss the ones that are, so [`search_text`] takes the
//! text it is given and [`search_files`] reads.

use std::path::{Path, PathBuf};

use crate::DocType;

/// What to look for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Query {
    pub text: String,
    /// Off by default: a search for `todo` should find `TODO`, which is how
    /// the word is actually written in the documents this app opens.
    pub case_sensitive: bool,
}

impl Query {
    pub fn new(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            case_sensitive: false,
        }
    }

    pub fn case_sensitive(mut self, yes: bool) -> Self {
        self.case_sensitive = yes;
        self
    }

    /// Whether this query is worth running.
    ///
    /// A blank query matches at every byte offset in every document, which is
    /// not a useful result — it is a hang with a progress bar.
    pub fn is_runnable(&self) -> bool {
        !self.text.trim().is_empty()
    }
}

/// One hit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Match {
    pub path: PathBuf,
    /// Byte offset of the match in the document, for [`crate::Document`]-style
    /// navigation.
    pub offset: usize,
    /// 1-based, as every editor counts them.
    pub line: usize,
    /// 1-based column, in characters rather than bytes — a byte column is
    /// wrong the moment a line contains anything non-ASCII.
    pub column: usize,
    /// The whole line the match is on, for the result row.
    pub line_text: String,
    /// Character offset of the match within `line_text`, so the row can
    /// highlight it without searching the line again.
    pub line_offset: usize,
}

/// How many results to collect before stopping.
///
/// A cap rather than a warning: a two-letter query against a large vault
/// matches hundreds of thousands of times, and neither the list nor the person
/// reading it wants them. [`Results::truncated`] is what says the cap was hit,
/// so the UI can say so rather than presenting a partial answer as complete.
pub const DEFAULT_LIMIT: usize = 500;

/// Files above this size are skipped.
///
/// A document this large is a generated artifact or a database dump, not
/// something a person is searching for a phrase in — and reading it costs more
/// than every real document in the folder combined.
const MAX_FILE_BYTES: u64 = 4 * 1024 * 1024;

/// Longest line kept in a result row.
///
/// A minified file is one line of a megabyte. Without this the row itself
/// becomes the memory problem the file-size cap was meant to prevent.
const MAX_LINE_CHARS: usize = 300;

/// What a search found.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Results {
    pub matches: Vec<Match>,
    /// True when the limit stopped the search before it ran out of documents.
    pub truncated: bool,
    /// How many distinct documents contributed a match.
    pub files: usize,
}

impl Results {
    pub fn is_empty(&self) -> bool {
        self.matches.is_empty()
    }
}

/// Search `text`, attributing every match to `path`.
///
/// Case-insensitivity is done by lowercasing both sides. That is not a Unicode
/// case-folding — `ß` does not match `SS` — but it is what every editor's
/// "ignore case" does, and the alternative pulls in a full folding table for a
/// difference nobody searching a Markdown file will notice.
pub fn search_text(path: &Path, text: &str, query: &Query, limit: usize, out: &mut Results) {
    if !query.is_runnable() || out.matches.len() >= limit {
        return;
    }
    let needle = if query.case_sensitive {
        query.text.clone()
    } else {
        query.text.to_lowercase()
    };
    let before = out.matches.len();

    // One pass over the lines rather than over the whole text: the line number,
    // the column and the row's own text all come from the line we are already
    // holding, so searching the document as one string would mean finding the
    // enclosing line again for every hit.
    //
    // The line number is carried rather than recomputed. Counting newlines from
    // the start of the document per line is the obvious spelling and makes the
    // whole search quadratic — on a 100K-line document that is the difference
    // between milliseconds and minutes.
    let mut offset = 0usize;
    for (index, line_text) in text.split_inclusive('\n').enumerate() {
        let line_number = index + 1;
        let haystack = if query.case_sensitive {
            line_text.to_string()
        } else {
            line_text.to_lowercase()
        };

        // Byte indices into the lowercased line map back onto the original only
        // when lowercasing preserved every byte length. It does not in general
        // (`İ` lowercases to two chars), so a shifted index is caught here and
        // the line is reported once at its start rather than at a wrong column.
        let aligned = haystack.len() == line_text.len();

        let mut from = 0usize;
        while let Some(found) = haystack[from..].find(&needle) {
            let at = from + found;
            let (column, line_offset, byte_at) = if aligned {
                let chars_before = line_text[..at].chars().count();
                (chars_before + 1, chars_before, at)
            } else {
                (1, 0, 0)
            };
            out.matches.push(Match {
                path: path.to_path_buf(),
                offset: offset + byte_at,
                line: line_number,
                column,
                line_text: clip(line_text.trim_end_matches(['\n', '\r'])),
                line_offset,
            });
            if out.matches.len() >= limit {
                out.truncated = true;
                break;
            }
            // Advance past this match, never by zero: an empty needle is
            // rejected by `is_runnable`, but a defensive step keeps a future
            // caller from spinning here forever.
            from = at + needle.len().max(1);
            if from >= haystack.len() {
                break;
            }
            if !aligned {
                // The column was already reported as the line start; reporting
                // the same line again would just repeat that row.
                break;
            }
        }

        if out.matches.len() >= limit {
            out.truncated = true;
            break;
        }
        offset += line_text.len();
    }

    if out.matches.len() > before {
        out.files += 1;
    }
}

/// Truncate a result line so one minified file cannot dominate the list.
fn clip(line: &str) -> String {
    if line.chars().count() <= MAX_LINE_CHARS {
        return line.to_string();
    }
    let head: String = line.chars().take(MAX_LINE_CHARS).collect();
    format!("{head}…")
}

/// Search each of `paths`, reading from disk.
///
/// `skip` names documents whose text the caller already has — the open tabs,
/// whose unsaved edits make the file on disk the wrong thing to read. The
/// caller passes those to [`search_text`] instead.
pub fn search_files(paths: &[PathBuf], query: &Query, limit: usize, out: &mut Results) {
    if !query.is_runnable() {
        return;
    }
    for path in paths {
        if out.matches.len() >= limit {
            out.truncated = true;
            return;
        }
        let Ok(meta) = std::fs::metadata(path) else {
            continue;
        };
        if meta.len() > MAX_FILE_BYTES {
            continue;
        }
        // Lossy rather than skipping on invalid UTF-8: a document with one bad
        // byte still opens in this app, so it should still be searchable.
        let Ok(bytes) = std::fs::read(path) else {
            continue;
        };
        let text = String::from_utf8_lossy(&bytes);
        search_text(path, &text, query, limit, out);
    }
}

/// Directories never worth walking. Same list the skill and instruction
/// walkers use, plus the build outputs a documentation search would drown in.
const SKIP_DIRS: &[&str] = &[
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
];

/// How deep to walk a folder looking for documents.
///
/// Deep enough for a real repository's `docs/guides/advanced/x.md`, bounded so
/// a search cannot wander into a vendored tree that the skip list missed.
const MAX_WALK_DEPTH: usize = 12;

/// Every document under `root`, in a stable order.
///
/// Only what [`DocType::is_document`] accepts: searching a folder means
/// searching the documents in it, and a binary that happens to contain the
/// query bytes is not a result anyone wants.
pub fn document_paths(root: &Path) -> Vec<PathBuf> {
    let mut found = Vec::new();
    walk(root, 0, &mut found);
    // Sorted so the result list is the same across runs — `read_dir` order is
    // not guaranteed, and a search that reorders itself between two identical
    // queries reads as a bug.
    found.sort();
    found
}

fn walk(dir: &Path, depth: usize, out: &mut Vec<PathBuf>) {
    if depth > MAX_WALK_DEPTH {
        return;
    }
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        // Symlinked directories are not followed: a junctioned harness
        // directory pointing at an ancestor would otherwise be walked until the
        // depth cap on every branch.
        if file_type.is_dir() {
            if SKIP_DIRS.contains(&name) || file_type.is_symlink() {
                continue;
            }
            walk(&path, depth + 1, out);
        } else if DocType::of(&path).is_document() {
            out.push(path);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(text: &str, query: &str) -> Results {
        let mut out = Results::default();
        search_text(
            Path::new("a.md"),
            text,
            &Query::new(query),
            DEFAULT_LIMIT,
            &mut out,
        );
        out
    }

    #[test]
    fn reports_line_and_column_one_based() {
        let out = run("alpha\nbeta gamma\n", "gamma");
        assert_eq!(out.matches.len(), 1);
        let m = &out.matches[0];
        assert_eq!(m.line, 2);
        assert_eq!(m.column, 6, "columns count from 1, like every editor");
        assert_eq!(m.line_text, "beta gamma");
        assert_eq!(&"alpha\nbeta gamma\n"[m.offset..m.offset + 5], "gamma");
    }

    #[test]
    fn is_case_insensitive_by_default_and_exact_on_request() {
        assert_eq!(run("A TODO here\n", "todo").matches.len(), 1);

        let mut out = Results::default();
        search_text(
            Path::new("a.md"),
            "A TODO here\n",
            &Query::new("todo").case_sensitive(true),
            DEFAULT_LIMIT,
            &mut out,
        );
        assert!(out.is_empty(), "case-sensitive must not match TODO");
    }

    #[test]
    fn finds_every_occurrence_on_one_line() {
        let out = run("ab ab ab\n", "ab");
        assert_eq!(out.matches.len(), 3);
        let columns: Vec<usize> = out.matches.iter().map(|m| m.column).collect();
        assert_eq!(columns, vec![1, 4, 7]);
    }

    #[test]
    fn columns_count_characters_not_bytes() {
        // The byte column would be 7 here and point into the middle of a
        // codepoint — which is both wrong and, if used to slice, a panic.
        let out = run("中文字 x\n", "x");
        assert_eq!(out.matches[0].column, 5);
        // And the byte offset still slices cleanly.
        let text = "中文字 x\n";
        assert_eq!(&text[out.matches[0].offset..out.matches[0].offset + 1], "x");
    }

    #[test]
    fn the_offset_survives_case_insensitive_matching() {
        // Lowercasing the haystack is what makes the match findable; the offset
        // has to index the *original* text or opening the result lands in the
        // wrong place.
        let text = "Some GAMMA here\n";
        let out = run(text, "gamma");
        assert_eq!(
            &text[out.matches[0].offset..out.matches[0].offset + 5],
            "GAMMA"
        );
    }

    #[test]
    fn a_blank_query_finds_nothing_rather_than_everything() {
        for query in ["", "   ", "\t\n"] {
            assert!(
                run("anything at all\n", query).is_empty(),
                "{query:?} must not match"
            );
            assert!(!Query::new(query).is_runnable());
        }
    }

    #[test]
    fn the_limit_stops_the_search_and_says_so() {
        let text = "x\n".repeat(50);
        let mut out = Results::default();
        search_text(Path::new("a.md"), &text, &Query::new("x"), 10, &mut out);
        assert_eq!(out.matches.len(), 10);
        assert!(
            out.truncated,
            "hitting the cap must be visible to the caller"
        );
    }

    #[test]
    fn a_long_line_is_clipped_in_the_result_row() {
        // A minified file is one line of a megabyte; without the clip the row
        // becomes the memory problem the file-size cap was meant to prevent.
        let text = format!("{}needle{}\n", "a".repeat(5000), "b".repeat(5000));
        let out = run(&text, "needle");
        assert_eq!(out.matches.len(), 1);
        assert!(out.matches[0].line_text.chars().count() <= MAX_LINE_CHARS + 1);
    }

    #[test]
    fn counts_the_files_that_contributed() {
        let mut out = Results::default();
        for name in ["a.md", "b.md", "c.md"] {
            search_text(
                Path::new(name),
                if name == "c.md" { "nothing\n" } else { "hit\n" },
                &Query::new("hit"),
                DEFAULT_LIMIT,
                &mut out,
            );
        }
        assert_eq!(out.matches.len(), 2);
        assert_eq!(out.files, 2, "a file with no match must not be counted");
    }

    #[test]
    fn walking_a_folder_finds_documents_and_skips_noise() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::write(root.join("README.md"), "x").unwrap();
        std::fs::write(root.join("notes.txt"), "x").unwrap();
        std::fs::create_dir_all(root.join("docs/deep")).unwrap();
        std::fs::write(root.join("docs/deep/guide.md"), "x").unwrap();
        std::fs::create_dir_all(root.join("node_modules/pkg")).unwrap();
        std::fs::write(root.join("node_modules/pkg/readme.md"), "x").unwrap();

        let paths = document_paths(root);
        let names: Vec<String> = paths
            .iter()
            .map(|p| p.file_name().unwrap().to_string_lossy().to_string())
            .collect();
        assert!(names.contains(&"README.md".to_string()));
        assert!(names.contains(&"guide.md".to_string()), "must descend");
        assert!(
            !paths
                .iter()
                .any(|p| p.components().any(|c| c.as_os_str() == "node_modules")),
            "node_modules is where a documentation search goes to die"
        );
        assert!(!names.contains(&"notes.txt".to_string()));
    }

    #[test]
    fn searching_files_reads_them() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a.md");
        std::fs::write(&path, "one\ntwo needle three\n").unwrap();

        let mut out = Results::default();
        search_files(
            &[path.clone(), dir.path().join("missing.md")],
            &Query::new("needle"),
            DEFAULT_LIMIT,
            &mut out,
        );
        assert_eq!(
            out.matches.len(),
            1,
            "a missing file must be skipped, not fatal"
        );
        assert_eq!(out.matches[0].line, 2);
        assert_eq!(out.matches[0].path, path);
    }

    #[test]
    fn an_enormous_file_is_skipped() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("huge.md");
        // Just over the cap, filled with the query so a failure to skip is
        // unmistakable rather than a subtle count difference.
        let text = "needle\n".repeat((MAX_FILE_BYTES as usize / 7) + 16);
        std::fs::write(&path, &text).unwrap();

        let mut out = Results::default();
        search_files(&[path], &Query::new("needle"), DEFAULT_LIMIT, &mut out);
        assert!(
            out.is_empty(),
            "a 4MB document is a generated artifact, not something being read"
        );
    }

    /// Searching must be linear in the document, not quadratic.
    ///
    /// The obvious spelling — count newlines from the start of the text to find
    /// each line's number — is quadratic, and it does not fail, it just takes
    /// minutes on the 100K-line documents this app already has a parser test
    /// for. A wall-clock bound is the only thing that catches it.
    ///
    /// The needle appears **once, at the end**, on purpose. Filling the
    /// document with matches instead makes this test vacuous: the result cap
    /// stops the walk after [`DEFAULT_LIMIT`] lines, so the remaining 99,500
    /// are never visited and the quadratic term never shows up.
    #[test]
    fn a_large_document_searches_in_bounded_time() {
        let mut text = "some ordinary line of prose here\n".repeat(50_000);
        text.push_str("the needle\n");

        let started = std::time::Instant::now();
        let out = run(&text, "needle");
        let elapsed = started.elapsed();

        assert_eq!(out.matches.len(), 1);
        assert_eq!(out.matches[0].line, 50_001);
        assert!(
            elapsed < std::time::Duration::from_secs(3),
            "searching 50K lines took {elapsed:?}; the line number is being \
             recomputed from the start of the document per line"
        );
    }
}
