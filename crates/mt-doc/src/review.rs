//! Provider-independent, read-only Review contracts.
//!
//! This module deliberately contains no provider, transport, filesystem, or
//! GPUI code.  It owns the immutable request scope and the small structured
//! result that an application may render.  Model output is treated as data:
//! none of the types below represent an action, a patch, a tool call, or an
//! executable document.

use std::borrow::Cow;
use std::fmt;
use std::path::Path;

use serde::{Deserialize, Serialize};

/// The structured response schema used by the first Review implementation.
pub const REVIEW_SCHEMA_VERSION: &str = "review-v1";

/// Review never displays more clarification questions than this.
pub const MAX_CLARIFICATION_QUESTIONS: usize = 5;
/// Maximum source bytes represented by one Agent Skill file.
pub const MAX_SKILL_FILE_BYTES: u64 = 512 * 1024;
/// Maximum source bytes represented by one Agent Skill package.
pub const MAX_SKILL_PACKAGE_BYTES: u64 = 4 * 1024 * 1024;

/// The kind of artifact the reviewer should understand.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ArtifactLens {
    Prompt,
    Specification,
    Plan,
    AgentInstructions,
    AgentSkill,
}

impl ArtifactLens {
    /// The stable label used in disclosures and diagnostics.
    pub const fn label(self) -> &'static str {
        match self {
            Self::Prompt => "Prompt",
            Self::Specification => "Specification",
            Self::Plan => "Plan",
            Self::AgentInstructions => "Agent Instructions",
            Self::AgentSkill => "Agent Skill",
        }
    }

    /// A deliberately conservative, filesystem-free default.
    pub const fn default_lens() -> Self {
        Self::Prompt
    }

    /// Infer a lens from a path without reading it.  The caller can always
    /// replace this inferred value before constructing a request.
    pub fn infer_from_path(path: impl AsRef<Path>) -> Self {
        let path = path.as_ref();
        let name = path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or_default()
            .to_ascii_lowercase();
        if name == "skill.md" {
            return Self::AgentSkill;
        }
        if matches!(name.as_str(), "agents.md" | "claude.md" | "claude.local.md")
            || name.ends_with(".instructions.md")
            || name.contains("rule")
        {
            return Self::AgentInstructions;
        }
        if name.contains("plan") || name.contains("roadmap") {
            return Self::Plan;
        }
        if name.contains("spec") || name.contains("requirement") {
            return Self::Specification;
        }
        Self::Prompt
    }
}

/// A checked, half-open byte range into one source value.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ByteRange {
    pub start: u64,
    pub end: u64,
}

impl ByteRange {
    pub const fn new(start: u64, end: u64) -> Result<Self, ReviewValidationError> {
        if start > end {
            return Err(ReviewValidationError::InvalidByteRange { start, end });
        }
        Ok(Self { start, end })
    }

    pub const fn empty(at: u64) -> Self {
        Self { start: at, end: at }
    }

    pub const fn len(self) -> u64 {
        self.end - self.start
    }

    pub const fn is_empty(self) -> bool {
        self.start == self.end
    }

    pub const fn contains(self, other: Self) -> bool {
        self.start <= other.start && other.end <= self.end
    }

    fn validate(self, source_len: u64) -> Result<(), ReviewValidationError> {
        if self.start > self.end || self.end > source_len {
            return Err(ReviewValidationError::RangeOutsideSource {
                start: self.start,
                end: self.end,
                source_len,
            });
        }
        Ok(())
    }
}

/// A stable source location.  Byte ranges are preferred; line ranges are
/// useful when a provider cannot preserve byte offsets.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum SourceLocation {
    ByteRange { start: u64, end: u64 },
    LineRange { start: u64, end: u64 },
}

impl SourceLocation {
    pub const fn bytes(start: u64, end: u64) -> Self {
        Self::ByteRange { start, end }
    }

    pub const fn lines(start: u64, end: u64) -> Self {
        Self::LineRange { start, end }
    }

    fn validate(
        self,
        source: Option<&str>,
        allowed: Option<ByteRange>,
    ) -> Result<(), ReviewValidationError> {
        match self {
            Self::ByteRange { start, end } => {
                let range = ByteRange::new(start, end)?;
                let source_len = source.map_or(u64::MAX, |source| source.len() as u64);
                if source.is_some() {
                    range.validate(source_len)?;
                    if let Some(source) = source {
                        let byte_start = usize::try_from(start).map_err(|_| {
                            ReviewValidationError::RangeOutsideSource {
                                start,
                                end,
                                source_len,
                            }
                        })?;
                        let byte_end = usize::try_from(end).map_err(|_| {
                            ReviewValidationError::RangeOutsideSource {
                                start,
                                end,
                                source_len,
                            }
                        })?;
                        if !source.is_char_boundary(byte_start)
                            || !source.is_char_boundary(byte_end)
                        {
                            return Err(ReviewValidationError::RangeNotUtf8Boundary { start, end });
                        }
                    }
                }
                if let Some(allowed) = allowed
                    && !allowed.contains(range)
                {
                    return Err(ReviewValidationError::AnchorOutsideSelection);
                }
            }
            Self::LineRange { start, end } => {
                if start == 0 || start > end {
                    return Err(ReviewValidationError::InvalidLineRange { start, end });
                }
                if let Some(source) = source {
                    let last_line = line_count(source);
                    if end > last_line {
                        return Err(ReviewValidationError::LineOutsideSource {
                            start,
                            end,
                            last_line,
                        });
                    }
                    if let Some(allowed) = allowed {
                        let allowed_lines = line_range_for_bytes(source, allowed);
                        if start < *allowed_lines.start() || end > *allowed_lines.end() {
                            return Err(ReviewValidationError::AnchorOutsideSelection);
                        }
                    }
                }
            }
        }
        Ok(())
    }
}

/// A finding anchor that cannot silently invent a source location.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum SourceAnchor {
    Document {
        location: SourceLocation,
    },
    AgentSkillFile {
        path: String,
        location: SourceLocation,
    },
    DocumentWide,
}

impl SourceAnchor {
    pub const fn document(location: SourceLocation) -> Self {
        Self::Document { location }
    }

    pub const fn document_wide() -> Self {
        Self::DocumentWide
    }

    pub fn agent_skill_file(
        path: impl AsRef<str>,
        location: SourceLocation,
    ) -> Result<Self, ReviewValidationError> {
        Ok(Self::AgentSkillFile {
            path: normalize_relative_path(path.as_ref())?,
            location,
        })
    }
}

/// The exact input scope disclosed for one Review request.
///
/// A selection is intentionally marked as missing surrounding context.  There
/// is no Effective Agent Context variant here; Goal 08 owns that boundary.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ReviewScope {
    Document,
    Selection {
        range: ByteRange,
        missing_context: bool,
    },
    AgentSkillPackage,
}

impl ReviewScope {
    pub const fn document() -> Self {
        Self::Document
    }

    pub fn selection(range: ByteRange) -> Self {
        Self::Selection {
            range,
            missing_context: true,
        }
    }

    pub const fn agent_skill_package() -> Self {
        Self::AgentSkillPackage
    }

    pub const fn is_selection(self) -> bool {
        matches!(self, Self::Selection { .. })
    }

    pub const fn selection_range(self) -> Option<ByteRange> {
        match self {
            Self::Selection { range, .. } => Some(range),
            _ => None,
        }
    }

    pub const fn missing_context(self) -> bool {
        matches!(
            self,
            Self::Selection {
                missing_context: true,
                ..
            }
        )
    }

    pub const fn kind(self) -> ReviewScopeKind {
        match self {
            Self::Document => ReviewScopeKind::Document,
            Self::Selection { .. } => ReviewScopeKind::Selection,
            Self::AgentSkillPackage => ReviewScopeKind::AgentSkillPackage,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReviewScopeKind {
    Document,
    Selection,
    AgentSkillPackage,
}

/// The source identity captured before Review leaves the application thread.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SourceSnapshot {
    pub revision: u64,
    pub source_generation: u64,
}

impl SourceSnapshot {
    pub const fn new(revision: u64, source_generation: u64) -> Self {
        Self {
            revision,
            source_generation,
        }
    }

    pub const fn matches(self, other: Self) -> bool {
        self.revision == other.revision && self.source_generation == other.source_generation
    }
}

impl Default for SourceSnapshot {
    fn default() -> Self {
        Self::new(0, 0)
    }
}

/// A regular file entry in an Agent Skill package.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SkillPackageFile {
    pub path: String,
    pub byte_size: u64,
    pub inclusion_reason: String,
    pub payload: SkillFilePayload,
}

impl SkillPackageFile {
    pub fn text(
        path: impl AsRef<str>,
        content: impl Into<String>,
        inclusion_reason: impl Into<String>,
    ) -> Result<Self, SkillPackageError> {
        let content = content.into();
        let byte_size = content.len() as u64;
        Self::new(
            path,
            byte_size,
            inclusion_reason,
            SkillFilePayload::Utf8 { content },
        )
    }

    pub fn binary_metadata(
        path: impl AsRef<str>,
        byte_size: u64,
        sha256: impl Into<String>,
        inclusion_reason: impl Into<String>,
    ) -> Result<Self, SkillPackageError> {
        Self::new(
            path,
            byte_size,
            inclusion_reason,
            SkillFilePayload::Binary {
                sha256: sha256.into(),
            },
        )
    }

    pub fn binary_raw(
        path: impl AsRef<str>,
        bytes: impl Into<Vec<u8>>,
        sha256: impl Into<String>,
        inclusion_reason: impl Into<String>,
    ) -> Result<Self, SkillPackageError> {
        let bytes = bytes.into();
        let byte_size = bytes.len() as u64;
        Self::new(
            path,
            byte_size,
            inclusion_reason,
            SkillFilePayload::RawBinary {
                bytes,
                sha256: sha256.into(),
            },
        )
    }

