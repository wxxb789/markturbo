//! Frozen Agent Skill package snapshots and headless Review request building.
//!
//! The snapshot owns the exact inventory and bytes disclosed to Review. Reads
//! fail closed for symbolic links and revalidate against the same file identity
//! before a result may be used for navigation or another operation.

use std::ffi::OsStr;
use std::fmt;
use std::fs::{self as std_fs, File};
use std::io::Read;
use std::ops::Range;
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;
use std::time::SystemTime;

use sha2::{Digest as _, Sha256};

use crate::document::io::{
    self as document_io, FileObjectId, SkillObjectIdentity, SkillOrigin, SkillOriginRoot,
};
use crate::review::{
    ArtifactLens, ByteRange, MAX_SKILL_FILE_BYTES, MAX_SKILL_PACKAGE_BYTES,
    ReviewRequest as DocumentReviewRequest, ReviewValidationError, SkillFilePayload, SkillPackage,
    SkillPackageError, SkillPackageFile, SkillPackageOmission, SourceAnchor, SourceLocation,
    SourceSnapshot,
};

const MAX_AGENT_SKILL_INVENTORY_ENTRIES: usize = 1_024;
const MAX_AGENT_SKILL_DIRECTORY_DEPTH: usize = 32;

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
    entrypoint_identity: Option<SkillSupportingFileIdentity>,
    package: SkillPackage,
    supporting_files: Vec<SkillSupportingFileIdentity>,
}

