//! Headless effective-context snapshots and explicit, frozen Review selection.
//!
//! Discovery occurrences retain Codex ordering and raw byte-budget effects.
//! Content deduplication is only a MarkTurbo display/Review projection. Skill
//! availability never adds bodies to the automatic chain; an explicitly
//! configured project fallback is an instruction candidate regardless of name.

use std::fmt;
use std::path::{Component, Path, PathBuf};
use std::sync::LazyLock;

use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};

use super::skill::{Origin, Skill};
use crate::diagnostic::Diagnostic;

mod resolver;
pub use resolver::{ContextRevalidationError, resolve};

/// The only verified effective-context profile.
pub const CODEX_PROFILE_ID: &str = "codex-agents-md-2026-08-29";
/// The inspected upstream revision, not the current live documentation.
pub const CODEX_SOURCE_REVISION: &str = "b8c86376a258e55efc8e5ecfbabc21c16c07d814";
/// Immutable upstream source base for the profile's checked-in evidence.
pub const CODEX_SOURCE_URL: &str =
    "https://raw.githubusercontent.com/openai/codex/b8c86376a258e55efc8e5ecfbabc21c16c07d814/";

/// Explicit effective configuration for one local execution context.
///
/// Callers supply paths and configuration; this domain reads no environment or
/// configuration files. `target` names the artifact, while `cwd` determines
/// project instruction discovery. Paths preserve lexical discovery identity.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ContextInput {
    pub workspace: PathBuf,
    pub target: PathBuf,
    pub cwd: PathBuf,
    pub profile_id: String,
    pub codex_home: PathBuf,
    /// True for an explicit environment/scenario override; false for the ordinary default.
    pub codex_home_override: bool,
    pub fallback_filenames: Vec<String>,
    pub project_doc_max_bytes: usize,
    pub project_root_markers: Vec<String>,
    pub project_trust: ProjectTrust,
}

/// Explicit project trust; unspecified trust does not mean explicit distrust.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProjectTrust {
    Trusted,
    Untrusted,
    Unspecified,
}

/// Frozen profile evidence identity supplied by the checked-in profile.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ContextProfile {
    pub id: String,
    pub source_revision: String,
    pub source_url: String,
    pub retrieved_on: String,
    /// SHA-256 of the checked-in profile snapshot, in lowercase hexadecimal.
    pub content_digest: String,
}

impl ContextProfile {
    /// Bind checked-in profile evidence to the frozen Codex source identity.
    pub fn codex() -> Self {
        static CONTENT_DIGEST: LazyLock<String> = LazyLock::new(|| {
            format!(
                "{:x}",
                Sha256::digest(include_bytes!(
                    "../../assets/context-profiles/codex-agents-md-2026-08-29.json"
                ))
            )
        });
        Self {
            id: CODEX_PROFILE_ID.to_string(),
            source_revision: CODEX_SOURCE_REVISION.to_string(),
            source_url: CODEX_SOURCE_URL.to_string(),
            retrieved_on: "2026-10-02".to_string(),
            content_digest: CONTENT_DIGEST.clone(),
        }
    }
}

/// Whether the requested profile and inputs established a usable chain.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ResolutionStatus {
    Resolved,
    Partial,
    InvalidInput,
    InventoryOnly,
}

/// The Codex discovery rule selecting one source, not an AGENTS text parser.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InclusionRule {
    GlobalOverride,
    GlobalAgents,
    ProjectOverride,
    ProjectAgents,
    ProjectFallback,
}

/// One occurrence in the actual discovery chain, including empty/failed reads.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SourceOccurrence {
    /// Lexical path used for discovery and ordinary source navigation.
    pub path: PathBuf,
    /// Optional physical identity for displaying aliases, never discovery order.
    pub physical_path: Option<PathBuf>,
    #[serde(with = "origin_wire")]
    pub origin: Origin,
    /// Discovery directory; project applicability is root through execution cwd.
    pub scope: PathBuf,
    pub rule: InclusionRule,
    /// File bytes before trimming, truncation or lossy UTF-8 decoding.
    pub raw_bytes: usize,
    /// Raw bytes actually included; global bytes do not consume project budget.
    pub included_bytes: usize,
    /// Project budget already consumed before this occurrence, including aliases.
    pub project_bytes_before: usize,
    /// Index into the deduplicated content table; absent for no effective text.
    pub content_index: Option<usize>,
}