    fn new(
        path: impl AsRef<str>,
        byte_size: u64,
        inclusion_reason: impl Into<String>,
        payload: SkillFilePayload,
    ) -> Result<Self, SkillPackageError> {
        let path = normalize_relative_path(path.as_ref()).map_err(SkillPackageError::Validation)?;
        let inclusion_reason = non_empty(inclusion_reason.into(), "inclusion reason")
            .map_err(SkillPackageError::Validation)?;
        if byte_size > MAX_SKILL_FILE_BYTES {
            return Err(SkillPackageError::FileTooLarge {
                path,
                byte_size,
                limit: MAX_SKILL_FILE_BYTES,
            });
        }
        match &payload {
            SkillFilePayload::Utf8 { content } if content.len() as u64 != byte_size => {
                return Err(SkillPackageError::PayloadSizeMismatch {
                    path,
                    declared: byte_size,
                    actual: content.len() as u64,
                });
            }
            SkillFilePayload::RawBinary { bytes, .. } if bytes.len() as u64 != byte_size => {
                return Err(SkillPackageError::PayloadSizeMismatch {
                    path,
                    declared: byte_size,
                    actual: bytes.len() as u64,
                });
            }
            SkillFilePayload::Binary { sha256 } | SkillFilePayload::RawBinary { sha256, .. }
                if !valid_sha256(sha256) =>
            {
                return Err(SkillPackageError::InvalidSha256);
            }
            _ => {}
        }
        Ok(Self {
            path,
            byte_size,
            inclusion_reason,
            payload,
        })
    }

    pub fn is_binary_metadata_only(&self) -> bool {
        matches!(self.payload, SkillFilePayload::Binary { .. })
    }

    pub fn raw_bytes(&self) -> Option<&[u8]> {
        match &self.payload {
            SkillFilePayload::Utf8 { content } => Some(content.as_bytes()),
            SkillFilePayload::RawBinary { bytes, .. } => Some(bytes),
            SkillFilePayload::Binary { .. } => None,
        }
    }

    /// Validate a value that came from serde rather than one of the
    /// constructors above.  Decoding is deliberately not a bypass around the
    /// package size, path, or payload invariants.
    fn validate(&self) -> Result<(), SkillPackageError> {
        let normalized =
            normalize_relative_path(&self.path).map_err(SkillPackageError::Validation)?;
        if normalized != self.path {
            return Err(SkillPackageError::Validation(
                ReviewValidationError::AnchorPathNotNormalized,
            ));
        }
        if self.inclusion_reason.trim().is_empty() {
            return Err(SkillPackageError::Validation(
                ReviewValidationError::EmptyText("inclusion reason"),
            ));
        }
        if self.byte_size > MAX_SKILL_FILE_BYTES {
            return Err(SkillPackageError::FileTooLarge {
                path: self.path.clone(),
                byte_size: self.byte_size,
                limit: MAX_SKILL_FILE_BYTES,
            });
        }
        match &self.payload {
            SkillFilePayload::Utf8 { content } => {
                if content.len() as u64 != self.byte_size {
                    return Err(SkillPackageError::PayloadSizeMismatch {
                        path: self.path.clone(),
                        declared: self.byte_size,
                        actual: content.len() as u64,
                    });
                }
            }
            SkillFilePayload::Binary { sha256 } => {
                if !valid_sha256(sha256) {
                    return Err(SkillPackageError::InvalidSha256);
                }
            }
            SkillFilePayload::RawBinary { bytes, sha256 } => {
                if bytes.len() as u64 != self.byte_size {
                    return Err(SkillPackageError::PayloadSizeMismatch {
                        path: self.path.clone(),
                        declared: self.byte_size,
                        actual: bytes.len() as u64,
                    });
                }
                if !valid_sha256(sha256) {
                    return Err(SkillPackageError::InvalidSha256);
                }
            }
        }
        Ok(())
    }
}

/// How a package entry is represented at the provider boundary.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum SkillFilePayload {
    Utf8 {
        content: String,
    },
    /// A binary or non-UTF-8 file whose raw bytes were not explicitly selected.
    Binary {
        sha256: String,
    },
    /// Raw binary bytes are allowed only after an explicit user selection.
    RawBinary {
        bytes: Vec<u8>,
        sha256: String,
    },
}

/// A normally in-scope file that was not sent.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SkillPackageOmission {
    pub path: String,
    pub reason: String,
    pub symlink: bool,
}

impl SkillPackageOmission {
    pub fn symlink(
        path: impl AsRef<str>,
        reason: impl Into<String>,
    ) -> Result<Self, SkillPackageError> {
        Self::new(path, reason, true)
    }

    pub fn new(
        path: impl AsRef<str>,
        reason: impl Into<String>,
        symlink: bool,
    ) -> Result<Self, SkillPackageError> {
        Ok(Self {
            path: normalize_relative_path(path.as_ref()).map_err(SkillPackageError::Validation)?,
            reason: non_empty(reason.into(), "omission reason")
                .map_err(SkillPackageError::Validation)?,
            symlink,
        })
    }

    fn validate(&self) -> Result<(), SkillPackageError> {
        let normalized =
            normalize_relative_path(&self.path).map_err(SkillPackageError::Validation)?;
        if normalized != self.path {
            return Err(SkillPackageError::Validation(
                ReviewValidationError::AnchorPathNotNormalized,
            ));
        }
        if self.reason.trim().is_empty() {
            return Err(SkillPackageError::Validation(
                ReviewValidationError::EmptyText("omission reason"),
            ));
        }
        Ok(())
    }
}

/// A deterministic, bounded Agent Skill package.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SkillPackage {
    pub files: Vec<SkillPackageFile>,
    #[serde(default)]
    pub omissions: Vec<SkillPackageOmission>,
}

impl SkillPackage {
    pub fn new(
        mut files: Vec<SkillPackageFile>,
        mut omissions: Vec<SkillPackageOmission>,
    ) -> Result<Self, SkillPackageError> {
        validate_skill_package_entries(&files, &omissions)?;
        files.sort_by(|left, right| left.path.as_bytes().cmp(right.path.as_bytes()));
        omissions.sort_by(|left, right| left.path.as_bytes().cmp(right.path.as_bytes()));
        Ok(Self { files, omissions })
    }

    pub fn validate(&self) -> Result<(), SkillPackageError> {
        validate_skill_package_entries(&self.files, &self.omissions)
    }

    pub fn total_byte_size(&self) -> u64 {
        self.files.iter().map(|file| file.byte_size).sum()
    }

    pub fn is_partial(&self) -> bool {
        !self.omissions.is_empty()
    }

    /// Every path is normalized and sorted in UTF-8 byte order.
    pub fn files(&self) -> &[SkillPackageFile] {
        &self.files
    }

    pub fn omissions(&self) -> &[SkillPackageOmission] {
        &self.omissions
    }

    /// Build the exact source frames a provider adapter may serialize.
    pub fn source_frames(&self) -> Vec<SkillSourceFrame> {
        self.files.iter().map(SkillSourceFrame::from_file).collect()
    }

    /// Encode each frame as path length, path bytes, content length, content
    /// bytes.  Lengths are big-endian u64 values to make the framing
    /// unambiguous and independent of host architecture.
    pub fn framed_bytes(&self) -> Vec<u8> {
        let capacity = self
            .files
            .iter()
            .map(SkillPackageFile::encoded_frame_len)
            .sum();
        let mut encoded = Vec::with_capacity(capacity);
        for file in &self.files {
            let content = file.source_content();
            encoded.extend_from_slice(&(file.path.len() as u64).to_be_bytes());
            encoded.extend_from_slice(file.path.as_bytes());
            encoded.extend_from_slice(&(content.len() as u64).to_be_bytes());
            encoded.extend_from_slice(&content);
        }
        encoded
    }
}

impl SkillPackageFile {
    fn source_content(&self) -> Cow<'_, [u8]> {
        match &self.payload {
            SkillFilePayload::Utf8 { content } => Cow::Borrowed(content.as_bytes()),
            SkillFilePayload::RawBinary { bytes, .. } => Cow::Borrowed(bytes),
            SkillFilePayload::Binary { sha256 } => {
                Cow::Owned(metadata_only_source_content(self.byte_size, sha256))
            }
        }
    }

    fn encoded_frame_len(&self) -> usize {
        16 + self.path.len() + self.source_content().len()
    }
}

/// One deterministic source frame, kept separate from its byte encoding so
/// callers can disclose metadata without ever exposing omitted raw bytes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkillSourceFrame {
    path: String,
    content: Vec<u8>,
    metadata_only: bool,
}

impl SkillSourceFrame {
    fn from_file(file: &SkillPackageFile) -> Self {
        Self {
            path: file.path.clone(),
            content: file.source_content().into_owned(),
            metadata_only: file.is_binary_metadata_only(),
        }
    }

    pub fn path(&self) -> &str {
        &self.path
    }

    pub fn path_bytes(&self) -> &[u8] {
        self.path.as_bytes()
    }

    pub fn content_bytes(&self) -> &[u8] {
        &self.content
    }

    pub fn metadata_only(&self) -> bool {
        self.metadata_only
    }
}

