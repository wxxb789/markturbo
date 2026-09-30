//! Frozen Agent Skill package snapshots and headless Review request building.
//!
//! The snapshot owns the exact inventory and bytes disclosed to Review. Reads
//! fail closed for symbolic links and revalidate against the same file identity
//! before a result may be used for navigation or another operation.

use std::ffi::OsStr;
use std::fmt;
use std::ops::Range;
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;

use sha2::{Digest as _, Sha256};

use crate::document::io::{self as document_io, SkillOrigin};
use crate::review::{
    ArtifactLens, ByteRange, MAX_SKILL_FILE_BYTES, MAX_SKILL_PACKAGE_BYTES,
    ReviewRequest as DocumentReviewRequest, ReviewValidationError, SkillFilePayload, SkillPackage,
    SkillPackageError, SkillPackageFile, SkillPackageOmission, SourceAnchor, SourceLocation,
    SourceSnapshot,
};

mod source;

/// Whether Review covers one whole source or an explicit editor selection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReviewTarget {
    Document,
    Selection,
}

/// Inputs for building an immutable Review request.
///
/// `source_text` is the current editor text. For a clean Agent Skill entrypoint
/// the package is read from disk; when it is dirty, this text is the exact
/// entrypoint payload frozen into the package. The request borrows all source
/// data only for the duration of [`build_review_request`].
pub struct ReviewRequestBuildRequest<'a> {
    target: ReviewTarget,
    lens: ArtifactLens,
    source_path: Option<&'a Path>,
    source_text: &'a str,
    selection: Option<&'a Range<usize>>,
    snapshot: SourceSnapshot,
    skill_entrypoint_is_dirty: bool,
    skill_origin: Option<&'a SkillOrigin>,
}

impl<'a> ReviewRequestBuildRequest<'a> {
    pub fn new(
        target: ReviewTarget,
        lens: ArtifactLens,
        source_path: Option<&'a Path>,
        source_text: &'a str,
        selection: Option<&'a Range<usize>>,
        snapshot: SourceSnapshot,
        skill_entrypoint_is_dirty: bool,
    ) -> Self {
        Self {
            target,
            lens,
            source_path,
            source_text,
            selection,
            snapshot,
            skill_entrypoint_is_dirty,
            skill_origin: None,
        }
    }

    /// Attach the origin captured by `document::io::load`. Agent Skill Review
    /// rejects a missing origin instead of deriving provenance from a path.
    pub fn with_skill_origin(mut self, origin: Option<&'a SkillOrigin>) -> Self {
        self.skill_origin = origin;
        self
    }
}

/// A validated Review request and, for Agent Skills, its frozen source identity.
pub struct ReviewRequestBuildResult {
    request: DocumentReviewRequest,
    skill_package: Option<FrozenSkillPackage>,
}

impl ReviewRequestBuildResult {
    pub fn into_parts(self) -> (DocumentReviewRequest, Option<FrozenSkillPackage>) {
        (self.request, self.skill_package)
    }
}

/// Why a headless Review request or frozen Agent Skill package could not be built.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReviewRequestBuildError {
    AgentSkillRootUnavailable,
    AgentSkillEntrypointUnavailable,
    AgentSkillSourceUnavailable,
    AgentSkillSelectionUnsupported,
    AgentSkillPathIsNotUtf8,
    AgentSkillReadFailed,
    AgentSkillSourceChanged,
    AgentSkillFileTooLarge { byte_size: u64 },
    AgentSkillPackageTooLarge { byte_size: u64 },
    InvalidAgentSkillPackage(SkillPackageError),
    InvalidSelection,
    InvalidRequest(ReviewValidationError),
}

impl fmt::Display for ReviewRequestBuildError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::AgentSkillRootUnavailable => {
                formatter.write_str("Agent Skill directory is unavailable")
            }
            Self::AgentSkillEntrypointUnavailable => {
                formatter.write_str("Agent Skill entrypoint is unavailable")
            }
            Self::AgentSkillSourceUnavailable => {
                formatter.write_str("Agent Skill source has no verified load-time origin")
            }
            Self::AgentSkillSelectionUnsupported => {
                formatter.write_str("Agent Skill Review cannot use a selection")
            }
            Self::AgentSkillPathIsNotUtf8 => {
                formatter.write_str("Agent Skill path is not valid UTF-8")
            }
            Self::AgentSkillReadFailed => {
                formatter.write_str("Agent Skill source could not be read")
            }
            Self::AgentSkillSourceChanged => {
                formatter.write_str("Agent Skill source changed while preparing Review")
            }
            Self::AgentSkillFileTooLarge { byte_size } => write!(
                formatter,
                "Agent Skill file is {byte_size} bytes, above the {MAX_SKILL_FILE_BYTES}-byte source limit"
            ),
            Self::AgentSkillPackageTooLarge { byte_size } => write!(
                formatter,
                "Agent Skill package is {byte_size} bytes, above the {MAX_SKILL_PACKAGE_BYTES}-byte source limit"
            ),
            Self::InvalidAgentSkillPackage(error) => error.fmt(formatter),
            Self::InvalidSelection => formatter.write_str("selection is outside the frozen source"),
            Self::InvalidRequest(error) => error.fmt(formatter),
        }
    }
}

impl std::error::Error for ReviewRequestBuildError {}

/// Build a provider-independent Review request from frozen source inputs.
///
/// Agent Skill requests require the whole `SKILL.md` entrypoint and freeze the
/// complete disclosed inventory. A selection over that entrypoint is rejected;
/// ordinary document and selection requests keep their canonical source
/// coordinates.
pub fn build_review_request(
    request: ReviewRequestBuildRequest<'_>,
) -> Result<ReviewRequestBuildResult, ReviewRequestBuildError> {
    let ReviewRequestBuildRequest {
        target,
        lens,
        source_path,
        source_text,
        selection,
        snapshot,
        skill_entrypoint_is_dirty,
        skill_origin,
    } = request;

    match target {
        ReviewTarget::Selection
            if lens == ArtifactLens::AgentSkill && source_path.is_some_and(is_skill_entrypoint) =>
        {
            Err(ReviewRequestBuildError::AgentSkillSelectionUnsupported)
        }
        ReviewTarget::Document
            if lens == ArtifactLens::AgentSkill && source_path.is_some_and(is_skill_entrypoint) =>
        {
            let source_path =
                source_path.ok_or(ReviewRequestBuildError::AgentSkillEntrypointUnavailable)?;
            let origin =
                skill_origin.ok_or(ReviewRequestBuildError::AgentSkillSourceUnavailable)?;
            if !origin.matches_source_path(source_path) {
                return Err(ReviewRequestBuildError::AgentSkillSourceChanged);
            }
            let entrypoint = if skill_entrypoint_is_dirty {
                RequestedEntrypoint::EditorText(source_text)
            } else {
                RequestedEntrypoint::Disk
            };
            let skill_package = build_frozen_agent_skill_package(origin, entrypoint)?;
            let request =
                DocumentReviewRequest::agent_skill(skill_package.package().clone(), snapshot)
                    .map_err(ReviewRequestBuildError::InvalidRequest)?;
            Ok(ReviewRequestBuildResult {
                request,
                skill_package: Some(skill_package),
            })
        }
        _ if lens == ArtifactLens::AgentSkill => {
            Err(ReviewRequestBuildError::AgentSkillEntrypointUnavailable)
        }
        ReviewTarget::Document => {
            let request = DocumentReviewRequest::document(lens, source_text, snapshot)
                .map_err(ReviewRequestBuildError::InvalidRequest)?;
            Ok(ReviewRequestBuildResult {
                request,
                skill_package: None,
            })
        }
        ReviewTarget::Selection => {
            let range = selection.ok_or(ReviewRequestBuildError::InvalidSelection)?;
            if range.is_empty() || source_text.get(range.clone()).is_none() {
                return Err(ReviewRequestBuildError::InvalidSelection);
            }
            let absolute_range = ByteRange::new(range.start as u64, range.end as u64)
                .map_err(ReviewRequestBuildError::InvalidRequest)?;
            let request =
                DocumentReviewRequest::selection(lens, source_text, absolute_range, snapshot)
                    .map_err(ReviewRequestBuildError::InvalidRequest)?;
            Ok(ReviewRequestBuildResult {
                request,
                skill_package: None,
            })
        }
    }
}

