//! Offline filesystem interpretation of the checked-in Codex profile.

use std::collections::BTreeMap;
use std::fmt;
use std::fs;
use std::io;
use std::path::{Component, Path, PathBuf};

use serde::Deserialize;
use sha2::{Digest as _, Sha256};

use super::{
    CODEX_PROFILE_ID, CODEX_SOURCE_REVISION, ContextContent, ContextInput, ContextProfile,
    InclusionRule, ProjectTrust, ResolutionStatus, ResolvedContext, SelectedContext,
    SourceOccurrence,
};
use crate::agent_artifacts::skill::{Origin, Skill};
use crate::diagnostic::Diagnostic;

const SNAPSHOT: &[u8] =
    include_bytes!("../../../assets/context-profiles/codex-agents-md-2026-08-29.json");
const GLOBAL_SEPARATOR: &str = "\n\n--- project-doc ---\n\n";
const PROJECT_SEPARATOR: &str = "\n\n";

#[derive(Deserialize)]
struct Snapshot {
    profile_id: String,
    upstream_revision: String,
    assumptions: Vec<String>,
    rules: Vec<SnapshotRule>,
}

#[derive(Deserialize)]
struct SnapshotRule {
    id: String,
}

/// Resolve one explicit local configuration without environment or network reads.
///
/// `target` is validated against the workspace, but only `cwd` drives discovery.
/// Failures retain diagnostics and source navigation; no instruction is executed.
pub fn resolve(input: &ContextInput, skills: &[Skill]) -> ResolvedContext {
    let mut result = ResolvedContext {
        input: input.clone(),
        profile: None,
        project_root: None,
        status: ResolutionStatus::InvalidInput,
        assumptions: Vec::new(),
        discovery_digest: None,
        sources: Vec::new(),
        contents: Vec::new(),
        diagnostics: Vec::new(),
        available_skills: skills.to_vec(),
    };
    if input.profile_id != CODEX_PROFILE_ID {
        result.status = ResolutionStatus::InventoryOnly;
        result.diagnostics.push(Diagnostic::warning(
            "context.unsupported-profile",
            "Only the frozen Codex profile supports effective-context resolution.",
        ));
        return result;
    }
    let snapshot = match serde_json::from_slice::<Snapshot>(SNAPSHOT) {
        Ok(snapshot)
            if snapshot.profile_id == CODEX_PROFILE_ID
                && snapshot.upstream_revision == CODEX_SOURCE_REVISION
                && [
                    "project-cwd",
                    "global-first-readable",
                    "project-first-existing",
                    "project-order",
                    "root-marker",
                    "fallback-normalization",
                    "raw-byte-truncation",
                    "separators",
                    "symlink-read",
                    "untrusted-project",
                    "opaque-text",
                    "skills-on-demand",
                    "codex-home-env",
                    "project-budget-excludes-global",
                ]
                .iter()
                .all(|id| snapshot.rules.iter().any(|rule| rule.id == *id)) =>
        {
            snapshot
        }
        _ => {
            result.diagnostics.push(Diagnostic::error(
                "context.profile-snapshot",
                "The embedded profile does not establish the frozen resolver rules.",
            ));
            return result;
        }
    };
    result.profile = Some(ContextProfile::codex());
    result.assumptions = snapshot.assumptions;
    let mut state = Sha256::new();
    if let Err(diagnostic) = validate_input(input, &mut state) {
        result.diagnostics.push(diagnostic);
        return result;
    }
    result.status = ResolutionStatus::Resolved;
    let mut global = None;
    for (name, rule) in [
        ("AGENTS.override.md", InclusionRule::GlobalOverride),
        ("AGENTS.md", InclusionRule::GlobalAgents),
    ] {
        let candidate = candidate(&input.codex_home.join(name), &mut state);
        match candidate.data {
            CandidateData::File => match read_selected(&candidate.path, &mut state) {
                Ok(bytes) => {
                    let decoded = String::from_utf8_lossy(&bytes);
                    let included_bytes = bytes.len()
                        - (decoded.len() - decoded.trim_start().len())
                        - (decoded.trim_start().len() - decoded.trim().len());
                    let text = decoded.trim().to_string();
                    if !text.is_empty() {
                        let raw_bytes = bytes.len();
                        global = Some((candidate, rule, text, included_bytes, raw_bytes));
                        break;
                    }
                }
                Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                Err(error) => result.diagnostics.push(discovery_error(
                    "context.global-read",
                    &candidate.path,
                    &error,
                    false,
                )),
            },
            CandidateData::Failure(ref error) => result.diagnostics.push(discovery_error(
                "context.global-read",
                &candidate.path,
                error,
                false,
            )),
            CandidateData::Missing | CandidateData::NotFile => {}
        }
    }
    if let Some((candidate, rule, text, included_bytes, raw_bytes)) = global {
        let content_index = content(&mut result, &candidate.path, &text);
        diagnose_opaque(&mut result, &candidate.path, &text);
        result.sources.push(SourceOccurrence {
            path: candidate.path,
            physical_path: candidate.physical_path,
            origin: Origin::Global,
            scope: input.codex_home.clone(),
            rule,
            raw_bytes,
            included_bytes,
            project_bytes_before: 0,
            content_index: Some(content_index),
        });
    }

    let root = project_root(input, &mut state, &mut result);
    result.project_root = Some(root.clone());
    let global_contents = result.contents.len();
    let mut project_failed = false;
    let mut directories: Vec<_> = input
        .cwd
        .ancestors()
        .take_while(|directory| directory.starts_with(&root))
        .collect();
    directories.reverse();
    let mut names = vec![
        (
            "AGENTS.override.md".to_string(),
            InclusionRule::ProjectOverride,
        ),
        ("AGENTS.md".to_string(), InclusionRule::ProjectAgents),
    ];
    for name in input.fallback_filenames.iter().map(|name| name.trim()) {
        if !name.is_empty() && !names.iter().any(|(existing, _)| existing == name) {
            names.push((name.to_string(), InclusionRule::ProjectFallback));
        }
    }
    let mut selected = Vec::new();
    if input.project_trust != ProjectTrust::Untrusted && input.project_doc_max_bytes > 0 {
        for directory in directories {
            for (name, rule) in &names {
                let candidate = candidate(&directory.join(name), &mut state);
                match candidate.data {
                    CandidateData::Failure(ref error) => {
                        project_failed = true;
                        result.diagnostics.push(discovery_error(
                            "context.project-read",
                            &candidate.path,
                            error,
                            true,
                        ));
                        break;
                    }
                    CandidateData::File => {
                        selected.push((candidate, *rule, directory.to_path_buf()));
                        break;
                    }
                    CandidateData::Missing | CandidateData::NotFile => {}
                }
            }
        }
    }
    let mut used = 0;
    if !project_failed {
        for (candidate, rule, scope) in selected {
            if used >= input.project_doc_max_bytes {
                break;
            }
            let bytes = match read_selected(&candidate.path, &mut state) {
                Ok(bytes) => bytes,
                Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
                Err(error) => {
                    result.diagnostics.push(discovery_error(
                        "context.project-read",
                        &candidate.path,
                        &error,
                        true,
                    ));
                    project_failed = true;
                    break;
                }
            };
            let raw_bytes = bytes.len();
            let mut occurrence = SourceOccurrence {
                path: candidate.path,
                physical_path: candidate.physical_path,
                origin: Origin::Workspace,
                scope,
                rule,
                raw_bytes,
                included_bytes: 0,
                project_bytes_before: used,
                content_index: None,
            };
            let end = bytes.len().min(input.project_doc_max_bytes - used);
            let text = String::from_utf8_lossy(&bytes[..end]).into_owned();
            if !text.trim().is_empty() {
                occurrence.included_bytes = end;
                occurrence.content_index = Some(content(&mut result, &occurrence.path, &text));
                diagnose_opaque(&mut result, &occurrence.path, &text);
                used += end;
                if end < bytes.len() {
                    result.diagnostics.push(Diagnostic::info(
                        "context.project-truncated",
                        format!(
                            "{}: {end}/{raw_bytes} raw bytes included",
                            occurrence.path.display()
                        ),
                    ));
                }
            }
            result.sources.push(occurrence);
        }
    }
    if project_failed {
        result.status = ResolutionStatus::Partial;
        // Codex's project read is one fallible operation. Never present an
        // accidentally successful prefix as the effective project chain.
        result.contents.truncate(global_contents);
        for source in &mut result.sources {
            if source.origin == Origin::Workspace {
                source.content_index = None;
                source.included_bytes = 0;
            }
        }
        result.diagnostics.push(Diagnostic::error(
            "context.project-unavailable",
            "Project discovery or reading failed; all project instruction content is unavailable.",
        ));
    }
    if result.contents.is_empty() {
        result.diagnostics.push(Diagnostic::info(
            "context.no-instructions",
            "No effective automatic instructions were found for this configuration.",
        ));
    }
    result.discovery_digest = Some(state.finalize().into());
    duplicate_lines(&mut result);
    result
}

