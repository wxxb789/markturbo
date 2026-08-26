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

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

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
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Skill {
    /// The skill directory.
    pub dir: PathBuf,
    /// Path to the entry document.
    pub entry: PathBuf,
    /// Discovery root this skill was found under, e.g. `.claude/skills`.
    pub root: PathBuf,
    /// Whether this came from the workspace or from a global harness directory.
    pub origin: Origin,
    /// Other paths reaching the same skill directory, typically because a
    /// harness directory is a symlink or junction into a canonical one.
    pub aliases: Vec<PathBuf>,
    /// Effective name: frontmatter `name`, else the directory name.
    pub name: String,
    pub meta: SkillMeta,
    /// Validation results. An `Error` here means the skill is non-conformant,
    /// not that it failed to load.
    pub diagnostics: Vec<Diagnostic>,
    /// Conventional supporting directories that actually exist.
    pub support_dirs: Vec<PathBuf>,
}

/// Where a skill was found.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Origin {
    /// Under the open workspace — the skills this project ships.
    Workspace,
    /// Under a harness's global directory, so available in every project.
    Global,
}

impl Origin {
    pub fn label(self) -> &'static str {
        match self {
            Origin::Workspace => "workspace",
            Origin::Global => "global",
        }
    }
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
        self.meta
            .description
            .as_deref()
            .unwrap_or("(no description)")
    }

    /// Whether the skill marks itself as internal.
    ///
    /// The spec's reference tooling hides these from ordinary discovery; see
    /// [`Discovery::include_internal`].
    pub fn is_internal(&self) -> bool {
        self.meta
            .metadata
            .get("internal")
            .is_some_and(|v| v.eq_ignore_ascii_case("true"))
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

    // Where each top-level key sits in the file, so a diagnostic can point at
    // the line the user has to edit rather than just naming the field.
    let lines = KeyLines::of(source, &fm);

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
                    serde_yaml::Value::Sequence(items) => items.iter().filter_map(scalar).collect(),
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
                None => diags.push(lines.attach(
                    Diagnostic::warning(
                        "skill",
                        "`metadata` must be a map from string keys to string values",
                    ),
                    "metadata",
                )),
            },
            other => {
                if let Some(v) = scalar(val) {
                    meta.extra.insert(other.to_string(), v);
                } else {
                    meta.extra.insert(other.to_string(), "…".to_string());
                }
                diags.push(lines.attach(
                    Diagnostic::warning(
                        "skill",
                        format!(
                            "unexpected field `{other}`; the spec allows only {}",
                            ALLOWED_FIELDS.join(", ")
                        ),
                    ),
                    other,
                ));
            }
        }
    }

    validate(&meta, dir_name, &lines, &mut diags);
    (meta, diags)
}

/// The 1-based line of each top-level frontmatter key.
///
/// Scanned from the raw text rather than taken from the YAML parse:
/// `serde_yaml` discards spans by the time a `Mapping` is handed back, and a
/// validation message that cannot say *where* sends the user hunting through a
/// file to find a field this code already read.
#[derive(Debug, Default)]
struct KeyLines {
    lines: BTreeMap<String, usize>,
    /// Line of the opening `---`, used when a rule is about the block as a
    /// whole (a missing required field has no line of its own).
    fence: usize,
}

impl KeyLines {
    fn of(source: &str, fm: &frontmatter::Frontmatter) -> Self {
        // The fence is the line before the YAML body starts.
        let fence = line_of(source, fm.body_start).saturating_sub(1).max(1);
        let mut lines = BTreeMap::new();
        for (offset, raw) in fm.raw.split_inclusive('\n').enumerate() {
            let text = raw.trim_end_matches(['\n', '\r']);
            // Top-level keys only: an indented line belongs to the key above it,
            // and `metadata:`'s children must not shadow their parent.
            if !text.starts_with([' ', '\t', '#'])
                && let Some((key, _)) = text.split_once(':')
                && !key.is_empty()
            {
                lines
                    .entry(key.trim().to_string())
                    .or_insert(fence + 1 + offset);
            }
        }
        Self { lines, fence }
    }

    /// Anchor `diag` to `key`'s line, or to the frontmatter fence when the key
    /// is absent — which is exactly the case for a missing required field.
    fn attach(&self, diag: Diagnostic, key: &str) -> Diagnostic {
        diag.at_line(self.lines.get(key).copied().unwrap_or(self.fence))
    }
}

/// The 1-based line containing `offset`.
fn line_of(source: &str, offset: usize) -> usize {
    source[..offset.min(source.len())].matches('\n').count() + 1
}