fn is_skill_entrypoint(path: &Path) -> bool {
    path.file_name() == Some(OsStr::new("SKILL.md"))
}

#[derive(Clone, Copy)]
enum RequestedEntrypoint<'a> {
    Disk,
    EditorText(&'a str),
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum EntrypointSource {
    Disk,
    EditorText,
}

/// One immutable Agent Skill snapshot. All fields stay private so callers can
/// neither broaden the disclosed package nor replace the identities it owns.
#[derive(Clone)]
pub struct FrozenSkillPackage {
    inner: Arc<FrozenSkillPackageInner>,
}

struct FrozenSkillPackageInner {
    origin: SkillOrigin,
    entrypoint_source: EntrypointSource,
    entrypoint_identity: Option<source::SkillSourceIdentity>,
    package: SkillPackage,
    supporting_files: Vec<source::SkillSourceIdentity>,
}

impl FrozenSkillPackage {
    /// The exact package payload disclosed to the provider request.
    pub fn package(&self) -> &SkillPackage {
        &self.inner.package
    }

    /// The filesystem root whose contents were inventoried.
    pub fn root(&self) -> &Path {
        self.inner.origin.canonical_root()
    }

    /// Whether a filesystem change path is the package root or beneath either
    /// the loaded lexical root or its authenticated canonical root.
    ///
    /// This accepts paths that no longer exist, as watcher events commonly
    /// describe removed files. Equivalent spellings are resolved through the
    /// existing path identity check only when the literal component paths do
    /// not already establish containment.
    pub fn path_affects_root(&self, candidate: &Path) -> bool {
        let origin = &self.inner.origin;
        skill_path_is_root_or_below(origin.lexical_root(), candidate)
            || skill_path_is_root_or_below(origin.canonical_root(), candidate)
    }

    /// Whether this frozen package uses editor text rather than a disk-backed
    /// SKILL.md entrypoint. Such an entrypoint has no disk identity for
    /// navigation; callers must resolve it against the matching editor source.
    pub fn entrypoint_is_editor_text(&self) -> bool {
        self.inner.entrypoint_source == EntrypointSource::EditorText
    }

    pub fn revalidate(&self) -> Result<(), ReviewRequestBuildError> {
        let requested_entrypoint = match self.inner.entrypoint_source {
            EntrypointSource::Disk => RequestedEntrypoint::Disk,
            EntrypointSource::EditorText => {
                let Some(file) = self
                    .inner
                    .package
                    .files()
                    .iter()
                    .find(|file| file.path == "SKILL.md")
                else {
                    return Err(ReviewRequestBuildError::AgentSkillSourceChanged);
                };
                let SkillFilePayload::Utf8 { content } = &file.payload else {
                    return Err(ReviewRequestBuildError::AgentSkillSourceChanged);
                };
                RequestedEntrypoint::EditorText(content)
            }
        };
        let current = build_frozen_agent_skill_package(&self.inner.origin, requested_entrypoint)?;
        if current.inner.package == self.inner.package
            && current.inner.entrypoint_identity == self.inner.entrypoint_identity
            && current.inner.supporting_files == self.inner.supporting_files
        {
            Ok(())
        } else {
            Err(ReviewRequestBuildError::AgentSkillSourceChanged)
        }
    }

    /// Whether an open editor path names one of this snapshot's supporting files.
    pub fn contains_supporting_path(&self, candidate: &Path) -> bool {
        self.inner.supporting_files.iter().any(|file| {
            source::package_path(self.inner.origin.canonical_root(), &file.path)
                .is_ok_and(|path| document_io::paths_match(&path, candidate))
        })
    }

    /// Read a disk-backed package file through the authenticated root,
    /// refusing links and returning bytes only when they match the frozen
    /// identity and digest. Editor-only SKILL.md content returns `None`.
    pub fn read_frozen_path(&self, candidate: &Path) -> Option<Vec<u8>> {
        source::read_frozen_source(
            &self.inner.origin,
            candidate,
            self.inner.entrypoint_identity.as_ref(),
            &self.inner.supporting_files,
        )
        .map(|source| source.bytes)
    }

    /// Build an editable source document for a frozen supporting file without
    /// reopening its path. The returned file has no Skill entrypoint origin.
    pub fn load_frozen_supporting_file(
        &self,
        candidate: &Path,
    ) -> Option<crate::document::io::LoadedFile> {
        let source = source::read_frozen_source(
            &self.inner.origin,
            candidate,
            None,
            &self.inner.supporting_files,
        )?;
        let canonical_path =
            source::package_path(self.inner.origin.canonical_root(), &source.relative_path).ok()?;
        document_io::loaded_file_from_frozen_snapshot(&canonical_path, source.bytes, source.stamp)
            .ok()
    }

    /// Resolve only an anchor already validated against this exact package.
    /// The model supplies a relative name, never an executable path or action.
    pub fn resolve_anchor(&self, anchor: &SourceAnchor) -> Option<(PathBuf, usize)> {
        let SourceAnchor::AgentSkillFile { path, location } = anchor else {
            return None;
        };
        let file = self
            .inner
            .package
            .files()
            .iter()
            .find(|file| file.path == path.as_str())?;
        let offset = match &file.payload {
            SkillFilePayload::Utf8 { content } => {
                let raw_offset = review_location_offset(*location, content)?;
                let transform = if file.path == "SKILL.md"
                    && self.inner.entrypoint_source == EntrypointSource::EditorText
                {
                    source::SkillEditorTextTransform::IDENTITY
                } else if file.path == "SKILL.md" {
                    self.inner.entrypoint_identity.as_ref()?.editor_transform?
                } else {
                    self.inner
                        .supporting_files
                        .iter()
                        .find(|source| source.path == file.path)?
                        .editor_transform?
                };
                skill_editor_offset(content, raw_offset, transform)?
            }
            SkillFilePayload::RawBinary { .. } => {
                let SourceLocation::ByteRange { start, .. } = location else {
                    return None;
                };
                usize::try_from(*start).ok()?
            }
            SkillFilePayload::Binary { .. } => return None,
        };
        source::navigation_path(&self.inner.origin, path)
            .ok()
            .map(|path| (path, offset))
    }
}

/// Resolve a document-scoped finding to its source byte offset.
///
/// Agent Skill file anchors are resolved against a [`FrozenSkillPackage`]
/// instead, because they must be limited to its disclosed inventory.
pub fn resolve_document_anchor_offset(anchor: &SourceAnchor, source: &str) -> Option<usize> {
    match anchor {
        SourceAnchor::Document { location } => review_location_offset(*location, source),
        SourceAnchor::DocumentWide => Some(0),
        SourceAnchor::AgentSkillFile { .. } => None,
    }
}

fn build_frozen_agent_skill_package(
    origin: &SkillOrigin,
    requested_entrypoint: RequestedEntrypoint<'_>,
) -> Result<FrozenSkillPackage, ReviewRequestBuildError> {
    let allow_dirty_entrypoint = matches!(requested_entrypoint, RequestedEntrypoint::EditorText(_));
    let dirty_entrypoint_byte_size = match requested_entrypoint {
        RequestedEntrypoint::Disk => None,
        RequestedEntrypoint::EditorText(text) => Some(text.len() as u64),
    };
    let inventory = source::acquire_skill_inventory(origin, dirty_entrypoint_byte_size)?;
    let frozen_origin = inventory.origin;
    let mut omissions = inventory
        .omissions
        .into_iter()
        .map(|omission| match omission.kind {
            source::SkillOmissionKind::Symlink => {
                SkillPackageOmission::symlink(omission.path, "symbolic link omitted")
                    .map_err(ReviewRequestBuildError::InvalidAgentSkillPackage)
            }
            source::SkillOmissionKind::NonRegular => {
                SkillPackageOmission::new(omission.path, "non-regular file omitted", false)
                    .map_err(ReviewRequestBuildError::InvalidAgentSkillPackage)
            }
        })
        .collect::<Result<Vec<_>, _>>()?;
    omissions.sort_by(|left, right| left.path.as_bytes().cmp(right.path.as_bytes()));

    let mut files = Vec::with_capacity(inventory.files.len().saturating_add(1));
    let mut supporting_files = Vec::with_capacity(inventory.files.len().saturating_sub(1));
    let mut entrypoint_identity = None;
    for source in inventory.files {
        let relative_path = source.identity.path.as_str();
        let is_entrypoint = relative_path == "SKILL.md";

        let file = skill_package_file_from_disk_bytes(
            relative_path,
            &source.bytes,
            if is_entrypoint {
                "skill entrypoint"
            } else {
                "supporting file"
            },
        )?;
        files.push(file);
        if is_entrypoint {
            entrypoint_identity = Some(source.identity);
        } else {
            supporting_files.push(source.identity);
        }
    }

    if allow_dirty_entrypoint {
        // The editor snapshot remains authoritative even if the root entrypoint
        // is missing, a link, unreadable, or too large on disk.
        omissions.retain(|omission| omission.path != "SKILL.md");
        if files.len().saturating_add(omissions.len()) >= source::MAX_AGENT_SKILL_INVENTORY_ENTRIES
        {
            return Err(ReviewRequestBuildError::AgentSkillPackageTooLarge {
                byte_size: u64::MAX,
            });
        }
        let RequestedEntrypoint::EditorText(text) = requested_entrypoint else {
            unreachable!("dirty Agent Skill source must use editor text");
        };
        let file = SkillPackageFile::text("SKILL.md", text, "skill entrypoint")
            .map_err(ReviewRequestBuildError::InvalidAgentSkillPackage)?;
        files.push(file);
    }

    let entrypoint_source = match requested_entrypoint {
        RequestedEntrypoint::Disk => EntrypointSource::Disk,
        RequestedEntrypoint::EditorText(_) => EntrypointSource::EditorText,
    };
    let package = SkillPackage::new(files, omissions)
        .map_err(ReviewRequestBuildError::InvalidAgentSkillPackage)?;
    origin
        .open_root()
        .map_err(|_| ReviewRequestBuildError::AgentSkillSourceChanged)?;
    Ok(FrozenSkillPackage {
        inner: Arc::new(FrozenSkillPackageInner {
            origin: frozen_origin,
            entrypoint_source,
            entrypoint_identity,
            package,
            supporting_files,
        }),
    })
}

fn skill_package_file_from_disk_bytes(
    relative_path: &str,
    bytes: &[u8],
    inclusion_reason: &str,
) -> Result<SkillPackageFile, ReviewRequestBuildError> {
    let byte_size = bytes.len() as u64;
    let file = if crate::workspace::walk::bytes_look_binary(bytes) {
        SkillPackageFile::binary_metadata(
            relative_path,
            byte_size,
            sha256_hex(bytes),
            inclusion_reason,
        )
    } else if let Ok(text) = std::str::from_utf8(bytes) {
        SkillPackageFile::text(relative_path, text, inclusion_reason)
    } else {
        SkillPackageFile::binary_metadata(
            relative_path,
            byte_size,
            sha256_hex(bytes),
            inclusion_reason,
        )
    };
    file.map_err(ReviewRequestBuildError::InvalidAgentSkillPackage)
}

fn skill_path_is_root_or_below(root: &Path, candidate: &Path) -> bool {
    fn safely_below(root: &Path, candidate: &Path) -> bool {
        candidate.strip_prefix(root).is_ok_and(|relative| {
            relative
                .components()
                .all(|component| matches!(component, Component::Normal(_)))
        })
    }

    if safely_below(root, candidate) {
        return true;
    }

    // Watchers may spell an existing root with a different path identity (in
    // particular, with or without Windows' verbatim prefix). Walk the event's
    // ancestors so deleted leaves still match through an existing parent.
    // The component check rejects paths that only reach the root through `..`.
    let mut ancestor = candidate;
    loop {
        if document_io::paths_match(root, ancestor) && safely_below(ancestor, candidate) {
            return true;
        }
        let Some(parent) = ancestor.parent() else {
            return false;
        };
        if parent == ancestor {
            return false;
        }
        ancestor = parent;
    }
}

fn review_location_offset(location: SourceLocation, source: &str) -> Option<usize> {
    match location {
        SourceLocation::ByteRange { start, .. } => usize::try_from(start).ok(),
        SourceLocation::LineRange { start, .. } => {
            let line = usize::try_from(start).ok()?;
            if line == 0 {
                return None;
            }
            if line == 1 {
                return Some(0);
            }
            source
                .match_indices('\n')
                .nth(line - 2)
                .map(|(offset, _)| offset + 1)
        }
    }
}

/// `document::io::load` removes a UTF-8 BOM and normalizes CRLF to LF. Map a
/// frozen source anchor to the corresponding editor-buffer offset.
fn skill_editor_offset(
    source: &str,
    raw_offset: usize,
    transform: source::SkillEditorTextTransform,
) -> Option<usize> {
    let bytes = source.as_bytes();
    if raw_offset > bytes.len() || !source.is_char_boundary(raw_offset) {
        return None;
    }
    let bom_length =
        if transform.strip_utf8_bom && bytes.starts_with(&[0xef, 0xbb, 0xbf]) && raw_offset >= 3 {
            3
        } else {
            0
        };
    let removed_cr = if transform.normalize_crlf {
        bytes
            .iter()
            .enumerate()
            .take(raw_offset)
            .filter(|(index, byte)| **byte == b'\r' && bytes.get(index + 1) == Some(&b'\n'))
            .count()
    } else {
        0
    };
    raw_offset.checked_sub(bom_length + removed_cr)
}

fn sha256_hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";

    let digest = Sha256::digest(bytes);
    let mut encoded = String::with_capacity(digest.len() * 2);
    for byte in digest {
        encoded.push(HEX[usize::from(byte >> 4)] as char);
        encoded.push(HEX[usize::from(byte & 0x0f)] as char);
    }
    encoded
}