#[derive(Clone, PartialEq, Eq)]
struct SkillSupportingFileIdentity {
    path: String,
    byte_size: u64,
    digest: [u8; 32],
    modified: Option<SystemTime>,
    object_identity: SkillObjectIdentity,
    object_id: Option<FileObjectId>,
    editor_transform: Option<SkillEditorTextTransform>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
struct SkillEditorTextTransform {
    strip_utf8_bom: bool,
    normalize_crlf: bool,
}

impl SkillEditorTextTransform {
    const IDENTITY: Self = Self {
        strip_utf8_bom: false,
        normalize_crlf: false,
    };
}

struct ReadSkillSupportingFile {
    bytes: Vec<u8>,
    identity: SkillSupportingFileIdentity,
}

struct InventoriedSkillFile {
    relative_path: String,
    file: File,
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
            skill_package_path(self.inner.origin.canonical_root(), &file.path)
                .is_ok_and(|path| document_io::paths_match(&path, candidate))
        })
    }

    /// Read a disk-backed package file through the authenticated root,
    /// refusing links and returning bytes only when they match the frozen
    /// identity and digest. Editor-only SKILL.md content returns `None`.
    pub fn read_frozen_path(&self, candidate: &Path) -> Option<Vec<u8>> {
        let relative_path = normalized_skill_origin_path(&self.inner.origin, candidate).ok()?;
        let expected = if relative_path == "SKILL.md" {
            self.inner.entrypoint_identity.as_ref()
        } else {
            self.inner
                .supporting_files
                .iter()
                .find(|source| source.path == relative_path)
        };
        let expected = expected?;
        let root = self.inner.origin.open_root().ok()?;
        let file = open_skill_file(&root, &relative_path).ok()?;
        let (bytes, _) = read_opened_frozen_source(file, expected)?;
        self.inner.origin.open_root().ok()?;
        Some(bytes)
    }

    /// Build an editable source document for a frozen supporting file without
    /// reopening its path. The returned file has no Skill entrypoint origin.
    pub fn load_frozen_supporting_file(
        &self,
        candidate: &Path,
    ) -> Option<crate::document::io::LoadedFile> {
        let relative_path = normalized_skill_origin_path(&self.inner.origin, candidate).ok()?;
        if relative_path == "SKILL.md" {
            return None;
        }
        let expected = self
            .inner
            .supporting_files
            .iter()
            .find(|source| source.path == relative_path)?;
        let bytes = self.read_frozen_path(candidate)?;
        let stamp = document_io::FileStamp {
            modified: expected.modified,
            len: expected.byte_size,
            digest: expected.digest,
            object_id: expected.object_id,
        };
        let canonical_path =
            skill_package_path(self.inner.origin.canonical_root(), &relative_path).ok()?;
        document_io::loaded_file_from_frozen_snapshot(&canonical_path, bytes, stamp).ok()
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
                    SkillEditorTextTransform::IDENTITY
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
        skill_package_navigation_path(&self.inner.origin, path)
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
    let root = origin
        .open_root()
        .map_err(|_| ReviewRequestBuildError::AgentSkillSourceChanged)?;
    #[cfg(test)]
    run_skill_root_validated_hook();

    let allow_dirty_entrypoint = matches!(requested_entrypoint, RequestedEntrypoint::EditorText(_));
    let mut inventoried_files = Vec::new();
    let mut omissions = Vec::new();
    let mut held_directories = Vec::new();
    let mut visited_directories = 1;
    collect_agent_skill_paths(
        &root,
        root.directory(),
        root.canonical_root(),
        "",
        &mut inventoried_files,
        &mut omissions,
        &mut held_directories,
        &mut visited_directories,
        0,
        allow_dirty_entrypoint,
    )?;
    inventoried_files.sort_by(|left, right| {
        left.relative_path
            .as_bytes()
            .cmp(right.relative_path.as_bytes())
    });
    omissions.sort_by(|left, right| left.path.as_bytes().cmp(right.path.as_bytes()));

    let mut files = Vec::with_capacity(inventoried_files.len().saturating_add(1));
    let mut supporting_files = Vec::with_capacity(inventoried_files.len().saturating_sub(1));
    let mut entrypoint_identity = None;
    let mut total_byte_size = 0_u64;
    let mut found_entrypoint = false;
    for inventoried in inventoried_files {
        let relative_path = inventoried.relative_path;
        if relative_path == "SKILL.md"
            && matches!(requested_entrypoint, RequestedEntrypoint::EditorText(_))
        {
            continue;
        }
        let source = read_opened_skill_supporting_file(inventoried.file, &relative_path)?;
        let is_entrypoint = relative_path == "SKILL.md";
        if is_entrypoint {
            if !origin.matches_entrypoint(
                source.identity.object_identity,
                source.identity.byte_size,
                &source.identity.digest,
                source.identity.modified,
                source.identity.object_id,
            ) {
                return Err(ReviewRequestBuildError::AgentSkillSourceChanged);
            }
            found_entrypoint = true;
            entrypoint_identity = Some(source.identity.clone());
        } else {
            supporting_files.push(source.identity.clone());
        }

        let byte_size = source.identity.byte_size;
        let file = skill_package_file_from_disk_bytes(
            &relative_path,
            &source.bytes,
            if is_entrypoint {
                "skill entrypoint"
            } else {
                "supporting file"
            },
        )?;
        total_byte_size = total_byte_size.checked_add(byte_size).ok_or(
            ReviewRequestBuildError::AgentSkillPackageTooLarge {
                byte_size: u64::MAX,
            },
        )?;
        if total_byte_size > MAX_SKILL_PACKAGE_BYTES {
            return Err(ReviewRequestBuildError::AgentSkillPackageTooLarge {
                byte_size: total_byte_size,
            });
        }
        files.push(file);
    }

    if allow_dirty_entrypoint {
        // The editor snapshot remains authoritative even if the root entrypoint
        // is missing, a link, unreadable, or too large on disk.
        omissions.retain(|omission| omission.path != "SKILL.md");
        if files.len().saturating_add(omissions.len()) >= MAX_AGENT_SKILL_INVENTORY_ENTRIES {
            return Err(ReviewRequestBuildError::AgentSkillPackageTooLarge {
                byte_size: u64::MAX,
            });
        }
        let RequestedEntrypoint::EditorText(text) = requested_entrypoint else {
            unreachable!("dirty Agent Skill source must use editor text");
        };
        let byte_size = text.len() as u64;
        if byte_size > MAX_SKILL_FILE_BYTES {
            return Err(ReviewRequestBuildError::AgentSkillFileTooLarge { byte_size });
        }
        let file = SkillPackageFile::text("SKILL.md", text, "skill entrypoint")
            .map_err(ReviewRequestBuildError::InvalidAgentSkillPackage)?;
        total_byte_size = total_byte_size.checked_add(byte_size).ok_or(
            ReviewRequestBuildError::AgentSkillPackageTooLarge {
                byte_size: u64::MAX,
            },
        )?;
        if total_byte_size > MAX_SKILL_PACKAGE_BYTES {
            return Err(ReviewRequestBuildError::AgentSkillPackageTooLarge {
                byte_size: total_byte_size,
            });
        }
        files.push(file);
        found_entrypoint = true;
    }
    if !found_entrypoint {
        return Err(ReviewRequestBuildError::AgentSkillSourceChanged);
    }

    // The canonical descriptor remained authoritative throughout traversal;
    // this second check rejects an ancestor alias changed during inventory.
    origin
        .open_root()
        .map_err(|_| ReviewRequestBuildError::AgentSkillSourceChanged)?;

    let entrypoint_source = match requested_entrypoint {
        RequestedEntrypoint::Disk => EntrypointSource::Disk,
        RequestedEntrypoint::EditorText(_) => EntrypointSource::EditorText,
    };
    let package = SkillPackage::new(files, omissions)
        .map_err(ReviewRequestBuildError::InvalidAgentSkillPackage)?;
    Ok(FrozenSkillPackage {
        inner: Arc::new(FrozenSkillPackageInner {
            origin: origin.clone(),
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

fn collect_agent_skill_paths(
    root: &SkillOriginRoot,
    directory: &File,
    directory_path: &Path,
    relative_directory: &str,
    files: &mut Vec<InventoriedSkillFile>,
    omissions: &mut Vec<SkillPackageOmission>,
    held_directories: &mut Vec<File>,
    visited_directories: &mut usize,
    depth: usize,
    allow_dirty_entrypoint: bool,
) -> Result<(), ReviewRequestBuildError> {
    if depth > MAX_AGENT_SKILL_DIRECTORY_DEPTH {
        return Err(ReviewRequestBuildError::AgentSkillPackageTooLarge {
            byte_size: u64::MAX,
        });
    }
    visit_skill_directory_entries(directory, directory_path, &mut |entry| {
        collect_agent_skill_entry(
            root,
            directory,
            directory_path,
            relative_directory,
            files,
            omissions,
            held_directories,
            visited_directories,
            depth,
            allow_dirty_entrypoint,
            entry,
        )
    })
}

fn collect_agent_skill_entry(
    root: &SkillOriginRoot,
    directory: &File,
    directory_path: &Path,
    relative_directory: &str,
    files: &mut Vec<InventoriedSkillFile>,
    omissions: &mut Vec<SkillPackageOmission>,
    held_directories: &mut Vec<File>,
    visited_directories: &mut usize,
    depth: usize,
    allow_dirty_entrypoint: bool,
    entry: SkillDirectoryEntry,
) -> Result<(), ReviewRequestBuildError> {
    #[cfg(windows)]
    let _ = directory;
    let name = entry.name;
    let file_name = name
        .to_str()
        .ok_or(ReviewRequestBuildError::AgentSkillPathIsNotUtf8)?;
    let relative_path = if relative_directory.is_empty() {
        file_name.to_owned()
    } else {
        let mut path = String::with_capacity(relative_directory.len() + 1 + file_name.len());
        path.push_str(relative_directory);
        path.push('/');
        path.push_str(file_name);
        path
    };
    validated_skill_relative_components(&relative_path)?;

    #[cfg(all(unix, any(target_os = "linux", target_os = "macos")))]
    if allow_dirty_entrypoint
        && relative_path == "SKILL.md"
        && entry.kind == Some(UNIX_DT_DIRECTORY)
    {
        return Err(ReviewRequestBuildError::AgentSkillSourceChanged);
    }

    #[cfg(windows)]
    {
        let path = directory_path.join(&name);
        let metadata = match std_fs::symlink_metadata(&path) {
            Ok(metadata) => metadata,
            Err(error)
                if allow_dirty_entrypoint
                    && relative_path == "SKILL.md"
                    && error.kind() == std::io::ErrorKind::NotFound =>
            {
                return Ok(());
            }
            Err(_) => return Err(ReviewRequestBuildError::AgentSkillReadFailed),
        };
        if windows_metadata_is_reparse(&metadata) {
            push_skill_omission(
                files,
                omissions,
                allow_dirty_entrypoint,
                SkillPackageOmission::symlink(relative_path, "symbolic link omitted")
                    .map_err(ReviewRequestBuildError::InvalidAgentSkillPackage)?,
            )?;
        } else if metadata.is_dir() && allow_dirty_entrypoint && relative_path == "SKILL.md" {
            return Err(ReviewRequestBuildError::AgentSkillSourceChanged);
        } else if metadata.is_dir() {
            record_skill_directory(visited_directories)?;
            let child = hold_regular_skill_directory(&path)?;
            collect_agent_skill_paths(
                root,
                &child,
                &path,
                &relative_path,
                files,
                omissions,
                held_directories,
                visited_directories,
                depth.saturating_add(1),
                allow_dirty_entrypoint,
            )?;
            held_directories.push(child);
        } else if metadata.is_file() {
            if allow_dirty_entrypoint && relative_path == "SKILL.md" {
                return Ok(());
            }
            ensure_skill_inventory_room(files, omissions, allow_dirty_entrypoint)?;
            let file = open_regular_skill_file(&path)
                .map_err(|_| ReviewRequestBuildError::AgentSkillReadFailed)?;
            if !opened_skill_file_is_regular(
                &file
                    .metadata()
                    .map_err(|_| ReviewRequestBuildError::AgentSkillReadFailed)?,
            ) {
                return Err(ReviewRequestBuildError::AgentSkillSourceChanged);
            }
            files.push(InventoriedSkillFile {
                relative_path,
                file,
            });
        } else {
            push_skill_omission(
                files,
                omissions,
                allow_dirty_entrypoint,
                SkillPackageOmission::new(relative_path, "non-regular file omitted", false)
                    .map_err(ReviewRequestBuildError::InvalidAgentSkillPackage)?,
            )?;
        }
    }

    #[cfg(all(unix, any(target_os = "linux", target_os = "macos")))]
    {
        let child = match open_skill_child_at(directory, &name) {
            Ok(child) => child,
            Err(_) if skill_child_is_symlink_at(directory, &name) => {
                push_skill_omission(
                    files,
                    omissions,
                    allow_dirty_entrypoint,
                    SkillPackageOmission::symlink(relative_path, "symbolic link omitted")
                        .map_err(ReviewRequestBuildError::InvalidAgentSkillPackage)?,
                )?;
                return Ok(());
            }
            Err(error)
                if allow_dirty_entrypoint
                    && relative_path == "SKILL.md"
                    && (error.kind() == std::io::ErrorKind::NotFound
                        || entry.kind == Some(UNIX_DT_REGULAR)) =>
            {
                return Ok(());
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Err(ReviewRequestBuildError::AgentSkillSourceChanged);
            }
            Err(_) => return Err(ReviewRequestBuildError::AgentSkillReadFailed),
        };
        let metadata = child
            .metadata()
            .map_err(|_| ReviewRequestBuildError::AgentSkillReadFailed)?;
        if entry.inode.is_some_and(|inode| {
            use std::os::unix::fs::MetadataExt as _;
            metadata.ino() != inode
        }) {
            return Err(ReviewRequestBuildError::AgentSkillSourceChanged);
        }
        if metadata.file_type().is_symlink() {
            push_skill_omission(
                files,
                omissions,
                allow_dirty_entrypoint,
                SkillPackageOmission::symlink(relative_path, "symbolic link omitted")
                    .map_err(ReviewRequestBuildError::InvalidAgentSkillPackage)?,
            )?;
        } else if metadata.is_dir() && allow_dirty_entrypoint && relative_path == "SKILL.md" {
            return Err(ReviewRequestBuildError::AgentSkillSourceChanged);
        } else if metadata.is_dir() {
            record_skill_directory(visited_directories)?;
            collect_agent_skill_paths(
                root,
                &child,
                &directory_path.join(&name),
                &relative_path,
                files,
                omissions,
                held_directories,
                visited_directories,
                depth.saturating_add(1),
                allow_dirty_entrypoint,
            )?;
            held_directories.push(child);
        } else if metadata.is_file() {
            if allow_dirty_entrypoint && relative_path == "SKILL.md" {
                return Ok(());
            }
            ensure_skill_inventory_room(files, omissions, allow_dirty_entrypoint)?;
            files.push(InventoriedSkillFile {
                relative_path,
                file: child,
            });
        } else {
            push_skill_omission(
                files,
                omissions,
                allow_dirty_entrypoint,
                SkillPackageOmission::new(relative_path, "non-regular file omitted", false)
                    .map_err(ReviewRequestBuildError::InvalidAgentSkillPackage)?,
            )?;
        }
    }
    Ok(())
}

fn ensure_skill_inventory_room(
    files: &[InventoriedSkillFile],
    omissions: &[SkillPackageOmission],
    allow_dirty_entrypoint: bool,
) -> Result<(), ReviewRequestBuildError> {
    let omission_count = if allow_dirty_entrypoint {
        omissions
            .iter()
            .filter(|omission| omission.path != "SKILL.md")
            .count()
    } else {
        omissions.len()
    };
    if files.len().saturating_add(omission_count) >= MAX_AGENT_SKILL_INVENTORY_ENTRIES {
        Err(ReviewRequestBuildError::AgentSkillPackageTooLarge {
            byte_size: u64::MAX,
        })
    } else {
        Ok(())
    }
}

fn record_skill_directory(visited_directories: &mut usize) -> Result<(), ReviewRequestBuildError> {
    *visited_directories = visited_directories.checked_add(1).ok_or(
        ReviewRequestBuildError::AgentSkillPackageTooLarge {
            byte_size: u64::MAX,
        },
    )?;
    if *visited_directories > MAX_AGENT_SKILL_INVENTORY_ENTRIES {
        Err(ReviewRequestBuildError::AgentSkillPackageTooLarge {
            byte_size: u64::MAX,
        })
    } else {
        Ok(())
    }
}

fn push_skill_omission(
    files: &[InventoriedSkillFile],
    omissions: &mut Vec<SkillPackageOmission>,
    allow_dirty_entrypoint: bool,
    omission: SkillPackageOmission,
) -> Result<(), ReviewRequestBuildError> {
    ensure_skill_inventory_room(files, omissions, allow_dirty_entrypoint)?;
    omissions.push(omission);
    Ok(())
}

struct SkillDirectoryEntry {
    name: std::ffi::OsString,
    #[cfg(unix)]
    inode: Option<u64>,
    #[cfg(unix)]
    kind: Option<u8>,
}

#[cfg(windows)]
fn visit_skill_directory_entries(
    _directory: &File,
    path: &Path,
    visit: &mut impl FnMut(SkillDirectoryEntry) -> Result<(), ReviewRequestBuildError>,
) -> Result<(), ReviewRequestBuildError> {
    for entry in
        std_fs::read_dir(path).map_err(|_| ReviewRequestBuildError::AgentSkillReadFailed)?
    {
        let entry = entry.map_err(|_| ReviewRequestBuildError::AgentSkillReadFailed)?;
        visit(SkillDirectoryEntry {
            name: entry.file_name(),
        })?;
    }
    Ok(())
}

#[cfg(all(unix, any(target_os = "linux", target_os = "macos")))]
fn visit_skill_directory_entries(
    directory: &File,
    _path: &Path,
    visit: &mut impl FnMut(SkillDirectoryEntry) -> Result<(), ReviewRequestBuildError>,
) -> Result<(), ReviewRequestBuildError> {
    read_skill_directory_descriptor(directory, visit)
}

#[cfg(all(unix, target_os = "linux"))]
#[repr(C)]
struct SkillDirectoryRecord {
    inode: u64,
    _offset: i64,
    _record_length: u16,
    file_type: u8,
    name: [std::ffi::c_char; 0],
}

#[cfg(all(unix, target_os = "macos"))]
#[repr(C)]
struct SkillDirectoryRecord {
    inode: u64,
    _seek_offset: u64,
    _record_length: u16,
    _name_length: u16,
    file_type: u8,
    name: [std::ffi::c_char; 0],
}

#[cfg(all(unix, any(target_os = "linux", target_os = "macos")))]
fn read_skill_directory_descriptor(
    directory: &File,
    visit: &mut impl FnMut(SkillDirectoryEntry) -> Result<(), ReviewRequestBuildError>,
) -> Result<(), ReviewRequestBuildError> {
    use std::ffi::{CStr, c_char, c_int};
    use std::os::fd::IntoRawFd as _;
    use std::os::unix::ffi::OsStringExt as _;

    #[repr(C)]
    struct DirectoryStream {
        _opaque: [u8; 0],
    }

    unsafe extern "C" {
        fn fdopendir(descriptor: c_int) -> *mut DirectoryStream;
        fn readdir(directory: *mut DirectoryStream) -> *mut SkillDirectoryRecord;
        fn closedir(directory: *mut DirectoryStream) -> c_int;
        fn close(descriptor: c_int) -> c_int;
        #[cfg(target_os = "linux")]
        fn __errno_location() -> *mut c_int;
        #[cfg(target_os = "macos")]
        fn __error() -> *mut c_int;
    }

    struct Stream(*mut DirectoryStream);
    impl Drop for Stream {
        fn drop(&mut self) {
            // SAFETY: the stream owns the descriptor transferred to fdopendir.
            unsafe {
                closedir(self.0);
            }
        }
    }

    let descriptor = directory
        .try_clone()
        .map_err(|_| ReviewRequestBuildError::AgentSkillReadFailed)?
        .into_raw_fd();
    // SAFETY: `descriptor` is an owned duplicate of an open directory fd.
    let stream = unsafe { fdopendir(descriptor) };
    if stream.is_null() {
        // SAFETY: fdopendir did not take ownership when it failed.
        unsafe {
            close(descriptor);
        }
        return Err(ReviewRequestBuildError::AgentSkillReadFailed);
    }
    let stream = Stream(stream);
    loop {
        // POSIX readdir signals end-of-directory and errors with the same null
        // pointer, so clear and inspect errno for each call.
        #[cfg(target_os = "linux")]
        let errno = unsafe { __errno_location() };
        #[cfg(target_os = "macos")]
        let errno = unsafe { __error() };
        unsafe { *errno = 0 };
        // SAFETY: `stream` remains live until the loop exits.
        let record = unsafe { readdir(stream.0) };
        if record.is_null() {
            if unsafe { *errno } != 0 {
                return Err(ReviewRequestBuildError::AgentSkillReadFailed);
            }
            break;
        }
        let name_offset = std::mem::offset_of!(SkillDirectoryRecord, name);
        // SAFETY: `readdir` returned a valid native dirent whose name field is
        // a NUL-terminated byte string for the lifetime of the next call.
        let name =
            unsafe { CStr::from_ptr((record.cast::<u8>().add(name_offset)).cast::<c_char>()) };
        let bytes = name.to_bytes();
        if bytes == b"." || bytes == b".." {
            continue;
        }
        // SAFETY: `record` points to a valid platform dirent with the declared
        // ABI layout for this target.
        let inode = unsafe { (*record).inode };
        let kind = unsafe { (*record).file_type };
        visit(SkillDirectoryEntry {
            name: std::ffi::OsString::from_vec(bytes.to_vec()),
            inode: Some(inode),
            kind: Some(kind),
        })?;
    }
    Ok(())
}

#[cfg(all(unix, not(any(target_os = "linux", target_os = "macos"))))]
fn visit_skill_directory_entries(
    _directory: &File,
    _path: &Path,
    _visit: &mut impl FnMut(SkillDirectoryEntry) -> Result<(), ReviewRequestBuildError>,
) -> Result<(), ReviewRequestBuildError> {
    Err(ReviewRequestBuildError::AgentSkillReadFailed)
}

#[cfg(not(any(windows, unix)))]
fn visit_skill_directory_entries(
    _directory: &File,
    _path: &Path,
    _visit: &mut impl FnMut(SkillDirectoryEntry) -> Result<(), ReviewRequestBuildError>,
) -> Result<(), ReviewRequestBuildError> {
    Err(ReviewRequestBuildError::AgentSkillReadFailed)
}

/// On Windows, directory handles deny write/delete sharing. Every directory
/// reached below the authenticated root remains held through file reads.
#[cfg(windows)]
fn hold_regular_skill_directory(path: &Path) -> Result<File, ReviewRequestBuildError> {
    use std::os::windows::fs::{MetadataExt as _, OpenOptionsExt as _};
    use windows::Win32::Storage::FileSystem::{
        FILE_ATTRIBUTE_REPARSE_POINT, FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT,
        FILE_SHARE_READ,
    };

    let file = std_fs::OpenOptions::new()
        .read(true)
        .share_mode(FILE_SHARE_READ.0)
        .custom_flags(FILE_FLAG_BACKUP_SEMANTICS.0 | FILE_FLAG_OPEN_REPARSE_POINT.0)
        .open(path)
        .map_err(|_| ReviewRequestBuildError::AgentSkillReadFailed)?;
    let metadata = file
        .metadata()
        .map_err(|_| ReviewRequestBuildError::AgentSkillReadFailed)?;
    if !metadata.is_dir() || metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT.0 != 0 {
        return Err(ReviewRequestBuildError::AgentSkillSourceChanged);
    }
    Ok(file)
}

#[cfg(test)]
fn read_regular_skill_supporting_file(
    root: &SkillOriginRoot,
    relative_path: &str,
) -> Result<ReadSkillSupportingFile, ReviewRequestBuildError> {
    let file = open_skill_file(root, relative_path)
        .map_err(|_| ReviewRequestBuildError::AgentSkillReadFailed)?;
    read_opened_skill_supporting_file(file, relative_path)
}

fn read_opened_skill_supporting_file(
    mut file: File,
    relative_path: &str,
) -> Result<ReadSkillSupportingFile, ReviewRequestBuildError> {
    let before = file
        .metadata()
        .map_err(|_| ReviewRequestBuildError::AgentSkillReadFailed)?;
    if !opened_skill_file_is_regular(&before) {
        return Err(ReviewRequestBuildError::AgentSkillSourceChanged);
    }
    if before.len() > MAX_SKILL_FILE_BYTES {
        return Err(ReviewRequestBuildError::AgentSkillFileTooLarge {
            byte_size: before.len(),
        });
    }
    let object_id = document_io::file_object_id(&file)
        .map_err(|_| ReviewRequestBuildError::AgentSkillReadFailed)?;
    let object_identity = document_io::skill_object_identity(&file)
        .map_err(|_| ReviewRequestBuildError::AgentSkillReadFailed)?;
    let mut bytes = Vec::with_capacity(usize::try_from(before.len()).unwrap_or(0));
    file.by_ref()
        .take(MAX_SKILL_FILE_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| ReviewRequestBuildError::AgentSkillReadFailed)?;
    if bytes.len() as u64 > MAX_SKILL_FILE_BYTES {
        return Err(ReviewRequestBuildError::AgentSkillFileTooLarge {
            byte_size: bytes.len() as u64,
        });
    }
    let after = file
        .metadata()
        .map_err(|_| ReviewRequestBuildError::AgentSkillReadFailed)?;
    if !opened_skill_file_is_regular(&after)
        || before.len() != bytes.len() as u64
        || after.len() != bytes.len() as u64
        || before.modified().ok() != after.modified().ok()
        || document_io::skill_object_identity(&file)
            .map_err(|_| ReviewRequestBuildError::AgentSkillReadFailed)?
            != object_identity
    {
        return Err(ReviewRequestBuildError::AgentSkillSourceChanged);
    }
    Ok(ReadSkillSupportingFile {
        identity: SkillSupportingFileIdentity {
            path: relative_path.to_owned(),
            byte_size: after.len(),
            digest: Sha256::digest(&bytes).into(),
            modified: after.modified().ok(),
            object_identity,
            object_id,
            editor_transform: skill_editor_text_transform(&bytes),
        },
        bytes,
    })
}

fn read_opened_frozen_source(
    mut file: File,
    expected: &SkillSupportingFileIdentity,
) -> Option<(Vec<u8>, document_io::FileStamp)> {
    let Ok(before) = file.metadata() else {
        return None;
    };
    if !opened_skill_file_is_regular(&before)
        || before.len() != expected.byte_size
        || before.modified().ok() != expected.modified
    {
        return None;
    }
    let Ok(object_identity) = document_io::skill_object_identity(&file) else {
        return None;
    };
    let Ok(object_id) = document_io::file_object_id(&file) else {
        return None;
    };
    if object_identity != expected.object_identity || object_id != expected.object_id {
        return None;
    }

    let mut bytes = Vec::with_capacity(usize::try_from(expected.byte_size).ok()?);
    file.by_ref()
        .take(MAX_SKILL_FILE_BYTES + 1)
        .read_to_end(&mut bytes)
        .ok()?;
    let byte_size = bytes.len() as u64;
    if byte_size > MAX_SKILL_FILE_BYTES {
        return None;
    }

    let Ok(after) = file.metadata() else {
        return None;
    };
    let digest: [u8; 32] = Sha256::digest(&bytes).into();
    if byte_size != expected.byte_size
        || !opened_skill_file_is_regular(&after)
        || after.len() != byte_size
        || after.modified().ok() != expected.modified
        || document_io::skill_object_identity(&file).ok() != Some(expected.object_identity)
        || document_io::file_object_id(&file).ok() != Some(expected.object_id)
        || digest != expected.digest
    {
        return None;
    }
    let stamp = document_io::FileStamp::from_bytes(&after, &bytes, object_id);
    Some((bytes, stamp))
}

fn open_skill_file(root: &SkillOriginRoot, relative_path: &str) -> std::io::Result<File> {
    let components =
        validated_skill_relative_components(relative_path).map_err(invalid_skill_path_error)?;
    open_skill_file_components(root, &components)
}

#[cfg(windows)]
fn open_skill_file_components(
    root: &SkillOriginRoot,
    components: &[&str],
) -> std::io::Result<File> {
    let (filename, directories) = components
        .split_last()
        .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::InvalidInput, "empty Skill path"))?;
    let mut path = root.canonical_root().to_path_buf();
    let mut held = Vec::with_capacity(directories.len());
    for component in directories {
        path.push(component);
        held.push(hold_regular_skill_directory(&path).map_err(review_error_to_io)?);
    }
    path.push(filename);
    open_regular_skill_file(&path)
}

#[cfg(windows)]
fn open_regular_skill_file(path: &Path) -> std::io::Result<File> {
    use std::os::windows::fs::OpenOptionsExt as _;
    use windows::Win32::Storage::FileSystem::{FILE_FLAG_OPEN_REPARSE_POINT, FILE_SHARE_READ};

    std_fs::OpenOptions::new()
        .read(true)
        .share_mode(FILE_SHARE_READ.0)
        .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT.0)
        .open(path)
}

#[cfg(all(unix, any(target_os = "linux", target_os = "macos")))]
const UNIX_O_NOFOLLOW: i32 = if cfg!(target_os = "linux") {
    0o400000
} else {
    0x100
};

#[cfg(all(unix, any(target_os = "linux", target_os = "macos")))]
const UNIX_O_CLOEXEC: i32 = if cfg!(target_os = "linux") {
    0o2000000
} else {
    0x1000000
};

#[cfg(all(unix, any(target_os = "linux", target_os = "macos")))]
const UNIX_O_NONBLOCK: i32 = if cfg!(target_os = "linux") {
    0o4000
} else {
    0x4
};

#[cfg(all(unix, any(target_os = "linux", target_os = "macos")))]
const UNIX_DT_DIRECTORY: u8 = 4;

#[cfg(all(unix, any(target_os = "linux", target_os = "macos")))]
const UNIX_DT_REGULAR: u8 = 8;

#[cfg(all(unix, any(target_os = "linux", target_os = "macos")))]
fn open_skill_child_at(directory: &File, name: &std::ffi::OsStr) -> std::io::Result<File> {
    use std::ffi::{CString, c_char, c_int};
    use std::os::fd::{AsRawFd as _, FromRawFd as _};
    use std::os::unix::ffi::OsStrExt as _;

    unsafe extern "C" {
        fn openat(directory: c_int, path: *const c_char, flags: c_int) -> c_int;
    }

    let name = CString::new(name.as_bytes())
        .map_err(|_| std::io::Error::new(std::io::ErrorKind::InvalidInput, "NUL in Skill name"))?;
    // O_NOFOLLOW makes the opened descriptor authoritative; O_NONBLOCK keeps
    // a raced FIFO from stalling inventory before it can be classified.
    // SAFETY: `directory` is a live directory fd and `name` is one component.
    let descriptor = unsafe {
        openat(
            directory.as_raw_fd(),
            name.as_ptr(),
            UNIX_O_NOFOLLOW | UNIX_O_NONBLOCK | UNIX_O_CLOEXEC,
        )
    };
    if descriptor < 0 {
        return Err(std::io::Error::last_os_error());
    }
    // SAFETY: `openat` returned a new owned descriptor.
    Ok(unsafe { File::from_raw_fd(descriptor) })
}

#[cfg(all(unix, any(target_os = "linux", target_os = "macos")))]
fn skill_child_is_symlink_at(directory: &File, name: &std::ffi::OsStr) -> bool {
    use std::ffi::{CString, c_char, c_int, c_void};
    use std::os::fd::AsRawFd as _;
    use std::os::unix::ffi::OsStrExt as _;

    unsafe extern "C" {
        fn readlinkat(
            directory: c_int,
            path: *const c_char,
            buffer: *mut c_char,
            size: usize,
        ) -> isize;
    }

    let Ok(name) = CString::new(name.as_bytes()) else {
        return false;
    };
    let mut probe = 0_u8;
    // SAFETY: `directory` is a live directory fd, the name is NUL-terminated,
    // and `probe` supplies one writable byte; no target is followed.
    unsafe {
        readlinkat(
            directory.as_raw_fd(),
            name.as_ptr(),
            (&raw mut probe).cast::<c_void>().cast::<c_char>(),
            1,
        ) >= 0
    }
}

#[cfg(all(unix, any(target_os = "linux", target_os = "macos")))]
fn open_skill_file_components(
    root: &SkillOriginRoot,
    components: &[&str],
) -> std::io::Result<File> {
    let mut directory = root.directory().try_clone()?;
    for (index, component) in components.iter().enumerate() {
        let next = open_skill_child_at(&directory, std::ffi::OsStr::new(component))?;
        let metadata = next.metadata()?;
        if index + 1 == components.len() {
            if !metadata.is_file() || metadata.file_type().is_symlink() {
                return Err(std::io::Error::other(
                    "Agent Skill source is not a regular file",
                ));
            }
            return Ok(next);
        }
        if !metadata.is_dir() || metadata.file_type().is_symlink() {
            return Err(std::io::Error::other(
                "Agent Skill path component is not a directory",
            ));
        }
        directory = next;
    }
    Err(std::io::Error::new(
        std::io::ErrorKind::InvalidInput,
        "Agent Skill supporting path is empty",
    ))
}

#[cfg(all(unix, not(any(target_os = "linux", target_os = "macos"))))]
fn open_skill_file_components(_: &SkillOriginRoot, _: &[&str]) -> std::io::Result<File> {
    Err(std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        "no descriptor-relative Agent Skill file open is available on this target",
    ))
}

#[cfg(not(any(windows, unix)))]
fn open_skill_file_components(_: &SkillOriginRoot, _: &[&str]) -> std::io::Result<File> {
    Err(std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        "no no-follow Agent Skill file open is available on this target",
    ))
}

