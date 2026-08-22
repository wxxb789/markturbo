//! Harness instruction files.
//!
//! A skill is one kind of agent artifact; the other is the instruction file a
//! harness reads unprompted — `CLAUDE.md`, `AGENTS.md`, `.cursor/rules/*`,
//! `*.instructions.md`. Those are what actually shape an agent's behavior in a
//! repository, and until now they were only reachable by finding them in the
//! file tree by hand.
//!
//! Discovery mirrors [`crate::skill`]: the same project/global split, the same
//! canonical-path deduplication, and the same rule that a file present in both
//! places appears once with the other path recorded as an alias.

use std::path::{Path, PathBuf};

use crate::DocType;

/// Where an instruction file was found.
///
/// The same distinction skills draw, and for the same reason: a global
/// `~/.claude/CLAUDE.md` applies to every project, so reading a repository's
/// effective instructions means reading both and knowing which is which.
pub use crate::skill::Origin;

/// One instruction file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Instruction {
    /// The file itself.
    pub path: PathBuf,
    /// Directory the search started from, which is what names the harness.
    pub root: PathBuf,
    /// The harness directory this root represents (`.claude`, `.github`), or
    /// empty when the root is the workspace itself.
    ///
    /// Stored rather than derived from `root`: the workspace directory may well
    /// be named with a leading dot (a temp dir, a dotfiles repo), so the path
    /// alone cannot say whether a component is a harness convention.
    pub harness_dir: String,
    pub origin: Origin,
    /// What kind of artifact it is, so the UI can label it without re-deriving.
    pub doc_type: DocType,
    /// Other paths reaching the same file — a junctioned harness directory
    /// typically produces several.
    pub aliases: Vec<PathBuf>,
}

impl Instruction {
    /// Display name: the path relative to its root, qualified by the harness
    /// directory it came from.
    ///
    /// Half a dozen harnesses call their file `AGENTS.md`, so a flat list of
    /// file names is a list of identical rows. What distinguishes them is which
    /// harness directory they came from — not the parent, which for a nested
    /// rule file is `rules` for everyone.
    pub fn label(&self) -> String {
        let relative = self
            .path
            .strip_prefix(&self.root)
            .ok()
            .map(|p| p.to_string_lossy().replace('\\', "/"))
            .unwrap_or_else(|| {
                self.path
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or_default()
                    .to_string()
            });
        if self.harness_dir.is_empty() {
            relative
        } else {
            format!("{}/{relative}", self.harness_dir)
        }
    }
}

/// Instruction file names a harness reads from a directory it owns.
///
/// `AGENTS.md` is the cross-vendor convention and `CLAUDE.md` the most widely
/// deployed; the rest are recognized because [`DocType`] already classifies
/// them, and a file the app can label is a file it should be able to find.
const INSTRUCTION_NAMES: &[&str] = &[
    "AGENTS.md",
    "CLAUDE.md",
    "CLAUDE.local.md",
    "GEMINI.md",
    "QWEN.md",
    "AGENT.md",
];

/// Directories to search relative to a harness root, in addition to the root.
///
/// Cursor keeps its rules one level down and Copilot keeps its instructions in
/// a sibling; both are conventions rather than a single file.
const NESTED: &[&str] = &["rules", "instructions", "memories"];

/// How deep to look inside a nested directory. One level: these hold rule files
/// directly, and a deeper walk would start picking up unrelated Markdown.
const NESTED_DEPTH: usize = 1;

/// Instruction directories no skills root points at.
///
/// The derivation below covers every harness that keeps skills *and*
/// instructions in one directory, which is most of them. These two are the
/// exceptions: Cursor's skills live in the shared `.agents/skills` while its
/// rules live in `.cursor/rules`, and Copilot's `*.instructions.md` live in
/// `.github`, which is not a skills root at all.
const EXTRA_ROOTS: &[&str] = &[".cursor", ".github"];