impl SourceOccurrence {
    pub fn is_truncated(&self) -> bool {
        self.origin == Origin::Workspace
            && self.content_index.is_some()
            && self.included_bytes > 0
            && self.included_bytes < self.raw_bytes
    }
}

/// A unique effective text. Provenance remains in every referring occurrence.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ContextContent {
    pub text: String,
}

/// Local resolution output, assembled by the resolver without Review authority.
///
/// The filesystem boundary must validate target/cwd membership in the workspace
/// before marking this resolved. Workspace aliases and Windows path casing are
/// filesystem identity questions, not lexical prefix rules. Preserve lexical
/// input paths and derive the project root and scopes from the same lexical cwd.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedContext {
    pub input: ContextInput,
    pub profile: Option<ContextProfile>,
    pub project_root: Option<PathBuf>,
    pub status: ResolutionStatus,
    pub assumptions: Vec<String>,
    /// Local discovery state only; provider wire and canonical Review digests omit it.
    pub discovery_digest: Option<[u8; 32]>,
    /// Actual global-first, root-to-cwd occurrences; never content-deduplicated.
    pub sources: Vec<SourceOccurrence>,
    pub contents: Vec<ContextContent>,
    pub diagnostics: Vec<Diagnostic>,
    /// Catalog metadata only; availability never includes Skill bodies.
    pub available_skills: Vec<Skill>,
}

impl ResolvedContext {
    /// Select only explicitly named occurrence indices, in original chain order.
    ///
    /// Empty selection selects nothing. Repeated, missing and content-free
    /// indices fail rather than silently expanding or changing the selection.
    /// The result owns its text and retains every explicitly selected alias.
    pub fn select_sources(
        &self,
        indices: &[usize],
    ) -> Result<SelectedContext, ContextSelectionError> {
        if !matches!(
            self.status,
            ResolutionStatus::Resolved | ResolutionStatus::Partial
        ) {
            return Err(ContextSelectionError::ResolutionUnavailable);
        }
        let profile = self
            .profile
            .clone()
            .ok_or(ContextSelectionError::UnsupportedProfile)?;
        let mut requested = indices.to_vec();
        requested.sort_unstable();
        if requested.windows(2).any(|pair| pair[0] == pair[1]) {
            return Err(ContextSelectionError::DuplicateSource);
        }
        let mut sources = Vec::with_capacity(requested.len());
        let mut contents: Vec<ContextContent> = Vec::new();
        for index in requested {
            let mut occurrence = self
                .sources
                .get(index)
                .ok_or(ContextSelectionError::UnknownSource(index))?
                .clone();
            let content = occurrence
                .content_index
                .and_then(|i| self.contents.get(i))
                .filter(|content| !content.text.trim().is_empty())
                .ok_or(ContextSelectionError::SourceNotSelectable(index))?;
            let content_index = match contents.iter().position(|existing| existing == content) {
                Some(index) => index,
                None => {
                    contents.push(content.clone());
                    contents.len() - 1
                }
            };
            occurrence.content_index = Some(content_index);
            sources.push(SelectedSource {
                source_index: index,
                occurrence,
            });
        }
        let selected = SelectedContext {
            input: self.input.clone(),
            profile,
            project_root: self.project_root.clone(),
            status: self.status,
            assumptions: self.assumptions.clone(),
            discovery_digest: self.discovery_digest,
            sources,
            contents,
        };
        selected.validate()?;
        Ok(selected)
    }
}

/// A named occurrence in the explicit selection, retaining original chain index.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SelectedSource {
    source_index: usize,
    occurrence: SourceOccurrence,
}

impl SelectedSource {
    pub fn source_index(&self) -> usize {
        self.source_index
    }
    pub fn occurrence(&self) -> &SourceOccurrence {
        &self.occurrence
    }
}

/// Immutable, deterministic projection for one explicitly selected Review scope.
///
/// Deserialization validates the same profile, applicability and selection
/// invariants as local selection. It grants no endpoint or outbound consent.
/// It validates a frozen projection, not current filesystem identity; membership
/// validation belongs to the resolver, and decoding never reopens source files.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SelectedContext {
    input: ContextInput,
    profile: ContextProfile,
    project_root: Option<PathBuf>,
    status: ResolutionStatus,
    assumptions: Vec<String>,
    /// Local deterministic serialization retains this; outbound projections must omit it.
    discovery_digest: Option<[u8; 32]>,
    sources: Vec<SelectedSource>,
    contents: Vec<ContextContent>,
}

