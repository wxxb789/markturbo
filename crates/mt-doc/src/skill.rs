//! Agent Skills: model, parsing, validation, and workspace discovery.
//!
//! Implements the vendor-neutral spec at <https://agentskills.io/specification>
//! (required `name`/`description`; optional `license`, `allowed-tools`,
//! `metadata`, `compatibility`).
//!
//! Two deliberate departures from a strict spec reading, both to avoid
//! rejecting real skills in the wild:
//!
//! * `name` is optional and defaults to the directory name — that is what
//!   Claude Code does, and a large share of published skills rely on it. A
//!   mismatch is reported as a conformance diagnostic, not a parse failure.
//! * Unknown frontmatter keys are a warning, not an error, because several
//!   vendors add their own (`model`, `paths`, `icon`, …) to otherwise valid
//!   skills.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use crate::diagnostic::{Diagnostic, Severity};
use crate::frontmatter;

/// Spec limits, quoted from <https://agentskills.io/specification>.
pub const NAME_MAX: usize = 64;
pub const DESCRIPTION_MAX: usize = 1024;
pub const COMPATIBILITY_MAX: usize = 500;

/// Fields the spec allows in `SKILL.md` frontmatter.
pub const ALLOWED_FIELDS: &[&str] = &[
    "name",
    "description",
    "license",
    "allowed-tools",
    "metadata",
    "compatibility",
];

/// Parsed `SKILL.md` frontmatter.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SkillMeta {
    pub name: Option<String>,
    pub description: Option<String>,
    pub license: Option<String>,
    /// Space-separated list, per the spec. Kept split for display.
    pub allowed_tools: Vec<String>,
    pub compatibility: Option<String>,
    /// String→string map. Non-string scalars are stringified, matching the
    /// reference implementation's `strictyaml` behavior.
    pub metadata: BTreeMap<String, String>,
    /// Keys outside the spec, preserved so the inspector can show them and
    /// nothing is silently dropped.
    pub extra: BTreeMap<String, String>,
}

/// A discovered skill: a directory whose entry document is `SKILL.md`.
#[derive(Debug, Clone)]
pub struct Skill {
    /// The skill directory.
    pub dir: PathBuf,
    /// Path to the entry document.
    pub entry: PathBuf,
    /// Discovery root this skill was found under, e.g. `.claude/skills`.
    pub root: PathBuf,
    /// Effective name: frontmatter `name`, else the directory name.
    pub name: String,
    pub meta: SkillMeta,
    /// Validation results. An `Error` here means the skill is non-conformant,
    /// not that it failed to load.
    pub diagnostics: Vec<Diagnostic>,
    /// Conventional supporting directories that actually exist.
    pub support_dirs: Vec<PathBuf>,
}

impl Skill {
    /// Whether validation found no errors.
    pub fn is_valid(&self) -> bool {
        !self
            .diagnostics
            .iter()
            .any(|d| d.severity == Severity::Error)
    }

    /// Short description for list rows.
    pub fn summary(&self) -> &str {
        self.meta.description.as_deref().unwrap_or("(no description)")
    }
}

/// Parse `SKILL.md` source into metadata plus validation diagnostics.
///
/// `dir_name` is the containing directory's name, needed for the spec's
/// name-must-match-directory rule and as the fallback name.
pub fn parse(source: &str, dir_name: &str) -> (SkillMeta, Vec<Diagnostic>) {
    let mut diags = Vec::new();
    let mut meta = SkillMeta::default();

    let (fm, _) = frontmatter::split(source);
    let Some(fm) = fm else {
        diags.push(Diagnostic::error(
            "skill",
            if frontmatter::has_unterminated_fence(source) {
                "SKILL.md frontmatter not properly closed with ---"
            } else {
                "SKILL.md must start with YAML frontmatter (---)"
            },
        ));
        return (meta, diags);
    };

    let value = match frontmatter::parse_yaml(&fm.raw) {
        Ok(value) => value,
        Err(diag) => {
            diags.push(diag);
            return (meta, diags);
        }
    };

    let Some(map) = value.as_mapping() else {
        diags.push(Diagnostic::error(
            "skill",
            "SKILL.md frontmatter must be a YAML mapping",
        ));
        return (meta, diags);
    };

    for (key, val) in map {
        let Some(key) = key.as_str() else {
            diags.push(Diagnostic::warning(
                "skill",
                "non-string frontmatter key ignored",
            ));
            continue;
        };
        match key {
            "name" => meta.name = scalar(val),
            "description" => meta.description = scalar(val),
            "license" => meta.license = scalar(val),
            "compatibility" => meta.compatibility = scalar(val),
            "allowed-tools" => {
                meta.allowed_tools = match val {
                    // The spec says space-separated string; accept a YAML list
                    // too since several published skills use one.
                    serde_yaml::Value::Sequence(items) => {
                        items.iter().filter_map(scalar).collect()
                    }
                    other => scalar(other)
                        .unwrap_or_default()
                        .split_whitespace()
                        .map(str::to_string)
                        .collect(),
                };
            }
            "metadata" => match val.as_mapping() {
                Some(m) => {
                    for (k, v) in m {
                        if let (Some(k), Some(v)) = (k.as_str(), scalar(v)) {
                            meta.metadata.insert(k.to_string(), v);
                        }
                    }
                }
                None => diags.push(Diagnostic::warning(
                    "skill",
                    "`metadata` must be a map from string keys to string values",
                )),
            },
            other => {
                if let Some(v) = scalar(val) {
                    meta.extra.insert(other.to_string(), v);
                } else {
                    meta.extra.insert(other.to_string(), "…".to_string());
                }
                diags.push(Diagnostic::warning(
                    "skill",
                    format!(
                        "unexpected field `{other}`; the spec allows only {}",
                        ALLOWED_FIELDS.join(", ")
                    ),
                ));
            }
        }
    }

    validate(&meta, dir_name, &mut diags);
    (meta, diags)
}