/// Roots to search for instruction files, relative to the workspace.
///
/// Derived from the harness table rather than listed separately: a harness that
/// keeps skills in `.claude/skills` keeps its instructions in `.claude`, so the
/// parent of each skills root is where to look. The workspace root itself comes
/// first because that is where `AGENTS.md` and `CLAUDE.md` actually live.
pub fn project_roots() -> Vec<PathBuf> {
    let mut roots: Vec<PathBuf> = vec![PathBuf::new()];
    for root in crate::harness::project_roots() {
        // `skills` (OpenClaw's bare root) has no meaningful parent — its parent
        // is the workspace, which is already first.
        let Some(parent) = root.rsplit_once('/').map(|(head, _)| head) else {
            continue;
        };
        let path: PathBuf = parent.split('/').collect();
        if !roots.contains(&path) {
            roots.push(path);
        }
    }
    for extra in EXTRA_ROOTS {
        let path: PathBuf = extra.split('/').collect();
        if !roots.contains(&path) {
            roots.push(path);
        }
    }
    roots
}

/// Global instruction directories on this machine.
///
/// The parents of the global skills roots, deduplicated — `~/.claude/skills`
/// yields `~/.claude`, which is where `CLAUDE.md` lives.
pub fn global_roots() -> Vec<PathBuf> {
    let mut roots: Vec<PathBuf> = Vec::new();
    for root in crate::harness::global_roots() {
        let Some(parent) = root.parent() else {
            continue;
        };
        let parent = parent.to_path_buf();
        if !roots.contains(&parent) {
            roots.push(parent);
        }
    }
    roots
}

/// Discover instruction files under `workspace`.
///
/// Workspace only, like [`crate::skill::discover`] — hermetic by default so a
/// test cannot pick up the developer's own `~/.claude/CLAUDE.md`.
pub fn discover(workspace: &Path) -> Vec<Instruction> {
    discover_with(workspace, false)
}

/// Discover instruction files, optionally including the global directories.
pub fn discover_with(workspace: &Path, global: bool) -> Vec<Instruction> {
    let mut found = Found::default();

    for rel in project_roots() {
        let root = workspace.join(&rel);
        // The relative root *is* the harness directory, which is why it is
        // carried rather than recovered from the absolute path: a workspace
        // named `.tmp1234` would otherwise look like a harness convention.
        let harness = rel.to_string_lossy().replace('\\', "/");
        scan(&root, &root, &harness, Origin::Workspace, &mut found);
    }
    if global {
        for root in global_roots() {
            let harness = root
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or_default()
                .to_string();
            scan(&root, &root, &harness, Origin::Global, &mut found);
        }
    }

    let mut instructions = found.instructions;
    instructions.sort_by(|a, b| {
        a.origin
            .cmp(&b.origin)
            .then_with(|| a.label().cmp(&b.label()))
            .then_with(|| a.path.cmp(&b.path))
    });
    instructions
}

/// Collect instruction files directly in `dir`, plus one level into the
/// conventional nested directories.
fn scan(root: &Path, dir: &Path, harness: &str, origin: Origin, out: &mut Found) {
    if !dir.is_dir() {
        return;
    }
    collect(root, dir, harness, origin, out);
    for nested in NESTED {
        let child = dir.join(nested);
        if child.is_dir() {
            walk(root, &child, harness, origin, 0, out);
        }
    }
}

fn walk(root: &Path, dir: &Path, harness: &str, origin: Origin, depth: usize, out: &mut Found) {
    if depth > NESTED_DEPTH || !dir.is_dir() {
        return;
    }
    collect(root, dir, harness, origin, out);
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let mut children: Vec<PathBuf> = entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.is_dir())
        .filter(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .is_none_or(|n| !crate::walk::is_noise_dir(n))
        })
        .collect();
    children.sort();
    for child in children {
        walk(root, &child, harness, origin, depth + 1, out);
    }
}

/// Record every instruction file directly inside `dir`.
fn collect(root: &Path, dir: &Path, harness: &str, origin: Origin, out: &mut Found) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let mut files: Vec<PathBuf> = entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.is_file())
        .filter(|p| is_instruction(p))
        .collect();
    files.sort();
    for path in files {
        out.insert(Instruction {
            root: root.to_path_buf(),
            harness_dir: harness.to_string(),
            doc_type: DocType::of(&path),
            origin,
            aliases: Vec::new(),
            path,
        });
    }
}