impl<'de> Deserialize<'de> for SelectedContext {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Wire {
            input: ContextInput,
            profile: ContextProfile,
            project_root: Option<PathBuf>,
            status: ResolutionStatus,
            assumptions: Vec<String>,
            discovery_digest: Option<[u8; 32]>,
            sources: Vec<SelectedSource>,
            contents: Vec<ContextContent>,
        }
        let wire = Wire::deserialize(deserializer)?;
        let selected = Self {
            input: wire.input,
            profile: wire.profile,
            project_root: wire.project_root,
            status: wire.status,
            assumptions: wire.assumptions,
            discovery_digest: wire.discovery_digest,
            sources: wire.sources,
            contents: wire.contents,
        };
        selected.validate().map_err(serde::de::Error::custom)?;
        Ok(selected)
    }
}

impl SelectedContext {
    pub fn input(&self) -> &ContextInput {
        &self.input
    }
    pub fn profile(&self) -> &ContextProfile {
        &self.profile
    }
    pub fn project_root(&self) -> Option<&Path> {
        self.project_root.as_deref()
    }
    pub fn status(&self) -> ResolutionStatus {
        self.status
    }
    pub fn assumptions(&self) -> &[String] {
        &self.assumptions
    }
    /// Local revalidation state, never part of provider wire or canonical Review digest.
    pub fn discovery_digest(&self) -> Option<[u8; 32]> {
        self.discovery_digest
    }
    pub fn sources(&self) -> &[SelectedSource] {
        &self.sources
    }
    pub fn contents(&self) -> &[ContextContent] {
        &self.contents
    }
    pub fn is_empty(&self) -> bool {
        self.sources.is_empty()
    }

    /// Exact frozen text for a named occurrence, including duplicate provenance.
    pub fn source_content(&self, source: &SelectedSource) -> Option<&str> {
        self.sources
            .iter()
            .find(|selected| *selected == source)
            .and_then(|selected| selected.occurrence.content_index)
            .and_then(|index| self.contents.get(index))
            .map(|content| content.text.as_str())
    }