/// Deterministic metadata substituted for a binary source file whose raw bytes
/// were not explicitly selected. It carries exactly the file size and digest,
/// but no user content, and is framed like every other package source.
pub fn metadata_only_source_content(byte_size: u64, sha256: &str) -> Vec<u8> {
    const METADATA_FRAME_VERSION: &[u8] = b"markturbo-agent-skill-metadata-v1\0";
    let mut content = Vec::with_capacity(METADATA_FRAME_VERSION.len() + 8 + sha256.len());
    content.extend_from_slice(METADATA_FRAME_VERSION);
    content.extend_from_slice(&byte_size.to_be_bytes());
    content.extend_from_slice(sha256.as_bytes());
    content
}

/// The source supplied to a Review request.  The document form carries the
/// full in-memory source even for a selection so anchors can be checked.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ReviewSource {
    Document {
        text: String,
        #[serde(default)]
        path: Option<String>,
    },
    AgentSkillPackage {
        package: SkillPackage,
    },
}

impl ReviewSource {
    pub fn document(text: impl Into<String>) -> Self {
        Self::Document {
            text: text.into(),
            path: None,
        }
    }

    pub fn document_at(text: impl Into<String>, path: impl Into<String>) -> Self {
        Self::Document {
            text: text.into(),
            path: Some(path.into()),
        }
    }

    pub fn agent_skill_package(package: SkillPackage) -> Self {
        Self::AgentSkillPackage { package }
    }

    pub fn document_text(&self) -> Option<&str> {
        match self {
            Self::Document { text, .. } => Some(text),
            Self::AgentSkillPackage { .. } => None,
        }
    }

    pub fn package(&self) -> Option<&SkillPackage> {
        match self {
            Self::Document { .. } => None,
            Self::AgentSkillPackage { package } => Some(package),
        }
    }
}

/// A frozen, provider-independent Review request.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReviewRequest {
    pub lens: ArtifactLens,
    pub scope: ReviewScope,
    pub source: ReviewSource,
    pub snapshot: SourceSnapshot,
}

impl ReviewRequest {
    pub fn decode_json(input: &str) -> Result<Self, ReviewDecodeError> {
        let request: Self = serde_json::from_str(input)
            .map_err(|error| ReviewDecodeError::Serde(error.to_string()))?;
        request.validate().map_err(ReviewDecodeError::Validation)?;
        Ok(request)
    }

    pub fn new(
        lens: ArtifactLens,
        scope: ReviewScope,
        source: ReviewSource,
        snapshot: SourceSnapshot,
    ) -> Result<Self, ReviewValidationError> {
        let request = Self {
            lens,
            scope,
            source,
            snapshot,
        };
        request.validate()?;
        Ok(request)
    }

    pub fn document(
        lens: ArtifactLens,
        text: impl Into<String>,
        snapshot: SourceSnapshot,
    ) -> Result<Self, ReviewValidationError> {
        Self::new(
            lens,
            ReviewScope::Document,
            ReviewSource::document(text),
            snapshot,
        )
    }

    pub fn selection(
        lens: ArtifactLens,
        text: impl Into<String>,
        range: ByteRange,
        snapshot: SourceSnapshot,
    ) -> Result<Self, ReviewValidationError> {
        Self::new(
            lens,
            ReviewScope::selection(range),
            ReviewSource::document(text),
            snapshot,
        )
    }

    pub fn agent_skill(
        package: SkillPackage,
        snapshot: SourceSnapshot,
    ) -> Result<Self, ReviewValidationError> {
        Self::new(
            ArtifactLens::AgentSkill,
            ReviewScope::AgentSkillPackage,
            ReviewSource::AgentSkillPackage { package },
            snapshot,
        )
    }

    pub fn validate(&self) -> Result<(), ReviewValidationError> {
        match (&self.scope, &self.source) {
            (ReviewScope::Document, ReviewSource::Document { .. }) => {}
            (
                ReviewScope::Selection {
                    range,
                    missing_context,
                },
                ReviewSource::Document { text, .. },
            ) => {
                if !missing_context {
                    return Err(ReviewValidationError::SelectionContextNotMarkedMissing);
                }
                range.validate(text.len() as u64)?;
                let start = usize::try_from(range.start).map_err(|_| {
                    ReviewValidationError::RangeOutsideSource {
                        start: range.start,
                        end: range.end,
                        source_len: text.len() as u64,
                    }
                })?;
                let end = usize::try_from(range.end).map_err(|_| {
                    ReviewValidationError::RangeOutsideSource {
                        start: range.start,
                        end: range.end,
                        source_len: text.len() as u64,
                    }
                })?;
                if !text.is_char_boundary(start) || !text.is_char_boundary(end) {
                    return Err(ReviewValidationError::RangeNotUtf8Boundary {
                        start: range.start,
                        end: range.end,
                    });
                }
            }
            (ReviewScope::AgentSkillPackage, ReviewSource::AgentSkillPackage { package }) => {
                validate_skill_package(package)?;
            }
            (ReviewScope::AgentSkillPackage, ReviewSource::Document { .. }) => {
                return Err(ReviewValidationError::ScopeSourceMismatch);
            }
            (_, ReviewSource::AgentSkillPackage { .. }) => {
                return Err(ReviewValidationError::ScopeSourceMismatch);
            }
        }
        if self.lens == ArtifactLens::AgentSkill
            && !matches!(self.scope, ReviewScope::AgentSkillPackage)
        {
            return Err(ReviewValidationError::LensScopeMismatch);
        }
        if self.lens != ArtifactLens::AgentSkill
            && matches!(self.scope, ReviewScope::AgentSkillPackage)
        {
            return Err(ReviewValidationError::LensScopeMismatch);
        }
        Ok(())
    }

    /// The exact user-content bytes covered by this request, excluding any
    /// provider protocol framing.
    pub fn outbound_bytes(&self) -> Vec<u8> {
        match (&self.scope, &self.source) {
            (ReviewScope::Document, ReviewSource::Document { text, .. }) => {
                text.as_bytes().to_vec()
            }
            (ReviewScope::Selection { range, .. }, ReviewSource::Document { text, .. }) => text
                .get(range.start as usize..range.end as usize)
                .map_or_else(Vec::new, |slice| slice.as_bytes().to_vec()),
            (ReviewScope::AgentSkillPackage, ReviewSource::AgentSkillPackage { package }) => {
                package.framed_bytes()
            }
            _ => Vec::new(),
        }
    }

    pub fn outbound_text(&self) -> Option<&str> {
        match (&self.scope, &self.source) {
            (ReviewScope::Document, ReviewSource::Document { text, .. }) => Some(text),
            (ReviewScope::Selection { range, .. }, ReviewSource::Document { text, .. }) => {
                text.get(range.start as usize..range.end as usize)
            }
            _ => None,
        }
    }

    pub fn is_current(&self, snapshot: SourceSnapshot) -> bool {
        self.snapshot.matches(snapshot)
    }
}

/// Inert prose supplied by a model.  It has no rendering or execution API.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct StructuredText(String);

impl StructuredText {
    pub fn new(text: impl Into<String>) -> Result<Self, ReviewValidationError> {
        let text = text.into();
        non_empty(text, "structured text").map(Self)
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn into_string(self) -> String {
        self.0
    }
}

impl From<String> for StructuredText {
    fn from(value: String) -> Self {
        Self(value)
    }
}

impl From<&str> for StructuredText {
    fn from(value: &str) -> Self {
        Self(value.to_owned())
    }
}

impl fmt::Display for StructuredText {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

/// The “What I understand” sections shown before findings and questions.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReviewSections {
    pub stated_goal: StructuredText,
    pub relevant_context: Vec<StructuredText>,
    pub constraints: Vec<StructuredText>,
    pub non_goals: Vec<StructuredText>,
    pub expected_deliverable: StructuredText,
    pub success_evidence: Vec<StructuredText>,
    pub inferred_assumptions: Vec<StructuredText>,
    pub unresolved_decisions: Vec<StructuredText>,
}

impl ReviewSections {
    pub fn validate(&self) -> Result<(), ReviewValidationError> {
        validate_text(&self.stated_goal)?;
        validate_text(&self.expected_deliverable)?;
        for list in [
            &self.relevant_context,
            &self.constraints,
            &self.non_goals,
            &self.success_evidence,
            &self.inferred_assumptions,
            &self.unresolved_decisions,
        ] {
            for text in list.iter() {
                validate_text(text)?;
            }
        }
        Ok(())
    }
}

/// Whether a finding is directly stated by the source or inferred by the
/// reviewer.  The distinction is explicit so the UI cannot collapse the two.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FindingKind {
    Source,
    #[serde(alias = "source_statement")]
    SourceStatement,
    Inference,
}

/// One grounded, non-mutating Review finding.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Finding {
    pub kind: FindingKind,
    pub text: StructuredText,
    pub anchor: SourceAnchor,
}

impl Finding {
    pub fn source(text: impl Into<StructuredText>, anchor: SourceAnchor) -> Self {
        Self {
            kind: FindingKind::Source,
            text: text.into(),
            anchor,
        }
    }

    pub fn inference(text: impl Into<StructuredText>, anchor: SourceAnchor) -> Self {
        Self {
            kind: FindingKind::Inference,
            text: text.into(),
            anchor,
        }
    }
}

/// Explicit question priority, retained in the structured result.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ClarificationPriority {
    Critical,
    High,
    Medium,
    Low,
}

/// One question whose answer could materially change the requested outcome.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ClarificationQuestion {
    pub question: StructuredText,
    pub priority: ClarificationPriority,
    #[serde(deserialize_with = "deserialize_required_option")]
    pub impact: Option<StructuredText>,
}

impl ClarificationQuestion {
    pub fn new(question: impl Into<StructuredText>, priority: ClarificationPriority) -> Self {
        Self {
            question: question.into(),
            priority,
            impact: None,
        }
    }

    pub fn with_impact(mut self, impact: impl Into<StructuredText>) -> Self {
        self.impact = Some(impact.into());
        self
    }
}

