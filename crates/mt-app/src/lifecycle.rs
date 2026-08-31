//! Destructive document lifecycle decisions, independent of GPUI and paths.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT_DOCUMENT_ID: AtomicU64 = AtomicU64::new(1);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct DocumentId(u64);

impl DocumentId {
    pub fn next() -> Self {
        Self(NEXT_DOCUMENT_ID.fetch_add(1, Ordering::Relaxed))
    }

    #[cfg(test)]
    const fn test(value: u64) -> Self {
        Self(value)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BufferSnapshot {
    revision: u64,
    text: String,
}

impl BufferSnapshot {
    pub fn new(revision: u64, text: String) -> Self {
        Self { revision, text }
    }

    pub fn matches(&self, revision: u64, text: &str) -> bool {
        self.revision == revision && self.text == text
    }

    pub fn text(&self) -> &str {
        &self.text
    }
}

/// The source identity an asynchronous operation observed before leaving the
/// UI thread. A Save As may preserve both buffer revision and exact text while
/// changing the source document, so that boundary has its own generation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AsyncSnapshot {
    buffer: BufferSnapshot,
    source_generation: u64,
}

impl AsyncSnapshot {
    pub fn new(revision: u64, text: String, source_generation: u64) -> Self {
        Self {
            buffer: BufferSnapshot::new(revision, text),
            source_generation,
        }
    }

    pub fn matches(&self, revision: u64, text: &str, source_generation: u64) -> bool {
        self.source_generation == source_generation && self.buffer.matches(revision, text)
    }

    pub fn text(&self) -> &str {
        self.buffer.text()
    }

