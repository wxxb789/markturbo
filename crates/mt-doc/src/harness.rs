//! Where Agent Skills live.
//!
//! Transcribed from `vercel-labs/skills` (`src/agents.ts`), which is the
//! de-facto registry of harness conventions. Discovery compatibility with that
//! project is the goal, so this stays a faithful transcription rather than a
//! curated subset — including the many harnesses that deliberately share
//! `.agents/skills`, since dedup collapses the overlap anyway.
//!
//! Two shapes matter and neither is `~/<name>/skills`:
//!
//! * **XDG** — Amp, OpenCode, Goose, Crush, Devin and the universal target
//!   resolve under `$XDG_CONFIG_HOME` (upstream uses the `xdg-basedir`
//!   package), falling back to `~/.config`.
//! * **Env override** — Claude Code, Codex, Mistral Vibe, Hermes, Autohand and
//!   Grok each honor their own environment variable before falling back.
//!
//! Getting those wrong is not cosmetic: they are exactly the harnesses whose
//! skills would silently fail to appear.

use std::path::PathBuf;

/// How a harness's global skills directory is located.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GlobalRoot {
    /// `~/<suffix>`.
    Home(&'static str),
    /// `$XDG_CONFIG_HOME/<suffix>`, else `~/.config/<suffix>`.
    XdgConfig(&'static str),
    /// `$<var>/<suffix>` when set, else `~/<fallback>`.
    Env {
        var: &'static str,
        suffix: &'static str,
        fallback: &'static str,
    },
    /// Project-only harness. Eve and PromptScript have no global location.
    None,
}

impl GlobalRoot {
    /// Resolve against the environment. `None` when the harness has no global
    /// directory, or when the home directory cannot be determined.
    pub fn resolve(self) -> Option<PathBuf> {
        match self {
            GlobalRoot::None => None,
            GlobalRoot::Home(suffix) => Some(home()?.join(rel(suffix))),
            GlobalRoot::XdgConfig(suffix) => {
                let base = non_empty("XDG_CONFIG_HOME")
                    .map(PathBuf::from)
                    .or_else(|| Some(home()?.join(".config")))?;
                Some(base.join(rel(suffix)))
            }
            GlobalRoot::Env {
                var,
                suffix,
                fallback,
            } => match non_empty(var) {
                Some(base) => Some(PathBuf::from(base).join(rel(suffix))),
                None => Some(home()?.join(rel(fallback))),
            },
        }
    }
}

/// An environment variable's value, treating blank as unset.
///
/// Upstream trims before testing (`process.env.CODEX_HOME?.trim() || …`), so a
/// variable set to whitespace falls through to the default rather than
/// producing a path rooted at the filesystem root.
fn non_empty(var: &str) -> Option<String> {
    let value = std::env::var(var).ok()?;
    let trimmed = value.trim();
    (!trimmed.is_empty()).then(|| trimmed.to_string())
}

fn home() -> Option<PathBuf> {
    for var in ["HOME", "USERPROFILE"] {
        if let Some(value) = non_empty(var) {
            return Some(PathBuf::from(value));
        }
    }
    // Windows without USERPROFILE, which happens in some service contexts.
    let drive = non_empty("HOMEDRIVE")?;
    let path = non_empty("HOMEPATH")?;
    Some(PathBuf::from(format!("{drive}{path}")))
}

/// Convert a `/`-separated relative path into a platform path.
fn rel(path: &str) -> PathBuf {
    path.split('/').collect()
}

/// One harness's conventions.
#[derive(Debug, Clone, Copy)]
pub struct Harness {
    /// The `--agent` value upstream uses, so the two can be cross-referenced.
    pub id: &'static str,
    /// Workspace-relative skills directory.
    pub project: &'static str,
    pub global: GlobalRoot,
}

const fn h(id: &'static str, project: &'static str, global: GlobalRoot) -> Harness {
    Harness {
        id,
        project,
        global,
    }
}

/// Every harness `vercel-labs/skills` supports.
///
/// Adding one is a single line here; nothing else in the app knows the list.
// One row per harness, one line each: the value of this table is that it can be
// diffed against upstream's `src/agents.ts` at a glance. rustfmt would explode
// each row across four lines and destroy exactly that.
#[rustfmt::skip]
pub const HARNESSES: &[Harness] = &[
    h("aider-desk", ".aider-desk/skills", GlobalRoot::Home(".aider-desk/skills")),
    h("amp", ".agents/skills", GlobalRoot::XdgConfig("agents/skills")),
    h("antigravity", ".agents/skills", GlobalRoot::Home(".gemini/antigravity/skills")),
    h("antigravity-cli", ".agents/skills", GlobalRoot::Home(".gemini/antigravity-cli/skills")),
    h("astrbot", "data/skills", GlobalRoot::Home(".astrbot/data/skills")),
    h("autohand-code", ".autohand/skills", GlobalRoot::Env { var: "AUTOHAND_HOME", suffix: "skills", fallback: ".autohand/skills" }),
    h("augment", ".augment/skills", GlobalRoot::Home(".augment/skills")),
    h("bob", ".bob/skills", GlobalRoot::Home(".bob/skills")),
    h("claude-code", ".claude/skills", GlobalRoot::Env { var: "CLAUDE_CONFIG_DIR", suffix: "skills", fallback: ".claude/skills" }),
    // OpenClaw picks the first of .openclaw / .clawdbot / .moltbot that exists;
    // listing all three lets the `is_dir` check do that selection for us.
    h("openclaw", "skills", GlobalRoot::Home(".openclaw/skills")),
    h("openclaw-clawdbot", "skills", GlobalRoot::Home(".clawdbot/skills")),
    h("openclaw-moltbot", "skills", GlobalRoot::Home(".moltbot/skills")),
    h("cline", ".agents/skills", GlobalRoot::Home(".agents/skills")),
    h("codearts-agent", ".codeartsdoer/skills", GlobalRoot::Home(".codeartsdoer/skills")),
    h("codebuddy", ".codebuddy/skills", GlobalRoot::Home(".codebuddy/skills")),
    h("codemaker", ".codemaker/skills", GlobalRoot::Home(".codemaker/skills")),
    h("codestudio", ".codestudio/skills", GlobalRoot::Home(".codestudio/skills")),
    h("codex", ".agents/skills", GlobalRoot::Env { var: "CODEX_HOME", suffix: "skills", fallback: ".codex/skills" }),
    h("command-code", ".commandcode/skills", GlobalRoot::Home(".commandcode/skills")),
    h("continue", ".continue/skills", GlobalRoot::Home(".continue/skills")),
    h("cortex", ".cortex/skills", GlobalRoot::Home(".snowflake/cortex/skills")),
    h("crush", ".crush/skills", GlobalRoot::Home(".config/crush/skills")),
    h("cursor", ".agents/skills", GlobalRoot::Home(".cursor/skills")),
    h("deepagents", ".agents/skills", GlobalRoot::Home(".deepagents/agent/skills")),
    h("devin", ".devin/skills", GlobalRoot::XdgConfig("devin/skills")),
    h("dexto", ".agents/skills", GlobalRoot::Home(".agents/skills")),
    h("droid", ".factory/skills", GlobalRoot::Home(".factory/skills")),
    h("eve", "agent/skills", GlobalRoot::None),
    h("firebender", ".agents/skills", GlobalRoot::Home(".firebender/skills")),
    h("forgecode", ".forge/skills", GlobalRoot::Home(".forge/skills")),
    h("gemini-cli", ".agents/skills", GlobalRoot::Home(".gemini/skills")),
    h("github-copilot", ".agents/skills", GlobalRoot::Home(".copilot/skills")),
    h("goose", ".goose/skills", GlobalRoot::XdgConfig("goose/skills")),
    h("grok", ".grok/skills", GlobalRoot::Env { var: "GROK_HOME", suffix: "skills", fallback: ".grok/skills" }),
    h("hermes-agent", ".hermes/skills", GlobalRoot::Env { var: "HERMES_HOME", suffix: "skills", fallback: ".hermes/skills" }),
    h("inference-sh", ".inferencesh/skills", GlobalRoot::Home(".inferencesh/skills")),
    h("jazz", ".jazz/skills", GlobalRoot::Home(".jazz/skills")),
    h("junie", ".junie/skills", GlobalRoot::Home(".junie/skills")),
    h("iflow-cli", ".iflow/skills", GlobalRoot::Home(".iflow/skills")),
    h("kilo", ".kilocode/skills", GlobalRoot::Home(".kilocode/skills")),
    h("kimchi", ".kimchi/skills", GlobalRoot::Home(".config/kimchi/harness/skills")),
    h("kimi-code-cli", ".agents/skills", GlobalRoot::Home(".agents/skills")),
    h("kiro-cli", ".kiro/skills", GlobalRoot::Home(".kiro/skills")),
    h("kode", ".kode/skills", GlobalRoot::Home(".kode/skills")),
    h("lingma", ".lingma/skills", GlobalRoot::Home(".lingma/skills")),
    h("loaf", ".agents/skills", GlobalRoot::Home(".agents/skills")),
    h("mcpjam", ".mcpjam/skills", GlobalRoot::Home(".mcpjam/skills")),
    h("minimax-code", ".minimax/skills", GlobalRoot::Home(".minimax/skills")),
    h("mistral-vibe", ".vibe/skills", GlobalRoot::Env { var: "VIBE_HOME", suffix: "skills", fallback: ".vibe/skills" }),
    h("moxby", ".moxby/skills", GlobalRoot::Home(".moxby/skills")),
    h("mux", ".mux/skills", GlobalRoot::Home(".mux/skills")),
    h("neovate", ".neovate/skills", GlobalRoot::Home(".neovate/skills")),
    h("ona", ".ona/skills", GlobalRoot::Home(".ona/skills")),
    h("opencode", ".agents/skills", GlobalRoot::XdgConfig("opencode/skills")),
    h("openhands", ".openhands/skills", GlobalRoot::Home(".openhands/skills")),
    h("pi", ".pi/skills", GlobalRoot::Home(".pi/agent/skills")),
    h("pochi", ".pochi/skills", GlobalRoot::Home(".pochi/skills")),
    h("posit-assistant", ".posit/assistant/skills", GlobalRoot::Home(".posit/assistant/skills")),
    h("promptscript", ".agents/skills", GlobalRoot::None),
    h("qoder", ".qoder/skills", GlobalRoot::Home(".qoder/skills")),
    h("qoder-cn", ".qoder/skills", GlobalRoot::Home(".qoder-cn/skills")),
    h("qwen-code", ".qwen/skills", GlobalRoot::Home(".qwen/skills")),
    h("reasonix", ".reasonix/skills", GlobalRoot::Home(".reasonix/skills")),
    h("replit", ".agents/skills", GlobalRoot::XdgConfig("agents/skills")),
    h("roo", ".roo/skills", GlobalRoot::Home(".roo/skills")),
    h("rovodev", ".rovodev/skills", GlobalRoot::Home(".rovodev/skills")),
    h("tabnine-cli", ".tabnine/agent/skills", GlobalRoot::Home(".tabnine/agent/skills")),
    h("terramind", ".terramind/skills", GlobalRoot::Home(".terramind/skills")),
    h("tinycloud", ".tinycloud/skills", GlobalRoot::Home(".tinycloud/skills")),
    h("trae", ".trae/skills", GlobalRoot::Home(".trae/skills")),
    h("trae-cn", ".trae/skills", GlobalRoot::Home(".trae-cn/skills")),
    h("universal", ".agents/skills", GlobalRoot::XdgConfig("agents/skills")),
    h("warp", ".agents/skills", GlobalRoot::Home(".agents/skills")),
    h("windsurf", ".windsurf/skills", GlobalRoot::Home(".codeium/windsurf/skills")),
    h("zcode", ".zcode/skills", GlobalRoot::Home(".zcode/skills")),
    h("zed", ".agents/skills", GlobalRoot::Home(".agents/skills")),
    h("zencoder", ".zencoder/skills", GlobalRoot::Home(".zencoder/skills")),
    h("zenflow", ".zencoder/skills", GlobalRoot::Home(".zencoder/skills")),
    h("adal", ".adal/skills", GlobalRoot::Home(".adal/skills")),
];

/// Workspace-relative skill directories, deduplicated and ordered.
///
/// Ordering decides which root a skill reachable from several is attributed to,
/// so the conventions people actually name come first.
pub fn project_roots() -> Vec<&'static str> {
    let mut roots: Vec<&'static str> = Vec::with_capacity(HARNESSES.len());
    // `skills/` and `.agents/skills` are the vendor-neutral conventions and
    // `.claude/skills` is the most widely deployed; the rest follow in table
    // order, which keeps this deterministic without hand-ranking 70 harnesses.
    for preferred in PREFERRED_ROOTS {
        roots.push(preferred);
    }
    for harness in HARNESSES {
        if !roots.contains(&harness.project) {
            roots.push(harness.project);
        }
    }
    roots
}

