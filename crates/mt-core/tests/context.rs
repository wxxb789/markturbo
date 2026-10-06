//! Deterministic offline Codex discovery against the frozen evidence snapshot.
//! Windows directory aliases use junctions, which require no symlink privilege.

use std::collections::BTreeMap;
use std::fs;
use std::path::Path;
#[cfg(windows)]
use std::path::PathBuf;

use mt_core::agent_artifacts::context::{
    CODEX_PROFILE_ID, CODEX_SOURCE_REVISION, ContextInput, ContextProfile, InclusionRule,
    ProjectTrust, ResolutionStatus, ResolvedContext, resolve,
};
use mt_core::agent_artifacts::skill::{Origin, Skill, SkillMeta};
use mt_core::diagnostic::Severity;
use serde::Deserialize;
use sha2::{Digest as _, Sha256};

#[derive(Deserialize)]
struct FixtureSpec {
    cwd: String,
    target: String,
    #[serde(default)]
    fallbacks: Vec<String>,
    files: BTreeMap<String, String>,
}

struct Fixture {
    temp: tempfile::TempDir,
    input: ContextInput,
}

impl Fixture {
    fn load(name: &str) -> Self {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/effective-context")
            .join(format!("{name}.json"));
        let spec: FixtureSpec = serde_json::from_slice(&fs::read(path).expect("fixture bytes"))
            .expect("machine-readable fixture");
        let temp = tempfile::tempdir().expect("isolated filesystem fixture");
        let workspace = temp.path().join("workspace");
        let home = temp.path().join("home");
        fs::create_dir_all(&workspace).expect("workspace");
        fs::create_dir_all(&home).expect("Codex home");
        for (path, text) in spec.files {
            write(&temp.path().join(path), text);
        }
        let cwd = if spec.cwd == "." {
            workspace.clone()
        } else {
            workspace.join(spec.cwd)
        };
        fs::create_dir_all(&cwd).expect("execution cwd");
        let input = ContextInput {
            target: workspace.join(spec.target),
            cwd,
            workspace,
            profile_id: CODEX_PROFILE_ID.to_string(),
            codex_home: home,
            codex_home_override: true,
            fallback_filenames: spec.fallbacks,
            project_doc_max_bytes: 32_768,
            project_root_markers: vec![".git".to_string()],
            project_trust: ProjectTrust::Trusted,
        };
        Self { temp, input }
    }

    fn resolve(&self) -> ResolvedContext {
        resolve(&self.input, &[])
    }
}

fn write(path: &Path, text: impl AsRef<[u8]>) {
    fs::create_dir_all(path.parent().expect("fixture parent")).expect("fixture directories");
    fs::write(path, text).expect("fixture file");
}

fn has_diagnostic(context: &ResolvedContext, source: &str) -> bool {
    context
        .diagnostics
        .iter()
        .any(|diagnostic| diagnostic.source == source)
}

fn directory_alias(target: &Path, link: &Path) {
    #[cfg(windows)]
    {
        // CMD's mklink parser needs native separators even though Windows
        // filesystem APIs accept the mixed spelling used by JSON fixtures.
        let target: PathBuf = target.components().collect();
        let link: PathBuf = link.components().collect();
        let output = std::process::Command::new("cmd")
            .args(["/d", "/c", "mklink", "/J"])
            .arg(&link)
            .arg(&target)
            .output()
            .expect("Windows junction command");
        assert!(
            output.status.success(),
            "junction creation failed: {:?}",
            output.status
        );
    }
    #[cfg(unix)]
    std::os::unix::fs::symlink(target, link).expect("directory symlink");
}

fn remove_directory_alias(link: &Path) {
    #[cfg(windows)]
    fs::remove_dir(link).expect("remove junction without its target");
    #[cfg(unix)]
    fs::remove_file(link).expect("remove symlink without its target");
}

#[test]
fn deep_root_nested_order_uses_cwd_not_target_ancestors() {
    let fixture = Fixture::load("deep");
    let context = fixture.resolve();
    assert_eq!(context.status, ResolutionStatus::Resolved);
    assert_eq!(context.project_root, Some(fixture.input.workspace.clone()));
    assert_eq!(
        context
            .sources
            .iter()
            .map(|source| &source.path)
            .collect::<Vec<_>>(),
        vec![
            &fixture.input.workspace.join("AGENTS.md"),
            &fixture.input.workspace.join("nested/AGENTS.md"),
        ]
    );
    assert_eq!(context.automatic_text(), "ROOT_SENTINEL\n\nNESTED_SENTINEL");
    assert_eq!(
        context.sources[1].scope,
        fixture.input.workspace.join("nested")
    );
    assert_eq!(
        context.sources[1].project_bytes_before,
        "ROOT_SENTINEL".len()
    );
    assert_eq!(context.input, fixture.input);
}

#[test]
fn sibling_instruction_is_excluded_even_when_target_is_in_sibling() {
    let fixture = Fixture::load("deep");
    let context = fixture.resolve();
    assert!(!context.sources.iter().any(|source| {
        source
            .path
            .starts_with(fixture.input.workspace.join("sibling"))
    }));
    assert!(!context.automatic_text().contains("SIBLING_SENTINEL"));
}

#[test]
fn outside_target_and_cwd_are_rejected_by_physical_membership() {
    let mut fixture = Fixture::load("outside");
    fixture.input.target = fixture.temp.path().join("outside.md");
    let context = fixture.resolve();
    assert_eq!(context.status, ResolutionStatus::InvalidInput);
    assert!(has_diagnostic(&context, "context.outside-workspace"));
    assert!(context.sources.is_empty());
    fixture.input.target = fixture.input.workspace.join("task.md");
    fixture.input.cwd = fixture.temp.path().to_path_buf();
    assert_eq!(fixture.resolve().status, ResolutionStatus::InvalidInput);
    let outside = fixture.temp.path().join("outside-directory");
    write(&outside.join("task.md"), "OUTSIDE_SENTINEL");
    let alias = fixture.input.workspace.join("external-alias");
    directory_alias(&outside, &alias);
    fixture.input.cwd = fixture.input.workspace.clone();
    fixture.input.target = alias.join("task.md");
    assert!(has_diagnostic(
        &fixture.resolve(),
        "context.outside-workspace"
    ));
    fixture.input.target = fixture.input.workspace.join("task.md");
    fixture.input.cwd = alias.clone();
    assert!(has_diagnostic(
        &fixture.resolve(),
        "context.outside-workspace"
    ));
    remove_directory_alias(&alias);
}

#[test]
fn workspace_alias_uses_physical_membership_without_rewriting_lexical_cwd() {
    let mut fixture = Fixture::load("deep");
    let workspace = fixture.input.workspace.clone();
    let alias = fixture.temp.path().join("workspace-alias");
    directory_alias(&workspace, &alias);
    fixture.input.workspace = alias.clone();
    let context = fixture.resolve();
    assert_eq!(context.status, ResolutionStatus::Resolved);
    assert_eq!(context.input.cwd, workspace.join("nested/deep"));
    assert_eq!(context.project_root, Some(workspace.clone()));
    assert_eq!(context.sources[0].scope, workspace);
    assert!(
        context
            .select_sources(&[0, 1])
            .expect("physical workspace alias selection")
            .revalidate()
            .is_ok()
    );
    remove_directory_alias(&alias);
}