/// Apply the spec's validation rules to already-parsed metadata.
fn validate(meta: &SkillMeta, dir_name: &str, diags: &mut Vec<Diagnostic>) {
    match meta.name.as_deref().map(str::trim) {
        None => diags.push(Diagnostic::warning(
            "skill",
            format!("missing `name`; defaulting to directory name `{dir_name}`"),
        )),
        Some("") => diags.push(Diagnostic::error("skill", "`name` must not be empty")),
        Some(name) => {
            let chars = name.chars().count();
            if chars > NAME_MAX {
                diags.push(Diagnostic::error(
                    "skill",
                    format!("`name` must be 1-{NAME_MAX} characters (got {chars})"),
                ));
            }
            if name != name.to_lowercase() {
                diags.push(Diagnostic::error(
                    "skill",
                    "`name` must be lowercase",
                ));
            }
            // Unicode alphanumeric, matching the reference validator: `café-tools`
            // and `日本語-skill` are conformant.
            if let Some(bad) = name.chars().find(|c| !(c.is_alphanumeric() || *c == '-')) {
                diags.push(Diagnostic::error(
                    "skill",
                    format!("`name` contains invalid character `{bad}`"),
                ));
            }
            if name.starts_with('-') || name.ends_with('-') {
                diags.push(Diagnostic::error(
                    "skill",
                    "Skill name cannot start or end with a hyphen",
                ));
            }
            if name.contains("--") {
                diags.push(Diagnostic::error(
                    "skill",
                    "Skill name cannot contain consecutive hyphens",
                ));
            }
            if !nfkc_eq(name, dir_name) {
                diags.push(Diagnostic::warning(
                    "skill",
                    format!("Directory name '{dir_name}' must match skill name '{name}'"),
                ));
            }
        }
    }

    match meta.description.as_deref().map(str::trim) {
        None | Some("") => diags.push(Diagnostic::error(
            "skill",
            "`description` is required and must not be empty",
        )),
        Some(d) => {
            let chars = d.chars().count();
            if chars > DESCRIPTION_MAX {
                diags.push(Diagnostic::error(
                    "skill",
                    format!("`description` must be 1-{DESCRIPTION_MAX} characters (got {chars})"),
                ));
            }
        }
    }

    if let Some(compat) = meta.compatibility.as_deref() {
        let chars = compat.chars().count();
        if chars > COMPATIBILITY_MAX {
            diags.push(Diagnostic::error(
                "skill",
                format!("`compatibility` must be at most {COMPATIBILITY_MAX} characters (got {chars})"),
            ));
        }
    }
}

/// Compare two names the way the spec's reference validator does.
///
/// The reference implementation NFKC-normalizes both sides. Pulling in a full
/// Unicode normalization crate for one comparison is not worth it, so this
/// folds the cases that actually occur in skill names: ASCII-compatible
/// fullwidth forms and the common dash variants.
///
/// ponytail: approximate NFKC; swap in `unicode-normalization` if a real skill
/// name ever needs the full mapping.
fn nfkc_eq(a: &str, b: &str) -> bool {
    fn fold(s: &str) -> String {
        s.trim()
            .chars()
            .map(|c| match c {
                // Fullwidth ASCII block -> ASCII.
                '\u{ff01}'..='\u{ff5e}' => {
                    char::from_u32(c as u32 - 0xfee0).unwrap_or(c)
                }
                // Dash variants -> hyphen-minus.
                '\u{2010}'..='\u{2015}' | '\u{2212}' | '\u{fe58}' | '\u{fe63}' => '-',
                other => other,
            })
            .collect()
    }
    fold(a) == fold(b)
}