fn validate_input(input: &ContextInput, state: &mut Sha256) -> Result<(), Diagnostic> {
    // Reject every explicit remote reference before probing any input path.
    // Marker strings otherwise remain verbatim, including spaces and parents.
    for path in [
        &input.workspace,
        &input.target,
        &input.cwd,
        &input.codex_home,
    ]
    .into_iter()
    .map(PathBuf::as_path)
    .chain(
        input
            .fallback_filenames
            .iter()
            .map(|name| Path::new(name.trim())),
    )
    .chain(input.project_root_markers.iter().map(Path::new))
    {
        if remote_path(path) {
            return Err(path_error(remote_path_error()));
        }
    }
    for (name, path, directory) in [
        ("workspace", &input.workspace, true),
        ("target", &input.target, false),
        ("cwd", &input.cwd, true),
        ("CODEX_HOME", &input.codex_home, true),
    ] {
        if !path.is_absolute() || path.components().any(|part| part == Component::ParentDir) {
            return Err(Diagnostic::error(
                "context.invalid-path",
                format!("{name} must be an absolute lexical path without parent traversal."),
            ));
        }
        let metadata = match local_metadata(path) {
            Ok(metadata) => metadata,
            Err(error)
                if name == "CODEX_HOME"
                    && !input.codex_home_override
                    && error.kind() == io::ErrorKind::NotFound =>
            {
                continue;
            }
            Err(error) => {
                if error.kind() == io::ErrorKind::Unsupported {
                    return Err(path_error(error));
                }
                return Err(Diagnostic::error(
                    "context.invalid-path",
                    format!("{name}: {error}"),
                ));
            }
        };
        if directory && !metadata.is_dir() {
            return Err(Diagnostic::error(
                "context.invalid-path",
                format!("{name} must name an existing directory."),
            ));
        }
    }
    let workspace = local_canonicalize(&input.workspace).map_err(path_error)?;
    let cwd = local_canonicalize(&input.cwd).map_err(path_error)?;
    let target = local_canonicalize(&input.target).map_err(path_error)?;
    if !physical_member(&cwd, &workspace) || !physical_member(&target, &workspace) {
        return Err(Diagnostic::error(
            "context.outside-workspace",
            "The target and execution cwd must resolve inside the open workspace.",
        ));
    }
    for path in [&workspace, &cwd, &target] {
        hash(state, path.as_os_str().as_encoded_bytes());
    }
    match local_canonicalize(&input.codex_home) {
        Ok(home) => hash(state, home.as_os_str().as_encoded_bytes()),
        Err(error) if !input.codex_home_override && error.kind() == io::ErrorKind::NotFound => {
            hash(state, b"missing-default-codex-home")
        }
        Err(error) => return Err(path_error(error)),
    }
    // These are effective filename inputs, not layered configuration sources.
    // Absolute or traversing candidates cannot have a valid discovery scope.
    for name in input.fallback_filenames.iter().map(|name| name.trim()) {
        let path = Path::new(name);
        if path.is_absolute()
            || path
                .components()
                .any(|part| matches!(part, Component::ParentDir | Component::Prefix(_)))
        {
            return Err(Diagnostic::error(
                "context.unsupported-configuration",
                "Instruction filenames must remain relative to their discovery directory.",
            ));
        }
    }
    Ok(())
}