#[test]
fn missing_removed_and_renamed_sources_invalidate_selection() {
    for rename in [false, true] {
        let fixture = Fixture::load("removed");
        let context = fixture.resolve();
        let selected = context.select_sources(&[0]).expect("named source");
        assert!(selected.revalidate().is_ok());
        let source = fixture.input.workspace.join("AGENTS.md");
        if rename {
            fs::rename(&source, source.with_file_name("RENAMED.md")).expect("rename instruction");
        } else {
            fs::remove_file(&source).expect("remove instruction");
        }
        assert!(selected.revalidate().is_err());
        let current = fixture.resolve();
        assert!(current.sources.is_empty());
        assert!(has_diagnostic(&current, "context.no-instructions"));
        assert_eq!(current.status, ResolutionStatus::Resolved);
    }
}

#[test]
fn multiple_discovery_aliases_preserve_occurrences_and_physical_identity() {
    let mut fixture = Fixture::load("aliases");
    let alias = fixture.temp.path().join("home-alias");
    directory_alias(&fixture.input.workspace, &alias);
    fixture.input.codex_home = alias.clone();
    let context = fixture.resolve();
    assert_eq!(context.status, ResolutionStatus::Resolved);
    assert_eq!(context.sources.len(), 2);
    assert_eq!(context.contents.len(), 1);
    assert_eq!(context.sources[0].path, alias.join("AGENTS.md"));
    assert_eq!(
        context.sources[1].path,
        fixture.input.workspace.join("AGENTS.md")
    );
    assert_eq!(
        context.sources[0].physical_path,
        context.sources[1].physical_path
    );
    assert_eq!(context.sources[1].project_bytes_before, 0);
    assert_eq!(context.sources[1].included_bytes, "SHARED_SENTINEL".len());
    assert_eq!(
        context.automatic_text(),
        "SHARED_SENTINEL\n\n--- project-doc ---\n\nSHARED_SENTINEL"
    );
    let selected = context
        .select_sources(&[0, 1])
        .expect("explicit alias selection");
    assert_eq!(selected.sources().len(), 2);
    assert_eq!(selected.contents().len(), 1);
    assert!(selected.revalidate().is_ok());
    remove_directory_alias(&alias);
    directory_alias(&fixture.temp.path().join("alternate"), &alias);
    assert!(selected.revalidate().is_err());
    remove_directory_alias(&alias);
}

#[test]
fn lexical_execution_cwd_is_preserved_through_directory_alias() {
    let mut fixture = Fixture::load("deep");
    let alias = fixture.input.workspace.join("execution-alias");
    directory_alias(&fixture.input.workspace.join("nested"), &alias);
    fixture.input.cwd = alias.join("deep");
    let context = fixture.resolve();
    assert_eq!(context.status, ResolutionStatus::Resolved);
    assert_eq!(context.input.cwd, fixture.input.cwd);
    assert_eq!(context.sources[1].path, alias.join("AGENTS.md"));
    assert_eq!(context.sources[1].scope, alias);
    assert_eq!(
        context.sources[1].physical_path,
        Some(
            fs::canonicalize(fixture.input.workspace.join("nested/AGENTS.md"))
                .expect("physical identity")
        )
    );
    assert!(
        context
            .select_sources(&[0, 1])
            .expect("lexical alias selection")
            .revalidate()
            .is_ok()
    );
    remove_directory_alias(&alias);
}

#[test]
fn global_lossy_utf8_tracks_raw_included_bytes_without_projection_failure() {
    let fixture = Fixture::load("removed");
    write(
        &fixture.input.codex_home.join("AGENTS.md"),
        [b' ', 0xff, b' '],
    );
    let context = fixture.resolve();
    assert_eq!(context.sources[0].raw_bytes, 3);
    assert_eq!(context.sources[0].included_bytes, 1);
    assert_eq!(context.contents[0].text, "\u{fffd}");
    assert!(
        context
            .select_sources(&[0])
            .expect("lossy global selection")
            .revalidate()
            .is_ok()
    );
}

#[test]
fn duplicate_content_and_directives_report_all_provenance_after_budget() {
    let mut fixture = Fixture::load("duplicates");
    let context = fixture.resolve();
    assert_eq!(context.sources.len(), 3);
    assert_eq!(context.contents.len(), 2);
    assert_eq!(
        context
            .sources
            .iter()
            .map(|source| &source.path)
            .collect::<Vec<_>>(),
        vec![
            &fixture.input.workspace.join("AGENTS.md"),
            &fixture.input.workspace.join("nested/AGENTS.md"),
            &fixture.input.workspace.join("nested/deep/AGENTS.md"),
        ]
    );
    assert_eq!(
        context.sources[0].content_index,
        context.sources[1].content_index
    );
    assert_eq!(
        context.sources[1].project_bytes_before,
        "DUP_SENTINEL".len()
    );
    assert_eq!(
        context.sources[2].project_bytes_before,
        2 * "DUP_SENTINEL".len()
    );
    assert!(has_diagnostic(&context, "context.duplicate-source"));
    let duplicates: Vec<_> = context
        .diagnostics
        .iter()
        .filter(|d| d.source == "context.duplicate-directive")
        .collect();
    assert_eq!(duplicates.len(), 2);
    assert!(duplicates.iter().all(|d| d.line == Some(1)));
    assert!(
        duplicates[0]
            .message
            .contains(&context.sources[1].path.display().to_string())
    );
    assert!(
        duplicates[1]
            .message
            .contains(&context.sources[2].path.display().to_string())
    );
    assert_eq!(context.diagnostics, fixture.resolve().diagnostics);
    fixture.input.project_doc_max_bytes = 2 * "DUP_SENTINEL".len();
    let budgeted = fixture.resolve();
    assert_eq!(budgeted.sources.len(), 2);
    assert_eq!(budgeted.contents.len(), 1);
    assert_eq!(
        budgeted
            .sources
            .iter()
            .map(|source| source.included_bytes)
            .sum::<usize>(),
        fixture.input.project_doc_max_bytes
    );
}

#[test]
fn malformed_metadata_import_and_scope_remain_opaque_with_diagnostics() {
    let fixture = Fixture::load("opaque");
    let context = fixture.resolve();
    assert_eq!(context.status, ResolutionStatus::Resolved);
    assert_eq!(context.sources[0].scope, fixture.input.workspace);
    assert!(context.automatic_text().contains("OPAQUE_SENTINEL"));
    for code in [
        "context.unsupported-metadata",
        "context.malformed-metadata",
        "context.unsupported-import",
        "context.unsupported-scope",
    ] {
        assert!(
            has_diagnostic(&context, code),
            "missing diagnostic code {code}"
        );
    }
    assert!(
        context
            .diagnostics
            .iter()
            .any(|d| d.source == "context.unsupported-scope" && d.line == Some(3))
    );
}

#[test]
fn global_override_and_project_precedence_use_frozen_separators() {
    let fixture = Fixture::load("precedence");
    let context = fixture.resolve();
    assert_eq!(
        context.sources.iter().map(|s| s.rule).collect::<Vec<_>>(),
        vec![
            InclusionRule::GlobalOverride,
            InclusionRule::ProjectOverride,
            InclusionRule::ProjectAgents,
        ]
    );
    assert_eq!(
        context.sources.iter().map(|s| s.origin).collect::<Vec<_>>(),
        vec![Origin::Global, Origin::Workspace, Origin::Workspace]
    );
    assert_eq!(
        context.automatic_text(),
        "GLOBAL_OVERRIDE_SENTINEL\n\n--- project-doc ---\n\nROOT_OVERRIDE_SENTINEL\n\nNESTED_SENTINEL"
    );
    assert_eq!(
        context.sources[0].included_bytes,
        "GLOBAL_OVERRIDE_SENTINEL".len()
    );
    assert_eq!(context.sources[1].project_bytes_before, 0);
}