#[cfg(test)]
mod tests {
    use super::source::{
        MAX_AGENT_SKILL_INVENTORY_ENTRIES, install_skill_root_validated_hook,
        read_regular_skill_supporting_file,
    };
    use super::{
        FrozenSkillPackage, ReviewRequestBuildError, ReviewRequestBuildRequest,
        ReviewRequestBuildResult, ReviewTarget, build_review_request,
        resolve_document_anchor_offset,
    };
    use crate::document::io::{self as document_io, SkillOrigin, SkillOriginRoot};
    use crate::review::{
        ArtifactLens, ByteRange, MAX_SKILL_FILE_BYTES, ReviewScope, SkillFilePayload, SourceAnchor,
        SourceLocation, SourceSnapshot,
    };
    use std::fs;
    use std::path::Path;

    fn build_skill_request(
        root: &Path,
        source_text: &str,
        entrypoint_is_dirty: bool,
    ) -> Result<ReviewRequestBuildResult, ReviewRequestBuildError> {
        let skill_path = root.join("SKILL.md");
        let loaded = document_io::load(&skill_path).unwrap();
        build_skill_request_with_origin(
            &skill_path,
            source_text,
            entrypoint_is_dirty,
            loaded.skill_origin(),
        )
    }

    fn build_skill_request_with_origin(
        skill_path: &Path,
        source_text: &str,
        entrypoint_is_dirty: bool,
        origin: Option<&SkillOrigin>,
    ) -> Result<ReviewRequestBuildResult, ReviewRequestBuildError> {
        build_review_request(
            ReviewRequestBuildRequest::new(
                ReviewTarget::Document,
                ArtifactLens::AgentSkill,
                Some(skill_path),
                source_text,
                None,
                SourceSnapshot::default(),
                entrypoint_is_dirty,
            )
            .with_skill_origin(origin),
        )
    }