/// Apply the spec's validation rules to already-parsed metadata.
fn validate(meta: &SkillMeta, dir_name: &str, lines: &KeyLines, diags: &mut Vec<Diagnostic>) {
    let at_name = |d: Diagnostic| lines.attach(d, "name");

    match meta.name.as_deref().map(str::trim) {
        None => diags.push(at_name(Diagnostic::warning(
            "skill",
            format!("missing `name`; defaulting to directory name `{dir_name}`"),
        ))),
        Some("") => diags.push(at_name(Diagnostic::error(
            "skill",
            "`name` must not be empty",
        ))),
        Some(name) => {
            let chars = name.chars().count();
            if chars > NAME_MAX {
                diags.push(at_name(Diagnostic::error(
                    "skill",
                    format!("`name` must be 1-{NAME_MAX} characters (got {chars})"),
                )));
            }
            if name != name.to_lowercase() {
                diags.push(at_name(Diagnostic::error(
                    "skill",
                    "`name` must be lowercase",
                )));
            }
            // Unicode alphanumeric, matching the reference validator: `café-tools`
            // and `日本語-skill` are conformant.
            if let Some(bad) = name.chars().find(|c| !(c.is_alphanumeric() || *c == '-')) {
                // Naming the character alone leaves the reader to guess what the
                // rule is; underscores and spaces in particular look perfectly
                // reasonable until you know only hyphens separate words here.
                diags.push(at_name(Diagnostic::error(
                    "skill",
                    format!(
                        "`name` may contain only letters, digits and hyphens — \
                         found `{bad}` in `{name}`. Rename it to `{}`.",
                        suggest_name(name)
                    ),
                )));
            }
            if name.starts_with('-') || name.ends_with('-') {
                diags.push(at_name(Diagnostic::error(
                    "skill",
                    "Skill name cannot start or end with a hyphen",
                )));
            }
            if name.contains("--") {
                diags.push(at_name(Diagnostic::error(
                    "skill",
                    "Skill name cannot contain consecutive hyphens",
                )));
            }
            if !nfkc_eq(name, dir_name) {
                diags.push(at_name(Diagnostic::warning(
                    "skill",
                    format!("Directory name '{dir_name}' must match skill name '{name}'"),
                )));
            }
        }
    }

    match meta.description.as_deref().map(str::trim) {
        None | Some("") => diags.push(lines.attach(
            Diagnostic::error("skill", "`description` is required and must not be empty"),
            "description",
        )),
        Some(d) => {
            let chars = d.chars().count();
            if chars > DESCRIPTION_MAX {
                diags.push(lines.attach(
                    Diagnostic::error(
                        "skill",
                        format!(
                            "`description` must be 1-{DESCRIPTION_MAX} characters (got {chars})"
                        ),
                    ),
                    "description",
                ));
            }
        }
    }

    if let Some(compat) = meta.compatibility.as_deref() {
        let chars = compat.chars().count();
        if chars > COMPATIBILITY_MAX {
            diags.push(lines.attach(
                Diagnostic::error(
                    "skill",
                    format!("`compatibility` must be at most {COMPATIBILITY_MAX} characters (got {chars})"),
                ),
                "compatibility",
            ));
        }
    }
}

/// A conformant name derived from a rejected one, for the diagnostic to offer.
///
/// Every disallowed character becomes a hyphen, which is what the offenders in
/// the wild — `_leading_underscore`, `My Skill`, `foo.bar` — actually mean.
fn suggest_name(name: &str) -> String {
    let mapped: String = name
        .to_lowercase()
        .chars()
        .map(|c| if c.is_alphanumeric() { c } else { '-' })
        .collect();
    // The same rules the name itself must satisfy: no leading, trailing or
    // consecutive hyphens. Suggesting something still invalid would be worse
    // than suggesting nothing.
    let mut out = String::with_capacity(mapped.len());
    for c in mapped.chars() {
        if c == '-' && (out.is_empty() || out.ends_with('-')) {
            continue;
        }
        out.push(c);
    }
    while out.ends_with('-') {
        out.pop();
    }
    out
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
                '\u{ff01}'..='\u{ff5e}' => char::from_u32(c as u32 - 0xfee0).unwrap_or(c),
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
/// skills.
///
/// Derived from the harness table in [`crate::harness`], so supporting a new
/// harness is one line there and nothing else in the app changes.
pub fn discovery_roots() -> Vec<&'static str> {
    crate::harness::project_roots()
}

/// Conventional supporting directories inside a skill.
const SUPPORT_DIRS: &[&str] = &["scripts", "references", "assets"];

/// How deep to descend under a discovery root when looking for `SKILL.md`.
///
/// Matches the Vercel walker's documented three levels, which allows category
/// folders (`root/category/skill/SKILL.md`) without scanning a whole repo.
const MAX_DEPTH: usize = 3;

/// What to search.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Discovery {
    /// Search the harness global directories (`~/.claude/skills`, …) as well as
    /// the workspace.
    pub global: bool,
    /// Include skills marked `metadata.internal: true`.
    ///
    /// The reference tooling hides these unless `INSTALL_INTERNAL_SKILLS=1`;
    /// [`Discovery::from_env`] honors the same variable.
    pub include_internal: bool,
}

impl Discovery {
    /// Workspace only — the conservative default, and what tests want.
    pub const WORKSPACE: Self = Self {
        global: false,
        include_internal: false,
    };

    /// Everything a harness on this machine could see.
    pub fn everything() -> Self {
        Self {
            global: true,
            ..Self::from_env()
        }
    }

    /// Defaults, with `INSTALL_INTERNAL_SKILLS` applied.
    pub fn from_env() -> Self {
        let include_internal = std::env::var("INSTALL_INTERNAL_SKILLS")
            .is_ok_and(|v| matches!(v.trim(), "1" | "true"));
        Self {
            global: false,
            include_internal,
        }
    }
}

/// Parsed global discovery results that survive ordinary refreshes.
///
/// Workspace roots are deliberately not cached: their watcher events describe
/// the project the user is actively editing, and their scan is already cheap.
/// Global roots are the expensive part, so each one is reused only while every
/// directory visited by the previous walk and every discovered entry document
/// still has the same filesystem stamp.
#[derive(Debug, Default)]
pub struct DiscoveryCache {
    global_roots: BTreeMap<PathBuf, CachedRoot>,
    #[cfg(test)]
    global_scans: usize,
}

impl DiscoveryCache {
    /// Force the next discovery to rescan every global root.
    pub fn clear(&mut self) {
        self.global_roots.clear();
    }
}

#[derive(Debug, Clone)]
struct CachedRoot {
    fingerprint: RootFingerprint,
    skills: Vec<KeyedSkill>,
}

#[derive(Debug, Clone)]
struct KeyedSkill {
    canonical_dir: PathBuf,
    skill: Skill,
}