#[test]
fn unsupported_local_and_remote_imports_are_not_followed() {
    let fixture = Fixture::load("import");
    let context = fixture.resolve();
    assert_eq!(context.sources.len(), 1);
    assert!(!context.automatic_text().contains("NOT_IMPORTED_SENTINEL"));
    assert_eq!(
        context
            .diagnostics
            .iter()
            .filter(|d| d.source == "context.unsupported-import")
            .map(|d| d.line)
            .collect::<Vec<_>>(),
        vec![Some(1), Some(2)]
    );
}

#[test]
fn platform_case_and_separators_follow_filesystem_identity() {
    let mut fixture = Fixture::load("platform");
    #[cfg(windows)]
    let workspace_is_alias = {
        fixture.input.workspace = PathBuf::from(
            fixture
                .input
                .workspace
                .to_string_lossy()
                .to_ascii_uppercase()
                .replace('\\', "/"),
        );
        fixture.input.target =
            PathBuf::from(fixture.input.target.to_string_lossy().replace('\\', "/"));
        true
    };
    #[cfg(not(windows))]
    let workspace_is_alias = {
        let alternate = fixture.temp.path().join("WORKSPACE");
        let is_alias = match fs::create_dir(&alternate) {
            Ok(()) => false,
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => true,
            Err(error) => panic!("case directory probe failed: {error}"),
        };
        fixture.input.workspace = alternate;
        is_alias
    };

    let context = fixture.resolve();
    if workspace_is_alias {
        assert_eq!(context.status, ResolutionStatus::Resolved);
        assert_eq!(context.input, fixture.input);
        assert_eq!(
            context.sources[0].scope,
            fixture.input.cwd.parent().expect("lexical root")
        );
        assert!(
            context
                .select_sources(&[0])
                .expect("case alias selection")
                .revalidate()
                .is_ok()
        );
    } else {
        // A distinct, differently cased existing directory is not an alias.
        assert_eq!(context.status, ResolutionStatus::InvalidInput);
        assert!(has_diagnostic(&context, "context.outside-workspace"));
    }
}

#[test]
fn empty_project_override_suppresses_fallback_but_empty_global_override_does_not() {
    let fixture = Fixture::load("fallback");
    let context = fixture.resolve();
    assert_eq!(context.sources.len(), 3);
    assert_eq!(context.sources[0].rule, InclusionRule::GlobalAgents);
    assert_eq!(context.sources[1].rule, InclusionRule::ProjectOverride);
    assert_eq!(context.sources[1].raw_bytes, 3);
    assert_eq!(context.sources[1].included_bytes, 0);
    assert_eq!(context.sources[1].content_index, None);
    assert_eq!(context.sources[2].rule, InclusionRule::ProjectFallback);
    assert_eq!(
        context.sources[2].path,
        fixture.input.workspace.join("nested/TEAM.md")
    );
    assert_eq!(context.sources[2].project_bytes_before, 0);
    assert_eq!(
        context.automatic_text(),
        "GLOBAL_FALLBACK_SENTINEL\n\n--- project-doc ---\n\nTEAM_SENTINEL"
    );
}

#[test]
fn whitespace_normalized_nested_fallback_resolves_selects_and_round_trips() {
    let fixture = Fixture::load("nested-fallback");
    let context = fixture.resolve();
    assert_eq!(context.status, ResolutionStatus::Resolved);
    assert_eq!(context.sources.len(), 2);
    assert!(
        context
            .sources
            .iter()
            .all(|source| source.rule == InclusionRule::ProjectFallback)
    );
    assert_eq!(context.sources[0].scope, fixture.input.workspace);
    assert_eq!(
        context.sources[1].scope,
        fixture.input.workspace.join("nested")
    );
    for source in &context.sources {
        assert_eq!(source.path, source.scope.join("docs/TEAM.md"));
        assert_ne!(source.path.parent(), Some(source.scope.as_path()));
    }
    let selected = context
        .select_sources(&[0, 1])
        .expect("nested fallback sources are selectable");
    let wire = serde_json::to_vec(&selected).expect("local selected fallback wire");
    let decoded: mt_core::agent_artifacts::context::SelectedContext =
        serde_json::from_slice(&wire).expect("nested fallback decode");
    assert_eq!(decoded, selected);
    assert!(decoded.revalidate().is_ok());
}

#[test]
fn raw_byte_truncation_is_lossy_and_global_and_separators_do_not_consume_budget() {
    let mut fixture = Fixture::load("budget");
    fixture.input.project_doc_max_bytes = 2;
    let context = fixture.resolve();
    assert_eq!(context.sources.len(), 2);
    assert_eq!(context.sources[1].included_bytes, 2);
    assert_eq!(context.sources[1].raw_bytes, "\u{20ac}ROOT_SENTINEL".len());
    assert!(context.sources[1].is_truncated());
    assert_eq!(
        context.contents[context.sources[1].content_index.expect("truncated content")].text,
        "\u{fffd}"
    );
    assert_eq!(context.sources[0].included_bytes, "GLOBAL_SENTINEL".len());
    assert_eq!(
        context.automatic_text(),
        "GLOBAL_SENTINEL\n\n--- project-doc ---\n\n\u{fffd}"
    );
    let selected = context.select_sources(&[1]).expect("truncated source");
    write(
        &fixture.input.workspace.join("AGENTS.md"),
        "\u{20ac}CHANGED_SUFFIX_SENTINEL",
    );
    assert!(selected.revalidate().is_err());
}

#[test]
fn untrusted_and_zero_budget_retain_global_but_never_skill_bodies() {
    for untrusted in [false, true] {
        let mut fixture = Fixture::load("budget");
        if untrusted {
            fixture.input.project_trust = ProjectTrust::Untrusted;
        } else {
            fixture.input.project_doc_max_bytes = 0;
        }
        let dir = fixture.input.workspace.join(".agents/skills/demo");
        let skill = Skill {
            entry: dir.join("SKILL.md"),
            root: dir.parent().expect("catalog root").to_path_buf(),
            dir,
            origin: Origin::Workspace,
            aliases: Vec::new(),
            name: "demo".to_string(),
            meta: SkillMeta::default(),
            diagnostics: Vec::new(),
            support_dirs: Vec::new(),
        };
        let context = resolve(&fixture.input, std::slice::from_ref(&skill));
        assert_eq!(context.status, ResolutionStatus::Resolved);
        assert_eq!(context.sources.len(), 1);
        assert_eq!(context.automatic_text(), "GLOBAL_SENTINEL");
        assert_eq!(context.available_skills, vec![skill]);
        assert!(
            context
                .select_sources(&[0])
                .expect("global selection")
                .revalidate()
                .is_ok()
        );
    }
}