    fn frozen_skill_package(
        root: &Path,
        source_text: &str,
        entrypoint_is_dirty: bool,
    ) -> Result<FrozenSkillPackage, ReviewRequestBuildError> {
        let (_, package) =
            build_skill_request(root, source_text, entrypoint_is_dirty)?.into_parts();
        Ok(package.expect("Agent Skill request owns its frozen package"))
    }

    fn opened_test_skill_root(root: &Path) -> SkillOriginRoot {
        let loaded = document_io::load(&root.join("SKILL.md")).unwrap();
        loaded.skill_origin().unwrap().open_root().unwrap()
    }

    #[test]
    fn skill_document_review_uses_a_frozen_package_with_deterministic_inventory() {
        let directory = tempfile::tempdir().unwrap();
        let skill = directory.path().join("SKILL.md");
        fs::write(&skill, "disk entrypoint").unwrap();
        fs::create_dir(directory.path().join("references")).unwrap();
        fs::write(
            directory.path().join("references").join("guide.md"),
            "guide",
        )
        .unwrap();
        fs::create_dir(directory.path().join("assets")).unwrap();
        fs::write(
            directory.path().join("assets").join("blob.bin"),
            [0xff, 0x00],
        )
        .unwrap();

        let (request, frozen) = build_skill_request(directory.path(), "frozen entrypoint", false)
            .unwrap()
            .into_parts();
        assert!(matches!(request.scope, ReviewScope::AgentSkillPackage));
        let frozen = frozen.expect("Agent Skill request owns its frozen package");
        let package = request.source.package().unwrap();
        assert_eq!(frozen.package(), package);
        assert_eq!(
            package
                .files()
                .iter()
                .map(|file| file.path.as_str())
                .collect::<Vec<_>>(),
            ["SKILL.md", "assets/blob.bin", "references/guide.md"]
        );
        assert!(matches!(
            &package.files()[0].payload,
            SkillFilePayload::Utf8 { content } if content == "disk entrypoint"
        ));
        assert!(package.files()[1].is_binary_metadata_only());
    }

    #[test]
    fn clean_skill_entrypoint_preserves_raw_bytes_and_revalidates_identity() {
        let directory = tempfile::tempdir().unwrap();
        let skill = directory.path().join("SKILL.md");
        let raw = b"\xef\xbb\xbf# Skill\r\n";
        fs::write(&skill, raw).unwrap();

        let frozen = frozen_skill_package(directory.path(), "# Skill\n", false).unwrap();
        assert!(!frozen.entrypoint_is_editor_text());
        let entrypoint = &frozen.package().files()[0];
        assert!(matches!(
            &entrypoint.payload,
            SkillFilePayload::Utf8 { content } if content.as_bytes() == raw
        ));

        fs::write(&skill, "# Skill\n").unwrap();
        assert!(matches!(
            frozen.revalidate(),
            Err(ReviewRequestBuildError::AgentSkillSourceChanged)
        ));
    }

    #[cfg(any(unix, windows))]
    #[test]
    fn frozen_skill_rejects_retargeted_ancestor_with_identical_files() {
        let directory = tempfile::tempdir().unwrap();
        let original_tree = directory.path().join("original");
        let alternate_tree = directory.path().join("alternate");
        let original = original_tree.join("skill");
        let alternate = alternate_tree.join("skill");
        fs::create_dir_all(&original).unwrap();
        fs::create_dir_all(&alternate).unwrap();
        fs::write(original.join("SKILL.md"), "entrypoint").unwrap();
        fs::write(original.join("notes.md"), "same supporting source").unwrap();
        for name in ["SKILL.md", "notes.md"] {
            fs::hard_link(original.join(name), alternate.join(name)).unwrap();
        }

        let alias = directory.path().join("alias");
        #[cfg(unix)]
        std::os::unix::fs::symlink(directory.path().join("original"), &alias).unwrap();
        #[cfg(windows)]
        let uses_symlink = if std::os::windows::fs::symlink_dir(&original_tree, &alias).is_ok() {
            true
        } else {
            // A directory rename still exercises ancestor retargeting on
            // Windows installations without symbolic-link privileges.
            fs::rename(&original_tree, &alias).unwrap();
            false
        };
        let frozen = frozen_skill_package(&alias.join("skill"), "entrypoint", false).unwrap();

        #[cfg(unix)]
        fs::remove_file(&alias).unwrap();
        #[cfg(windows)]
        if uses_symlink {
            fs::remove_dir(&alias).unwrap();
        } else {
            fs::rename(&alias, directory.path().join("retained-original")).unwrap();
            fs::rename(&alternate_tree, &alias).unwrap();
        }
        #[cfg(unix)]
        std::os::unix::fs::symlink(directory.path().join("alternate"), &alias).unwrap();
        #[cfg(windows)]
        if uses_symlink {
            std::os::windows::fs::symlink_dir(&alternate_tree, &alias).unwrap();
        }

        assert!(matches!(
            frozen.revalidate(),
            Err(ReviewRequestBuildError::AgentSkillSourceChanged)
        ));
    }

