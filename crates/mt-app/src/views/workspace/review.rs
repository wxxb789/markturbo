//! App-owned state for the Review → Approved Revision workflow.
//!
//! `Workspace` still coordinates GPUI prompts, document events, recovery I/O,
//! and the picker, but the frozen results, answer bindings, cancellation
//! tickets, freshness rules, decisions, and approval identity have one owner
//! here. Recovery itself remains owned by `Workspace`'s recovery coordinator.

use std::{
    collections::{BTreeSet, HashMap},
    path::Path,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, IconName, Sizable as _, StyledExt as _,
    button::{Button, ButtonVariants as _},
    h_flex,
    input::{Input, InputEvent, InputState},
    list::ListItem,
    spinner::Spinner,
    v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use mt_core::agent_artifacts::package::{
    FrozenSkillPackage, ReviewRequestBuildError, ReviewRequestBuildRequest,
    ReviewRequestBuildResult, ReviewTarget, build_review_request, resolve_document_anchor_offset,
};
use mt_core::document::lifecycle::{AsyncSnapshot, DocumentId};
use mt_core::model::{ConsentCapability, ConsentDecision};
use mt_core::recovery::{RecoveryKey, RevisionRecovery};
use mt_core::review::provider::{
    PreparedReview, REVIEW_REQUEST_TIMEOUT, ReviewError, ReviewLanguage, ReviewRequestError,
    ReviewTransportResult, RevisionAnswer, RevisionAnswers, RevisionError,
    RevisionQuestionCoverageStatus, RevisionTransportResult, revision_question_id,
    revision_request_binding,
};
use mt_core::review::revision::ChangeId;
use mt_core::review::{
    ArtifactLens, ClarificationPriority, FindingKind, ReviewDiagnostic, ReviewDiagnosticCode,
    ReviewModelOutput, ReviewRequest as DocumentReviewRequest, ReviewStatus, SkillPackage,
    SourceAnchor, SourceLocation, SourceSnapshot, StructuredText,
};
use mt_core::workspace::watcher::Change;
use serde::Serialize;
use sha2::{Digest as _, Sha256};

use super::{
    CancelReview, REVIEW_DIAGNOSTIC_ACCESSIBILITY_ID, REVIEW_RESULT_ACCESSIBILITY_ID,
    REVIEW_RUN_ACCESSIBILITY_ID, REVISION_ACCEPT_ALL_ACCESSIBILITY_ID,
    REVISION_APPLY_ACCESSIBILITY_ID, REVISION_COPY_ACCESSIBILITY_ID,
    REVISION_COPY_RECOVERED_ANSWERS_ACCESSIBILITY_ID, REVISION_DISCARD_ANSWERS_ACCESSIBILITY_ID,
    REVISION_DISCARD_RECOVERED_ANSWERS_ACCESSIBILITY_ID, REVISION_DISMISS_ACCESSIBILITY_ID,
    REVISION_RECOVERED_ANSWERS_ACCESSIBILITY_ID, REVISION_REJECT_ALL_ACCESSIBILITY_ID,
    REVISION_RESULT_DISMISS_ACCESSIBILITY_ID, REVISION_RETRY_ACCESSIBILITY_ID,
    REVISION_RUN_ACCESSIBILITY_ID, REVISION_SAVE_ACCESSIBILITY_ID,
    REVISION_SAVE_AS_ACCESSIBILITY_ID, REVISION_STALE_ACCESSIBILITY_ID, ReviewDocument,
    ReviewSelection, Workspace,
};
use crate::i18n;
use crate::metrics;
/// One owner for Review output and the approved Revision built from it.
///
/// Fields are intentionally private: callers observe through borrowed views
/// and mutate workflow state through transitions below, rather than updating a
/// collection of related `Workspace` fields independently.
#[derive(Default)]
pub(super) struct ReviewFlow {
    review_lens_overrides: HashMap<DocumentId, ArtifactLens>,
    review_outcome: Option<ReviewOutcome>,
    review_target: Option<ReviewTargetState>,
    pending_review: Option<PendingReview>,
    review_generation: u64,
    review_panel_open: bool,

    revision_context: Option<WorkspaceRevisionContext>,
    revision_result: Option<WorkspaceRevisionResult>,
    revision_diagnostic: Option<String>,
    revision_generation: u64,
    pending_revision: Option<PendingRevision>,
    revision_answer_subscriptions: Vec<Subscription>,

    /// Typed answer records are observations of recovery state, not checkpoint
    /// storage. Save/retirement durability remains with Workspace recovery.
    recovered_revision_records: HashMap<RecoveryKey, RevisionRecovery>,
    recovered_revision_documents: HashMap<DocumentId, RecoveryKey>,
    pending_revision_recoveries: HashMap<RecoveryKey, (DocumentId, RevisionRecovery)>,

    /// Distinguishes a later explicit Apply, including a reject-all no-op.
    revision_approval_epoch: u64,
}

enum ReviewOutcome {
    Result(WorkspaceReviewResult),
    Diagnostic(WorkspaceReviewDiagnostic),
}

#[derive(Clone, Copy)]
struct ReviewTargetState {
    target: mt_core::agent_artifacts::package::ReviewTarget,
    document_id: DocumentId,
}

/// UI-owned metadata around an immutable provider result.
///
/// The provider result contains only validated, inert structured data. The
/// flow adds the document identity and exact editor snapshot needed to decide
/// whether it is still current. Selection anchors remain in canonical
/// document coordinates; the selected range only explains omitted context.
pub(super) struct WorkspaceReviewResult {
    pub(super) document_id: DocumentId,
    pub(super) source_snapshot: AsyncSnapshot,
    pub(super) target: mt_core::agent_artifacts::package::ReviewTarget,
    pub(super) selection: Option<std::ops::Range<usize>>,
    pub(super) lens: ArtifactLens,
    pub(super) partial: bool,
    /// Identities of the non-entrypoint sources frozen into an Agent Skill
    /// request. They are rechecked before transport and when a result lands.
    pub(super) skill_package: Option<FrozenSkillPackage>,
    pub(super) supporting_sources_current: bool,
    pub(super) result: ReviewTransportResult,
}

/// The immutable Review inputs and user answer draft for Revision.
///
/// Input entities are a UI projection only; the provider boundary receives
/// the typed `RevisionAnswers` rebuilt from `answer_states` at Run time.
pub(super) struct WorkspaceRevisionContext {
    pub(super) document_id: DocumentId,
    pub(super) source_snapshot: AsyncSnapshot,
    pub(super) request: DocumentReviewRequest,
    pub(super) review_output: ReviewModelOutput,
    pub(super) skill_package: Option<FrozenSkillPackage>,
    pub(super) supporting_sources_current: bool,
    pub(super) applied: Option<RevisionApplyIdentity>,
    /// Copy/export resolves answer recovery without pretending the document
    /// source was applied or saved.
    pub(super) answers_exported: bool,
    pub(super) answer_states: Vec<RevisionAnswer>,
    pub(super) answer_inputs: Vec<Entity<InputState>>,
}

/// Identity of the exact approved preview. A boolean alone cannot bind a
/// later Save As to the decisions that were applied.
pub(super) struct RevisionApplyIdentity {
    preview: String,
    decisions: Vec<(ChangeId, bool)>,
}

/// UI-owned metadata around one validated Revision provider result.
pub(super) struct WorkspaceRevisionResult {
    pub(super) document_id: DocumentId,
    pub(super) source_snapshot: AsyncSnapshot,
    pub(super) result: RevisionTransportResult,
    pub(super) decisions: Vec<(ChangeId, bool)>,
    pub(super) preview: String,
}

/// UI ownership for a Review diagnostic. A failure cannot be mistaken for a
/// validated provider result.
pub(super) struct WorkspaceReviewDiagnostic {
    pub(super) document_id: DocumentId,
    pub(super) lens: ArtifactLens,
    pub(super) diagnostic: ReviewDiagnostic,
}

/// Immutable identity of one in-flight Review. Its diagnostic remains bound
/// to the originating document if focus moves before cancellation.
struct PendingReview {
    cancelled: Arc<AtomicBool>,
    document_id: DocumentId,
    lens: ArtifactLens,
}

/// Immutable identity of one in-flight Revision.
struct PendingRevision {
    cancelled: Arc<AtomicBool>,
    document_id: DocumentId,
    generation: u64,
}

/// Identity carried by the one Save As ticket allowed to continue an explicit
/// Revision Apply.
#[derive(Clone, Copy)]
pub(super) struct RevisionSaveAsApproval {
    pub(super) source_stamp: (u64, u64),
    revision_generation: u64,
    approval_epoch: u64,
}

#[derive(Serialize)]
struct WorkspaceRevisionRecoveryExport {
    source_sha256: String,
    source_revision: u64,
    source_generation: u64,
    artifact_lens_sha256: String,
    review_context_sha256: String,
    answers_sha256: String,
    answers: Vec<WorkspaceRevisionExportAnswer>,
}

#[derive(Serialize)]
struct WorkspaceRevisionExportAnswer {
    question_index: usize,
    state: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    answer: Option<String>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum OpenReviewPanel {
    Opened,
    AnswersRetained,
    AnswersBelongToOtherDocument,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum RevisionSaveOutcome {
    NotApplied,
    AnswersRetained,
    Cleared,
}

impl ReviewFlow {
    pub(super) fn is_reviewing(&self) -> bool {
        self.pending_review.is_some()
    }

    pub(super) fn is_revision_running(&self) -> bool {
        self.pending_revision.is_some()
    }

    pub(super) fn review_panel_is_open(&self) -> bool {
        self.review_panel_open
    }

    pub(super) fn review_result(&self) -> Option<&WorkspaceReviewResult> {
        match self.review_outcome.as_ref() {
            Some(ReviewOutcome::Result(result)) => Some(result),
            Some(ReviewOutcome::Diagnostic(_)) | None => None,
        }
    }

    pub(super) fn review_diagnostic(&self) -> Option<&WorkspaceReviewDiagnostic> {
        match self.review_outcome.as_ref() {
            Some(ReviewOutcome::Diagnostic(diagnostic)) => Some(diagnostic),
            Some(ReviewOutcome::Result(_)) | None => None,
        }
    }

    pub(super) fn review_target_document_id(&self) -> Option<DocumentId> {
        self.review_target.map(|state| state.document_id)
    }

    pub(super) fn review_target_for_document(
        &self,
        document_id: DocumentId,
    ) -> Option<mt_core::agent_artifacts::package::ReviewTarget> {
        self.review_target
            .filter(|state| state.document_id == document_id)
            .map(|state| state.target)
    }

    pub(super) fn review_result_belongs_to(&self, document_id: DocumentId) -> bool {
        self.review_result()
            .is_some_and(|result| result.document_id == document_id)
    }

    pub(super) fn review_diagnostic_belongs_to(&self, document_id: DocumentId) -> bool {
        self.review_diagnostic()
            .is_some_and(|diagnostic| diagnostic.document_id == document_id)
    }

    pub(super) fn pending_review_belongs_to(&self, document_id: DocumentId) -> bool {
        self.pending_review
            .as_ref()
            .is_some_and(|pending| pending.document_id == document_id)
    }

    pub(super) fn revision_context(&self) -> Option<&WorkspaceRevisionContext> {
        self.revision_context.as_ref()
    }

    pub(super) fn revision_result(&self) -> Option<&WorkspaceRevisionResult> {
        self.revision_result.as_ref()
    }

    pub(super) fn revision_diagnostic(&self) -> Option<&str> {
        self.revision_diagnostic.as_deref()
    }

    pub(super) fn has_revision_context(&self) -> bool {
        self.revision_context.is_some()
    }

    pub(super) fn has_revision_result(&self) -> bool {
        self.revision_result.is_some()
    }

    pub(super) fn revision_is_applied(&self, document_id: DocumentId) -> bool {
        self.revision_context
            .as_ref()
            .is_some_and(|context| context.document_id == document_id && context.applied.is_some())
    }

    pub(super) fn has_recovered_revision_record(&self, key: &RecoveryKey) -> bool {
        self.recovered_revision_records.contains_key(key)
    }

    pub(super) fn recovered_revision_record_for_document(
        &self,
        document_id: DocumentId,
        current_key: &RecoveryKey,
    ) -> Option<(RecoveryKey, RevisionRecovery)> {
        let (key, recovery) =
            self.recovered_revision_record_ref_for_document(document_id, current_key)?;
        Some((key.clone(), recovery.clone()))
    }

    pub(super) fn recovered_revision_record_ref_for_document(
        &self,
        document_id: DocumentId,
        current_key: &RecoveryKey,
    ) -> Option<(&RecoveryKey, &RevisionRecovery)> {
        let owned_key = self.recovered_revision_documents.get(&document_id)?;
        // A path-derived key may collide with another tab's recovered record.
        // Only the explicit DocumentId binding grants access; when Save As has
        // rotated the source key, the same binding still selects its old key.
        let key = if owned_key == current_key {
            current_key
        } else {
            owned_key
        };
        self.recovered_revision_records.get_key_value(key)
    }

    pub(super) fn recovered_revision_answers_for_active_document(
        &self,
        document_id: DocumentId,
        current_key: &RecoveryKey,
    ) -> Option<(RecoveryKey, RevisionRecovery)> {
        self.recovered_revision_record_for_document(document_id, current_key)
    }

    pub(super) fn store_recovered_revision_record(
        &mut self,
        document_id: DocumentId,
        key: RecoveryKey,
        recovery: RevisionRecovery,
    ) {
        self.recovered_revision_records
            .insert(key.clone(), recovery);
        self.recovered_revision_documents.insert(document_id, key);
    }

    pub(super) fn remove_recovered_revision_document(&mut self, document_id: DocumentId) {
        self.recovered_revision_documents.remove(&document_id);
    }

    pub(super) fn move_recovered_revision_document(
        &mut self,
        from_document_id: DocumentId,
        to_document_id: DocumentId,
        key: &RecoveryKey,
    ) {
        if self.recovered_revision_documents.get(&from_document_id) == Some(key) {
            self.remove_recovered_revision_document(from_document_id);
            self.recovered_revision_documents
                .insert(to_document_id, key.clone());
        }
    }

    pub(super) fn remove_recovered_revision_record(&mut self, key: &RecoveryKey) {
        self.recovered_revision_records.remove(key);
        self.recovered_revision_documents
            .retain(|_, candidate| candidate != key);
    }

    pub(super) fn stage_revision_recovery_retirement(
        &mut self,
        key: RecoveryKey,
        document_id: DocumentId,
        recovery: RevisionRecovery,
    ) {
        self.pending_revision_recoveries
            .insert(key, (document_id, recovery));
    }

    pub(super) fn take_pending_revision_recovery(
        &mut self,
        key: &RecoveryKey,
    ) -> Option<(DocumentId, RevisionRecovery)> {
        self.pending_revision_recoveries.remove(key)
    }

    pub(super) fn restore_revision_recovery(
        &mut self,
        key: RecoveryKey,
        document_id: DocumentId,
        recovery: RevisionRecovery,
    ) {
        self.store_recovered_revision_record(document_id, key, recovery);
    }

    pub(super) fn set_review_lens_override(&mut self, document_id: DocumentId, lens: ArtifactLens) {
        self.review_lens_overrides.insert(document_id, lens);
    }

    pub(super) fn choose_review_lens(&mut self, document_id: DocumentId, lens: ArtifactLens) {
        let target = self.review_target_for_document(document_id).or_else(|| {
            self.review_result()
                .filter(|result| result.document_id == document_id)
                .map(|result| result.target)
        });
        self.set_review_lens_override(document_id, lens);
        self.review_target = target.map(|target| ReviewTargetState {
            target,
            document_id,
        });
        self.review_outcome = None;
    }

    pub(super) fn clear_review_outcome(&mut self) {
        self.review_outcome = None;
    }

    pub(super) fn remove_document_lens_override(&mut self, document_id: DocumentId) {
        self.review_lens_overrides.remove(&document_id);
    }

    pub(super) fn visible_review_lens(
        &self,
        document_id: DocumentId,
        inferred: ArtifactLens,
    ) -> ArtifactLens {
        self.review_lens_overrides
            .get(&document_id)
            .copied()
            .or_else(|| {
                self.review_result()
                    .filter(|result| result.document_id == document_id)
                    .map(|result| result.lens)
            })
            .or_else(|| {
                self.review_diagnostic()
                    .filter(|diagnostic| diagnostic.document_id == document_id)
                    .map(|diagnostic| diagnostic.lens)
            })
            .unwrap_or(inferred)
    }

    pub(super) fn open_review_panel(
        &mut self,
        target: mt_core::agent_artifacts::package::ReviewTarget,
        document_id: DocumentId,
        has_authored_answers: bool,
        has_answers_for_other_document: bool,
    ) -> OpenReviewPanel {
        if has_answers_for_other_document {
            return OpenReviewPanel::AnswersBelongToOtherDocument;
        }
        self.review_target = Some(ReviewTargetState {
            target,
            document_id,
        });
        self.review_panel_open = true;
        if has_authored_answers {
            OpenReviewPanel::AnswersRetained
        } else {
            self.review_outcome = None;
            OpenReviewPanel::Opened
        }
    }

    /// A fresh attempt supersedes the previous outcome even if request
    /// validation fails before prompting for consent.
    pub(super) fn begin_review_attempt(
        &mut self,
        target: mt_core::agent_artifacts::package::ReviewTarget,
        document_id: DocumentId,
    ) {
        self.review_outcome = None;
        self.review_target = Some(ReviewTargetState {
            target,
            document_id,
        });
        self.review_panel_open = true;
    }

    pub(super) fn begin_review_request(
        &mut self,
        document_id: DocumentId,
        lens: ArtifactLens,
    ) -> (u64, Arc<AtomicBool>) {
        let generation = self.review_generation.wrapping_add(1);
        self.review_generation = generation;
        let cancelled = Arc::new(AtomicBool::new(false));
        self.pending_review = Some(PendingReview {
            cancelled: cancelled.clone(),
            document_id,
            lens,
        });
        self.review_panel_open = true;
        (generation, cancelled)
    }

    pub(super) fn review_request_is_current(
        &self,
        generation: u64,
        cancelled: &Arc<AtomicBool>,
    ) -> bool {
        self.review_generation == generation
            && !cancelled.load(Ordering::Acquire)
            && self
                .pending_review
                .as_ref()
                .is_some_and(|pending| Arc::ptr_eq(&pending.cancelled, cancelled))
    }

    pub(super) fn finish_review_request(
        &mut self,
        generation: u64,
        cancelled: &Arc<AtomicBool>,
    ) -> bool {
        if !self.review_request_is_current(generation, cancelled) {
            return false;
        }
        self.pending_review = None;
        true
    }

    pub(super) fn cancel_review(&mut self) -> Option<(DocumentId, ArtifactLens)> {
        let pending = self.pending_review.take()?;
        pending.cancelled.store(true, Ordering::Release);
        self.review_generation = self.review_generation.wrapping_add(1);
        Some((pending.document_id, pending.lens))
    }

    pub(super) fn cancel_review_for_document(&mut self, document_id: DocumentId) -> bool {
        if !self.pending_review_belongs_to(document_id) {
            return false;
        }
        let Some((_, _)) = self.cancel_review() else {
            return false;
        };
        if self.review_target_document_id() == Some(document_id) {
            self.review_target = None;
        }
        self.review_panel_open = false;
        true
    }

    pub(super) fn dismiss_review(&mut self, authored_answers_for_active_document: bool) -> bool {
        if authored_answers_for_active_document {
            self.review_panel_open = true;
            return false;
        }
        if let Some(pending) = self.pending_review.take() {
            pending.cancelled.store(true, Ordering::Release);
        }
        self.review_generation = self.review_generation.wrapping_add(1);
        self.review_panel_open = false;
        self.review_outcome = None;
        self.review_target = None;
        true
    }

    pub(super) fn set_review_diagnostic(
        &mut self,
        document_id: DocumentId,
        lens: ArtifactLens,
        diagnostic: ReviewDiagnostic,
    ) -> String {
        let status = diagnostic.message.as_str().to_owned();
        self.pending_review = None;
        self.review_outcome = Some(ReviewOutcome::Diagnostic(WorkspaceReviewDiagnostic {
            document_id,
            lens,
            diagnostic,
        }));
        self.review_panel_open = true;
        status
    }

    pub(super) fn install_review_result(&mut self, mut result: WorkspaceReviewResult, stale: bool) {
        if stale {
            result.result.result.status = ReviewStatus::Stale;
        }
        self.review_outcome = Some(ReviewOutcome::Result(result));
        self.review_target = None;
        self.review_panel_open = true;
    }

    pub(super) fn clear_review_target(&mut self) {
        self.review_target = None;
    }

    /// Apply source and supporting-file invalidation as one flow transition.
    pub(super) fn observe_document_change(
        &mut self,
        document_id: DocumentId,
        source_snapshot: &AsyncSnapshot,
        source_path: Option<&Path>,
        is_dirty: bool,
    ) -> bool {
        let mut changed = false;
        if let Some(ReviewOutcome::Result(review)) = &mut self.review_outcome {
            if review.document_id == document_id && review.source_snapshot != *source_snapshot {
                review.result.result.status = ReviewStatus::Stale;
                changed = true;
            }
            if is_dirty
                && source_path.is_some_and(|path| {
                    review
                        .skill_package
                        .as_ref()
                        .is_some_and(|package| package.contains_supporting_path(path))
                })
            {
                review.supporting_sources_current = false;
                review.result.result.status = ReviewStatus::Stale;
                changed = true;
            }
        }
        if is_dirty
            && source_path.is_some_and(|path| {
                self.revision_context
                    .as_ref()
                    .and_then(|context| context.skill_package.as_ref())
                    .is_some_and(|package| package.contains_supporting_path(path))
            })
            && let Some(context) = &mut self.revision_context
        {
            context.supporting_sources_current = false;
            changed = true;
        }
        changed
    }

    /// A watcher event can invalidate the frozen Agent Skill inventory even
    /// when no open editor buffer emits a document event.
    pub(super) fn observe_supporting_source_changes(&mut self, changes: &[Change]) -> bool {
        let review_changed = self.review_result().is_some_and(|review| {
            review.skill_package.as_ref().is_some_and(|package| {
                changes
                    .iter()
                    .any(|change| package.path_affects_root(change.path()))
            })
        });
        let revision_changed = self
            .revision_context
            .as_ref()
            .and_then(|context| context.skill_package.as_ref())
            .is_some_and(|package| {
                changes
                    .iter()
                    .any(|change| package.path_affects_root(change.path()))
            });
        if review_changed && let Some(ReviewOutcome::Result(review)) = &mut self.review_outcome {
            review.supporting_sources_current = false;
            review.result.result.status = ReviewStatus::Stale;
        }
        if revision_changed && let Some(context) = &mut self.revision_context {
            context.supporting_sources_current = false;
        }
        review_changed || revision_changed
    }

    pub(super) fn mark_review_supporting_sources_stale(&mut self) {
        if let Some(ReviewOutcome::Result(review)) = &mut self.review_outcome {
            review.supporting_sources_current = false;
            review.result.result.status = ReviewStatus::Stale;
        }
    }

    pub(super) fn take_bound_revision_answers(
        &mut self,
        document_id: DocumentId,
        recovery_key: &RecoveryKey,
        request: &DocumentReviewRequest,
        review_output: &ReviewModelOutput,
    ) -> Vec<RevisionAnswer> {
        let mut answer_states = (0..review_output.clarification_questions.len())
            .map(|_| RevisionAnswer::unanswered())
            .collect::<Vec<_>>();
        let recovered_key = self
            .recovered_revision_record_for_document(document_id, recovery_key)
            .map(|(key, _)| key);
        if let Some(recovered_key) = recovered_key
            && let Some(recovery) = self.recovered_revision_records.remove(&recovered_key)
        {
            let source_digest = Sha256::digest(request.outbound_bytes());
            let binding = recovery.binding();
            let source_matches = binding.source_sha256().as_slice() == source_digest.as_slice();
            let binding_matches = source_matches
                && recovery.answers().len() == answer_states.len()
                && build_revision_recovery(request, review_output, recovery.answers().as_slice())
                    .is_some_and(|expected| {
                        let expected = expected.binding();
                        expected.source_sha256() == binding.source_sha256()
                            && expected.artifact_lens_digest() == binding.artifact_lens_digest()
                            && expected.review_context_digest() == binding.review_context_digest()
                            && expected.answers_digest() == binding.answers_digest()
                    });
            if binding_matches {
                answer_states = recovery.answers().as_slice().to_vec();
                if self
                    .recovered_revision_documents
                    .get(&document_id)
                    .is_some_and(|key| *key == recovered_key)
                {
                    self.recovered_revision_documents.remove(&document_id);
                }
            } else {
                // A mismatched record remains quarantined instead of binding
                // its answers to a newer source or Review context.
                self.recovered_revision_records
                    .insert(recovered_key.clone(), recovery);
                self.recovered_revision_documents
                    .insert(document_id, recovered_key);
            }
        }
        answer_states
    }

    pub(super) fn install_revision_context(&mut self, context: WorkspaceRevisionContext) {
        self.cancel_pending_revision();
        // A new Review context cannot inherit an older Save As approval, even
        // when it belongs to the same document and text.
        self.revision_generation = self.revision_generation.wrapping_add(1);
        self.revision_answer_subscriptions.clear();
        self.revision_context = Some(context);
        self.revision_result = None;
        self.revision_diagnostic = None;
    }

    pub(super) fn subscribe_revision_answer(&mut self, subscription: Subscription) {
        self.revision_answer_subscriptions.push(subscription);
    }

    pub(super) fn update_revision_answer_text(&mut self, index: usize, value: String) -> bool {
        if self.is_revision_running() || self.revision_result.is_some() {
            return false;
        }
        let Some(context) = &mut self.revision_context else {
            return false;
        };
        if !matches!(
            context.answer_states.get(index),
            Some(RevisionAnswer::Answered(_))
        ) {
            return false;
        }
        context.answer_states[index] = RevisionAnswer::answered(value);
        clear_revision_applied_state(context);
        context.answers_exported = false;
        self.revision_result = None;
        self.revision_diagnostic = None;
        true
    }

    pub(super) fn set_revision_answer_state(
        &mut self,
        index: usize,
        state: RevisionAnswer,
    ) -> Option<Option<Entity<InputState>>> {
        if self.is_revision_running() || self.revision_result.is_some() {
            return None;
        }
        let context = self.revision_context.as_mut()?;
        if index >= context.answer_states.len() {
            return None;
        }
        let should_focus = matches!(state, RevisionAnswer::Answered(_));
        context.answer_states[index] = state;
        clear_revision_applied_state(context);
        context.answers_exported = false;
        let input = should_focus
            .then(|| context.answer_inputs.get(index).cloned())
            .flatten();
        self.revision_result = None;
        self.revision_diagnostic = None;
        Some(input)
    }

    pub(super) fn revision_answers(&self) -> Result<RevisionAnswers, RevisionError> {
        let Some(context) = &self.revision_context else {
            return Err(RevisionError::InvalidRequest);
        };
        RevisionAnswers::new(context.answer_states.clone())
            .map_err(|_| RevisionError::InvalidAnswers)
    }

    pub(super) fn revision_recovery_for_document(
        &self,
        document_id: DocumentId,
        recovery_key: &RecoveryKey,
    ) -> Option<RevisionRecovery> {
        if let Some(context) = self.revision_context.as_ref().filter(|context| {
            context.document_id == document_id
                && !context.answers_exported
                && context
                    .answer_states
                    .iter()
                    .any(|answer| !matches!(answer, RevisionAnswer::Unanswered))
        }) {
            return build_revision_recovery(
                &context.request,
                &context.review_output,
                &context.answer_states,
            );
        }
        self.recovered_revision_record_for_document(document_id, recovery_key)
            .map(|(_, recovery)| recovery)
    }

    pub(super) fn revision_context_is_current(
        &self,
        active_document_id: Option<DocumentId>,
        active_snapshot: Option<&AsyncSnapshot>,
        skill_package_is_current: bool,
    ) -> bool {
        self.revision_context.as_ref().is_some_and(|context| {
            active_document_id == Some(context.document_id)
                && active_snapshot == Some(&context.source_snapshot)
                && context.supporting_sources_current
                && skill_package_is_current
        })
    }

    pub(super) fn revision_result_is_current(
        &self,
        revision: &WorkspaceRevisionResult,
        active_document_id: Option<DocumentId>,
        active_snapshot: Option<&AsyncSnapshot>,
        skill_package_is_current: bool,
    ) -> bool {
        let source_current = active_document_id == Some(revision.document_id)
            && active_snapshot == Some(&revision.source_snapshot);
        let package_current = self
            .revision_context
            .as_ref()
            .filter(|context| context.document_id == revision.document_id)
            .is_none_or(|context| context.supporting_sources_current && skill_package_is_current);
        source_current && package_current
    }

    pub(super) fn mark_revision_supporting_sources_stale(&mut self) {
        if let Some(context) = &mut self.revision_context {
            context.supporting_sources_current = false;
        }
    }

    #[cfg(test)]
    pub(super) fn set_revision_supporting_sources_current(&mut self, current: bool) {
        if let Some(context) = &mut self.revision_context {
            context.supporting_sources_current = current;
        }
    }

    pub(super) fn set_revision_diagnostic(&mut self, message: impl Into<String>) {
        self.revision_diagnostic = Some(message.into());
        self.pending_revision = None;
    }

    pub(super) fn begin_revision_request(
        &mut self,
        document_id: DocumentId,
    ) -> (u64, Arc<AtomicBool>) {
        let generation = self.revision_generation.wrapping_add(1);
        self.revision_generation = generation;
        let cancelled = Arc::new(AtomicBool::new(false));
        self.pending_revision = Some(PendingRevision {
            cancelled: cancelled.clone(),
            document_id,
            generation,
        });
        self.revision_result = None;
        self.revision_diagnostic = None;
        (generation, cancelled)
    }

    pub(super) fn revision_request_is_current(
        &self,
        generation: u64,
        cancelled: &Arc<AtomicBool>,
    ) -> bool {
        self.revision_generation == generation
            && !cancelled.load(Ordering::Acquire)
            && self.pending_revision.as_ref().is_some_and(|pending| {
                pending.generation == generation && Arc::ptr_eq(&pending.cancelled, cancelled)
            })
    }

    pub(super) fn finish_revision_request(&mut self, generation: u64) -> bool {
        if self.revision_generation != generation
            || self
                .pending_revision
                .as_ref()
                .is_none_or(|pending| pending.generation != generation)
        {
            return false;
        }
        self.pending_revision = None;
        true
    }

    pub(super) fn cancel_revision_for_document(&mut self, document_id: DocumentId) -> bool {
        if self
            .pending_revision
            .as_ref()
            .is_none_or(|pending| pending.document_id != document_id)
        {
            return false;
        }
        self.cancel_pending_revision();
        true
    }

    pub(super) fn cancel_revision(&mut self) -> bool {
        if self.pending_revision.is_none() {
            return false;
        }
        self.cancel_pending_revision();
        true
    }

    fn cancel_pending_revision(&mut self) {
        if let Some(pending) = self.pending_revision.take() {
            pending.cancelled.store(true, Ordering::Release);
            self.revision_generation = self.revision_generation.wrapping_add(1);
        }
    }

    pub(super) fn dismiss_revision(&mut self) {
        if let Some(pending) = self.pending_revision.take() {
            pending.cancelled.store(true, Ordering::Release);
        }
        self.revision_generation = self.revision_generation.wrapping_add(1);
        self.revision_result = None;
        self.revision_diagnostic = None;
        // Deliberately retain answer context until an explicit discard.
    }

    pub(super) fn revision_has_authored_answers(&self) -> bool {
        self.revision_context_has_authored_answers()
            || self.recovered_revision_records.values().any(|recovery| {
                recovery
                    .answers()
                    .as_slice()
                    .iter()
                    .any(|answer| !matches!(answer, RevisionAnswer::Unanswered))
            })
    }

    pub(super) fn revision_context_has_authored_answers(&self) -> bool {
        self.revision_context.as_ref().is_some_and(|context| {
            context
                .answer_states
                .iter()
                .any(|answer| !matches!(answer, RevisionAnswer::Unanswered))
                && !context.answers_exported
        })
    }

    pub(super) fn revision_context_has_authored_answers_for_document(
        &self,
        document_id: DocumentId,
    ) -> bool {
        self.revision_context.as_ref().is_some_and(|context| {
            context.document_id == document_id
                && context
                    .answer_states
                    .iter()
                    .any(|answer| !matches!(answer, RevisionAnswer::Unanswered))
                && !context.answers_exported
        })
    }

    pub(super) fn revision_context_has_authored_answers_for_other_document(
        &self,
        document_id: DocumentId,
    ) -> bool {
        self.revision_context.as_ref().is_some_and(|context| {
            context.document_id != document_id
                && context
                    .answer_states
                    .iter()
                    .any(|answer| !matches!(answer, RevisionAnswer::Unanswered))
                && !context.answers_exported
        })
    }

    pub(super) fn revision_has_authored_answers_for_document(
        &self,
        document_id: DocumentId,
        recovery_key: Option<&RecoveryKey>,
    ) -> bool {
        if self.revision_context_has_authored_answers_for_document(document_id) {
            return true;
        }
        recovery_key
            .and_then(|key| self.recovered_revision_record_for_document(document_id, key))
            .is_some_and(|(_, recovery)| {
                recovery
                    .answers()
                    .as_slice()
                    .iter()
                    .any(|answer| !matches!(answer, RevisionAnswer::Unanswered))
            })
    }

    pub(super) fn revision_answers_incorporated(&self) -> bool {
        let Some(context) = &self.revision_context else {
            return false;
        };
        let Some(revision) = self
            .revision_result
            .as_ref()
            .filter(|revision| revision.document_id == context.document_id)
        else {
            return false;
        };
        context
            .answer_states
            .iter()
            .enumerate()
            .all(|(index, answer)| {
                let status = revision
                    .result
                    .question_coverage()
                    .iter()
                    .find(|coverage| coverage.question_index() == index)
                    .map(|coverage| coverage.status());
                revision_answer_is_incorporated(answer, status, &revision.decisions)
            })
    }

    pub(super) fn clear_revision_answers(&mut self) {
        if let Some(pending) = self.pending_revision.take() {
            pending.cancelled.store(true, Ordering::Release);
        }
        self.revision_generation = self.revision_generation.wrapping_add(1);
        self.revision_result = None;
        self.revision_context = None;
        self.revision_answer_subscriptions.clear();
        self.revision_diagnostic = None;
    }

    pub(super) fn clear_revision_after_save(
        &mut self,
        document_id: DocumentId,
    ) -> RevisionSaveOutcome {
        let applied = self
            .revision_context
            .as_ref()
            .is_some_and(|context| context.document_id == document_id && context.applied.is_some());
        if !applied {
            return RevisionSaveOutcome::NotApplied;
        }
        if !self.revision_answers_incorporated() {
            return RevisionSaveOutcome::AnswersRetained;
        }
        self.revision_result = None;
        self.revision_context = None;
        self.revision_answer_subscriptions.clear();
        self.revision_diagnostic = None;
        RevisionSaveOutcome::Cleared
    }

    pub(super) fn refresh_revision_applied_state(
        &mut self,
        document_id: DocumentId,
        document_text_matches_preview: bool,
    ) -> bool {
        let Some(context) = self
            .revision_context
            .as_ref()
            .filter(|context| context.applied.is_some())
        else {
            return false;
        };
        let context_document_id = context.document_id;
        let still_applied = self.revision_result.as_ref().is_some_and(|revision| {
            self.applied_revision_identity_matches(context, revision)
                && revision.document_id == context_document_id
                && document_id == context_document_id
                && document_text_matches_preview
        });
        if !still_applied
            && let Some(context) = &mut self.revision_context
            && context.document_id == context_document_id
        {
            clear_revision_applied_state(context);
            return true;
        }
        false
    }

    pub(super) fn revision_applied_state_is_current_for_render(
        &self,
        active_document_id: Option<DocumentId>,
    ) -> bool {
        let Some(context) = self
            .revision_context
            .as_ref()
            .filter(|context| context.applied.is_some() && context.supporting_sources_current)
        else {
            return false;
        };
        let Some(revision) = self
            .revision_result
            .as_ref()
            .filter(|revision| revision.document_id == context.document_id)
        else {
            return false;
        };
        active_document_id == Some(context.document_id)
            && self.applied_revision_identity_matches(context, revision)
    }

    pub(super) fn revision_applied_state_is_current_for_document(
        &mut self,
        document_id: DocumentId,
        document_text_matches_preview: bool,
        skill_package_is_current: bool,
    ) -> bool {
        let Some(context) = self
            .revision_context
            .as_ref()
            .filter(|context| context.applied.is_some() && context.document_id == document_id)
        else {
            return false;
        };
        let Some(revision) = self
            .revision_result
            .as_ref()
            .filter(|revision| revision.document_id == context.document_id)
        else {
            return false;
        };
        if !document_text_matches_preview
            || !self.applied_revision_identity_matches(context, revision)
        {
            return false;
        }
        let package_current = context.supporting_sources_current && skill_package_is_current;
        if !package_current {
            self.mark_revision_supporting_sources_stale();
        }
        package_current
    }

    fn applied_revision_identity_matches(
        &self,
        context: &WorkspaceRevisionContext,
        revision: &WorkspaceRevisionResult,
    ) -> bool {
        context.applied.as_ref().is_some_and(|applied| {
            revision_apply_identity_matches(
                Some(applied.preview.as_str()),
                Some(applied.decisions.as_slice()),
                &revision.preview,
                &revision.decisions,
            )
        })
    }

    pub(super) fn set_revision_decision(&mut self, change_id: ChangeId, accepted: bool) -> bool {
        let changed = {
            let Some(revision) = &mut self.revision_result else {
                return false;
            };
            if let Some((_, decision)) = revision
                .decisions
                .iter_mut()
                .find(|(id, _)| *id == change_id)
            {
                if *decision == accepted {
                    false
                } else {
                    *decision = accepted;
                    true
                }
            } else {
                revision.decisions.push((change_id, accepted));
                true
            }
        };
        if !changed {
            return false;
        }
        if let Some(revision) = &mut self.revision_result
            && let Ok(preview) = revision.result.proposal().compose(&revision.decisions)
        {
            revision.preview = preview;
        }
        if let Some(context) = &mut self.revision_context {
            clear_revision_applied_state(context);
            context.answers_exported = false;
        }
        true
    }

    pub(super) fn set_all_revision_decisions(&mut self, accepted: bool) -> bool {
        let changed = {
            let Some(revision) = &mut self.revision_result else {
                return false;
            };
            let decisions = revision_change_ids(revision.result.proposal())
                .into_iter()
                .map(|change_id| (change_id, accepted))
                .collect::<Vec<_>>();
            if revision.decisions == decisions {
                false
            } else {
                revision.decisions = decisions;
                true
            }
        };
        if !changed {
            return false;
        }
        if let Some(revision) = &mut self.revision_result
            && let Ok(preview) = revision.result.proposal().compose(&revision.decisions)
        {
            revision.preview = preview;
        }
        if let Some(context) = &mut self.revision_context {
            clear_revision_applied_state(context);
            context.answers_exported = false;
        }
        true
    }

    pub(super) fn revision_preview(&self) -> Option<&str> {
        self.revision_result
            .as_ref()
            .map(|revision| revision.preview.as_str())
    }

    pub(super) fn install_revision_result(
        &mut self,
        document_id: DocumentId,
        source_snapshot: AsyncSnapshot,
        result: RevisionTransportResult,
        supporting_sources_current: bool,
        stale_diagnostic: Option<String>,
    ) {
        if let Some(context) = &mut self.revision_context {
            context.supporting_sources_current = supporting_sources_current;
        }
        let decisions = revision_change_ids(result.proposal())
            .into_iter()
            .map(|change_id| (change_id, false))
            .collect::<Vec<_>>();
        let preview = result
            .proposal()
            .compose(&decisions)
            .expect("a validated reject-all preview must compose");
        self.revision_result = Some(WorkspaceRevisionResult {
            document_id,
            source_snapshot,
            result,
            decisions,
            preview,
        });
        self.revision_diagnostic = stale_diagnostic;
    }

    pub(super) fn record_revision_apply(&mut self, preview: &str, decisions: &[(ChangeId, bool)]) {
        if let Some(context) = &mut self.revision_context {
            context.applied = Some(RevisionApplyIdentity {
                preview: preview.to_owned(),
                decisions: decisions.to_vec(),
            });
        }
        self.revision_approval_epoch = self.revision_approval_epoch.wrapping_add(1);
    }

    pub(super) fn clear_revision_diagnostic(&mut self) {
        self.revision_diagnostic = None;
    }

    pub(super) fn clear_revision_applied_state(&mut self) {
        if let Some(context) = &mut self.revision_context {
            clear_revision_applied_state(context);
        }
    }

    pub(super) fn record_revision_apply_failure(&mut self) {
        self.clear_revision_applied_state();
    }

    pub(super) fn mark_revision_answers_exported_if_incorporated(&mut self) {
        let incorporated = self.revision_answers_incorporated();
        if let Some(context) = &mut self.revision_context {
            context.answers_exported = incorporated;
        }
    }

    pub(super) fn revision_save_as_approval(
        &self,
        source_stamp: (u64, u64),
    ) -> RevisionSaveAsApproval {
        RevisionSaveAsApproval {
            source_stamp,
            revision_generation: self.revision_generation,
            approval_epoch: self.revision_approval_epoch,
        }
    }

    pub(super) fn revision_save_as_approval_is_current(
        &self,
        document_id: DocumentId,
        approval: RevisionSaveAsApproval,
    ) -> bool {
        approval.revision_generation == self.revision_generation
            && approval.approval_epoch == self.revision_approval_epoch
            && self
                .revision_context
                .as_ref()
                .is_some_and(|context| context.document_id == document_id)
    }

    pub(super) fn reject_stale_revision_save_as(
        &mut self,
        document_id: DocumentId,
        approval: RevisionSaveAsApproval,
    ) -> bool {
        if !self.revision_save_as_approval_is_current(document_id, approval) {
            return false;
        }
        self.clear_revision_applied_state();
        true
    }

    #[cfg(test)]
    pub(super) fn replace_revision_result(&mut self, result: WorkspaceRevisionResult) {
        self.revision_result = Some(result);
    }

    #[cfg(test)]
    pub(super) fn set_review_panel_open(&mut self, open: bool) {
        self.review_panel_open = open;
    }
}

impl Workspace {
    pub(super) fn on_review_document(
        &mut self,
        _: &ReviewDocument,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.review_flow.is_reviewing() {
            return;
        }
        self.open_review_panel(ReviewTarget::Document, cx);
    }

    pub(super) fn on_review_selection(
        &mut self,
        _: &ReviewSelection,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.review_flow.is_reviewing() {
            return;
        }
        self.open_review_panel(ReviewTarget::Selection, cx);
    }

    pub(super) fn on_cancel_review(
        &mut self,
        _: &CancelReview,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some((document_id, lens)) = self.review_flow.cancel_review() else {
            return;
        };
        self.set_review_diagnostic(
            document_id,
            lens,
            ReviewDiagnostic::new(
                ReviewDiagnosticCode::Cancelled,
                i18n::t(i18n::Key::ReviewCancelled, cx).to_string(),
            ),
            cx,
        );
        self.set_status(i18n::t(i18n::Key::ReviewCancelled, cx).into(), cx);
    }

    pub(super) fn cancel_pending_review_for_document(
        &mut self,
        document_id: DocumentId,
        cx: &mut Context<Self>,
    ) {
        if !self.review_flow.cancel_review_for_document(document_id) {
            return;
        }
        self.set_status(i18n::t(i18n::Key::ReviewDocumentClosed, cx).into(), cx);
        cx.notify();
    }

    pub(super) fn revision_has_authored_answers_for_document(
        &self,
        document_id: DocumentId,
        cx: &App,
    ) -> bool {
        let Some(document) = self.document_by_id(document_id, cx) else {
            return self
                .review_flow
                .revision_has_authored_answers_for_document(document_id, None);
        };
        let document = document.read(cx);
        self.review_flow.revision_has_authored_answers_for_document(
            document.id(),
            Some(&document.recovery_key()),
        )
    }

    fn revision_context_is_current(&self, cx: &Context<Self>) -> bool {
        let Some(context) = self.review_flow.revision_context() else {
            return false;
        };
        let active_document = self.active_document().map(|document| {
            let document = document.read(cx);
            (document.id(), document.async_snapshot(cx))
        });
        let skill_package_is_current = context.skill_package.as_ref().is_none_or(|package| {
            !self.has_dirty_skill_supporting_document(package, cx) && package.revalidate().is_ok()
        });
        self.review_flow.revision_context_is_current(
            active_document
                .as_ref()
                .map(|(document_id, _)| *document_id),
            active_document
                .as_ref()
                .map(|(_, source_snapshot)| source_snapshot),
            skill_package_is_current,
        )
    }

    fn revision_result_is_current(
        &self,
        revision: &WorkspaceRevisionResult,
        cx: &Context<Self>,
    ) -> bool {
        let active_document = self.active_document().map(|document| {
            let document = document.read(cx);
            (document.id(), document.async_snapshot(cx))
        });
        let skill_package_is_current = self
            .review_flow
            .revision_context()
            .filter(|context| context.document_id == revision.document_id)
            .and_then(|context| context.skill_package.as_ref())
            .is_none_or(|package| !self.has_dirty_skill_supporting_document(package, cx));
        self.review_flow.revision_result_is_current(
            revision,
            active_document
                .as_ref()
                .map(|(document_id, _)| *document_id),
            active_document
                .as_ref()
                .map(|(_, source_snapshot)| source_snapshot),
            skill_package_is_current,
        )
    }

    fn revision_result_is_current_for_commit(&mut self, cx: &Context<Self>) -> bool {
        let Some(revision) = self.review_flow.revision_result() else {
            return false;
        };
        if !self.revision_result_is_current(revision, cx) {
            return false;
        }
        let package_current = self
            .review_flow
            .revision_context()
            .and_then(|context| context.skill_package.as_ref())
            .is_none_or(|package| package.revalidate().is_ok());
        if !package_current {
            self.review_flow.mark_revision_supporting_sources_stale();
        }
        package_current
    }

    pub(super) fn set_revision_diagnostic(
        &mut self,
        message: impl Into<String>,
        cx: &mut Context<Self>,
    ) {
        self.review_flow.set_revision_diagnostic(message);
        cx.notify();
    }

    pub(super) fn on_revision_provider_failure(
        &mut self,
        error: RevisionError,
        cx: &mut Context<Self>,
    ) {
        self.set_revision_diagnostic(
            format!(
                "{}: {error}",
                i18n::t(i18n::Key::RevisionProviderFailed, cx)
            ),
            cx,
        );
    }

    pub(super) fn on_cancel_revision(&mut self, cx: &mut Context<Self>) {
        if !self.review_flow.cancel_revision() {
            return;
        }
        self.set_revision_diagnostic(i18n::t(i18n::Key::RevisionCancelledAnswersRetained, cx), cx);
    }

    pub(super) fn dismiss_revision(&mut self, cx: &mut Context<Self>) {
        self.review_flow.dismiss_revision();
        cx.notify();
    }

    pub(super) fn refresh_revision_applied_state(
        &mut self,
        document: &Entity<super::DocumentView>,
        cx: &mut Context<Self>,
    ) {
        let edited_document_id = document.read(cx).id();
        let Some(reviewed_document_id) = self
            .review_flow
            .revision_context()
            .map(|context| context.document_id)
            .filter(|document_id| self.review_flow.revision_is_applied(*document_id))
        else {
            return;
        };
        if reviewed_document_id != edited_document_id {
            return;
        }
        let text_matches_preview = self
            .review_flow
            .revision_result()
            .filter(|revision| revision.document_id == edited_document_id)
            .is_some_and(|revision| {
                let document = document.read(cx);
                document.id() == edited_document_id && document.text_matches(&revision.preview, cx)
            });
        if self
            .review_flow
            .refresh_revision_applied_state(edited_document_id, text_matches_preview)
        {
            cx.notify();
        }
    }

    fn revision_applied_state_is_current_for_render(&self, cx: &Context<Self>) -> bool {
        self.review_flow
            .revision_applied_state_is_current_for_render(
                self.active_document()
                    .map(|document| document.read(cx).id()),
            )
    }

    /// Interactive Revision actions are scoped to the currently active tab.
    pub(super) fn revision_applied_state_is_current_for_commit(
        &mut self,
        cx: &Context<Self>,
    ) -> bool {
        let Some(document_id) = self
            .active_document()
            .map(|document| document.read(cx).id())
        else {
            return false;
        };
        self.revision_applied_state_is_current_for_document(document_id, cx)
    }

    /// Async Save As continuations stay bound to their captured tab if focus moves.
    pub(super) fn revision_applied_state_is_current_for_document(
        &mut self,
        document_id: DocumentId,
        cx: &Context<Self>,
    ) -> bool {
        if !self.review_flow.revision_is_applied(document_id) {
            return false;
        }
        let Some(context) = self
            .review_flow
            .revision_context()
            .filter(|context| context.document_id == document_id)
        else {
            return false;
        };
        let Some(revision) = self
            .review_flow
            .revision_result()
            .filter(|revision| revision.document_id == context.document_id)
        else {
            return false;
        };
        let Some(document) = self.document_by_id(document_id, cx) else {
            return false;
        };
        let document_text_matches_preview = document.read(cx).id() == context.document_id
            && document.read(cx).text_matches(&revision.preview, cx);
        let skill_package_is_current = context.skill_package.as_ref().is_none_or(|package| {
            !self.has_dirty_skill_supporting_document(package, cx) && package.revalidate().is_ok()
        });
        self.review_flow
            .revision_applied_state_is_current_for_document(
                document_id,
                document_text_matches_preview,
                skill_package_is_current,
            )
    }

    pub(super) fn cancel_pending_revision_for_document(
        &mut self,
        document_id: DocumentId,
        cx: &mut Context<Self>,
    ) {
        if self.review_flow.cancel_revision_for_document(document_id) {
            cx.notify();
        }
    }

    pub(super) fn cancel_pending_revision(&mut self, cx: &mut Context<Self>) {
        if self.review_flow.cancel_revision() {
            cx.notify();
        }
    }

    /// Make the inferred lens observable and correctable before any outbound
    /// consent is requested. The actual request begins only from the Review
    /// panel's explicit Run command.
    pub(super) fn open_review_panel(&mut self, target: ReviewTarget, cx: &mut Context<Self>) {
        let Some(document) = self.active_document() else {
            return;
        };
        let document_id = document.read(cx).id();
        let has_answers_for_other_document = self
            .review_flow
            .revision_context_has_authored_answers_for_other_document(document_id);
        let has_authored_answers = self.revision_has_authored_answers_for_document(document_id, cx);
        match self.review_flow.open_review_panel(
            target,
            document_id,
            has_authored_answers,
            has_answers_for_other_document,
        ) {
            OpenReviewPanel::AnswersBelongToOtherDocument => {
                self.set_status(i18n::t(i18n::Key::RevisionAnswersRetained, cx).into(), cx);
                return;
            }
            OpenReviewPanel::AnswersRetained => {
                self.set_status(i18n::t(i18n::Key::RevisionAnswersRetained, cx).into(), cx);
            }
            OpenReviewPanel::Opened => {}
        }
        self.right_panel_open = true;
        cx.notify();
    }

    /// Forget a Review panel and invalidate every pending completion before it
    /// can make the panel visible again.
    pub(super) fn dismiss_review(&mut self, cx: &mut Context<Self>) {
        let active_document_id = self
            .active_document()
            .map(|document| document.read(cx).id());
        let authored_for_active_document = active_document_id.is_some_and(|document_id| {
            self.revision_has_authored_answers_for_document(document_id, cx)
        });
        if !self
            .review_flow
            .dismiss_review(authored_for_active_document)
        {
            self.set_status(i18n::t(i18n::Key::RevisionAnswersRetained, cx).into(), cx);
            self.right_panel_open = true;
            cx.notify();
            return;
        }
        cx.notify();
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn install_revision_context(
        &mut self,
        document_id: DocumentId,
        recovery_key: RecoveryKey,
        source_snapshot: AsyncSnapshot,
        request: DocumentReviewRequest,
        review_output: ReviewModelOutput,
        skill_package: Option<FrozenSkillPackage>,
        supporting_sources_current: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let mut answer_inputs = Vec::with_capacity(review_output.clarification_questions.len());
        for _ in 0..review_output.clarification_questions.len() {
            answer_inputs.push(cx.new(|cx| InputState::new(window, cx)));
        }
        let answer_states = self.review_flow.take_bound_revision_answers(
            document_id,
            &recovery_key,
            &request,
            &review_output,
        );

        // Restore the visible editor value before subscribing to Change. The
        // typed answer state is the recovery authority, while InputState is
        // only its editable UI projection.
        for (input, state) in answer_inputs.iter().zip(answer_states.iter()) {
            if let RevisionAnswer::Answered(value) = state {
                input.update(cx, |input, cx| {
                    input.set_value(value.clone(), window, cx);
                });
            }
        }
        let mut answer_subscriptions = Vec::with_capacity(answer_inputs.len());
        for (index, input) in answer_inputs.iter().enumerate() {
            let subscription = cx.subscribe_in(
                input,
                window,
                move |this: &mut Self, _, event: &InputEvent, _, cx| {
                    if matches!(event, InputEvent::Change) {
                        this.on_revision_answer_input(index, cx);
                    }
                },
            );
            answer_subscriptions.push(subscription);
        }
        self.review_flow
            .install_revision_context(WorkspaceRevisionContext {
                document_id,
                source_snapshot,
                request,
                review_output,
                skill_package,
                supporting_sources_current,
                applied: None,
                answers_exported: false,
                answer_states,
                answer_inputs,
            });
        for subscription in answer_subscriptions {
            self.review_flow.subscribe_revision_answer(subscription);
        }
        cx.notify();
    }

    fn on_revision_answer_input(&mut self, index: usize, cx: &mut Context<Self>) {
        let value = self
            .review_flow
            .revision_context()
            .and_then(|context| context.answer_inputs.get(index))
            .map(|input| input.read(cx).value().to_string());
        let Some(value) = value else {
            return;
        };
        if self.review_flow.update_revision_answer_text(index, value) {
            if let Some(document) = self.active_document().cloned() {
                self.arm_document_recovery(&document, cx);
            }
            cx.notify();
        }
    }

    pub(super) fn set_revision_answer_state(
        &mut self,
        index: usize,
        state: RevisionAnswer,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(input) = self.review_flow.set_revision_answer_state(index, state) else {
            return;
        };
        if let Some(input) = input {
            input.update(cx, |input, cx| input.focus(window, cx));
        }
        if let Some(document) = self.active_document().cloned() {
            self.arm_document_recovery(&document, cx);
        }
        cx.notify();
    }

    /// Make a Review over the given document's latest editor state stale, and
    /// invalidate frozen supporting sources when an open Skill buffer changes.
    pub(super) fn mark_review_stale_for_document(
        &mut self,
        document: &gpui_kit::Entity<super::DocumentView>,
        cx: &mut gpui_kit::Context<Self>,
    ) {
        let (document_id, source_path, source_snapshot, is_dirty) = {
            let document = document.read(cx);
            (
                document.id(),
                document.source_path().map(Path::to_path_buf),
                document.async_snapshot(cx),
                document.is_dirty(),
            )
        };
        if self.review_flow.observe_document_change(
            document_id,
            &source_snapshot,
            source_path.as_deref(),
            is_dirty,
        ) {
            cx.notify();
        }
    }
}

impl Workspace {
    pub(super) fn set_revision_decision(
        &mut self,
        change_id: ChangeId,
        accepted: bool,
        cx: &mut Context<Self>,
    ) {
        if self.review_flow.set_revision_decision(change_id, accepted) {
            cx.notify();
        }
    }

    pub(super) fn set_all_revision_decisions(&mut self, accepted: bool, cx: &mut Context<Self>) {
        if self.review_flow.set_all_revision_decisions(accepted) {
            cx.notify();
        }
    }

    fn revision_preview(&self) -> Option<String> {
        self.review_flow.revision_preview().map(str::to_owned)
    }

    pub(super) fn copy_revision(&mut self, cx: &mut Context<Self>) {
        let applied_preview_current = self.revision_applied_state_is_current_for_commit(cx);
        if !applied_preview_current && !self.revision_result_is_current_for_commit(cx) {
            self.set_revision_diagnostic(i18n::t(i18n::Key::RevisionStale, cx), cx);
            return;
        }
        let Some(text) = self.revision_preview() else {
            return;
        };
        cx.write_to_clipboard(ClipboardItem::new_string(text));
        // Copy exports only the approved artifact. Retire answer recovery only
        // when every authored answer is present in that exact output.
        self.review_flow
            .mark_revision_answers_exported_if_incorporated();
        self.checkpoint_recovery_at(cx.background_executor().now(), cx);
        self.set_status(i18n::t(i18n::Key::RevisionCopied, cx).into(), cx);
    }

    pub(super) fn start_revision(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.review_flow.is_revision_running() {
            return;
        }
        let Some(context) = self.review_flow.revision_context() else {
            return;
        };
        let document_id = context.document_id;
        let request = context.request.clone();
        let review_output = context.review_output.clone();
        let source_snapshot = context.source_snapshot.clone();
        let skill_package = context.skill_package.clone();
        let supporting_sources_current = context.supporting_sources_current;
        if !self.revision_context_is_current(cx) {
            self.set_revision_diagnostic(i18n::t(i18n::Key::RevisionSourceChanged, cx), cx);
            return;
        }
        let answers = match self.review_flow.revision_answers() {
            Ok(answers) => answers,
            Err(error) => {
                self.set_revision_diagnostic(error.to_string(), cx);
                return;
            }
        };
        let language = match crate::settings::AppSettings::global(cx).language {
            mt_core::settings::Language::English => {
                mt_core::review::provider::ReviewLanguage::English
            }
            mt_core::settings::Language::Chinese => {
                mt_core::review::provider::ReviewLanguage::SimplifiedChinese
            }
        };
        let settings = crate::settings::AppSettings::global(cx).clone();
        let vault = crate::credentials::AppCredentialVault::global(cx).clone();
        let prepared = match PreparedReview::from_settings(&settings, &vault) {
            Ok(prepared) => {
                match prepared.bind_revision_request(request, review_output, answers, language) {
                    Ok(prepared) => prepared,
                    Err(error) => {
                        self.set_revision_diagnostic(
                            format!(
                                "{}: {error}",
                                i18n::t(i18n::Key::RevisionPreparationFailed, cx)
                            ),
                            cx,
                        );
                        return;
                    }
                }
            }
            Err(error) => {
                self.set_revision_diagnostic(
                    format!(
                        "{}: {error}",
                        i18n::t(i18n::Key::RevisionPreparationFailed, cx)
                    ),
                    cx,
                );
                return;
            }
        };

        let prompt_description = i18n::model_request_disclosure(prepared.disclosure(), cx);
        let answer = window.prompt(
            PromptLevel::Warning,
            i18n::t(i18n::Key::RevisionConsentTitle, cx),
            Some(&prompt_description),
            &[
                PromptButton::ok(i18n::t(i18n::Key::SendToModel, cx)),
                PromptButton::cancel(i18n::t(i18n::Key::Cancel, cx)),
            ],
            cx,
        );
        let pending = Arc::new(Mutex::new(Some(prepared)));
        let (generation, cancelled) = self.review_flow.begin_revision_request(document_id);
        self.set_status(i18n::t(i18n::Key::RevisionWaitingForConsent, cx).into(), cx);

        let doc = self
            .active_document()
            .cloned()
            .map(|document| document.downgrade());
        let cancelled_for_request = cancelled.clone();
        cx.spawn_in(window, async move |this, cx| {
            let approved = answer.await.unwrap_or(1) == 0;
            let pending = pending.clone();
            loop {
                let pending = pending.clone();
                let cancelled = cancelled_for_request.clone();
                let doc = doc.clone();
                let source_snapshot = source_snapshot.clone();
                let skill_package = skill_package.clone();
                let supporting_sources_current = supporting_sources_current;
                if crate::views::try_update_in(&this, cx, move |this, window, cx| {
                    let Some(prepared) = pending
                        .lock()
                        .unwrap_or_else(|poisoned| poisoned.into_inner())
                        .take()
                    else {
                        return;
                    };
                    if !this
                        .review_flow
                        .revision_request_is_current(generation, &cancelled)
                    {
                        return;
                    }
                    if !approved {
                        this.set_revision_diagnostic(
                            i18n::t(i18n::Key::RevisionConsentCancelledAnswersRetained, cx),
                            cx,
                        );
                        return;
                    }
                    if !this.revision_context_is_current(cx) {
                        this.set_revision_diagnostic(
                            i18n::t(i18n::Key::RevisionScopeChangedDuringConsent, cx),
                            cx,
                        );
                        return;
                    }
                    let mut consent = ConsentCapability::from_decision(
                        prepared.disclosure(),
                        ConsentDecision::Approve,
                    );
                    let authorization = match prepared.authorize(&mut consent) {
                        Ok(authorization) => authorization,
                        Err(error) => {
                            this.set_revision_diagnostic(
                                format!(
                                    "{}: {error}",
                                    i18n::t(i18n::Key::RevisionAuthorizationFailed, cx)
                                ),
                                cx,
                            );
                            return;
                        }
                    };
                    this.set_status(i18n::t(i18n::Key::RevisionRunning, cx).into(), cx);
                    let cancel_for_request = cancelled.clone();
                    let doc = doc.clone();
                    let skill_package = skill_package.clone();
                    let supporting_sources_current = supporting_sources_current;
                    cx.spawn_in(window, async move |this, cx| {
                        let result = cx
                            .background_spawn(async move {
                                prepared.execute_with(
                                    authorization,
                                    &cancel_for_request,
                                    REVIEW_REQUEST_TIMEOUT,
                                )
                            })
                            .await;
                        let result = Arc::new(Mutex::new(Some(result)));
                        loop {
                            let result = result.clone();
                            let doc = doc.clone();
                            let source_snapshot = source_snapshot.clone();
                            let skill_package = skill_package.clone();
                            let supporting_sources_current = supporting_sources_current;
                            if crate::views::try_update_in(&this, cx, move |this, _, cx| {
                                let Some(result) = result
                                    .lock()
                                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                                    .take()
                                else {
                                    return;
                                };
                                if !this.review_flow.finish_revision_request(generation) {
                                    return;
                                }
                                match result {
                                    Ok(result) => {
                                        let Some(document) =
                                            doc.as_ref().and_then(gpui_kit::WeakEntity::upgrade)
                                        else {
                                            this.set_revision_diagnostic(
                                                i18n::t(i18n::Key::RevisionDocumentClosed, cx),
                                                cx,
                                            );
                                            return;
                                        };
                                        let supporting_sources_current = supporting_sources_current
                                            && skill_package.as_ref().is_none_or(|package| {
                                                !this.has_dirty_skill_supporting_document(
                                                    package, cx,
                                                ) && package.revalidate().is_ok()
                                            });
                                        let stale = document.read(cx).async_snapshot(cx)
                                            != source_snapshot
                                            || !supporting_sources_current;
                                        this.review_flow.install_revision_result(
                                            document_id,
                                            source_snapshot.clone(),
                                            result,
                                            supporting_sources_current,
                                            stale.then(|| {
                                                i18n::t(i18n::Key::RevisionStale, cx).to_owned()
                                            }),
                                        );
                                        this.set_status(
                                            i18n::t(
                                                if stale {
                                                    i18n::Key::RevisionReadyStale
                                                } else {
                                                    i18n::Key::RevisionReady
                                                },
                                                cx,
                                            )
                                            .into(),
                                            cx,
                                        );
                                    }
                                    Err(error) => {
                                        this.on_revision_provider_failure(error.error(), cx);
                                    }
                                }
                            })
                            .is_some()
                            {
                                break;
                            }
                            if this.upgrade().is_none() {
                                break;
                            }
                            cx.background_executor()
                                .timer(Duration::from_millis(1))
                                .await;
                        }
                    })
                    .detach();
                })
                .is_some()
                {
                    break;
                }
                if this.upgrade().is_none() {
                    break;
                }
                cx.background_executor()
                    .timer(Duration::from_millis(1))
                    .await;
            }
        })
        .detach();
    }

    pub(super) fn apply_revision(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        if !self.review_flow.has_revision_result() {
            return false;
        }
        if !self.revision_result_is_current_for_commit(cx) {
            self.set_revision_diagnostic(i18n::t(i18n::Key::RevisionStale, cx), cx);
            return false;
        }
        let revision = self
            .review_flow
            .revision_result()
            .expect("a current Revision result must remain installed");
        let Some(document) = self.active_document().cloned() else {
            return false;
        };
        let proposal = revision.result.proposal().clone();
        let decisions = revision.decisions.clone();
        let preview = revision.preview.clone();
        match document.update(cx, |document, cx| {
            document.apply_approved_revision(&proposal, &decisions, window, cx)
        }) {
            Ok(true) => {
                // Keep the stale proposal and answer context inspectable until
                // an explicit Save/Discard boundary completes. This is what
                // lets Save, Save As cancellation, and conflict prompts retain
                // the user's answers rather than silently retiring them.
                self.review_flow.record_revision_apply(&preview, &decisions);
                self.review_flow.clear_revision_diagnostic();
                self.set_status(i18n::t(i18n::Key::RevisionApplied, cx).into(), cx);
                true
            }
            Ok(false) => {
                // A current reject-all or empty proposal is already the
                // document's exact text. Treat it as an approved no-op so
                // Save and Save As still cross the normal safe boundary.
                self.review_flow.record_revision_apply(&preview, &decisions);
                self.set_status(i18n::t(i18n::Key::RevisionNoApprovedChanges, cx).into(), cx);
                false
            }
            Err(error) => {
                self.review_flow.record_revision_apply_failure();
                self.set_revision_diagnostic(
                    format!("{}: {error}", i18n::t(i18n::Key::RevisionApplyFailed, cx)),
                    cx,
                );
                false
            }
        }
    }

    /// Continue Review after the potentially expensive Agent Skill inventory
    /// has been frozen on the background executor. This method runs on the UI
    /// thread: it owns document checks, consent presentation, and all UI state
    /// mutations. The provider request remains bound to the frozen inventory.
    #[allow(clippy::too_many_arguments)]
    fn start_review_after_preparation(
        &mut self,
        built_request: Result<ReviewRequestBuildResult, ReviewRequestBuildError>,
        target: ReviewTarget,
        document_id: DocumentId,
        lens: ArtifactLens,
        language: ReviewLanguage,
        selection: Option<std::ops::Range<usize>>,
        source_snapshot: AsyncSnapshot,
        doc: WeakEntity<super::DocumentView>,
        generation: u64,
        cancelled: Arc<AtomicBool>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self
            .review_flow
            .review_request_is_current(generation, &cancelled)
        {
            return;
        }
        let Some(document) = doc.upgrade() else {
            self.review_flow
                .finish_review_request(generation, &cancelled);
            self.set_status(i18n::t(i18n::Key::ReviewDocumentClosed, cx).into(), cx);
            return;
        };
        if document.read(cx).async_snapshot(cx) != source_snapshot {
            self.review_flow
                .finish_review_request(generation, &cancelled);
            self.set_review_diagnostic(
                document_id,
                lens,
                ReviewDiagnostic::new(
                    ReviewDiagnosticCode::InvalidRequest,
                    i18n::t(i18n::Key::ReviewDocumentChanged, cx).to_string(),
                ),
                cx,
            );
            return;
        }

        let built_request = match built_request {
            Ok(request) => request,
            Err(error) => {
                log::debug!("Review request preparation failed: {error}");
                self.review_flow
                    .finish_review_request(generation, &cancelled);
                self.set_review_diagnostic(
                    document_id,
                    lens,
                    self.review_build_diagnostic(&error, cx),
                    cx,
                );
                return;
            }
        };
        let (request, skill_package) = built_request.into_parts();
        let partial = request
            .source
            .package()
            .is_some_and(SkillPackage::is_partial);
        let frozen_request = request.clone();
        if let Some(skill_package) = &skill_package
            && self.has_dirty_skill_supporting_document(skill_package, cx)
        {
            self.review_flow
                .finish_review_request(generation, &cancelled);
            self.set_review_diagnostic(
                document_id,
                lens,
                ReviewDiagnostic::new(
                    ReviewDiagnosticCode::InvalidRequest,
                    i18n::t(i18n::Key::ReviewSkillPackageChanged, cx).to_string(),
                ),
                cx,
            );
            return;
        }

        let settings = crate::settings::AppSettings::global(cx).clone();
        let vault = crate::credentials::AppCredentialVault::global(cx).clone();
        let prepared = match PreparedReview::from_settings(&settings, &vault) {
            Ok(prepared) => match prepared.bind_document_request(request, language) {
                Ok(prepared) => prepared,
                Err(error) => {
                    self.review_flow
                        .finish_review_request(generation, &cancelled);
                    self.set_review_diagnostic(
                        document_id,
                        lens,
                        self.review_error_diagnostic(&error, cx),
                        cx,
                    );
                    return;
                }
            },
            Err(error) => {
                self.review_flow
                    .finish_review_request(generation, &cancelled);
                self.set_review_diagnostic(
                    document_id,
                    lens,
                    self.review_error_diagnostic(&error, cx),
                    cx,
                );
                return;
            }
        };
        let prompt_description = i18n::model_request_disclosure(prepared.disclosure(), cx);
        let answer = window.prompt(
            PromptLevel::Warning,
            i18n::t(i18n::Key::ModelRequestConsentTitle, cx),
            Some(&prompt_description),
            &[
                PromptButton::ok(i18n::t(i18n::Key::SendToModel, cx)),
                PromptButton::cancel(i18n::t(i18n::Key::Cancel, cx)),
            ],
            cx,
        );

        let pending = Arc::new(Mutex::new(Some(prepared)));
        let selection_for_result = selection.clone();
        let request_for_result = frozen_request.clone();
        let lens_for_result = lens;
        let skill_package_for_result = skill_package.clone();
        let cancelled_for_request = cancelled.clone();
        cx.spawn_in(window, async move |this, cx| {
            let approved = answer.await.unwrap_or(1) == 0;
            let pending = pending.clone();

            loop {
                let pending = pending.clone();
                let doc = doc.clone();
                let source_snapshot = source_snapshot.clone();
                let cancelled = cancelled_for_request.clone();
                let selection = selection_for_result.clone();
                let partial = partial;
                let request_for_result = request_for_result.clone();
                let skill_package = skill_package_for_result.clone();
                if crate::views::try_update_in(&this, cx, move |this, window, cx| {
                    let Some(prepared) = pending
                        .lock()
                        .unwrap_or_else(|poisoned| poisoned.into_inner())
                        .take()
                    else {
                        return;
                    };

                    if !this
                        .review_flow
                        .review_request_is_current(generation, &cancelled)
                    {
                        return;
                    }
                    if !approved {
                        this.set_review_diagnostic(
                            document_id,
                            lens_for_result,
                            ReviewDiagnostic::new(
                                ReviewDiagnosticCode::Cancelled,
                                i18n::t(i18n::Key::ReviewCancelled, cx).to_string(),
                            ),
                            cx,
                        );
                        return;
                    }

                    let Some(document) = doc.upgrade() else {
                        this.review_flow
                            .finish_review_request(generation, &cancelled);
                        this.set_status(i18n::t(i18n::Key::ReviewDocumentClosed, cx).into(), cx);
                        return;
                    };
                    if document.read(cx).async_snapshot(cx) != source_snapshot {
                        this.review_flow
                            .finish_review_request(generation, &cancelled);
                        this.set_review_diagnostic(
                            document_id,
                            lens_for_result,
                            ReviewDiagnostic::new(
                                ReviewDiagnosticCode::InvalidRequest,
                                i18n::t(i18n::Key::ReviewDocumentChanged, cx).to_string(),
                            ),
                            cx,
                        );
                        return;
                    }
                    if let Some(skill_package) = &skill_package
                        && this.has_dirty_skill_supporting_document(skill_package, cx)
                    {
                        this.review_flow
                            .finish_review_request(generation, &cancelled);
                        this.set_review_diagnostic(
                            document_id,
                            lens_for_result,
                            ReviewDiagnostic::new(
                                ReviewDiagnosticCode::InvalidRequest,
                                i18n::t(i18n::Key::ReviewSkillPackageChanged, cx).to_string(),
                            ),
                            cx,
                        );
                        return;
                    }

                    // Revalidation belongs before authorization. The frozen
                    // package is checked on a background executor, then the UI
                    // rechecks generation, document snapshot, and dirty
                    // supporting buffers before creating the authorization.
                    let pending = Arc::new(Mutex::new(Some(prepared)));
                    let package_for_revalidation = skill_package.clone();
                    let cancelled_for_revalidation = cancelled.clone();
                    cx.spawn_in(window, async move |this, cx| {
                        let revalidation = cx
                            .background_spawn(async move {
                                package_for_revalidation
                                    .map_or(Ok(()), |package| package.revalidate())
                            })
                            .await;
                        let revalidation = Arc::new(Mutex::new(Some(revalidation)));
                        loop {
                            let revalidation = revalidation.clone();
                            let pending = pending.clone();
                            let doc = doc.clone();
                            let source_snapshot = source_snapshot.clone();
                            let selection = selection.clone();
                            let partial = partial;
                            let request_for_result = request_for_result.clone();
                            let request_for_result_for_update = request_for_result.clone();
                            let skill_package = skill_package.clone();
                            let cancelled = cancelled_for_revalidation.clone();
                            if crate::views::try_update_in(&this, cx, move |this, window, cx| {
                                let Some(revalidation) = revalidation
                                    .lock()
                                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                                    .take()
                                else {
                                    return;
                                };
                                let Some(prepared) = pending
                                    .lock()
                                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                                    .take()
                                else {
                                    return;
                                };
                                if !this
                                    .review_flow
                                    .review_request_is_current(generation, &cancelled)
                                {
                                    return;
                                }
                                let Some(document) = doc.upgrade() else {
                                    this.review_flow
                                        .finish_review_request(generation, &cancelled);
                                    this.set_status(
                                        i18n::t(i18n::Key::ReviewDocumentClosed, cx).into(),
                                        cx,
                                    );
                                    return;
                                };
                                if document.read(cx).async_snapshot(cx) != source_snapshot {
                                    this.set_review_diagnostic(
                                        document_id,
                                        lens_for_result,
                                        ReviewDiagnostic::new(
                                            ReviewDiagnosticCode::InvalidRequest,
                                            i18n::t(i18n::Key::ReviewDocumentChanged, cx)
                                                .to_string(),
                                        ),
                                        cx,
                                    );
                                    return;
                                }
                                if let Some(skill_package) = &skill_package
                                    && this.has_dirty_skill_supporting_document(skill_package, cx)
                                {
                                    this.set_review_diagnostic(
                                        document_id,
                                        lens_for_result,
                                        ReviewDiagnostic::new(
                                            ReviewDiagnosticCode::InvalidRequest,
                                            i18n::t(
                                                i18n::Key::ReviewSkillPackageChanged,
                                                cx,
                                            )
                                            .to_string(),
                                        ),
                                        cx,
                                    );
                                    return;
                                }
                                if let Err(error) = revalidation {
                                    this.set_review_diagnostic(
                                        document_id,
                                        lens_for_result,
                                        this.review_build_diagnostic(&error, cx),
                                        cx,
                                    );
                                    return;
                                }

                                let mut consent = ConsentCapability::from_decision(
                                    prepared.disclosure(),
                                    ConsentDecision::Approve,
                                );
                                let authorization = match prepared.authorize(&mut consent) {
                                    Ok(authorization) => authorization,
                                    Err(error) => {
                                        this.set_review_diagnostic(
                                            document_id,
                                            lens_for_result,
                                            this.review_error_diagnostic(&error, cx),
                                            cx,
                                        );
                                        return;
                                    }
                                };
                                let cancel_for_request = cancelled.clone();
                                this.set_status(
                                    i18n::t(i18n::Key::ReviewWaiting, cx).into(),
                                    cx,
                                );
                                cx.spawn_in(window, async move |this, cx| {
                                    let result = cx
                                        .background_spawn(async move {
                                            prepared.execute_with(
                                                authorization,
                                                &cancel_for_request,
                                                REVIEW_REQUEST_TIMEOUT,
                                            )
                                        })
                                        .await;
                                    let package_for_completion_revalidation = skill_package.clone();
                                    let supporting_sources_revalidated = cx
                                        .background_spawn(async move {
                                            package_for_completion_revalidation
                                                .is_none_or(|package| package.revalidate().is_ok())
                                        })
                                        .await;
                                    let result = Arc::new(Mutex::new(Some(result)));
                                    let cancelled_for_completion = cancelled.clone();
                                    loop {
                                        let result = result.clone();
                                        let doc = doc.clone();
                                        let source_snapshot = source_snapshot.clone();
                                        let selection = selection.clone();
                                        let partial = partial;
                                        let skill_package = skill_package.clone();
                                        let cancelled = cancelled_for_completion.clone();
                                        let request_for_result_for_update =
                                            request_for_result_for_update.clone();
                                        if crate::views::try_update_in(
                                            &this,
                                            cx,
                                            move |this, window, cx| {
                                                let Some(result) = result
                                                    .lock()
                                                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                                                    .take()
                                                else {
                                                    return;
                                                };
                                                if !this
                                                    .review_flow
                                                    .finish_review_request(generation, &cancelled)
                                                {
                                                    return;
                                                }
                                                match result {
                                                    Ok(result) => {
                                                        let Some(document) = doc.upgrade() else {
                                                            this.set_status(
                                                                i18n::t(
                                                                    i18n::Key::ReviewDocumentClosed,
                                                                    cx,
                                                                )
                                                                .into(),
                                                                cx,
                                                            );
                                                            return;
                                                        };
                                                        let supporting_sources_current =
                                                            skill_package.as_ref().is_none_or(
                                                                |package| {
                                                                    supporting_sources_revalidated
                                                                        && !this
                                                                            .has_dirty_skill_supporting_document(
                                                                                package, cx,
                                                                            )
                                                                },
                                                            );
                                                        let stale = document
                                                            .read(cx)
                                                            .async_snapshot(cx)
                                                            != source_snapshot
                                                            || !supporting_sources_current;
                                                        let document_id = document.read(cx).id();
                                                        let recovery_key =
                                                            document.read(cx).recovery_key();
                                                        if this
                                                            .review_flow
                                                            .revision_context_has_authored_answers_for_other_document(
                                                                document_id,
                                                            )
                                                        {
                                                            this.review_flow.clear_review_target();
                                                            this.set_status(
                                                                i18n::t(
                                                                    i18n::Key::RevisionAnswersRetained,
                                                                    cx,
                                                                )
                                                                .into(),
                                                                cx,
                                                            );
                                                            return;
                                                        }
                                                        this.review_flow.install_review_result(
                                                            WorkspaceReviewResult {
                                                                document_id,
                                                                source_snapshot:
                                                                    source_snapshot.clone(),
                                                                target,
                                                                selection,
                                                                lens: lens_for_result,
                                                                partial,
                                                                skill_package:
                                                                    skill_package.clone(),
                                                                supporting_sources_current,
                                                                result,
                                                            },
                                                            stale,
                                                        );
                                                        if let Some(output) = this
                                                            .review_flow
                                                            .review_result()
                                                            .and_then(|review| {
                                                                review.result.result.output.clone()
                                                            })
                                                        {
                                                            this.install_revision_context(
                                                                document_id,
                                                                recovery_key,
                                                                source_snapshot.clone(),
                                                                request_for_result_for_update,
                                                                output,
                                                                skill_package,
                                                                supporting_sources_current,
                                                                window,
                                                                cx,
                                                            );
                                                        }
                                                        this.set_status(
                                                            i18n::t(
                                                                if stale {
                                                                    i18n::Key::ReviewStale
                                                                } else {
                                                                    i18n::Key::ReviewReady
                                                                },
                                                                cx,
                                                            )
                                                            .into(),
                                                            cx,
                                                        );
                                                    }
                                                    Err(error) => {
                                                        this.set_review_diagnostic(
                                                            document_id,
                                                            lens_for_result,
                                                            this.review_error_diagnostic(&error, cx),
                                                            cx,
                                                        );
                                                    }
                                                }
                                            },
                                        )
                                        .is_some()
                                        {
                                            break;
                                        }
                                        if this.upgrade().is_none() {
                                            break;
                                        }
                                        cx.background_executor()
                                            .timer(Duration::from_millis(1))
                                            .await;
                                    }
                                })
                                .detach();
                            })
                            .is_some()
                            {
                                break;
                            }
                            if this.upgrade().is_none() {
                                break;
                            }
                            cx.background_executor()
                                .timer(Duration::from_millis(1))
                                .await;
                        }
                    })
                    .detach();
                })
                .is_some()
                {
                    break;
                }
                if this.upgrade().is_none() {
                    break;
                }
                cx.background_executor()
                    .timer(Duration::from_millis(1))
                    .await;
            }
        })
        .detach();
    }

    /// Run a read-only Review over a frozen editor snapshot.
    ///
    /// Consent is bound to the exact provider disclosure and is consumed once.
    /// The request is rechecked against the same snapshot after consent and
    /// before transport; a result that lands after an edit is retained only as
    /// stale inspection. Review never mutates editor source text.
    pub(super) fn review(
        &mut self,
        target: ReviewTarget,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(doc) = self.active_document().cloned() else {
            return;
        };
        let (document_id, source_path, skill_entrypoint_is_dirty, skill_origin) = {
            let document = doc.read(cx);
            (
                document.id(),
                document.source_path().map(Path::to_path_buf),
                document.is_dirty(),
                document.skill_origin().cloned(),
            )
        };
        if self.revision_has_authored_answers_for_document(document_id, cx) {
            self.set_status(i18n::t(i18n::Key::RevisionAnswersRetained, cx).into(), cx);
            self.review_flow
                .open_review_panel(target, document_id, true, false);
            self.right_panel_open = true;
            cx.notify();
            return;
        }
        let source_snapshot = doc.read(cx).async_snapshot(cx);
        let full_text = source_snapshot.text().to_owned();
        // Infer a conservative first lens from the path. Once the user has
        // corrected it in the Review panel, keep that choice for this tab.
        let lens = self.visible_review_lens(cx);

        // An attempted Review supersedes the previously displayed operation
        // even when its scope is invalid or its provider is unavailable.
        self.review_flow.begin_review_attempt(target, document_id);
        self.right_panel_open = true;
        cx.notify();
        let selection = match target {
            ReviewTarget::Document => None,
            ReviewTarget::Selection => {
                let range = doc.read(cx).selection(cx);
                if range.is_empty() {
                    self.set_review_diagnostic(
                        document_id,
                        lens,
                        ReviewDiagnostic::new(
                            ReviewDiagnosticCode::EmptySelection,
                            i18n::t(i18n::Key::ReviewEmptySelection, cx).to_string(),
                        ),
                        cx,
                    );
                    return;
                }
                if full_text.get(range.clone()).is_none() {
                    self.set_review_diagnostic(
                        document_id,
                        lens,
                        ReviewDiagnostic::new(
                            ReviewDiagnosticCode::InvalidRequest,
                            i18n::t(i18n::Key::ReviewFailed, cx).to_string(),
                        ),
                        cx,
                    );
                    return;
                }
                Some(range)
            }
        };

        let language = match crate::settings::AppSettings::global(cx).language {
            mt_core::settings::Language::English => ReviewLanguage::English,
            mt_core::settings::Language::Chinese => ReviewLanguage::SimplifiedChinese,
        };
        let document_snapshot =
            SourceSnapshot::new(doc.read(cx).revision(), source_snapshot.source_generation());
        let (generation, cancelled) = self.review_flow.begin_review_request(document_id, lens);
        self.right_panel_open = true;
        self.set_status(i18n::t(i18n::Key::ReviewWaiting, cx).into(), cx);

        let doc = doc.downgrade();
        let source_path_for_prepare = source_path.clone();
        let full_text_for_prepare = full_text.clone();
        let selection_for_prepare = selection.clone();
        let skill_origin_for_prepare = skill_origin;
        let selection_for_result = selection.clone();
        cx.spawn_in(window, async move |this, cx| {
            let built_request = cx
                .background_spawn(async move {
                    build_review_request(
                        ReviewRequestBuildRequest::new(
                            target,
                            lens,
                            source_path_for_prepare.as_deref(),
                            &full_text_for_prepare,
                            selection_for_prepare.as_ref(),
                            document_snapshot,
                            skill_entrypoint_is_dirty,
                        )
                        .with_skill_origin(skill_origin_for_prepare.as_ref()),
                    )
                })
                .await;
            let built_request = Arc::new(Mutex::new(Some(built_request)));
            loop {
                let built_request = built_request.clone();
                let selection_for_update = selection_for_result.clone();
                let source_snapshot_for_update = source_snapshot.clone();
                let doc_for_update = doc.clone();
                let cancelled_for_update = cancelled.clone();
                if crate::views::try_update_in(&this, cx, move |this, window, cx| {
                    let Some(built_request) = built_request
                        .lock()
                        .unwrap_or_else(|poisoned| poisoned.into_inner())
                        .take()
                    else {
                        return;
                    };
                    this.start_review_after_preparation(
                        built_request,
                        target,
                        document_id,
                        lens,
                        language,
                        selection_for_update,
                        source_snapshot_for_update,
                        doc_for_update,
                        generation,
                        cancelled_for_update,
                        window,
                        cx,
                    );
                })
                .is_some()
                {
                    break;
                }
                if this.upgrade().is_none() {
                    break;
                }
                cx.background_executor()
                    .timer(Duration::from_millis(1))
                    .await;
            }
        })
        .detach();
    }

    fn set_review_diagnostic(
        &mut self,
        document_id: DocumentId,
        lens: ArtifactLens,
        diagnostic: ReviewDiagnostic,
        cx: &mut Context<Self>,
    ) {
        let status = self
            .review_flow
            .set_review_diagnostic(document_id, lens, diagnostic);
        self.right_panel_open = true;
        self.set_status(status, cx);
    }

    fn review_build_diagnostic(
        &self,
        error: &ReviewRequestBuildError,
        cx: &Context<Self>,
    ) -> ReviewDiagnostic {
        let code = match error {
            ReviewRequestBuildError::InvalidSelection => ReviewDiagnosticCode::EmptySelection,
            ReviewRequestBuildError::AgentSkillFileTooLarge { .. }
            | ReviewRequestBuildError::AgentSkillPackageTooLarge { .. } => {
                ReviewDiagnosticCode::OversizedPayload
            }
            _ => ReviewDiagnosticCode::InvalidRequest,
        };
        ReviewDiagnostic::new(
            code,
            i18n::t(review_request_build_status_key(error), cx).to_string(),
        )
    }

    fn review_error_diagnostic(&self, error: &ReviewError, cx: &Context<Self>) -> ReviewDiagnostic {
        let (code, key) = match error {
            ReviewError::NoAvailableCredential => (
                ReviewDiagnosticCode::NoProvider,
                i18n::Key::ReviewNoProvider,
            ),
            ReviewError::MissingCredential { .. } => (
                ReviewDiagnosticCode::Unavailable,
                i18n::Key::ReviewMissingCredential,
            ),
            ReviewError::RequestTooLarge { .. }
            | ReviewError::ResponseTooLarge { .. }
            | ReviewError::InvalidRequest {
                reason:
                    ReviewRequestError::FileTooLarge { .. } | ReviewRequestError::SourceTooLarge { .. },
            } => (
                ReviewDiagnosticCode::OversizedPayload,
                i18n::Key::ReviewOversized,
            ),
            ReviewError::Cancelled { .. } => {
                (ReviewDiagnosticCode::Cancelled, i18n::Key::ReviewCancelled)
            }
            ReviewError::Timeout { .. } => {
                (ReviewDiagnosticCode::Timeout, i18n::Key::ReviewTimeout)
            }
            ReviewError::MalformedResponse { .. } | ReviewError::MissingResponseText { .. } => (
                ReviewDiagnosticCode::MalformedResponse,
                i18n::Key::ReviewMalformed,
            ),
            ReviewError::TransportUnavailable { .. } | ReviewError::RequestFailed { .. } => (
                ReviewDiagnosticCode::Unavailable,
                i18n::Key::ReviewUnavailable,
            ),
            ReviewError::InvalidRequest { .. } => (
                ReviewDiagnosticCode::InvalidRequest,
                i18n::Key::ReviewFailed,
            ),
            _ => (ReviewDiagnosticCode::Unavailable, i18n::Key::ReviewFailed),
        };
        ReviewDiagnostic::new(code, i18n::t(key, cx).to_string())
    }

    fn visible_review_lens(&self, cx: &Context<Self>) -> ArtifactLens {
        let Some(document) = self.active_document() else {
            return ArtifactLens::default_lens();
        };
        let document = document.read(cx);
        let inferred = document
            .source_path()
            .map(ArtifactLens::infer_from_path)
            .unwrap_or_else(ArtifactLens::default_lens);
        self.review_flow
            .visible_review_lens(document.id(), inferred)
    }

    pub(super) fn reveal_review_anchor(
        &mut self,
        anchor: &SourceAnchor,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some((document_id, source_snapshot, supporting_sources_current)) =
            self.review_flow.review_result().map(|review| {
                (
                    review.document_id,
                    review.source_snapshot.clone(),
                    review.supporting_sources_current,
                )
            })
        else {
            return;
        };
        let document_current = self.active_document().is_some_and(|document| {
            let document = document.read(cx);
            document.id() == document_id && document.async_snapshot(cx) == source_snapshot
        });
        if !document_current || !supporting_sources_current {
            self.set_status(i18n::t(i18n::Key::ReviewStale, cx).into(), cx);
            return;
        }

        if let Some(offset) = resolve_document_anchor_offset(anchor, source_snapshot.text()) {
            self.reveal_offset(offset, window, cx);
            return;
        }

        let Some(skill_package) = self
            .review_flow
            .review_result()
            .and_then(|review| review.skill_package.clone())
        else {
            return;
        };
        if self.has_dirty_skill_supporting_document(&skill_package, cx)
            || skill_package.revalidate().is_err()
        {
            self.mark_review_skill_package_stale(cx);
            return;
        }
        let Some((path, offset)) = skill_package.resolve_anchor(anchor) else {
            return;
        };
        let is_skill_entrypoint_anchor =
            matches!(anchor, SourceAnchor::AgentSkillFile { path, .. } if path == "SKILL.md");
        let editor_only_entrypoint_anchor =
            skill_package.entrypoint_is_editor_text() && is_skill_entrypoint_anchor;
        if editor_only_entrypoint_anchor {
            // The active document and this result were checked against the same
            // immutable snapshot above. In particular, editor-only SKILL.md
            // anchors must reveal that dirty buffer rather than reopening the
            // entrypoint through its canonical pathname.
            self.reveal_offset(offset, window, cx);
            return;
        }
        if is_skill_entrypoint_anchor {
            // A disk-backed SKILL.md is also the active source document whose
            // identity and text snapshot were verified above. Keep navigation
            // on that tab rather than reopening the frozen package pathname.
            self.reveal_offset(offset, window, cx);
            return;
        }
        self.open_frozen_supporting_file(&skill_package, path, offset, window, cx);
    }

    pub(super) fn mark_review_skill_package_stale(&mut self, cx: &mut Context<Self>) {
        self.review_flow.mark_review_supporting_sources_stale();
        self.set_status(i18n::t(i18n::Key::ReviewStale, cx).into(), cx);
        cx.notify();
    }

    pub(super) fn render_recovered_revision_panel(&self, cx: &Context<Self>) -> AnyElement {
        let answer_count = self
            .recovered_revision_answers_for_active_document(cx)
            .map_or(0, |(_, recovery)| recovery.answers().len());
        v_flex()
            .id("review-panel")
            .size_full()
            .p(metrics::inset())
            .gap(metrics::gap_group())
            .overflow_y_scroll()
            .child(
                v_flex()
                    .id("revision-recovered-answers")
                    .role(gpui::Role::Group)
                    .aria_label(i18n::t(i18n::Key::RevisionRecoveredAnswers, cx))
                    .accessibility_id(REVISION_RECOVERED_ANSWERS_ACCESSIBILITY_ID)
                    .gap(metrics::gap())
                    .child(div().text_xs().font_medium().child(format!(
                        "{} ({answer_count})",
                        i18n::t(i18n::Key::RevisionRecoveredAnswers, cx)
                    )))
                    .child(
                        h_flex()
                            .gap(metrics::gap())
                            .child(
                                Button::new("revision-copy-recovered-answers")
                                    .accessibility_id(
                                        REVISION_COPY_RECOVERED_ANSWERS_ACCESSIBILITY_ID,
                                    )
                                    .label(i18n::t(i18n::Key::RevisionCopyRecoveredAnswers, cx))
                                    .small()
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.copy_recovered_revision_answers(cx);
                                    })),
                            )
                            .child(
                                Button::new("revision-discard-recovered-answers")
                                    .accessibility_id(
                                        REVISION_DISCARD_RECOVERED_ANSWERS_ACCESSIBILITY_ID,
                                    )
                                    .label(i18n::t(i18n::Key::RevisionDiscardRecoveredAnswers, cx))
                                    .small()
                                    .ghost()
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.discard_recovered_revision_answers(cx);
                                    })),
                            ),
                    ),
            )
            .into_any_element()
    }

    pub(super) fn render_review_idle_panel(&self, cx: &Context<Self>) -> AnyElement {
        let run_target = self.active_document().and_then(|document| {
            self.review_flow
                .review_target_for_document(document.read(cx).id())
        });
        v_flex()
            .id("review-panel")
            .size_full()
            .p(metrics::inset())
            .gap(metrics::gap_group())
            .overflow_y_scroll()
            .child(
                div()
                    .text_sm()
                    .text_color(cx.theme().muted_foreground)
                    .child(i18n::t(i18n::Key::ReviewNoResult, cx)),
            )
            .when_some(run_target, |this, target| {
                this.child(
                    Button::new("run-review")
                        .accessibility_id(REVIEW_RUN_ACCESSIBILITY_ID)
                        .label(i18n::t(i18n::Key::ReviewRun, cx))
                        .small()
                        .primary()
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.review(target, window, cx);
                        })),
                )
            })
            .into_any_element()
    }
}

