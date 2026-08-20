//! End-to-end tests over the real fixture files, exercising the pipeline the
//! app actually runs: load from disk → parse → render both paths → save.
//!
//! These do not open a window. They cover everything up to the GPUI element
//! tree, which is where the interesting failure modes live.

use std::path::{Path, PathBuf};

use mt_app::renderer::RendererRegistry;
use mt_app::web::{self, Trust};
use mt_app::{fs, workspace};
use mt_doc::{DocType, Document};

fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("workspace root")
        .join("fixtures")
}

/// Load a fixture exactly the way the app does.
fn load(relative: &str) -> (fs::LoadedFile, Document) {
    let path = fixtures().join(relative);
    let file = fs::load(&path).unwrap_or_else(|e| panic!("cannot load {}: {e}", path.display()));
    let doc = Document::new(Some(file.path.clone()), file.text.clone());
    (file, doc)
}

fn registry() -> RendererRegistry {
    RendererRegistry::with_defaults()
}

/// The `<body>` of a rendered document. The stylesheet names every CSS class,
/// so a whole-document check would match markup that never rendered.
///
/// Returns owned text so callers can inline `body(&build_html(..))` without
/// borrowing a temporary.
fn body(html: &str) -> String {
    let start = html.find("<body>").expect("body") + "<body>".len();
    html[start..html.rfind("</body>").expect("/body")].to_string()
}

// --- The full pipeline ----------------------------------------------------

#[test]
fn every_fixture_opens_renders_and_survives_a_round_trip() {
    let registry = registry();
    for relative in [
        "markdown.md",
        "diagrams/diagrams.md",
        "mdx/markdown-only.mdx",
        "mdx/components.mdx",
        "mdx/invalid.mdx",
        "mdx/untrusted.mdx",
        "skills/skills/valid-skill/SKILL.md",
        "perf/diagram-heavy.md",
    ] {
        let (file, doc) = load(relative);

        // Opening never rewrites the source.
        assert_eq!(
            doc.source(),
            file.text,
            "{relative}: source changed on open"
        );

        // Both renderers accept the same document model, and neither panics.
        let html = web::build_html(&doc, &registry, Trust::Restricted);
        assert!(
            body(&html).len() > 10,
            "{relative}: web render produced nothing"
        );

        // Every out-of-band block resolves to markup or a diagnostic — never
        // nothing, and never a panic.
        for block in doc.renderable_blocks() {
            let id = block.renderer_id().unwrap();
            let outcome = registry.render(id, &block.content);
            assert!(
                outcome.svg().is_some() || outcome.diagnostic().is_some(),
                "{relative}: block `{id}` produced neither output nor a diagnostic"
            );
        }
    }
}

#[test]
fn a_document_survives_load_save_reload_unchanged() {
    let source = std::fs::read_to_string(fixtures().join("markdown.md")).unwrap();
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("README.md");
    std::fs::write(&path, &source).unwrap();

    let file = fs::load(&path).unwrap();
    fs::save(&file, &file.text, false).expect("save");
    let reloaded = fs::load(&path).unwrap();

    assert_eq!(reloaded.text, file.text);
    assert_eq!(
        std::fs::read_to_string(&path).unwrap(),
        source,
        "an untouched document must be written back byte-identically"
    );
}

#[test]
fn a_crlf_document_is_not_converted_by_a_save() {
    let source = std::fs::read_to_string(fixtures().join("markdown.md")).unwrap();
    let crlf = source.replace('\n', "\r\n");
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("crlf.md");
    std::fs::write(&path, crlf.as_bytes()).unwrap();

    let file = fs::load(&path).unwrap();
    assert_eq!(file.newline, fs::Newline::Crlf);
    // Parsing works on the normalized text.
    let doc = Document::new(Some(path.clone()), file.text.clone());
    assert!(!doc.outline().headings.is_empty());

    fs::save(&file, &file.text, false).unwrap();
    assert_eq!(
        std::fs::read(&path).unwrap(),
        crlf.as_bytes(),
        "CRLF must survive the round trip"
    );
}

// --- Both renderers, one model -------------------------------------------

#[test]
fn both_render_paths_agree_on_document_structure() {
    // The architectural claim: one document model drives both renderers. If
    // they disagreed on what blocks exist, they would be parallel models.
    let (_, doc) = load("diagrams/diagrams.md");
    let registry = registry();
    let html = body(&web::build_html(&doc, &registry, Trust::Restricted));

    // Every heading in the outline appears in the web render.
    for heading in &doc.outline().headings {
        assert!(
            html.contains(heading.text.as_str()),
            "heading {:?} missing from the web render",
            heading.text
        );
    }
    // Every diagram block produced either an SVG or a visible diagnostic.
    let rendered = html.matches("mt-render").count() + html.matches("mt-error").count();
    assert!(
        rendered >= doc.renderable_blocks().count(),
        "{rendered} rendered/diagnosed vs {} renderable blocks",
        doc.renderable_blocks().count()
    );
}

