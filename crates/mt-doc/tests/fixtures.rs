//! Integration tests over the checked-in fixtures.
//!
//! These assert the behaviors the goal requires of real files on disk, rather
//! than of strings constructed in a unit test: the same documents a user opens.

use std::path::{Path, PathBuf};

use mt_doc::{BlockKind, DiagramKind, DocType, Document, Severity, skill};

fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("workspace root")
        .join("fixtures")
}

/// Path of `path` relative to `base`, with forward slashes.
fn crate_relative(path: &Path, base: &Path) -> String {
    path.strip_prefix(base)
        .unwrap_or(path)
        .components()
        .filter_map(|c| c.as_os_str().to_str())
        .collect::<Vec<_>>()
        .join("/")
}

fn open(relative: &str) -> Document {
    let path = fixtures().join(relative);
    let source = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("cannot read {}: {e}", path.display()));
    Document::new(Some(path), source)
}

// --- Markdown -------------------------------------------------------------

#[test]
fn markdown_fixture_parses_every_construct() {
    let doc = open("markdown.md");
    assert_eq!(doc.doc_type(), DocType::Markdown);

    let headings: Vec<_> = doc.outline().headings.iter().map(|h| h.text.as_str()).collect();
    for expected in [
        "Heading 1",
        "Heading 2 with code",
        "Heading 3",
        "Lists",
        "Blockquotes",
        "Table",
        "Fenced code",
        "Images",
        "Unicode and CJK",
    ] {
        assert!(
            headings.contains(&expected),
            "missing heading {expected:?} in {headings:?}"
        );
    }

    // Frontmatter is parsed and not a content block.
    assert_eq!(
        doc.frontmatter()
            .and_then(|v| v.get("title"))
            .and_then(|v| v.as_str()),
        Some("Markdown feature sweep")
    );

    // Fenced code keeps its language and is never treated as a diagram.
    let rust = doc
        .blocks()
        .iter()
        .find(|b| matches!(&b.kind, BlockKind::Code { lang: Some(l) } if l == "rust"))
        .expect("rust fence");
    assert!(rust.content.contains("String::new()"));
}

#[test]
fn markdown_fixture_survives_a_lossless_round_trip() {
    let path = fixtures().join("markdown.md");
    let source = std::fs::read_to_string(&path).unwrap();
    let doc = Document::new(Some(path), source.clone());
    assert_eq!(doc.source(), source, "opening must never normalize");
}

#[test]
fn every_block_range_is_a_valid_slice() {
    for name in ["markdown.md", "diagrams/diagrams.md"] {
        let doc = open(name);
        for block in doc.blocks() {
            assert!(
                doc.source().get(block.range.clone()).is_some(),
                "{name}: bad range {:?} (not a char boundary?)",
                block.range
            );
        }
    }
}

// --- Diagrams and math ----------------------------------------------------

#[test]
fn diagram_fixture_classifies_every_technology() {
    let doc = open("diagrams/diagrams.md");
    let kinds: Vec<_> = doc
        .blocks()
        .iter()
        .filter_map(|b| match &b.kind {
            BlockKind::Diagram(k) => Some(k.clone()),
            _ => None,
        })
        .collect();

    for expected in [DiagramKind::Mermaid, DiagramKind::D2, DiagramKind::PlantUml] {
        assert!(
            kinds.iter().filter(|k| **k == expected).count() >= 2,
            "expected a valid and an invalid {expected}, got {kinds:?}"
        );
    }
    assert!(kinds.contains(&DiagramKind::Other("graphviz".into())));

    // Math appears both as `$$` and as a ```math fence.
    let math = doc
        .blocks()
        .iter()
        .filter(|b| b.kind == BlockKind::Math)
        .count();
    assert!(math >= 3, "expected several math blocks, got {math}");
}