    #[cfg(any(unix, windows))]
    #[test]
    fn loaded_origin_rejects_ancestor_retargeted_before_freeze() {
        let directory = tempfile::tempdir().unwrap();
        let original_tree = directory.path().join("original");
        let alternate_tree = directory.path().join("alternate");
        let original = original_tree.join("skill");
        let alternate = alternate_tree.join("skill");
        fs::create_dir_all(&original).unwrap();
        fs::create_dir_all(&alternate).unwrap();
        fs::write(original.join("SKILL.md"), "same entrypoint").unwrap();
        fs::write(original.join("notes.md"), "same supporting source").unwrap();
        for name in ["SKILL.md", "notes.md"] {
            fs::hard_link(original.join(name), alternate.join(name)).unwrap();
        }

        let alias = directory.path().join("alias");
        #[cfg(unix)]
        std::os::unix::fs::symlink(&original_tree, &alias).unwrap();
        #[cfg(windows)]
        let uses_symlink = if std::os::windows::fs::symlink_dir(&original_tree, &alias).is_ok() {
            true
        } else {
            fs::rename(&original_tree, &alias).unwrap();
            false
        };
        let skill = alias.join("skill/SKILL.md");
        let loaded = document_io::load(&skill).unwrap();
        let origin = loaded.skill_origin().unwrap().clone();

        #[cfg(unix)]
        {
            fs::remove_file(&alias).unwrap();
            std::os::unix::fs::symlink(&alternate_tree, &alias).unwrap();
        }
        #[cfg(windows)]
        if uses_symlink {
            fs::remove_dir(&alias).unwrap();
            std::os::windows::fs::symlink_dir(&alternate_tree, &alias).unwrap();
        } else {
            fs::rename(&alias, directory.path().join("retained-original")).unwrap();
            fs::rename(&alternate_tree, &alias).unwrap();
        }

        assert!(matches!(
            build_skill_request_with_origin(&skill, &loaded.text, false, Some(&origin)),
            Err(ReviewRequestBuildError::AgentSkillSourceChanged)
        ));
    }

    #[test]
    fn agent_skill_review_requires_a_load_time_origin() {
        let directory = tempfile::tempdir().unwrap();
        let skill = directory.path().join("SKILL.md");
        fs::write(&skill, "entrypoint").unwrap();

        assert!(matches!(
            build_review_request(ReviewRequestBuildRequest::new(
                ReviewTarget::Document,
                ArtifactLens::AgentSkill,
                Some(&skill),
                "entrypoint",
                None,
                SourceSnapshot::default(),
                false,
            )),
            Err(ReviewRequestBuildError::AgentSkillSourceUnavailable)
        ));
    }

    #[test]
    fn clean_skill_entrypoint_must_match_the_loaded_source_identity() {
        let directory = tempfile::tempdir().unwrap();
        let external = tempfile::tempdir().unwrap();
        let skill = directory.path().join("SKILL.md");
        let identical = external.path().join("same-bytes.md");
        fs::write(&skill, "same entrypoint").unwrap();
        fs::write(&identical, "same entrypoint").unwrap();
        let loaded = document_io::load(&skill).unwrap();
        let origin = loaded.skill_origin().unwrap().clone();
        fs::remove_file(&skill).unwrap();
        fs::hard_link(&identical, &skill).unwrap();

        assert!(matches!(
            build_skill_request_with_origin(&skill, &loaded.text, false, Some(&origin)),
            Err(ReviewRequestBuildError::AgentSkillSourceChanged)
        ));
    }

    #[cfg(unix)]
    #[test]
    fn a_stable_symlinked_ancestor_keeps_its_loaded_skill_origin() {
        use std::os::unix::fs::symlink;

        let directory = tempfile::tempdir().unwrap();
        let actual_root = directory.path().join("actual");
        fs::create_dir_all(actual_root.join("skill")).unwrap();
        fs::write(actual_root.join("skill/SKILL.md"), "entrypoint").unwrap();
        fs::write(actual_root.join("skill/guide.md"), "guide").unwrap();
        let alias = directory.path().join("alias");
        symlink(&actual_root, &alias).unwrap();
        let skill = alias.join("skill/SKILL.md");
        let loaded = document_io::load(&skill).unwrap();
        let origin = loaded.skill_origin().unwrap();

        let frozen = build_skill_request_with_origin(&skill, &loaded.text, false, Some(origin))
            .unwrap()
            .into_parts()
            .1
            .unwrap();
        assert_eq!(
            frozen.root(),
            actual_root.join("skill").canonicalize().unwrap()
        );
        assert!(frozen.revalidate().is_ok());
    }

    #[cfg(any(unix, windows))]
    #[test]
    fn ancestor_retarget_during_revalidation_never_inventories_the_alternate_tree() {
        #[cfg(unix)]
        use std::os::unix::fs::symlink;

        let directory = tempfile::tempdir().unwrap();
        let original_tree = directory.path().join("original");
        let alternate_tree = directory.path().join("alternate");
        let original_root = original_tree.join("skill");
        let alternate_root = alternate_tree.join("skill");
        fs::create_dir_all(&original_root).unwrap();
        fs::create_dir_all(&alternate_root).unwrap();
        fs::write(original_root.join("SKILL.md"), "original entrypoint").unwrap();
        fs::write(original_root.join("notes.md"), "original support").unwrap();
        fs::write(alternate_root.join("SKILL.md"), "alternate entrypoint").unwrap();
        fs::write(alternate_root.join("secret.md"), "alternate-only name").unwrap();
        let alias = directory.path().join("alias");
        #[cfg(unix)]
        {
            symlink(&original_tree, &alias).unwrap();
        }
        #[cfg(windows)]
        let uses_symlink = std::os::windows::fs::symlink_dir(&original_tree, &alias).is_ok();
        #[cfg(unix)]
        let loaded_root = alias.join("skill");
        #[cfg(windows)]
        let loaded_root = if uses_symlink {
            alias.join("skill")
        } else {
            original_root.clone()
        };
        let frozen = frozen_skill_package(&loaded_root, "original entrypoint", false).unwrap();

        #[cfg(unix)]
        {
            let alias_for_hook = alias.clone();
            install_skill_root_validated_hook(move || {
                fs::remove_file(&alias_for_hook).unwrap();
                symlink(&alternate_tree, &alias_for_hook).unwrap();
            });
            assert!(matches!(
                frozen.revalidate(),
                Err(ReviewRequestBuildError::AgentSkillSourceChanged)
            ));
        }
        #[cfg(windows)]
        if uses_symlink {
            let alias_for_hook = alias.clone();
            install_skill_root_validated_hook(move || {
                fs::remove_dir(&alias_for_hook).unwrap();
                std::os::windows::fs::symlink_dir(&alternate_tree, &alias_for_hook).unwrap();
            });
            assert!(matches!(
                frozen.revalidate(),
                Err(ReviewRequestBuildError::AgentSkillSourceChanged)
            ));
        } else {
            let original_for_hook = original_root.clone();
            let retained_for_hook = directory.path().join("retained-original");
            install_skill_root_validated_hook(move || {
                assert!(fs::rename(&original_for_hook, &retained_for_hook).is_err());
            });
            assert!(frozen.revalidate().is_ok());
        }
        assert!(
            frozen
                .package()
                .files()
                .iter()
                .all(|file| file.path != "secret.md")
        );
    }

    #[test]
    fn valid_utf8_binary_supporting_file_is_metadata_only() {
        let directory = tempfile::tempdir().unwrap();
        fs::write(directory.path().join("SKILL.md"), "entrypoint").unwrap();
        fs::write(directory.path().join("asset.bin"), b"valid\0utf8").unwrap();

        let frozen = frozen_skill_package(directory.path(), "entrypoint", false).unwrap();
        let asset = frozen
            .package()
            .files()
            .iter()
            .find(|file| file.path == "asset.bin")
            .unwrap();
        assert!(asset.is_binary_metadata_only());
        assert_eq!(asset.raw_bytes(), None);
    }