/// Roots that win attribution when a skill is reachable from several.
const PREFERRED_ROOTS: &[&str] = &["skills", ".agents/skills", ".claude/skills"];

/// Which harnesses use a given workspace-relative root.
///
/// Several share one directory (`.agents/skills` alone serves a dozen), so this
/// returns every match rather than a single answer.
pub fn harnesses_for_project(root: &str) -> Vec<&'static str> {
    HARNESSES
        .iter()
        .filter(|h| h.project == root)
        .map(|h| h.id)
        .collect()
}

/// Which harnesses resolve to a given global directory on this machine.
pub fn harnesses_for_global(root: &std::path::Path) -> Vec<&'static str> {
    HARNESSES
        .iter()
        .filter(|h| h.global.resolve().is_some_and(|p| p == root))
        .map(|h| h.id)
        .collect()
}

/// A single display name for whichever harnesses own `root`.
///
/// Grouping by harness needs one label per group, and the shared directories
/// are exactly the ones with several owners — `.agents/skills` is not "amp", it
/// is the vendor-neutral convention. Naming the convention beats picking an
/// arbitrary harness out of twelve.
pub fn label_for_root(root: &std::path::Path, is_global: bool) -> String {
    if is_global {
        let owners = harnesses_for_global(root);
        return match owners.len() {
            1 => owners[0].to_string(),
            0 => fallback_label(root),
            _ => shared_root_label(root),
        };
    }

    // A project root is stored as the workspace-relative string it was joined
    // onto, so recover it by matching path components from the end.
    //
    // Only the *most specific* match counts. OpenClaw's project root is the
    // bare `skills`, which is a component-suffix of every other root — take
    // every match and `.factory/skills` comes back owned by droid plus three
    // OpenClaw variants, and renders as "shared".
    let matches: Vec<&Harness> = HARNESSES
        .iter()
        .filter(|h| ends_with_components(root, h.project))
        .collect();
    let longest = matches
        .iter()
        .map(|h| h.project.split('/').count())
        .max()
        .unwrap_or(0);
    let owners: Vec<&Harness> = matches
        .into_iter()
        .filter(|h| h.project.split('/').count() == longest)
        .collect();

    match owners.len() {
        1 => owners[0].id.to_string(),
        0 => fallback_label(root),
        // Shared: name the convention as the table spells it, not as the path
        // happens to end — the bare `skills` root would otherwise pick up the
        // workspace directory's own name as a prefix.
        _ => owners[0].project.to_string(),
    }
}

