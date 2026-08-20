//! The sample workspace shipped with a release must actually work.
//!
//! It is the first thing a new user opens, so a broken fixture there is a
//! broken first impression. These assert the claims `sample/README.md` makes.

use std::path::{Path, PathBuf};

use mt_app::renderer::RendererRegistry;
use mt_app::web::{self, Trust};
use mt_app::{fs, workspace};
use mt_doc::{DocType, Document, Severity};

fn sample() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("workspace root")
        .join("sample")
}

fn open(relative: &str) -> Document {
    let path = sample().join(relative);
    let file = fs::load(&path).unwrap_or_else(|e| panic!("cannot load {}: {e}", path.display()));
    Document::new(Some(path), file.text)
}

#[test]
fn every_sample_document_opens_without_errors() {
    for relative in ["README.md", "AGENTS.md", "docs/diagrams.md"] {
        let doc = open(relative);
        let errors: Vec<_> = doc
            .diagnostics()
            .iter()
            .filter(|d| d.severity == Severity::Error)
            .collect();
        assert!(
            errors.is_empty(),
            "{relative} has errors a first-time user would see: {errors:?}"
        );
    }
}

#[test]
fn the_readme_demonstrates_what_it_claims() {
    let doc = open("README.md");

    // It promises live Mermaid and math.
    let ids: Vec<_> = doc
        .renderable_blocks()
        .filter_map(|b| b.renderer_id())
        .collect();
    assert!(ids.contains(&"mermaid"), "README promises Mermaid");
    assert!(ids.contains(&"math"), "README promises math");

    // And that both actually render, since it tells the user to look at them.
    let registry = RendererRegistry::with_defaults();
    for block in doc.renderable_blocks() {
        let id = block.renderer_id().unwrap();
        assert!(
            registry.render(id, &block.content).svg().is_some(),
            "the README's {id} block does not render: {:?}",
            registry.render(id, &block.content).diagnostic()
        );
    }
}

#[test]
fn agent_artifacts_are_labelled_as_the_readme_says() {
    // The README shows a table of recognized names; the sample must back it up.
    assert_eq!(DocType::of(&sample().join("AGENTS.md")), DocType::Agents);
    assert_eq!(
        DocType::of(&sample().join(".claude/skills/hello-diagrams/SKILL.md")),
        DocType::Skill
    );
    assert_eq!(DocType::of(&sample().join("README.md")), DocType::Markdown);
}

#[test]
fn the_diagram_page_renders_its_valid_blocks_and_diagnoses_its_invalid_ones() {
    let doc = open("docs/diagrams.md");
    let registry = RendererRegistry::with_defaults();

    let mut rendered = std::collections::HashSet::new();
    let mut diagnosed = 0;
    for block in doc.renderable_blocks() {
        let id = block.renderer_id().unwrap();
        let outcome = registry.render(id, &block.content);
        match (outcome.svg(), outcome.diagnostic()) {
            (Some(_), _) => {
                rendered.insert(id.to_string());
            }
            (_, Some(_)) => diagnosed += 1,
            _ => panic!("{id} produced neither output nor a diagnostic"),
        }
    }

    // The three always-available renderers must succeed on this page.
    for id in ["mermaid", "d2", "math"] {
        assert!(
            rendered.contains(id),
            "docs/diagrams.md: {id} rendered nothing; rendered: {rendered:?}"
        );
    }
    // And the deliberately-invalid blocks must be caught, not silently drawn.
    assert!(
        diagnosed >= 2,
        "expected the invalid Mermaid and math blocks to be diagnosed, got {diagnosed}"
    );
}

#[test]
fn the_unregistered_technology_is_reported_as_such() {
    // The page claims graphviz is reported rather than shown as a code block.
    let doc = open("docs/diagrams.md");
    assert!(
        doc.diagnostics().iter().any(|d| d.source == "graphviz"),
        "graphviz should report that no renderer is registered"
    );
}