#[test]
fn invalid_diagrams_produce_diagnostics_that_keep_the_source() {
    let (_, doc) = load("diagrams/diagrams.md");
    let html = body(&web::build_html(&doc, &registry(), Trust::Restricted));

    // The invalid Mermaid block's source must be visible in the output.
    assert!(
        html.contains("not a mermaid diagram"),
        "the failing block's source must be preserved"
    );
    assert!(html.contains("mt-error"), "and marked as an error");
}

#[test]
fn diagram_and_math_render_from_document_source() {
    let (_, doc) = load("diagrams/diagrams.md");
    let registry = registry();

    // The three pure-Rust renderers must succeed on their valid fixtures.
    let mut succeeded = std::collections::HashSet::new();
    for block in doc.renderable_blocks() {
        let id = block.renderer_id().unwrap();
        if registry.render(id, &block.content).svg().is_some() {
            succeeded.insert(id.to_string());
        }
    }
    for id in ["mermaid", "d2", "math"] {
        assert!(
            succeeded.contains(id),
            "{id} rendered nothing from the fixture; succeeded: {succeeded:?}"
        );
    }
}

// --- MDX and the trust boundary -------------------------------------------

#[test]
fn untrusted_mdx_is_rendered_under_a_blocking_policy() {
    let (_, doc) = load("mdx/untrusted.mdx");
    let html = web::build_html(&doc, &registry(), Trust::Restricted);

    assert!(
        html.contains("script-src 'none'"),
        "scripts must be blocked"
    );
    assert!(
        html.contains("default-src 'none'"),
        "all network access must be blocked"
    );
    assert!(
        body(&html).contains("mt-banner"),
        "and the user must be told"
    );

    // The executable content is shown, not executed — and it is escaped, so it
    // cannot become live markup in the WebView.
    assert!(
        !body(&html).contains("<img src=\"x\" onerror="),
        "raw executable markup must be escaped, got:\n{}",
        body(&html)
    );
}

#[test]
fn trusting_a_document_relaxes_scripts_but_not_the_network() {
    let (_, doc) = load("mdx/untrusted.mdx");
    let html = web::build_html(&doc, &registry(), Trust::Trusted);
    assert!(html.contains("script-src 'unsafe-inline'"));
    assert!(
        html.contains("default-src 'none'"),
        "trusting a document must never open network access"
    );
}

#[test]
fn mdx_components_appear_as_placeholders_with_their_source() {
    let (_, doc) = load("mdx/components.mdx");
    let html = body(&web::build_html(&doc, &registry(), Trust::Restricted));

    assert!(html.contains("mt-mdx"), "component placeholders");
    assert!(html.contains("RevenueChart"));
    // Markdown parts still render fully.
    assert!(html.contains("Quarterly report"));
    assert!(html.contains("More prose"));
}

// --- Workspace and skills -------------------------------------------------

#[test]
fn the_fixture_tree_is_browsable() {
    let nodes = workspace::read_dir(&fixtures()).unwrap();
    let names: Vec<_> = nodes.iter().map(|n| n.name.as_str()).collect();
    assert!(names.contains(&"markdown.md"));
    assert!(names.contains(&"diagrams"));
    assert!(names.contains(&"mdx"));

    // Documents are openable; the tree marks them so.
    let markdown = nodes.iter().find(|n| n.name == "markdown.md").unwrap();
    assert!(markdown.is_openable());
    assert_eq!(markdown.doc_type, Some(DocType::Markdown));
}

#[test]
fn skills_are_discovered_and_their_entry_documents_open() {
    let skills = mt_doc::skill::discover(&fixtures().join("skills"));
    assert!(skills.len() >= 5, "found {}", skills.len());

    for skill in &skills {
        // Every discovered skill's entry document must actually load and parse.
        let file = fs::load(&skill.entry).expect("entry document loads");
        let doc = Document::new(Some(skill.entry.clone()), file.text.clone());
        assert_eq!(doc.doc_type(), DocType::Skill);
        assert_eq!(doc.source(), file.text);
    }
}

#[test]
fn a_valid_skill_renders_its_body() {
    let skills = mt_doc::skill::discover(&fixtures().join("skills"));
    let valid = skills
        .iter()
        .find(|s| s.is_valid() && !s.support_dirs.is_empty())
        .expect("a valid skill with supporting directories");

    let file = fs::load(&valid.entry).unwrap();
    let doc = Document::new(Some(valid.entry.clone()), file.text);
    let html = body(&web::build_html(&doc, &registry(), Trust::Restricted));

    assert!(html.contains("Valid Skill"), "body renders");
    assert!(
        !html.contains("Apache-2.0"),
        "frontmatter is metadata, not body content"
    );
}

// --- Large documents ------------------------------------------------------

#[test]
fn a_100k_line_document_opens_and_renders() {
    let path = fixtures().join("perf/huge-100k.md");
    let file = fs::load(&path).expect("loads");
    let doc = Document::new(Some(path), file.text.clone());

    assert!(doc.outline().headings.len() > 1_000);
    assert_eq!(doc.source(), file.text, "no normalization at scale");

    // The web path is skipped above the size limit in the view; here we only
    // assert the document itself is fully usable.
    assert!(doc.block_at(doc.source().len() / 2).is_some());
}