    fn validate(&self) -> Result<(), ContextSelectionError> {
        let invalid = ContextSelectionError::InvalidProjection;
        if self.input.profile_id != CODEX_PROFILE_ID || self.profile != ContextProfile::codex() {
            return Err(ContextSelectionError::UnsupportedProfile);
        }
        let invalid_path = [
            &self.input.workspace,
            &self.input.cwd,
            &self.input.target,
            &self.input.codex_home,
        ]
        .into_iter()
        .chain(self.project_root.iter())
        .chain(
            self.sources
                .iter()
                .flat_map(|source| [&source.occurrence.path, &source.occurrence.scope]),
        )
        .any(|path| {
            !path.is_absolute() || path.components().any(|part| part == Component::ParentDir)
        });
        if !matches!(
            self.status,
            ResolutionStatus::Resolved | ResolutionStatus::Partial
        ) || invalid_path
            || self
                .project_root
                .as_ref()
                .is_some_and(|root| !root.is_absolute() || !self.input.cwd.starts_with(root))
        {
            return Err(invalid);
        }
        if self
            .sources
            .windows(2)
            .any(|pair| pair[0].source_index >= pair[1].source_index)
        {
            return Err(invalid);
        }
        let mut seen_contents = Vec::new();
        let mut previous_project: Option<&SourceOccurrence> = None;
        let mut seen_global = false;
        for source in &self.sources {
            let occurrence = &source.occurrence;
            let content_index = occurrence
                .content_index
                .ok_or(ContextSelectionError::InvalidProjection)?;
            let content = self
                .contents
                .get(content_index)
                .ok_or(ContextSelectionError::InvalidProjection)?;
            if content.text.trim().is_empty()
                || occurrence.included_bytes == 0
                || occurrence.included_bytes > occurrence.raw_bytes
                || (occurrence.rule != InclusionRule::ProjectFallback
                    && occurrence.path.parent() != Some(occurrence.scope.as_path()))
            {
                return Err(invalid);
            }
            let name = occurrence
                .path
                .file_name()
                .and_then(|name| name.to_str())
                .ok_or(ContextSelectionError::InvalidProjection)?;
            match occurrence.origin {
                Origin::Global => {
                    if seen_global
                        || previous_project.is_some()
                        || occurrence.scope != self.input.codex_home
                        || occurrence.project_bytes_before != 0
                        || !matches!(
                            (occurrence.rule, name),
                            (InclusionRule::GlobalOverride, "AGENTS.override.md")
                                | (InclusionRule::GlobalAgents, "AGENTS.md")
                        )
                    {
                        return Err(invalid);
                    }
                    seen_global = true;
                }
                Origin::Workspace => {
                    let root = self
                        .project_root
                        .as_ref()
                        .ok_or(ContextSelectionError::InvalidProjection)?;
                    let budget_end = occurrence
                        .project_bytes_before
                        .checked_add(occurrence.included_bytes)
                        .ok_or(ContextSelectionError::InvalidProjection)?;
                    let named = match occurrence.rule {
                        InclusionRule::ProjectOverride => name == "AGENTS.override.md",
                        InclusionRule::ProjectAgents => name == "AGENTS.md",
                        InclusionRule::ProjectFallback => self
                            .input
                            .fallback_filenames
                            .iter()
                            .map(|fallback| fallback.trim())
                            .any(|fallback| {
                                !fallback.is_empty()
                                    && fallback != "AGENTS.md"
                                    && fallback != "AGENTS.override.md"
                                    && occurrence.path == occurrence.scope.join(fallback)
                            }),
                        _ => false,
                    };
                    if !named
                        || self.input.project_trust == ProjectTrust::Untrusted
                        || !occurrence.scope.starts_with(root)
                        || !self.input.cwd.starts_with(&occurrence.scope)
                        || budget_end > self.input.project_doc_max_bytes
                        || previous_project.is_some_and(|previous| {
                            previous.scope == occurrence.scope
                                || !occurrence.scope.starts_with(&previous.scope)
                                || occurrence.project_bytes_before
                                    < previous.project_bytes_before + previous.included_bytes
                        })
                    {
                        return Err(invalid);
                    }
                    previous_project = Some(occurrence);
                }
            }
            if !seen_contents.contains(&content_index) {
                if content_index != seen_contents.len() {
                    return Err(invalid);
                }
                seen_contents.push(content_index);
            }
        }
        if seen_contents.len() != self.contents.len()
            || self
                .contents
                .iter()
                .enumerate()
                .any(|(index, content)| self.contents[..index].contains(content))
        {
            return Err(invalid);
        }
        Ok(())
    }
}

/// Selection/decoding failures; source content failures remain resolver diagnostics.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ContextSelectionError {
    ResolutionUnavailable,
    UnsupportedProfile,
    DuplicateSource,
    UnknownSource(usize),
    SourceNotSelectable(usize),
    InvalidProjection,
}

impl fmt::Display for ContextSelectionError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ResolutionUnavailable => formatter.write_str("effective context is not resolved"),
            Self::UnsupportedProfile => {
                formatter.write_str("effective context profile is not supported")
            }
            Self::DuplicateSource => {
                formatter.write_str("context source was selected more than once")
            }
            Self::UnknownSource(index) => write!(formatter, "unknown context source index {index}"),
            Self::SourceNotSelectable(index) => write!(
                formatter,
                "context source {index} has no effective instruction text"
            ),
            Self::InvalidProjection => formatter.write_str("invalid frozen context selection"),
        }
    }
}

impl std::error::Error for ContextSelectionError {}

mod origin_wire {
    use super::Origin;
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S>(origin: &Origin, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(origin.label())
    }

    pub fn deserialize<'de, D>(deserializer: D) -> Result<Origin, D::Error>
    where
        D: Deserializer<'de>,
    {
        match String::deserialize(deserializer)?.as_str() {
            "global" => Ok(Origin::Global),
            "workspace" => Ok(Origin::Workspace),
            _ => Err(serde::de::Error::custom("invalid context source origin")),
        }
    }
}

#[cfg(test)]
mod effective_context_selection {
    use super::super::skill::SkillMeta;
    use super::*;
    use serde_json::Value;