fn path_error(error: io::Error) -> Diagnostic {
    Diagnostic::error(
        if error.kind() == io::ErrorKind::Unsupported {
            "context.unsupported-remote"
        } else {
            "context.invalid-path"
        },
        error.to_string(),
    )
}

fn remote_path(path: &Path) -> bool {
    let spelling = path.to_string_lossy().replace('\\', "/");
    if let Some(rest) = spelling
        .strip_prefix("//?/")
        .or_else(|| spelling.strip_prefix("//./"))
    {
        let mut components = rest.split('/');
        let prefix = components.next().unwrap_or_default();
        return prefix.eq_ignore_ascii_case("UNC")
            || (prefix.eq_ignore_ascii_case("GLOBALROOT")
                && components
                    .next()
                    .is_some_and(|component| component.eq_ignore_ascii_case("Device"))
                && components.next().is_some_and(|device| {
                    ["Mup", "LanmanRedirector", "WebDavRedirector"]
                        .into_iter()
                        .any(|redirector| device.eq_ignore_ascii_case(redirector))
                }));
    }
    if spelling.starts_with("//") {
        return true;
    }
    spelling.split_once("://").is_some_and(|(scheme, _)| {
        scheme.len() > 1
            && scheme
                .as_bytes()
                .first()
                .is_some_and(u8::is_ascii_alphabetic)
            && scheme
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'+' | b'-' | b'.'))
    })
}

