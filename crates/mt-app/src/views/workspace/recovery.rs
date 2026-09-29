//! App-owned recovery state and the editor/window transitions that protect it.
//!
//! `RecoveryStore` owns durable records and `DocumentView` owns editor bytes.
//! This flow owns the application protocol between them: startup arbitration,
//! the one checkpoint schedule/worker, retirement tickets, and the safe close
//! continuation after a durable Save or Discard marker.

use std::{
    collections::{HashMap, HashSet},
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

use gpui_kit::*;
use mt_core::document::lifecycle::{
    DestructiveAction, DestructiveRequest, DestructiveResolution, DocumentId,
};
use mt_core::model::RevisionRequestBinding;
use mt_core::recovery::{
    CancellableRecoveryCheckpointAttempt, CheckpointAttemptTiming, CheckpointBatchOutcome,
    CheckpointSchedule, RecoveredRecord, RecoveryError, RecoveryKey, RecoveryMaintenance,
    RecoveryRetirement, RecoveryRetirementBatch, RecoveryStore, RecoveryToken,
    RetirementCompletion, RevisionRecovery,
};
use mt_core::review::provider::RevisionAnswer;
use mt_core::workspace::tabs::TabIdentity;

use super::{PendingStartupDestructive, Workspace};
use crate::i18n;
use crate::views::document::{DocumentView, PreparedRecovery};

/// Sole owner of the app-side recovery state; the store and scheduler retain
/// their policy in `mt-core`.
#[derive(Default)]
pub(super) struct RecoveryFlow {
    pub(super) recovery: Option<RecoveryStore>,
    /// True until the startup recovery scan either completes or fails.
    pub(super) startup_recovery_pending: bool,
    /// Original keys for documents opened before startup recovery is ready.
    /// Save As observes the new path although the old checkpoint still belongs
    /// to the path that was open when startup began.
    pub(super) startup_recovery_keys: HashMap<DocumentId, RecoveryKey>,
    /// Dirty source keys that successful Save As operations must retire.
    pub(super) save_as_recovery_keys: HashMap<DocumentId, RecoveryKey>,
    /// Explicit Save or Discard decisions that still need a durable marker.
    /// `None` keeps unknown-origin work fail-closed for every destructive action.
    pub(super) pending_recovery_retirements: HashMap<RecoveryKey, Option<DocumentId>>,
    pub(super) recovery_retirements: HashMap<RecoveryKey, RecoveryRetirement>,
    pub(super) recovery_retirement_batches: HashMap<RecoveryKey, RecoveryRetirementBatch>,
    pub(super) recovery_retirement_retries: HashSet<RecoveryKey>,
    pub(super) recovery_schedules: HashMap<DocumentId, DocumentRecoveryState>,
    /// Content identities paused until retirement resolves, scoped to a
    /// document incarnation so reopening the same path is not suppressed.
    pub(super) recovery_retirement_suppressions:
        HashMap<RecoveryKey, HashMap<DocumentId, RecoveryContentIdentity>>,
    /// True while the one physical checkpoint batch owned by this workspace is running.
    pub(super) recovery_checkpoint_worker_active: bool,
    pub(super) recovery_warning: Option<String>,
    /// One wake-up for the earliest dirty-buffer checkpoint deadline.
    pub(super) _recovery_timer: Option<Task<()>>,
    /// A task can wake while being dropped, so replacement also bumps a token.
    pub(super) recovery_timer_generation: u64,
}

impl RecoveryFlow {
    pub(super) fn new() -> Self {
        Self {
            startup_recovery_pending: true,
            ..Self::default()
        }
    }
}

pub(super) struct DocumentRecoveryState {
    pub(super) key: RecoveryKey,
    pub(super) content_identity: RecoveryContentIdentity,
    pub(super) suppressed_oversized_revision: Option<RecoveryContentIdentity>,
    pub(super) token: Option<RecoveryToken>,
    pub(super) schedule: CheckpointSchedule,
    pub(super) in_flight: Option<RecoveryAttempt>,
    /// The current due boundary has already cancelled or warned while the
    /// physical workspace worker remains occupied.
    pub(super) deadline_reported: bool,
    pub(super) protection_warning: bool,
}

#[derive(Debug, Clone)]
pub(super) struct RecoveryAttempt {
    pub(super) token: RecoveryToken,
    pub(super) content_identity: RecoveryContentIdentity,
    pub(super) timing: CheckpointAttemptTiming,
    pub(super) cancelled: Arc<AtomicBool>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct RecoveryContentIdentity {
    pub(super) revision: u64,
    pub(super) revision_binding: Option<RevisionRequestBinding>,
}

impl RecoveryContentIdentity {
    pub(super) fn for_revision(revision: u64) -> Self {
        Self {
            revision,
            revision_binding: None,
        }
    }

    fn from_revision_recovery(revision: u64, revision_recovery: Option<&RevisionRecovery>) -> Self {
        Self {
            revision,
            revision_binding: revision_recovery.map(|recovery| *recovery.binding()),
        }
    }
}

impl RecoveryAttempt {
    fn cancel(&self) {
        self.cancelled.store(true, Ordering::Release);
    }
}

impl PartialEq for RecoveryAttempt {
    fn eq(&self, other: &Self) -> bool {
        self.token == other.token
            && self.content_identity == other.content_identity
            && self.timing == other.timing
            && Arc::ptr_eq(&self.cancelled, &other.cancelled)
    }
}

impl Eq for RecoveryAttempt {}

fn cancel_recovery_attempt(state: &mut DocumentRecoveryState) {
    let Some(attempt) = state.in_flight.as_ref() else {
        return;
    };
    attempt.cancel();
}

fn cancel_recovery_attempt_at(state: &mut DocumentRecoveryState, now: Instant) {
    let Some(attempt) = state.in_flight.take() else {
        return;
    };
    attempt.cancel();
    if now >= attempt.timing.durable_complete_by {
        state
            .schedule
            .checkpoint_deadline_missed(attempt.timing, now);
        state.protection_warning = true;
    } else {
        state.schedule.checkpoint_cancelled(attempt.timing, now);
    }
}

pub(super) fn current_checkpoint_write_completed_for_identity(
    attempt_is_current: bool,
    state_identity: RecoveryContentIdentity,
    attempt_identity: RecoveryContentIdentity,
    outcome: &CheckpointBatchOutcome,
) -> bool {
    attempt_is_current
        && state_identity == attempt_identity
        && matches!(outcome, CheckpointBatchOutcome::Written)
}

#[derive(Default)]
pub(super) struct StartupRecovery {
    pub(super) recovery: Option<RecoveryStore>,
    pub(super) documents: Vec<PreparedRecovery>,
    pub(super) recovery_issue_count: usize,
    pub(super) recovery_error: Option<String>,
}

fn insert_scoped_recovery_key(
    keys: &mut HashMap<RecoveryKey, Option<DocumentId>>,
    key: RecoveryKey,
    document_id: Option<DocumentId>,
) -> Option<DocumentId> {
    *keys
        .entry(key)
        .and_modify(|current| {
            if *current != document_id {
                *current = None;
            }
        })
        .or_insert(document_id)
}

pub(super) fn prepare_recovery_records(
    records: Vec<RecoveredRecord>,
) -> (Vec<PreparedRecovery>, usize) {
    let mut documents = Vec::with_capacity(records.len());
    let mut skipped = 0;
    for recovered in records {
        match DocumentView::prepare_recovery(recovered) {
            Ok(document) => documents.push(document),
            Err(_) => skipped += 1,
        }
    }
    (documents, skipped)
}

/// Open, verify, and parse recovery data without making it a prerequisite for editing.
pub(super) fn startup_recovery() -> StartupRecovery {
    #[cfg(not(test))]
    {
        match RecoveryStore::open() {
            Ok((store, maintenance)) => match store.recover() {
                Ok(scan) => {
                    let scan_issues = scan.issues.len();
                    let (documents, preparation_issues) = prepare_recovery_records(scan.records);
                    StartupRecovery {
                        recovery: Some(store),
                        documents,
                        recovery_issue_count: maintenance.issues.len()
                            + scan_issues
                            + preparation_issues,
                        recovery_error: None,
                    }
                }
                Err(error) => StartupRecovery {
                    recovery: Some(store),
                    recovery_issue_count: maintenance.issues.len(),
                    recovery_error: Some(error.to_string()),
                    ..StartupRecovery::default()
                },
            },
            Err(error) => StartupRecovery {
                recovery_error: Some(error.to_string()),
                ..StartupRecovery::default()
            },
        }
    }
    #[cfg(test)]
    {
        // Tests install an explicit reversible protector when they need durable
        // records; opening production DPAPI storage would make test state leak
        // across runs and hide which records a test owns.
        StartupRecovery::default()
    }
}

pub(super) fn startup_recovery_status(
    restored: usize,
    skipped: usize,
    recovery_error: Option<&str>,
) -> Option<String> {
    let summary = (restored > 0 || skipped > 0).then(|| {
        format!(
            "Restored {restored} recovery checkpoint(s); skipped {skipped} unavailable or invalid record(s)."
        )
    });
    match (recovery_error, summary) {
        (Some(error), Some(summary)) => {
            Some(format!("{error}. Editing remains available. {summary}"))
        }
        (Some(error), None) => Some(format!("{error}. Editing remains available.")),
        (None, Some(summary)) => Some(summary),
        (None, None) => None,
    }
}

pub(super) fn checkpoint_batch_status(
    maintenance_issues: usize,
    last_error: Option<&str>,
) -> Option<String> {
    let maintenance = (maintenance_issues > 0).then(|| {
        format!(
            "Recovery skipped {maintenance_issues} malformed, oversized, expired, or unreadable record(s)."
        )
    });
    match (last_error, maintenance) {
        (Some(error), Some(maintenance)) => Some(format!(
            "{error}. Editing and source files are unchanged. {maintenance}"
        )),
        (Some(error), None) => Some(format!("{error}. Editing and source files are unchanged.")),
        (None, Some(maintenance)) => Some(maintenance),
        (None, None) => None,
    }
}

impl Workspace {
    pub(super) fn is_startup_recovery_pending(&self) -> bool {
        self.recovery_flow.startup_recovery_pending
    }

    pub(super) fn startup_recovery_key(&self, document_id: DocumentId) -> Option<&RecoveryKey> {
        self.recovery_flow.startup_recovery_keys.get(&document_id)
    }

    pub(super) fn remember_startup_recovery_key(
        &mut self,
        document: &Entity<DocumentView>,
        cx: &Context<Self>,
    ) {
        if !self.recovery_flow.startup_recovery_pending {
            return;
        }
        let document = document.read(cx);
        let document_id = document.id();
        let key = document.recovery_key();
        self.recovery_flow
            .startup_recovery_keys
            .entry(document_id)
            .or_insert(key);
    }

    pub(super) fn remember_save_as_recovery_key(
        &mut self,
        document_id: DocumentId,
        key: RecoveryKey,
    ) {
        self.recovery_flow
            .save_as_recovery_keys
            .insert(document_id, key);
    }

    pub(super) fn should_preserve_unreconciled_startup_recovery(
        &self,
        document_id: DocumentId,
        source_was_dirty: bool,
        source_was_conflicted: bool,
    ) -> bool {
        if source_was_dirty
            || source_was_conflicted
            || !self.recovery_flow.startup_recovery_pending
            || self.recovery_flow.recovery.is_some()
            || self
                .recovery_flow
                .recovery_schedules
                .contains_key(&document_id)
            || self
                .recovery_flow
                .save_as_recovery_keys
                .contains_key(&document_id)
        {
            return false;
        }
        let Some(key) = self.recovery_flow.startup_recovery_keys.get(&document_id) else {
            return false;
        };
        !self
            .recovery_flow
            .pending_recovery_retirements
            .contains_key(key)
            && !self.recovery_flow.recovery_retirements.contains_key(key)
            && !self
                .recovery_flow
                .recovery_retirement_batches
                .contains_key(key)
    }

    pub(super) fn forget_save_as_recovery_key(&mut self, document_id: DocumentId) {
        self.recovery_flow
            .save_as_recovery_keys
            .remove(&document_id);
    }

    /// Select the key this Save or Discard resolves while retaining an
    /// independent startup key for recovery reconciliation.
    pub(super) fn take_recovery_key_for_retirement(
        &mut self,
        document_id: DocumentId,
        current_key: RecoveryKey,
    ) -> RecoveryKey {
        let save_as_key = self
            .recovery_flow
            .save_as_recovery_keys
            .remove(&document_id);
        let preserve_startup_key = {
            let retirement_key = save_as_key.as_ref().unwrap_or(&current_key);
            self.recovery_flow
                .recovery_schedules
                .get(&document_id)
                .is_some_and(|state| {
                    state.key == *retirement_key
                        && self
                            .recovery_flow
                            .startup_recovery_keys
                            .get(&document_id)
                            .is_some_and(|startup_key| startup_key != retirement_key)
                })
        };
        if !preserve_startup_key {
            self.recovery_flow
                .startup_recovery_keys
                .remove(&document_id);
        }
        save_as_key.unwrap_or(current_key)
    }

    pub(super) fn is_undurable_recovery_retirement(&self, key: &RecoveryKey) -> bool {
        self.recovery_flow
            .pending_recovery_retirements
            .contains_key(key)
    }

    pub(super) fn has_durable_recovery_retirement(&self, key: &RecoveryKey) -> bool {
        self.recovery_flow.recovery_retirements.contains_key(key)
            || self
                .recovery_flow
                .recovery_retirement_batches
                .contains_key(key)
    }

    pub(super) fn recovery_warning(&self) -> Option<&str> {
        self.recovery_flow.recovery_warning.as_deref()
    }

    pub(super) fn note_document_edited(
        &mut self,
        document: &Entity<DocumentView>,
        cx: &mut Context<Self>,
    ) {
        let key = document.read(cx).recovery_key();
        self.recovery_flow.pending_recovery_retirements.remove(&key);
        self.arm_document_recovery(document, cx);
    }

    pub(super) fn retire_clean_document_recovery(
        &mut self,
        document_id: DocumentId,
        current_key: RecoveryKey,
        cx: &mut Context<Self>,
    ) -> Option<RecoveryKey> {
        if self.should_preserve_unreconciled_startup_recovery(document_id, false, false) {
            return None;
        }
        let key = self.take_recovery_key_for_retirement(document_id, current_key);
        self.retire_document_recovery(document_id, Some(key), cx)
    }

    pub(super) fn begin_revision_recovery_retirement(
        &mut self,
        document_id: DocumentId,
        key: RecoveryKey,
        recovery: RevisionRecovery,
        cx: &mut Context<Self>,
    ) -> bool {
        let scheduled_key_matches = self
            .recovery_flow
            .recovery_schedules
            .get(&document_id)
            .is_some_and(|state| state.key == key);
        if scheduled_key_matches {
            self.retire_document_recovery(document_id, Some(key.clone()), cx);
        } else {
            self.invalidate_recovery(&key, Some(document_id), cx);
        }
        if self.has_durable_recovery_retirement(&key) {
            self.review_flow.remove_recovered_revision_record(&key);
            // This retires quarantined answers, not the current editor or its live answers.
            self.release_recovery_retirement_suppressions(std::iter::once(&key), cx);
            self.review_flow
                .stage_revision_recovery_retirement(key, document_id, recovery);
            true
        } else {
            false
        }
    }

    /// Capture the typed ReviewFlow answer binding together with the editor
    /// revision; answer-only edits therefore invalidate checkpoint identity.
    fn revision_recovery_snapshot(
        &self,
        document_id: DocumentId,
        key: &RecoveryKey,
        revision: u64,
    ) -> (RecoveryContentIdentity, Option<RevisionRecovery>) {
        let revision_recovery = if self
            .review_flow
            .revision_context_has_authored_answers_for_document(document_id)
        {
            self.review_flow
                .revision_recovery_for_document(document_id, key)
        } else {
            self.review_flow
                .recovered_revision_record_ref_for_document(document_id, key)
                .and_then(|(record_key, recovery)| (record_key == key).then(|| recovery.clone()))
        };
        let identity =
            RecoveryContentIdentity::from_revision_recovery(revision, revision_recovery.as_ref());
        (identity, revision_recovery)
    }

    fn recovery_key_needs_isolation(
        &self,
        document_id: DocumentId,
        key: &RecoveryKey,
        revision: u64,
        live_recovery: Option<&RevisionRecovery>,
    ) -> bool {
        let Some((record_key, recovered)) = self
            .review_flow
            .recovered_revision_record_ref_for_document(document_id, key)
        else {
            return false;
        };
        if record_key != key {
            return false;
        }
        if !recovered
            .answers()
            .as_slice()
            .iter()
            .any(|answer| !matches!(answer, RevisionAnswer::Unanswered))
        {
            return false;
        }

        if self
            .review_flow
            .revision_context_has_authored_answers_for_document(document_id)
        {
            return live_recovery.is_none_or(|live| live.binding() != recovered.binding());
        }

        self.recovery_flow
            .recovery_schedules
            .get(&document_id)
            .is_some_and(|state| state.key == *key && state.content_identity.revision != revision)
    }

    pub(super) fn defer_startup_recovery<F>(
        &mut self,
        load_startup_recovery: F,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) where
        F: FnOnce() -> StartupRecovery + Send + 'static,
    {
        let startup_targets = self.startup_recovery_targets(cx);
        cx.defer_in(window, move |this, window, cx| {
            this.start_startup_recovery(load_startup_recovery, startup_targets, window, cx);
        });
    }

    fn start_startup_recovery<F>(
        &mut self,
        load_startup_recovery: F,
        startup_targets: HashMap<PathBuf, (DocumentId, u64, u64)>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) where
        F: FnOnce() -> StartupRecovery + Send + 'static,
    {
        let task = cx.spawn_in(window, async move |this, cx| {
            let startup = cx
                .background_spawn(async move { load_startup_recovery() })
                .await;
            let startup = Arc::new(Mutex::new(Some((startup, startup_targets))));
            loop {
                let startup = startup.clone();
                if crate::views::try_update_in(&this, cx, move |this, window, cx| {
                    let startup = startup
                        .lock()
                        .unwrap_or_else(|poisoned| poisoned.into_inner())
                        .take();
                    if let Some((startup, startup_targets)) = startup {
                        this.restore_startup_recovery(startup, startup_targets, window, cx);
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
        });
        self._tasks.push(task);
    }

    pub(super) fn startup_recovery_targets(
        &self,
        cx: &App,
    ) -> HashMap<PathBuf, (DocumentId, u64, u64)> {
        self.tabs
            .iter()
            .filter_map(|tab| {
                let document = tab.payload.view.read(cx);
                let (revision, source_generation) = document.source_stamp();
                tab.path().map(|path| {
                    (
                        path.to_path_buf(),
                        (document.id(), revision, source_generation),
                    )
                })
            })
            .collect()
    }

    fn isolate_live_document_recovery_key(
        &mut self,
        document: &Entity<DocumentView>,
        document_id: DocumentId,
        recovered_key: &RecoveryKey,
        revision: u64,
        cx: &mut Context<Self>,
    ) {
        let (current_key, dirty) = {
            let document = document.read(cx);
            (document.recovery_key(), document.is_dirty())
        };
        if current_key != *recovered_key {
            return;
        }

        let live_answers = self
            .review_flow
            .revision_context_has_authored_answers_for_document(document_id);
        let needs_schedule = dirty || live_answers;
        let now = cx.background_executor().now();
        let store = self.recovery_flow.recovery.clone();
        let mut retained_key_warning = store
            .as_ref()
            .is_some_and(|store| store.activate_and_current_token(recovered_key).1);
        let new_key = document.update(cx, |document, _| document.rotate_recovery_key());
        let content_identity = self
            .revision_recovery_snapshot(document_id, &new_key, revision)
            .0;

        let Some(mut state) = self.recovery_flow.recovery_schedules.remove(&document_id) else {
            if needs_schedule {
                self.arm_document_recovery_at(document, now, cx);
                if retained_key_warning
                    && let Some(state) = self.recovery_flow.recovery_schedules.get_mut(&document_id)
                {
                    state.protection_warning = true;
                    self.refresh_recovery_warning(cx);
                }
            } else {
                self.schedule_recovery_timer(cx);
                self.refresh_recovery_warning(cx);
            }
            return;
        };

        cancel_recovery_attempt_at(&mut state, now);
        if state.key != *recovered_key
            && let Some(store) = &store
        {
            retained_key_warning |= store.activate_and_current_token(&state.key).1;
        }

        if needs_schedule {
            let (token, protection_warning) = store.as_ref().map_or((None, false), |store| {
                let (token, deferred) = store.activate_and_current_token(&new_key);
                (Some(token), deferred)
            });
            if state.content_identity != content_identity {
                if state.suppressed_oversized_revision.take().is_some() {
                    state.schedule = CheckpointSchedule::default();
                }
                state.content_identity = content_identity;
            }
            state.key = new_key;
            state.token = token;
            state.in_flight = None;
            state.protection_warning |= retained_key_warning || protection_warning;
            state.schedule.mark_dirty(now);
            self.recovery_flow
                .recovery_schedules
                .insert(document_id, state);
        }
        self.schedule_recovery_timer(cx);
        self.refresh_recovery_warning(cx);
    }

    pub(super) fn restore_prepared_recovery(
        &mut self,
        documents: Vec<PreparedRecovery>,
        startup_targets: Option<&HashMap<PathBuf, (DocumentId, u64, u64)>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> (usize, usize) {
        let mut duplicate_source_paths = HashSet::new();
        for (index, document) in documents.iter().enumerate() {
            let Some(path) = document.source_path() else {
                continue;
            };
            if documents[index + 1..]
                .iter()
                .any(|candidate| candidate.source_path() == Some(path))
            {
                duplicate_source_paths.insert(path.to_path_buf());
            }
        }

        let mut restored = 0;
        let mut skipped = 0;
        for mut prepared in documents {
            let source_path = prepared.source_path().map(Path::to_path_buf);
            let recovery_key = prepared.recovery_key();
            let revision_recovery = prepared.take_revision_recovery();
            let mut recovered_only = source_path
                .as_deref()
                .is_some_and(|path| duplicate_source_paths.contains(path));
            let mut recovered_from_document_id = None;
            if let Some(path) = &source_path
                && let Some(ix) = self.tabs.index_of(path)
            {
                let document = self
                    .document_at(ix)
                    .cloned()
                    .expect("an indexed recovery path must have a document");
                let (live_document_id, live_recovery_key, live_source_generation) = {
                    let document = document.read(cx);
                    prepared.mark_conflicted_if_source_changed(document);
                    (
                        document.id(),
                        document.recovery_key(),
                        document.source_stamp().1,
                    )
                };
                let expected = startup_targets.and_then(|targets| targets.get(path).copied());
                let source_generation_changed = expected
                    .map_or(live_source_generation != 0, |target| {
                        live_source_generation != target.2
                    });
                recovered_only |= live_recovery_key != recovery_key || source_generation_changed;

                if !recovered_only {
                    let Some(startup_targets) = startup_targets else {
                        skipped += 1;
                        continue;
                    };
                    let expected = startup_targets.get(path).copied();
                    let mut prepared_to_apply = Some(prepared);
                    let applied = document.update(cx, |document, cx| {
                        if !document.can_accept_startup_recovery(expected) {
                            return false;
                        }
                        document.apply_startup_recovery(
                            prepared_to_apply
                                .take()
                                .expect("accepted recovery retains its prepared record"),
                            window,
                            cx,
                        );
                        true
                    });
                    if applied {
                        self.register_restored_recovery(&document, cx);
                        restored += 1;
                        if let Some(revision_recovery) = revision_recovery {
                            self.review_flow.store_recovered_revision_record(
                                document.read(cx).id(),
                                recovery_key,
                                revision_recovery,
                            );
                        }
                        continue;
                    }
                    prepared =
                        prepared_to_apply.expect("rejected recovery retains its prepared record");
                    recovered_only = true;
                }

                if recovered_only {
                    if live_recovery_key == recovery_key {
                        let live_revision = document.read(cx).revision();
                        self.isolate_live_document_recovery_key(
                            &document,
                            live_document_id,
                            &recovery_key,
                            live_revision,
                            cx,
                        );
                    }
                    recovered_from_document_id = Some(live_document_id);
                }
            }

            let registry = self.registry.clone();
            let view = cx.new(|cx| DocumentView::from_recovery(prepared, registry, window, cx));
            if let Some(path) = source_path {
                self.insert_document_with_recovery(
                    if recovered_only {
                        TabIdentity::Recovered(recovery_key.clone())
                    } else {
                        TabIdentity::File(path)
                    },
                    view.clone(),
                    false,
                    window,
                    cx,
                );
            } else {
                self.insert_memory_document(view.clone(), false, window, cx);
            }
            let recovered_document_id = view.read(cx).id();
            if let Some(live_document_id) = recovered_from_document_id {
                self.review_flow.move_recovered_revision_document(
                    live_document_id,
                    recovered_document_id,
                    &recovery_key,
                );
            }
            self.register_restored_recovery(&view, cx);
            if let Some(revision_recovery) = revision_recovery {
                self.review_flow.store_recovered_revision_record(
                    view.read(cx).id(),
                    recovery_key,
                    revision_recovery,
                );
            }
            restored += 1;
        }
        (restored, skipped)
    }

    fn register_restored_recovery(
        &mut self,
        document: &Entity<DocumentView>,
        cx: &mut Context<Self>,
    ) {
        let Some(store) = self.recovery_flow.recovery.clone() else {
            return;
        };
        let (id, key, revision) = {
            let document = document.read(cx);
            (document.id(), document.recovery_key(), document.revision())
        };
        let (token, protection_warning) = store.activate_and_current_token(&key);
        let mut schedule = CheckpointSchedule::default();
        schedule.mark_durable_baseline(cx.background_executor().now());
        self.recovery_flow.recovery_schedules.insert(
            id,
            DocumentRecoveryState {
                key,
                content_identity: RecoveryContentIdentity::for_revision(revision),
                suppressed_oversized_revision: None,
                token: Some(token),
                schedule,
                in_flight: None,
                deadline_reported: false,
                protection_warning,
            },
        );
        self.schedule_recovery_timer(cx);
        self.refresh_recovery_warning(cx);
    }

    pub(super) fn resume_recovery_destructive(
        &mut self,
        mut pending: PendingStartupDestructive,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match pending.request.revalidate(&self.lifecycle_documents(cx)) {
            DestructiveResolution::Prompt(_) => {
                self.release_recovery_retirement_suppressions(
                    pending.keys.iter().map(|(key, _)| key),
                    cx,
                );
                self.rearm_dirty_recovery(cx);
                self.pending_destructive_recovery.extend(pending.keys);
                self.pending_destructive = Some(pending.request);
                self.prompt_destructive(window, cx);
            }
            DestructiveResolution::Proceed(action) => {
                if self.destructive_action_has_revision_answers(&action, cx) {
                    self.release_recovery_retirement_suppressions(
                        pending.keys.iter().map(|(key, _)| key),
                        cx,
                    );
                    self.rearm_dirty_recovery(cx);
                    self.set_status(i18n::t(i18n::Key::RevisionAnswersRetained, cx).into(), cx);
                    return;
                }
                self.perform_after_discard_retirement(
                    pending.request,
                    action,
                    pending.keys,
                    window,
                    cx,
                );
            }
            DestructiveResolution::Cancelled | DestructiveResolution::SaveFailed(_) => {
                self.release_recovery_retirement_suppressions(
                    pending.keys.iter().map(|(key, _)| key),
                    cx,
                );
                self.rearm_dirty_recovery(cx);
            }
        }
    }

    pub(super) fn restore_startup_recovery(
        &mut self,
        mut startup: StartupRecovery,
        startup_targets: HashMap<PathBuf, (DocumentId, u64, u64)>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        startup.documents.retain(|document| {
            !self
                .recovery_flow
                .pending_recovery_retirements
                .contains_key(&document.recovery_key())
        });
        self.recovery_flow.recovery = startup.recovery.take();
        self.recovery_flow.startup_recovery_pending = false;
        self.recovery_flow.startup_recovery_keys.clear();
        let pending_startup_destructive = self.pending_startup_destructive.take();
        let (restored, restore_skipped) =
            self.restore_prepared_recovery(startup.documents, Some(&startup_targets), window, cx);
        if let Some(status) = startup_recovery_status(
            restored,
            startup.recovery_issue_count + restore_skipped,
            startup.recovery_error.as_deref(),
        ) {
            self.set_status(status, cx);
        }
        debug_assert!(!self.recovery_flow.startup_recovery_pending);
        log::debug!("recovery startup finished");
        if self.recovery_flow.recovery.is_none() {
            if pending_startup_destructive.is_some()
                || !self.recovery_flow.pending_recovery_retirements.is_empty()
            {
                self.set_status(
                    "Recovery storage is unavailable, so its checkpoint could not be cleared. The document remains open."
                        .into(),
                    cx,
                );
            }
            return;
        }

        if let Some(pending) = pending_startup_destructive {
            self.flush_pending_recovery_retirements_except(&pending.keys, cx);
            self.resume_recovery_destructive(pending, window, cx);
        } else {
            self.flush_pending_recovery_retirements_except(&[], cx);
        }
        let dirty_documents: Vec<_> = self
            .document_views()
            .into_iter()
            .filter(|document| {
                let document = document.read(cx);
                let key = document.recovery_key();
                document.is_dirty()
                    && !self
                        .recovery_flow
                        .pending_recovery_retirements
                        .contains_key(&key)
                    && !self.recovery_flow.recovery_retirements.contains_key(&key)
                    && !self
                        .recovery_flow
                        .recovery_retirement_batches
                        .contains_key(&key)
                    && self
                        .recovery_flow
                        .recovery_schedules
                        .get(&document.id())
                        .is_none_or(|state| state.token.is_none())
            })
            .collect();
        for document in dirty_documents {
            self.arm_document_recovery(&document, cx);
        }
    }

    fn flush_pending_recovery_retirements_except(
        &mut self,
        action_keys: &[(RecoveryKey, Option<DocumentId>)],
        cx: &mut Context<Self>,
    ) {
        let pending: Vec<_> = self
            .recovery_flow
            .pending_recovery_retirements
            .iter()
            .filter(|(key, _)| !action_keys.iter().any(|(action_key, _)| action_key == *key))
            .map(|(key, document_id)| (key.clone(), *document_id))
            .collect();
        for (key, document_id) in pending {
            self.invalidate_recovery(&key, document_id, cx);
        }
    }

    pub(super) fn pending_recovery_keys(
        &self,
        action: &DestructiveAction,
    ) -> Vec<(RecoveryKey, Option<DocumentId>)> {
        self.recovery_flow
            .pending_recovery_retirements
            .iter()
            .filter_map(|(key, document_id)| match action {
                DestructiveAction::CloseTab(id) => (document_id.is_none()
                    || *document_id == Some(*id))
                .then(|| (key.clone(), *document_id)),
                DestructiveAction::CloseWindow | DestructiveAction::ReplaceWorkspace(_) => {
                    Some((key.clone(), *document_id))
                }
            })
            .collect()
    }

    fn recovery_content_retirement_suppressed(
        &self,
        key: &RecoveryKey,
        document_id: DocumentId,
        content_identity: &RecoveryContentIdentity,
    ) -> bool {
        self.recovery_flow
            .recovery_retirement_suppressions
            .get(key)
            .and_then(|documents| documents.get(&document_id))
            == Some(content_identity)
    }

    fn destructive_retirement_waits_for_key(&self, key: &RecoveryKey) -> bool {
        self.pending_startup_destructive
            .as_ref()
            .is_some_and(|pending| {
                pending
                    .keys
                    .iter()
                    .any(|(pending_key, _)| pending_key == key)
            })
    }

    fn recovery_content_identity_for_retirement(
        &self,
        key: &RecoveryKey,
        document_id: Option<DocumentId>,
        cx: &App,
    ) -> Option<(DocumentId, RecoveryContentIdentity)> {
        if let Some(document_id) = document_id {
            if let Some(document) = self.document_by_id(document_id, cx) {
                let document = document.read(cx);
                if document.recovery_key() == *key {
                    return Some((
                        document_id,
                        self.revision_recovery_snapshot(document_id, key, document.revision())
                            .0,
                    ));
                }
            }
            if let Some(state) = self
                .recovery_flow
                .recovery_schedules
                .get(&document_id)
                .filter(|state| state.key == *key)
            {
                return Some((document_id, state.content_identity));
            }
            return None;
        }
        if let Some((document_id, state)) = self
            .recovery_flow
            .recovery_schedules
            .iter()
            .find(|(_, state)| state.key == *key)
        {
            return Some((*document_id, state.content_identity));
        }
        self.tabs.iter().find_map(|tab| {
            let document = tab.payload.view.read(cx);
            if document.recovery_key() != *key {
                return None;
            }
            let document_id = document.id();
            Some((
                document_id,
                self.revision_recovery_snapshot(document_id, key, document.revision())
                    .0,
            ))
        })
    }

    fn release_recovery_retirement_suppressions<'a>(
        &mut self,
        keys: impl IntoIterator<Item = &'a RecoveryKey>,
        cx: &mut Context<Self>,
    ) {
        let mut changed = false;
        for key in keys {
            changed |= self
                .recovery_flow
                .recovery_retirement_suppressions
                .remove(key)
                .is_some();
        }
        if changed {
            self.schedule_recovery_timer(cx);
        }
    }

    pub(super) fn perform_after_discard_retirement(
        &mut self,
        request: DestructiveRequest,
        action: DestructiveAction,
        mut scoped_keys: Vec<(RecoveryKey, Option<DocumentId>)>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        scoped_keys.extend(self.pending_recovery_keys(&action));
        let mut merged = HashMap::new();
        for (key, document_id) in scoped_keys {
            insert_scoped_recovery_key(&mut merged, key, document_id);
        }
        let scoped_keys: Vec<_> = merged.into_iter().collect();
        let keys: Vec<_> = scoped_keys.iter().map(|(key, _)| key.clone()).collect();
        if keys.iter().any(|key| {
            self.recovery_flow
                .pending_recovery_retirements
                .contains_key(key)
                && (self.recovery_flow.recovery_retirements.contains_key(key)
                    || self
                        .recovery_flow
                        .recovery_retirement_batches
                        .contains_key(key))
        }) {
            self.schedule_destructive_retirement_continuation(
                PendingStartupDestructive {
                    request,
                    keys: scoped_keys,
                },
                window,
                cx,
            );
            return;
        }
        let dirty_keys: HashSet<_> = self
            .document_views()
            .into_iter()
            .filter_map(|document| {
                let document = document.read(cx);
                document.is_dirty().then(|| document.recovery_key())
            })
            .collect();
        if keys.iter().any(|key| {
            dirty_keys.contains(key)
                && (self.recovery_flow.recovery_retirements.contains_key(key)
                    || self
                        .recovery_flow
                        .recovery_retirement_batches
                        .contains_key(key))
        }) {
            self.schedule_destructive_retirement_continuation(
                PendingStartupDestructive {
                    request,
                    keys: scoped_keys,
                },
                window,
                cx,
            );
            return;
        }
        let scoped_keys: Vec<_> = scoped_keys
            .into_iter()
            .filter(|(key, _)| {
                !self.recovery_flow.recovery_retirements.contains_key(key)
                    && !self
                        .recovery_flow
                        .recovery_retirement_batches
                        .contains_key(key)
            })
            .collect();
        let keys: Vec<_> = scoped_keys.iter().map(|(key, _)| key.clone()).collect();
        if keys
            .iter()
            .any(|key| self.recovery_flow.recovery_retirement_retries.contains(key))
        {
            self.schedule_destructive_retirement_continuation(
                PendingStartupDestructive {
                    request,
                    keys: scoped_keys,
                },
                window,
                cx,
            );
            return;
        }
        if keys.is_empty() {
            self.perform_destructive(action, window, cx);
            return;
        }
        let Some(store) = self.recovery_flow.recovery.clone() else {
            for (key, document_id) in &scoped_keys {
                insert_scoped_recovery_key(
                    &mut self.recovery_flow.pending_recovery_retirements,
                    key.clone(),
                    *document_id,
                );
                self.remove_recovery_state_for_key(key, cx);
            }
            if self.recovery_flow.startup_recovery_pending {
                self.pending_startup_destructive = Some(PendingStartupDestructive {
                    request,
                    keys: scoped_keys,
                });
                self.set_status(
                    "Waiting for recovery storage to clear its checkpoint. The document remains open."
                        .into(),
                    cx,
                );
            } else {
                self.set_status(
                    "Recovery storage is unavailable, so its checkpoint could not be cleared. The document remains open."
                        .into(),
                    cx,
                );
            }
            return;
        };

        for (key, document_id) in &scoped_keys {
            insert_scoped_recovery_key(
                &mut self.recovery_flow.pending_recovery_retirements,
                key.clone(),
                *document_id,
            );
        }
        let now = cx.background_executor().now();
        for key in &keys {
            self.cancel_recovery_attempts_for_key(key, now);
        }
        let batch = match store.begin_retirements(keys.iter().cloned()) {
            Ok(batch) => batch,
            Err(error) => {
                self.rearm_dirty_recovery(cx);
                self.set_status(
                    format!(
                        "Could not clear the recovery checkpoint: {error}. The document remains open."
                    ),
                    cx,
                );
                return;
            }
        };
        for key in &keys {
            self.recovery_flow.pending_recovery_retirements.remove(key);
            self.recovery_flow
                .recovery_retirement_batches
                .insert(key.clone(), batch.clone());
        }
        // The marker is durable now; pause only the current content identity.
        for (key, document_id) in &scoped_keys {
            if let Some((document_id, content_identity)) =
                self.recovery_content_identity_for_retirement(key, *document_id, cx)
            {
                self.recovery_flow
                    .recovery_retirement_suppressions
                    .entry(key.clone())
                    .or_default()
                    .insert(document_id, content_identity);
            }
        }
        self.pending_destructive = Some(request);
        self.schedule_recovery_timer(cx);
        self.refresh_recovery_warning(cx);

        cx.spawn_in(window, async move |this, cx| {
            let completed_batch = batch.clone();
            let completed = cx
                .background_spawn(async move {
                    let result = store.complete_retirements(batch.clone());
                    if result.is_err() {
                        store.abandon_retirements(&batch);
                    }
                    result
                })
                .await;
            let completed = Arc::new(Mutex::new(Some((scoped_keys, completed_batch, completed))));
            loop {
                let completed_for_update = completed.clone();
                if crate::views::try_update_in(&this, cx, move |this, window, cx| {
                    let completed = completed_for_update
                        .lock()
                        .unwrap_or_else(|poisoned| poisoned.into_inner())
                        .take();
                    if let Some(completed) = completed {
                        let (scoped_keys, batch, result) = completed;
                        this.finish_discard_retirements(scoped_keys, batch, result, window, cx);
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
    }

    fn schedule_destructive_retirement_continuation(
        &mut self,
        pending: PendingStartupDestructive,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.pending_startup_destructive = Some(pending);
        self.set_status(
            "Waiting for recovery checkpoint cleanup before continuing. The document remains open."
                .into(),
            cx,
        );
        cx.spawn_in(window, async move |this, cx| {
            cx.background_executor().timer(Duration::from_secs(1)).await;
            loop {
                if crate::views::try_update_in(&this, cx, |this, window, cx| {
                    if let Some(pending) = this.pending_startup_destructive.take() {
                        this.resume_recovery_destructive(pending, window, cx);
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
    }

    pub(super) fn finish_discard_retirements(
        &mut self,
        scoped_keys: Vec<(RecoveryKey, Option<DocumentId>)>,
        batch: RecoveryRetirementBatch,
        result: Result<RetirementCompletion, RecoveryError>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let keys: Vec<_> = scoped_keys.iter().map(|(key, _)| key.clone()).collect();
        if keys
            .iter()
            .any(|key| self.recovery_flow.recovery_retirement_batches.get(key) != Some(&batch))
        {
            if let Some(request) = self.pending_destructive.take() {
                self.resume_recovery_destructive(
                    PendingStartupDestructive {
                        request,
                        keys: scoped_keys,
                    },
                    window,
                    cx,
                );
            }
            return;
        }
        let Some(mut request) = self.pending_destructive.take() else {
            self.release_recovery_retirement_suppressions(keys.iter(), cx);
            self.rearm_dirty_recovery(cx);
            return;
        };

        let mut wait_for_replayed_retirement = false;
        let cleanup_error = match result {
            Ok(RetirementCompletion::Retired { .. }) => {
                self.finish_recovery_retirement_batch(&keys, &batch, cx);
                wait_for_replayed_retirement = keys.iter().any(|key| {
                    self.recovery_flow
                        .pending_recovery_retirements
                        .contains_key(key)
                });
                None
            }
            Ok(RetirementCompletion::CleanupPending { error }) => {
                self.schedule_recovery_retirement_batch_retry(keys.clone(), batch.clone(), cx);
                Some(format!(
                    "Recovery checkpoint was cleared, but cleanup remains pending: {error}"
                ))
            }
            Err(error) => {
                self.finish_recovery_retirement_batch(&keys, &batch, cx);
                let mut suppression_changed = false;
                for key in &keys {
                    if !self.recovery_flow.recovery_retirements.contains_key(key)
                        && !self
                            .recovery_flow
                            .recovery_retirement_batches
                            .contains_key(key)
                    {
                        suppression_changed |= self
                            .recovery_flow
                            .recovery_retirement_suppressions
                            .remove(key)
                            .is_some();
                    }
                }
                if suppression_changed {
                    self.schedule_recovery_timer(cx);
                }
                self.rearm_dirty_recovery(cx);
                self.set_status(
                    format!(
                        "Could not clear the recovery checkpoint: {error}. The document remains open."
                    ),
                    cx,
                );
                return;
            }
        };
        if wait_for_replayed_retirement {
            self.schedule_destructive_retirement_continuation(
                PendingStartupDestructive {
                    request,
                    keys: scoped_keys,
                },
                window,
                cx,
            );
            return;
        }

        match request.revalidate(&self.lifecycle_documents(cx)) {
            DestructiveResolution::Prompt(_) => {
                self.release_recovery_retirement_suppressions(keys.iter(), cx);
                self.rearm_dirty_recovery(cx);
                self.pending_destructive_recovery.extend(scoped_keys);
                self.pending_destructive = Some(request);
                self.prompt_destructive(window, cx);
            }
            DestructiveResolution::Proceed(action) => {
                if self.destructive_action_has_revision_answers(&action, cx) {
                    self.release_recovery_retirement_suppressions(keys.iter(), cx);
                    self.rearm_dirty_recovery(cx);
                    self.set_status(i18n::t(i18n::Key::RevisionAnswersRetained, cx).into(), cx);
                    return;
                }
                for (key, _) in scoped_keys {
                    self.remove_recovery_state_for_key(&key, cx);
                }
                if let Some(error) = cleanup_error {
                    self.set_status(error, cx);
                }
                self.perform_destructive(action, window, cx);
            }
            DestructiveResolution::Cancelled | DestructiveResolution::SaveFailed(_) => {
                self.release_recovery_retirement_suppressions(keys.iter(), cx);
                self.rearm_dirty_recovery(cx);
            }
        }
    }

    fn rearm_dirty_recovery(&mut self, cx: &mut Context<Self>) {
        let dirty: Vec<_> = self
            .document_views()
            .into_iter()
            .filter(|document| {
                let (document_id, dirty) = {
                    let document = document.read(cx);
                    (document.id(), document.is_dirty())
                };
                dirty || self.revision_has_authored_answers_for_document(document_id, cx)
            })
            .collect();
        for document in dirty {
            self.arm_document_recovery(&document, cx);
        }
    }

    fn refresh_recovery_warning(&mut self, cx: &mut Context<Self>) {
        let warning = self
            .recovery_flow
            .recovery_schedules
            .values()
            .any(|state| state.protection_warning)
            .then(|| {
                "Recovery protection is unavailable for at least one dirty document. Editing and source files are unchanged."
                    .to_string()
            });
        if self.recovery_flow.recovery_warning != warning {
            self.recovery_flow.recovery_warning = warning;
            cx.notify();
        }
    }

    /// Remove one document's deadline before invalidating checkpoint work.
    pub(super) fn remove_recovery_schedule(
        &mut self,
        id: DocumentId,
        cx: &mut Context<Self>,
    ) -> Option<RecoveryKey> {
        let key = self
            .recovery_flow
            .recovery_schedules
            .remove(&id)
            .map(|state| {
                if let Some(attempt) = &state.in_flight {
                    attempt.cancel();
                }
                state.key
            });
        self.schedule_recovery_timer(cx);
        self.refresh_recovery_warning(cx);
        key
    }

    /// Invalidate in-flight checkpoint capabilities before deleting durable data.
    pub(super) fn invalidate_recovery(
        &mut self,
        key: &RecoveryKey,
        document_id: Option<DocumentId>,
        cx: &mut Context<Self>,
    ) {
        let document_id = insert_scoped_recovery_key(
            &mut self.recovery_flow.pending_recovery_retirements,
            key.clone(),
            document_id,
        );
        let content_identity = self.recovery_content_identity_for_retirement(key, document_id, cx);
        let had_owner = self.recovery_flow.recovery_retirements.contains_key(key)
            || self
                .recovery_flow
                .recovery_retirement_batches
                .contains_key(key);
        let Some(store) = self.recovery_flow.recovery.clone() else {
            return;
        };
        let ticket = match store.begin_retirement(key) {
            Ok(ticket) => ticket,
            Err(error) => {
                self.set_status(
                    format!("Could not clear the recovery checkpoint: {error}"),
                    cx,
                );
                if !had_owner {
                    self.rearm_dirty_recovery(cx);
                    self.schedule_recovery_retirement_retry(key.clone(), cx);
                }
                return;
            }
        };
        self.recovery_flow.pending_recovery_retirements.remove(key);
        let stale_batch = self
            .recovery_flow
            .recovery_retirement_batches
            .get(key)
            .cloned();
        let stale_batch_keys: Vec<_> = stale_batch
            .as_ref()
            .map(|batch| {
                self.recovery_flow
                    .recovery_retirement_batches
                    .iter()
                    .filter(|(_, current)| *current == batch)
                    .map(|(key, _)| key.clone())
                    .collect()
            })
            .unwrap_or_default();
        for stale_key in &stale_batch_keys {
            self.recovery_flow
                .recovery_retirement_batches
                .remove(stale_key);
            self.recovery_flow
                .recovery_retirement_retries
                .remove(stale_key);
        }
        self.recovery_flow
            .recovery_retirements
            .insert(key.clone(), ticket.clone());
        // `begin_retirement` has published the non-restorable marker.
        if let Some((document_id, content_identity)) = content_identity {
            self.recovery_flow
                .recovery_retirement_suppressions
                .entry(key.clone())
                .or_default()
                .insert(document_id, content_identity);
        }
        self.schedule_recovery_timer(cx);
        self.spawn_recovery_retirement_completion(
            key.clone(),
            ticket,
            document_id,
            Duration::ZERO,
            cx,
        );
        for stale_key in stale_batch_keys {
            if stale_key != *key
                && let Some(&document_id) = self
                    .recovery_flow
                    .pending_recovery_retirements
                    .get(&stale_key)
            {
                self.invalidate_recovery(&stale_key, document_id, cx);
            }
        }
    }

    fn spawn_recovery_retirement_completion(
        &mut self,
        key: RecoveryKey,
        ticket: RecoveryRetirement,
        document_id: Option<DocumentId>,
        delay: Duration,
        cx: &mut Context<Self>,
    ) {
        let Some(store) = self.recovery_flow.recovery.clone() else {
            return;
        };
        cx.spawn(async move |this, cx| {
            if !delay.is_zero() {
                cx.background_executor().timer(delay).await;
            }
            let completed_ticket = ticket.clone();
            let result = cx
                .background_spawn(async move {
                    let result = store.complete_retirement(ticket.clone());
                    if result.is_err() {
                        store.abandon_retirement(&ticket);
                    }
                    result
                })
                .await;
            let result = Arc::new(Mutex::new(Some(result)));
            loop {
                let key = key.clone();
                let ticket = completed_ticket.clone();
                let result_for_update = result.clone();
                if crate::views::try_update(&this, cx, move |this, cx| {
                    if this.recovery_flow.recovery_retirements.get(&key) != Some(&ticket) {
                        return;
                    }
                    let result = result_for_update
                        .lock()
                        .unwrap_or_else(|poisoned| poisoned.into_inner())
                        .take();
                    if let Some(result) = result {
                        this.finish_recovery_retirement(key, ticket, document_id, result, cx);
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
    }

    pub(super) fn finish_recovery_retirement(
        &mut self,
        key: RecoveryKey,
        ticket: RecoveryRetirement,
        document_id: Option<DocumentId>,
        result: Result<RetirementCompletion, RecoveryError>,
        cx: &mut Context<Self>,
    ) {
        if self.recovery_flow.recovery_retirements.get(&key) != Some(&ticket) {
            return;
        }
        self.recovery_flow.recovery_retirement_retries.remove(&key);
        let pending_revision_recovery = self.review_flow.take_pending_revision_recovery(&key);
        match result {
            Ok(RetirementCompletion::Retired { .. }) => {
                self.recovery_flow.recovery_retirements.remove(&key);
                if let Some(&document_id) =
                    self.recovery_flow.pending_recovery_retirements.get(&key)
                {
                    self.invalidate_recovery(&key, document_id, cx);
                } else if !self.destructive_retirement_waits_for_key(&key) {
                    // A replay ticket may finish before its destructive walk resumes.
                    self.release_recovery_retirement_suppressions(std::iter::once(&key), cx);
                    self.rearm_dirty_recovery(cx);
                }
            }
            Ok(RetirementCompletion::CleanupPending { error }) => {
                self.set_status(
                    format!(
                        "Recovery checkpoint was cleared, but cleanup remains pending: {error}"
                    ),
                    cx,
                );
                self.schedule_recovery_retirement_completion_retry(key, ticket, document_id, cx);
            }
            Err(error) => {
                self.recovery_flow.recovery_retirements.remove(&key);
                if let Some((document_id, recovery)) = pending_revision_recovery {
                    self.review_flow
                        .restore_revision_recovery(key.clone(), document_id, recovery);
                    self.rearm_dirty_recovery(cx);
                }
                insert_scoped_recovery_key(
                    &mut self.recovery_flow.pending_recovery_retirements,
                    key.clone(),
                    document_id,
                );
                self.set_status(
                    format!("Could not clear the recovery checkpoint: {error}"),
                    cx,
                );
                self.schedule_recovery_retirement_retry(key, cx);
            }
        }
    }

    fn schedule_recovery_retirement_completion_retry(
        &mut self,
        key: RecoveryKey,
        ticket: RecoveryRetirement,
        document_id: Option<DocumentId>,
        cx: &mut Context<Self>,
    ) {
        if !self
            .recovery_flow
            .recovery_retirement_retries
            .insert(key.clone())
        {
            return;
        }
        self.spawn_recovery_retirement_completion(
            key,
            ticket,
            document_id,
            Duration::from_secs(1),
            cx,
        );
    }

    pub(super) fn finish_recovery_retirement_batch(
        &mut self,
        keys: &[RecoveryKey],
        batch: &RecoveryRetirementBatch,
        cx: &mut Context<Self>,
    ) {
        if keys
            .iter()
            .any(|key| self.recovery_flow.recovery_retirement_batches.get(key) != Some(batch))
        {
            return;
        }
        for key in keys {
            self.recovery_flow.recovery_retirement_batches.remove(key);
            self.recovery_flow.recovery_retirement_retries.remove(key);
        }
        let queued: Vec<_> = keys
            .iter()
            .filter_map(|key| {
                self.recovery_flow
                    .pending_recovery_retirements
                    .get(key)
                    .map(|document_id| (key.clone(), *document_id))
            })
            .collect();
        for (key, document_id) in queued {
            self.invalidate_recovery(&key, document_id, cx);
        }
    }

    fn schedule_recovery_retirement_batch_retry(
        &mut self,
        keys: Vec<RecoveryKey>,
        batch: RecoveryRetirementBatch,
        cx: &mut Context<Self>,
    ) {
        if keys.is_empty()
            || keys
                .iter()
                .any(|key| self.recovery_flow.recovery_retirement_retries.contains(key))
        {
            return;
        }
        let Some(store) = self.recovery_flow.recovery.clone() else {
            return;
        };
        self.recovery_flow
            .recovery_retirement_retries
            .extend(keys.iter().cloned());
        cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(Duration::from_secs(1))
                .await;
            let completed_batch = batch.clone();
            let result = cx
                .background_spawn(async move {
                    let result = store.complete_retirements(batch.clone());
                    if result.is_err() {
                        store.abandon_retirements(&batch);
                    }
                    result
                })
                .await;
            let completed = Arc::new(Mutex::new(Some((keys, completed_batch, result))));
            loop {
                let completed_for_update = completed.clone();
                if crate::views::try_update(&this, cx, move |this, cx| {
                    let completed = completed_for_update
                        .lock()
                        .unwrap_or_else(|poisoned| poisoned.into_inner())
                        .take();
                    if let Some((keys, batch, result)) = completed {
                        if keys.iter().any(|key| {
                            this.recovery_flow
                                .recovery_retirement_batches
                                .get(key)
                                != Some(&batch)
                        }) {
                            return;
                        }
                        for key in &keys {
                            this.recovery_flow
                                .recovery_retirement_retries
                                .remove(key);
                        }
                        match result {
                            Ok(RetirementCompletion::Retired { .. }) => {
                                this.finish_recovery_retirement_batch(&keys, &batch, cx);
                            }
                            Ok(RetirementCompletion::CleanupPending { error }) => {
                                this.set_status(
                                    format!(
                                        "Recovery checkpoint was cleared, but cleanup remains pending: {error}"
                                    ),
                                    cx,
                                );
                                this.schedule_recovery_retirement_batch_retry(keys, batch, cx);
                            }
                            Err(error) => {
                                this.finish_recovery_retirement_batch(&keys, &batch, cx);
                                this.rearm_dirty_recovery(cx);
                                this.set_status(
                                    format!(
                                        "Could not finish recovery checkpoint cleanup: {error}"
                                    ),
                                    cx,
                                );
                            }
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
    }

    fn schedule_recovery_retirement_retry(&mut self, key: RecoveryKey, cx: &mut Context<Self>) {
        if !self
            .recovery_flow
            .recovery_retirement_retries
            .insert(key.clone())
        {
            return;
        }
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(Duration::from_secs(1)).await;
            loop {
                let key = key.clone();
                if crate::views::try_update(&this, cx, move |this, cx| {
                    let pending = this
                        .recovery_flow
                        .pending_recovery_retirements
                        .get(&key)
                        .copied();
                    let owned = this.recovery_flow.recovery_retirements.contains_key(&key)
                        || this
                            .recovery_flow
                            .recovery_retirement_batches
                            .contains_key(&key);
                    if owned {
                        return;
                    }
                    this.recovery_flow.recovery_retirement_retries.remove(&key);
                    if let Some(document_id) = pending {
                        this.invalidate_recovery(&key, document_id, cx);
                    } else {
                        this.release_recovery_retirement_suppressions(std::iter::once(&key), cx);
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
    }

    pub(super) fn retire_document_recovery(
        &mut self,
        id: DocumentId,
        fallback_key: Option<RecoveryKey>,
        cx: &mut Context<Self>,
    ) -> Option<RecoveryKey> {
        let key = self.remove_recovery_schedule(id, cx).or(fallback_key);
        if let Some(key) = &key {
            self.invalidate_recovery(key, Some(id), cx);
        }
        key
    }

    pub(super) fn cancel_recovery_attempts_for_key(&mut self, key: &RecoveryKey, now: Instant) {
        for state in self
            .recovery_flow
            .recovery_schedules
            .values_mut()
            .filter(|state| state.key == *key)
        {
            cancel_recovery_attempt_at(state, now);
            state.token = None;
        }
    }

    fn remove_recovery_state_for_key(&mut self, key: &RecoveryKey, cx: &mut Context<Self>) {
        self.recovery_flow.recovery_schedules.retain(|_, state| {
            if state.key == *key {
                if let Some(attempt) = &state.in_flight {
                    attempt.cancel();
                }
                false
            } else {
                true
            }
        });
        self.recovery_flow
            .recovery_retirement_suppressions
            .remove(key);
        self.schedule_recovery_timer(cx);
        self.refresh_recovery_warning(cx);
    }

    pub(super) fn arm_document_recovery(
        &mut self,
        document: &Entity<DocumentView>,
        cx: &mut Context<Self>,
    ) {
        self.arm_document_recovery_at(document, cx.background_executor().now(), cx);
    }

    pub(super) fn arm_document_recovery_at(
        &mut self,
        document: &Entity<DocumentView>,
        now: Instant,
        cx: &mut Context<Self>,
    ) {
        let store = self.recovery_flow.recovery.clone();
        let (id, revision, mut key, file_backed, dirty) = {
            let document = document.read(cx);
            (
                document.id(),
                document.revision(),
                document.recovery_key(),
                document.source_path().is_some(),
                document.is_dirty(),
            )
        };
        let (mut content_identity, revision_recovery) =
            self.revision_recovery_snapshot(id, &key, revision);
        let mut retained_key_warning = false;
        let isolate_pre_scan_edit = self.recovery_flow.startup_recovery_pending
            && self.recovery_flow.recovery.is_none()
            && file_backed
            && dirty
            && self.recovery_flow.startup_recovery_keys.get(&id) == Some(&key);
        if isolate_pre_scan_edit
            || self.recovery_key_needs_isolation(id, &key, revision, revision_recovery.as_ref())
        {
            let previous_key = key.clone();
            if let Some(store) = &store {
                retained_key_warning = store.activate_and_current_token(&previous_key).1;
            }
            if let Some(mut previous_state) = self.recovery_flow.recovery_schedules.remove(&id) {
                cancel_recovery_attempt(&mut previous_state);
                if previous_state.key != previous_key
                    && let Some(store) = &store
                {
                    retained_key_warning |= store.activate_and_current_token(&previous_state.key).1;
                }
                retained_key_warning |= previous_state.protection_warning;
            }
            self.recovery_flow
                .pending_recovery_retirements
                .remove(&previous_key);
            key = document.update(cx, |document, _| document.rotate_recovery_key());
            if !self
                .review_flow
                .revision_context_has_authored_answers_for_document(id)
            {
                content_identity = RecoveryContentIdentity::from_revision_recovery(revision, None);
            }
        }
        let replaced_key = self
            .recovery_flow
            .recovery_schedules
            .get_mut(&id)
            .and_then(|state| {
                (state.key != key).then(|| {
                    cancel_recovery_attempt(state);
                    state.key.clone()
                })
            });
        if let Some(previous_key) = replaced_key {
            self.invalidate_recovery(&previous_key, Some(id), cx);
        }
        if self.recovery_content_retirement_suppressed(&key, id, &content_identity) {
            self.remove_recovery_schedule(id, cx);
            return;
        }
        match self.recovery_flow.recovery_schedules.entry(id) {
            std::collections::hash_map::Entry::Vacant(entry) => {
                let mut schedule = CheckpointSchedule::default();
                schedule.mark_dirty(now);
                let (token, mut protection_warning) =
                    store.as_ref().map_or((None, false), |store| {
                        let (token, deferred) = store.activate_and_current_token(&key);
                        (Some(token), deferred)
                    });
                protection_warning |= retained_key_warning;
                entry.insert(DocumentRecoveryState {
                    key,
                    content_identity,
                    suppressed_oversized_revision: None,
                    token,
                    schedule,
                    in_flight: None,
                    deadline_reported: false,
                    protection_warning,
                });
            }
            std::collections::hash_map::Entry::Occupied(mut entry) => {
                let state = entry.get_mut();
                if state.key != key {
                    cancel_recovery_attempt(state);
                    state.key = key.clone();
                    let (token, protection_warning) =
                        store.as_ref().map_or((None, false), |store| {
                            let (token, deferred) = store.activate_and_current_token(&key);
                            (Some(token), deferred)
                        });
                    state.token = token;
                    state.schedule = CheckpointSchedule::default();
                    state.in_flight = None;
                    state.deadline_reported = false;
                    state.content_identity = content_identity;
                    state.suppressed_oversized_revision = None;
                    state.protection_warning = protection_warning;
                } else if state.content_identity != content_identity {
                    cancel_recovery_attempt(state);
                    if state.suppressed_oversized_revision.take().is_some() {
                        state.schedule = CheckpointSchedule::default();
                    }
                    state.content_identity = content_identity;
                }
                if state.token.is_none()
                    && let Some(store) = &store
                {
                    let (token, protection_deferred) = store.activate_and_current_token(&key);
                    state.token = Some(token);
                    state.protection_warning |= protection_deferred;
                }
                state.protection_warning |= retained_key_warning;
                state.schedule.mark_dirty(now);
            }
        }
        self.schedule_recovery_timer(cx);
        self.refresh_recovery_warning(cx);
    }

    pub(super) fn schedule_recovery_timer(&mut self, cx: &mut Context<Self>) {
        self.recovery_flow.recovery_timer_generation =
            self.recovery_flow.recovery_timer_generation.wrapping_add(1);
        let generation = self.recovery_flow.recovery_timer_generation;
        // Dropping the previous task is the primary cancellation mechanism;
        // `generation` also protects the narrow race where it wakes first.
        self.recovery_flow._recovery_timer = None;
        let recovery_available = self.recovery_flow.recovery.is_some();
        let worker_active = self.recovery_flow.recovery_checkpoint_worker_active;
        let now = cx.background_executor().now();
        let Some(deadline) = self
            .recovery_flow
            .recovery_schedules
            .iter()
            .filter_map(|(id, state)| match &state.in_flight {
                _ if self.recovery_content_retirement_suppressed(
                    &state.key,
                    *id,
                    &state.content_identity,
                ) =>
                {
                    None
                }
                _ if state.suppressed_oversized_revision.as_ref()
                    == Some(&state.content_identity) =>
                {
                    None
                }
                Some(attempt) if !state.deadline_reported => {
                    Some(attempt.timing.durable_complete_by)
                }
                Some(_) => None,
                None if worker_active && state.deadline_reported => None,
                None if recovery_available || !state.protection_warning => {
                    state.schedule.next_deadline()
                }
                None => None,
            })
            .min()
        else {
            return;
        };
        let delay = deadline.saturating_duration_since(now);
        self.recovery_flow._recovery_timer = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(delay).await;
            loop {
                if crate::views::try_update(&this, cx, |this, cx| {
                    if this.recovery_flow.recovery_timer_generation != generation {
                        return;
                    }
                    this.recovery_flow._recovery_timer = None;
                    this.checkpoint_recovery(cx);
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
        }));
    }

    pub(super) fn checkpoint_recovery(&mut self, cx: &mut Context<Self>) {
        self.checkpoint_recovery_at(cx.background_executor().now(), cx);
    }

    pub(super) fn checkpoint_recovery_at(&mut self, now: Instant, cx: &mut Context<Self>) {
        let Some(store) = self.recovery_flow.recovery.clone() else {
            let suppressions = &self.recovery_flow.recovery_retirement_suppressions;
            for (id, state) in &mut self.recovery_flow.recovery_schedules {
                if suppressions
                    .get(&state.key)
                    .and_then(|documents| documents.get(id))
                    == Some(&state.content_identity)
                {
                    continue;
                }
                if state.in_flight.is_none() && state.schedule.is_due(now) {
                    state.protection_warning = true;
                }
            }
            self.refresh_recovery_warning(cx);
            self.schedule_recovery_timer(cx);
            return;
        };
        let documents = self.document_views();
        let worker_active = self.recovery_flow.recovery_checkpoint_worker_active;
        let mut open_ids = HashSet::new();
        let mut active_keys = HashSet::new();
        let mut due = Vec::new();
        let mut preflight_error = None;

        for document in documents {
            let (id, dirty, revision, key, text_byte_len) = {
                let document = document.read(cx);
                (
                    document.id(),
                    document.is_dirty(),
                    document.revision(),
                    document.recovery_key(),
                    document.text_byte_len(cx),
                )
            };
            open_ids.insert(id);
            let answer_dirty = self
                .review_flow
                .revision_has_authored_answers_for_document(id, Some(&key));
            if !dirty && !answer_dirty {
                if let Some(state) = self.recovery_flow.recovery_schedules.remove(&id) {
                    self.invalidate_recovery(&state.key, Some(id), cx);
                }
                continue;
            }
            active_keys.insert(key.clone());
            let (content_identity, revision_recovery) =
                self.revision_recovery_snapshot(id, &key, revision);

            if self.recovery_content_retirement_suppressed(&key, id, &content_identity) {
                if let Some(mut state) = self.recovery_flow.recovery_schedules.remove(&id) {
                    if let Some(attempt) = state.in_flight.take() {
                        attempt.cancel();
                    }
                    if state.key != key {
                        self.invalidate_recovery(&state.key, Some(id), cx);
                    }
                }
                continue;
            }

            let replaced_key =
                self.recovery_flow
                    .recovery_schedules
                    .get_mut(&id)
                    .and_then(|state| {
                        (state.key != key).then(|| {
                            if let Some(attempt) = &state.in_flight {
                                attempt.cancel();
                            }
                            state.key.clone()
                        })
                    });
            if let Some(previous_key) = replaced_key {
                self.invalidate_recovery(&previous_key, Some(id), cx);
            }
            let state = self
                .recovery_flow
                .recovery_schedules
                .entry(id)
                .or_insert_with(|| {
                    let mut schedule = CheckpointSchedule::default();
                    schedule.mark_dirty(now);
                    let (token, protection_warning) = store.activate_and_current_token(&key);
                    DocumentRecoveryState {
                        key: key.clone(),
                        content_identity,
                        suppressed_oversized_revision: None,
                        token: Some(token),
                        schedule,
                        in_flight: None,
                        deadline_reported: false,
                        protection_warning,
                    }
                });
            if state.key != key {
                cancel_recovery_attempt(state);
                state.key = key.clone();
                state.content_identity = content_identity;
                let (token, protection_warning) = store.activate_and_current_token(&key);
                state.token = Some(token);
                state.schedule = CheckpointSchedule::default();
                state.schedule.mark_dirty(now);
                state.in_flight = None;
                state.deadline_reported = false;
                state.suppressed_oversized_revision = None;
                state.protection_warning = protection_warning;
            } else if state.content_identity != content_identity {
                cancel_recovery_attempt(state);
                if state.suppressed_oversized_revision.take().is_some() {
                    state.schedule = CheckpointSchedule::default();
                }
                state.content_identity = content_identity;
                state.schedule.mark_dirty(now);
            }
            if let Some(attempt) = state.in_flight.as_ref()
                && now >= attempt.timing.durable_complete_by
                && !state.deadline_reported
            {
                let timing = attempt.timing;
                attempt.cancel();
                state.schedule.checkpoint_deadline_missed(timing, now);
                state.deadline_reported = true;
                state.protection_warning = true;
            }
            if state.in_flight.is_none() && state.schedule.is_due(now) {
                if state.suppressed_oversized_revision.as_ref() == Some(&content_identity) {
                    continue;
                }
                if worker_active {
                    state.deadline_reported = true;
                    state.protection_warning = true;
                    continue;
                }
                let plaintext_ceiling = store.plaintext_admission_ceiling();
                if text_byte_len as u64 > plaintext_ceiling {
                    state.suppressed_oversized_revision = Some(content_identity);
                    state.protection_warning = true;
                    preflight_error = Some(
                        RecoveryError::OversizedCheckpoint {
                            bytes: text_byte_len as u64,
                            limit: plaintext_ceiling,
                        }
                        .to_string(),
                    );
                    continue;
                }
                // Capture the generation immediately before dispatch. A later
                // Save or Discard invalidates it before deleting the record.
                state.token = Some(store.current_token(&state.key));
                let timing = state
                    .schedule
                    .checkpoint_dispatched(now)
                    .expect("a due dirty recovery schedule must produce attempt timing");
                let attempt = RecoveryAttempt {
                    token: state
                        .token
                        .clone()
                        .expect("a ready recovery store must provide a checkpoint token"),
                    content_identity,
                    timing,
                    cancelled: Arc::new(AtomicBool::new(false)),
                };
                state.in_flight = Some(attempt.clone());
                state.deadline_reported = false;
                due.push((
                    id,
                    attempt,
                    document
                        .read(cx)
                        .recovery_checkpoint_with_revision(cx, revision_recovery),
                ));
            }
        }
        self.recovery_flow
            .recovery_schedules
            .retain(|id, _| open_ids.contains(id));
        self.refresh_recovery_warning(cx);
        if let Some(error) = preflight_error {
            self.set_status(
                checkpoint_batch_status(0, Some(&error))
                    .expect("a checkpoint error must produce visible status"),
                cx,
            );
        }
        if due.is_empty() {
            self.schedule_recovery_timer(cx);
            return;
        }

        debug_assert!(!self.recovery_flow.recovery_checkpoint_worker_active);
        self.recovery_flow.recovery_checkpoint_worker_active = true;
        self.schedule_recovery_timer(cx);

        cx.spawn(async move |this, cx| {
            let background_executor = cx.background_executor().clone();
            let batch = cx
                .background_spawn(async move {
                    let batch = store.checkpoint_batch_if_current_cancellable(
                        due.iter().map(|(_, attempt, checkpoint)| {
                            CancellableRecoveryCheckpointAttempt {
                                checkpoint,
                                token: &attempt.token,
                                cancelled: attempt.cancelled.as_ref(),
                            }
                        }),
                        &active_keys,
                    );
                    let store_returned_at = background_executor.now();
                    let results = due
                        .into_iter()
                        .zip(batch.outcomes)
                        .map(|((id, attempt, _), outcome)| (id, attempt, outcome))
                        .collect::<Vec<_>>();
                    (results, batch.maintenance, store_returned_at)
                })
                .await;

            let batch = Arc::new(Mutex::new(Some(batch)));
            loop {
                let batch = batch.clone();
                if crate::views::try_update(&this, cx, move |this, cx| {
                    let batch = batch
                        .lock()
                        .unwrap_or_else(|poisoned| poisoned.into_inner())
                        .take();
                    if let Some((results, maintenance, store_returned_at)) = batch {
                        this.finish_recovery_checkpoints(
                            results,
                            maintenance,
                            store_returned_at,
                            cx,
                        );
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
    }

    pub(super) fn finish_recovery_checkpoints(
        &mut self,
        results: Vec<(DocumentId, RecoveryAttempt, CheckpointBatchOutcome)>,
        maintenance: RecoveryMaintenance,
        store_returned_at: Instant,
        cx: &mut Context<Self>,
    ) {
        let now = cx.background_executor().now();
        let worker_released =
            std::mem::take(&mut self.recovery_flow.recovery_checkpoint_worker_active);
        let maintenance_issues = maintenance.issues.len();
        let mut last_error = None;
        for (id, attempt, outcome) in results {
            let attempt_is_current = self
                .recovery_flow
                .recovery_schedules
                .get(&id)
                .is_some_and(|state| state.in_flight.as_ref() == Some(&attempt));
            let written_identity_is_current = self
                .recovery_flow
                .recovery_schedules
                .get(&id)
                .is_some_and(|state| {
                    current_checkpoint_write_completed_for_identity(
                        attempt_is_current,
                        state.content_identity,
                        attempt.content_identity,
                        &outcome,
                    )
                });
            if let Some(state) = self.recovery_flow.recovery_schedules.get_mut(&id)
                && attempt_is_current
            {
                let current = state
                    .in_flight
                    .take()
                    .expect("the current recovery attempt must still be in flight");
                let deadline_reported = std::mem::take(&mut state.deadline_reported);
                let content_identity_is_current =
                    state.content_identity == current.content_identity;
                let oversized_revision_is_current = content_identity_is_current
                    && matches!(
                        &outcome,
                        CheckpointBatchOutcome::Failed(RecoveryError::OversizedCheckpoint { .. })
                    );
                if oversized_revision_is_current {
                    state.suppressed_oversized_revision = Some(current.content_identity);
                    state.protection_warning = true;
                } else if store_returned_at > current.timing.durable_complete_by {
                    if !deadline_reported {
                        state
                            .schedule
                            .checkpoint_deadline_missed(current.timing, store_returned_at);
                    }
                    state.protection_warning = true;
                } else {
                    match &outcome {
                        CheckpointBatchOutcome::Written if content_identity_is_current => {
                            if deadline_reported
                                && state.content_identity == current.content_identity
                            {
                                state
                                    .schedule
                                    .mark_durable_baseline(current.timing.snapshot_at);
                            } else if !deadline_reported {
                                state.schedule.checkpoint_written(current.timing);
                            }
                            if state.content_identity == current.content_identity {
                                state.protection_warning = false;
                            }
                        }
                        CheckpointBatchOutcome::Written => {
                            if !deadline_reported {
                                state
                                    .schedule
                                    .checkpoint_cancelled(current.timing, store_returned_at);
                            }
                        }
                        CheckpointBatchOutcome::Superseded if !deadline_reported => {
                            if current.cancelled.load(Ordering::Acquire) {
                                state
                                    .schedule
                                    .checkpoint_cancelled(current.timing, store_returned_at);
                            } else {
                                state.schedule.checkpoint_superseded(current.timing);
                            }
                        }
                        CheckpointBatchOutcome::Failed(_) | CheckpointBatchOutcome::Deferred
                            if !deadline_reported =>
                        {
                            state
                                .schedule
                                .checkpoint_failed(current.timing, store_returned_at);
                            state.protection_warning = true;
                        }
                        CheckpointBatchOutcome::Superseded
                        | CheckpointBatchOutcome::Failed(_)
                        | CheckpointBatchOutcome::Deferred => {}
                    }
                }
            }
            match outcome {
                CheckpointBatchOutcome::Written if written_identity_is_current => {
                    log::debug!("recovery checkpoint written")
                }
                CheckpointBatchOutcome::Failed(error) if attempt_is_current => {
                    last_error = Some(error.to_string())
                }
                CheckpointBatchOutcome::Written
                | CheckpointBatchOutcome::Superseded
                | CheckpointBatchOutcome::Deferred
                | CheckpointBatchOutcome::Failed(_) => {}
            }
        }

        if let Some(status) = checkpoint_batch_status(maintenance_issues, last_error.as_deref()) {
            self.set_status(status, cx);
        }
        self.refresh_recovery_warning(cx);
        if worker_released {
            self.checkpoint_recovery_at(now, cx);
        } else {
            self.schedule_recovery_timer(cx);
        }
    }
}