#[test]
fn skill_catalog_alone_never_includes_bodies_but_explicit_filename_fallback_does() {
    for filename in ["SKILL.md", "skill.md"] {
        let mut fixture = Fixture::load("removed");
        let entry = fixture.input.cwd.join(filename);
        write(&entry, "SKILL_BODY_SENTINEL");
        let skill = Skill {
            dir: fixture.input.cwd.clone(),
            entry: entry.clone(),
            root: fixture.input.workspace.clone(),
            origin: Origin::Workspace,
            aliases: Vec::new(),
            name: "demo".to_string(),
            meta: SkillMeta::default(),
            diagnostics: Vec::new(),
            support_dirs: Vec::new(),
        };
        let context = resolve(&fixture.input, std::slice::from_ref(&skill));
        assert_eq!(context.available_skills, vec![skill.clone()]);
        assert!(!context.automatic_text().contains("SKILL_BODY_SENTINEL"));
        fs::remove_file(fixture.input.workspace.join("AGENTS.md"))
            .expect("remove built-in candidate");
        fixture.input.fallback_filenames = vec![filename.to_string()];
        let fallback = resolve(&fixture.input, std::slice::from_ref(&skill));
        assert_eq!(fallback.status, ResolutionStatus::Resolved);
        assert_eq!(fallback.sources.len(), 1);
        assert_eq!(fallback.sources[0].rule, InclusionRule::ProjectFallback);
        assert_eq!(fallback.sources[0].path, entry);
        assert_eq!(fallback.automatic_text(), "SKILL_BODY_SENTINEL");
        assert_eq!(fallback.available_skills, vec![skill]);
        assert!(
            fallback
                .select_sources(&[0])
                .expect("explicit instruction fallback selection")
                .revalidate()
                .is_ok()
        );
    }
}

#[test]
fn no_marker_empty_markers_and_nearest_marker_use_execution_cwd() {
    let mut fixture = Fixture::load("deep");
    fs::remove_file(fixture.input.workspace.join(".git")).expect("remove root marker");
    assert_eq!(
        fixture.resolve().project_root,
        Some(fixture.input.cwd.clone())
    );
    assert!(fixture.resolve().sources.is_empty());
    write(&fixture.input.workspace.join(".git"), "MARKER_SENTINEL");
    fixture.input.project_root_markers.clear();
    assert_eq!(
        fixture.resolve().project_root,
        Some(fixture.input.cwd.clone())
    );
    fixture.input.project_root_markers.push(".git".to_string());
    write(
        &fixture.input.workspace.join("nested/.git"),
        "NEAREST_MARKER_SENTINEL",
    );
    let context = fixture.resolve();
    assert_eq!(
        context.project_root,
        Some(fixture.input.workspace.join("nested"))
    );
    assert_eq!(context.sources.len(), 1);
}

#[test]
fn no_marker_watches_probed_ancestors_until_a_new_marker_expands_the_chain() {
    let mut fixture = Fixture::load("deep");
    // A fixture-specific name makes this independent of markers above the
    // platform temp directory, without changing the process's filesystem.
    let marker = format!(
        ".context-root-{}",
        fixture
            .temp
            .path()
            .file_name()
            .expect("unique fixture name")
            .to_string_lossy()
    );
    fixture.input.project_root_markers = vec![marker.clone()];
    fixture.input.fallback_filenames = vec!["docs/TEAM.md".to_string()];
    fs::create_dir(fixture.input.workspace.join("docs")).expect("ancestor fallback directory");
    write(&fixture.input.cwd.join("AGENTS.md"), "CWD_SENTINEL");
    let context = fixture.resolve();
    assert_eq!(context.project_root, Some(fixture.input.cwd.clone()));
    assert_eq!(context.automatic_text(), "CWD_SENTINEL");
    let selected = context.select_sources(&[0]).expect("cwd-only selection");
    let paths = context.watch_paths();
    let directories = context.watch_directories();
    for ancestor in fixture.input.cwd.ancestors() {
        assert!(paths.contains(&ancestor.join(&marker)));
        assert!(directories.contains(&ancestor.to_path_buf()));
    }
    assert!(!paths.contains(&fixture.input.workspace.join("AGENTS.md")));
    assert!(!paths.contains(&fixture.input.workspace.join("docs/TEAM.md")));
    assert!(!directories.contains(&fixture.input.workspace.join("docs")));

    let new_marker = fixture.input.workspace.join(&marker);
    write(&new_marker, "NEW_MARKER_SENTINEL");
    assert!(
        paths.contains(&new_marker),
        "marker creation must be an exact context event"
    );
    assert!(selected.revalidate().is_err());
    let current = fixture.resolve();
    assert_eq!(current.project_root, Some(fixture.input.workspace.clone()));
    assert_eq!(
        current.automatic_text(),
        "ROOT_SENTINEL\n\nNESTED_SENTINEL\n\nCWD_SENTINEL"
    );
    assert!(
        !current
            .watch_paths()
            .contains(&fixture.temp.path().join(&marker))
    );
    assert!(
        !current
            .watch_directories()
            .contains(&fixture.temp.path().to_path_buf())
    );
}

#[test]
fn empty_marker_configuration_adds_no_ancestor_marker_or_instruction_watches() {
    let mut fixture = Fixture::load("deep");
    fixture.input.project_root_markers.clear();
    let context = fixture.resolve();
    let paths = context.watch_paths();
    assert!(
        fixture
            .input
            .cwd
            .ancestors()
            .all(|ancestor| !paths.contains(&ancestor.join(".git")))
    );
    assert!(!paths.contains(&fixture.input.workspace.join("AGENTS.md")));
    assert!(
        !context
            .watch_directories()
            .contains(&fixture.temp.path().to_path_buf())
    );
}

#[test]
fn discovery_candidates_content_and_root_changes_invalidate_same_input_selection() {
    for change in ["override", "fallback", "root", "content", "global"] {
        let mut fixture = Fixture::load("removed");
        fixture.input.fallback_filenames = vec!["TEAM.md".to_string()];
        let selected = fixture
            .resolve()
            .select_sources(&[0])
            .expect("frozen selection");
        assert!(selected.revalidate().is_ok());
        let path = match change {
            "override" => fixture.input.workspace.join("AGENTS.override.md"),
            "fallback" => fixture.input.cwd.join("TEAM.md"),
            "root" => fixture.input.cwd.join(".git"),
            "content" => fixture.input.workspace.join("AGENTS.md"),
            "global" => fixture.input.codex_home.join("AGENTS.md"),
            _ => unreachable!("fixed fixture changes"),
        };
        write(&path, "CHANGED_SENTINEL");
        assert!(
            selected.revalidate().is_err(),
            "change not detected: {change}"
        );
    }
}

#[test]
fn discovery_watches_stop_at_root_and_include_global_and_fallback_parents() {
    let mut fixture = Fixture::load("deep");
    fixture.input.fallback_filenames = vec!["docs/TEAM.md".to_string()];
    fs::create_dir(fixture.input.workspace.join("docs")).expect("fallback directory");
    let watches = fixture.resolve().watch_directories();
    for path in [
        &fixture.input.cwd,
        &fixture.input.workspace,
        &fixture.input.codex_home,
    ] {
        assert!(
            watches.contains(&path.to_path_buf()),
            "missing watch directory {}",
            path.display()
        );
    }
    assert!(watches.contains(&fixture.input.workspace.join("docs")));
    assert!(!watches.contains(&fixture.temp.path().to_path_buf()));
    assert!(watches.windows(2).all(|pair| pair[0] < pair[1]));
}