fn remote_path_error() -> io::Error {
    io::Error::new(
        io::ErrorKind::Unsupported,
        "Remote context paths are not supported.",
    )
}

fn local_path(path: &Path) -> io::Result<()> {
    if remote_path(path) {
        return Err(remote_path_error());
    }
    // Probe local ancestors first, so a known remote link target is never
    // traversed while probing its children. Mapped filesystems still rely on
    // the profile's explicit local-filesystem assumption.
    let ancestors: Vec<_> = path.ancestors().collect();
    for ancestor in ancestors.into_iter().rev() {
        if let Ok(target) = fs::read_link(ancestor)
            && remote_path(&target)
        {
            return Err(remote_path_error());
        }
    }
    Ok(())
}

fn local_metadata(path: &Path) -> io::Result<fs::Metadata> {
    local_path(path)?;
    fs::metadata(path)
}

fn local_canonicalize(path: &Path) -> io::Result<PathBuf> {
    local_path(path)?;
    fs::canonicalize(path)
}

fn physical_member(path: &Path, root: &Path) -> bool {
    #[cfg(windows)]
    {
        let mut components = path.components();
        root.components().all(|root| {
            components.next().is_some_and(|part| {
                part.as_os_str()
                    .to_string_lossy()
                    .eq_ignore_ascii_case(&root.as_os_str().to_string_lossy())
            })
        })
    }
    #[cfg(not(windows))]
    {
        path.starts_with(root)
    }
}