#[derive(Debug, Clone)]
enum RootFingerprint {
    Missing,
    Present {
        canonical_root: PathBuf,
        directories: Vec<PathStamp>,
        entries: Vec<PathStamp>,
    },
}

impl RootFingerprint {
    fn matches(&self, root: &Path) -> bool {
        match self {
            Self::Missing => match std::fs::metadata(root) {
                Ok(metadata) => !metadata.is_dir(),
                Err(err) => err.kind() == std::io::ErrorKind::NotFound,
            },
            Self::Present {
                canonical_root,
                directories,
                entries,
            } => {
                std::fs::canonicalize(root).ok().as_ref() == Some(canonical_root)
                    && directories.iter().all(PathStamp::matches_directory)
                    && entries.iter().all(PathStamp::matches_file)
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct PathStamp {
    path: PathBuf,
    modified: SystemTime,
    len: Option<u64>,
}

impl PathStamp {
    fn directory(path: &Path) -> Option<Self> {
        let metadata = std::fs::metadata(path).ok()?;
        if !metadata.is_dir() {
            return None;
        }
        Some(Self {
            path: path.to_path_buf(),
            modified: metadata.modified().ok()?,
            len: None,
        })
    }

    fn file(path: &Path) -> Option<Self> {
        let metadata = std::fs::metadata(path).ok()?;
        if !metadata.is_file() {
            return None;
        }
        Some(Self {
            path: path.to_path_buf(),
            modified: metadata.modified().ok()?,
            len: Some(metadata.len()),
        })
    }

    fn matches_directory(&self) -> bool {
        Self::directory(&self.path).as_ref() == Some(self)
    }

    fn matches_file(&self) -> bool {
        Self::file(&self.path).as_ref() == Some(self)
    }
}

struct FingerprintBuilder {
    canonical_root: Option<PathBuf>,
    directories: Vec<PathStamp>,
    entries: Vec<PathStamp>,
    cacheable: bool,
}

impl FingerprintBuilder {
    fn new(root: &Path) -> Self {
        Self {
            canonical_root: std::fs::canonicalize(root).ok(),
            directories: Vec::new(),
            entries: Vec::new(),
            cacheable: true,
        }
    }

    fn record_directory(&mut self, path: &Path) -> bool {
        match PathStamp::directory(path) {
            Some(stamp) => {
                self.directories.push(stamp);
                true
            }
            None => {
                self.cacheable = false;
                false
            }
        }
    }

    fn invalidate(&mut self) {
        self.cacheable = false;
    }

    fn record_entry(&mut self, before: Option<PathStamp>, path: &Path, read_ok: bool) {
        let after = PathStamp::file(path);
        match (read_ok, before, after) {
            (true, Some(before), Some(after)) if before == after => self.entries.push(after),
            _ => self.cacheable = false,
        }
    }

    fn finish(self) -> Option<RootFingerprint> {
        if !self.cacheable {
            return None;
        }
        Some(RootFingerprint::Present {
            canonical_root: self.canonical_root?,
            directories: self.directories,
            entries: self.entries,
        })
    }
}

/// Discover skills under `workspace`.
///
/// Searches the workspace's own conventional directories only. Use
/// [`discover_with`] to also search the harness global directories — this
/// signature deliberately stays hermetic so a test or a headless tool cannot
/// accidentally pick up whatever the developer has installed.
pub fn discover(workspace: &Path) -> Vec<Skill> {
    discover_with(workspace, Discovery::WORKSPACE)
}

/// Discover skills, controlling how far to look.
///
/// A directory containing `SKILL.md` is a leaf: nested skills below it are not
/// visited, matching the documented "shallower shadows nested" rule. Results
/// are sorted by origin then name so the explorer is stable across runs.
pub fn discover_with(workspace: &Path, options: Discovery) -> Vec<Skill> {
    let workspace_roots: Vec<PathBuf> = crate::harness::project_roots()
        .into_iter()
        .map(|root| workspace.join(rel_path(root)))
        .collect();
    let global_roots = options
        .global
        .then(crate::harness::global_roots)
        .unwrap_or_default();

    discover_uncached(&workspace_roots, &global_roots, options.include_internal)
}

/// Discover skills while reusing unchanged global roots.
pub fn discover_with_cache(
    workspace: &Path,
    options: Discovery,
    cache: &mut DiscoveryCache,
) -> Vec<Skill> {
    let workspace_roots: Vec<PathBuf> = crate::harness::project_roots()
        .into_iter()
        .map(|root| workspace.join(rel_path(root)))
        .collect();
    let global_roots = options
        .global
        .then(crate::harness::global_roots)
        .unwrap_or_default();

    discover_from_roots(
        &workspace_roots,
        &global_roots,
        options.include_internal,
        cache,
    )
}

fn discover_uncached(
    workspace_roots: &[PathBuf],
    global_roots: &[PathBuf],
    include_internal: bool,
) -> Vec<Skill> {
    let mut found = Found::default();

    for root in workspace_roots {
        walk(root, root, Origin::Workspace, 0, &mut found, None);
    }

    for root in global_roots {
        walk(root, root, Origin::Global, 0, &mut found, None);
    }

    finish_discovery(found, include_internal)
}

fn discover_from_roots(
    workspace_roots: &[PathBuf],
    global_roots: &[PathBuf],
    include_internal: bool,
    cache: &mut DiscoveryCache,
) -> Vec<Skill> {
    // Turning global discovery off is a view choice, not an invalidation. Keep
    // the snapshots so turning it back on does not cold-scan every harness.
    if !global_roots.is_empty() {
        cache
            .global_roots
            .retain(|root, _| global_roots.contains(root));
    }

    let mut found = Found::default();
    for root in workspace_roots {
        walk(root, root, Origin::Workspace, 0, &mut found, None);
    }

    for root in global_roots {
        for keyed in cached_root(root, cache) {
            found.insert_keyed(keyed.canonical_dir, keyed.skill);
        }
    }

    finish_discovery(found, include_internal)
}

fn cached_root(root: &Path, cache: &mut DiscoveryCache) -> Vec<KeyedSkill> {
    if let Some(cached) = cache.global_roots.get(root)
        && cached.fingerprint.matches(root)
    {
        return cached.skills.clone();
    }

    #[cfg(test)]
    {
        cache.global_scans += 1;
    }

    let (skills, fingerprint) = scan_root(root);
    publish_root_scan(root, cache, skills, fingerprint)
}

fn publish_root_scan(
    root: &Path,
    cache: &mut DiscoveryCache,
    skills: Vec<KeyedSkill>,
    fingerprint: Option<RootFingerprint>,
) -> Vec<KeyedSkill> {
    match fingerprint {
        Some(fingerprint) => {
            cache.global_roots.insert(
                root.to_path_buf(),
                CachedRoot {
                    fingerprint,
                    skills: skills.clone(),
                },
            );
        }
        None => {
            return cache
                .global_roots
                .get(root)
                .map(|cached| cached.skills.clone())
                .unwrap_or_default();
        }
    }
    skills
}

fn scan_root(root: &Path) -> (Vec<KeyedSkill>, Option<RootFingerprint>) {
    match std::fs::metadata(root) {
        Ok(metadata) if metadata.is_dir() => {}
        Ok(_) => return (Vec::new(), Some(RootFingerprint::Missing)),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            return (Vec::new(), Some(RootFingerprint::Missing));
        }
        Err(_) => return (Vec::new(), None),
    }

    let mut found = Found::default();
    let mut fingerprint = FingerprintBuilder::new(root);
    walk(
        root,
        root,
        Origin::Global,
        0,
        &mut found,
        Some(&mut fingerprint),
    );
    (found.into_keyed(), fingerprint.finish())
}

fn finish_discovery(found: Found, include_internal: bool) -> Vec<Skill> {
    let mut skills = found.skills;

    if !include_internal {
        skills.retain(|s| !s.is_internal());
    }
    skills.sort_by(|a, b| {
        a.origin
            .cmp(&b.origin)
            .then_with(|| a.name.cmp(&b.name))
            .then_with(|| a.dir.cmp(&b.dir))
    });
    skills
}

/// Convert a `/`-separated relative root into a platform path.
fn rel_path(path: &str) -> PathBuf {
    path.split('/').collect()
}

/// Accumulator that collapses paths reaching the same directory.
#[derive(Default)]
struct Found {
    skills: Vec<Skill>,
    /// Canonical directory of each accepted skill, parallel to `skills`.
    keys: Vec<PathBuf>,
    key_indices: HashMap<PathBuf, usize>,
}

impl Found {
    /// Record `skill`, or fold it into an existing entry as an alias.
    ///
    /// Keyed on the canonical path so a junction and its target collapse. The
    /// first arrival wins, which is why root order encodes preference.
    ///
    /// This is also what makes a symlink cycle harmless: a revisited directory
    /// produces an alias, never a second entry. Termination itself comes from
    /// [`MAX_DEPTH`] — a deliberate visited-set would suppress exactly the
    /// cross-root revisits that aliases are made of.
    fn insert(&mut self, skill: Skill) {
        let key = canonical(&skill.dir);
        self.insert_keyed(key, skill);
    }

    fn insert_keyed(&mut self, key: PathBuf, skill: Skill) {
        if let Some(&ix) = self.key_indices.get(&key) {
            let existing = &mut self.skills[ix];
            for alias in std::iter::once(skill.dir).chain(skill.aliases) {
                if existing.dir != alias && !existing.aliases.contains(&alias) {
                    existing.aliases.push(alias);
                }
            }
            return;
        }
        self.key_indices.insert(key.clone(), self.skills.len());
        self.keys.push(key);
        self.skills.push(skill);
    }

    fn into_keyed(self) -> Vec<KeyedSkill> {
        self.keys
            .into_iter()
            .zip(self.skills)
            .map(|(canonical_dir, skill)| KeyedSkill {
                canonical_dir,
                skill,
            })
            .collect()
    }
}

/// A path's canonical form, for identity comparison only.
///
/// Falls back to the path as given: `canonicalize` fails on a broken link, and
/// a skill whose directory cannot be resolved should still be listed once
/// rather than vanish. The result is never displayed — on Windows it is a
/// `\\?\` UNC path.
fn canonical(path: &Path) -> PathBuf {
    std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

fn walk(
    root: &Path,
    dir: &Path,
    origin: Origin,
    depth: usize,
    out: &mut Found,
    mut fingerprint: Option<&mut FingerprintBuilder>,
) {
    if depth > MAX_DEPTH {
        return;
    }
    if let Some(fingerprint) = fingerprint.as_deref_mut() {
        if !fingerprint.record_directory(dir) {
            return;
        }
    } else if !dir.is_dir() {
        return;
    }
    if let Some(skill) = load_with_origin_recorded(root, dir, origin, fingerprint.as_deref_mut()) {
        // Leaf: do not descend into a skill's own subdirectories.
        out.insert(skill);
        return;
    }
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(_) => {
            if let Some(fingerprint) = fingerprint.as_deref_mut() {
                fingerprint.cacheable = false;
            }
            return;
        }
    };
    let mut children = Vec::new();
    for entry in entries {
        let entry = match entry {
            Ok(entry) => entry,
            Err(_) => {
                if let Some(fingerprint) = fingerprint.as_deref_mut() {
                    fingerprint.invalidate();
                }
                continue;
            }
        };
        let file_type = match entry.file_type() {
            Ok(file_type) => file_type,
            Err(_) => {
                if let Some(fingerprint) = fingerprint.as_deref_mut() {
                    fingerprint.invalidate();
                }
                continue;
            }
        };
        let path = entry.path();
        let is_dir = if file_type.is_dir() {
            true
        } else if file_type.is_symlink() {
            match std::fs::metadata(&path) {
                Ok(metadata) => metadata.is_dir(),
                Err(_) => {
                    if let Some(fingerprint) = fingerprint.as_deref_mut() {
                        fingerprint.invalidate();
                    }
                    false
                }
            }
        } else {
            false
        };
        if is_dir
            && path
                .file_name()
                .and_then(|n| n.to_str())
                .is_none_or(|n| !crate::walk::is_noise_dir(n))
        {
            children.push(path);
        }
    }
    children.sort();
    for child in children {
        walk(
            root,
            &child,
            origin,
            depth + 1,
            out,
            fingerprint.as_deref_mut(),
        );
    }
}

/// Load the skill rooted at `dir`, if it has an entry document.
pub fn load(root: &Path, dir: &Path) -> Option<Skill> {
    load_with_origin(root, dir, Origin::Workspace)
}

fn load_with_origin(root: &Path, dir: &Path, origin: Origin) -> Option<Skill> {
    load_with_origin_recorded(root, dir, origin, None)
}

fn load_with_origin_recorded(
    root: &Path,
    dir: &Path,
    origin: Origin,
    mut fingerprint: Option<&mut FingerprintBuilder>,
) -> Option<Skill> {
    let entry = match entry_path(dir) {
        Ok(Some(entry)) => entry,
        Ok(None) => return None,
        Err(_) => {
            if let Some(fingerprint) = fingerprint.as_deref_mut() {
                fingerprint.invalidate();
            }
            return None;
        }
    };
    let dir_name = dir.file_name()?.to_str()?.to_string();
    let before = fingerprint.as_ref().and_then(|_| PathStamp::file(&entry));

    let (meta, mut diagnostics, read_ok) = match std::fs::read_to_string(&entry) {
        Ok(source) => {
            let (meta, diagnostics) = parse(&source, &dir_name);
            (meta, diagnostics, true)
        }
        Err(err) => (
            SkillMeta::default(),
            vec![Diagnostic::error(
                "skill",
                format!("cannot read {}: {err}", entry.display()),
            )],
            false,
        ),
    };
    if let Some(fingerprint) = fingerprint.as_deref_mut() {
        fingerprint.record_entry(before, &entry, read_ok);
    }

    let name = meta
        .name
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or(&dir_name)
        .to_string();

    let mut support_dirs = Vec::new();
    for name in SUPPORT_DIRS {
        let path = dir.join(name);
        match std::fs::metadata(&path) {
            Ok(metadata) if metadata.is_dir() => support_dirs.push(path),
            Ok(_) => {}
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
            Err(_) => {
                if let Some(fingerprint) = fingerprint.as_deref_mut() {
                    fingerprint.invalidate();
                }
            }
        }
    }

    diagnostics.sort_by_key(|d| d.severity);

    Some(Skill {
        dir: dir.to_path_buf(),
        entry,
        root: root.to_path_buf(),
        origin,
        aliases: Vec::new(),
        name,
        meta,
        diagnostics,
        support_dirs,
    })
}

/// Find a skill's entry document. `SKILL.md` is canonical; `skill.md` is
/// accepted because the reference parser does and case-insensitive filesystems
/// make the distinction meaningless anyway.
fn entry_path(dir: &Path) -> std::io::Result<Option<PathBuf>> {
    for name in ["SKILL.md", "skill.md"] {
        let candidate = dir.join(name);
        match std::fs::metadata(&candidate) {
            Ok(metadata) if metadata.is_file() => return Ok(Some(candidate)),
            Ok(_) => {}
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
            Err(err) => return Err(err),
        }
    }
    Ok(None)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

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
        assert_eq!(
            meta.metadata.get("version").map(String::as_str),
            Some("1.0")
        );
        // Non-string scalars are stringified, per strictyaml semantics.
        assert_eq!(
            meta.metadata.get("internal").map(String::as_str),
            Some("true")
        );
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
        assert!(bad("has space", "has space")[0].contains("only letters, digits and hyphens"));
        assert!(bad(&"x".repeat(65), "x")[0].contains("1-64"));
    }

    /// The message the user saw for `_my-commit-push-pr` named the character but
    /// not the rule, so "why is it invalid?" was a fair question. It must state
    /// the rule and offer the fix.
    #[test]
    fn an_invalid_character_explains_the_rule_and_suggests_a_name() {
        let src = "---\nname: _my-commit-push-pr\ndescription: d\n---\n";
        let (_, diags) = parse(src, "_my-commit-push-pr");
        let message = errors(&diags)
            .into_iter()
            .find(|m| m.contains("letters, digits and hyphens"))
            .expect("the name rule must be stated");
        assert!(message.contains('_'), "must name the offending character");
        assert!(
            message.contains("`my-commit-push-pr`"),
            "must suggest a conformant name, got {message}"
        );
    }

    #[test]
    fn suggested_names_are_themselves_valid() {
        for input in [
            "_my-commit-push-pr",
            "My Skill",
            "foo.bar",
            "__a__b__",
            "a_",
        ] {
            let suggested = suggest_name(input);
            assert!(!suggested.is_empty(), "{input} produced nothing");
            assert!(
                !suggested.starts_with('-') && !suggested.ends_with('-'),
                "{input} -> {suggested} has an edge hyphen"
            );
            assert!(!suggested.contains("--"), "{input} -> {suggested}");
            assert!(
                suggested.chars().all(|c| c.is_alphanumeric() || c == '-'),
                "{input} -> {suggested}"
            );
            assert_eq!(suggested, suggested.to_lowercase());
        }
    }

    #[test]
    fn diagnostics_point_at_the_offending_line() {
        let src = "---\nname: Bad_Name\nlicense: MIT\nmodel: opus\n---\n\n# body\n";
        let (_, diags) = parse(src, "Bad_Name");
        for diag in &diags {
            assert!(
                diag.line.is_some(),
                "every field diagnostic needs a line: {diag:?}"
            );
        }
        // `name:` is line 2 — the fence is line 1. Match on `name` rules
        // specifically: the unexpected-field message lists every allowed field,
        // so a bare `contains("name")` matches it too.
        let name_lines: Vec<_> = diags
            .iter()
            .filter(|d| d.message.starts_with("`name`"))
            .filter_map(|d| d.line)
            .collect();
        assert!(
            !name_lines.is_empty() && name_lines.iter().all(|l| *l == 2),
            "name rules must point at line 2, got {name_lines:?}"
        );
        // `model:` is line 4.
        let unexpected = diags
            .iter()
            .find(|d| d.message.contains("unexpected field"))
            .expect("model is not a spec field");
        assert_eq!(unexpected.line, Some(4));
    }

    #[test]
    fn a_missing_field_points_at_the_frontmatter_fence() {
        // A required field that is absent has no line of its own; the fence is
        // where the user has to add it.
        let (_, diags) = parse("---\nname: demo\n---\n", "demo");
        let missing = diags
            .iter()
            .find(|d| d.message.contains("`description`"))
            .expect("description is required");
        assert_eq!(missing.line, Some(1));
    }

    #[test]
    fn nested_metadata_keys_do_not_shadow_top_level_ones() {
        // `metadata:`'s children are indented; a naive scan would record
        // `name` at the nested line and point every name diagnostic there.
        let src = "---\nmetadata:\n  name: nested\ndescription: d\nname: Bad\n---\n";
        let (_, diags) = parse(src, "Bad");
        let name_diag = diags
            .iter()
            .find(|d| d.message.contains("lowercase"))
            .expect("Bad is not lowercase");
        assert_eq!(name_diag.line, Some(5), "the top-level `name:` is line 5");
    }

    #[test]
    fn unicode_names_are_valid() {
        // The reference validator uses Unicode `isalnum`, not `[a-z0-9-]`.
        let (_, diags) = parse(
            "---\nname: 日本語-skill\ndescription: d\n---\n",
            "日本語-skill",
        );
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

    fn write_skill(root: &Path, name: &str, description: &str) -> PathBuf {
        let dir = root.join(name);
        fs::create_dir_all(&dir).unwrap();
        fs::write(
            dir.join("SKILL.md"),
            format!("---\nname: {name}\ndescription: {description}\n---\n"),
        )
        .unwrap();
        dir
    }

    fn stub_skill(root: &Path, dir: &Path, aliases: Vec<PathBuf>) -> Skill {
        Skill {
            dir: dir.to_path_buf(),
            entry: dir.join("SKILL.md"),
            root: root.to_path_buf(),
            origin: Origin::Global,
            aliases,
            name: "alpha".to_string(),
            meta: SkillMeta::default(),
            diagnostics: Vec::new(),
            support_dirs: Vec::new(),
        }
    }

    #[test]
    fn cached_root_merge_preserves_uncached_alias_semantics() {
        let root_one = PathBuf::from("one");
        let root_two = PathBuf::from("two");
        let primary = root_one.join("alpha");
        let second = root_two.join("alpha");
        let second_alias_one = root_two.join("alias-one");
        let second_alias_two = root_two.join("alias-two");
        let key = PathBuf::from("canonical-alpha");

        let mut uncached = Found::default();
        for dir in [
            &primary,
            &second,
            &second_alias_one,
            &primary,
            &second_alias_one,
            &second_alias_two,
        ] {
            uncached.insert_keyed(key.clone(), stub_skill(&root_one, dir, Vec::new()));
        }

        let mut cached = Found::default();
        cached.insert_keyed(key.clone(), stub_skill(&root_one, &primary, Vec::new()));
        cached.insert_keyed(
            key,
            stub_skill(
                &root_two,
                &second,
                vec![second_alias_one, primary.clone(), second_alias_two],
            ),
        );

        assert_eq!(
            cached.skills[0].aliases, uncached.skills[0].aliases,
            "a cached root must contribute its primary path and every nested alias exactly once"
        );
    }

    #[test]
    fn cached_and_uncached_discovery_results_are_identical() {
        let temp = tempdir().unwrap();
        let global = temp.path().join("global");
        write_skill(&global, "alpha", "first");
        let detour = global.join("detour");
        fs::create_dir_all(&detour).unwrap();
        let roots = vec![global.clone(), detour.join("..")];

        let uncached = discover_uncached(&[], &roots, false);
        let mut cache = DiscoveryCache::default();
        let cold = discover_from_roots(&[], &roots, false, &mut cache);
        let warm = discover_from_roots(&[], &roots, false, &mut cache);

        assert_eq!(cold, uncached);
        assert_eq!(warm, uncached);
        assert_eq!(uncached.len(), 1);
        assert_eq!(uncached[0].aliases, vec![roots[1].join("alpha")]);
    }

    #[test]
    fn traversal_io_failure_is_never_cached() {
        let temp = tempdir().unwrap();
        let invalid_root = temp.path().join("invalid\0root");
        let mut cache = DiscoveryCache::default();

        let first =
            discover_from_roots(&[], std::slice::from_ref(&invalid_root), false, &mut cache);
        let second = discover_from_roots(&[], &[invalid_root], false, &mut cache);

        assert!(first.is_empty());
        assert!(second.is_empty());
        assert_eq!(
            cache.global_scans, 2,
            "an I/O failure must be retried instead of cached as a missing root"
        );
    }

    #[test]
    fn entry_probe_io_failure_makes_the_root_uncacheable() {
        let temp = tempdir().unwrap();
        let invalid_dir = temp.path().join("invalid\0entry");
        let mut fingerprint = FingerprintBuilder::new(temp.path());

        let loaded = load_with_origin_recorded(
            temp.path(),
            &invalid_dir,
            Origin::Global,
            Some(&mut fingerprint),
        );

        assert!(loaded.is_none());
        assert!(
            fingerprint.finish().is_none(),
            "an entry probe error must prevent the partial root scan from being cached"
        );
    }

    #[test]
    fn a_normally_missing_entry_keeps_an_empty_root_cacheable() {
        let temp = tempdir().unwrap();
        let global = temp.path().join("global");
        fs::create_dir_all(global.join("empty")).unwrap();
        let mut cache = DiscoveryCache::default();

        let first = discover_from_roots(&[], std::slice::from_ref(&global), false, &mut cache);
        let second = discover_from_roots(&[], &[global], false, &mut cache);

        assert!(first.is_empty());
        assert!(second.is_empty());
        assert_eq!(cache.global_scans, 1);
    }

    #[test]
    fn failed_rescan_keeps_the_last_successful_snapshot_and_retries() {
        let temp = tempdir().unwrap();
        let global = temp.path().join("global");
        fs::create_dir_all(&global).unwrap();
        let old_dir = global.join("old");
        let partial_dir = global.join("partial");
        let old = KeyedSkill {
            canonical_dir: old_dir.clone(),
            skill: stub_skill(&global, &old_dir, Vec::new()),
        };
        let partial = KeyedSkill {
            canonical_dir: partial_dir.clone(),
            skill: stub_skill(&global, &partial_dir, Vec::new()),
        };
        let mut cache = DiscoveryCache::default();
        cache.global_roots.insert(
            global.clone(),
            CachedRoot {
                fingerprint: RootFingerprint::Missing,
                skills: vec![old],
            },
        );

        let published = publish_root_scan(&global, &mut cache, vec![partial], None);

        assert_eq!(published.len(), 1);
        assert_eq!(published[0].skill.dir, old_dir);
        assert_eq!(cache.global_roots[&global].skills[0].skill.dir, old_dir);

        let retried = cached_root(&global, &mut cache);
        assert!(retried.is_empty());
        assert_eq!(cache.global_scans, 1);
    }

    #[test]
    fn failed_initial_scan_publishes_nothing_and_can_retry() {
        let temp = tempdir().unwrap();
        let global = temp.path().join("global");
        write_skill(&global, "alpha", "complete");
        let partial_dir = global.join("partial");
        let partial = KeyedSkill {
            canonical_dir: partial_dir.clone(),
            skill: stub_skill(&global, &partial_dir, Vec::new()),
        };
        let mut cache = DiscoveryCache::default();

        let published = publish_root_scan(&global, &mut cache, vec![partial], None);

        assert!(published.is_empty());
        assert!(!cache.global_roots.contains_key(&global));

        let retried = cached_root(&global, &mut cache);
        assert_eq!(retried.len(), 1);
        assert_eq!(retried[0].skill.name, "alpha");
        assert_eq!(cache.global_scans, 1);
    }

    #[test]
    fn unchanged_global_root_reuses_its_cached_scan() {
        let temp = tempdir().unwrap();
        let global = temp.path().join("global");
        write_skill(&global, "alpha", "first");
        let mut cache = DiscoveryCache::default();

        let first = discover_from_roots(&[], std::slice::from_ref(&global), false, &mut cache);
        let second = discover_from_roots(&[], &[global], false, &mut cache);

        assert_eq!(first.len(), 1);
        assert_eq!(second.len(), 1);
        assert_eq!(
            cache.global_scans, 1,
            "the warm refresh must not walk the root again"
        );
    }

    #[test]
    fn editing_a_skill_invalidates_the_cache_without_a_root_mtime_change() {
        let temp = tempdir().unwrap();
        let global = temp.path().join("global");
        let skill_dir = write_skill(&global, "alpha", "first");
        let root_mtime = fs::metadata(&global).unwrap().modified().unwrap();
        let mut cache = DiscoveryCache::default();

        let first = discover_from_roots(&[], std::slice::from_ref(&global), false, &mut cache);
        fs::write(
            skill_dir.join("SKILL.md"),
            "---\nname: alpha\ndescription: second and longer\n---\n",
        )
        .unwrap();
        assert_eq!(
            fs::metadata(&global).unwrap().modified().unwrap(),
            root_mtime,
            "editing a descendant must leave the root directory stamp unchanged for this proof"
        );

        let second = discover_from_roots(&[], &[global], false, &mut cache);

        assert_eq!(first[0].summary(), "first");
        assert_eq!(second[0].summary(), "second and longer");
        assert_eq!(
            cache.global_scans, 2,
            "the entry stamp must invalidate the root snapshot"
        );
    }

    #[test]
    fn only_the_changed_global_root_is_rescanned() {
        let temp = tempdir().unwrap();
        let one = temp.path().join("one");
        let two = temp.path().join("two");
        write_skill(&one, "alpha", "first");
        let changed = write_skill(&two, "beta", "first");
        let mut cache = DiscoveryCache::default();

        let _ = discover_from_roots(&[], &[one.clone(), two.clone()], false, &mut cache);
        fs::write(
            changed.join("SKILL.md"),
            "---\nname: beta\ndescription: changed and longer\n---\n",
        )
        .unwrap();
        let skills = discover_from_roots(&[], &[one, two], false, &mut cache);

        assert_eq!(skills.len(), 2);
        assert_eq!(
            cache.global_scans, 3,
            "two cold roots plus the one dirty root"
        );
    }

    #[test]
    fn category_directory_changes_invalidate_added_renamed_and_deleted_skills() {
        let temp = tempdir().unwrap();
        let global = temp.path().join("global");
        let category = global.join("category");
        let alpha = write_skill(&category, "alpha", "first");
        let mut cache = DiscoveryCache::default();

        let initial = discover_from_roots(&[], std::slice::from_ref(&global), false, &mut cache);
        assert_eq!(initial.len(), 1);

        let beta = write_skill(&category, "beta", "second");
        let added = discover_from_roots(&[], std::slice::from_ref(&global), false, &mut cache);
        assert_eq!(
            added
                .iter()
                .map(|skill| skill.name.as_str())
                .collect::<Vec<_>>(),
            vec!["alpha", "beta"]
        );

        let renamed = category.join("renamed-alpha");
        fs::rename(&alpha, &renamed).unwrap();
        let after_rename =
            discover_from_roots(&[], std::slice::from_ref(&global), false, &mut cache);
        assert_eq!(after_rename.len(), 2);
        assert!(after_rename.iter().any(|skill| skill.dir == renamed));
        assert!(!after_rename.iter().any(|skill| skill.dir == alpha));

        fs::remove_dir_all(&beta).unwrap();
        let after_delete = discover_from_roots(&[], &[global], false, &mut cache);
        assert_eq!(after_delete.len(), 1);
        assert_eq!(after_delete[0].dir, renamed);
        assert_eq!(
            cache.global_scans, 4,
            "each category entry change must invalidate the cached root"
        );
    }

    #[test]
    fn clearing_the_cache_forces_every_active_root_to_rescan() {
        let temp = tempdir().unwrap();
        let one = temp.path().join("one");
        let two = temp.path().join("two");
        write_skill(&one, "alpha", "first");
        write_skill(&two, "beta", "second");
        let roots = vec![one, two];
        let mut cache = DiscoveryCache::default();

        let initial = discover_from_roots(&[], &roots, false, &mut cache);
        let warm = discover_from_roots(&[], &roots, false, &mut cache);
        assert_eq!(initial, warm);
        assert_eq!(cache.global_scans, 2);

        cache.clear();
        let rescanned = discover_from_roots(&[], &roots, false, &mut cache);

        assert_eq!(rescanned, initial);
        assert_eq!(cache.global_scans, 4);
    }

    #[test]
    fn restoring_an_evicted_global_root_rescans_only_that_root() {
        let temp = tempdir().unwrap();
        let one = temp.path().join("one");
        let two = temp.path().join("two");
        write_skill(&one, "alpha", "first");
        write_skill(&two, "beta", "second");
        let both = vec![one.clone(), two.clone()];
        let mut cache = DiscoveryCache::default();

        let initial = discover_from_roots(&[], &both, false, &mut cache);
        let reduced = discover_from_roots(&[], std::slice::from_ref(&one), false, &mut cache);
        let restored = discover_from_roots(&[], &both, false, &mut cache);

        assert_eq!(initial.len(), 2);
        assert_eq!(
            reduced
                .iter()
                .map(|skill| skill.name.as_str())
                .collect::<Vec<_>>(),
            vec!["alpha"]
        );
        assert_eq!(restored, initial);
        assert_eq!(
            cache.global_scans, 3,
            "the retained root stays warm while the restored root scans once"
        );
    }

    #[test]
    fn disabling_global_discovery_keeps_the_cache_warm() {
        let temp = tempdir().unwrap();
        let global = temp.path().join("global");
        write_skill(&global, "alpha", "first");
        let mut cache = DiscoveryCache::default();

        let _ = discover_from_roots(&[], std::slice::from_ref(&global), false, &mut cache);
        let hidden = discover_from_roots(&[], &[], false, &mut cache);
        let restored = discover_from_roots(&[], &[global], false, &mut cache);

        assert!(hidden.is_empty());
        assert_eq!(restored.len(), 1);
        assert_eq!(cache.global_scans, 1);
    }

    #[test]
    fn internal_filtering_reuses_the_unfiltered_snapshot() {
        let temp = tempdir().unwrap();
        let global = temp.path().join("global");
        let dir = write_skill(&global, "private", "hidden");
        fs::write(
            dir.join("SKILL.md"),
            "---\nname: private\ndescription: hidden\nmetadata:\n  internal: true\n---\n",
        )
        .unwrap();
        let mut cache = DiscoveryCache::default();

        let hidden = discover_from_roots(&[], std::slice::from_ref(&global), false, &mut cache);
        let shown = discover_from_roots(&[], &[global], true, &mut cache);

        assert!(hidden.is_empty());
        assert_eq!(shown.len(), 1);
        assert_eq!(cache.global_scans, 1);
    }

    #[test]
    fn a_missing_global_root_is_rechecked_when_it_appears() {
        let temp = tempdir().unwrap();
        let global = temp.path().join("later");
        let mut cache = DiscoveryCache::default();

        let missing = discover_from_roots(&[], std::slice::from_ref(&global), false, &mut cache);
        write_skill(&global, "alpha", "appeared");
        let present = discover_from_roots(&[], &[global], false, &mut cache);

        assert!(missing.is_empty());
        assert_eq!(present.len(), 1);
        assert_eq!(cache.global_scans, 2);
    }
}