    fn resolved() -> ResolvedContext {
        let base = std::env::current_dir().expect("test working directory");
        let workspace = base.join("context-workspace");
        let cwd = workspace.join("nested");
        let codex_home = base.join("codex-home");
        ResolvedContext {
            input: ContextInput {
                workspace: workspace.clone(),
                target: cwd.join("task.md"),
                cwd: cwd.clone(),
                profile_id: CODEX_PROFILE_ID.to_string(),
                codex_home: codex_home.clone(),
                codex_home_override: false,
                fallback_filenames: vec!["TEAM.md".to_string()],
                project_doc_max_bytes: 32_768,
                project_root_markers: vec![".git".to_string()],
                project_trust: ProjectTrust::Trusted,
            },
            profile: Some(ContextProfile::codex()),
            project_root: Some(workspace.clone()),
            status: ResolutionStatus::Resolved,
            assumptions: Vec::new(),
            discovery_digest: Some([7; 32]),
            sources: vec![
                SourceOccurrence {
                    path: codex_home.join("AGENTS.md"),
                    physical_path: None,
                    origin: Origin::Global,
                    scope: codex_home,
                    rule: InclusionRule::GlobalAgents,
                    raw_bytes: 8,
                    included_bytes: 6,
                    project_bytes_before: 0,
                    content_index: Some(0),
                },
                SourceOccurrence {
                    path: workspace.join("AGENTS.md"),
                    physical_path: Some(workspace.join("instructions.md")),
                    origin: Origin::Workspace,
                    scope: workspace.clone(),
                    rule: InclusionRule::ProjectAgents,
                    raw_bytes: 3,
                    included_bytes: 3,
                    project_bytes_before: 0,
                    content_index: Some(1),
                },
                SourceOccurrence {
                    path: cwd.join("AGENTS.override.md"),
                    physical_path: Some(workspace.join("instructions.md")),
                    origin: Origin::Workspace,
                    scope: cwd.clone(),
                    rule: InclusionRule::ProjectOverride,
                    raw_bytes: 3,
                    included_bytes: 3,
                    project_bytes_before: 3,
                    content_index: Some(1),
                },
            ],
            contents: vec![
                ContextContent {
                    text: "global".to_string(),
                },
                ContextContent {
                    text: "one".to_string(),
                },
            ],
            diagnostics: Vec::new(),
            available_skills: vec![Skill {
                dir: workspace.join(".agents/skills/demo"),
                entry: workspace.join(".agents/skills/demo/SKILL.md"),
                root: workspace.join(".agents/skills"),
                origin: Origin::Workspace,
                aliases: Vec::new(),
                name: "demo".to_string(),
                meta: SkillMeta::default(),
                diagnostics: Vec::new(),
                support_dirs: Vec::new(),
            }],
        }
    }

    #[test]
    fn selection_preserves_chain_order_not_click_order() {
        let context = resolved();
        let selected = context.select_sources(&[2, 0]).expect("explicit selection");
        assert_eq!(
            selected
                .sources()
                .iter()
                .map(SelectedSource::source_index)
                .collect::<Vec<_>>(),
            vec![0, 2]
        );
        assert_eq!(
            selected.source_content(&selected.sources()[0]),
            Some("global")
        );
        assert_eq!(selected.source_content(&selected.sources()[1]), Some("one"));
        assert_eq!(selected.sources()[1].occurrence().project_bytes_before, 3);
        assert_eq!(selected.input(), &context.input);
        assert_eq!(
            selected.profile(),
            context.profile.as_ref().expect("profile")
        );
    }

    #[test]
    fn no_selection_includes_no_sources_content_or_skills() {
        let context = resolved();
        let selected = context
            .select_sources(&[])
            .expect("empty explicit selection");
        assert!(selected.is_empty());
        assert!(selected.contents().is_empty());
        let wire = serde_json::to_value(&selected).expect("encode selection");
        assert!(wire.get("available_skills").is_none());
        assert!(wire["sources"].as_array().expect("source array").is_empty());
        assert_eq!(context.sources.len(), 3);
        assert_eq!(context.available_skills.len(), 1);
    }