fn project_root(input: &ContextInput, state: &mut Sha256, result: &mut ResolvedContext) -> PathBuf {
    if input.project_root_markers.is_empty() {
        return input.cwd.clone();
    }
    // Only probe through the nearest marker. More distant markers cannot
    // affect this chain until that root disappears; revalidation then rescans.
    for directory in input.cwd.ancestors() {
        hash(state, directory.as_os_str().as_encoded_bytes());
        for marker in &input.project_root_markers {
            let path = directory.join(marker);
            hash(state, path.as_os_str().as_encoded_bytes());
            match local_metadata(&path) {
                Ok(metadata) => {
                    hash(
                        state,
                        if metadata.is_dir() {
                            b"directory"
                        } else {
                            b"entry"
                        },
                    );
                    if let Ok(physical) = local_canonicalize(&path) {
                        hash(state, physical.as_os_str().as_encoded_bytes());
                    }
                    return directory.to_path_buf();
                }
                Err(error) if error.kind() == io::ErrorKind::NotFound => hash(state, b"missing"),
                Err(error) => {
                    hash(state, format!("{:?}", error.kind()).as_bytes());
                    result.diagnostics.push(discovery_error(
                        "context.project-marker",
                        &path,
                        &error,
                        false,
                    ));
                }
            }
        }
    }
    input.cwd.clone()
}

enum CandidateData {
    Missing,
    NotFile,
    File,
    Failure(io::Error),
}

struct Candidate {
    path: PathBuf,
    physical_path: Option<PathBuf>,
    data: CandidateData,
}

fn candidate(path: &Path, state: &mut Sha256) -> Candidate {
    hash(state, path.as_os_str().as_encoded_bytes());
    let mut candidate = Candidate {
        path: path.to_path_buf(),
        physical_path: None,
        data: CandidateData::Missing,
    };
    if let Err(error) = local_path(path) {
        hash(state, b"unsupported-remote");
        candidate.data = CandidateData::Failure(error);
        return candidate;
    }
    if let Ok(link) = fs::read_link(path) {
        hash(state, link.as_os_str().as_encoded_bytes());
    }
    match local_metadata(path) {
        Ok(metadata) => {
            candidate.physical_path = match local_canonicalize(path) {
                Ok(physical) => Some(physical),
                Err(error) if metadata.is_file() => {
                    hash(state, format!("{:?}", error.kind()).as_bytes());
                    candidate.data = CandidateData::Failure(error);
                    return candidate;
                }
                Err(_) => None,
            };
            if let Some(physical) = &candidate.physical_path {
                hash(state, physical.as_os_str().as_encoded_bytes());
            }
            if metadata.is_file() {
                hash(state, b"file");
                candidate.data = CandidateData::File;
            } else {
                hash(state, b"not-file");
                candidate.data = CandidateData::NotFile;
            }
        }
        Err(error) => {
            hash(state, format!("{:?}", error.kind()).as_bytes());
            candidate.data = if error.kind() == io::ErrorKind::NotFound {
                CandidateData::Missing
            } else {
                CandidateData::Failure(error)
            };
        }
    }
    candidate
}

fn read_selected(path: &Path, state: &mut Sha256) -> io::Result<Vec<u8>> {
    match local_path(path).and_then(|()| fs::read(path)) {
        Ok(bytes) => {
            hash(state, &bytes);
            Ok(bytes)
        }
        Err(error) => {
            hash(state, format!("{:?}", error.kind()).as_bytes());
            Err(error)
        }
    }
}

fn discovery_error(source: &str, path: &Path, error: &io::Error, project: bool) -> Diagnostic {
    if error.kind() == io::ErrorKind::Unsupported {
        return if project {
            Diagnostic::error(
                "context.unsupported-remote",
                format!("{}: {error}", path.display()),
            )
        } else {
            Diagnostic::warning(
                "context.unsupported-remote",
                format!("{}: {error}", path.display()),
            )
        };
    }
    // ERROR_CANT_RESOLVE_FILENAME on Windows; ELOOP on macOS/Linux.
    let loop_code = if cfg!(windows) {
        1921
    } else if cfg!(target_os = "macos") {
        62
    } else {
        40
    };
    let cyclic = error.raw_os_error() == Some(loop_code)
        || path.ancestors().any(|ancestor| {
            let mut current = ancestor.to_path_buf();
            let mut seen = Vec::new();
            while let Ok(target) = local_path(&current).and_then(|()| fs::read_link(&current)) {
                if remote_path(&target) {
                    return false;
                }
                if seen.contains(&current) {
                    return true;
                }
                seen.push(current.clone());
                let next = if target.is_absolute() {
                    target
                } else {
                    current.parent().unwrap_or(&current).join(target)
                };
                current = PathBuf::new();
                for component in next.components() {
                    if component == Component::ParentDir {
                        current.pop();
                    } else {
                        current.push(component.as_os_str());
                    }
                }
                if current.starts_with(ancestor) {
                    return true;
                }
            }
            false
        });
    let source = if cyclic {
        "context.cyclic-discovery"
    } else {
        source
    };
    let message = format!("{}: {error}", path.display());
    if project {
        Diagnostic::error(source, message)
    } else {
        Diagnostic::warning(source, message)
    }
}