    #[test]
    fn selection_review_of_skill_entrypoint_is_explicitly_unsupported() {
        let directory = tempfile::tempdir().unwrap();
        let skill = directory.path().join("SKILL.md");
        fs::write(&skill, "entrypoint").unwrap();

        let selection = 0..5;
        let loaded = document_io::load(&skill).unwrap();
        assert!(matches!(
            build_review_request(
                ReviewRequestBuildRequest::new(
                    ReviewTarget::Selection,
                    ArtifactLens::AgentSkill,
                    Some(&skill),
                    "entrypoint",
                    Some(&selection),
                    SourceSnapshot::default(),
                    false,
                )
                .with_skill_origin(loaded.skill_origin())
            ),
            Err(ReviewRequestBuildError::AgentSkillSelectionUnsupported)
        ));
    }

    #[test]
    fn non_utf8_skill_entrypoint_uses_raw_metadata_and_revalidates_the_raw_source() {
        let directory = tempfile::tempdir().unwrap();
        let skill = directory.path().join("SKILL.md");
        let original = [0xff, 0x81, 0x40];
        fs::write(&skill, original).unwrap();

        let frozen = frozen_skill_package(directory.path(), "decoded entrypoint", false).unwrap();
        let entrypoint = &frozen.package().files()[0];
        assert_eq!(entrypoint.path, "SKILL.md");
        assert_eq!(entrypoint.byte_size, original.len() as u64);
        let expected_digest = super::sha256_hex(&original);
        assert!(matches!(
            &entrypoint.payload,
            SkillFilePayload::Binary { sha256 } if sha256 == &expected_digest
        ));

        fs::write(&skill, [0xff, 0x81, 0x41]).unwrap();
        assert!(matches!(
            frozen.revalidate(),
            Err(ReviewRequestBuildError::AgentSkillSourceChanged)
        ));
    }

    #[test]
    fn dirty_non_utf8_skill_entrypoint_sends_explicit_unsaved_utf8_text() {
        let directory = tempfile::tempdir().unwrap();
        let skill = directory.path().join("SKILL.md");
        fs::write(&skill, [0xff, 0x81, 0x40]).unwrap();

        let frozen = frozen_skill_package(directory.path(), "unsaved UTF-8 text", true).unwrap();
        assert!(frozen.entrypoint_is_editor_text());
        let entrypoint = &frozen.package().files()[0];
        assert!(matches!(
            &entrypoint.payload,
            SkillFilePayload::Utf8 { content } if content == "unsaved UTF-8 text"
        ));
        assert_eq!(entrypoint.byte_size, "unsaved UTF-8 text".len() as u64);
        fs::write(&skill, [0xff, 0x81, 0x41]).unwrap();
        assert!(
            frozen.revalidate().is_ok(),
            "the frozen editor snapshot must not be recategorized from disk while the document remains unchanged"
        );
    }

    #[test]
    fn dirty_skill_entrypoint_ignores_deleted_or_oversized_disk_entrypoint() {
        let directory = tempfile::tempdir().unwrap();
        let skill = directory.path().join("SKILL.md");
        fs::write(&skill, vec![b'x'; (MAX_SKILL_FILE_BYTES + 1) as usize]).unwrap();
        let loaded = document_io::load(&skill).unwrap();
        let origin = loaded.skill_origin().unwrap().clone();

        for remove_disk_entrypoint in [false, true] {
            if remove_disk_entrypoint {
                fs::remove_file(&skill).unwrap();
            }
            let frozen =
                build_skill_request_with_origin(&skill, "unsaved entrypoint", true, Some(&origin))
                    .unwrap()
                    .into_parts()
                    .1
                    .unwrap();
            assert!(matches!(
                &frozen.package().files()[0].payload,
                SkillFilePayload::Utf8 { content } if content == "unsaved entrypoint"
            ));
            assert!(
                frozen
                    .package()
                    .omissions()
                    .iter()
                    .all(|omission| omission.path != "SKILL.md")
            );
            if remove_disk_entrypoint {
                assert!(frozen.revalidate().is_ok());
            }
        }
    }

    #[cfg(unix)]
    #[test]
    fn dirty_skill_entrypoint_replaces_a_symlinked_disk_entrypoint() {
        use std::os::unix::fs::symlink;

        let directory = tempfile::tempdir().unwrap();
        let target = directory.path().join("outside.md");
        let skill = directory.path().join("SKILL.md");
        fs::write(&target, "disk target").unwrap();
        fs::write(&skill, "original entrypoint").unwrap();
        let loaded = document_io::load(&skill).unwrap();
        let origin = loaded.skill_origin().unwrap().clone();
        fs::remove_file(&skill).unwrap();
        symlink(&target, &skill).unwrap();

        let frozen =
            build_skill_request_with_origin(&skill, "unsaved entrypoint", true, Some(&origin))
                .unwrap()
                .into_parts()
                .1
                .unwrap();
        assert!(matches!(
            &frozen.package().files()[0].payload,
            SkillFilePayload::Utf8 { content } if content == "unsaved entrypoint"
        ));
        assert!(
            frozen
                .package()
                .omissions()
                .iter()
                .all(|omission| omission.path != "SKILL.md")
        );
        assert!(frozen.read_frozen_path(&skill).is_none());
        assert!(frozen.revalidate().is_ok());
    }

    #[cfg(any(unix, windows))]
    #[test]
    fn dirty_skill_entrypoint_does_not_inventory_a_replacement_directory() {
        let directory = tempfile::tempdir().unwrap();
        let skill = directory.path().join("SKILL.md");
        fs::write(&skill, "original entrypoint").unwrap();
        let loaded = document_io::load(&skill).unwrap();
        let origin = loaded.skill_origin().unwrap().clone();
        fs::remove_file(&skill).unwrap();
        fs::create_dir(&skill).unwrap();
        fs::write(skill.join("alternate-only.md"), "must not be inventoried").unwrap();

        assert!(matches!(
            build_skill_request_with_origin(&skill, "unsaved entrypoint", true, Some(&origin)),
            Err(ReviewRequestBuildError::AgentSkillSourceChanged)
        ));
    }

    #[cfg(unix)]
    #[test]
    fn unix_skill_inventory_rejects_a_backslash_path_component() {
        let directory = tempfile::tempdir().unwrap();
        fs::write(directory.path().join("SKILL.md"), "entrypoint").unwrap();
        fs::write(directory.path().join(r"support\name.md"), "supporting").unwrap();

        assert!(matches!(
            build_skill_request(directory.path(), "entrypoint", false),
            Err(ReviewRequestBuildError::AgentSkillPathIsNotUtf8)
        ));
    }

    #[test]
    fn frozen_skill_package_rejects_changed_supporting_sources_before_send() {
        let directory = tempfile::tempdir().unwrap();
        fs::write(directory.path().join("SKILL.md"), "entrypoint").unwrap();
        let guide = directory.path().join("guide.md");
        fs::write(&guide, "original supporting source").unwrap();

        let frozen = frozen_skill_package(directory.path(), "entrypoint", false).unwrap();
        fs::write(&guide, "changed supporting source").unwrap();

        assert!(matches!(
            frozen.revalidate(),
            Err(ReviewRequestBuildError::AgentSkillSourceChanged)
        ));
    }

    #[test]
    fn skill_package_size_limits_abort_before_a_request_is_built() {
        let directory = tempfile::tempdir().unwrap();
        fs::write(directory.path().join("SKILL.md"), "entrypoint").unwrap();
        fs::write(
            directory.path().join("oversize.md"),
            vec![b'x'; (MAX_SKILL_FILE_BYTES + 1) as usize],
        )
        .unwrap();

        assert!(matches!(
            build_skill_request(directory.path(), "entrypoint", false),
            Err(ReviewRequestBuildError::AgentSkillFileTooLarge { .. })
        ));
    }