fn deserialize_required_option<'de, D, T>(deserializer: D) -> Result<Option<T>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Deserialize<'de>,
{
    Option::deserialize(deserializer)
}

/// Strict provider response schema.  It contains no status, mutation, apply,
/// tool, or UI-action field; those belong to the application boundary.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ReviewModelOutput {
    pub schema_version: String,
    pub scope: ReviewScope,
    pub understood_intent: ReviewSections,
    pub findings: Vec<Finding>,
    pub clarification_questions: Vec<ClarificationQuestion>,
}

impl<'de> Deserialize<'de> for ReviewModelOutput {
    fn deserialize<D>(_: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        Err(serde::de::Error::custom(
            "ReviewModelOutput must be decoded with its frozen ReviewRequest",
        ))
    }
}

impl ReviewModelOutput {
    /// Alias matching the terminology used by the application boundary.
    pub fn decode(input: &str, request: &ReviewRequest) -> Result<Self, ReviewDecodeError> {
        ReviewModelOutputWire::decode_json(input)?
            .into_output(request)
            .map_err(ReviewDecodeError::Validation)
    }

    /// Decode the provider wire format against its frozen source request.
    ///
    /// Provider locations are quotes, never direct byte or line positions;
    /// the returned anchor is derived locally from `request`.
    pub fn decode_json(input: &str, request: &ReviewRequest) -> Result<Self, ReviewDecodeError> {
        Self::decode(input, request)
    }

    pub fn validate_shape(&self) -> Result<(), ReviewValidationError> {
        if self.schema_version != REVIEW_SCHEMA_VERSION {
            return Err(ReviewValidationError::UnsupportedSchemaVersion(
                self.schema_version.clone(),
            ));
        }
        self.understood_intent.validate()?;
        if self.clarification_questions.len() > MAX_CLARIFICATION_QUESTIONS {
            return Err(ReviewValidationError::TooManyClarificationQuestions {
                count: self.clarification_questions.len(),
                max: MAX_CLARIFICATION_QUESTIONS,
            });
        }
        for finding in &self.findings {
            validate_text(&finding.text)?;
        }
        for question in &self.clarification_questions {
            validate_text(&question.question)?;
            if let Some(impact) = &question.impact {
                validate_text(impact)?;
            }
        }
        if !self.scope.missing_context() && matches!(self.scope, ReviewScope::Selection { .. }) {
            return Err(ReviewValidationError::SelectionContextNotMarkedMissing);
        }
        Ok(())
    }

    /// Full validation against the frozen request.  A response is not
    /// renderable until this check succeeds.
    pub fn validate_against(&self, request: &ReviewRequest) -> Result<(), ReviewValidationError> {
        request.validate()?;
        self.validate_shape()?;
        if self.scope != request.scope {
            return Err(ReviewValidationError::ResultScopeMismatch);
        }
        match (&request.source, &request.scope) {
            (ReviewSource::Document { text, .. }, ReviewScope::Document) => {
                for finding in &self.findings {
                    validate_document_anchor(&finding.anchor, text, None)?;
                }
            }
            (ReviewSource::Document { text, .. }, ReviewScope::Selection { range, .. }) => {
                for finding in &self.findings {
                    validate_document_anchor(&finding.anchor, text, Some(*range))?;
                }
            }
            (ReviewSource::AgentSkillPackage { package }, ReviewScope::AgentSkillPackage) => {
                for finding in &self.findings {
                    validate_package_anchor(&finding.anchor, package)?;
                }
            }
            _ => return Err(ReviewValidationError::ScopeSourceMismatch),
        }
        Ok(())
    }

    pub fn sections(&self) -> &ReviewSections {
        &self.understood_intent
    }
}

/// The untrusted provider wire shape. Provider locations are deliberately not
/// trusted: every localized finding supplies a unique source quote, and
/// the immutable request derives the renderable byte range locally.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ReviewModelOutputWire {
    schema_version: String,
    scope: ReviewScope,
    understood_intent: ReviewSections,
    findings: Vec<ReviewFindingWire>,
    clarification_questions: Vec<ClarificationQuestion>,
}

impl ReviewModelOutputWire {
    fn decode_json(input: &str) -> Result<Self, ReviewDecodeError> {
        let output: Self = serde_json::from_str(input)
            .map_err(|error| ReviewDecodeError::Serde(error.to_string()))?;
        output
            .validate_shape()
            .map_err(ReviewDecodeError::Validation)?;
        Ok(output)
    }

    fn validate_shape(&self) -> Result<(), ReviewValidationError> {
        if self.schema_version != REVIEW_SCHEMA_VERSION {
            return Err(ReviewValidationError::UnsupportedSchemaVersion(
                self.schema_version.clone(),
            ));
        }
        self.understood_intent.validate()?;
        if self.clarification_questions.len() > MAX_CLARIFICATION_QUESTIONS {
            return Err(ReviewValidationError::TooManyClarificationQuestions {
                count: self.clarification_questions.len(),
                max: MAX_CLARIFICATION_QUESTIONS,
            });
        }
        for finding in &self.findings {
            finding.validate()?;
        }
        for question in &self.clarification_questions {
            validate_text(&question.question)?;
            if let Some(impact) = &question.impact {
                validate_text(impact)?;
            }
        }
        if !self.scope.missing_context() && matches!(self.scope, ReviewScope::Selection { .. }) {
            return Err(ReviewValidationError::SelectionContextNotMarkedMissing);
        }
        Ok(())
    }

    fn into_output(
        self,
        request: &ReviewRequest,
    ) -> Result<ReviewModelOutput, ReviewValidationError> {
        request.validate()?;
        self.validate_shape()?;
        if self.scope != request.scope {
            return Err(ReviewValidationError::ResultScopeMismatch);
        }
        let findings = self
            .findings
            .into_iter()
            .map(|finding| finding.into_finding(request))
            .collect::<Result<Vec<_>, _>>()?;
        let output = ReviewModelOutput {
            schema_version: self.schema_version,
            scope: self.scope,
            understood_intent: self.understood_intent,
            findings,
            clarification_questions: self.clarification_questions,
        };
        output.validate_against(request)?;
        Ok(output)
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ReviewFindingWire {
    kind: FindingKind,
    text: StructuredText,
    anchor: ReviewAnchorWire,
}

impl ReviewFindingWire {
    fn validate(&self) -> Result<(), ReviewValidationError> {
        if self.kind == FindingKind::Inference {
            validate_text(&self.text)?;
        }
        self.anchor.validate()
    }

    fn into_finding(self, request: &ReviewRequest) -> Result<Finding, ReviewValidationError> {
        let Self { kind, text, anchor } = self;
        let resolved_anchor = anchor.resolve(request)?;
        let text = match kind {
            FindingKind::Source | FindingKind::SourceStatement => resolved_anchor
                .source_quote
                .ok_or(ReviewValidationError::SourceFindingRequiresQuote)?,
            FindingKind::Inference => text,
        };
        Ok(Finding {
            kind,
            text,
            anchor: resolved_anchor.anchor,
        })
    }
}

struct ResolvedReviewAnchor {
    anchor: SourceAnchor,
    source_quote: Option<StructuredText>,
}

struct ResolvedQuote {
    location: SourceLocation,
    source_text: StructuredText,
}

#[derive(Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum ReviewAnchorWire {
    DocumentQuote { quote: StructuredText },
    AgentSkillFileQuote { path: String, quote: StructuredText },
    DocumentWide,
}

impl ReviewAnchorWire {
    fn validate(&self) -> Result<(), ReviewValidationError> {
        match self {
            Self::DocumentQuote { quote } | Self::AgentSkillFileQuote { quote, .. } => {
                validate_text(quote)
            }
            Self::DocumentWide => Ok(()),
        }
    }

    fn resolve(
        self,
        request: &ReviewRequest,
    ) -> Result<ResolvedReviewAnchor, ReviewValidationError> {
        match (self, &request.source, request.scope) {
            (
                Self::DocumentQuote { quote },
                ReviewSource::Document { text, .. },
                ReviewScope::Document,
            ) => {
                let resolved = unique_quote_match(text, quote.as_str(), 0)?;
                Ok(ResolvedReviewAnchor {
                    anchor: SourceAnchor::document(resolved.location),
                    source_quote: Some(resolved.source_text),
                })
            }
            (
                Self::DocumentQuote { quote },
                ReviewSource::Document { text, .. },
                ReviewScope::Selection { range, .. },
            ) => {
                let start = usize::try_from(range.start).map_err(|_| {
                    ReviewValidationError::RangeOutsideSource {
                        start: range.start,
                        end: range.end,
                        source_len: text.len() as u64,
                    }
                })?;
                let end = usize::try_from(range.end).map_err(|_| {
                    ReviewValidationError::RangeOutsideSource {
                        start: range.start,
                        end: range.end,
                        source_len: text.len() as u64,
                    }
                })?;
                let resolved = unique_quote_match(&text[start..end], quote.as_str(), range.start)?;
                Ok(ResolvedReviewAnchor {
                    anchor: SourceAnchor::document(resolved.location),
                    source_quote: Some(resolved.source_text),
                })
            }
            (
                Self::AgentSkillFileQuote { path, quote },
                ReviewSource::AgentSkillPackage { package },
                ReviewScope::AgentSkillPackage,
            ) => {
                let normalized = normalize_relative_path(&path)?;
                if normalized != path {
                    return Err(ReviewValidationError::AnchorPathNotNormalized);
                }
                let file = package
                    .files()
                    .iter()
                    .find(|file| file.path == path)
                    .ok_or_else(|| ReviewValidationError::AnchorPathNotInPackage(path.clone()))?;
                let SkillFilePayload::Utf8 { content } = &file.payload else {
                    return Err(ReviewValidationError::AnchorOutsideSource);
                };
                let resolved = unique_quote_match(content, quote.as_str(), 0)?;
                Ok(ResolvedReviewAnchor {
                    anchor: SourceAnchor::agent_skill_file(path, resolved.location)?,
                    source_quote: Some(resolved.source_text),
                })
            }
            (Self::DocumentWide, ReviewSource::Document { .. }, ReviewScope::Document) => {
                Ok(ResolvedReviewAnchor {
                    anchor: SourceAnchor::DocumentWide,
                    source_quote: None,
                })
            }
            (Self::DocumentWide, ReviewSource::Document { .. }, ReviewScope::Selection { .. }) => {
                Err(ReviewValidationError::AnchorOutsideSelection)
            }
            (
                Self::DocumentWide,
                ReviewSource::AgentSkillPackage { .. },
                ReviewScope::AgentSkillPackage,
            ) => Err(ReviewValidationError::AnchorOutsideSource),
            _ => Err(ReviewValidationError::ScopeSourceMismatch),
        }
    }
}

/// Application-owned lifecycle state for a Review result.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReviewStatus {
    Ready,
    Stale,
    Diagnostic,
}