    pub fn source_generation(&self) -> u64 {
        self.source_generation
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DocumentLifecycle {
    pub id: DocumentId,
    pub dirty: bool,
    /// The exact buffer state that a destructive decision would dispose of.
    /// A document can become dirty again while another document's prompt is
    /// open, so dirty alone is not permission to proceed.
    pub snapshot: BufferSnapshot,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DestructiveAction {
    CloseTab(DocumentId),
    CloseWindow,
    ReplaceWorkspace(PathBuf),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DirtyDecision {
    Save,
    Discard,
    Cancel,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DestructiveResolution {
    Prompt(DocumentId),
    Proceed(DestructiveAction),
    Cancelled,
    SaveFailed(DocumentId),
}

#[derive(Debug, Clone)]
pub struct DestructiveRequest {
    action: DestructiveAction,
    prompted: Option<PromptedDocument>,
    handled: HashMap<DocumentId, BufferSnapshot>,
}

#[derive(Debug, Clone)]
struct PromptedDocument {
    id: DocumentId,
    snapshot: BufferSnapshot,
}

impl DestructiveRequest {
    pub fn new(action: DestructiveAction, documents: &[DocumentLifecycle]) -> Self {
        let mut request = Self {
            action,
            prompted: None,
            handled: HashMap::new(),
        };
        request.revalidate(documents);
        request
    }

    pub fn current(&self) -> Option<DocumentId> {
        self.prompted.as_ref().map(|document| document.id)
    }

    pub fn initial_resolution(&self) -> DestructiveResolution {
        match self.current() {
            Some(id) => DestructiveResolution::Prompt(id),
            None => DestructiveResolution::Proceed(self.action.clone()),
        }
    }

    /// Re-scan every document that this action could destroy. A matching
    /// handled snapshot remains authorized; a new revision needs its own
    /// explicit decision.
    pub fn revalidate(&mut self, documents: &[DocumentLifecycle]) -> DestructiveResolution {
        let mut next_prompt = None;
        for document in self.relevant_documents(documents) {
            let handled = self
                .handled
                .get(&document.id)
                .is_some_and(|handled| handled == &document.snapshot);
            if document.dirty && !handled {
                next_prompt = Some(PromptedDocument {
                    id: document.id,
                    snapshot: document.snapshot.clone(),
                });
                break;
            }
        }
        self.prompted = next_prompt;
        self.initial_resolution()
    }

    /// A modal answer is permission only for the exact dirty snapshot that
    /// caused the modal. The caller checks this before invoking a Save or
    /// recording a Discard.
    pub fn current_prompt_matches(&self, documents: &[DocumentLifecycle]) -> bool {
        let Some(prompted) = self.prompted.as_ref() else {
            return false;
        };
        self.relevant_documents(documents).any(|document| {
            document.id == prompted.id && document.dirty && document.snapshot == prompted.snapshot
        })
    }

    pub fn decide(
        &mut self,
        decision: DirtyDecision,
        saved_snapshot: Option<BufferSnapshot>,
        documents: &[DocumentLifecycle],
    ) -> DestructiveResolution {
        let Some(prompted) = self.prompted.clone() else {
            return self.revalidate(documents);
        };

        match decision {
            DirtyDecision::Cancel => DestructiveResolution::Cancelled,
            DirtyDecision::Save if saved_snapshot.as_ref() != Some(&prompted.snapshot) => {
                DestructiveResolution::SaveFailed(prompted.id)
            }
            DirtyDecision::Save => {
                // A successful save makes the document clean, so it no longer
                // matches `current_prompt_matches`. The caller supplies the
                // snapshot it actually wrote as the durable authorization.
                self.handled.insert(prompted.id, prompted.snapshot);
                self.revalidate(documents)
            }
            DirtyDecision::Discard => {
                // A result from an old prompt cannot authorize discarding text
                // that arrived while the prompt was open.
                if !self.current_prompt_matches(documents) {
                    return self.revalidate(documents);
                }
                self.handled.insert(prompted.id, prompted.snapshot);
                self.revalidate(documents)
            }
        }
    }

    fn relevant_documents<'a>(
        &'a self,
        documents: &'a [DocumentLifecycle],
    ) -> impl Iterator<Item = &'a DocumentLifecycle> + 'a {
        documents.iter().filter(move |document| match &self.action {
            DestructiveAction::CloseTab(id) => document.id == *id,
            DestructiveAction::CloseWindow | DestructiveAction::ReplaceWorkspace(_) => true,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn file(id: u64, dirty: bool) -> DocumentLifecycle {
        DocumentLifecycle {
            id: DocumentId::test(id),
            dirty,
            snapshot: BufferSnapshot::new(0, format!("document {id}")),
        }
    }

    fn revised_file(id: u64, revision: u64, text: &str) -> DocumentLifecycle {
        DocumentLifecycle {
            dirty: true,
            snapshot: BufferSnapshot::new(revision, text.into()),
            ..file(id, true)
        }
    }

    #[test]
    fn a_clean_tab_closes_without_a_prompt() {
        let request = DestructiveRequest::new(
            DestructiveAction::CloseTab(DocumentId::test(1)),
            &[file(1, false)],
        );

        assert_eq!(
            request.initial_resolution(),
            DestructiveResolution::Proceed(DestructiveAction::CloseTab(DocumentId::test(1)))
        );
    }

    #[test]
    fn save_must_succeed_before_a_dirty_tab_can_close() {
        let mut request = DestructiveRequest::new(
            DestructiveAction::CloseTab(DocumentId::test(1)),
            &[file(1, true)],
        );

        assert_eq!(
            request.initial_resolution(),
            DestructiveResolution::Prompt(DocumentId::test(1))
        );
        assert_eq!(
            request.decide(DirtyDecision::Save, None, &[file(1, true)]),
            DestructiveResolution::SaveFailed(DocumentId::test(1))
        );

        let mut request = DestructiveRequest::new(
            DestructiveAction::CloseTab(DocumentId::test(1)),
            &[file(1, true)],
        );
        assert_eq!(
            request.decide(
                DirtyDecision::Save,
                Some(BufferSnapshot::new(0, "document 1".into())),
                &[file(1, false)],
            ),
            DestructiveResolution::Proceed(DestructiveAction::CloseTab(DocumentId::test(1)))
        );
    }

    #[test]
    fn discard_proceeds_and_cancel_preserves_the_document() {
        let action = DestructiveAction::CloseTab(DocumentId::test(1));
        let mut discard = DestructiveRequest::new(action.clone(), &[file(1, true)]);
        assert_eq!(
            discard.decide(DirtyDecision::Discard, None, &[file(1, true)]),
            DestructiveResolution::Proceed(action.clone())
        );

        let mut cancel = DestructiveRequest::new(action, &[file(1, true)]);
        assert_eq!(
            cancel.decide(DirtyDecision::Cancel, None, &[file(1, true)]),
            DestructiveResolution::Cancelled
        );
    }

    #[test]
    fn window_close_walks_every_dirty_document() {
        let mut request = DestructiveRequest::new(
            DestructiveAction::CloseWindow,
            &[file(1, true), file(2, false), file(3, true)],
        );

        assert_eq!(request.current(), Some(DocumentId::test(1)));
        assert_eq!(
            request.decide(
                DirtyDecision::Discard,
                None,
                &[file(1, true), file(2, false), file(3, true)],
            ),
            DestructiveResolution::Prompt(DocumentId::test(3))
        );
        assert_eq!(
            request.decide(
                DirtyDecision::Save,
                Some(BufferSnapshot::new(0, "document 3".into())),
                &[file(1, true), file(2, false), file(3, false)],
            ),
            DestructiveResolution::Proceed(DestructiveAction::CloseWindow)
        );
    }

    #[test]
    fn a_dirty_in_memory_document_uses_the_same_boundary() {
        let document = mt_doc::Document::new(None, "unsaved text".into());
        assert!(document.path().is_none());
        let document = DocumentLifecycle {
            id: DocumentId::test(7),
            dirty: true,
            snapshot: BufferSnapshot::new(1, "unsaved text".into()),
        };
        let mut request = DestructiveRequest::new(
            DestructiveAction::CloseWindow,
            std::slice::from_ref(&document),
        );

        assert_eq!(
            request.initial_resolution(),
            DestructiveResolution::Prompt(DocumentId::test(7))
        );
        assert_eq!(
            request.decide(DirtyDecision::Cancel, None, &[document]),
            DestructiveResolution::Cancelled
        );
    }

    #[test]
    fn a_document_changed_while_its_prompt_is_open_needs_a_new_prompt() {
        let mut request = DestructiveRequest::new(
            DestructiveAction::CloseWindow,
            &[revised_file(1, 1, "first")],
        );
        let changed = [revised_file(1, 2, "newer first")];

        assert!(!request.current_prompt_matches(&changed));
        assert_eq!(
            request.decide(DirtyDecision::Discard, None, &changed),
            DestructiveResolution::Prompt(DocumentId::test(1)),
        );
        assert_eq!(
            request.decide(DirtyDecision::Discard, None, &changed),
            DestructiveResolution::Proceed(DestructiveAction::CloseWindow),
        );
    }

    #[test]
    fn revalidation_prompts_for_a_document_that_becomes_dirty_later() {
        let mut request = DestructiveRequest::new(
            DestructiveAction::CloseWindow,
            &[revised_file(1, 1, "first"), file(2, false)],
        );
        let after_first_discard = [revised_file(1, 1, "first"), revised_file(2, 1, "second")];

        assert_eq!(
            request.decide(DirtyDecision::Discard, None, &after_first_discard),
            DestructiveResolution::Prompt(DocumentId::test(2)),
        );
    }

    #[test]
    fn a_save_only_handles_the_snapshot_that_reached_disk() {
        let mut request = DestructiveRequest::new(
            DestructiveAction::CloseWindow,
            &[revised_file(1, 1, "first")],
        );
        let clean_after_a_different_save = [DocumentLifecycle {
            dirty: false,
            snapshot: BufferSnapshot::new(2, "newer first".into()),
            ..file(1, false)
        }];

        assert_eq!(
            request.decide(
                DirtyDecision::Save,
                Some(BufferSnapshot::new(2, "newer first".into())),
                &clean_after_a_different_save,
            ),
            DestructiveResolution::SaveFailed(DocumentId::test(1)),
        );
    }

    #[test]
    fn a_result_from_revision_n_cannot_replace_revision_n_plus_one() {
        let snapshot = BufferSnapshot::new(10, "draft".into());

        assert!(snapshot.matches(10, "draft"));
        assert!(!snapshot.matches(11, "newer draft"));
        assert!(
            !snapshot.matches(11, "draft"),
            "editing away and back still creates a newer source revision"
        );
    }

    #[test]
    fn an_async_result_cannot_cross_a_save_as_boundary_with_the_same_text() {
        let snapshot = AsyncSnapshot::new(10, "draft".into(), 4);

        assert!(snapshot.matches(10, "draft", 4));
        assert!(
            !snapshot.matches(10, "draft", 5),
            "Save As changes the source even when its exact editor buffer survives"
        );
    }
}