    #[test]
    fn supporting_file_reader_rejects_an_oversize_file_before_allocating_its_length() {
        let directory = tempfile::tempdir().unwrap();
        fs::write(directory.path().join("SKILL.md"), "entrypoint").unwrap();
        fs::write(
            directory.path().join("oversize.md"),
            vec![b'x'; (MAX_SKILL_FILE_BYTES + 1) as usize],
        )
        .unwrap();
        let root = opened_test_skill_root(directory.path());

        assert!(matches!(
            read_regular_skill_supporting_file(&root, "oversize.md"),
            Err(ReviewRequestBuildError::AgentSkillFileTooLarge { byte_size })
                if byte_size == MAX_SKILL_FILE_BYTES + 1
        ));
    }

    #[test]
    fn agent_skill_lens_requires_a_whole_skill_entrypoint_document() {
        let directory = tempfile::tempdir().unwrap();
        let not_a_skill = directory.path().join("prompt.md");
        fs::write(&not_a_skill, "prompt").unwrap();

        assert!(matches!(
            build_review_request(ReviewRequestBuildRequest::new(
                ReviewTarget::Document,
                ArtifactLens::AgentSkill,
                Some(&not_a_skill),
                "prompt",
                None,
                SourceSnapshot::default(),
                false,
            )),
            Err(ReviewRequestBuildError::AgentSkillEntrypointUnavailable)
        ));
        let selection = 0..5;
        assert!(matches!(
            build_review_request(ReviewRequestBuildRequest::new(
                ReviewTarget::Selection,
                ArtifactLens::AgentSkill,
                Some(&not_a_skill),
                "0123456789",
                Some(&selection),
                SourceSnapshot::default(),
                false,
            )),
            Err(ReviewRequestBuildError::AgentSkillEntrypointUnavailable)
        ));
    }

    #[test]
    fn skill_package_aggregate_limit_aborts_before_a_request_is_built() {
        let directory = tempfile::tempdir().unwrap();
        fs::write(directory.path().join("SKILL.md"), "entrypoint").unwrap();
        for index in 0..9 {
            fs::write(
                directory.path().join(format!("support-{index}.md")),
                vec![b'x'; MAX_SKILL_FILE_BYTES as usize],
            )
            .unwrap();
        }

        assert!(matches!(
            build_skill_request(directory.path(), "entrypoint", false),
            Err(ReviewRequestBuildError::AgentSkillPackageTooLarge { .. })
        ));
    }

    #[test]
    fn inventory_limit_allows_empty_directories_at_the_entry_cap() {
        let directory = tempfile::tempdir().unwrap();
        fs::write(directory.path().join("SKILL.md"), "entrypoint").unwrap();
        for index in 0..MAX_AGENT_SKILL_INVENTORY_ENTRIES - 1 {
            fs::write(
                directory.path().join(format!("support-{index:04}.md")),
                b"x",
            )
            .unwrap();
        }
        fs::create_dir(directory.path().join("zz-empty")).unwrap();

        let frozen = frozen_skill_package(directory.path(), "entrypoint", false).unwrap();
        assert_eq!(
            frozen.package().files().len(),
            MAX_AGENT_SKILL_INVENTORY_ENTRIES
        );
    }

    #[test]
    fn skill_inventory_bounds_empty_directory_traversal() {
        let directory = tempfile::tempdir().unwrap();
        fs::write(directory.path().join("SKILL.md"), "entrypoint").unwrap();
        for index in 0..MAX_AGENT_SKILL_INVENTORY_ENTRIES {
            fs::create_dir(directory.path().join(format!("empty-{index:04}"))).unwrap();
        }

        assert!(matches!(
            frozen_skill_package(directory.path(), "entrypoint", false),
            Err(ReviewRequestBuildError::AgentSkillPackageTooLarge { .. })
        ));
    }

    #[cfg(unix)]
    #[test]
    fn symbolic_links_are_disclosed_as_omissions_without_being_followed() {
        use std::os::unix::fs::symlink;

        let directory = tempfile::tempdir().unwrap();
        fs::write(directory.path().join("SKILL.md"), "entrypoint").unwrap();
        let external = tempfile::tempdir().unwrap();
        fs::write(external.path().join("secret.md"), "must not be read").unwrap();
        symlink(
            external.path().join("secret.md"),
            directory.path().join("linked.md"),
        )
        .unwrap();

        let (request, _) = build_skill_request(directory.path(), "entrypoint", false)
            .unwrap()
            .into_parts();
        let package = request.source.package().unwrap();
        assert!(package.files().iter().all(|file| file.path != "linked.md"));
        assert_eq!(package.omissions().len(), 1);
        assert_eq!(package.omissions()[0].path, "linked.md");
        assert!(package.omissions()[0].symlink);
    }

    #[cfg(unix)]
    #[test]
    fn supporting_file_reader_refuses_a_symlink_target() {
        use std::os::unix::fs::symlink;

        let directory = tempfile::tempdir().unwrap();
        fs::write(directory.path().join("SKILL.md"), "entrypoint").unwrap();
        let external = tempfile::tempdir().unwrap();
        let target = external.path().join("secret.md");
        fs::write(&target, "must not be read").unwrap();
        symlink(&target, directory.path().join("linked.md")).unwrap();
        let root = opened_test_skill_root(directory.path());

        assert!(matches!(
            read_regular_skill_supporting_file(&root, "linked.md"),
            Err(ReviewRequestBuildError::AgentSkillReadFailed)
                | Err(ReviewRequestBuildError::AgentSkillSourceChanged)
        ));
    }

    #[cfg(windows)]
    #[test]
    fn supporting_file_reader_refuses_a_symlink_target() {
        let directory = tempfile::tempdir().unwrap();
        fs::write(directory.path().join("SKILL.md"), "entrypoint").unwrap();
        let external = tempfile::tempdir().unwrap();
        let target = external.path().join("secret.md");
        let link = directory.path().join("linked.md");
        fs::write(&target, "must not be read").unwrap();
        if let Err(error) = std::os::windows::fs::symlink_file(&target, &link) {
            eprintln!("skipping symlink no-follow test: {error}");
            return;
        }
        let root = opened_test_skill_root(directory.path());

        assert!(matches!(
            read_regular_skill_supporting_file(&root, "linked.md"),
            Err(ReviewRequestBuildError::AgentSkillReadFailed)
                | Err(ReviewRequestBuildError::AgentSkillSourceChanged)
        ));
    }