#[test]
fn exact_watch_paths_include_missing_candidates_and_only_nearest_root_markers() {
    let mut fixture = Fixture::load("deep");
    fixture.input.fallback_filenames = vec![" docs/TEAM.md ".to_string()];
    let context = fixture.resolve();
    let paths = context.watch_paths();
    for directory in [
        &fixture.input.workspace,
        &fixture.input.workspace.join("nested"),
        &fixture.input.cwd,
    ] {
        assert!(paths.contains(&directory.join(".git")));
        assert!(paths.contains(&directory.join("AGENTS.override.md")));
    }
    assert!(paths.contains(&fixture.input.cwd.join("AGENTS.md")));
    assert!(paths.contains(&fixture.input.cwd.join("docs/TEAM.md")));
    assert!(paths.contains(&fixture.input.cwd.join("docs")));
    assert!(paths.contains(&fixture.input.codex_home.join("AGENTS.override.md")));
    assert!(paths.contains(&fixture.input.codex_home.join("AGENTS.md")));
    assert!(!paths.contains(&fixture.temp.path().join(".git")));
    assert!(!paths.contains(&fixture.input.workspace.join(".git/HEAD")));
    assert!(!paths.contains(&fixture.input.workspace.join("sibling/AGENTS.md")));
    assert!(!paths.contains(&fixture.input.workspace.join("docs/TEAM.md")));
    assert!(paths.windows(2).all(|pair| pair[0] < pair[1]));
    assert_eq!(paths, context.watch_paths());
}

#[test]
fn exact_watch_paths_preserve_aliases_and_exclude_shadowed_candidates() {
    let mut fixture = Fixture::load("aliases");
    let alias = fixture.temp.path().join("watch-home-alias");
    directory_alias(&fixture.input.workspace, &alias);
    fixture.input.codex_home = alias.clone();
    let paths = fixture.resolve().watch_paths();
    assert!(paths.contains(&alias));
    assert!(paths.contains(&alias.join("AGENTS.md")));
    assert!(paths.contains(&fixture.input.workspace.join("AGENTS.md")));
    assert!(
        paths.contains(
            &fs::canonicalize(fixture.input.workspace.join("AGENTS.md"))
                .expect("physical instruction identity")
        )
    );
    remove_directory_alias(&alias);

    let mut fixture = Fixture::load("precedence");
    fixture.input.fallback_filenames = vec!["TEAM.md".to_string()];
    let paths = fixture.resolve().watch_paths();
    assert!(paths.contains(&fixture.input.codex_home.join("AGENTS.override.md")));
    assert!(paths.contains(&fixture.input.workspace.join("AGENTS.override.md")));
    assert!(!paths.contains(&fixture.input.codex_home.join("AGENTS.md")));
    assert!(!paths.contains(&fixture.input.workspace.join("AGENTS.md")));
    assert!(!paths.contains(&fixture.input.workspace.join("TEAM.md")));
}

#[test]
fn markers_above_nearest_root_do_not_change_discovery_or_watches() {
    let fixture = Fixture::load("deep");
    let context = fixture.resolve();
    let selected = context
        .select_sources(&[0, 1])
        .expect("nearest-root selection");
    write(&fixture.temp.path().join(".git"), "DISTANT_MARKER_SENTINEL");
    let current = fixture.resolve();
    assert_eq!(current.project_root, context.project_root);
    assert_eq!(current.discovery_digest, context.discovery_digest);
    assert_eq!(current.watch_directories(), context.watch_directories());
    assert!(selected.revalidate().is_ok());
    fs::remove_file(fixture.input.workspace.join(".git")).expect("remove nearest marker");
    assert!(selected.revalidate().is_err());
}

#[test]
fn marker_metadata_errors_warn_and_continue_without_discarding_project_sources() {
    let mut fixture = Fixture::load("deep");
    let cycle = fixture.input.cwd.join("marker-cycle");
    directory_alias(&cycle, &cycle);
    fixture.input.project_root_markers =
        vec!["marker-cycle/marker".to_string(), ".git".to_string()];
    let context = fixture.resolve();
    assert_eq!(context.status, ResolutionStatus::Resolved);
    assert_eq!(context.project_root, Some(fixture.input.workspace.clone()));
    assert_eq!(context.sources.len(), 2);
    assert!(
        context
            .sources
            .iter()
            .all(|source| source.content_index.is_some())
    );
    assert!(
        context
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.source == "context.cyclic-discovery"
                && diagnostic.severity == Severity::Warning)
    );
    assert!(!has_diagnostic(&context, "context.project-unavailable"));
    assert!(
        context
            .select_sources(&[0, 1])
            .expect("chain after ignored marker error")
            .revalidate()
            .is_ok()
    );
    remove_directory_alias(&cycle);
}

#[test]
fn root_marker_strings_are_verbatim_including_empty_and_parent_segments() {
    for marker in ["", "../marker-sentinel"] {
        let mut fixture = Fixture::load("deep");
        if !marker.is_empty() {
            write(
                &fixture
                    .input
                    .cwd
                    .parent()
                    .expect("marker parent")
                    .join("marker-sentinel"),
                "VERBATIM_MARKER_SENTINEL",
            );
        }
        fixture.input.project_root_markers = vec![marker.to_string()];
        let context = fixture.resolve();
        assert_eq!(context.status, ResolutionStatus::Resolved);
        assert_eq!(context.project_root, Some(fixture.input.cwd.clone()));
        assert!(!has_diagnostic(
            &context,
            "context.unsupported-configuration"
        ));
        if marker.is_empty() {
            assert!(
                !context.watch_directories().contains(
                    &fixture
                        .input
                        .cwd
                        .parent()
                        .expect("cwd parent")
                        .to_path_buf()
                )
            );
        }
    }
    let mut fixture = Fixture::load("deep");
    let marker_dir = fixture.input.workspace.join("nested/ marker-dir");
    write(&marker_dir.join(".git"), "VERBATIM_MARKER_SENTINEL");
    fixture.input.project_root_markers = vec![" marker-dir/.git".to_string()];
    let context = fixture.resolve();
    assert_eq!(
        context.project_root,
        Some(fixture.input.workspace.join("nested"))
    );
    assert_eq!(context.sources.len(), 1);
    assert!(context.watch_directories().contains(&marker_dir));
}

#[test]
fn shadowed_content_changes_do_not_change_discovery_digest_or_selection() {
    let mut fixture = Fixture::load("precedence");
    fixture.input.fallback_filenames = vec!["TEAM.md".to_string()];
    let context = fixture.resolve();
    let selected = context
        .select_sources(&[0, 1])
        .expect("winning sources only");
    for path in [
        fixture.input.codex_home.join("AGENTS.md"),
        fixture.input.workspace.join("AGENTS.md"),
        fixture.input.workspace.join("TEAM.md"),
    ] {
        write(&path, "SHADOWED_BODY_SENTINEL");
    }
    let current = fixture.resolve();
    assert_eq!(current.status, ResolutionStatus::Resolved);
    assert_eq!(current.discovery_digest, context.discovery_digest);
    assert!(!current.automatic_text().contains("SHADOWED_BODY_SENTINEL"));
    assert!(selected.revalidate().is_ok());
}

#[cfg(windows)]
#[test]
fn unreadable_shadowed_global_and_project_files_are_not_probed_or_read() {
    use std::os::windows::fs::OpenOptionsExt;

    let mut fixture = Fixture::load("precedence");
    fixture.input.fallback_filenames = vec!["TEAM.md".to_string()];
    let fallback = fixture.input.workspace.join("TEAM.md");
    write(&fallback, "SHADOWED_FALLBACK_SENTINEL");
    let context = fixture.resolve();
    let selected = context.select_sources(&[0, 1]).expect("winning sources");
    let _locks: Vec<_> = [
        fixture.input.codex_home.join("AGENTS.md"),
        fixture.input.workspace.join("AGENTS.md"),
        fallback,
    ]
    .into_iter()
    .map(|path| {
        fs::OpenOptions::new()
            .read(true)
            .share_mode(0)
            .open(path)
            .expect("exclusive shadowed file lock")
    })
    .collect();
    let current = fixture.resolve();
    assert_eq!(current.status, ResolutionStatus::Resolved);
    assert_eq!(current.discovery_digest, context.discovery_digest);
    assert!(!has_diagnostic(&current, "context.global-read"));
    assert!(!has_diagnostic(&current, "context.project-read"));
    assert!(selected.revalidate().is_ok());
}