#[test]
fn the_sample_skills_show_both_a_valid_and_an_invalid_case() {
    let skills = mt_doc::skill::discover(&sample());
    let names: Vec<_> = skills.iter().map(|s| s.name.as_str()).collect();
    assert!(names.contains(&"hello-diagrams"), "got {names:?}");

    let valid = skills.iter().find(|s| s.name == "hello-diagrams").unwrap();
    assert!(
        valid.is_valid(),
        "the sample's good skill must validate cleanly: {:?}",
        valid.diagnostics
    );
    assert!(
        valid.meta.description.is_some() && valid.meta.license.is_some(),
        "the inspector needs populated metadata to be worth opening"
    );
    assert_eq!(
        valid.support_dirs.len(),
        2,
        "scripts/ and references/ should both be detected"
    );

    // The broken one must be listed *and* flagged — the README says so.
    let broken = skills
        .iter()
        .find(|s| s.dir.ends_with("broken-example"))
        .expect("the broken skill must still appear in the list");
    assert!(!broken.is_valid());
    let messages: Vec<_> = broken
        .diagnostics
        .iter()
        .map(|d| d.message.as_str())
        .collect();
    assert!(
        messages.iter().any(|m| m.contains("description")),
        "missing description should be reported: {messages:?}"
    );
    assert!(
        messages.iter().any(|m| m.contains("lowercase")),
        "non-lowercase name should be reported: {messages:?}"
    );
    assert!(
        messages.iter().any(|m| m.contains("model")),
        "the non-spec field should be reported: {messages:?}"
    );
}

#[test]
fn translation_preserves_what_the_readme_promises() {
    use mt_doc::translate::Scope;

    let doc = open("README.md");
    let service = mt_app::translate::Provider::Echo.build().unwrap();
    let out = mt_doc::translate::translate(&doc, &Scope::Document, "zh", service.as_ref()).unwrap();

    // The README names exactly these as untouched. Verify each.
    assert!(
        out.text.contains("let value = String::new();"),
        "code must survive"
    );
    assert!(out.text.contains("`code`"), "inline code must survive");
    assert!(
        out.text.contains("(https://example.com/unchanged)"),
        "link targets must survive"
    );
    assert!(
        out.text.contains("Human -->|writes| Markdown;"),
        "diagram source must survive"
    );
    assert!(out.text.contains(r"\frac{n(n+1)}{2}"), "math must survive");

    // Block markup survives even though the text after it is translated —
    // that is the whole point, so assert the marker, not the heading text.
    assert!(
        out.text.lines().any(|l| l.starts_with("# ")),
        "the `#` heading marker must survive"
    );
    assert!(
        out.text.lines().any(|l| l.starts_with("## ")),
        "`##` must survive"
    );
    assert!(
        out.text.lines().any(|l| l.starts_with("1. ")),
        "ordered list markers must survive"
    );
    assert!(
        out.text.lines().any(|l| l.trim_start().starts_with("- ")),
        "bullet markers must survive"
    );
    assert!(
        out.text.lines().any(|l| l.starts_with("> ")),
        "blockquote markers must survive"
    );

    // Table structure is unchanged: same pipe count in and out.
    assert_eq!(
        out.text.matches('|').count(),
        doc.source().matches('|').count(),
        "table columns changed"
    );

    // And prose was actually processed, so the demo shows something.
    assert!(out.text.contains("[zh]"), "prose should be marked");
}

#[test]
fn the_sample_tree_is_browsable_and_shows_its_documents() {
    let nodes = workspace::read_dir(&sample()).unwrap();
    let names: Vec<_> = nodes.iter().map(|n| n.name.as_str()).collect();
    assert!(names.contains(&"README.md"));
    assert!(names.contains(&"AGENTS.md"));
    assert!(names.contains(&"docs"));

    for node in nodes.iter().filter(|n| !n.is_dir) {
        assert!(
            node.is_openable(),
            "{} is in the sample but cannot be opened",
            node.name
        );
    }
}

#[test]
fn sample_documents_render_through_the_web_path_too() {
    let registry = RendererRegistry::with_defaults();
    for relative in ["README.md", "AGENTS.md", "docs/diagrams.md"] {
        let doc = open(relative);
        let html = web::build_html(&doc, &registry, Trust::Restricted);
        assert!(
            html.contains("<svg") || relative == "AGENTS.md",
            "{relative}: expected rendered diagrams in the web path"
        );
        assert!(
            html.contains("script-src 'none'"),
            "{relative}: the default policy must block scripts"
        );
    }
}

#[test]
fn the_sample_uses_lf_endings_so_it_looks_the_same_everywhere() {
    // `.gitattributes` marks fixtures binary; the sample should be committed
    // with LF so a Windows checkout does not show stray characters.
    for relative in ["README.md", "AGENTS.md", "docs/diagrams.md"] {
        let bytes = std::fs::read(sample().join(relative)).unwrap();
        assert!(
            !bytes.windows(2).any(|w| w == b"\r\n"),
            "{relative} contains CRLF"
        );
    }
}
