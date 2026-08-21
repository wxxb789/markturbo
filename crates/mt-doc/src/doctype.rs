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
    /// Source, config or log text — openable in Source, never rendered.
    Text,
    /// `.html` / `.htm` — openable in Web and Source, never rendered natively.
    Html,
    /// Not openable. Everything the allowlist did not recognize lands here.
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
            DocType::Text => "Text",
            DocType::Html => "HTML",
            DocType::Other => "Unknown",
        }
    }

    /// Whether this document participates in the agent-instruction ecosystem.
    ///
    /// Kept as a predicate rather than a separate enum so a future "effective
    /// context" view can filter without a second classification pass.
    pub fn is_agent_artifact(self) -> bool {
        matches!(
            self,
            DocType::Skill
                | DocType::Agents
                | DocType::Claude
                | DocType::CursorRule
                | DocType::Instructions
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

        if matches!(ext.as_str(), "html" | "htm") {
            return DocType::Html;
        }

        // `.env.local` and `.env.production` read the same as `.env`, so the
        // dotfile names match by prefix as well as exactly.
        if TEXT_EXTS.contains(&ext.as_str())
            || TEXT_NAMES.contains(&lower.as_str())
            || lower.starts_with(".env.")
        {
            return DocType::Text;
        }

        DocType::Other
    }

    /// Whether this document can be opened at all. Used by the file tree,
    /// drag-and-drop and the folder search to decide what to admit.
    pub fn is_document(self) -> bool {
        self != DocType::Other
    }

    /// Whether a native rendered preview means anything for this document.
    ///
    /// Only the Markdown family goes through the parse-and-render pipeline.
    /// `Text` is source, `Html` belongs to the WebView, and neither has a
    /// meaningful native render — offering one shows an empty pane.
    pub fn renders(self) -> bool {
        matches!(
            self,
            DocType::Markdown
                | DocType::Mdx
                | DocType::Skill
                | DocType::Agents
                | DocType::Claude
                | DocType::CursorRule
                | DocType::Instructions
        )
    }
}

/// Extensions we are willing to read as text.
///
/// This is an allowlist and must stay one. A denylist would classify every
/// unknown extension as openable, and the first `.png`, `.exe` or `.zip` in the
/// tree would be loaded into an editor as mojibake — or, at repository scale,
/// megabytes of binary walked by the folder search. Anything not listed here
/// stays [`DocType::Other`], which is the "we will not open this" case.
const TEXT_EXTS: &[&str] = &[
    "rs",
    "toml",
    "json",
    "jsonc",
    "yaml",
    "yml",
    "py",
    "js",
    "mjs",
    "cjs",
    "ts",
    "tsx",
    "jsx",
    "sh",
    "bash",
    "zsh",
    "fish",
    "ps1",
    "txt",
    "text",
    "log",
    "csv",
    "tsv",
    "ini",
    "cfg",
    "conf",
    "env",
    "css",
    "scss",
    "less",
    "xml",
    "svg",
    "sql",
    "go",
    "rb",
    "java",
    "kt",
    "kts",
    "c",
    "h",
    "cpp",
    "hpp",
    "cs",
    "swift",
    "php",
    "lua",
    "r",
    "jl",
    "zig",
    "dockerfile",
    "gitignore",
    "gitattributes",
    "editorconfig",
    "lock",
    "properties",
    "gradle",
    "make",
    "mk",
    "cmake",
    "proto",
    "graphql",
    "gql",
    "vue",
    "svelte",
    "astro",
    "diff",
    "patch",
];