#[test]
fn local_discovery_digest_round_trips_without_entering_assumptions() {
    let fixture = Fixture::load("deep");
    let context = fixture.resolve();
    let selected = context
        .select_sources(&[0])
        .expect("local named-source selection");
    assert!(context.discovery_digest.is_some());
    assert_eq!(selected.discovery_digest(), context.discovery_digest);
    let wire = serde_json::to_value(&selected).expect("local selection wire");
    assert_eq!(
        wire["discovery_digest"],
        serde_json::to_value(context.discovery_digest).expect("local digest wire")
    );
    let snapshot: serde_json::Value = serde_json::from_slice(include_bytes!(
        "../assets/context-profiles/codex-agents-md-2026-08-29.json"
    ))
    .expect("frozen assumptions");
    assert_eq!(wire["assumptions"], snapshot["assumptions"]);
    let decoded: mt_core::agent_artifacts::context::SelectedContext =
        serde_json::from_value(wire).expect("local selection decode");
    assert_eq!(decoded, selected);
    assert!(decoded.revalidate().is_ok());
}

#[test]
fn broken_directory_alias_candidates_are_missing_and_fallback_applies() {
    let mut fixture = Fixture::load("removed");
    fs::remove_file(fixture.input.workspace.join("AGENTS.md")).expect("remove built-in candidate");
    let target = fixture.temp.path().join("removed-alias-target");
    write(&target.join("AGENTS.md"), "REMOVED_TARGET_SENTINEL");
    let alias = fixture.input.cwd.join("broken");
    directory_alias(&target, &alias);
    fs::remove_file(target.join("AGENTS.md")).expect("remove alias target source");
    fs::remove_dir(&target).expect("leave a genuinely broken alias");
    fixture.input.fallback_filenames = vec!["broken/AGENTS.md".to_string(), "TEAM.md".to_string()];
    write(&fixture.input.cwd.join("TEAM.md"), "FALLBACK_SENTINEL");
    let context = fixture.resolve();
    assert_eq!(context.status, ResolutionStatus::Resolved);
    assert_eq!(context.sources.len(), 1);
    assert_eq!(context.sources[0].rule, InclusionRule::ProjectFallback);
    assert_eq!(context.sources[0].path, fixture.input.cwd.join("TEAM.md"));
    assert!(!has_diagnostic(&context, "context.project-read"));
    assert!(!has_diagnostic(&context, "context.cyclic-discovery"));
    assert_eq!(context.automatic_text(), "FALLBACK_SENTINEL");
    assert!(
        context
            .select_sources(&[0])
            .expect("fallback after missing alias")
            .revalidate()
            .is_ok()
    );
    remove_directory_alias(&alias);
}

#[test]
fn unsupported_profile_configuration_and_explicit_invalid_home_are_diagnosed() {
    let mut fixture = Fixture::load("removed");
    fixture.input.profile_id = "other-harness".to_string();
    assert_eq!(fixture.resolve().status, ResolutionStatus::InventoryOnly);
    fixture.input.profile_id = CODEX_PROFILE_ID.to_string();
    fixture.input.codex_home = fixture.temp.path().join("nonexistent-home");
    assert_eq!(fixture.resolve().status, ResolutionStatus::InvalidInput);
    fixture.input.codex_home = fixture.temp.path().join("home");
    fixture.input.fallback_filenames = vec!["../outside.md".to_string()];
    assert!(has_diagnostic(
        &fixture.resolve(),
        "context.unsupported-configuration"
    ));
    fixture.input.fallback_filenames.clear();
    for claim in ["layered_config", "sandbox", "environments", "plugins"] {
        let mut value = serde_json::to_value(&fixture.input).expect("input wire");
        value[claim] = serde_json::json!(true);
        assert!(serde_json::from_value::<ContextInput>(value).is_err());
    }
}

#[cfg(windows)]
#[test]
fn explicit_remote_inputs_and_references_are_rejected_before_filesystem_probes() {
    // Embedded NUL keeps UNC and redirector cases offline even if the guard
    // regresses: Windows cannot open these names. The diagnostic code must come
    // from the remote guard, not from the filesystem's invalid-name error.
    for remote in [
        "https://offline.invalid/AGENTS.md",
        "smb://offline.invalid/share/AGENTS.md",
        "file://offline.invalid/share/AGENTS.md",
        "\\\\offline\0\\share\\AGENTS.md",
        "//offline\0/share/AGENTS.md",
        "\\\\?\\UNC\\offline\0\\share\\AGENTS.md",
        "//?/UNC/offline\0/share/AGENTS.md",
        "\\\\?\\unc\\offline\0\\share\\AGENTS.md",
        "\\\\?\\GLOBALROOT\\Device\\Mup\\offline\0\\share\\AGENTS.md",
        "\\\\.\\GLOBALROOT\\Device\\Mup\\offline\0\\share\\AGENTS.md",
        "//?/GLOBALROOT/Device/Mup/offline\0/share/AGENTS.md",
        "//./GLOBALROOT/Device/Mup/offline\0/share/AGENTS.md",
        "\\\\?\\globalroot\\device\\mup\\offline\0\\share\\AGENTS.md",
        "\\\\.\\globalroot\\device\\mup\\offline\0\\share\\AGENTS.md",
        "\\\\?\\GlObAlRoOt/DeViCe\\MuP/offline\0\\share/AGENTS.md",
        "//./GlObAlRoOt\\DeViCe/MuP\\offline\0/share\\AGENTS.md",
        "\\\\?\\GLOBALROOT\\Device\\LanmanRedirector\\offline\0\\share\\AGENTS.md",
        "//./globalroot/device/lanmanredirector/offline\0/share/AGENTS.md",
        "\\\\?\\GLOBALROOT\\Device\\WebDavRedirector\\offline\0\\share\\AGENTS.md",
        "//./globalroot/device/webdavredirector/offline\0/share/AGENTS.md",
    ] {
        for field in ["workspace", "target", "cwd", "home", "fallback", "marker"] {
            let mut fixture = Fixture::load("removed");
            // Even later references must be rejected before an earlier local
            // metadata failure; no resolution or watcher I/O is needed.
            fixture.input.workspace = fixture.temp.path().join("missing-workspace");
            fixture.input.codex_home = fixture.temp.path().join("missing-home");
            match field {
                "workspace" => fixture.input.workspace = PathBuf::from(remote),
                "target" => fixture.input.target = PathBuf::from(remote),
                "cwd" => fixture.input.cwd = PathBuf::from(remote),
                "home" => fixture.input.codex_home = PathBuf::from(remote),
                "fallback" => fixture.input.fallback_filenames = vec![format!(" {remote} ")],
                "marker" => fixture.input.project_root_markers = vec![remote.to_string()],
                _ => unreachable!("fixed input fields"),
            }
            let context = fixture.resolve();
            assert_eq!(context.status, ResolutionStatus::InvalidInput);
            assert_eq!(context.diagnostics.len(), 1);
            assert_eq!(
                context.diagnostics[0].source, "context.unsupported-remote",
                "{field}: {remote:?}"
            );
            assert_eq!(context.diagnostics[0].severity, Severity::Error);
            assert!(context.sources.is_empty());
            assert!(context.contents.is_empty());
            assert!(context.discovery_digest.is_none());
            assert!(context.watch_paths().is_empty());
            assert!(context.watch_directories().is_empty());
        }
    }
}