fn hash(state: &mut Sha256, bytes: &[u8]) {
    state.update((bytes.len() as u64).to_le_bytes());
    state.update(bytes);
}

fn push_watch_path(paths: &mut Vec<PathBuf>, path: &Path) {
    paths.push(path.to_path_buf());
    if let Ok(physical) = local_canonicalize(path) {
        paths.push(physical);
    } else if let (Some(parent), Some(name)) = (path.parent(), path.file_name())
        && let Ok(physical_parent) = local_canonicalize(parent)
    {
        paths.push(physical_parent.join(name));
    }
}

fn content(result: &mut ResolvedContext, path: &Path, text: &str) -> usize {
    if let Some(index) = result
        .contents
        .iter()
        .position(|content| content.text == text)
    {
        if let Some(previous) = result
            .sources
            .iter()
            .find(|source| source.content_index == Some(index))
        {
            result.diagnostics.push(Diagnostic::info(
                "context.duplicate-source",
                format!("{} duplicates {}", path.display(), previous.path.display()),
            ));
        }
        index
    } else {
        result.contents.push(ContextContent {
            text: text.to_string(),
        });
        result.contents.len() - 1
    }
}

fn diagnose_opaque(result: &mut ResolvedContext, path: &Path, text: &str) {
    let mut frontmatter = false;
    let mut closed = false;
    for (index, line) in text.lines().enumerate() {
        let trimmed = line.trim();
        if index == 0 && trimmed == "---" {
            frontmatter = true;
            result.diagnostics.push(
                Diagnostic::warning(
                    "context.unsupported-metadata",
                    format!(
                        "{}: metadata is opaque instruction text, not a Codex applicability rule",
                        path.display()
                    ),
                )
                .at_line(index + 1),
            );
        } else if frontmatter && trimmed == "---" {
            closed = true;
        }
        let lower = trimmed.to_ascii_lowercase();
        if lower.starts_with("@import")
            || lower.starts_with("import:")
            || lower.starts_with("imports:")
        {
            result.diagnostics.push(
                Diagnostic::warning(
                    "context.unsupported-import",
                    format!("{}: import-like text is not followed", path.display()),
                )
                .at_line(index + 1),
            );
        }
        if lower.starts_with("scope:")
            || lower.starts_with("paths:")
            || lower.starts_with("applyto:")
            || lower.starts_with("globs:")
        {
            result.diagnostics.push(
                Diagnostic::warning(
                    "context.unsupported-scope",
                    format!(
                        "{}: scope-like text does not alter Codex discovery applicability",
                        path.display()
                    ),
                )
                .at_line(index + 1),
            );
        }
    }
    if frontmatter && !closed {
        result.diagnostics.push(
            Diagnostic::warning(
                "context.malformed-metadata",
                format!(
                    "{}: unterminated metadata-like block remains opaque instruction text",
                    path.display()
                ),
            )
            .at_line(1),
        );
    }
}

