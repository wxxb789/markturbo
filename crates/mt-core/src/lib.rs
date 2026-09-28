//! Headless markturbo workflows and document engine.
//!
//! This crate has no GPUI dependency. Document editing and outbound-request
//! policy can run in headless tools or behind the desktop presentation layer.

pub mod agent_artifacts;
pub mod credentials;
pub mod diagnostic;
pub mod document;
pub mod model;
pub mod recovery;
pub mod rendering;
pub mod review;
pub mod runtime_paths;
pub mod settings;
pub mod translate;
pub mod workspace;

pub use agent_artifacts::harness::{GlobalRoot, Harness};
pub use agent_artifacts::instruction::Instruction;
pub use agent_artifacts::skill::{Discovery, Origin, Skill, SkillMeta};
pub use diagnostic::{Diagnostic, Severity};
pub use document::block::{Block, BlockKind, DiagramKind};
pub use document::doc::Document;
pub use document::doctype::DocType;
pub use document::outline::{Heading, Outline};
pub use review::{
    ArtifactLens, ByteRange, ClarificationPriority, ClarificationQuestion, Finding, FindingKind,
    MAX_CLARIFICATION_QUESTIONS, MAX_SKILL_FILE_BYTES, MAX_SKILL_PACKAGE_BYTES,
    REVIEW_SCHEMA_VERSION, ReviewDiagnostic, ReviewDiagnosticCode, ReviewModelOutput,
    ReviewRequest, ReviewResult, ReviewScope, ReviewScopeKind, ReviewSections, ReviewSource,
    ReviewStatus, SkillFilePayload, SkillPackage, SkillPackageError, SkillPackageFile,
    SkillPackageOmission, SkillSourceFrame, SourceAnchor, SourceLocation, SourceSnapshot,
    StructuredText,
};