impl ReviewStatus {
    pub const fn is_stale(self) -> bool {
        matches!(self, Self::Stale)
    }

    pub const fn is_diagnostic(self) -> bool {
        matches!(self, Self::Diagnostic)
    }
}

/// Diagnostics are status, not model content.  They are safe to show when no
/// provider is configured or a response is cancelled, timed out, malformed,
/// oversized, or unavailable.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReviewDiagnosticCode {
    NoProvider,
    Cancelled,
    Timeout,
    MalformedResponse,
    OversizedPayload,
    Unavailable,
    EmptySelection,
    InvalidRequest,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReviewDiagnostic {
    pub code: ReviewDiagnosticCode,
    pub message: StructuredText,
}

impl ReviewDiagnostic {
    pub fn new(code: ReviewDiagnosticCode, message: impl Into<StructuredText>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }
}

/// The rendered result envelope.  Stale results retain their inert structured
/// output for inspection, but callers must not present them as current.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ReviewResult {
    pub status: ReviewStatus,
    pub snapshot: SourceSnapshot,
    pub output: Option<ReviewModelOutput>,
    pub diagnostic: Option<ReviewDiagnostic>,
}

impl<'de> Deserialize<'de> for ReviewResult {
    fn deserialize<D>(_: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        Err(serde::de::Error::custom(
            "ReviewResult must be constructed from a validated ReviewRequest",
        ))
    }
}

impl ReviewResult {
    pub fn ready(
        request: &ReviewRequest,
        output: ReviewModelOutput,
    ) -> Result<Self, ReviewValidationError> {
        output.validate_against(request)?;
        Ok(Self {
            status: ReviewStatus::Ready,
            snapshot: request.snapshot,
            output: Some(output),
            diagnostic: None,
        })
    }

    pub fn stale(
        request: &ReviewRequest,
        output: ReviewModelOutput,
    ) -> Result<Self, ReviewValidationError> {
        output.validate_against(request)?;
        Ok(Self {
            status: ReviewStatus::Stale,
            snapshot: request.snapshot,
            output: Some(output),
            diagnostic: None,
        })
    }

    pub fn diagnostic(snapshot: SourceSnapshot, diagnostic: ReviewDiagnostic) -> Self {
        Self {
            status: ReviewStatus::Diagnostic,
            snapshot,
            output: None,
            diagnostic: Some(diagnostic),
        }
    }

    /// Decode a provider response with full-schema validation.  It is
    /// intentionally impossible to obtain a partially trusted output.
    pub fn decode_json(
        input: &str,
        request: &ReviewRequest,
    ) -> Result<ReviewModelOutput, ReviewDecodeError> {
        ReviewModelOutput::decode_json(input, request)
    }

    pub fn validate(&self, request: &ReviewRequest) -> Result<(), ReviewValidationError> {
        match (self.status, &self.output, &self.diagnostic) {
            (ReviewStatus::Ready | ReviewStatus::Stale, Some(output), None) => {
                output.validate_against(request)
            }
            (ReviewStatus::Diagnostic, None, Some(diagnostic)) => {
                validate_text(&diagnostic.message)
            }
            _ => Err(ReviewValidationError::InvalidResultState),
        }
    }

    pub fn is_stale_against(&self, current: SourceSnapshot) -> bool {
        !self.snapshot.matches(current)
    }

    /// Retain the same immutable output while changing only its application
    /// status when a newer source snapshot is observed.
    pub fn with_current_snapshot(mut self, current: SourceSnapshot) -> Self {
        if self.status != ReviewStatus::Diagnostic && !self.snapshot.matches(current) {
            self.status = ReviewStatus::Stale;
        }
        self
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReviewDecodeError {
    Serde(String),
    Validation(ReviewValidationError),
}

impl fmt::Display for ReviewDecodeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Serde(message) => write!(formatter, "invalid Review JSON: {message}"),
            Self::Validation(error) => error.fmt(formatter),
        }
    }
}

impl std::error::Error for ReviewDecodeError {}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReviewValidationError {
    EmptyText(&'static str),
    InvalidByteRange {
        start: u64,
        end: u64,
    },
    RangeOutsideSource {
        start: u64,
        end: u64,
        source_len: u64,
    },
    RangeNotUtf8Boundary {
        start: u64,
        end: u64,
    },
    InvalidLineRange {
        start: u64,
        end: u64,
    },
    LineOutsideSource {
        start: u64,
        end: u64,
        last_line: u64,
    },
    AnchorOutsideSelection,
    AnchorOutsideSource,
    AnchorQuoteAbsent,
    AnchorQuoteAmbiguous,
    SourceFindingRequiresQuote,
    AnchorPathNotInPackage(String),
    AnchorPathNotNormalized,
    SelectionContextNotMarkedMissing,
    ScopeSourceMismatch,
    LensScopeMismatch,
    ResultScopeMismatch,
    InvalidSkillPackage(String),
    UnsupportedSchemaVersion(String),
    TooManyClarificationQuestions {
        count: usize,
        max: usize,
    },
    InvalidResultState,
}

impl fmt::Display for ReviewValidationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EmptyText(field) => write!(formatter, "{field} must not be empty"),
            Self::InvalidByteRange { start, end } => {
                write!(formatter, "invalid byte range {start}..{end}")
            }
            Self::RangeOutsideSource {
                start,
                end,
                source_len,
            } => write!(
                formatter,
                "byte range {start}..{end} exceeds source length {source_len}"
            ),
            Self::RangeNotUtf8Boundary { start, end } => {
                write!(
                    formatter,
                    "byte range {start}..{end} is not on UTF-8 boundaries"
                )
            }
            Self::InvalidLineRange { start, end } => {
                write!(formatter, "invalid 1-based line range {start}..{end}")
            }
            Self::LineOutsideSource {
                start,
                end,
                last_line,
            } => write!(
                formatter,
                "line range {start}..{end} exceeds last source line {last_line}"
            ),
            Self::AnchorOutsideSelection => {
                formatter.write_str("source anchor falls outside the selected range")
            }
            Self::AnchorOutsideSource => formatter.write_str("source anchor is outside the source"),
            Self::AnchorQuoteAbsent => {
                formatter.write_str("source anchor quote is absent from the frozen source")
            }
            Self::AnchorQuoteAmbiguous => {
                formatter.write_str("source anchor quote is not unique in the frozen source")
            }
            Self::SourceFindingRequiresQuote => {
                formatter.write_str("source findings require a locally resolved source quote")
            }
            Self::AnchorPathNotInPackage(path) => {
                write!(
                    formatter,
                    "source anchor path is not in the Agent Skill package: {path}"
                )
            }
            Self::AnchorPathNotNormalized => {
                formatter.write_str("source anchor path is not normalized")
            }
            Self::SelectionContextNotMarkedMissing => formatter
                .write_str("selection Review must mark surrounding document context as missing"),
            Self::ScopeSourceMismatch => {
                formatter.write_str("Review scope and source do not match")
            }
            Self::LensScopeMismatch => {
                formatter.write_str("artifact lens and Review scope do not match")
            }
            Self::ResultScopeMismatch => {
                formatter.write_str("result scope does not match request scope")
            }
            Self::InvalidSkillPackage(message) => {
                write!(formatter, "invalid Agent Skill package: {message}")
            }
            Self::UnsupportedSchemaVersion(version) => {
                write!(formatter, "unsupported Review schema version: {version}")
            }
            Self::TooManyClarificationQuestions { count, max } => {
                write!(
                    formatter,
                    "Review contains {count} clarification questions; maximum is {max}"
                )
            }
            Self::InvalidResultState => formatter.write_str("invalid Review result status payload"),
        }
    }
}

impl std::error::Error for ReviewValidationError {}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SkillPackageError {
    Validation(ReviewValidationError),
    InvalidRelativePath,
    EmptyInclusionReason,
    EmptyOmissionReason,
    DuplicatePath,
    ByteSizeOverflow,
    FileTooLarge {
        path: String,
        byte_size: u64,
        limit: u64,
    },
    PackageTooLarge {
        byte_size: u64,
        limit: u64,
    },
    PayloadSizeMismatch {
        path: String,
        declared: u64,
        actual: u64,
    },
    InvalidSha256,
}