    #[test]
    fn frozen_package_anchor_navigation_only_resolves_listed_relative_files() {
        let directory = tempfile::tempdir().unwrap();
        let skill = directory.path().join("SKILL.md");
        fs::write(&skill, "entrypoint").unwrap();
        fs::create_dir(directory.path().join("references")).unwrap();
        let guide = directory.path().join("references").join("guide.md");
        fs::write(&guide, "first\nsecond\n").unwrap();
        let frozen = frozen_skill_package(directory.path(), "entrypoint", false).unwrap();
        assert_eq!(
            frozen.read_frozen_path(&guide).unwrap().as_slice(),
            b"first\nsecond\n"
        );
        let supporting_document = frozen.load_frozen_supporting_file(&guide).unwrap();
        assert_eq!(supporting_document.text, "first\nsecond\n");
        assert!(supporting_document.skill_origin().is_none());

        let anchor =
            SourceAnchor::agent_skill_file("references/guide.md", SourceLocation::lines(2, 2))
                .unwrap();
        assert_eq!(frozen.resolve_anchor(&anchor), Some((guide.clone(), 6)));
        assert_eq!(
            frozen.resolve_anchor(
                &SourceAnchor::agent_skill_file(
                    "references/guide.md",
                    SourceLocation::bytes(6, 12),
                )
                .unwrap(),
            ),
            Some((guide.clone(), 6))
        );
        assert_eq!(
            frozen.resolve_anchor(
                &SourceAnchor::agent_skill_file("SKILL.md", SourceLocation::bytes(0, 0)).unwrap(),
            ),
            Some((skill, 0))
        );
        assert!(
            frozen
                .resolve_anchor(&SourceAnchor::AgentSkillFile {
                    path: "../outside.md".into(),
                    location: SourceLocation::bytes(0, 1),
                })
                .is_none()
        );
        assert!(frozen.resolve_anchor(&SourceAnchor::DocumentWide).is_none());

        let alternate = tempfile::tempdir().unwrap();
        let identical_guide = alternate.path().join("guide.md");
        fs::write(&identical_guide, "first\nsecond\n").unwrap();
        fs::remove_file(&guide).unwrap();
        fs::hard_link(&identical_guide, &guide).unwrap();
        assert!(frozen.read_frozen_path(&guide).is_none());
        assert!(frozen.load_frozen_supporting_file(&guide).is_none());
        assert_eq!(supporting_document.text, "first\nsecond\n");
    }

    #[cfg(any(unix, windows))]
    #[test]
    fn frozen_navigation_refuses_a_symlink_to_the_same_frozen_file() {
        let directory = tempfile::tempdir().unwrap();
        let skill = directory.path().join("SKILL.md");
        let guide = directory.path().join("guide.md");
        fs::write(&skill, "entrypoint").unwrap();
        fs::write(&guide, "frozen support").unwrap();
        let frozen = frozen_skill_package(directory.path(), "entrypoint", false).unwrap();

        let external = tempfile::tempdir().unwrap();
        let same_object = external.path().join("same-object.md");
        fs::hard_link(&guide, &same_object).unwrap();
        fs::remove_file(&guide).unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(&same_object, &guide).unwrap();
        #[cfg(windows)]
        if let Err(error) = std::os::windows::fs::symlink_file(&same_object, &guide) {
            eprintln!("skipping frozen navigation leaf-symlink fixture: {error}");
            return;
        }

        assert!(frozen.read_frozen_path(&guide).is_none());
        assert!(frozen.load_frozen_supporting_file(&guide).is_none());
    }

    #[test]
    fn frozen_package_anchor_navigation_maps_bom_and_crlf_to_editor_offsets() {
        let directory = tempfile::tempdir().unwrap();
        fs::write(directory.path().join("SKILL.md"), "entrypoint").unwrap();
        let guide = directory.path().join("guide.md");
        fs::write(
            &guide,
            [&b"\xef\xbb\xbf"[..], &b"first\r\nsecond\r\n"[..]].concat(),
        )
        .unwrap();
        let frozen = frozen_skill_package(directory.path(), "entrypoint", false).unwrap();

        assert_eq!(
            frozen.resolve_anchor(
                &SourceAnchor::agent_skill_file("guide.md", SourceLocation::bytes(10, 16)).unwrap(),
            ),
            Some((guide.clone(), 6))
        );
        assert_eq!(
            frozen.resolve_anchor(
                &SourceAnchor::agent_skill_file("guide.md", SourceLocation::lines(2, 2)).unwrap(),
            ),
            Some((guide, 6))
        );
    }

    #[test]
    fn editor_anchor_mapping_uses_only_the_transform_applied_to_each_source() {
        let directory = tempfile::tempdir().unwrap();
        let skill = directory.path().join("SKILL.md");
        fs::write(&skill, "entrypoint").unwrap();
        let mixed = directory.path().join("mixed.md");
        fs::write(&mixed, "first\r\nsecond\nthird\n").unwrap();
        let frozen = frozen_skill_package(directory.path(), "entrypoint", false).unwrap();
        let mixed_offset = "first\r\nsecond\nthird\n".find("second").unwrap();
        assert_eq!(
            frozen.resolve_anchor(
                &SourceAnchor::agent_skill_file(
                    "mixed.md",
                    SourceLocation::bytes(mixed_offset as u64, (mixed_offset + 6) as u64),
                )
                .unwrap(),
            ),
            Some((mixed, mixed_offset))
        );

        let editor_text = "\u{feff}first\r\nsecond";
        let dirty = frozen_skill_package(directory.path(), editor_text, true).unwrap();
        assert_eq!(
            dirty.resolve_anchor(
                &SourceAnchor::agent_skill_file("SKILL.md", SourceLocation::bytes(3, 8)).unwrap(),
            ),
            Some((skill, 3))
        );
    }

    #[test]
    fn selection_review_uses_full_document_coordinates_for_anchor_navigation() {
        let selection = 10..20;
        let request = build_review_request(ReviewRequestBuildRequest::new(
            ReviewTarget::Selection,
            ArtifactLens::Prompt,
            None,
            "0123456789abcdefghij",
            Some(&selection),
            SourceSnapshot::default(),
            false,
        ))
        .unwrap();
        let (request, package) = request.into_parts();
        assert!(package.is_none());
        assert_eq!(request.outbound_text(), Some("abcdefghij"));
        assert!(matches!(
            request.scope,
            ReviewScope::Selection { range, .. }
                if range == ByteRange::new(10, 20).unwrap()
        ));
        assert_eq!(
            resolve_document_anchor_offset(
                &SourceAnchor::document(SourceLocation::bytes(13, 18)),
                "0123456789abcdefghij",
            ),
            Some(13)
        );
        assert_eq!(
            resolve_document_anchor_offset(
                &SourceAnchor::document(SourceLocation::lines(2, 2)),
                "first\nsecond\n",
            ),
            Some(6)
        );
        assert_eq!(
            resolve_document_anchor_offset(&SourceAnchor::DocumentWide, "whole document"),
            Some(0)
        );
    }

    #[test]
    fn empty_or_out_of_bounds_selection_is_rejected_before_provider_resolution() {
        let source = "0123456789abcdefghij";
        let reversed = (source.len() / 2 + 1)..(source.len() / 2);
        for selection in [0..0, 9..(source.len() + 1), reversed] {
            assert!(matches!(
                build_review_request(ReviewRequestBuildRequest::new(
                    ReviewTarget::Selection,
                    ArtifactLens::Prompt,
                    None,
                    source,
                    Some(&selection),
                    SourceSnapshot::default(),
                    false,
                )),
                Err(ReviewRequestBuildError::InvalidSelection)
            ));
        }
    }

    #[test]
    fn supporting_path_membership_excludes_entrypoint_and_omissions() {
        let directory = tempfile::tempdir().unwrap();
        fs::write(directory.path().join("SKILL.md"), "entrypoint").unwrap();
        fs::write(directory.path().join("guide.md"), "guide").unwrap();
        let frozen = frozen_skill_package(directory.path(), "entrypoint", false).unwrap();

        assert!(frozen.contains_supporting_path(&directory.path().join("guide.md")));
        assert!(!frozen.contains_supporting_path(&directory.path().join("SKILL.md")));
        assert!(!frozen.contains_supporting_path(&directory.path().join("absent.md")));
    }
}