#[cfg(windows)]
#[test]
fn verbatim_local_drive_inputs_preserve_discovery_and_revalidation() {
    let mut fixture = Fixture::load("deep");
    fixture.input.workspace = fs::canonicalize(&fixture.input.workspace).expect("local workspace");
    fixture.input.target = fs::canonicalize(&fixture.input.target).expect("local target");
    fixture.input.cwd = fs::canonicalize(&fixture.input.cwd).expect("local cwd");
    fixture.input.codex_home = fs::canonicalize(&fixture.input.codex_home).expect("local home");
    let context = fixture.resolve();
    assert_eq!(context.status, ResolutionStatus::Resolved);
    assert_eq!(context.input, fixture.input);
    assert_eq!(context.sources.len(), 2);
    assert_eq!(context.automatic_text(), "ROOT_SENTINEL\n\nNESTED_SENTINEL");
    assert!(context.watch_paths().contains(&fixture.input.cwd));
    assert!(
        context
            .watch_directories()
            .contains(&fixture.input.workspace)
    );
    assert!(
        context
            .select_sources(&[0, 1])
            .expect("local drive selection")
            .revalidate()
            .is_ok()
    );
}

#[cfg(unix)]
#[test]
fn known_remote_link_targets_are_not_followed_for_inputs_candidates_or_markers() {
    let mut fixture = Fixture::load("removed");
    let alias = fixture.input.cwd.join("remote-alias");
    std::os::unix::fs::symlink(r"\\offline.invalid\share", &alias)
        .expect("offline remote spelling");
    fixture.input.fallback_filenames = vec!["remote-alias/AGENTS.md".to_string()];
    let context = fixture.resolve();
    assert_eq!(context.status, ResolutionStatus::Partial);
    assert!(has_diagnostic(&context, "context.unsupported-remote"));
    assert!(has_diagnostic(&context, "context.project-unavailable"));
    assert!(context.contents.is_empty());
    assert!(
        !context
            .watch_paths()
            .iter()
            .any(|path| path.starts_with(r"\\offline.invalid\share"))
    );

    fixture.input.fallback_filenames.clear();
    fixture.input.project_root_markers =
        vec!["remote-alias/marker".to_string(), ".git".to_string()];
    let context = fixture.resolve();
    assert_eq!(context.status, ResolutionStatus::Resolved);
    assert_eq!(context.sources.len(), 1);
    assert!(context.diagnostics.iter().any(|diagnostic| {
        diagnostic.source == "context.unsupported-remote"
            && diagnostic.severity == Severity::Warning
    }));
    assert!(
        context
            .select_sources(&[0])
            .expect("local source after rejected marker")
            .revalidate()
            .is_ok()
    );

    fixture.input.cwd = alias;
    let context = fixture.resolve();
    assert_eq!(context.status, ResolutionStatus::InvalidInput);
    assert!(has_diagnostic(&context, "context.unsupported-remote"));
    assert!(context.sources.is_empty());
}

#[cfg(unix)]
#[test]
fn chained_remote_links_and_target_ancestors_are_rejected_offline() {
    use std::os::unix::fs::symlink;

    for kind in ["file", "directory", "target-ancestor"] {
        let mut fixture = Fixture::load("removed");
        let remote = fixture.input.cwd.join("remote-alias");
        let local = fixture.input.cwd.join("local-alias");
        // Backslashes are ordinary filename bytes on Unix, so even a broken
        // guard cannot reach a network filesystem through this fixture.
        symlink(r"\\offline.invalid\share", &remote).expect("offline remote spelling");
        let target = if kind == "target-ancestor" {
            "remote-alias/child"
        } else {
            "remote-alias"
        };
        symlink(target, &local).expect("relative local-looking hop");
        if kind == "file" {
            symlink("local-alias", fixture.input.cwd.join("AGENTS.md"))
                .expect("built-in instruction link");
        } else {
            fixture.input.fallback_filenames = vec!["local-alias/AGENTS.md".to_string()];
        }
        let context = fixture.resolve();
        assert_eq!(context.status, ResolutionStatus::Partial, "{kind}");
        assert!(
            context.diagnostics.iter().any(|diagnostic| {
                diagnostic.source == "context.unsupported-remote"
                    && diagnostic.severity == Severity::Error
            }),
            "{kind}"
        );
        assert!(has_diagnostic(&context, "context.project-unavailable"));
        assert!(context.contents.is_empty());
        assert!(
            !context
                .watch_paths()
                .iter()
                .any(|path| path.starts_with(r"\\offline.invalid\share"))
        );
        assert!(
            !context
                .watch_directories()
                .iter()
                .any(|path| path.starts_with(r"\\offline.invalid\share"))
        );

        if kind == "file" {
            fs::remove_file(fixture.input.cwd.join("AGENTS.md")).expect("remove instruction link");
        }
        fixture.input.fallback_filenames.clear();
        fixture.input.project_root_markers =
            vec!["local-alias/marker".to_string(), ".git".to_string()];
        let context = fixture.resolve();
        assert_eq!(context.status, ResolutionStatus::Resolved);
        assert!(context.diagnostics.iter().any(|diagnostic| {
            diagnostic.source == "context.unsupported-remote"
                && diagnostic.severity == Severity::Warning
        }));
        assert!(
            context
                .select_sources(&[0])
                .expect("local source after remote marker")
                .revalidate()
                .is_ok()
        );

        fixture.input.project_root_markers = vec![".git".to_string()];
        symlink(&local, fixture.input.codex_home.join("AGENTS.md")).expect("global linked source");
        let context = fixture.resolve();
        assert_eq!(context.status, ResolutionStatus::Resolved);
        assert!(context.diagnostics.iter().any(|diagnostic| {
            diagnostic.source == "context.unsupported-remote"
                && diagnostic.severity == Severity::Warning
        }));
        assert_eq!(context.automatic_text(), "REMOVABLE_SENTINEL");

        for field in ["workspace", "target", "cwd", "home"] {
            let mut input = fixture.input.clone();
            match field {
                "workspace" => input.workspace = local.clone(),
                "target" => input.target = local.join("task.md"),
                "cwd" => input.cwd = local.clone(),
                "home" => input.codex_home = local.clone(),
                _ => unreachable!("fixed input fields"),
            }
            let context = resolve(&input, &[]);
            assert_eq!(
                context.status,
                ResolutionStatus::InvalidInput,
                "{kind}: {field}"
            );
            assert!(
                has_diagnostic(&context, "context.unsupported-remote"),
                "{kind}: {field}"
            );
            assert!(context.sources.is_empty());
        }
    }
}