#[test]
fn an_unregistered_technology_reports_info_not_error() {
    let doc = open("diagrams/diagrams.md");
    let graphviz = doc
        .diagnostics()
        .iter()
        .find(|d| d.source == "graphviz")
        .expect("graphviz diagnostic");
    assert_eq!(graphviz.severity, Severity::Info);
    // And the document still opened with all its other blocks.
    assert!(doc.outline().headings.len() > 5);
}

// --- MDX ------------------------------------------------------------------

#[test]
fn markdown_only_mdx_parses_as_markdown() {
    let doc = open("mdx/markdown-only.mdx");
    assert_eq!(doc.doc_type(), DocType::Mdx);
    assert!(doc.diagnostics().is_empty(), "{:?}", doc.diagnostics());
    assert_eq!(doc.outline().headings.len(), 2);
    assert!(
        doc.outline().structural.is_empty(),
        "no JSX means no structural entries"
    );
}

#[test]
fn mdx_fixture_identifies_every_construct() {
    use mt_doc::block::MdxKind;

    let doc = open("mdx/components.mdx");
    let kinds: Vec<_> = doc
        .blocks()
        .iter()
        .filter_map(|b| match b.kind {
            BlockKind::Mdx(k) => Some(k),
            _ => None,
        })
        .collect();

    assert!(kinds.contains(&MdxKind::EsmStatement), "imports/exports");
    assert!(kinds.contains(&MdxKind::JsxElement), "JSX elements");
    assert!(kinds.contains(&MdxKind::Expression), "expressions");

    // The outline works even though the file is mostly components.
    let labels: Vec<_> = doc
        .outline()
        .structural
        .iter()
        .map(|e| e.label.as_str())
        .collect();
    assert!(
        labels.iter().any(|l| l.contains("RevenueChart")),
        "got {labels:?}"
    );
    assert_eq!(doc.outline().headings.len(), 2);
}

#[test]
fn invalid_mdx_diagnoses_without_losing_content() {
    let path = fixtures().join("mdx/invalid.mdx");
    let source = std::fs::read_to_string(&path).unwrap();
    let doc = Document::new(Some(path), source.clone());

    assert!(
        doc.diagnostics().iter().any(|d| d.source == "mdx"),
        "expected an mdx diagnostic, got {:?}",
        doc.diagnostics()
    );
    assert_eq!(doc.source(), source, "source must survive a parse failure");
    assert!(!doc.blocks().is_empty(), "document must still be viewable");
}

#[test]
fn untrusted_mdx_opens_without_executing_anything() {
    // Parsing is pure: there is no JS engine in the document engine at all, so
    // opening executable content cannot run it. The trust boundary that matters
    // is the WebView's CSP, asserted in the web module's own tests.
    let doc = open("mdx/untrusted.mdx");
    assert_eq!(doc.doc_type(), DocType::Mdx);
    assert!(
        doc.source().contains("attacker.example"),
        "content is preserved verbatim, not sanitized on open"
    );
}

// --- Skills ---------------------------------------------------------------

fn discovered() -> Vec<mt_doc::Skill> {
    skill::discover(&fixtures().join("skills"))
}

#[test]
fn discovers_skills_across_every_convention() {
    let skills = discovered();
    let names: Vec<_> = skills.iter().map(|s| s.name.as_str()).collect();

    for expected in [
        "valid-skill",
        "nested-skill",
        "missing-metadata",
        "malformed-yaml",
        "agents-skill",
    ] {
        assert!(names.contains(&expected), "missing {expected} in {names:?}");
    }

    // Multiple discovery roots were searched. Compare full paths: the last
    // component is "skills" for every convention.
    let roots: std::collections::HashSet<_> = skills.iter().map(|s| s.root.clone()).collect();
    assert!(roots.len() >= 3, "expected several roots, got {roots:?}");
    let root_names: Vec<String> = roots
        .iter()
        .map(|r| crate_relative(r, &fixtures().join("skills")))
        .collect();
    for expected in ["skills", ".agents/skills", ".claude/skills"] {
        assert!(
            root_names.iter().any(|r| r == expected),
            "{expected} not among {root_names:?}"
        );
    }
}