impl fmt::Display for SkillPackageError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Validation(error) => error.fmt(formatter),
            Self::InvalidRelativePath => {
                formatter.write_str("Agent Skill path must be relative and normalized")
            }
            Self::EmptyInclusionReason => {
                formatter.write_str("Agent Skill file inclusion reason is empty")
            }
            Self::EmptyOmissionReason => {
                formatter.write_str("Agent Skill omission reason is empty")
            }
            Self::DuplicatePath => formatter.write_str("Agent Skill package paths must be unique"),
            Self::ByteSizeOverflow => {
                formatter.write_str("Agent Skill package byte size overflowed")
            }
            Self::FileTooLarge {
                path,
                byte_size,
                limit,
            } => {
                write!(
                    formatter,
                    "Agent Skill file {path} is {byte_size} bytes; limit is {limit}"
                )
            }
            Self::PackageTooLarge { byte_size, limit } => {
                write!(
                    formatter,
                    "Agent Skill package is {byte_size} bytes; limit is {limit}"
                )
            }
            Self::PayloadSizeMismatch {
                path,
                declared,
                actual,
            } => {
                write!(
                    formatter,
                    "payload for {path} declares {declared} bytes but contains {actual}"
                )
            }
            Self::InvalidSha256 => {
                formatter.write_str("binary metadata requires a 64-character SHA-256 hex digest")
            }
        }
    }
}

impl std::error::Error for SkillPackageError {}

fn validate_text(text: &StructuredText) -> Result<(), ReviewValidationError> {
    if text.as_str().trim().is_empty() {
        Err(ReviewValidationError::EmptyText("structured text"))
    } else {
        Ok(())
    }
}

fn non_empty(value: String, field: &'static str) -> Result<String, ReviewValidationError> {
    if value.trim().is_empty() {
        Err(ReviewValidationError::EmptyText(field))
    } else {
        Ok(value)
    }
}