fn duplicate_lines(result: &mut ResolvedContext) {
    let mut seen: BTreeMap<String, (PathBuf, usize)> = BTreeMap::new();
    let mut diagnostics = Vec::new();
    for source in &result.sources {
        let Some(text) = source
            .content_index
            .and_then(|index| result.contents.get(index))
        else {
            continue;
        };
        for (index, line) in text.text.lines().enumerate() {
            if line.trim().is_empty() {
                continue;
            }
            if let Some((previous, previous_line)) = seen.get(line) {
                diagnostics.push(
                    Diagnostic::info(
                        "context.duplicate-directive",
                        format!(
                            "{}:{} duplicates {}:{}",
                            source.path.display(),
                            index + 1,
                            previous.display(),
                            previous_line
                        ),
                    )
                    .at_line(index + 1),
                );
            } else {
                seen.insert(line.to_string(), (source.path.clone(), index + 1));
            }
        }
    }
    result.diagnostics.extend(diagnostics);
}

impl ResolvedContext {
    /// Upstream automatic text, including repeated occurrences and raw ordering.
    /// Display and Review projections may use `contents` to deduplicate instead.
    pub fn automatic_text(&self) -> String {
        let mut text = String::new();
        let mut last_origin = None;
        for source in &self.sources {
            let Some(content) = source
                .content_index
                .and_then(|index| self.contents.get(index))
            else {
                continue;
            };
            if !text.is_empty() {
                text.push_str(if last_origin == Some(Origin::Global) {
                    GLOBAL_SEPARATOR
                } else {
                    PROJECT_SEPARATOR
                });
            }
            text.push_str(&content.text);
            last_origin = Some(source.origin);
        }
        text
    }

    /// Exact paths that may change this resolution, including missing candidates.
    /// Pair with `watch_directories` for non-recursive subscriptions. Marker
    /// opt-ins name the marker itself, never all of its descendants. This only
    /// probes metadata; shadowed instruction bodies are not read or watched.
    pub fn watch_paths(&self) -> Vec<PathBuf> {
        if !matches!(
            self.status,
            ResolutionStatus::Resolved | ResolutionStatus::Partial
        ) {
            return Vec::new();
        }
        let mut paths = Vec::new();
        for path in [
            &self.input.workspace,
            &self.input.cwd,
            &self.input.target,
            &self.input.codex_home,
        ] {
            push_watch_path(&mut paths, path);
        }
        let global_rule = self
            .sources
            .iter()
            .find(|source| source.origin == Origin::Global)
            .map(|source| source.rule);
        for (name, rule) in [
            ("AGENTS.override.md", InclusionRule::GlobalOverride),
            ("AGENTS.md", InclusionRule::GlobalAgents),
        ] {
            push_watch_path(&mut paths, &self.input.codex_home.join(name));
            if global_rule == Some(rule) {
                break;
            }
        }
        let mut names = vec!["AGENTS.override.md".to_string(), "AGENTS.md".to_string()];
        for name in self.input.fallback_filenames.iter().map(|name| name.trim()) {
            if !name.is_empty() && !names.iter().any(|existing| existing == name) {
                names.push(name.to_string());
            }
        }
        let root = self.project_root.as_deref().unwrap_or(&self.input.cwd);
        for directory in self
            .input
            .cwd
            .ancestors()
            .take_while(|directory| directory.starts_with(root))
        {
            push_watch_path(&mut paths, directory);
            for marker in &self.input.project_root_markers {
                let path = directory.join(marker);
                push_watch_path(&mut paths, &path);
                for parent in path
                    .ancestors()
                    .skip(1)
                    .take_while(|parent| parent.starts_with(directory))
                {
                    push_watch_path(&mut paths, parent);
                }
                if local_metadata(&path).is_ok() {
                    break;
                }
            }
            if self.input.project_trust != ProjectTrust::Untrusted
                && self.input.project_doc_max_bytes > 0
            {
                for name in &names {
                    let path = directory.join(name);
                    push_watch_path(&mut paths, &path);
                    for parent in path
                        .ancestors()
                        .skip(1)
                        .take_while(|parent| parent.starts_with(directory))
                    {
                        push_watch_path(&mut paths, parent);
                    }
                    match local_metadata(&path) {
                        Ok(metadata) if metadata.is_file() => break,
                        Err(error) if error.kind() != io::ErrorKind::NotFound => break,
                        Ok(_) | Err(_) => {}
                    }
                }
            }
        }
        for source in &self.sources {
            push_watch_path(&mut paths, &source.path);
            if let Some(physical) = &source.physical_path {
                paths.push(physical.clone());
            }
        }
        paths.sort();
        paths.dedup();
        paths
    }