#[cfg(windows)]
fn windows_metadata_is_reparse(metadata: &std_fs::Metadata) -> bool {
    use std::os::windows::fs::MetadataExt as _;
    use windows::Win32::Storage::FileSystem::FILE_ATTRIBUTE_REPARSE_POINT;

    metadata.file_type().is_symlink()
        || metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT.0 != 0
}

#[cfg(windows)]
fn opened_skill_file_is_regular(metadata: &std_fs::Metadata) -> bool {
    use std::os::windows::fs::MetadataExt as _;
    use windows::Win32::Storage::FileSystem::FILE_ATTRIBUTE_REPARSE_POINT;

    metadata.is_file() && metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT.0 == 0
}

#[cfg(not(windows))]
fn opened_skill_file_is_regular(metadata: &std_fs::Metadata) -> bool {
    metadata.is_file() && !metadata.file_type().is_symlink()
}

#[cfg(windows)]
fn review_error_to_io(error: ReviewRequestBuildError) -> std::io::Error {
    std::io::Error::other(error.to_string())
}

#[cfg(test)]
std::thread_local! {
    static SKILL_ROOT_VALIDATED_HOOK: std::cell::RefCell<Option<Box<dyn FnOnce()>>> = const {
        std::cell::RefCell::new(None)
    };
}