/// Render a YAML scalar as a string, matching `strictyaml`'s everything-is-a-string
/// behavior so `version: 1.0` and `version: "1.0"` agree.
fn scalar(value: &serde_yaml::Value) -> Option<String> {
    match value {
        serde_yaml::Value::String(s) => Some(s.clone()),
        serde_yaml::Value::Number(n) => Some(n.to_string()),
        serde_yaml::Value::Bool(b) => Some(b.to_string()),
        _ => None,
    }
}

/// Directory names, relative to the workspace root, that conventionally hold
/// skills. Ordered by how load-bearing each is in the ecosystem.
///
/// Adding a convention is a one-line change here — that is the extensibility
/// the goal asks for, without a plugin system.
pub const DISCOVERY_ROOTS: &[&str] = &[
    "skills",
    ".agents/skills",
    ".claude/skills",
    ".cursor/skills",
    ".codex/skills",
    ".github/skills",
    ".opencode/skills",
    ".gemini/skills",
];

/// Conventional supporting directories inside a skill.
const SUPPORT_DIRS: &[&str] = &["scripts", "references", "assets"];

/// How deep to descend under a discovery root when looking for `SKILL.md`.
///
/// Matches the Vercel walker's documented three levels, which allows category
/// folders (`root/category/skill/SKILL.md`) without scanning a whole repo.
const MAX_DEPTH: usize = 3;

/// Discover skills under `workspace`.
///
/// A directory containing `SKILL.md` is a leaf: nested skills below it are not
/// visited, matching the documented "shallower shadows nested" rule. Results
/// are sorted by name so the explorer is stable across runs.
pub fn discover(workspace: &Path) -> Vec<Skill> {
    let mut skills = Vec::new();
    for root_rel in DISCOVERY_ROOTS {
        let root = workspace.join(root_rel.replace('/', std::path::MAIN_SEPARATOR_STR));
        if !root.is_dir() {
            continue;
        }
        walk(&root, &root, 0, &mut skills);
    }
    skills.sort_by(|a, b| a.name.cmp(&b.name).then_with(|| a.dir.cmp(&b.dir)));
    skills
}

fn walk(root: &Path, dir: &Path, depth: usize, out: &mut Vec<Skill>) {
    if depth > MAX_DEPTH {
        return;
    }
    if let Some(skill) = load(root, dir) {
        // Leaf: do not descend into a skill's own subdirectories.
        out.push(skill);
        return;
    }
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let mut children: Vec<PathBuf> = entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.is_dir())
        .collect();
    children.sort();
    for child in children {
        walk(root, &child, depth + 1, out);
    }
}

/// Load the skill rooted at `dir`, if it has an entry document.
pub fn load(root: &Path, dir: &Path) -> Option<Skill> {
    let entry = entry_path(dir)?;
    let dir_name = dir.file_name()?.to_str()?.to_string();

    let (meta, mut diagnostics) = match std::fs::read_to_string(&entry) {
        Ok(source) => parse(&source, &dir_name),
        Err(err) => (
            SkillMeta::default(),
            vec![Diagnostic::error(
                "skill",
                format!("cannot read {}: {err}", entry.display()),
            )],
        ),
    };

    let name = meta
        .name
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or(&dir_name)
        .to_string();

    let support_dirs: Vec<PathBuf> = SUPPORT_DIRS
        .iter()
        .map(|d| dir.join(d))
        .filter(|p| p.is_dir())
        .collect();

    diagnostics.sort_by_key(|d| d.severity);

    Some(Skill {
        dir: dir.to_path_buf(),
        entry,
        root: root.to_path_buf(),
        name,
        meta,
        diagnostics,
        support_dirs,
    })
}