pub(super) fn revision_change_ids(
    proposal: &mt_core::review::revision::RevisionProposal,
) -> Vec<ChangeId> {
    proposal
        .hunks()
        .iter()
        .map(|hunk| hunk.change_id())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

pub(super) fn revision_answer_is_incorporated(
    answer: &RevisionAnswer,
    coverage: Option<&RevisionQuestionCoverageStatus>,
    decisions: &[(ChangeId, bool)],
) -> bool {
    match answer {
        RevisionAnswer::Unanswered => true,
        RevisionAnswer::IntentionallyUnspecified => matches!(
            coverage,
            Some(RevisionQuestionCoverageStatus::IntentionallyOmitted { .. })
        ),
        RevisionAnswer::Answered(_) => {
            let Some(RevisionQuestionCoverageStatus::Represented { change_ids }) = coverage else {
                return false;
            };
            !change_ids.is_empty()
                && change_ids.iter().all(|change_id| {
                    decisions
                        .iter()
                        .find(|(candidate, _)| candidate == change_id)
                        .is_some_and(|(_, accepted)| *accepted)
                })
        }
    }
}

pub(super) fn revision_apply_identity_matches(
    applied_preview: Option<&str>,
    applied_decisions: Option<&[(ChangeId, bool)]>,
    current_preview: &str,
    current_decisions: &[(ChangeId, bool)],
) -> bool {
    applied_preview == Some(current_preview) && applied_decisions == Some(current_decisions)
}

pub(super) fn revision_question_accessibility_id(
    index: usize,
    question: &mt_core::review::ClarificationQuestion,
) -> String {
    format!(
        "markturbo-revision-question-{}",
        revision_question_id(index, question)
    )
}

pub(super) fn build_revision_recovery(
    request: &DocumentReviewRequest,
    review_output: &ReviewModelOutput,
    answer_states: &[RevisionAnswer],
) -> Option<RevisionRecovery> {
    let answers = RevisionAnswers::for_recovery(answer_states.to_vec()).ok()?;
    let binding = revision_request_binding(request, review_output, &answers).ok()?;
    Some(RevisionRecovery::new(binding, answers))
}

pub(super) fn export_recovered_revision_answers(
    recovery: &RevisionRecovery,
) -> Result<String, serde_json::Error> {
    let answers = recovery
        .answers()
        .as_slice()
        .iter()
        .enumerate()
        .map(|(question_index, answer)| WorkspaceRevisionExportAnswer {
            question_index,
            state: answer.state_label().to_owned(),
            answer: match answer {
                RevisionAnswer::Answered(value) => Some(value.clone()),
                RevisionAnswer::Unanswered | RevisionAnswer::IntentionallyUnspecified => None,
            },
        })
        .collect::<Vec<_>>();
    let binding = recovery.binding();
    let payload = WorkspaceRevisionRecoveryExport {
        source_sha256: super::hex_bytes(binding.source_sha256()),
        source_revision: binding.source_revision(),
        source_generation: binding.source_generation(),
        artifact_lens_sha256: super::hex_bytes(binding.artifact_lens_digest()),
        review_context_sha256: super::hex_bytes(binding.review_context_digest()),
        answers_sha256: super::hex_bytes(binding.answers_digest()),
        answers,
    };
    serde_json::to_string_pretty(&payload)
}

fn review_lens_key(lens: ArtifactLens) -> i18n::Key {
    match lens {
        ArtifactLens::Prompt => i18n::Key::ReviewLensPrompt,
        ArtifactLens::Specification | ArtifactLens::Plan => i18n::Key::ReviewLensSpecification,
        ArtifactLens::AgentInstructions => i18n::Key::ReviewLensAgentInstructions,
        ArtifactLens::AgentSkill => i18n::Key::ReviewLensAgentSkill,
    }
}

fn review_lens_choice_is_selected(choice: ArtifactLens, visible: ArtifactLens) -> bool {
    choice == visible || (choice == ArtifactLens::Specification && visible == ArtifactLens::Plan)
}

fn clarification_priority_label(
    priority: ClarificationPriority,
    cx: &Context<Workspace>,
) -> SharedString {
    match priority {
        ClarificationPriority::Critical => i18n::t(i18n::Key::ReviewPriorityCritical, cx).into(),
        ClarificationPriority::High => i18n::t(i18n::Key::ReviewPriorityHigh, cx).into(),
        ClarificationPriority::Medium => i18n::t(i18n::Key::ReviewPriorityMedium, cx).into(),
        ClarificationPriority::Low => i18n::t(i18n::Key::ReviewPriorityLow, cx).into(),
    }
}

#[cfg(test)]
pub(super) fn next_review_lens(lens: ArtifactLens) -> ArtifactLens {
    match lens {
        ArtifactLens::Prompt => ArtifactLens::Specification,
        ArtifactLens::Specification => ArtifactLens::Plan,
        ArtifactLens::Plan => ArtifactLens::AgentInstructions,
        ArtifactLens::AgentInstructions => ArtifactLens::AgentSkill,
        ArtifactLens::AgentSkill => ArtifactLens::Prompt,
    }
}

fn push_review_text_section(
    content: &mut Vec<AnyElement>,
    label: impl Into<SharedString>,
    text: &str,
    cx: &Context<Workspace>,
) {
    content.push(
        v_flex()
            .gap(metrics::gap())
            .child(
                div()
                    .text_xs()
                    .font_medium()
                    .text_color(cx.theme().muted_foreground)
                    .child(label.into()),
            )
            // Deliberately use a plain GPUI text child. Model prose is inert
            // structured data and must never pass through Markdown/HTML/MDX.
            .child(div().text_sm().child(text.to_owned()))
            .into_any_element(),
    );
}

fn push_review_text_list(
    content: &mut Vec<AnyElement>,
    label: impl Into<SharedString>,
    values: &[StructuredText],
    cx: &Context<Workspace>,
) {
    if values.is_empty() {
        return;
    }
    let label: SharedString = label.into();
    let rows = values.iter().enumerate().map(|(ix, value)| {
        ListItem::new(SharedString::from(format!("review-text-{ix}-{label}")))
            .w_full()
            .child(div().text_sm().child(value.as_str().to_owned()))
            .into_any_element()
    });
    content.push(
        v_flex()
            .gap(metrics::gap())
            .child(
                div()
                    .text_xs()
                    .font_medium()
                    .text_color(cx.theme().muted_foreground)
                    .child(label.clone()),
            )
            .children(rows)
            .into_any_element(),
    );
}

fn review_anchor_label(anchor: &SourceAnchor, cx: &Context<Workspace>) -> String {
    match anchor {
        SourceAnchor::DocumentWide => i18n::t(i18n::Key::ReviewDocumentWide, cx).to_string(),
        SourceAnchor::Document { location } => review_location_label(*location),
        SourceAnchor::AgentSkillFile { path, location } => {
            format!("{path}:{}", review_location_label(*location))
        }
    }
}

fn review_location_label(location: SourceLocation) -> String {
    match location {
        SourceLocation::ByteRange { start, end } | SourceLocation::LineRange { start, end } => {
            format!("{start}..{end}")
        }
    }
}

fn review_request_build_status_key(error: &ReviewRequestBuildError) -> i18n::Key {
    match error {
        ReviewRequestBuildError::AgentSkillRootUnavailable
        | ReviewRequestBuildError::AgentSkillSourceUnavailable => {
            i18n::Key::ReviewSkillPackageUnavailable
        }
        ReviewRequestBuildError::AgentSkillEntrypointUnavailable => {
            i18n::Key::ReviewSkillPackageEntrypointMissing
        }
        ReviewRequestBuildError::AgentSkillSelectionUnsupported => {
            i18n::Key::ReviewSkillPackageSelectionUnsupported
        }
        ReviewRequestBuildError::AgentSkillPathIsNotUtf8 => {
            i18n::Key::ReviewSkillPackageNonUtf8Path
        }
        ReviewRequestBuildError::AgentSkillReadFailed => i18n::Key::ReviewSkillPackageReadFailed,
        ReviewRequestBuildError::AgentSkillSourceChanged => i18n::Key::ReviewSkillPackageChanged,
        ReviewRequestBuildError::AgentSkillFileTooLarge { .. }
        | ReviewRequestBuildError::AgentSkillPackageTooLarge { .. } => {
            i18n::Key::ReviewSkillPackageOversized
        }
        ReviewRequestBuildError::InvalidAgentSkillPackage(_)
        | ReviewRequestBuildError::InvalidSelection
        | ReviewRequestBuildError::InvalidRequest(_) => i18n::Key::ReviewFailed,
    }
}

fn clear_revision_applied_state(context: &mut WorkspaceRevisionContext) {
    context.applied = None;
}

impl Workspace {
    pub(super) fn render_review_panel(&self, cx: &Context<Self>) -> AnyElement {
        let visible_lens = self.visible_review_lens(cx);
        let active_document_id = self
            .active_document()
            .map(|document| document.read(cx).id());
        let active_document_has_authored_answers = active_document_id.is_some_and(|document_id| {
            self.revision_has_authored_answers_for_document(document_id, cx)
        });
        let lens_options = [
            ArtifactLens::Prompt,
            ArtifactLens::Specification,
            ArtifactLens::AgentInstructions,
            ArtifactLens::AgentSkill,
        ];
        let mut content = Vec::<AnyElement>::new();
        content.push(
            div()
                .text_xs()
                .text_color(cx.theme().muted_foreground)
                .child(i18n::t(i18n::Key::ReviewReadOnly, cx))
                .into_any_element(),
        );
        if self.review_flow.is_reviewing() {
            content.push(
                h_flex()
                    .gap(metrics::gap())
                    .items_center()
                    .child(Spinner::new().small())
                    .child(i18n::t(i18n::Key::ReviewWaiting, cx))
                    .child(
                        Button::new("cancel-review")
                            .icon(IconName::Close)
                            .xsmall()
                            .ghost()
                            .tooltip(i18n::t(i18n::Key::Cancel, cx))
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.on_cancel_review(&CancelReview, window, cx)
                            })),
                    )
                    .into_any_element(),
            );
        }
        content.push(
            v_flex()
                .gap(metrics::gap())
                .child(
                    div()
                        .text_xs()
                        .font_medium()
                        .text_color(cx.theme().muted_foreground)
                        .child(i18n::t(i18n::Key::ReviewLens, cx)),
                )
                .child(
                    h_flex()
                        .gap(metrics::gap())
                        .flex_wrap()
                        .children(lens_options.map(|lens| {
                            Button::new(SharedString::from(format!("review-lens-{}", lens.label())))
                                .label(i18n::t(review_lens_key(lens), cx))
                                .xsmall()
                                .disabled(
                                    self.review_flow.is_reviewing()
                                        || self.review_flow.is_revision_running()
                                        || self.review_flow.has_revision_result()
                                        || active_document_has_authored_answers,
                                )
                                .when(
                                    review_lens_choice_is_selected(lens, visible_lens),
                                    |button| button.primary(),
                                )
                                .when(
                                    !review_lens_choice_is_selected(lens, visible_lens),
                                    |button| button.ghost(),
                                )
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    let document_id = this
                                        .active_document()
                                        .map(|document| document.read(cx).id());
                                    if let Some(document_id) = document_id {
                                        if this.revision_has_authored_answers_for_document(
                                            document_id,
                                            cx,
                                        ) {
                                            this.set_status(
                                                i18n::t(i18n::Key::RevisionAnswersRetained, cx)
                                                    .into(),
                                                cx,
                                            );
                                            return;
                                        }
                                        this.review_flow.choose_review_lens(document_id, lens);
                                    } else {
                                        this.review_flow.clear_review_outcome();
                                    }
                                    cx.notify();
                                }))
                        })),
                )
                .into_any_element(),
        );

        let run_target = active_document_id
            .and_then(|document_id| self.review_flow.review_target_for_document(document_id));
        if let Some(target) = run_target
            && !self.review_flow.is_reviewing()
            && !active_document_has_authored_answers
        {
            content.push(
                Button::new("run-review")
                    .accessibility_id(REVIEW_RUN_ACCESSIBILITY_ID)
                    .label(i18n::t(i18n::Key::ReviewRun, cx))
                    .small()
                    .primary()
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.review(target, window, cx);
                    }))
                    .into_any_element(),
            );
        }

        let diagnostic = self
            .review_flow
            .review_diagnostic()
            .filter(|diagnostic| Some(diagnostic.document_id) == active_document_id);
        if let Some(diagnostic) = diagnostic {
            content.push(
                div()
                    .id("review-diagnostic")
                    .role(gpui_kit::Role::Label)
                    .aria_value(diagnostic.diagnostic.message.as_str())
                    .accessibility_id(REVIEW_DIAGNOSTIC_ACCESSIBILITY_ID)
                    .text_sm()
                    .text_color(cx.theme().warning)
                    .child(diagnostic.diagnostic.message.as_str().to_owned())
                    .into_any_element(),
            );
        }

        let recovered_answers = self
            .recovered_revision_answers_for_active_document(cx)
            .map(|(_, recovery)| recovery.answers().clone());
        if let Some(recovered_answers) = recovered_answers {
            let answer_count = recovered_answers.len();
            content.push(
                v_flex()
                    .id("revision-recovered-answers")
                    .role(gpui::Role::Group)
                    .aria_label(i18n::t(i18n::Key::RevisionRecoveredAnswers, cx))
                    .accessibility_id(REVISION_RECOVERED_ANSWERS_ACCESSIBILITY_ID)
                    .gap(metrics::gap())
                    .child(div().text_xs().font_medium().child(format!(
                        "{} ({answer_count})",
                        i18n::t(i18n::Key::RevisionRecoveredAnswers, cx)
                    )))
                    .child(
                        h_flex()
                            .gap(metrics::gap())
                            .child(
                                Button::new("revision-copy-recovered-answers")
                                    .accessibility_id(
                                        REVISION_COPY_RECOVERED_ANSWERS_ACCESSIBILITY_ID,
                                    )
                                    .label(i18n::t(i18n::Key::RevisionCopyRecoveredAnswers, cx))
                                    .small()
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.copy_recovered_revision_answers(cx);
                                    })),
                            )
                            .child(
                                Button::new("revision-discard-recovered-answers")
                                    .accessibility_id(
                                        REVISION_DISCARD_RECOVERED_ANSWERS_ACCESSIBILITY_ID,
                                    )
                                    .label(i18n::t(i18n::Key::RevisionDiscardRecoveredAnswers, cx))
                                    .small()
                                    .ghost()
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.discard_recovered_revision_answers(cx);
                                    })),
                            ),
                    )
                    .into_any_element(),
            );
        }

        let Some(review) = self.review_flow.review_result() else {
            if diagnostic.is_none() {
                content.push(
                    div()
                        .text_sm()
                        .text_color(cx.theme().muted_foreground)
                        .child(i18n::t(i18n::Key::ReviewNoResult, cx))
                        .into_any_element(),
                );
            }
            return v_flex()
                .id("review-panel")
                .size_full()
                .p(metrics::inset())
                .gap(metrics::gap_group())
                .overflow_y_scroll()
                .children(content)
                .into_any_element();
        };

        let stale = review.result.result.status.is_stale()
            || self.active_document().is_none_or(|document| {
                let document = document.read(cx);
                document.id() != review.document_id
                    || document.async_snapshot(cx) != review.source_snapshot
            })
            || !review.supporting_sources_current;
        let Some(output) = review.result.result.output.as_ref() else {
            content.push(
                div()
                    .text_sm()
                    .text_color(cx.theme().muted_foreground)
                    .child(i18n::t(i18n::Key::ReviewFailed, cx))
                    .into_any_element(),
            );
            return v_flex()
                .id("review-panel")
                .size_full()
                .p(metrics::inset())
                .gap(metrics::gap_group())
                .overflow_y_scroll()
                .children(content)
                .into_any_element();
        };
        let understanding = output.sections();
        if !stale {
            content.push(
                div()
                    .id("review-result")
                    .role(gpui_kit::Role::Label)
                    .aria_value(i18n::t(i18n::Key::ReviewReady, cx))
                    .accessibility_id(REVIEW_RESULT_ACCESSIBILITY_ID)
                    .text_sm()
                    .font_medium()
                    .child(i18n::t(i18n::Key::ReviewReady, cx))
                    .into_any_element(),
            );
        }
        if stale {
            content.push(
                div()
                    .text_xs()
                    .text_color(cx.theme().warning)
                    .child(i18n::t(i18n::Key::ReviewStale, cx))
                    .into_any_element(),
            );
        }
        if review.selection.is_some() {
            content.push(
                div()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child(i18n::t(i18n::Key::ReviewSelectionContextOmitted, cx))
                    .into_any_element(),
            );
        }
        if review.partial {
            content.push(
                div()
                    .text_xs()
                    .text_color(cx.theme().warning)
                    .child(i18n::t(i18n::Key::ReviewPartial, cx))
                    .into_any_element(),
            );
        }
        content.push(
            div()
                .text_xs()
                .text_color(cx.theme().muted_foreground)
                .child(format!(
                    "{} · {} {} · {} {} · {}",
                    review.result.metadata.provider().label(),
                    i18n::t(i18n::Key::ReviewRequestedModel, cx),
                    review.result.metadata.requested_model(),
                    i18n::t(i18n::Key::ReviewResponseModel, cx),
                    review.result.metadata.response_model(),
                    review.result.metadata.prompt_version(),
                ))
                .into_any_element(),
        );

        push_review_text_section(
            &mut content,
            i18n::t(i18n::Key::ReviewStatedGoal, cx),
            understanding.stated_goal.as_str(),
            cx,
        );
        push_review_text_list(
            &mut content,
            i18n::t(i18n::Key::ReviewContext, cx),
            &understanding.relevant_context,
            cx,
        );
        push_review_text_list(
            &mut content,
            i18n::t(i18n::Key::ReviewConstraints, cx),
            &understanding.constraints,
            cx,
        );
        push_review_text_list(
            &mut content,
            i18n::t(i18n::Key::ReviewNonGoals, cx),
            &understanding.non_goals,
            cx,
        );
        push_review_text_section(
            &mut content,
            i18n::t(i18n::Key::ReviewDeliverable, cx),
            understanding.expected_deliverable.as_str(),
            cx,
        );
        push_review_text_list(
            &mut content,
            i18n::t(i18n::Key::ReviewSuccessEvidence, cx),
            &understanding.success_evidence,
            cx,
        );
        push_review_text_list(
            &mut content,
            i18n::t(i18n::Key::ReviewAssumptions, cx),
            &understanding.inferred_assumptions,
            cx,
        );
        push_review_text_list(
            &mut content,
            i18n::t(i18n::Key::ReviewDecisions, cx),
            &understanding.unresolved_decisions,
            cx,
        );

        content.push(
            div()
                .text_xs()
                .font_medium()
                .text_color(cx.theme().muted_foreground)
                .child(i18n::t(i18n::Key::ReviewFindings, cx))
                .into_any_element(),
        );
        for (ix, finding) in output.findings.iter().enumerate() {
            let anchor = finding.anchor.clone();
            let anchor_label = review_anchor_label(&finding.anchor, cx);
            let kind = match finding.kind {
                FindingKind::Source | FindingKind::SourceStatement => {
                    i18n::t(i18n::Key::ReviewSourceStatement, cx)
                }
                FindingKind::Inference => i18n::t(i18n::Key::ReviewInference, cx),
            };
            let row = ListItem::new(SharedString::from(format!("review-finding-{ix}")))
                .w_full()
                .child(
                    v_flex()
                        .gap(metrics::gap())
                        .child(
                            h_flex()
                                .gap(metrics::gap())
                                .child(div().text_xs().font_medium().child(kind))
                                .child(
                                    div()
                                        .text_xs()
                                        .text_color(cx.theme().muted_foreground)
                                        .child(anchor_label),
                                ),
                        )
                        .child(div().text_sm().child(finding.text.as_str().to_owned())),
                );
            let row = row.on_click(cx.listener(move |this, _, window, cx| {
                this.reveal_review_anchor(&anchor, window, cx)
            }));
            content.push(row.into_any_element());
        }
        if output.findings.is_empty() {
            content.push(
                div()
                    .text_sm()
                    .text_color(cx.theme().muted_foreground)
                    .child(i18n::t(i18n::Key::ReviewNoFindings, cx))
                    .into_any_element(),
            );
        }

        content.push(
            div()
                .text_xs()
                .font_medium()
                .text_color(cx.theme().muted_foreground)
                .child(i18n::t(i18n::Key::ReviewQuestions, cx))
                .into_any_element(),
        );
        for (ix, question) in output.clarification_questions.iter().take(5).enumerate() {
            let impact = question
                .impact
                .as_ref()
                .map(|impact| impact.as_str().to_owned());
            content.push(
                ListItem::new(SharedString::from(format!("review-question-{ix}")))
                    .w_full()
                    .child(
                        v_flex()
                            .gap(metrics::gap())
                            .child(div().text_xs().font_medium().child(format!(
                                "{} {}",
                                i18n::t(i18n::Key::ReviewPriority, cx),
                                clarification_priority_label(question.priority, cx)
                            )))
                            .child(div().text_sm().child(question.question.as_str().to_owned()))
                            .when_some(impact, |this, impact| {
                                this.child(
                                    div()
                                        .text_xs()
                                        .text_color(cx.theme().muted_foreground)
                                        .child(impact),
                                )
                            }),
                    )
                    .into_any_element(),
            );
        }
        if output.clarification_questions.is_empty() {
            content.push(
                div()
                    .text_sm()
                    .text_color(cx.theme().muted_foreground)
                    .child(i18n::t(i18n::Key::ReviewNoQuestions, cx))
                    .into_any_element(),
            );
        }

        let active_document_id = self
            .active_document()
            .map(|document| document.read(cx).id());
        let revision_context = self.review_flow.revision_context().filter(|context| {
            Some(context.document_id) == active_document_id
                && self
                    .review_flow
                    .review_result_belongs_to(context.document_id)
        });
        if let Some(context) = revision_context {
            content.push(
                div()
                    .id("revision-answers")
                    .role(gpui::Role::Group)
                    .aria_label(i18n::t(i18n::Key::RevisionAnswers, cx))
                    .accessibility_id("markturbo-revision-answers")
                    .text_xs()
                    .font_medium()
                    .text_color(cx.theme().muted_foreground)
                    .child(i18n::t(i18n::Key::RevisionAnswers, cx))
                    .into_any_element(),
            );
            for (index, question) in output.clarification_questions.iter().enumerate() {
                let Some(input) = context.answer_inputs.get(index).cloned() else {
                    continue;
                };
                let state = context
                    .answer_states
                    .get(index)
                    .cloned()
                    .unwrap_or_else(RevisionAnswer::unanswered);
                let answered = matches!(state, RevisionAnswer::Answered(_));
                let unanswered_selected = matches!(state, RevisionAnswer::Unanswered);
                let unspecified_selected =
                    matches!(state, RevisionAnswer::IntentionallyUnspecified);
                let question_id = revision_question_accessibility_id(index, question);
                let question_text = question.question.as_str().to_owned();
                content.push(
                    v_flex()
                        .id(question_id.clone())
                        .accessibility_id(question_id.clone())
                        .gap(metrics::gap())
                        .child(div().text_sm().child(question_text.clone()))
                        .child(
                            h_flex()
                                .gap(metrics::gap())
                                .flex_wrap()
                                .child(
                                    Button::new(SharedString::from(format!(
                                        "{question_id}-unanswered"
                                    )))
                                    .label(i18n::t(i18n::Key::RevisionAnswerUnanswered, cx))
                                    .xsmall()
                                    .accessibility_id(SharedString::from(format!(
                                        "{question_id}-unanswered"
                                    )))
                                    .when(unanswered_selected, |button| button.primary())
                                    .when(!unanswered_selected, |button| button.ghost())
                                    .disabled(
                                        self.review_flow.is_revision_running()
                                            || self.review_flow.has_revision_result(),
                                    )
                                    .on_click(cx.listener(move |this, _, window, cx| {
                                        this.set_revision_answer_state(
                                            index,
                                            RevisionAnswer::unanswered(),
                                            window,
                                            cx,
                                        );
                                    })),
                                )
                                .child(
                                    Button::new(SharedString::from(format!(
                                        "{question_id}-unspecified"
                                    )))
                                    .label(i18n::t(
                                        i18n::Key::RevisionAnswerIntentionallyUnspecified,
                                        cx,
                                    ))
                                    .xsmall()
                                    .accessibility_id(SharedString::from(format!(
                                        "{question_id}-unspecified"
                                    )))
                                    .when(unspecified_selected, |button| button.primary())
                                    .when(!unspecified_selected, |button| button.ghost())
                                    .disabled(
                                        self.review_flow.is_revision_running()
                                            || self.review_flow.has_revision_result(),
                                    )
                                    .on_click(cx.listener(move |this, _, window, cx| {
                                        this.set_revision_answer_state(
                                            index,
                                            RevisionAnswer::intentionally_unspecified(),
                                            window,
                                            cx,
                                        );
                                    })),
                                )
                                .child(
                                    Button::new(SharedString::from(format!(
                                        "{question_id}-answered"
                                    )))
                                    .label(i18n::t(i18n::Key::RevisionAnswerAnswered, cx))
                                    .xsmall()
                                    .accessibility_id(SharedString::from(format!(
                                        "{question_id}-answered"
                                    )))
                                    .when(answered, |button| button.primary())
                                    .when(!answered, |button| button.ghost())
                                    .disabled(
                                        self.review_flow.is_revision_running()
                                            || self.review_flow.has_revision_result(),
                                    )
                                    .on_click(cx.listener(move |this, _, window, cx| {
                                        let value = this
                                            .review_flow
                                            .revision_context()
                                            .and_then(|context| context.answer_inputs.get(index))
                                            .map(|input| input.read(cx).value().to_string())
                                            .unwrap_or_default();
                                        this.set_revision_answer_state(
                                            index,
                                            RevisionAnswer::answered(value),
                                            window,
                                            cx,
                                        );
                                    })),
                                ),
                        )
                        .child(
                            Input::new(&input)
                                .small()
                                .w_full()
                                .aria_label(question_text)
                                .accessibility_id(SharedString::from(format!(
                                    "{question_id}-input"
                                )))
                                .disabled(
                                    !answered
                                        || self.review_flow.is_revision_running()
                                        || self.review_flow.has_revision_result(),
                                ),
                        )
                        .into_any_element(),
                );
            }
            if !self.review_flow.is_revision_running() && !self.review_flow.has_revision_result() {
                content.push(
                    h_flex()
                        .gap(metrics::gap())
                        .child(
                            Button::new("revision-run")
                                .accessibility_id(REVISION_RUN_ACCESSIBILITY_ID)
                                .label(i18n::t(i18n::Key::RevisionRun, cx))
                                .small()
                                .primary()
                                .disabled(stale)
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.start_revision(window, cx);
                                })),
                        )
                        .child(
                            Button::new("revision-discard-answers")
                                .accessibility_id(REVISION_DISCARD_ANSWERS_ACCESSIBILITY_ID)
                                .label(i18n::t(i18n::Key::RevisionDiscardAnswers, cx))
                                .small()
                                .ghost()
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.discard_revision_answers(cx);
                                })),
                        )
                        .into_any_element(),
                );
            }
        }
        if self.review_flow.is_revision_running() {
            content.push(
                h_flex()
                    .id("revision-running")
                    .items_center()
                    .gap(metrics::gap())
                    .child(Spinner::new().small())
                    .child(i18n::t(i18n::Key::RevisionRunning, cx))
                    .child(
                        Button::new("revision-cancel")
                            .icon(IconName::Close)
                            .xsmall()
                            .ghost()
                            .tooltip(i18n::t(i18n::Key::Cancel, cx))
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.on_cancel_revision(cx);
                            })),
                    )
                    .into_any_element(),
            );
        }
        if let Some(diagnostic) = self.review_flow.revision_diagnostic() {
            content.push(
                div()
                    .id("revision-diagnostic")
                    .accessibility_id("markturbo-revision-diagnostic")
                    .text_sm()
                    .text_color(cx.theme().warning)
                    .child(diagnostic.to_owned())
                    .into_any_element(),
            );
            if revision_context.is_some() && !self.review_flow.is_revision_running() {
                content.push(
                    h_flex()
                        .gap(metrics::gap())
                        .child(
                            Button::new("revision-retry")
                                .accessibility_id(REVISION_RETRY_ACCESSIBILITY_ID)
                                .label(i18n::t(i18n::Key::RevisionRetry, cx))
                                .small()
                                .primary()
                                .disabled(stale)
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.start_revision(window, cx);
                                })),
                        )
                        .child(
                            Button::new("revision-dismiss")
                                .accessibility_id(REVISION_DISMISS_ACCESSIBILITY_ID)
                                .label(i18n::t(i18n::Key::RevisionDismiss, cx))
                                .small()
                                .ghost()
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.dismiss_revision(cx);
                                })),
                        )
                        .into_any_element(),
                );
            }
        }
        let applied_awaiting_save = self.revision_applied_state_is_current_for_render(cx);
        if let Some(revision) = self.review_flow.revision_result() {
            let revision_stale = !self.revision_result_is_current(revision, cx);
            if revision_stale {
                content.push(
                    div()
                        .id("revision-stale")
                        .test_support()
                        .accessibility_id(REVISION_STALE_ACCESSIBILITY_ID)
                        .role(gpui::Role::Label)
                        .aria_label(i18n::t(i18n::Key::RevisionStaleInspection, cx))
                        .text_xs()
                        .text_color(cx.theme().warning)
                        .child(i18n::t(i18n::Key::RevisionStaleInspection, cx))
                        .into_any_element(),
                );
            }
            content.push(
                div()
                    .id("revision-result")
                    .role(gpui::Role::Group)
                    .aria_label(i18n::t(i18n::Key::RevisionResult, cx))
                    .accessibility_id("markturbo-revision-result")
                    .text_sm()
                    .font_medium()
                    .text_color(if revision_stale {
                        cx.theme().warning
                    } else {
                        cx.theme().foreground
                    })
                    .child(i18n::t(
                        if applied_awaiting_save {
                            i18n::Key::RevisionAppliedSavePending
                        } else if revision_stale {
                            i18n::Key::RevisionStaleInspection
                        } else {
                            i18n::Key::RevisionResult
                        },
                        cx,
                    ))
                    .into_any_element(),
            );
            content.push(
                h_flex()
                    .gap(metrics::gap())
                    .flex_wrap()
                    .child(
                        Button::new("revision-accept-all")
                            .accessibility_id(REVISION_ACCEPT_ALL_ACCESSIBILITY_ID)
                            .label(i18n::t(i18n::Key::RevisionAcceptAll, cx))
                            .small()
                            .primary()
                            .disabled(revision_stale)
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.set_all_revision_decisions(true, cx);
                            })),
                    )
                    .child(
                        Button::new("revision-reject-all")
                            .accessibility_id(REVISION_REJECT_ALL_ACCESSIBILITY_ID)
                            .label(i18n::t(i18n::Key::RevisionRejectAll, cx))
                            .small()
                            .ghost()
                            .disabled(revision_stale)
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.set_all_revision_decisions(false, cx);
                            })),
                    )
                    .child(
                        Button::new("revision-copy")
                            .accessibility_id(REVISION_COPY_ACCESSIBILITY_ID)
                            .label(i18n::t(i18n::Key::RevisionCopy, cx))
                            .small()
                            .outline()
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.copy_revision(cx);
                            })),
                    )
                    .child(
                        Button::new("revision-apply")
                            .accessibility_id(REVISION_APPLY_ACCESSIBILITY_ID)
                            .label(i18n::t(i18n::Key::RevisionApply, cx))
                            .small()
                            .primary()
                            .disabled(revision_stale)
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.apply_revision(window, cx);
                            })),
                    )
                    .child(
                        Button::new("revision-save")
                            .accessibility_id(REVISION_SAVE_ACCESSIBILITY_ID)
                            .label(i18n::t(i18n::Key::Save, cx))
                            .small()
                            .outline()
                            .disabled(!applied_awaiting_save)
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.save_revision(window, cx);
                            })),
                    )
                    .child(
                        Button::new("revision-save-as")
                            .accessibility_id(REVISION_SAVE_AS_ACCESSIBILITY_ID)
                            .label(i18n::t(i18n::Key::SaveAsPicker, cx))
                            .small()
                            .outline()
                            .disabled(!applied_awaiting_save)
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.save_as_revision(window, cx);
                            })),
                    )
                    .child(
                        Button::new("revision-dismiss-result")
                            .accessibility_id(REVISION_RESULT_DISMISS_ACCESSIBILITY_ID)
                            .label(i18n::t(i18n::Key::RevisionDismiss, cx))
                            .small()
                            .ghost()
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.dismiss_revision(cx);
                            })),
                    )
                    .into_any_element(),
            );
            let proposal = revision.result.proposal();
            for change_id in revision_change_ids(proposal) {
                let accepted = revision
                    .decisions
                    .iter()
                    .find(|(id, _)| *id == change_id)
                    .is_some_and(|(_, accepted)| *accepted);
                let change_hunks: Vec<_> = proposal
                    .hunks()
                    .iter()
                    .filter(|hunk| hunk.change_id() == change_id)
                    .collect();
                let rationale = change_hunks
                    .first()
                    .map(|hunk| hunk.rationale().to_owned())
                    .unwrap_or_default();
                let change_id_for_accept = change_id;
                let change_id_for_reject = change_id;
                let mut group =
                    v_flex()
                        .id(SharedString::from(format!(
                            "revision-change-{}",
                            change_id.0
                        )))
                        .role(gpui::Role::Group)
                        .aria_label(SharedString::from(format!(
                            "{} {}",
                            i18n::t(i18n::Key::RevisionChange, cx),
                            change_id.0
                        )))
                        .accessibility_id(SharedString::from(format!(
                            "markturbo-revision-change-{}",
                            change_id.0
                        )))
                        .gap(metrics::gap())
                        .border_1()
                        .border_color(cx.theme().border)
                        .p(metrics::inset())
                        .child(div().text_sm().font_medium().child(format!(
                            "{} {} · {}",
                            i18n::t(i18n::Key::RevisionChange, cx),
                            change_id.0,
                            i18n::t(
                                if accepted {
                                    i18n::Key::RevisionChangeAccepted
                                } else {
                                    i18n::Key::RevisionChangeRejected
                                },
                                cx,
                            )
                        )))
                        .child(div().text_xs().child(format!(
                            "{}: {rationale}",
                            i18n::t(i18n::Key::RevisionRationale, cx)
                        )))
                        .child(
                            h_flex()
                                .gap(metrics::gap())
                                .child(
                                    Button::new(SharedString::from(format!(
                                        "revision-change-{}-accept",
                                        change_id.0
                                    )))
                                    .label(i18n::t(i18n::Key::RevisionAccept, cx))
                                    .xsmall()
                                    .accessibility_id(SharedString::from(format!(
                                        "markturbo-revision-change-{}-accept",
                                        change_id.0
                                    )))
                                    .when(accepted, |button| button.primary())
                                    .when(!accepted, |button| button.ghost())
                                    .disabled(revision_stale)
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.set_revision_decision(change_id_for_accept, true, cx);
                                    })),
                                )
                                .child(
                                    Button::new(SharedString::from(format!(
                                        "revision-change-{}-reject",
                                        change_id.0
                                    )))
                                    .label(i18n::t(i18n::Key::RevisionReject, cx))
                                    .xsmall()
                                    .accessibility_id(SharedString::from(format!(
                                        "markturbo-revision-change-{}-reject",
                                        change_id.0
                                    )))
                                    .when(!accepted, |button| button.primary())
                                    .when(accepted, |button| button.ghost())
                                    .disabled(revision_stale)
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.set_revision_decision(change_id_for_reject, false, cx);
                                    })),
                                ),
                        );
                for hunk in change_hunks {
                    group = group.child(div().text_xs().child(format!(
                        "{}..{}: {} -> {}",
                        hunk.source().start,
                        hunk.source().end,
                        hunk.expected_source(),
                        hunk.replacement()
                    )));
                }
                content.push(group.into_any_element());
            }
            let preview: SharedString = self.revision_preview().unwrap_or_default().into();
            content.push(
                v_flex()
                    .id("revision-preview")
                    .role(gpui::Role::Group)
                    .aria_label(i18n::t(i18n::Key::RevisionFinalPreview, cx))
                    .accessibility_id("markturbo-revision-preview")
                    .gap(metrics::gap())
                    .child(
                        div()
                            .text_xs()
                            .font_medium()
                            .child(i18n::t(i18n::Key::RevisionFinalPreview, cx)),
                    )
                    .child(
                        div()
                            .id("revision-preview-source")
                            .role(gpui::Role::Label)
                            .aria_label(i18n::t(i18n::Key::RevisionFinalPreviewSource, cx))
                            .aria_value(preview.clone())
                            .accessibility_id("markturbo-revision-preview-source")
                            .text_xs()
                            .font_family("monospace")
                            .child(preview),
                    )
                    .into_any_element(),
            );
            content.push(
                div()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child(format!(
                        "{} {}",
                        revision.result.question_coverage().len(),
                        i18n::t(i18n::Key::RevisionCoverageValidated, cx)
                    ))
                    .into_any_element(),
            );
            for coverage in revision.result.question_coverage() {
                let status = match coverage.status() {
                    RevisionQuestionCoverageStatus::Represented { change_ids } => {
                        format!(
                            "{} {:?}",
                            i18n::t(i18n::Key::RevisionCoverageRepresented, cx),
                            change_ids
                        )
                    }
                    RevisionQuestionCoverageStatus::IntentionallyOmitted { reason } => {
                        format!(
                            "{}: {reason}",
                            i18n::t(i18n::Key::RevisionCoverageIntentionallyOmitted, cx)
                        )
                    }
                    RevisionQuestionCoverageStatus::NotAddressed => {
                        i18n::t(i18n::Key::RevisionCoverageNotAddressed, cx).to_owned()
                    }
                };
                content.push(
                    div()
                        .text_xs()
                        .child(format!(
                            "{} {}: {status}",
                            i18n::t(i18n::Key::RevisionCoverageQuestion, cx),
                            coverage.question_index()
                        ))
                        .into_any_element(),
                );
            }
        }

        v_flex()
            .id("review-panel")
            .size_full()
            .p(metrics::inset())
            .gap(metrics::gap_group())
            .overflow_y_scroll()
            .children(content)
            .into_any_element()
    }
}