#[cfg(test)]
fn install_skill_root_validated_hook(hook: impl FnOnce() + 'static) {
    SKILL_ROOT_VALIDATED_HOOK.with(|slot| {
        assert!(
            slot.borrow().is_none(),
            "a Skill root validation hook is already installed"
        );
        *slot.borrow_mut() = Some(Box::new(hook));
    });
}

#[cfg(test)]
fn run_skill_root_validated_hook() {
    SKILL_ROOT_VALIDATED_HOOK.with(|slot| {
        if let Some(hook) = slot.borrow_mut().take() {
            hook();
        }
    });
}

fn skill_package_path(
    root: &Path,
    relative_path: &str,
) -> Result<PathBuf, ReviewRequestBuildError> {
    let mut path = root.to_path_buf();
    for component in validated_skill_relative_components(relative_path)? {
        path.push(component);
    }
    Ok(path)
}

fn skill_package_navigation_path(
    origin: &SkillOrigin,
    relative_path: &str,
) -> Result<PathBuf, ReviewRequestBuildError> {
    let lexical = skill_package_path(origin.lexical_root(), relative_path)?;
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt as _;

        // Keep the original user-facing spelling for normal paths, but use
        // the canonical extended-length path when the lexical spelling would
        // exceed Win32's traditional MAX_PATH limit.
        if lexical.as_os_str().encode_wide().count() >= 260 {
            return skill_package_path(origin.canonical_root(), relative_path);
        }
    }
    Ok(lexical)
}