    /// Non-recursive directory watches for candidate changes in the current chain.
    /// Stops at the nearest project root; includes global discovery and alias parents.
    /// Consent revalidation rescans markers if the current root is removed.
    pub fn watch_directories(&self) -> Vec<PathBuf> {
        if !matches!(
            self.status,
            ResolutionStatus::Resolved | ResolutionStatus::Partial
        ) {
            return Vec::new();
        }
        let mut paths = Vec::new();
        if let Some(parent) = self.input.target.parent() {
            paths.push(parent.to_path_buf());
        }
        let root = self.project_root.as_deref().unwrap_or(&self.input.cwd);
        let global =
            self.input.codex_home.ancestors().find(|directory| {
                local_metadata(directory).is_ok_and(|metadata| metadata.is_dir())
            });
        for directory in self
            .input
            .cwd
            .ancestors()
            .take_while(|directory| directory.starts_with(root))
            .chain(global)
        {
            paths.push(directory.to_path_buf());
            if let Ok(physical) = local_canonicalize(directory) {
                paths.push(physical);
            }
            for name in self
                .input
                .project_root_markers
                .iter()
                .map(String::as_str)
                .chain(
                    self.input
                        .fallback_filenames
                        .iter()
                        .map(|name| name.trim())
                        .filter(|name| !name.is_empty()),
                )
            {
                let candidate = directory.join(name);
                let parent = if candidate.as_path() == directory {
                    Some(directory)
                } else {
                    candidate.parent()
                };
                if let Some(parent) = parent {
                    // A watcher must be attachable even before a nested fallback
                    // directory exists. Its nearest existing ancestor observes creation.
                    if let Some(existing) = parent
                        .ancestors()
                        .find(|path| local_metadata(path).is_ok_and(|metadata| metadata.is_dir()))
                    {
                        paths.push(existing.to_path_buf());
                    }
                }
            }
            if local_path(directory)
                .and_then(|()| fs::read_link(directory))
                .is_ok()
                && let Some(parent) = directory.parent()
            {
                paths.push(parent.to_path_buf());
            }
        }
        for source in &self.sources {
            for path in std::iter::once(&source.path).chain(source.physical_path.iter()) {
                if let Some(parent) = path.parent() {
                    paths.push(parent.to_path_buf());
                }
            }
        }
        paths.sort();
        paths.dedup();
        paths
    }
}

/// A previously disclosed projection no longer matches its discovery context.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ContextRevalidationError;

impl fmt::Display for ContextRevalidationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .write_str("effective context changed; resolve and confirm its named sources again")
    }
}

impl std::error::Error for ContextRevalidationError {}

impl SelectedContext {
    /// Reopen the same frozen input and every discovery candidate before consent use.
    /// Inventory Skill bodies are deliberately not an input to this comparison.
    pub fn revalidate(&self) -> Result<(), ContextRevalidationError> {
        if self.discovery_digest().is_none() {
            return Err(ContextRevalidationError);
        }
        let current = resolve(self.input(), &[]);
        let indices: Vec<_> = self
            .sources()
            .iter()
            .map(|source| source.source_index())
            .collect();
        let selected = current
            .select_sources(&indices)
            .map_err(|_| ContextRevalidationError)?;
        if selected == *self {
            Ok(())
        } else {
            Err(ContextRevalidationError)
        }
    }
}