fn valid_sha256(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn normalize_relative_path(raw: &str) -> Result<String, ReviewValidationError> {
    if raw.is_empty() || raw.contains('\0') {
        return Err(ReviewValidationError::AnchorPathNotNormalized);
    }
    let path = raw.replace('\\', "/");
    if path.starts_with('/')
        || path
            .as_bytes()
            .get(1)
            .is_some_and(|character| *character == b':')
    {
        return Err(ReviewValidationError::AnchorPathNotNormalized);
    }
    let mut components = Vec::new();
    for component in path.split('/') {
        match component {
            "" | "." => {}
            ".." => return Err(ReviewValidationError::AnchorPathNotNormalized),
            component => components.push(component),
        }
    }
    if components.is_empty() {
        return Err(ReviewValidationError::AnchorPathNotNormalized);
    }
    Ok(components.join("/"))
}

fn validate_skill_package_entries(
    files: &[SkillPackageFile],
    omissions: &[SkillPackageOmission],
) -> Result<(), SkillPackageError> {
    for file in files {
        file.validate()?;
    }
    for omission in omissions {
        omission.validate()?;
    }
    if has_duplicate_paths(files, omissions) {
        return Err(SkillPackageError::DuplicatePath);
    }
    let total_byte_size = files.iter().try_fold(0_u64, |total, file| {
        total
            .checked_add(file.byte_size)
            .ok_or(SkillPackageError::ByteSizeOverflow)
    })?;
    if total_byte_size > MAX_SKILL_PACKAGE_BYTES {
        return Err(SkillPackageError::PackageTooLarge {
            byte_size: total_byte_size,
            limit: MAX_SKILL_PACKAGE_BYTES,
        });
    }
    Ok(())
}

fn has_duplicate_paths(files: &[SkillPackageFile], omissions: &[SkillPackageOmission]) -> bool {
    let mut paths = Vec::with_capacity(files.len() + omissions.len());
    paths.extend(files.iter().map(|file| file.path.as_str()));
    paths.extend(omissions.iter().map(|omission| omission.path.as_str()));
    paths.sort_unstable();
    paths.windows(2).any(|pair| pair[0] == pair[1])
}

fn validate_skill_package(package: &SkillPackage) -> Result<(), ReviewValidationError> {
    package
        .validate()
        .map_err(|error| ReviewValidationError::InvalidSkillPackage(error.to_string()))
}

fn validate_document_anchor(
    anchor: &SourceAnchor,
    source: &str,
    allowed: Option<ByteRange>,
) -> Result<(), ReviewValidationError> {
    match anchor {
        SourceAnchor::Document { location } => location.validate(Some(source), allowed),
        SourceAnchor::DocumentWide => {
            if allowed.is_some() {
                Err(ReviewValidationError::AnchorOutsideSelection)
            } else {
                Ok(())
            }
        }
        SourceAnchor::AgentSkillFile { .. } => Err(ReviewValidationError::ScopeSourceMismatch),
    }
}

fn validate_package_anchor(
    anchor: &SourceAnchor,
    package: &SkillPackage,
) -> Result<(), ReviewValidationError> {
    let (path, location): (&String, SourceLocation) = match anchor {
        SourceAnchor::AgentSkillFile { path, location } => (path, *location),
        SourceAnchor::Document { .. } | SourceAnchor::DocumentWide => {
            return Err(ReviewValidationError::AnchorOutsideSource);
        }
    };
    let normalized = normalize_relative_path(path)?;
    if normalized != *path {
        return Err(ReviewValidationError::AnchorPathNotNormalized);
    }
    let file = package
        .files
        .iter()
        .find(|file| file.path == *path)
        .ok_or_else(|| ReviewValidationError::AnchorPathNotInPackage(path.clone()))?;
    match &file.payload {
        SkillFilePayload::Utf8 { content } => location.validate(Some(content), None),
        SkillFilePayload::RawBinary { bytes, .. } => match location {
            SourceLocation::ByteRange { start, end } => {
                ByteRange::new(start, end)?.validate(bytes.len() as u64)
            }
            SourceLocation::LineRange { .. } => Err(ReviewValidationError::AnchorOutsideSource),
        },
        SkillFilePayload::Binary { .. } => Err(ReviewValidationError::AnchorOutsideSource),
    }
}

fn unique_quote_match(
    source: &str,
    quote: &str,
    source_offset: u64,
) -> Result<ResolvedQuote, ReviewValidationError> {
    let exact_candidates = exact_quote_ranges(source, quote);
    let mut candidates = if exact_candidates.is_empty() {
        whitespace_folded_quote_ranges(source, quote)
    } else {
        exact_candidates
    };
    candidates.sort_unstable_by_key(|range| (range.start, range.end));
    candidates.dedup_by(|left, right| left.start == right.start && left.end == right.end);
    let range = match candidates.as_slice() {
        [] => return Err(ReviewValidationError::AnchorQuoteAbsent),
        [range] => range.clone(),
        _ => return Err(ReviewValidationError::AnchorQuoteAmbiguous),
    };
    let source_text = StructuredText::from(&source[range.clone()]);
    let range = contextual_anchor_range(source, range);
    let start = range.start;
    let end = range.end;
    let start = source_offset
        .checked_add(start as u64)
        .ok_or(ReviewValidationError::AnchorOutsideSource)?;
    let end = start
        .checked_add((end - range.start) as u64)
        .ok_or(ReviewValidationError::AnchorOutsideSource)?;
    Ok(ResolvedQuote {
        location: SourceLocation::bytes(start, end),
        source_text,
    })
}

fn contextual_anchor_range(source: &str, quote: std::ops::Range<usize>) -> std::ops::Range<usize> {
    const MAX_CONTEXT_BYTES: usize = 4 * 1024;

    let before = source[..quote.start].rfind("\n\n").map(|index| index + 2);
    let after = source[quote.end..]
        .find("\n\n")
        .map(|index| quote.end + index);
    if before.is_none() && after.is_none() {
        return quote;
    }
    let start = before.unwrap_or(0);
    let end = after.unwrap_or(source.len());
    if end - start <= MAX_CONTEXT_BYTES {
        start..end
    } else {
        quote
    }
}

fn exact_quote_ranges(source: &str, quote: &str) -> Vec<std::ops::Range<usize>> {
    quote_match_starts(source, quote)
        .map(|start| start..start + quote.len())
        .collect()
}

/// Match an anchor quote that preserves every non-whitespace byte but has
/// collapsed each source whitespace run to one space. This accounts for model
/// formatting of line breaks without accepting punctuation, Markdown, or text
/// changes. Ambiguous matches remain invalid.
fn whitespace_folded_quote_ranges(source: &str, quote: &str) -> Vec<std::ops::Range<usize>> {
    let segments: Vec<&str> = quote.split_whitespace().collect();
    if segments.len() < 2 {
        return Vec::new();
    }
    let mut candidates = Vec::new();
    for start in quote_match_starts(source, segments[0]) {
        let mut cursor = start + segments[0].len();
        let mut matched = true;
        for segment in &segments[1..] {
            let whitespace_start = cursor;
            while cursor < source.len() {
                let Some(character) = source[cursor..].chars().next() else {
                    break;
                };
                if !character.is_whitespace() {
                    break;
                }
                cursor += character.len_utf8();
            }
            if cursor == whitespace_start || !source[cursor..].starts_with(segment) {
                matched = false;
                break;
            }
            cursor += segment.len();
        }
        if matched {
            candidates.push(start..cursor);
        }
    }
    candidates
}

fn quote_match_starts<'a>(source: &'a str, quote: &'a str) -> impl Iterator<Item = usize> + 'a {
    source
        .char_indices()
        .map(|(start, _)| start)
        .filter(move |&start| source[start..].starts_with(quote))
}

fn line_count(source: &str) -> u64 {
    source.lines().count().max(1) as u64
}

fn line_range_for_bytes(source: &str, range: ByteRange) -> std::ops::RangeInclusive<u64> {
    let start = range.start.min(source.len() as u64) as usize;
    let end = range.end.min(source.len() as u64) as usize;
    let first = source.as_bytes()[..start]
        .iter()
        .filter(|byte| **byte == b'\n')
        .count() as u64
        + 1;
    // `end` is exclusive. A range ending immediately after a line terminator
    // contains the terminator but not the following line.
    let last_end = if start == end { end } else { end - 1 };
    let last = source.as_bytes()[..last_end]
        .iter()
        .filter(|byte| **byte == b'\n')
        .count() as u64
        + 1;
    first..=last
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sections() -> ReviewSections {
        ReviewSections {
            stated_goal: "make the request testable".into(),
            relevant_context: vec!["the document is a prompt".into()],
            constraints: vec!["do not mutate source".into()],
            non_goals: vec!["do not run the prompt".into()],
            expected_deliverable: "a bounded implementation".into(),
            success_evidence: vec!["focused tests pass".into()],
            inferred_assumptions: vec!["the caller owns provider setup".into()],
            unresolved_decisions: vec!["which model is configured".into()],
        }
    }

    fn output(scope: ReviewScope, anchor: SourceAnchor) -> ReviewModelOutput {
        ReviewModelOutput {
            schema_version: REVIEW_SCHEMA_VERSION.to_owned(),
            scope,
            understood_intent: sections(),
            findings: vec![Finding::source("the source says this", anchor)],
            clarification_questions: vec![ClarificationQuestion::new(
                "Which target should be prioritized?",
                ClarificationPriority::High,
            )],
        }
    }

    fn provider_document_finding(
        scope: ReviewScope,
        kind: &str,
        text: &str,
        anchor: serde_json::Value,
    ) -> serde_json::Value {
        serde_json::json!({
            "schema_version": REVIEW_SCHEMA_VERSION,
            "scope": scope,
            "understood_intent": {
                "stated_goal": "review the source",
                "relevant_context": [],
                "constraints": [],
                "non_goals": [],
                "expected_deliverable": "a review",
                "success_evidence": [],
                "inferred_assumptions": [],
                "unresolved_decisions": []
            },
            "findings": [{
                "kind": kind,
                "text": text,
                "anchor": anchor
            }],
            "clarification_questions": []
        })
    }

    #[test]
    fn selection_request_marks_missing_context_and_freezes_bytes() {
        let text = "prefix\n中文 selection\nsuffix";
        let start = text.find("中文").unwrap() as u64;
        let end = (text.find("selection").unwrap() + "selection".len()) as u64;
        let request = ReviewRequest::selection(
            ArtifactLens::Prompt,
            text,
            ByteRange::new(start, end).unwrap(),
            SourceSnapshot::new(7, 2),
        )
        .unwrap();
        assert!(request.scope.missing_context());
        assert_eq!(request.outbound_text(), Some("中文 selection"));
    }

    #[test]
    fn result_rejects_anchor_outside_selection() {
        let text = "before\nselected\nafter";
        let start = text.find("selected").unwrap() as u64;
        let end = start + "selected".len() as u64;
        let request = ReviewRequest::selection(
            ArtifactLens::Prompt,
            text,
            ByteRange::new(start, end).unwrap(),
            SourceSnapshot::default(),
        )
        .unwrap();
        let result = output(
            request.scope,
            SourceAnchor::document(SourceLocation::bytes(0, 6)),
        );
        assert!(matches!(
            result.validate_against(&request),
            Err(ReviewValidationError::AnchorOutsideSelection)
        ));
    }

    #[test]
    fn selection_ending_at_a_newline_does_not_include_the_next_line() {
        let request = ReviewRequest::selection(
            ArtifactLens::Prompt,
            "a\nb",
            ByteRange::new(0, 2).unwrap(),
            SourceSnapshot::default(),
        )
        .unwrap();
        let result = output(
            request.scope,
            SourceAnchor::document(SourceLocation::lines(2, 2)),
        );

        assert!(matches!(
            result.validate_against(&request),
            Err(ReviewValidationError::AnchorOutsideSelection)
        ));
    }

    #[test]
    fn output_decode_is_strict_and_caps_questions() {
        let request =
            ReviewRequest::document(ArtifactLens::Prompt, "source", SourceSnapshot::default())
                .unwrap();
        let json = serde_json::json!({
            "schema_version": REVIEW_SCHEMA_VERSION,
            "scope": {"kind": "document"},
            "understood_intent": {
                "stated_goal": "review the source",
                "relevant_context": [],
                "constraints": [],
                "non_goals": [],
                "expected_deliverable": "a review",
                "success_evidence": [],
                "inferred_assumptions": [],
                "unresolved_decisions": []
            },
            "findings": [{
                "kind": "source",
                "text": "the evidence is present",
                "anchor": {"kind": "document_quote", "quote": "source"}
            }],
            "clarification_questions": [{
                "question": "Which target should be prioritized?",
                "priority": "high",
                "impact": null
            }]
        });
        let decoded = ReviewModelOutput::decode_json(&json.to_string(), &request).unwrap();
        assert_eq!(decoded.clarification_questions.len(), 1);

        let mut value = json.clone();
        value["unexpected"] = serde_json::json!(true);
        assert!(ReviewModelOutput::decode_json(&value.to_string(), &request).is_err());

        let mut aliased_field = json.clone();
        let stated_goal = aliased_field["understood_intent"]
            .as_object_mut()
            .unwrap()
            .remove("stated_goal")
            .unwrap();
        aliased_field["understood_intent"]["goal"] = stated_goal;
        assert!(ReviewModelOutput::decode_json(&aliased_field.to_string(), &request).is_err());

        let mut missing_nullable_field = json.clone();
        missing_nullable_field["clarification_questions"][0]
            .as_object_mut()
            .unwrap()
            .remove("impact");
        assert!(
            ReviewModelOutput::decode_json(&missing_nullable_field.to_string(), &request).is_err()
        );

        let mut unsupported_anchor = json;
        unsupported_anchor["findings"][0]["anchor"] = serde_json::json!({
            "kind": "document_range",
            "start_byte": 0,
            "end_byte": 1,
        });
        assert!(ReviewModelOutput::decode_json(&unsupported_anchor.to_string(), &request).is_err());

        let mut too_many = output(
            ReviewScope::Document,
            SourceAnchor::document(SourceLocation::bytes(0, 1)),
        );
        too_many.clarification_questions = (0..=MAX_CLARIFICATION_QUESTIONS)
            .map(|_| ClarificationQuestion::new("question", ClarificationPriority::Low))
            .collect();
        assert!(matches!(
            too_many.validate_shape(),
            Err(ReviewValidationError::TooManyClarificationQuestions { .. })
        ));
    }

    #[test]
    fn direct_serde_decode_rejects_renderable_review_values() {
        let request =
            ReviewRequest::document(ArtifactLens::Prompt, "source", SourceSnapshot::default())
                .unwrap();
        let output = output(
            ReviewScope::Document,
            SourceAnchor::document(SourceLocation::bytes(0, 6)),
        );
        let output_json = serde_json::to_string(&output).unwrap();
        assert!(serde_json::from_str::<ReviewModelOutput>(&output_json).is_err());

        let result = ReviewResult::ready(&request, output).unwrap();
        let result_json = serde_json::to_string(&result).unwrap();
        assert!(serde_json::from_str::<ReviewResult>(&result_json).is_err());
    }

    #[test]
    fn provider_quote_anchors_are_derived_from_the_frozen_source() {
        let source = "before\nunique\nquoted evidence\nafter";
        let request =
            ReviewRequest::document(ArtifactLens::Prompt, source, SourceSnapshot::default())
                .unwrap();
        let response = serde_json::json!({
            "schema_version": REVIEW_SCHEMA_VERSION,
            "scope": {"kind": "document"},
            "understood_intent": {
                "stated_goal": "review the source",
                "relevant_context": [],
                "constraints": [],
                "non_goals": [],
                "expected_deliverable": "a review",
                "success_evidence": [],
                "inferred_assumptions": [],
                "unresolved_decisions": []
            },
            "findings": [{
                "kind": "source",
                "text": "the evidence is present",
                "anchor": {"kind": "document_quote", "quote": "unique quoted evidence"}
            }],
            "clarification_questions": []
        });

        let decoded = ReviewModelOutput::decode(&response.to_string(), &request).unwrap();
        let start = source.find("unique").unwrap() as u64;
        assert_eq!(
            decoded.findings[0].anchor,
            SourceAnchor::document(SourceLocation::bytes(
                start,
                start + "unique\nquoted evidence".len() as u64
            ))
        );
        assert_eq!(decoded.findings[0].text.as_str(), "unique\nquoted evidence");
    }

    #[test]
    fn source_finding_discards_provider_paraphrase_for_the_frozen_quote() {
        let request = ReviewRequest::document(
            ArtifactLens::Prompt,
            "actual source statement",
            SourceSnapshot::default(),
        )
        .unwrap();
        let response = provider_document_finding(
            request.scope,
            "source",
            "provider paraphrase",
            serde_json::json!({
                "kind": "document_quote",
                "quote": "actual source statement"
            }),
        );

        let decoded = ReviewModelOutput::decode(&response.to_string(), &request).unwrap();
        assert_eq!(decoded.findings[0].text.as_str(), "actual source statement");
    }

    #[test]
    fn source_statement_finding_recovers_original_folded_whitespace() {
        let request = ReviewRequest::document(
            ArtifactLens::Prompt,
            "actual\n  source statement",
            SourceSnapshot::default(),
        )
        .unwrap();
        let response = provider_document_finding(
            request.scope,
            "source_statement",
            "normalized by provider",
            serde_json::json!({
                "kind": "document_quote",
                "quote": "actual source statement"
            }),
        );

        let decoded = ReviewModelOutput::decode(&response.to_string(), &request).unwrap();
        assert_eq!(
            decoded.findings[0].text.as_str(),
            "actual\n  source statement"
        );
    }

    #[test]
    fn inference_finding_keeps_provider_text() {
        let request = ReviewRequest::document(
            ArtifactLens::Prompt,
            "actual source statement",
            SourceSnapshot::default(),
        )
        .unwrap();
        let response = provider_document_finding(
            request.scope,
            "inference",
            "provider inference",
            serde_json::json!({
                "kind": "document_quote",
                "quote": "actual source statement"
            }),
        );

        let decoded = ReviewModelOutput::decode(&response.to_string(), &request).unwrap();
        assert_eq!(decoded.findings[0].text.as_str(), "provider inference");
    }

    #[test]
    fn source_finding_rejects_document_wide_anchor() {
        let request = ReviewRequest::document(
            ArtifactLens::Prompt,
            "actual source statement",
            SourceSnapshot::default(),
        )
        .unwrap();
        let response = provider_document_finding(
            request.scope,
            "source",
            "provider paraphrase",
            serde_json::json!({"kind": "document_wide"}),
        );

        assert!(matches!(
            ReviewModelOutput::decode(&response.to_string(), &request),
            Err(ReviewDecodeError::Validation(
                ReviewValidationError::SourceFindingRequiresQuote
            ))
        ));
    }

    #[test]
    fn provider_quote_anchors_include_their_bounded_source_paragraph() {
        let source = "before\n\ncontext before proof\nunique proof\ncontext after proof\n\nafter";
        let request =
            ReviewRequest::document(ArtifactLens::Prompt, source, SourceSnapshot::default())
                .unwrap();
        let response = serde_json::json!({
            "schema_version": REVIEW_SCHEMA_VERSION,
            "scope": {"kind": "document"},
            "understood_intent": {
                "stated_goal": "review the source",
                "relevant_context": [],
                "constraints": [],
                "non_goals": [],
                "expected_deliverable": "a review",
                "success_evidence": [],
                "inferred_assumptions": [],
                "unresolved_decisions": []
            },
            "findings": [{
                "kind": "source",
                "text": "the proof is present",
                "anchor": {"kind": "document_quote", "quote": "unique proof"}
            }],
            "clarification_questions": []
        });

        let decoded = ReviewModelOutput::decode(&response.to_string(), &request).unwrap();
        let start = source.find("context before proof").unwrap() as u64;
        let end = source.find("\n\nafter").unwrap() as u64;
        assert_eq!(
            decoded.findings[0].anchor,
            SourceAnchor::document(SourceLocation::bytes(start, end))
        );
    }

    #[test]
    fn provider_quote_anchors_reject_absent_ambiguous_and_outside_selection_quotes() {
        let source = "repeat\nselected only\nrepeat";
        let document =
            ReviewRequest::document(ArtifactLens::Prompt, source, SourceSnapshot::default())
                .unwrap();
        let selection_start = source.find("selected").unwrap() as u64;
        let selection = ReviewRequest::selection(
            ArtifactLens::Prompt,
            source,
            ByteRange::new(
                selection_start,
                selection_start + "selected only".len() as u64,
            )
            .unwrap(),
            SourceSnapshot::default(),
        )
        .unwrap();
        let wire = |scope: ReviewScope, quote: &str| {
            serde_json::json!({
                "schema_version": REVIEW_SCHEMA_VERSION,
                "scope": scope,
                "understood_intent": {
                    "stated_goal": "review the source",
                    "relevant_context": [],
                    "constraints": [],
                    "non_goals": [],
                    "expected_deliverable": "a review",
                    "success_evidence": [],
                    "inferred_assumptions": [],
                    "unresolved_decisions": []
                },
                "findings": [{
                    "kind": "source",
                    "text": "the evidence is present",
                    "anchor": {"kind": "document_quote", "quote": quote}
                }],
                "clarification_questions": []
            })
        };

        assert!(matches!(
            ReviewModelOutput::decode(&wire(document.scope, "missing").to_string(), &document),
            Err(ReviewDecodeError::Validation(
                ReviewValidationError::AnchorQuoteAbsent
            ))
        ));
        assert!(matches!(
            ReviewModelOutput::decode(&wire(document.scope, "repeat").to_string(), &document),
            Err(ReviewDecodeError::Validation(
                ReviewValidationError::AnchorQuoteAmbiguous
            ))
        ));
        assert!(matches!(
            ReviewModelOutput::decode(&wire(selection.scope, "repeat").to_string(), &selection),
            Err(ReviewDecodeError::Validation(
                ReviewValidationError::AnchorQuoteAbsent
            ))
        ));

        let exact_and_folded = ReviewRequest::document(
            ArtifactLens::Prompt,
            "evidence quote\nevidence   quote",
            SourceSnapshot::default(),
        )
        .unwrap();
        let decoded = ReviewModelOutput::decode(
            &wire(exact_and_folded.scope, "evidence quote").to_string(),
            &exact_and_folded,
        )
        .unwrap();
        assert_eq!(
            decoded.findings[0].anchor,
            SourceAnchor::document(SourceLocation::bytes(0, "evidence quote".len() as u64))
        );

        let folded_overlap =
            ReviewRequest::document(ArtifactLens::Prompt, "a  a  a", SourceSnapshot::default())
                .unwrap();
        assert!(matches!(
            ReviewModelOutput::decode(
                &wire(folded_overlap.scope, "a a").to_string(),
                &folded_overlap,
            ),
            Err(ReviewDecodeError::Validation(
                ReviewValidationError::AnchorQuoteAmbiguous
            ))
        ));

        let overlapping =
            ReviewRequest::document(ArtifactLens::Prompt, "aaaa", SourceSnapshot::default())
                .unwrap();
        assert!(matches!(
            ReviewModelOutput::decode(&wire(overlapping.scope, "aaa").to_string(), &overlapping),
            Err(ReviewDecodeError::Validation(
                ReviewValidationError::AnchorQuoteAmbiguous
            ))
        ));
    }

    #[test]
    fn provider_quote_anchors_require_an_included_utf8_skill_file() {
        let package = SkillPackage::new(
            vec![SkillPackageFile::text("SKILL.md", "unique package evidence", "entry").unwrap()],
            Vec::new(),
        )
        .unwrap();
        let request = ReviewRequest::agent_skill(package, SourceSnapshot::default()).unwrap();
        let response = serde_json::json!({
            "schema_version": REVIEW_SCHEMA_VERSION,
            "scope": {"kind": "agent_skill_package"},
            "understood_intent": {
                "stated_goal": "review the skill",
                "relevant_context": [],
                "constraints": [],
                "non_goals": [],
                "expected_deliverable": "a review",
                "success_evidence": [],
                "inferred_assumptions": [],
                "unresolved_decisions": []
            },
            "findings": [{
                "kind": "source",
                "text": "the evidence is present",
                "anchor": {
                    "kind": "agent_skill_file_quote",
                    "path": "SKILL.md",
                    "quote": "unique package evidence"
                }
            }],
            "clarification_questions": []
        });

        let decoded = ReviewModelOutput::decode(&response.to_string(), &request).unwrap();
        assert_eq!(
            decoded.findings[0].anchor,
            SourceAnchor::agent_skill_file(
                "SKILL.md",
                SourceLocation::bytes(0, "unique package evidence".len() as u64)
            )
            .unwrap()
        );
    }

    #[test]
    fn stale_result_keeps_output_but_changes_status() {
        let request =
            ReviewRequest::document(ArtifactLens::Prompt, "source", SourceSnapshot::new(1, 1))
                .unwrap();
        let output = output(
            ReviewScope::Document,
            SourceAnchor::document(SourceLocation::bytes(0, 6)),
        );
        let result = ReviewResult::ready(&request, output)
            .unwrap()
            .with_current_snapshot(SourceSnapshot::new(2, 1));
        assert_eq!(result.status, ReviewStatus::Stale);
        assert!(result.output.is_some());
    }

    #[test]
    fn skill_package_is_sorted_and_framed_without_following_symlinks() {
        let files = vec![
            SkillPackageFile::text("b.md", "B", "supporting file").unwrap(),
            SkillPackageFile::text("SKILL.md", "A", "entry file").unwrap(),
        ];
        let omissions =
            vec![SkillPackageOmission::symlink("link.md", "symbolic link omitted").unwrap()];
        let package = SkillPackage::new(files, omissions).unwrap();
        assert_eq!(package.files()[0].path, "SKILL.md");
        assert!(package.is_partial());
        let frames = package.source_frames();
        assert_eq!(frames[0].path(), "SKILL.md");
        let bytes = package.framed_bytes();
        assert_eq!(&bytes[..8], &(8_u64.to_be_bytes()));
        assert_eq!(
            u64::from_be_bytes(bytes[8 + 8..8 + 8 + 8].try_into().unwrap()),
            1
        );
    }

    #[test]
    fn package_anchor_must_name_an_included_file() {
        let package = SkillPackage::new(
            vec![SkillPackageFile::text("SKILL.md", "hello", "entry").unwrap()],
            Vec::new(),
        )
        .unwrap();
        let request = ReviewRequest::agent_skill(package, SourceSnapshot::default()).unwrap();
        let anchor =
            SourceAnchor::agent_skill_file("missing.md", SourceLocation::bytes(0, 1)).unwrap();
        let result = output(ReviewScope::AgentSkillPackage, anchor);
        assert!(matches!(
            result.validate_against(&request),
            Err(ReviewValidationError::AnchorPathNotInPackage(_))
        ));
    }
}