fn validated_skill_relative_components(
    relative_path: &str,
) -> Result<Vec<&str>, ReviewRequestBuildError> {
    let components = relative_path.split('/').collect::<Vec<_>>();
    if components.is_empty()
        || components
            .iter()
            .any(|component| component.is_empty() || matches!(*component, "." | ".."))
    {
        return Err(ReviewRequestBuildError::AgentSkillPathIsNotUtf8);
    }
    // Backslash is a legal Unix filename byte, but the package contract
    // normalizes separators to `/`; accepting it would reopen another path.
    #[cfg(unix)]
    if components.iter().any(|component| component.contains('\\')) {
        return Err(ReviewRequestBuildError::AgentSkillPathIsNotUtf8);
    }
    Ok(components)
}

fn normalized_skill_relative_path(
    root: &Path,
    path: &Path,
) -> Result<String, ReviewRequestBuildError> {
    let relative_path = path
        .strip_prefix(root)
        .map_err(|_| ReviewRequestBuildError::AgentSkillPathIsNotUtf8)?;
    let mut components = Vec::new();
    for component in relative_path.components() {
        let Component::Normal(component) = component else {
            return Err(ReviewRequestBuildError::AgentSkillPathIsNotUtf8);
        };
        components.push(
            component
                .to_str()
                .ok_or(ReviewRequestBuildError::AgentSkillPathIsNotUtf8)?,
        );
    }
    if components.is_empty() {
        return Err(ReviewRequestBuildError::AgentSkillPathIsNotUtf8);
    }
    let normalized = components.join("/");
    validated_skill_relative_components(&normalized)?;
    Ok(normalized)
}