/// Find a skill's entry document. `SKILL.md` is canonical; `skill.md` is
/// accepted because the reference parser does and case-insensitive filesystems
/// make the distinction meaningless anyway.
fn entry_path(dir: &Path) -> Option<PathBuf> {
    for name in ["SKILL.md", "skill.md"] {
        let candidate = dir.join(name);
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn errors(diags: &[Diagnostic]) -> Vec<&str> {
        diags
            .iter()
            .filter(|d| d.severity == Severity::Error)
            .map(|d| d.message.as_str())
            .collect()
    }

    #[test]
    fn parses_a_valid_skill() {
        let src = "---\nname: pdf-tools\ndescription: Extract text from PDFs.\nlicense: MIT\nallowed-tools: Read Write Bash\ncompatibility: any agent\nmetadata:\n  version: \"1.0\"\n  internal: true\n---\n\n# PDF Tools\n";
        let (meta, diags) = parse(src, "pdf-tools");
        assert_eq!(errors(&diags), Vec::<&str>::new());
        assert_eq!(meta.name.as_deref(), Some("pdf-tools"));
        assert_eq!(meta.description.as_deref(), Some("Extract text from PDFs."));
        assert_eq!(meta.license.as_deref(), Some("MIT"));
        assert_eq!(meta.allowed_tools, vec!["Read", "Write", "Bash"]);
        assert_eq!(meta.metadata.get("version").map(String::as_str), Some("1.0"));
        // Non-string scalars are stringified, per strictyaml semantics.
        assert_eq!(meta.metadata.get("internal").map(String::as_str), Some("true"));
    }

    #[test]
    fn missing_description_is_an_error() {
        let (_, diags) = parse("---\nname: demo\n---\n\nbody\n", "demo");
        assert!(errors(&diags).iter().any(|m| m.contains("`description`")));
    }

    #[test]
    fn missing_name_warns_and_falls_back() {
        let (meta, diags) = parse("---\ndescription: A demo.\n---\n", "my-skill");
        assert!(meta.name.is_none());
        assert_eq!(errors(&diags), Vec::<&str>::new(), "must not be fatal");
        assert!(diags.iter().any(|d| d.message.contains("my-skill")));
    }

    #[test]
    fn no_frontmatter_is_an_error() {
        let (_, diags) = parse("# Just a heading\n", "demo");
        assert!(errors(&diags)[0].contains("must start with YAML frontmatter"));
    }

    #[test]
    fn unclosed_frontmatter_says_so() {
        let (_, diags) = parse("---\nname: demo\n# body\n", "demo");
        assert!(errors(&diags)[0].contains("not properly closed"));
    }

    #[test]
    fn malformed_yaml_is_an_error_not_a_panic() {
        let (_, diags) = parse("---\nname: [unclosed\n---\n", "demo");
        assert_eq!(diags.len(), 1);
        assert_eq!(diags[0].severity, Severity::Error);
    }

    #[test]
    fn non_mapping_frontmatter_is_rejected() {
        let (_, diags) = parse("---\n- a\n- b\n---\n", "demo");
        assert!(errors(&diags)[0].contains("must be a YAML mapping"));
    }

    #[test]
    fn name_rules() {
        let bad = |name: &str, dir: &str| {
            let src = format!("---\nname: {name}\ndescription: d\n---\n");
            let (_, d) = parse(&src, dir);
            errors(&d).iter().map(|s| s.to_string()).collect::<Vec<_>>()
        };
        assert!(bad("-lead", "-lead")[0].contains("start or end with a hyphen"));
        assert!(bad("trail-", "trail-")[0].contains("start or end with a hyphen"));
        assert!(bad("a--b", "a--b")[0].contains("consecutive hyphens"));
        assert!(bad("Upper", "Upper")[0].contains("lowercase"));
        assert!(bad("has space", "has space")[0].contains("invalid character"));
        assert!(bad(&"x".repeat(65), "x")[0].contains("1-64"));
    }

    #[test]
    fn unicode_names_are_valid() {
        // The reference validator uses Unicode `isalnum`, not `[a-z0-9-]`.
        let (_, diags) = parse("---\nname: 日本語-skill\ndescription: d\n---\n", "日本語-skill");
        assert_eq!(errors(&diags), Vec::<&str>::new());
    }

    #[test]
    fn directory_mismatch_is_a_warning_not_an_error() {
        let (_, diags) = parse("---\nname: alpha\ndescription: d\n---\n", "beta");
        assert_eq!(errors(&diags), Vec::<&str>::new());
        assert!(diags.iter().any(|d| d.message.contains("must match")));
    }

    #[test]
    fn fullwidth_dir_name_matches_after_folding() {
        let (_, diags) = parse("---\nname: demo\ndescription: d\n---\n", "ｄｅｍｏ");
        assert!(!diags.iter().any(|d| d.message.contains("must match")));
    }

    #[test]
    fn unknown_fields_warn_but_do_not_fail() {
        let (meta, diags) = parse(
            "---\nname: demo\ndescription: d\nmodel: opus\n---\n",
            "demo",
        );
        assert_eq!(errors(&diags), Vec::<&str>::new());
        assert_eq!(meta.extra.get("model").map(String::as_str), Some("opus"));
        assert!(diags.iter().any(|d| d.message.contains("unexpected field")));
    }

    #[test]
    fn description_length_limit() {
        let long = "x".repeat(DESCRIPTION_MAX + 1);
        let src = format!("---\nname: demo\ndescription: {long}\n---\n");
        let (_, diags) = parse(&src, "demo");
        assert!(errors(&diags)[0].contains("1-1024"));
    }

    #[test]
    fn allowed_tools_accepts_a_yaml_list() {
        let (meta, _) = parse(
            "---\nname: demo\ndescription: d\nallowed-tools:\n  - Read\n  - Write\n---\n",
            "demo",
        );
        assert_eq!(meta.allowed_tools, vec!["Read", "Write"]);
    }
}