/// A label for a root no harness in the table claims.
fn fallback_label(root: &std::path::Path) -> String {
    root.file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("unknown")
        .to_string()
}

/// Whether `path` ends with every component of the `/`-separated `suffix`.
///
/// Component-wise rather than textual: `.factory/skills` must not be reported as
/// ending with the bare `skills`, or every uniquely-owned root would look shared
/// with the three harnesses whose project root is just `skills`.
fn ends_with_components(path: &std::path::Path, suffix: &str) -> bool {
    let wanted: Vec<&str> = suffix.split('/').collect();
    let actual: Vec<&str> = path
        .components()
        .filter_map(|c| c.as_os_str().to_str())
        .collect();
    actual.len() >= wanted.len() && actual[actual.len() - wanted.len()..] == wanted[..]
}

/// The name of a global directory shared by several harnesses.
fn shared_root_label(root: &std::path::Path) -> String {
    let text = root.to_string_lossy().replace('\\', "/");
    // The last two components are the convention (`.agents/skills`); one alone
    // would render every shared root as the indistinguishable "skills".
    let parts: Vec<&str> = text.rsplit('/').take(2).collect();
    match parts.len() {
        2 => format!("{}/{}", parts[1], parts[0]),
        _ => text,
    }
}

/// Global skill directories that exist on this machine, in priority order.
///
/// Resolved and deduplicated by path — many harnesses share one directory, and
/// scanning `~/.agents/skills` seventy times would be absurd.
pub fn global_roots() -> Vec<PathBuf> {
    let mut roots: Vec<PathBuf> = Vec::new();
    // Mirror `project_roots`' preference so a skill present in both
    // ~/.agents/skills and ~/.claude/skills is attributed the same way.
    let preferred = [
        GlobalRoot::Home(".agents/skills"),
        GlobalRoot::Env {
            var: "CLAUDE_CONFIG_DIR",
            suffix: "skills",
            fallback: ".claude/skills",
        },
    ];
    for root in preferred
        .into_iter()
        .chain(HARNESSES.iter().map(|h| h.global))
    {
        if let Some(path) = root.resolve()
            && !roots.contains(&path)
        {
            roots.push(path);
        }
    }
    roots
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_harness_id_is_unique() {
        let mut ids: Vec<&str> = HARNESSES.iter().map(|h| h.id).collect();
        ids.sort_unstable();
        let count = ids.len();
        ids.dedup();
        assert_eq!(ids.len(), count, "duplicate harness id");
    }

    #[test]
    fn project_roots_are_deduplicated_and_preference_ordered() {
        let roots = project_roots();
        let mut sorted = roots.clone();
        sorted.sort_unstable();
        let count = sorted.len();
        sorted.dedup();
        assert_eq!(sorted.len(), count, "duplicate project root: {roots:?}");

        // Many harnesses share `.agents/skills`; it must appear once, early.
        assert_eq!(&roots[..3], PREFERRED_ROOTS);
        assert!(
            roots.len() > 40,
            "expected the full table, got {}",
            roots.len()
        );
    }

    #[test]
    fn a_relative_root_uses_platform_separators() {
        let path = rel(".posit/assistant/skills");
        assert_eq!(path.components().count(), 3);
        assert!(path.ends_with("skills"));
    }

    #[test]
    fn env_roots_prefer_their_variable() {
        let root = GlobalRoot::Env {
            var: "MT_TEST_HARNESS_HOME",
            suffix: "skills",
            fallback: ".fallback/skills",
        };

        // SAFETY: single-threaded test; the variable is unique to this test.
        unsafe { std::env::set_var("MT_TEST_HARNESS_HOME", "/tmp/harness") };
        assert!(root.resolve().unwrap().ends_with("skills"));
        assert!(
            root.resolve()
                .unwrap()
                .to_string_lossy()
                .contains("harness")
        );

        // Blank means unset, matching upstream's `?.trim() ||`.
        unsafe { std::env::set_var("MT_TEST_HARNESS_HOME", "   ") };
        let fallback = root.resolve().unwrap();
        assert!(
            fallback.to_string_lossy().contains(".fallback"),
            "blank must fall back, got {}",
            fallback.display()
        );

        unsafe { std::env::remove_var("MT_TEST_HARNESS_HOME") };
        assert!(
            root.resolve()
                .unwrap()
                .to_string_lossy()
                .contains(".fallback")
        );
    }

    #[test]
    fn xdg_roots_fall_back_to_dot_config() {
        let root = GlobalRoot::XdgConfig("opencode/skills");
        // SAFETY: single-threaded test.
        unsafe { std::env::remove_var("XDG_CONFIG_HOME") };
        let path = root.resolve().expect("home is always resolvable in tests");
        assert!(
            path.to_string_lossy().contains(".config"),
            "expected ~/.config fallback, got {}",
            path.display()
        );
        assert!(path.ends_with("skills"));
    }

    #[test]
    fn project_only_harnesses_have_no_global_root() {
        for id in ["eve", "promptscript"] {
            let harness = HARNESSES.iter().find(|h| h.id == id).unwrap();
            assert_eq!(harness.global.resolve(), None, "{id} must be project-only");
        }
    }

    #[test]
    fn global_roots_are_deduplicated() {
        let roots = global_roots();
        let mut sorted = roots.clone();
        sorted.sort();
        let count = sorted.len();
        sorted.dedup();
        assert_eq!(
            sorted.len(),
            count,
            "the many harnesses sharing ~/.agents/skills must collapse"
        );
        // Two project-only harnesses contribute nothing.
        assert!(roots.len() < HARNESSES.len());
    }

    #[test]
    fn a_uniquely_owned_root_is_labelled_with_its_harness() {
        let root = PathBuf::from("/home/u/project/.factory/skills");
        assert_eq!(label_for_root(&root, false), "droid");
    }

    #[test]
    fn a_shared_root_is_labelled_with_the_convention_not_one_owner() {
        // `.agents/skills` serves a dozen harnesses; calling the group "amp"
        // because it sorts first would be arbitrary and misleading.
        let root = PathBuf::from("/home/u/project/.agents/skills");
        let owners = harnesses_for_project(".agents/skills");
        assert!(owners.len() > 5, "expected a shared root, got {owners:?}");
        assert_eq!(label_for_root(&root, false), ".agents/skills");
    }

    #[test]
    fn an_unknown_root_falls_back_to_its_directory_name() {
        let root = PathBuf::from("/somewhere/entirely/custom");
        assert_eq!(label_for_root(&root, false), "custom");
    }

    #[test]
    fn every_project_root_produces_a_non_empty_label() {
        // Grouping by harness must never produce a blank header.
        for root in project_roots() {
            let path = PathBuf::from("/w").join(rel(root));
            let label = label_for_root(&path, false);
            assert!(!label.trim().is_empty(), "{root} produced no label");
        }
    }

    #[test]
    fn windows_separators_do_not_defeat_root_matching() {
        // Project roots are stored joined onto the workspace, so on Windows the
        // path uses backslashes while the table uses forward slashes.
        let root = PathBuf::from(r"Q:\repos\demo\.factory\skills");
        assert_eq!(label_for_root(&root, false), "droid");
    }

    #[test]
    fn the_bare_skills_root_does_not_claim_every_other_root() {
        // OpenClaw's project root is `skills`, a component-suffix of all 70
        // others. Only the most specific match may own a root.
        assert!(
            harnesses_for_project("skills")
                .iter()
                .any(|id| id.starts_with("openclaw")),
            "the bare `skills` root should exist in the table"
        );
        for root in [".factory/skills", ".claude/skills", ".goose/skills"] {
            let path = PathBuf::from("/w").join(rel(root));
            let label = label_for_root(&path, false);
            assert!(
                !label.starts_with("openclaw"),
                "{root} was claimed by the bare `skills` root as {label}"
            );
        }
        // And the bare root resolves to the convention itself, without picking
        // up the workspace directory's own name.
        assert_eq!(label_for_root(&PathBuf::from("/w/skills"), false), "skills");
    }
}