fn normalized_skill_origin_path(
    origin: &SkillOrigin,
    path: &Path,
) -> Result<String, ReviewRequestBuildError> {
    normalized_skill_relative_path(origin.canonical_root(), path)
        .or_else(|_| normalized_skill_relative_path(origin.lexical_root(), path))
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

fn invalid_skill_path_error(error: ReviewRequestBuildError) -> std::io::Error {
    std::io::Error::new(std::io::ErrorKind::InvalidInput, error.to_string())
}

fn skill_editor_text_transform(bytes: &[u8]) -> Option<SkillEditorTextTransform> {
    let (body, strip_utf8_bom) = if bytes.starts_with(&[0xef, 0xbb, 0xbf]) {
        (&bytes[3..], true)
    } else {
        (bytes, false)
    };
    let text = std::str::from_utf8(body).ok()?;
    Some(SkillEditorTextTransform {
        strip_utf8_bom,
        normalize_crlf: document_io::Newline::detect(text) == document_io::Newline::Crlf,
    })
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
    transform: SkillEditorTextTransform,
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
    use super::{
        FrozenSkillPackage, ReviewRequestBuildError, ReviewRequestBuildRequest,
        ReviewRequestBuildResult, ReviewTarget, SkillOriginRoot, build_review_request,
        read_regular_skill_supporting_file, resolve_document_anchor_offset,
    };
    use crate::document::io::{self as document_io, SkillOrigin};
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
            super::install_skill_root_validated_hook(move || {
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
            super::install_skill_root_validated_hook(move || {
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
            super::install_skill_root_validated_hook(move || {
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
        for index in 0..super::MAX_AGENT_SKILL_INVENTORY_ENTRIES - 1 {
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
            super::MAX_AGENT_SKILL_INVENTORY_ENTRIES
        );
    }

    #[test]
    fn skill_inventory_bounds_empty_directory_traversal() {
        let directory = tempfile::tempdir().unwrap();
        fs::write(directory.path().join("SKILL.md"), "entrypoint").unwrap();
        for index in 0..super::MAX_AGENT_SKILL_INVENTORY_ENTRIES {
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