    #[test]
    fn duplicate_content_retains_selected_provenance_and_raw_budget() {
        let context = resolved();
        let selected = context.select_sources(&[1, 2]).expect("selected aliases");
        assert_eq!(selected.sources().len(), 2);
        assert_eq!(
            selected.contents(),
            &[ContextContent {
                text: "one".to_string()
            }]
        );
        let first = selected.sources()[0].occurrence();
        let second = selected.sources()[1].occurrence();
        assert_ne!(first.path, second.path);
        assert_eq!(first.physical_path, second.physical_path);
        assert_eq!(first.content_index, second.content_index);
        assert_eq!(first.included_bytes + second.included_bytes, 6);
        assert_eq!(second.project_bytes_before, 3);
        let only_second = context.select_sources(&[2]).expect("one named alias");
        assert_eq!(only_second.sources().len(), 1);
        assert_eq!(only_second.sources()[0].source_index(), 2);
    }

    #[test]
    fn selection_owns_a_frozen_projection() {
        let mut context = resolved();
        let selected = context.select_sources(&[0, 1]).expect("explicit selection");
        context.contents[1].text = "changed".to_string();
        context.input.project_doc_max_bytes = 0;
        context.sources.clear();
        assert_eq!(selected.source_content(&selected.sources()[1]), Some("one"));
        assert_eq!(selected.input().project_doc_max_bytes, 32_768);
        assert_eq!(selected.sources().len(), 2);
    }

    #[test]
    fn projection_preserves_resolver_validated_workspace_aliases() {
        for workspace_name in ["workspace-alias", "CONTEXT-WORKSPACE"] {
            let mut context = resolved();
            context.input.workspace = context.input.workspace.with_file_name(workspace_name);
            let selected = context
                .select_sources(&[0, 1, 2])
                .expect("resolved workspace alias");
            assert_eq!(selected.input(), &context.input);
            assert_eq!(selected.project_root(), context.project_root.as_deref());
            let wire = serde_json::to_vec(&selected).expect("encode selection");
            assert_eq!(
                serde_json::from_slice::<SelectedContext>(&wire).expect("validated decode"),
                selected
            );
        }
    }

    #[test]
    fn configured_fallback_retains_discovery_scope_and_lexical_path() {
        let mut context = resolved();
        context.input.fallback_filenames = vec![" docs/TEAM.md ".to_string()];
        context.sources[1].path = context.input.workspace.join("docs/TEAM.md");
        context.sources[1].rule = InclusionRule::ProjectFallback;
        let selected = context
            .select_sources(&[1])
            .expect("explicit configured fallback");
        assert_eq!(
            selected.sources()[0].occurrence().scope,
            context.input.workspace
        );
        assert_eq!(
            selected.sources()[0].occurrence().path,
            context.sources[1].path
        );
        let wire = serde_json::to_vec(&selected).expect("encode selection");
        assert_eq!(
            serde_json::from_slice::<SelectedContext>(&wire).expect("validated decode"),
            selected
        );
    }

    #[test]
    fn skill_filename_requires_explicit_instruction_fallback_not_catalog_availability() {
        for filename in ["SKILL.md", "skill.md"] {
            let mut context = resolved();
            context.sources[1].path = context.input.workspace.join(filename);
            context.sources[1].rule = InclusionRule::ProjectFallback;
            assert_eq!(
                context.select_sources(&[1]),
                Err(ContextSelectionError::InvalidProjection)
            );
            context.input.fallback_filenames = vec![filename.to_string()];
            let selected = context
                .select_sources(&[1])
                .expect("configured instruction fallback");
            assert_eq!(selected.sources().len(), 1);
            assert_eq!(
                selected.sources()[0].occurrence().rule,
                InclusionRule::ProjectFallback
            );
            assert_eq!(
                selected.sources()[0].occurrence().path,
                context.sources[1].path
            );
            assert_eq!(selected.source_content(&selected.sources()[0]), Some("one"));
            let wire = serde_json::to_vec(&selected).expect("encode selection");
            assert_eq!(
                serde_json::from_slice::<SelectedContext>(&wire).expect("validated decode"),
                selected
            );
            assert_eq!(context.available_skills.len(), 1);
        }
    }