#[test]
fn a_valid_skill_parses_every_spec_field() {
    let skills = discovered();
    // Two skills share this name. Select by discovery root, since
    // `.claude/skills/valid-skill` also ends in `skills/valid-skill`.
    let plain_root = fixtures().join("skills").join("skills");
    let skill = skills
        .iter()
        .find(|s| s.name == "valid-skill" && s.root == plain_root)
        .unwrap_or_else(|| {
            panic!(
                "no valid-skill under {}; roots seen: {:?}",
                plain_root.display(),
                skills.iter().map(|s| &s.root).collect::<Vec<_>>()
            )
        });

    assert!(skill.is_valid(), "diagnostics: {:?}", skill.diagnostics);
    assert_eq!(skill.meta.license.as_deref(), Some("Apache-2.0"));
    assert_eq!(skill.meta.allowed_tools, vec!["Read", "Write", "Bash"]);
    assert!(skill.meta.compatibility.is_some());
    assert_eq!(
        skill.meta.metadata.get("version").map(String::as_str),
        Some("1.0")
    );
}

#[test]
fn supporting_directories_are_detected() {
    let skills = discovered();
    let with_support = skills
        .iter()
        .find(|s| !s.support_dirs.is_empty())
        .expect("a skill with scripts/references/assets");
    let names: Vec<_> = with_support
        .support_dirs
        .iter()
        .map(|d| d.file_name().unwrap().to_string_lossy().into_owned())
        .collect();
    for expected in ["scripts", "references", "assets"] {
        assert!(names.contains(&expected.to_string()), "got {names:?}");
    }
}

#[test]
fn invalid_skills_are_reported_but_still_listed() {
    let skills = discovered();

    let missing = skills
        .iter()
        .find(|s| s.name == "missing-metadata")
        .expect("listed despite being invalid");
    assert!(!missing.is_valid());
    assert!(
        missing
            .diagnostics
            .iter()
            .any(|d| d.severity == Severity::Error && d.message.contains("description")),
        "{:?}",
        missing.diagnostics
    );

    let malformed = skills
        .iter()
        .find(|s| s.name == "malformed-yaml")
        .expect("a skill whose YAML does not parse still appears");
    assert!(!malformed.is_valid());
}

#[test]
fn a_directory_name_mismatch_is_a_warning_not_an_error() {
    let skills = discovered();
    // The effective name comes from frontmatter, so look it up by directory.
    let mismatched = skills
        .iter()
        .find(|s| s.dir.ends_with("name-mismatch"))
        .expect("name-mismatch skill");
    assert!(
        mismatched.is_valid(),
        "a mismatch must not make the skill unusable: {:?}",
        mismatched.diagnostics
    );
    assert!(
        mismatched
            .diagnostics
            .iter()
            .any(|d| d.message.contains("must match")),
        "{:?}",
        mismatched.diagnostics
    );
    assert_eq!(mismatched.name, "a-different-name");
}

#[test]
fn a_name_collision_across_roots_keeps_both() {
    let skills = discovered();
    let collisions: Vec<_> = skills.iter().filter(|s| s.name == "valid-skill").collect();
    assert_eq!(
        collisions.len(),
        2,
        "both roots' skills must remain available, each tagged with its root"
    );
    assert_ne!(collisions[0].root, collisions[1].root);
}

#[test]
fn discovery_does_not_descend_into_a_skill() {
    // A skill directory is a leaf. `valid-skill` has subdirectories; none may
    // be reported as skills of their own.
    let skills = discovered();
    assert!(
        !skills.iter().any(|s| s.name == "scripts" || s.name == "references"),
        "supporting directories must not be mistaken for skills"
    );
}

#[test]
fn skill_entry_documents_are_recognized_as_skills() {
    for skill in discovered() {
        assert_eq!(
            DocType::of(&skill.entry),
            DocType::Skill,
            "{} should be typed as a skill",
            skill.entry.display()
        );
    }
}