/// Conventional file names the extension allowlist cannot reach.
///
/// Matched against the whole file name, because `Path::extension` returns
/// `None` both for `Makefile` and for a leading-dot name like `.gitignore` —
/// Rust does not treat the leading dot as an extension separator.
const TEXT_NAMES: &[&str] = &[
    "makefile",
    "dockerfile",
    "license",
    "licence",
    "copying",
    "notice",
    "authors",
    "changelog",
    "readme",
    "todo",
    "rakefile",
    "gemfile",
    "procfile",
    "justfile",
    "codeowners",
    ".gitignore",
    ".gitattributes",
    ".editorconfig",
    ".env",
];

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
        assert_eq!(
            ty("repo/.cursor/rules/nested/style.md"),
            DocType::CursorRule
        );
    }

    #[test]
    fn recognizes_plain_documents() {
        assert_eq!(ty("README.md"), DocType::Markdown);
        assert_eq!(ty("doc.markdown"), DocType::Markdown);
        assert_eq!(ty("page.mdx"), DocType::Mdx);
        assert_eq!(ty("main.rs"), DocType::Text);
        assert_eq!(ty("no_extension"), DocType::Other);
    }

    #[test]
    fn recognizes_html() {
        assert_eq!(ty("index.html"), DocType::Html);
        assert_eq!(ty("old/page.HTM"), DocType::Html);
    }

    #[test]
    fn recognizes_source_and_config_as_text() {
        for p in [
            "src/main.rs",
            "Cargo.toml",
            "package.json",
            "tsconfig.jsonc",
            "ci.yaml",
            "ci.yml",
            "app.py",
            "a.js",
            "a.mjs",
            "a.cjs",
            "a.ts",
            "a.tsx",
            "run.sh",
            "run.ps1",
            "notes.txt",
            "build.log",
            "data.csv",
            "app.ini",
            "style.css",
            "icon.svg",
            "q.sql",
            "main.go",
            "Main.java",
            "a.cpp",
            "Cargo.lock",
            "schema.proto",
            "App.vue",
            "fix.patch",
        ] {
            assert_eq!(ty(p), DocType::Text, "{p} should be openable text");
        }
    }

    #[test]
    fn recognizes_extensionless_conventional_names() {
        assert_eq!(ty("Makefile"), DocType::Text);
        assert_eq!(ty("repo/Dockerfile"), DocType::Text);
        assert_eq!(ty("LICENSE"), DocType::Text);
        assert_eq!(ty("LICENCE"), DocType::Text);
        assert_eq!(ty("COPYING"), DocType::Text);
        assert_eq!(ty("NOTICE"), DocType::Text);
        assert_eq!(ty("AUTHORS"), DocType::Text);
        assert_eq!(ty("CHANGELOG"), DocType::Text);
        assert_eq!(ty("TODO"), DocType::Text);
        assert_eq!(ty("Rakefile"), DocType::Text);
        assert_eq!(ty("Gemfile"), DocType::Text);
        assert_eq!(ty("Procfile"), DocType::Text);
        assert_eq!(ty("justfile"), DocType::Text);
        assert_eq!(ty(".github/CODEOWNERS"), DocType::Text);
        // Lowercase on disk is the same artifact, as with the Markdown names.
        assert_eq!(ty("makefile"), DocType::Text);
    }

    #[test]
    fn recognizes_dotfiles_that_have_no_extension() {
        // `Path::extension()` is `None` for all of these: the leading dot is
        // part of the name, not a separator.
        assert_eq!(ty(".gitignore"), DocType::Text);
        assert_eq!(ty("repo/.gitattributes"), DocType::Text);
        assert_eq!(ty(".editorconfig"), DocType::Text);
        assert_eq!(ty(".env"), DocType::Text);
        // The suffixed variants are the ones people actually keep on disk.
        assert_eq!(ty(".env.local"), DocType::Text);
        assert_eq!(ty("app/.env.production"), DocType::Text);
    }

    #[test]
    fn binaries_stay_unopenable() {
        // The allowlist is the whole safety story: nothing here is listed, so
        // nothing here is loaded into an editor as mojibake.
        for p in [
            "logo.png",
            "app.exe",
            "bundle.zip",
            "paper.pdf",
            "font.woff2",
            "clip.mp4",
            "lib.so",
            "archive.tar.gz",
        ] {
            assert_eq!(ty(p), DocType::Other, "{p} must not be openable");
            assert!(!ty(p).is_document());
        }
    }

    #[test]
    fn only_the_markdown_family_renders() {
        for p in [
            "README.md",
            "page.mdx",
            "SKILL.md",
            "AGENTS.md",
            "CLAUDE.md",
            "repo/.cursor/rules/style.mdc",
            "x/rust.instructions.md",
        ] {
            assert!(ty(p).renders(), "{p} has a native preview");
        }
        assert!(!ty("main.rs").renders(), "text has no native preview");
        assert!(!ty("index.html").renders(), "HTML belongs to the WebView");
        assert!(!ty("logo.png").renders());
    }

    #[test]
    fn text_and_html_are_openable_but_other_is_not() {
        assert!(ty("main.rs").is_document());
        assert!(ty("index.html").is_document());
        assert!(!ty("logo.png").is_document());
    }

    #[test]
    fn no_two_variants_share_a_label() {
        // `Other` used to be labeled "Text", which collided the moment `Text`
        // became a variant of its own — a tab badge cannot disambiguate a
        // rejected file from a source file.
        let all = [
            DocType::Markdown,
            DocType::Mdx,
            DocType::Skill,
            DocType::Agents,
            DocType::Claude,
            DocType::CursorRule,
            DocType::Instructions,
            DocType::Text,
            DocType::Html,
            DocType::Other,
        ];
        let mut labels: Vec<&str> = all.iter().map(|d| d.label()).collect();
        labels.sort_unstable();
        let before = labels.len();
        labels.dedup();
        assert_eq!(before, labels.len(), "labels must be unique: {labels:?}");
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