    #[test]
    fn selection_serialization_is_deterministic_and_round_trips() {
        let context = resolved();
        let first = context.select_sources(&[2, 0, 1]).expect("selection");
        let second = context
            .select_sources(&[1, 2, 0])
            .expect("same named sources");
        let first_wire = serde_json::to_vec(&first).expect("encode selection");
        assert_eq!(
            first_wire,
            serde_json::to_vec(&second).expect("encode selection")
        );
        let decoded: SelectedContext =
            serde_json::from_slice(&first_wire).expect("validated decode");
        assert_eq!(decoded, first);
        assert_eq!(
            serde_json::to_vec(&decoded).expect("re-encode selection"),
            first_wire
        );
    }

    #[test]
    fn local_selection_serialization_retains_typed_discovery_state_separately() {
        let context = resolved();
        let selected = context.select_sources(&[1]).expect("local selection");
        assert_eq!(selected.discovery_digest(), Some([7; 32]));
        assert!(selected.assumptions().is_empty());
        let wire = serde_json::to_value(&selected).expect("local deterministic wire");
        assert_eq!(
            wire["discovery_digest"],
            serde_json::to_value([7u8; 32]).expect("digest bytes")
        );
        let decoded: SelectedContext = serde_json::from_value(wire).expect("local decode");
        assert_eq!(decoded.discovery_digest(), selected.discovery_digest());
        assert_eq!(decoded, selected);
    }

    #[test]
    fn invalid_source_requests_are_not_silently_selected() {
        let mut context = resolved();
        assert_eq!(
            context.select_sources(&[0, 0]),
            Err(ContextSelectionError::DuplicateSource)
        );
        assert_eq!(
            context.select_sources(&[3]),
            Err(ContextSelectionError::UnknownSource(3))
        );
        context.sources[1].content_index = None;
        assert_eq!(
            context.select_sources(&[1]),
            Err(ContextSelectionError::SourceNotSelectable(1))
        );
        context.status = ResolutionStatus::InventoryOnly;
        assert_eq!(
            context.select_sources(&[0]),
            Err(ContextSelectionError::ResolutionUnavailable)
        );
    }

    fn rejects(mutator: impl FnOnce(&mut Value)) {
        let selected = resolved().select_sources(&[0, 1, 2]).expect("selection");
        let mut wire = serde_json::to_value(selected).expect("encode selection");
        mutator(&mut wire);
        assert!(serde_json::from_value::<SelectedContext>(wire).is_err());
    }

    #[test]
    fn serialized_input_cannot_bypass_profile_and_trust() {
        rejects(|wire| wire["input"]["profile_id"] = "other-harness".into());
        rejects(|wire| wire["profile"]["source_revision"] = "live-main".into());
        rejects(|wire| wire["profile"]["content_digest"] = "invalid".into());
        rejects(|wire| wire["profile"]["content_digest"] = "0".repeat(64).into());
        rejects(|wire| wire["input"]["project_trust"] = "untrusted".into());
        rejects(|wire| wire["status"] = "inventory_only".into());
        rejects(|wire| wire["input"]["project_doc_max_bytes"] = 5.into());
        rejects(|wire| {
            wire["input"]["target"] =
                serde_json::to_value(PathBuf::from("task.md")).expect("encode path")
        });
        rejects(|wire| {
            wire["input"]["target"] =
                serde_json::to_value(resolved().input.workspace.join("..").join("outside.md"))
                    .expect("encode path")
        });
    }

    #[test]
    fn serialized_input_cannot_bypass_explicit_source_projection() {
        rejects(|wire| {
            wire["sources"]
                .as_array_mut()
                .expect("source array")
                .swap(0, 1)
        });
        rejects(|wire| wire["sources"][2]["source_index"] = 1.into());
        rejects(|wire| wire["sources"][0]["occurrence"]["content_index"] = 10.into());
        rejects(|wire| {
            wire["contents"]
                .as_array_mut()
                .expect("content array")
                .push(serde_json::json!({"text": "unselected"}))
        });
        rejects(|wire| wire["sources"][1]["occurrence"]["rule"] = "inventory_only".into());
        rejects(|wire| wire["sources"][2]["occurrence"]["project_bytes_before"] = 0.into());
        rejects(|wire| {
            let path = resolved().input.workspace.join("SKILL.md");
            wire["sources"][1]["occurrence"]["path"] =
                serde_json::to_value(path).expect("encode path");
            wire["sources"][1]["occurrence"]["rule"] = "project_fallback".into();
        });
    }
}