#[test]
fn chained_local_directory_aliases_preserve_lexical_sources_and_revalidation() {
    let mut fixture = Fixture::load("deep");
    let second = fixture.temp.path().join("second-alias");
    let first = fixture.temp.path().join("first-alias");
    directory_alias(&fixture.input.workspace, &second);
    directory_alias(&second.join("nested"), &first);
    fixture.input.codex_home = first.clone();
    let context = fixture.resolve();
    assert_eq!(context.status, ResolutionStatus::Resolved);
    assert_eq!(context.sources[0].path, first.join("AGENTS.md"));
    assert_eq!(
        context.sources[0].physical_path,
        context.sources[2].physical_path
    );
    assert_eq!(
        context.automatic_text(),
        "NESTED_SENTINEL\n\n--- project-doc ---\n\nROOT_SENTINEL\n\nNESTED_SENTINEL"
    );
    assert!(
        context
            .select_sources(&[0, 1, 2])
            .expect("chained local aliases")
            .revalidate()
            .is_ok()
    );
    assert!(context.watch_paths().contains(&first.join("AGENTS.md")));
    remove_directory_alias(&first);
    remove_directory_alias(&second);
}

#[cfg(unix)]
#[test]
fn relative_link_targets_keep_parent_traversal_and_finite_repeated_links() {
    use std::os::unix::fs::symlink;

    let fixture = Fixture::load("removed");
    symlink("../AGENTS.md", fixture.input.cwd.join("relative-source"))
        .expect("relative instruction target");
    symlink("relative-source", fixture.input.cwd.join("AGENTS.md"))
        .expect("relative instruction chain");
    let context = fixture.resolve();
    assert_eq!(context.status, ResolutionStatus::Resolved);
    assert_eq!(context.sources.len(), 2);
    assert_eq!(
        context.sources[0].physical_path,
        context.sources[1].physical_path
    );
    assert!(
        context
            .select_sources(&[0, 1])
            .expect("relative source chain")
            .revalidate()
            .is_ok()
    );

    fs::remove_file(fixture.input.cwd.join("AGENTS.md")).expect("remove relative source chain");
    symlink("..", fixture.input.cwd.join("parent-alias")).expect("relative parent directory link");
    symlink(
        "parent-alias/nested/parent-alias/AGENTS.md",
        fixture.input.cwd.join("AGENTS.md"),
    )
    .expect("finite repeated parent alias");
    let context = fixture.resolve();
    assert_eq!(context.status, ResolutionStatus::Resolved);
    assert_eq!(context.sources.len(), 2);
    assert!(!has_diagnostic(&context, "context.cyclic-discovery"));
}

#[cfg(unix)]
#[test]
fn chained_relative_cycles_and_broken_targets_keep_discovery_diagnostics() {
    use std::os::unix::fs::symlink;

    for cyclic in [false, true] {
        let mut fixture = Fixture::load("removed");
        fixture.input.fallback_filenames = vec!["TEAM.md".to_string()];
        write(&fixture.input.cwd.join("TEAM.md"), "FALLBACK_SENTINEL");
        symlink("first-link", fixture.input.cwd.join("AGENTS.md")).expect("instruction link");
        symlink("second-link", fixture.input.cwd.join("first-link")).expect("first local hop");
        let target = if cyclic {
            "../nested/first-link"
        } else {
            "missing/../TEAM.md"
        };
        symlink(target, fixture.input.cwd.join("second-link")).expect("second local hop");
        let context = fixture.resolve();
        assert!(!has_diagnostic(&context, "context.unsupported-remote"));
        if cyclic {
            assert_eq!(context.status, ResolutionStatus::Partial);
            assert!(has_diagnostic(&context, "context.cyclic-discovery"));
            assert!(has_diagnostic(&context, "context.project-unavailable"));
            assert!(context.contents.is_empty());
        } else {
            assert_eq!(context.status, ResolutionStatus::Resolved);
            assert!(!has_diagnostic(&context, "context.cyclic-discovery"));
            assert_eq!(
                context.automatic_text(),
                "REMOVABLE_SENTINEL\n\nFALLBACK_SENTINEL"
            );
            assert!(
                context
                    .select_sources(&[0, 1])
                    .expect("fallback after broken chain")
                    .revalidate()
                    .is_ok()
            );
        }
    }
}

#[test]
fn missing_default_codex_home_is_allowed_but_explicit_override_must_exist() {
    let mut fixture = Fixture::load("removed");
    fixture.input.codex_home = fixture.temp.path().join("missing-default-home");
    fixture.input.codex_home_override = false;
    let context = fixture.resolve();
    assert_eq!(context.status, ResolutionStatus::Resolved);
    assert!(
        context
            .sources
            .iter()
            .all(|source| source.origin == Origin::Workspace)
    );
    let selected = context.select_sources(&[0]).expect("default home absence");
    assert!(selected.revalidate().is_ok());
    assert!(!selected.input().codex_home_override);
    fixture.input.codex_home_override = true;
    assert_eq!(fixture.resolve().status, ResolutionStatus::InvalidInput);
    fixture.input.codex_home_override = false;
    write(
        &fixture.input.codex_home.join("AGENTS.override.md"),
        "NEW_GLOBAL_SENTINEL",
    );
    assert!(selected.revalidate().is_err());
}

#[test]
fn cyclic_discovery_is_diagnosed_and_does_not_keep_a_project_prefix() {
    let mut fixture = Fixture::load("removed");
    let loop_path = fixture.input.cwd.join("loop");
    directory_alias(&loop_path, &loop_path);
    fixture.input.fallback_filenames = vec!["loop/AGENTS.md".to_string()];
    let context = fixture.resolve();
    assert_eq!(context.status, ResolutionStatus::Partial);
    assert!(has_diagnostic(&context, "context.cyclic-discovery"));
    assert!(has_diagnostic(&context, "context.project-unavailable"));
    assert!(
        context
            .sources
            .iter()
            .all(|source| source.content_index.is_none())
    );
    remove_directory_alias(&loop_path);
}

#[cfg(windows)]
#[test]
fn project_read_io_failure_retains_global_and_discards_entire_project_chain() {
    use std::os::windows::fs::OpenOptionsExt;

    let fixture = Fixture::load("precedence");
    let _locked = fs::OpenOptions::new()
        .read(true)
        .share_mode(0)
        .open(fixture.input.workspace.join("nested/AGENTS.md"))
        .expect("exclusive content lock");
    let context = fixture.resolve();
    assert_eq!(context.status, ResolutionStatus::Partial);
    assert!(has_diagnostic(&context, "context.project-unavailable"));
    assert_eq!(context.automatic_text(), "GLOBAL_OVERRIDE_SENTINEL");
    assert!(
        context
            .sources
            .iter()
            .filter(|source| source.origin == Origin::Workspace)
            .all(|source| source.content_index.is_none())
    );
}

#[test]
fn fixtures_bind_to_embedded_profile_digest_and_all_ten_named_classes() {
    let snapshot = include_bytes!("../assets/context-profiles/codex-agents-md-2026-08-29.json");
    let profile = ContextProfile::codex();
    assert_eq!(
        profile.content_digest,
        format!("{:x}", Sha256::digest(snapshot))
    );
    let map: serde_json::Value = serde_json::from_str(include_str!(
        "fixtures/effective-context/assertion-map.json"
    ))
    .expect("assertion map");
    assert_eq!(map["profile_id"], CODEX_PROFILE_ID);
    assert_eq!(map["source_revision"], CODEX_SOURCE_REVISION);
    assert_eq!(map["retrieved_on"], profile.retrieved_on);
    assert_eq!(map["profile_content_digest"], profile.content_digest);
    let cases = map["cases"].as_array().expect("ten fixture classes");
    assert_eq!(cases.len(), 10);
    for (index, case) in cases.iter().enumerate() {
        assert_eq!(case["case"], index + 1);
        let fixture = case["fixture"].as_str().expect("fixture filename");
        assert!(
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("tests/fixtures/effective-context")
                .join(fixture)
                .is_file()
        );
    }
}