/// Whether `path` is an instruction file.
///
/// Two ways to qualify: a name from [`INSTRUCTION_NAMES`], or a classification
/// [`DocType`] already recognizes as an agent artifact that is not a skill (a
/// `SKILL.md` belongs to the skill list, not here).
fn is_instruction(path: &Path) -> bool {
    let name = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or_default();
    if INSTRUCTION_NAMES
        .iter()
        .any(|known| known.eq_ignore_ascii_case(name))
    {
        return true;
    }
    let doc_type = DocType::of(path);
    doc_type.is_agent_artifact() && doc_type != DocType::Skill
}

/// Accumulator that collapses paths reaching the same file.
#[derive(Default)]
struct Found {
    instructions: Vec<Instruction>,
    /// Canonical path of each accepted file, parallel to `instructions`.
    keys: Vec<PathBuf>,
}

impl Found {
    /// Record `instruction`, or fold it into an existing entry as an alias.
    ///
    /// Keyed on the canonical path, falling back to the literal one: junctions
    /// and symlinks collapse, and a file behind a broken link still appears
    /// once rather than vanishing.
    fn insert(&mut self, instruction: Instruction) {
        let key =
            std::fs::canonicalize(&instruction.path).unwrap_or_else(|_| instruction.path.clone());
        if let Some(ix) = self.keys.iter().position(|k| k == &key) {
            let existing = &mut self.instructions[ix];
            if existing.path != instruction.path && !existing.aliases.contains(&instruction.path) {
                existing.aliases.push(instruction.path);
            }
            return;
        }
        self.keys.push(key);
        self.instructions.push(instruction);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(path: &Path, body: &str) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, body).unwrap();
    }

    #[test]
    fn the_workspace_root_is_searched_first() {
        // `AGENTS.md` and `CLAUDE.md` live at the top of a repository, so a
        // root list that did not include the workspace itself would miss the
        // two most common files entirely.
        assert_eq!(project_roots().first(), Some(&PathBuf::new()));
    }

    #[test]
    fn roots_are_derived_from_the_harness_table() {
        let roots = project_roots();
        for expected in [".claude", ".cursor", ".github", ".agents"] {
            assert!(
                roots.contains(&PathBuf::from(expected)),
                "{expected} missing from {roots:?}"
            );
        }
    }

    #[test]
    fn roots_are_deduplicated() {
        // A dozen harnesses share `.agents/skills`, so the naive derivation
        // would produce `.agents` a dozen times.
        let roots = project_roots();
        let mut seen = roots.clone();
        seen.sort();
        seen.dedup();
        assert_eq!(seen.len(), roots.len(), "duplicate roots in {roots:?}");
    }

    #[test]
    fn a_noise_directory_is_not_searched_for_instructions() {
        // Instruction discovery had no skip-directory coverage at all — not one
        // name — even though it walks nested directories under every harness
        // root. A `node_modules` inside `.claude/` holding a vendored
        // `CLAUDE.md` would be listed as the user's own instruction file, and a
        // `target/` there is where a repository keeps its megabytes.
        //
        // The whole shared list, so a name dropped from `walk::SKIP_DIRS` is
        // caught here rather than only wherever it happened to be tested.
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        write(
            &root.join(".claude/AGENTS.md"),
            "# real
",
        );
        for noise in crate::walk::SKIP_DIRS {
            write(
                &root.join(".claude").join(noise).join("AGENTS.md"),
                "# noise
",
            );
        }

        let found = discover(root);
        assert_eq!(
            found.len(),
            1,
            "only the real one: {:?}",
            found.iter().map(|i| &i.path).collect::<Vec<_>>()
        );
        assert!(found[0].path.ends_with("AGENTS.md"));
        assert!(
            found[0]
                .path
                .parent()
                .is_some_and(|p| p.ends_with(".claude")),
            "the one under .claude itself, not one nested inside noise: {:?}",
            found[0].path
        );
    }

    #[test]
    fn finds_the_common_instruction_files() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        write(&root.join("AGENTS.md"), "# agents\n");
        write(&root.join("CLAUDE.md"), "# claude\n");
        write(&root.join(".claude").join("CLAUDE.md"), "# scoped\n");
        // Not an instruction file: ordinary documentation.
        write(&root.join("README.md"), "# readme\n");

        let found = discover(root);
        let labels: Vec<String> = found.iter().map(|i| i.label()).collect();
        assert!(labels.contains(&"AGENTS.md".to_string()), "{labels:?}");
        assert!(labels.contains(&"CLAUDE.md".to_string()), "{labels:?}");
        assert!(
            labels.contains(&".claude/CLAUDE.md".to_string()),
            "{labels:?}"
        );
        assert!(
            !labels.iter().any(|l| l.contains("README")),
            "plain docs must not be listed: {labels:?}"
        );
    }

    #[test]
    fn a_skill_entry_is_not_an_instruction_file() {
        // SKILL.md is an agent artifact, but it belongs to the skill list —
        // listing it in both places would double-count every installed skill.
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        write(&root.join("SKILL.md"), "---\nname: x\n---\n");
        assert!(discover(root).is_empty(), "SKILL.md must be left to skills");
    }

    #[test]
    fn nested_rule_directories_are_searched() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        write(&root.join(".cursor").join("rules").join("style.md"), "x\n");
        write(&root.join(".github").join("a.instructions.md"), "scoped\n");

        let found = discover(root);
        let names: Vec<String> = found
            .iter()
            .map(|i| i.path.file_name().unwrap().to_string_lossy().to_string())
            .collect();
        assert!(names.contains(&"style.md".to_string()), "{names:?}");
        assert!(
            names.contains(&"a.instructions.md".to_string()),
            "{names:?}"
        );
    }

    #[test]
    fn each_file_carries_its_document_type() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        write(&root.join("AGENTS.md"), "x\n");
        write(&root.join("CLAUDE.md"), "x\n");

        let found = discover(root);
        let types: Vec<DocType> = found.iter().map(|i| i.doc_type).collect();
        assert!(types.contains(&DocType::Agents), "{types:?}");
        assert!(types.contains(&DocType::Claude), "{types:?}");
    }

    #[test]
    fn the_same_file_reached_twice_appears_once() {
        // `.agents` is derived from a dozen harnesses' skills roots, so without
        // deduplication one `.agents/AGENTS.md` would be listed a dozen times.
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        write(&root.join(".agents").join("AGENTS.md"), "x\n");

        let found = discover(root);
        let matching = found
            .iter()
            .filter(|i| i.path.ends_with("AGENTS.md"))
            .count();
        assert_eq!(matching, 1, "got {found:#?}");
    }

    #[test]
    fn discovery_is_hermetic_by_default() {
        // A test run must never depend on what the developer has in ~/.claude.
        let dir = tempfile::tempdir().unwrap();
        assert!(
            discover(dir.path())
                .iter()
                .all(|i| i.origin == Origin::Workspace)
        );
    }

    #[test]
    fn an_empty_workspace_yields_nothing_rather_than_failing() {
        let dir = tempfile::tempdir().unwrap();
        assert!(discover(dir.path()).is_empty());
    }

    #[test]
    fn labels_disambiguate_files_sharing_a_name() {
        // Two `CLAUDE.md` files, one at the root and one under `.claude`: a
        // list showing "CLAUDE.md" twice tells the reader nothing.
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        write(&root.join("CLAUDE.md"), "x\n");
        write(&root.join(".claude").join("CLAUDE.md"), "y\n");

        let labels: Vec<String> = discover(root).iter().map(|i| i.label()).collect();
        let unique: std::collections::HashSet<&String> = labels.iter().collect();
        assert_eq!(unique.len(), labels.len(), "ambiguous labels: {labels:?}");
    }
}
