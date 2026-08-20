//! Document type recognition.
//!
//! v0.1 only needs recognition + labeling. The important architectural property
//! is that recognition is a pure function of a path, so a future "effective
//! agent context" resolver can reuse it without touching the UI.

use std::path::Path;

/// What kind of artifact a file is, from the perspective of human-agent work.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DocType {
    /// Ordinary Markdown.
    Markdown,
    /// MDX: Markdown + JSX + ESM.
    Mdx,
    /// `SKILL.md` — the entry document of an Agent Skill.
    Skill,
    /// `AGENTS.md` — cross-vendor agent instructions.
    Agents,
    /// `CLAUDE.md` / `CLAUDE.local.md`.
    Claude,
    /// A `.cursor/rules/*` rule file.
    CursorRule,
    /// `*.instructions.md` (Copilot-style scoped instructions).
    Instructions,
    /// Anything else we can still show as text.
    Other,
}

impl DocType {
    /// Human-facing label, used for tab badges and the inspector.
    pub fn label(self) -> &'static str {
        match self {
            DocType::Markdown => "Markdown",
            DocType::Mdx => "MDX",
            DocType::Skill => "Agent Skill",
            DocType::Agents => "Agent Instructions",
            DocType::Claude => "Claude Instructions",
            DocType::CursorRule => "Cursor Rule",
            DocType::Instructions => "Scoped Instructions",
            DocType::Other => "Text",
        }
    }

    /// Whether this document participates in the agent-instruction ecosystem.
    ///
    /// Kept as a predicate rather than a separate enum so a future "effective
    /// context" view can filter without a second classification pass.
    pub fn is_agent_artifact(self) -> bool {
        matches!(
            self,
            DocType::Skill | DocType::Agents | DocType::Claude | DocType::CursorRule | DocType::Instructions
        )
    }

    /// Whether MDX constructs should be enabled when parsing.
    pub fn is_mdx(self) -> bool {
        self == DocType::Mdx
    }

    /// Classify a path. Specialized names win over the generic extension so
    /// `AGENTS.md` is an agent artifact rather than plain Markdown.
    pub fn of(path: &Path) -> Self {
        let name = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or_default();
        let ext = path
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or_default()
            .to_ascii_lowercase();

        if ext == "mdx" {
            return DocType::Mdx;
        }

        // Specialized Markdown names. Compared case-insensitively because
        // Windows and macOS filesystems are typically case-insensitive, so a
        // `Agents.md` on disk is the same artifact.
        let lower = name.to_ascii_lowercase();
        match lower.as_str() {
            "skill.md" => return DocType::Skill,
            "agents.md" => return DocType::Agents,
            "claude.md" | "claude.local.md" => return DocType::Claude,
            _ => {}
        }

        if lower.ends_with(".instructions.md") {
            return DocType::Instructions;
        }

        if is_cursor_rule(path) {
            return DocType::CursorRule;
        }

        if matches!(ext.as_str(), "md" | "markdown" | "mdown" | "mkd") {
            return DocType::Markdown;
        }

        DocType::Other
    }

    /// Whether this document can be shown by the Markdown/MDX document pipeline
    /// at all. Used by the file tree to decide what is openable.
    pub fn is_document(self) -> bool {
        self != DocType::Other
    }
}

/// True when `path` sits under a `.cursor/rules` directory.
///
/// Cursor rules are `.mdc` today but `.md` files under that directory are still
/// treated as rules by several tools, so match on location rather than
/// extension.
fn is_cursor_rule(path: &Path) -> bool {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    if !matches!(ext.as_str(), "mdc" | "md" | "markdown") {
        return false;
    }

    let components: Vec<String> = path
        .components()
        .filter_map(|c| c.as_os_str().to_str())
        .map(|s| s.to_ascii_lowercase())
        .collect();

    components
        .windows(2)
        .any(|w| w[0] == ".cursor" && w[1] == "rules")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ty(p: &str) -> DocType {
        DocType::of(Path::new(p))
    }

    #[test]
    fn recognizes_agent_artifacts() {
        assert_eq!(ty("a/b/SKILL.md"), DocType::Skill);
        assert_eq!(ty("AGENTS.md"), DocType::Agents);
        assert_eq!(ty("repo/CLAUDE.md"), DocType::Claude);
        assert_eq!(ty("repo/CLAUDE.local.md"), DocType::Claude);
        assert_eq!(ty("x/rust.instructions.md"), DocType::Instructions);
        assert_eq!(ty("repo/.cursor/rules/style.mdc"), DocType::CursorRule);
        assert_eq!(ty("repo/.cursor/rules/nested/style.md"), DocType::CursorRule);
    }

    #[test]
    fn recognizes_plain_documents() {
        assert_eq!(ty("README.md"), DocType::Markdown);
        assert_eq!(ty("doc.markdown"), DocType::Markdown);
        assert_eq!(ty("page.mdx"), DocType::Mdx);
        assert_eq!(ty("main.rs"), DocType::Other);
        assert_eq!(ty("no_extension"), DocType::Other);
    }

    #[test]
    fn mdx_wins_over_specialized_names() {
        // A `SKILL.mdx` is MDX first: the renderer path matters more than the
        // label, and a skill's entry document is spec'd as SKILL.md anyway.
        assert_eq!(ty("s/SKILL.mdx"), DocType::Mdx);
    }

    #[test]
    fn cursor_rules_need_the_directory_pair() {
        // `.cursor/foo.md` is not a rule; only `.cursor/rules/**`.
        assert_eq!(ty("repo/.cursor/foo.md"), DocType::Markdown);
        assert_eq!(ty("repo/rules/style.md"), DocType::Markdown);
    }

    #[test]
    fn case_insensitive_names() {
        assert_eq!(ty("agents.md"), DocType::Agents);
        assert_eq!(ty("Skill.MD"), DocType::Skill);
    }

    #[test]
    fn agent_artifact_predicate() {
        assert!(ty("AGENTS.md").is_agent_artifact());
        assert!(!ty("README.md").is_agent_artifact());
    }
}
