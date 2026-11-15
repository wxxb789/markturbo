//! Application state and the file operations the shell calls.
//!
//! One `AppState` owns the workspace root, the visible tree, open tabs and the
//! one `LoadedFile` per tab. Components hold a clone; every mutable value is a
//! signal so a change reaches only the bindings that read it.

use std::{
    cell::{Cell, RefCell},
    collections::{HashMap, HashSet},
    future::Future,
    path::{Path, PathBuf},
    rc::Rc,
    sync::atomic::{AtomicBool, Ordering},
    time::{Duration, Instant, SystemTime},
};

use mt_core::{
    Document,
    agent_artifacts::{
        context::{
            CODEX_PROFILE_ID, ContextInput, ProjectTrust, ResolvedContext, SelectedContext, resolve,
        },
        instruction::{self, Instruction},
        package::{ReviewRequestBuildRequest, ReviewTarget, build_review_request},
        skill::{self, Discovery, Skill},
    },
    credentials::CredentialVault,
    document::io::{self, FileStamp, LoadedFile, Newline, SaveError, SourceIdentity},
    model::{
        ConsentCapability, ConsentDecision, ModelRequestDisclosure, OutboundScope,
        OutboundScopeKind,
    },
    recovery::{RecoveredRecord, RecoveryCheckpoint, RecoveryKey, RecoveryMetadata, RecoveryStore},
    review::{
        ArtifactLens, ClarificationPriority, FindingKind, ReviewModelOutput, ReviewRequest,
        ReviewResult, ReviewStatus, SkillFilePayload, SourceAnchor, SourceLocation, SourceSnapshot,
        provider::{
            PreparedDocumentReviewRequest, PreparedReview, PreparedRevisionRequest, ReviewError,
            ReviewLanguage, RevisionAnswer, RevisionAnswers, RevisionError,
        },
        revision::{ChangeId, RevisionProposal},
    },
    settings::{Language, RecentTarget, RecentTargetKind, SettingsData, ThemePreference},
    translate::{
        Scope, TranslationRequest,
        provider::{PreparedTranslation, PreparedTranslationRequest, TranslationError},
    },
    usage::{ReviewDurationBucket, UsageAggregate, UsageEvent, UsageEventKind, UsageStore},
    workspace::{
        search::{self, Query},
        tree::{self, FileNode},
    },
};
use nsh_base::alert_dialog;
use nsh_core::{file_dialog::SaveDialog, prelude::*};

static FIRST_LAUNCH_CHECKED: AtomicBool = AtomicBool::new(false);

/// Which panes a document shows.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Pane {
    Source,
    Preview,
    Web,
    #[default]
    Split,
    SplitWeb,
}

/// One row of the sidebar.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub path: PathBuf,
    pub name: String,
    pub depth: usize,
    pub is_dir: bool,
}

/// One open document.
#[derive(Clone)]
pub struct Tab {
    pub id: u64,
    pub path: PathBuf,
    pub title: SharedString,
    pub text: Signal<SharedString>,
    pub dirty: Signal<bool>,
    pub revision: Signal<u64>,
    pub pane: Signal<Pane>,
    pub web_trusted: Signal<bool>,
    pub conflict: Signal<bool>,
    pub untitled: bool,
    pub recovery_key: RecoveryKey,
    pub loaded: Rc<RefCell<LoadedFile>>,
}

impl PartialEq for Tab {
    fn eq(&self, other: &Self) -> bool {
        self.id == other.id
    }
}

/// The shell's state.
#[derive(Clone)]
pub struct AppState {
    pub root: Signal<Option<PathBuf>>,
    pub entries: Signal<Vec<Entry>>,
    pub tabs: Signal<Vec<Tab>>,
    pub active: Signal<Option<u64>>,
    pub status: Signal<SharedString>,
    pub search_query: Signal<String>,
    pub search_results: Signal<Vec<SearchHit>>,
    pub search_ran: Signal<bool>,
    pub search_truncated: Signal<bool>,
    pub search_busy: Signal<bool>,
    pub settings: Signal<SettingsData>,
    pub settings_open: Signal<bool>,
    pub settings_credential: Signal<String>,
    pub settings_notice: Signal<SharedString>,
    /// Bumped after a credential is stored or removed, so the settings page
    /// re-reads the credential status: the vault has no signal of its own.
    pub settings_credential_revision: Signal<u64>,
    /// The last credential status and what it was computed for, so a render
    /// that changes nothing about the endpoint does not read the secure store
    /// again; the settings page is the only reader.
    pub(crate) settings_credential_status: Signal<Option<(CredentialStatusKey, CredentialStatus)>>,
    pub usage_counts: Signal<UsageAggregate>,
    pub usage_inspection_open: Signal<bool>,
    pub recovery_records: Signal<Vec<RecoveredRecord>>,
    pub context_open: Signal<bool>,
    pub context_resolved: Signal<Option<ResolvedContext>>,
    pub context_selected: Signal<Vec<usize>>,
    pub context_error: Signal<Option<SharedString>>,
    pub context_busy: Signal<bool>,
    pub harness_skills: Signal<Vec<Skill>>,
    pub harness_instructions: Signal<Vec<Instruction>>,
    pub review_open: Signal<bool>,
    pub review_consent: Signal<Option<SharedString>>,
    /// The waiting Review consent's structured disclosure and manifest.
    pub review_manifest: Signal<Option<OutboundConsent>>,
    pub review_view: Signal<Option<ReviewView>>,
    pub review_busy: Signal<bool>,
    /// The source column's camera ([`DocumentFocus`]): the line a docked
    /// surface wants levelled with it. Set it with
    /// [`AppState::focus_document_line`].
    pub document_focus: Signal<DocumentFocus>,
    /// Why the last Review attempt could not be prepared, when it could not:
    /// the field explains it and offers Settings. `None` normally.
    pub review_unavailable: Signal<Option<ReviewUnavailable>>,
    /// Which Review an in-flight request belongs to. Bumped when a review is
    /// prepared and when one is withdrawn: a task whose generation is stale
    /// writes nothing, so a cancelled (or replaced) run can never show its
    /// result.
    pub review_generation: Signal<u64>,
    pub translation_busy: Signal<bool>,
    pub revision_questions: Signal<Vec<RevisionQuestionView>>,
    pub revision_consent: Signal<Option<SharedString>>,
    /// The waiting Revision consent's structured disclosure and manifest.
    pub revision_manifest: Signal<Option<OutboundConsent>>,
    pub revision_hunks: Signal<Vec<RevisionHunkView>>,
    pub revision_status: Signal<Option<SharedString>>,
    pub revision_busy: Signal<bool>,
    /// The rail's preview slot (chrome lane): the tab a single press in the
    /// rail's file list opened, which the next single press replaces, until a
    /// double press pins it. `None` when nothing is slotted.
    pub preview_slot: Signal<Option<u64>>,
    /// The destructive close or quit guard (`ui::notices`): up while a
    /// decision over dirty buffers is waiting.
    pub guard: Signal<Option<GuardRequest>>,
    /// The tab whose disk comparison (`ui::notices`) is open, if any.
    pub conflict_compare: Signal<Option<u64>>,
    /// Which difference the open comparison shows (`ui::notices`).
    pub conflict_difference: Signal<usize>,
    /// The file-on-disk text of each conflicted document (`ui::notices`),
    /// keyed by its tab and read when the comparison opens. A tab's entry is
    /// ignored while its `conflict` is false.
    pub conflict_disks: Signal<Vec<(u64, String)>>,
    owner: Owner,
    vault: CredentialVault,
    usage: Rc<RefCell<UsageStore>>,
    pending_review: Rc<RefCell<Option<PreparedDocumentReviewRequest>>>,
    review_origin: Rc<RefCell<Option<ReviewOrigin>>>,
    review_binding: Rc<RefCell<Option<ReviewOrigin>>>,
    revision_context: Rc<RefCell<Option<RevisionContext>>>,
    pending_revision: Rc<RefCell<Option<PreparedRevisionRequest>>>,
    revision_proposal: Rc<RefCell<Option<RevisionProposal>>>,
    recovery: Option<Rc<RecoveryStore>>,
    recovery_tracks: Rc<RefCell<HashMap<u64, RecoveryTrack>>>,
    quit_discarded: Rc<Cell<bool>>,
    next_id: Rc<Cell<u64>>,
}

/// Checkpoint timing for one dirty tab.
struct RecoveryTrack {
    last_edit: Instant,
    last_checkpoint: Option<Instant>,
}

/// One clarification answer bound to a signal the panel edits.
#[derive(Clone, PartialEq)]
pub struct RevisionQuestionView {
    pub ordinal: usize,
    pub question: String,
    pub priority: String,
    pub answer: Signal<String>,
}

/// One proposed edit (or group of edits sharing a change id) for diff review.
#[derive(Clone, PartialEq)]
pub struct RevisionHunkView {
    pub ordinal: usize,
    pub change_id: u32,
    pub rationale: String,
    pub before: String,
    pub after: String,
    pub accepted: Signal<bool>,
}

/// One destructive decision the guard is asking about (`ui::notices`).
#[derive(Clone, PartialEq)]
pub struct GuardRequest {
    /// The tab being closed; `None` quits the window.
    pub tab: Option<u64>,
    /// Which control the keyboard is on (`ui::notices`): `0` Save, `1`
    /// Don't save, `2` Cancel (the default).
    pub choice: Signal<usize>,
    /// A save is going on: the guard's controls wait for it.
    pub busy: Signal<bool>,
    /// Why the guard is still up (a save that could not finish).
    pub notice: Signal<Option<SharedString>>,
}

/// The guard's controls, in the order its card draws them (`ui::notices`).
pub const GUARD_SAVE_CHOICE: usize = 0;
pub const GUARD_DISCARD_CHOICE: usize = 1;
pub const GUARD_CANCEL_CHOICE: usize = 2;
/// How many controls the guard has.
pub const GUARD_CHOICES: usize = 3;
/// Where one outbound item came from, for the consent card to name it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OutboundOrigin {
    /// The reviewed document itself.
    Document,
    /// One named Effective Agent Context source.
    NamedSource,
    /// One Agent Skill package file, whose own reason travels with it.
    PackageFile,
    /// A file the package would normally include but does not send.
    Omission,
    /// A resolved Effective Agent Context source that is not in the scope.
    ExcludedSource,
    /// The Review a Revision request carries as context.
    ReviewContext,
    /// The clarification answers a Revision request carries.
    Answers,
}

/// One item of a consent's outbound manifest: what leaves the device, why, and
/// its own bytes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OutboundItem {
    pub origin: OutboundOrigin,
    /// The item's path, relative to the workspace root when it is inside it.
    /// `None` for an item that is not a file (a request's framing).
    pub path: Option<String>,
    /// The package's own inclusion reason, when the item has one.
    pub reason: Option<String>,
    /// The item's own byte size.
    pub bytes: u64,
    /// Whether the raw content crosses the boundary. False for a package entry
    /// that leaves as path, size and hash only.
    pub content_leaves: bool,
}

/// What one waiting consent discloses: the structured disclosure the send will
/// be bound to, plus the manifest the card's rows are built from.
///
/// Built where the request is bound and from the same objects that will
/// authorize the send, so no number on the card is summarized or estimated.
#[derive(Clone, Debug, PartialEq)]
pub struct OutboundConsent {
    pub disclosure: ModelRequestDisclosure,
    /// The framed request's bytes, when mt-core computed them at bind time:
    /// the canonical review frame for a document or a document with Effective
    /// Agent Context, the framed payload for an Agent Skill package. `None`
    /// when only the items are known (a Revision).
    pub framed_bytes: Option<u64>,
    /// What leaves and why, in display order.
    pub items: Vec<OutboundItem>,
    /// Normally in-scope items that do not leave, in display order.
    pub stays: Vec<OutboundItem>,
}

/// Immutable inputs retained after a successful Review for a later Revision.
struct RevisionContext {
    tab_id: u64,
    snapshot: SourceSnapshot,
    request: ReviewRequest,
    output: ReviewModelOutput,
    language: ReviewLanguage,
}

/// The tab and source state a prepared Review belongs to.
#[derive(Clone, Copy)]
struct ReviewOrigin {
    tab_id: u64,
    snapshot: SourceSnapshot,
}

/// A Review result reduced to the strings the panel shows.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct ReviewView {
    pub status: String,
    pub diagnostic: Option<String>,
    pub sections: Vec<ReviewSection>,
    pub findings: Vec<String>,
    pub questions: Vec<String>,
    /// Whether the source moved on after this result: the field says so, and
    /// the column keeps only the afterimage.
    pub stale: bool,
    /// The findings with their anchors intact: the field writes their line
    /// ranges and the column lights the anchored words.
    pub findings_anchored: Vec<ReviewFindingView>,
    /// The questions with priority and impact kept apart: the field orders
    /// them and shows one at a time.
    pub questions_detail: Vec<ReviewQuestionView>,
    /// The tab this result belongs to; the column lights findings only in
    /// that tab's text.
    pub tab_id: Option<u64>,
    /// Why the review failed, when it did: the field writes the plain
    /// diagnostic line the spec asks for.
    pub failure: Option<ReviewFailure>,
}

/// The column's camera: the document line a surface asked the source editor to
/// level with its docked reading position (a review's racked finding, a
/// revision's current hunk). The document lane's `ui::document` scrolls to it;
/// nothing here changes the document.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct DocumentFocus {
    /// The tab the line belongs to; another tab's request is ignored.
    pub tab: u64,
    /// The line, from 0.
    pub line: u32,
    /// Bumped by every request, so asking again for the same line levels it
    /// again.
    pub request: u64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ReviewSection {
    pub title: String,
    pub items: Vec<String>,
}

/// One grounded finding as the review field and the document's light need it.
///
/// `stated` decides where the light is: a stated finding makes the anchored
/// glyphs themselves emit, an inferred one makes the air beside them. The
/// location is the model's anchor as the core resolved it, so neither surface
/// can invent a source position.
#[derive(Debug, Clone, PartialEq)]
pub struct ReviewFindingView {
    pub stated: bool,
    pub text: String,
    pub location: Option<SourceLocation>,
}

/// One clarification question with its priority and impact kept apart.
#[derive(Debug, Clone, PartialEq)]
pub struct ReviewQuestionView {
    pub priority: ClarificationPriority,
    pub question: String,
    pub impact: Option<String>,
}

/// Why Review could not be prepared, so nothing left the device.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReviewUnavailable {
    /// No model provider or credential is configured.
    NotSetUp,
    /// The document or its context cannot form a review request.
    Unpreparable,
}

/// Why a sent Review produced no result, as the field's copy names it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReviewFailure {
    /// The provider did not answer in time.
    Timeout,
    /// The answer could not be read as a review.
    Malformed,
    /// The wait was cancelled; nothing changed.
    Cancelled,
    /// Anything else, shown with the localized status and the detail.
    Other,
}

/// One content-search row.
#[derive(Debug, Clone, PartialEq)]
pub struct SearchHit {
    pub path: PathBuf,
    pub label: String,
}

impl AppState {
    pub fn new() -> Self {
        Self::build(CredentialVault::production())
    }

    /// The state with the credential vault given: a test passes its own
    /// in-memory vault, so nothing it does can read, write or delete a
    /// credential in the platform's secure store.
    #[cfg(test)]
    pub(crate) fn with_vault(vault: CredentialVault) -> Self {
        Self::build(vault)
    }

    /// The constructor's body, with the one choice a test overrides made a
    /// parameter.
    fn build(vault: CredentialVault) -> Self {
        let settings = SettingsData::load();
        set_color_scheme(match settings.theme {
            ThemePreference::System => nsh_core::platform::Appearance::System,
            ThemePreference::Light => nsh_core::platform::Appearance::Light,
            ThemePreference::Dark => nsh_core::platform::Appearance::Dark,
        });
        let (recovery, records, notice) = match RecoveryStore::open() {
            Ok((store, _maintenance)) => match store.recover() {
                Ok(scan) => {
                    let notice = (!scan.issues.is_empty()).then(|| {
                        SharedString::from(format!(
                            "{} recovery record(s) could not be read.",
                            scan.issues.len()
                        ))
                    });
                    (Some(Rc::new(store)), scan.records, notice)
                }
                Err(error) => (
                    Some(Rc::new(store)),
                    Vec::new(),
                    Some(SharedString::from(format!("Recovery scan failed: {error}"))),
                ),
            },
            Err(error) => (
                None,
                Vec::new(),
                Some(SharedString::from(format!("Recovery unavailable: {error}"))),
            ),
        };
        let status = notice.clone().unwrap_or_else(|| {
            SharedString::from(crate::i18n::t(
                settings.language,
                crate::i18n::Key::OpenFolderToBegin,
            ))
        });
        let usage = UsageStore::open(settings.usage_metrics_enabled);
        let state = Self {
            owner: Owner::current(),
            root: signal(None),
            entries: signal(Vec::new()),
            tabs: signal(Vec::new()),
            active: signal(None),
            status: signal(status),
            search_query: signal(String::new()),
            search_results: signal(Vec::new()),
            search_ran: signal(false),
            search_truncated: signal(false),
            search_busy: signal(false),
            settings: signal(settings),
            settings_open: signal(false),
            settings_credential: signal(String::new()),
            settings_notice: signal(SharedString::from("")),
            settings_credential_revision: signal(0_u64),
            settings_credential_status: signal(None),
            usage_counts: signal(UsageAggregate::default()),
            usage_inspection_open: signal(false),
            recovery_records: signal(records),
            context_open: signal(false),
            context_resolved: signal(None),
            context_selected: signal(Vec::new()),
            context_error: signal(None),
            context_busy: signal(false),
            harness_skills: signal(Vec::new()),
            harness_instructions: signal(Vec::new()),
            review_open: signal(false),
            review_consent: signal(None),
            review_manifest: signal(None),
            review_view: signal(None),
            review_busy: signal(false),
            document_focus: signal(DocumentFocus::default()),
            review_unavailable: signal(None),
            translation_busy: signal(false),
            revision_questions: signal(Vec::new()),
            revision_consent: signal(None),
            revision_manifest: signal(None),
            revision_hunks: signal(Vec::new()),
            revision_status: signal(None),
            revision_busy: signal(false),
            preview_slot: signal(None),
            vault,
            review_generation: signal(0),
            guard: signal(None),
            conflict_compare: signal(None),
            conflict_difference: signal(0),
            conflict_disks: signal(Vec::new()),
            usage: Rc::new(RefCell::new(usage)),
            pending_review: Rc::new(RefCell::new(None)),
            review_origin: Rc::new(RefCell::new(None)),
            review_binding: Rc::new(RefCell::new(None)),
            revision_context: Rc::new(RefCell::new(None)),
            pending_revision: Rc::new(RefCell::new(None)),
            revision_proposal: Rc::new(RefCell::new(None)),
            recovery,
            recovery_tracks: Rc::new(RefCell::new(HashMap::new())),
            quit_discarded: Rc::new(Cell::new(false)),
            next_id: Rc::new(Cell::new(1)),
        };
        state.record_first_launch();
        state
    }

    fn record_first_launch(&self) {
        if !self.usage.borrow().enabled() || FIRST_LAUNCH_CHECKED.swap(true, Ordering::Relaxed) {
            return;
        }
        match self.usage.borrow().inspect(SystemTime::now()) {
            Ok(counts) => {
                if !counts
                    .days()
                    .values()
                    .any(|day| day.events.contains_key(&UsageEventKind::FirstLaunch))
                {
                    self.record_usage(UsageEvent::FirstLaunch);
                }
            }
            Err(_) => self.usage_notice(crate::i18n::Key::UsageUnavailable),
        }
    }

    fn record_usage(&self, event: UsageEvent) {
        if self
            .usage
            .borrow()
            .record(event, SystemTime::now())
            .is_err()
        {
            self.usage_notice(crate::i18n::Key::UsageUnavailable);
        }
        if self.settings_open.get() {
            self.refresh_usage_counts();
        }
    }

    fn usage_notice(&self, key: crate::i18n::Key) {
        self.settings_notice.set(SharedString::from(crate::i18n::t(
            self.settings.get().language,
            key,
        )));
    }

    fn refresh_usage_counts(&self) {
        match self.usage.borrow().inspect(SystemTime::now()) {
            Ok(counts) => self.usage_counts.set(counts),
            Err(_) => self.usage_notice(crate::i18n::Key::UsageUnavailable),
        }
    }

    pub fn set_usage_enabled(&self, enabled: bool) {
        let mut settings = self.settings.get();
        settings.usage_metrics_enabled = enabled;
        // A failed persistence attempt must never leave collection running
        // after a user asks to disable it, or enable it without durable consent.
        self.usage.borrow_mut().set_enabled(false);
        self.settings
            .update(|settings| settings.usage_metrics_enabled = false);
        match settings.try_save_usage_metrics_consent() {
            Ok(settings) => {
                self.usage
                    .borrow_mut()
                    .set_enabled(settings.usage_metrics_enabled);
                self.settings.set(settings);
                self.usage_notice(crate::i18n::Key::UsageConsentSaved);
            }
            Err(_) => self.usage_notice(crate::i18n::Key::UsageConsentSaveFailed),
        }
        self.refresh_usage_counts();
    }

    pub fn inspect_usage(&self) {
        self.refresh_usage_counts();
        self.usage_inspection_open
            .set(!self.usage_inspection_open.get());
    }

    pub fn clear_usage(&self) {
        match self.usage.borrow().clear() {
            Ok(()) => {
                self.usage_counts.set(UsageAggregate::default());
                self.usage_notice(crate::i18n::Key::UsageCleared);
            }
            Err(_) => self.usage_notice(crate::i18n::Key::UsageUnavailable),
        }
    }

    pub fn export_usage(&self) {
        let state = self.clone();
        let language = self.settings.get().language;
        self.spawn_task(async move {
            let picked = pick_save_path(
                SaveDialog::new()
                    .title(crate::i18n::t(language, crate::i18n::Key::UsageExport))
                    .file_name(crate::i18n::t(
                        language,
                        crate::i18n::Key::UsageExportFileName,
                    ))
                    .filter(nsh_core::file_dialog::FileFilter::new(
                        crate::i18n::t(language, crate::i18n::Key::UsageExportFormat),
                        ["json"],
                    )),
            )
            .await;
            match picked {
                Ok(Some(path)) => {
                    let result = state.usage.borrow().export_to(&path, SystemTime::now());
                    state.usage_notice(if result.is_ok() {
                        crate::i18n::Key::UsageExported
                    } else {
                        crate::i18n::Key::UsageExportFailed
                    });
                }
                Ok(None) => {}
                Err(_) => state.usage_notice(crate::i18n::Key::UsageExportFailed),
            }
        });
    }

    fn record_review_failure(&self, error: &ReviewError) {
        match error {
            ReviewError::CredentialAccess { .. }
            | ReviewError::MissingCredential { .. }
            | ReviewError::NoAvailableCredential => {
                self.record_usage(UsageEvent::CredentialFailure)
            }
            ReviewError::UnsupportedConfiguredProvider
            | ReviewError::InvalidEndpoint { .. }
            | ReviewError::Timeout { .. }
            | ReviewError::TransportUnavailable { .. }
            | ReviewError::RequestFailed { .. }
            | ReviewError::MissingResponseText { .. }
            | ReviewError::ResponseTooLarge { .. }
            | ReviewError::MalformedResponse { .. } => {
                self.record_usage(UsageEvent::ProviderFailure)
            }
            _ => {}
        }
    }

    fn record_translation_failure(&self, error: &TranslationError) {
        match error {
            TranslationError::CredentialAccess { .. }
            | TranslationError::MissingCredential { .. }
            | TranslationError::NoAvailableCredential => {
                self.record_usage(UsageEvent::CredentialFailure)
            }
            TranslationError::UnsupportedConfiguredProvider
            | TranslationError::InvalidEndpoint { .. }
            | TranslationError::TransportUnavailable { .. }
            | TranslationError::RequestFailed { .. }
            | TranslationError::MissingResponseText { .. }
            | TranslationError::InvalidTranslationResponse { .. } => {
                self.record_usage(UsageEvent::ProviderFailure)
            }
            _ => {}
        }
    }

    /// Reads `path` as the workspace root and lists it. The filesystem stays
    /// the source of truth: nothing here caches file contents.
    pub fn open_folder(&self, path: PathBuf) {
        match tree::read_dir_deep(&path, 4) {
            Ok(nodes) => {
                let mut entries = Vec::new();
                flatten(&nodes, 0, &mut entries);
                self.root.set(Some(path.clone()));
                self.entries.set(entries);
                self.reset_context();
                self.record_recent(&path, RecentTargetKind::Workspace);
                self.status.set(SharedString::from(format!(
                    "{} {}",
                    crate::i18n::t(self.settings.get().language, crate::i18n::Key::Opened),
                    path.display()
                )));
            }
            Err(error) => self
                .status
                .set(SharedString::from(format!("Open folder failed: {error}"))),
        }
    }

    /// Opens `path` in a tab, or activates the tab already showing it.
    pub fn open_file(&self, path: PathBuf) {
        if let Some(tab) = self.tabs.get().iter().find(|tab| tab.path == path) {
            self.set_active(tab.id);
            return;
        }

        let state = self.clone();
        self.spawn_task(async move {
            let loaded = match blocking(move || io::load(&path)).await {
                Ok(Ok(loaded)) => loaded,
                Ok(Err(error)) => {
                    state
                        .status
                        .set(SharedString::from(format!("Open failed: {error}")));
                    return;
                }
                Err(error) => {
                    state
                        .status
                        .set(SharedString::from(format!("Open failed: {error}")));
                    return;
                }
            };
            state.add_tab(loaded);
            state.record_usage(UsageEvent::ArtifactOpened);
        });
    }

    /// Opens `path` in the rail's preview slot (chrome lane): the tab the
    /// next single press replaces, unless a file it replaces was edited,
    /// saved as a new document or pinned. A tab already showing `path` is
    /// activated where it is, and stays whatever it was.
    pub fn open_preview(&self, path: PathBuf) {
        if let Some(tab) = self.tabs.get().iter().find(|tab| tab.path == path) {
            self.set_active(tab.id);
            return;
        }
        if let Some(slot) = self.preview_slot.get() {
            // A request in flight for the slot's tab keeps it: its answer
            // would land on a closed tab. The tab holding the model's live
            // output keeps it too — replacing that tab would drop a reading
            // nobody has seen.
            let waiting = self.active.get() == Some(slot)
                && (self.review_busy.get() || self.revision_busy.get());
            // Only an untouched slot tab may be replaced: its text is what
            // the file on disk holds, so nothing is lost.
            let replaceable = !waiting
                && !self.holds_live_model_output(slot)
                && self
                    .tabs
                    .get()
                    .iter()
                    .any(|tab| tab.id == slot && !tab.dirty.get() && !tab.untitled);
            if replaceable {
                self.close_tab_now(slot);
            }
        }
        self.preview_slot.set(None);
        let state = self.clone();
        self.spawn_task(async move {
            let loaded = match blocking(move || io::load(&path)).await {
                Ok(Ok(loaded)) => loaded,
                Ok(Err(error)) => {
                    state
                        .status
                        .set(SharedString::from(format!("Open failed: {error}")));
                    return;
                }
                Err(error) => {
                    state
                        .status
                        .set(SharedString::from(format!("Open failed: {error}")));
                    return;
                }
            };
            let known: Vec<u64> = state.tabs.get().iter().map(|tab| tab.id).collect();
            state.add_tab(loaded);
            state.record_usage(UsageEvent::ArtifactOpened);
            let opened = state
                .tabs
                .get()
                .iter()
                .map(|tab| tab.id)
                .find(|id| !known.contains(id));
            if let Some(id) = opened {
                state.preview_slot.set(Some(id));
            }
        });
    }

    /// Pins the preview slot's tab (a double press): the next single press
    /// opens beside it instead of replacing it.
    pub fn pin_tab(&self, id: u64) {
        self.preview_slot.update(|slot| {
            if *slot == Some(id) {
                *slot = None
            }
        });
    }

    /// Whether `id` is the tab the model's live output belongs to
    /// (additive, chrome lane): a review that has come back and whose text
    /// has not moved since, or a revision the model proposed. A request in
    /// flight has produced nothing yet, so it does not count.
    pub fn holds_live_model_output(&self, id: u64) -> bool {
        if self.active.get() != Some(id) {
            return false;
        }
        if !self.revision_hunks.get().is_empty() || !self.revision_questions.get().is_empty() {
            return true;
        }
        let Some(binding) = *self.review_binding.borrow() else {
            return false;
        };
        if binding.tab_id != id {
            return false;
        }
        // The same test `note_edit` uses to call a review stale: the text has
        // not moved on since the model read it.
        self.tabs
            .get()
            .iter()
            .find(|tab| tab.id == id)
            .is_some_and(|tab| tab.revision.get() == binding.snapshot.revision)
    }

    /// Start an empty document that does not have a source file yet.
    pub fn new_document(&self) {
        self.add_untitled(String::new());
    }

    /// Create a document from the clipboard's text.
    pub fn new_from_clipboard(&self) {
        let state = self.clone();
        self.spawn_task(async move {
            let text = clipboard_string().await;
            if text.trim().is_empty() {
                state.status.set(SharedString::from(
                    crate::i18n::t(
                        state.settings.get().language,
                        crate::i18n::Key::ClipboardTextUnavailable,
                    )
                    .to_owned(),
                ));
                return;
            }
            state.add_untitled(text);
            state.record_usage(UsageEvent::ArtifactPasted);
        });
    }

    fn add_untitled(&self, text: String) {
        let state = self.clone();
        self.owner.run(move || state.add_untitled_inner(text));
    }

    fn add_untitled_inner(&self, text: String) {
        let id = self.next_id.get();
        self.next_id.set(id + 1);
        let path = PathBuf::from(format!("Untitled-{id}.md"));
        let dirty = !text.is_empty();
        let loaded = LoadedFile {
            path: path.clone(),
            text: text.clone(),
            stamp: FileStamp {
                modified: None,
                len: 0,
                digest: [0; 32],
                object_id: None,
            },
            skill_origin: None,
            newline: Newline::Lf,
            had_bom: false,
            encoding: encoding_rs::UTF_8,
            decode_had_errors: false,
            source_identity: SourceIdentity::Regular,
        };
        let title = SharedString::from(crate::i18n::t(
            self.settings.get().language,
            crate::i18n::Key::Untitled,
        ));
        let recovery_key = RecoveryKey::new_memory();
        let text = signal(SharedString::from(text));
        self.tabs.update(|tabs| {
            tabs.push(Tab {
                id,
                path,
                title,
                text,
                dirty: signal(dirty),
                revision: signal(0),
                pane: signal(Pane::Split),
                web_trusted: signal(false),
                conflict: signal(false),
                untitled: true,
                recovery_key,
                loaded: Rc::new(RefCell::new(loaded)),
            });
        });
        if dirty {
            self.note_edit(id);
        }
        self.set_active(id);
    }

    fn add_tab(&self, loaded: LoadedFile) {
        self.add_tab_with(loaded, false);
    }

    fn add_tab_with(&self, loaded: LoadedFile, dirty: bool) {
        let id = self.next_id.get();
        self.next_id.set(id + 1);

        let title = loaded
            .path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("Untitled")
            .to_owned();
        let text = signal(SharedString::from(loaded.text.clone()));
        let path = loaded.path.clone();
        let recovery_key = RecoveryKey::for_path(&path);
        self.record_recent(&path, RecentTargetKind::File);
        if dirty {
            self.note_edit(id);
        }
        self.tabs.update(|tabs| {
            tabs.push(Tab {
                id,
                path,
                title: SharedString::from(title),
                text,
                dirty: signal(dirty),
                revision: signal(0),
                pane: signal(Pane::Split),
                web_trusted: signal(false),
                conflict: signal(false),
                untitled: false,
                recovery_key,
                loaded: Rc::new(RefCell::new(loaded)),
            });
        });
        self.set_active(id);
    }

    /// Run app-owned async work with the window's lifetime. A `spawn` from a
    /// button handler otherwise inherits that transient element owner and is
    /// cancelled as soon as the handler's view is replaced.
    fn spawn_task<F>(&self, future: F)
    where
        F: Future<Output = ()> + 'static,
    {
        self.owner.run(|| {
            spawn(future);
        });
    }

    pub fn active_tab(&self) -> Option<Tab> {
        let active = self.active.get()?;
        self.tabs.get().iter().find(|tab| tab.id == active).cloned()
    }

    pub fn set_active(&self, id: u64) {
        let changed = self.active.get() != Some(id);
        if changed {
            self.reset_context();
        }
        self.active.set(Some(id));
        if changed && self.context_open.get() {
            self.refresh_context();
        }
    }

    fn reset_context(&self) {
        self.context_resolved.set(None);
        self.context_selected.set(Vec::new());
        self.context_error.set(None);
        self.harness_skills.set(Vec::new());
        self.harness_instructions.set(Vec::new());
    }

    pub fn toggle_context(&self) {
        let open = !self.context_open.get();
        self.context_open.set(open);
        if open {
            self.close_review();
            self.refresh_context();
        }
    }

    pub fn close_context(&self) {
        self.context_open.set(false);
    }

    /// Resolve the local Effective Agent Context chain for the active document.
    pub fn refresh_context(&self) {
        let Some(root) = self.root.get() else {
            self.context_error
                .set(Some(SharedString::from("Open a folder first.")));
            return;
        };
        let target = self
            .active_tab()
            .map(|tab| tab.path)
            .unwrap_or_else(|| root.clone());
        let cwd = target
            .parent()
            .filter(|parent| parent.starts_with(&root))
            .map(Path::to_path_buf)
            .unwrap_or_else(|| root.clone());
        let codex_home_env = std::env::var_os("CODEX_HOME").filter(|value| !value.is_empty());
        let codex_home_override = codex_home_env.is_some();
        let codex_home = codex_home_env
            .map(PathBuf::from)
            .or_else(|| dirs::home_dir().map(|home| home.join(".codex")))
            .unwrap_or_default();
        let scan_root = root.clone();
        let input = ContextInput {
            workspace: root,
            target,
            cwd,
            profile_id: CODEX_PROFILE_ID.to_owned(),
            codex_home,
            codex_home_override,
            fallback_filenames: Vec::new(),
            project_doc_max_bytes: 32_768,
            project_root_markers: vec![".git".to_owned()],
            project_trust: ProjectTrust::Unspecified,
        };
        self.context_busy.set(true);
        self.context_error.set(None);
        let state = self.clone();
        self.spawn_task(async move {
            let scanned = blocking(move || {
                let skills = skill::discover_with(&scan_root, Discovery::WORKSPACE);
                let instructions = instruction::discover(&scan_root);
                (resolve(&input, &skills), skills, instructions)
            })
            .await;
            state.context_busy.set(false);
            match scanned {
                Ok((resolved, skills, instructions)) => {
                    state.context_selected.set(Vec::new());
                    state.context_resolved.set(Some(resolved));
                    state.harness_skills.set(skills);
                    state.harness_instructions.set(instructions);
                }
                Err(error) => state.context_error.set(Some(SharedString::from(format!(
                    "Context resolution failed: {error}"
                )))),
            }
        });
    }

    pub fn toggle_context_source(&self, index: usize) {
        self.context_selected.update(|selected| {
            if let Some(position) = selected.iter().position(|selected| *selected == index) {
                selected.remove(position);
            } else {
                selected.push(index);
                selected.sort_unstable();
            }
        });
    }

    pub fn review_with_context(&self) {
        self.context_open.set(false);
        self.start_review();
    }

    /// Reopen a Welcome recent target.
    pub fn open_recent(&self, target: &RecentTarget) {
        match target.kind {
            RecentTargetKind::Workspace => self.open_folder(target.path.clone()),
            RecentTargetKind::File => {
                if let Some(parent) = target.path.parent() {
                    self.open_folder(parent.to_path_buf());
                }
                self.open_file(target.path.clone());
            }
        }
    }

    pub fn open_bundled_sample(&self) {
        match crate::app_paths::bundled_sample_dir() {
            Ok(directory) => self.open_folder(directory),
            Err(error) => self
                .status
                .set(SharedString::from(format!("Sample unavailable: {error}"))),
        }
    }

    /// Ask the source column to level `line` (from 0) of tab `tab_id` with
    /// its docked reading position — the review's racked finding, a
    /// revision's current hunk. The document lane's `ui::document` does the
    /// scrolling, so the anchored lines and the field's docked explanation
    /// sit level; the document itself is never touched.
    pub fn focus_document_line(&self, tab_id: u64, line: u32) {
        let request = self.document_focus.get().request.wrapping_add(1);
        self.document_focus.set(DocumentFocus {
            tab: tab_id,
            line,
            request,
        });
    }

    /// Remember that a tab was edited, so its next checkpoint waits for a
    /// short idle window instead of copying on every keystroke.
    pub fn note_edit(&self, id: u64) {
        let now = Instant::now();
        self.recovery_tracks
            .borrow_mut()
            .entry(id)
            .and_modify(|track| track.last_edit = now)
            .or_insert(RecoveryTrack {
                last_edit: now,
                last_checkpoint: None,
            });
        let changed = self.review_binding.borrow().is_some_and(|binding| {
            binding.tab_id == id
                && self
                    .tabs
                    .get()
                    .iter()
                    .find(|tab| tab.id == id)
                    .is_none_or(|tab| tab.revision.get() != binding.snapshot.revision)
        });
        if changed && let Some(mut view) = self.review_view.get() {
            view.status =
                crate::i18n::t(self.settings.get().language, crate::i18n::Key::ReviewStale)
                    .to_owned();
            view.stale = true;
            self.review_view.set(Some(view));
        }
    }

    /// The checkpoint identity and metadata for one tab. Untitled drafts use a
    /// session-unique memory key and omit a source path so recovery never
    /// invents a file that was never saved.
    fn checkpoint_for_tab(tab: &Tab) -> RecoveryCheckpoint {
        let loaded = tab.loaded.borrow();
        let metadata = if tab.untitled {
            RecoveryMetadata {
                source_path: None,
                encoding_name: loaded.encoding.name().to_owned(),
                had_bom: loaded.had_bom,
                newline: loaded.newline,
                original_stamp: loaded.stamp.clone(),
                source_identity: loaded.source_identity.clone(),
                decode_had_errors: loaded.decode_had_errors,
            }
        } else {
            RecoveryMetadata::from_loaded_file(&loaded)
        };
        RecoveryCheckpoint {
            key: tab.recovery_key.clone(),
            text: tab.text.get().to_string(),
            metadata,
            revision: None,
        }
    }

    /// Write idle or interval-due dirty buffers. Checkpoints are best effort:
    /// a failure is surfaced, never allowed to block editing or saving.
    pub fn checkpoint_tick(&self) {
        let Some(store) = self.recovery.as_ref() else {
            return;
        };
        let tabs: Vec<Tab> = self
            .tabs
            .get()
            .iter()
            .filter(|tab| tab.dirty.get())
            .cloned()
            .collect();
        if tabs.is_empty() {
            return;
        }
        let active_keys: HashSet<RecoveryKey> =
            tabs.iter().map(|tab| tab.recovery_key.clone()).collect();
        let now = Instant::now();
        let mut failure = None;
        for tab in tabs {
            let should_write = {
                let mut tracks = self.recovery_tracks.borrow_mut();
                let track = tracks.entry(tab.id).or_insert(RecoveryTrack {
                    last_edit: now,
                    last_checkpoint: None,
                });
                let idle = now.saturating_duration_since(track.last_edit) >= Duration::from_secs(2);
                let due = track.last_checkpoint.is_none_or(|last| {
                    now.saturating_duration_since(last) >= Duration::from_secs(10)
                });
                if idle || due {
                    track.last_checkpoint = Some(now);
                    true
                } else {
                    false
                }
            };
            if !should_write {
                continue;
            }
            let checkpoint = Self::checkpoint_for_tab(&tab);
            if let Err(error) = store.checkpoint(&checkpoint, &active_keys) {
                failure = Some(error.to_string());
            }
        }
        if let Some(error) = failure {
            self.status.set(SharedString::from(format!(
                "Recovery checkpoint failed: {error}"
            )));
        }
    }

    /// Best-effort checkpoint of every dirty buffer before the guarded quit
    /// dialog. This is not a substitute for the Save / Discard / Cancel
    /// decision; it only keeps the latest text recoverable if the dialog or
    /// save path fails.
    fn checkpoint_dirty_now(&self) {
        if self.quit_discarded.get() {
            return;
        }
        let Some(store) = self.recovery.as_ref() else {
            return;
        };
        let tabs: Vec<Tab> = self
            .tabs
            .get()
            .iter()
            .filter(|tab| tab.dirty.get())
            .cloned()
            .collect();
        if tabs.is_empty() {
            return;
        }
        let active_keys: HashSet<RecoveryKey> =
            tabs.iter().map(|tab| tab.recovery_key.clone()).collect();
        for tab in tabs {
            let checkpoint = Self::checkpoint_for_tab(&tab);
            let _ = store.checkpoint(&checkpoint, &active_keys);
        }
    }

    /// Delete one checkpoint once its text is saved or the tab is closed
    /// deliberately.
    fn retire_recovery_key(&self, key: &RecoveryKey) {
        if let Some(store) = &self.recovery {
            let _ = store.invalidate_and_delete(key);
        }
    }

    fn retire_tab_recovery(&self, tab: &Tab) {
        self.retire_recovery_key(&tab.recovery_key);
    }

    /// Reopen a recovered buffer as a dirty tab. Its original stamp is kept so
    /// Save still refuses to overwrite a file changed since the checkpoint.
    pub fn restore_recovered(&self, record: &RecoveredRecord) {
        let key = record.record.key.clone();
        let untitled = record.record.metadata.source_path.is_none();
        let loaded = recovered_loaded(record);
        let recovered_path = loaded.path.clone();
        self.add_tab_with(loaded, true);
        if untitled {
            if let Some(id) = self.active.get() {
                self.tabs.update(|tabs| {
                    if let Some(tab) = tabs.iter_mut().find(|tab| tab.id == id) {
                        tab.untitled = true;
                        tab.title = SharedString::from(crate::i18n::t(
                            self.settings.get().language,
                            crate::i18n::Key::Untitled,
                        ));
                        tab.recovery_key = key.clone();
                    }
                });
            }
            self.settings.update(|settings| {
                settings
                    .recent_targets
                    .retain(|target| target.path != recovered_path);
            });
        }
        if record.source_conflicted {
            if let Some(tab) = self.active_tab() {
                tab.conflict.set(true);
            }
            self.status.set(SharedString::from(
                "Recovered text differs from disk; reload, overwrite, or save as a new file.",
            ));
        }
        self.recovery_records
            .update(|records| records.retain(|record| record.record.key != key));
        self.record_usage(UsageEvent::RecoveryRestored);
    }

    pub fn discard_recovered(&self, record: &RecoveredRecord) {
        if let Some(store) = &self.recovery {
            let _ = store.invalidate_and_delete(&record.record.key);
        }
        let key = record.record.key.clone();
        self.recovery_records
            .update(|records| records.retain(|record| record.record.key != key));
    }

    pub fn restore_all_recovered(&self) {
        for record in self.recovery_records.get() {
            self.restore_recovered(&record);
        }
    }

    pub fn discard_all_recovered(&self) {
        for record in self.recovery_records.get() {
            self.discard_recovered(&record);
        }
    }

    fn record_recent(&self, path: &Path, kind: RecentTargetKind) {
        let name = path
            .file_name()
            .and_then(|name| name.to_str())
            .map(str::to_owned)
            .unwrap_or_else(|| path.display().to_string());
        self.settings.update(|settings| {
            settings.record_recent_target(RecentTarget::new(path.to_path_buf(), kind, name));
        });
        let _ = self.settings.get().try_save_general();
    }

    /// Search the workspace's documents for `text`. Runs the walk and the
    /// reads on a blocking thread; the result is content, never a file write.
    pub fn run_search(&self, text: &str) {
        let query = Query::new(text.to_owned());
        self.search_ran.set(true);
        self.search_results.set(Vec::new());
        self.search_truncated.set(false);
        if !query.is_runnable() {
            self.search_busy.set(false);
            return;
        }
        let Some(root) = self.root.get() else {
            self.status
                .set(SharedString::from("Open a folder before searching."));
            return;
        };
        self.search_busy.set(true);
        let state = self.clone();
        self.spawn_task(async move {
            let result = blocking(move || {
                let paths = search::document_paths(&root);
                let mut results = search::Results::default();
                search::search_files(&paths, &query, search::DEFAULT_LIMIT, &mut results);
                results
            })
            .await;
            state.search_busy.set(false);
            match result {
                Ok(results) => {
                    let hits = results
                        .matches
                        .iter()
                        .map(|hit| SearchHit {
                            path: hit.path.as_ref().clone(),
                            label: format!(
                                "{}:{}:{}  {}",
                                hit.path.display(),
                                hit.line,
                                hit.column,
                                hit.line_text.trim()
                            ),
                        })
                        .collect();
                    state.search_results.set(hits);
                    state.search_truncated.set(results.truncated);
                }
                Err(error) => state
                    .status
                    .set(SharedString::from(format!("Search failed: {error}"))),
            }
        });
    }

    pub fn clear_search(&self) {
        self.search_query.set(String::new());
        self.search_results.set(Vec::new());
        self.search_ran.set(false);
        self.search_truncated.set(false);
    }

    /// Ask before discarding unsaved text. Saving is attempted first; a failed
    /// save or a still-dirty buffer keeps the tab open.
    pub fn request_close_tab(&self, id: u64) {
        let dirty = self
            .tabs
            .get()
            .iter()
            .find(|tab| tab.id == id)
            .is_some_and(|tab| tab.dirty.get());
        if !dirty {
            self.close_tab_now(id);
            return;
        }
        // The in-app guard takes the decision (`ui::notices`): Save, Don't
        // save or Cancel, with Cancel the default.
        self.open_guard(Some(id));
    }

    /// Show the destructive close or quit guard for `tab` (`None` quits the
    /// window).
    fn open_guard(&self, tab: Option<u64>) {
        // The guard outlives the control that asked for it (the card replaces
        // the tab's own row), so its signals are the app's.
        let (choice, busy, notice) = self
            .owner
            .run(|| (signal(GUARD_CANCEL_CHOICE), signal(false), signal(None)));
        self.guard.set(Some(GuardRequest {
            tab,
            choice,
            busy,
            notice,
        }));
    }

    /// Read `tab`'s file for the conflict comparison (`ui::notices`),
    /// replacing any earlier text for it. The read is best-effort: a file
    /// that cannot be read leaves the comparison empty and the other
    /// decisions in place.
    pub fn refresh_conflict_disk(&self, tab: &Tab) {
        let disk = io::load(&tab.path).ok().map(|loaded| loaded.text);
        let id = tab.id;
        self.conflict_disks.update(|disks| {
            disks.retain(|(entry, _)| *entry != id);
            if let Some(disk) = disk {
                disks.push((id, disk));
            }
        });
    }

    /// Run `f` under the state's own owner: what it makes (a new tab's
    /// signals, a new guard's) then belongs to the app, not to the element
    /// whose handler asked. A handler in a row that the action itself
    /// removes (a recovered record's Restore) would otherwise take the
    /// document's signals down with its owner.
    pub fn owned<R>(&self, f: impl FnOnce() -> R) -> R {
        self.owner.run(f)
    }

    /// The guard's selection: `delta` steps of [`GUARD_CHOICES`], wrapping.
    pub fn guard_move(&self, delta: isize) {
        let Some(request) = self.guard.get() else {
            return;
        };
        let current = request.choice.get();
        let next = (current as isize + delta).rem_euclid(GUARD_CHOICES as isize) as usize;
        request.choice.set(next);
    }

    /// The guard's Enter: the selected control's decision.
    pub fn guard_activate(&self) {
        let Some(request) = self.guard.get() else {
            return;
        };
        match request.choice.get() {
            GUARD_DISCARD_CHOICE => self.guard_discard(),
            GUARD_CANCEL_CHOICE => self.guard_cancel(),
            _ => self.guard_save(),
        }
    }

    /// The guard's Save: save every document it covers, then finish the
    /// close. A save that cannot finish leaves the guard up with its notice.
    pub fn guard_save(&self) {
        let Some(request) = self.guard.get() else {
            return;
        };
        if request.busy.get() {
            return;
        }
        match request.tab {
            Some(id) => {
                self.set_active(id);
                let untitled = self
                    .tabs
                    .get()
                    .iter()
                    .find(|tab| tab.id == id)
                    .is_some_and(|tab| tab.untitled);
                if untitled {
                    // The picker is the next decision; nothing is discarded
                    // when it is cancelled.
                    self.guard.set(None);
                    self.save_active();
                    return;
                }
                let saved = self.save_tab(id);
                if saved {
                    self.guard.set(None);
                    self.close_tab_now(id);
                } else {
                    request.notice.set(Some(SharedString::from(
                        "Still open: the document could not be saved.",
                    )));
                }
            }
            None => {
                let state = self.clone();
                request.busy.set(true);
                request.notice.set(None);
                self.spawn_task(async move {
                    // Translation can dirty another tab while a save waits,
                    // and Cancel takes the guard away mid-flight.
                    let dirty: Vec<u64> = state
                        .tabs
                        .get()
                        .iter()
                        .filter(|tab| tab.dirty.get())
                        .map(|tab| tab.id)
                        .collect();
                    for id in dirty {
                        if state.guard.get().is_none() {
                            return;
                        }
                        let Some(tab) = state.tabs.get().iter().find(|tab| tab.id == id).cloned()
                        else {
                            continue;
                        };
                        let saved = if tab.untitled {
                            state.save_as_tab(tab).await
                        } else {
                            state.save_tab(id)
                        };
                        if !saved {
                            if let Some(request) = state.guard.get() {
                                request.busy.set(false);
                                request.notice.set(Some(SharedString::from(
                                    "Still open: a document could not be saved.",
                                )));
                            }
                            state.status.set(SharedString::from(
                                "Quit cancelled: a document could not be saved.",
                            ));
                            return;
                        }
                    }
                    if state.guard.get().is_some() {
                        state.guard.set(None);
                        nsh_core::element::desktop::current_window().close();
                    }
                });
            }
        }
    }

    /// The guard's Don't save: close the covered documents, retiring their
    /// recovery records.
    pub fn guard_discard(&self) {
        let Some(request) = self.guard.get() else {
            return;
        };
        self.guard.set(None);
        match request.tab {
            Some(id) => self.close_tab_now(id),
            None => {
                self.quit_discarded.set(true);
                // The same rule as Save, and the card lists the same
                // documents: whatever is dirty when the decision is made.
                let dirty: Vec<u64> = self
                    .tabs
                    .get()
                    .iter()
                    .filter(|tab| tab.dirty.get())
                    .map(|tab| tab.id)
                    .collect();
                for id in dirty {
                    if let Some(tab) = self.tabs.get().iter().find(|tab| tab.id == id) {
                        self.retire_tab_recovery(tab);
                    }
                }
                nsh_core::element::desktop::current_window().close();
            }
        }
    }

    /// The guard's Cancel: nothing changes and the guard goes.
    pub fn guard_cancel(&self) {
        if let Some(request) = self.guard.get() {
            request.busy.set(false);
            request.notice.set(None);
            self.guard.set(None);
        }
    }

    fn close_tab_now(&self, id: u64) {
        if let Some(tab) = self.tabs.get().iter().find(|tab| tab.id == id).cloned() {
            self.retire_tab_recovery(&tab);
        }
        self.recovery_tracks.borrow_mut().remove(&id);
        self.tabs.update(|tabs| tabs.retain(|tab| tab.id != id));
        if self.active.get() == Some(id) {
            let next = self.tabs.get().last().map(|tab| tab.id);
            match next {
                Some(id) => self.set_active(id),
                None => {
                    self.active.set(None);
                    self.reset_context();
                }
            }
        }
    }

    /// Reload the active document from disk after an explicit discard decision.
    pub fn reload_active(&self) {
        let Some(tab) = self.active_tab() else {
            return;
        };
        let state = self.clone();
        self.spawn_task(async move {
            let answer = alert_dialog()
                .title("Reload from disk?")
                .description("This discards the current buffer and loads the file again.")
                .destructive("reload", "Reload")
                .cancel("cancel", "Cancel")
                .show()
                .await;
            if !answer.is_ok_and(|answer| answer.action.as_deref() == Some("reload")) {
                return;
            }
            let path = tab.path.clone();
            let loaded = match blocking(move || io::load(&path)).await {
                Ok(Ok(loaded)) => loaded,
                Ok(Err(error)) => {
                    state
                        .status
                        .set(SharedString::from(format!("Reload failed: {error}")));
                    return;
                }
                Err(error) => {
                    state
                        .status
                        .set(SharedString::from(format!("Reload failed: {error}")));
                    return;
                }
            };
            let text = loaded.text.clone();
            *tab.loaded.borrow_mut() = loaded;
            tab.text.set(SharedString::from(text));
            tab.dirty.set(false);
            tab.conflict.set(false);
            tab.revision.update(|revision| *revision += 1);
            state.retire_tab_recovery(&tab);
            state.recovery_tracks.borrow_mut().remove(&tab.id);
            state.status.set(SharedString::from(format!(
                "Reloaded {}",
                tab.path.display()
            )));
        });
    }

    /// Overwrite a changed file only after an explicit destructive decision.
    pub fn overwrite_active(&self) {
        let Some(tab) = self.active_tab() else {
            return;
        };
        let state = self.clone();
        self.spawn_task(async move {
            let answer = alert_dialog()
                .title("Overwrite disk changes?")
                .description(
                    "The file changed after it was opened. Overwriting discards those changes.",
                )
                .destructive("overwrite", "Overwrite")
                .cancel("cancel", "Cancel")
                .show()
                .await;
            if !answer.is_ok_and(|answer| answer.action.as_deref() == Some("overwrite")) {
                return;
            }
            let text = tab.text.get().to_string();
            let stamp = {
                let loaded = tab.loaded.borrow();
                io::save(&loaded, &text, true)
            };
            match stamp {
                Ok(stamp) => {
                    tab.loaded.borrow_mut().stamp = stamp;
                    tab.dirty.set(false);
                    tab.conflict.set(false);
                    state.retire_tab_recovery(&tab);
                    state.recovery_tracks.borrow_mut().remove(&tab.id);
                    state.record_usage(UsageEvent::ArtifactSaved);
                    state
                        .status
                        .set(SharedString::from(format!("Saved {}", tab.path.display())));
                }
                Err(error) => state
                    .status
                    .set(SharedString::from(format!("Overwrite failed: {error:?}"))),
            }
        });
    }

    /// Save the active buffer to a new path. An existing destination is never
    /// replaced here; the user must choose a path that does not exist yet.
    pub fn save_as_active(&self) {
        let Some(tab) = self.active_tab() else {
            return;
        };
        let state = self.clone();
        self.spawn_task(async move {
            state.save_as_tab(tab).await;
        });
    }

    async fn save_as_tab(&self, tab: Tab) -> bool {
        let picked = pick_save_path(
            SaveDialog::new()
                .title("Save As")
                .file_name(tab.title.to_string())
                .filter(nsh_core::file_dialog::FileFilter::new(
                    "Markdown",
                    ["md", "markdown"],
                )),
        );
        match picked.await {
            Ok(Some(path)) => {
                let text = tab.text.get().to_string();
                let (newline, had_bom) = {
                    let loaded = tab.loaded.borrow();
                    (loaded.newline, loaded.had_bom)
                };
                match io::save_as(&path, &text, newline, had_bom) {
                    Ok(loaded) => {
                        self.close_tab_now(tab.id);
                        self.add_tab(loaded);
                        self.record_usage(UsageEvent::ArtifactSaved);
                        true
                    }
                    Err(SaveError::DestinationExists) => {
                        self.status.set(SharedString::from(
                            "Save As refused: that file exists. Choose a new name.",
                        ));
                        false
                    }
                    Err(error) => {
                        self.status
                            .set(SharedString::from(format!("Save As failed: {error:?}")));
                        false
                    }
                }
            }
            Ok(None) => false,
            Err(error) => {
                self.status.set(SharedString::from(format!(
                    "Save As picker failed: {error}"
                )));
                false
            }
        }
    }

    pub fn set_pane(&self, id: u64, pane: Pane) {
        if let Some(tab) = self.tabs.get().iter().find(|tab| tab.id == id) {
            tab.pane.set(pane);
        }
    }

    /// Scripts in the Web pane need one explicit decision per tab.
    pub fn request_web_trust(&self, id: u64) {
        let state = self.clone();
        self.spawn_task(async move {
            let answer = alert_dialog()
                .title("Run scripts in this document?")
                .description(
                    "Trusted HTML and MDX may execute scripts inside the Web pane. Only trust content you opened deliberately.",
                )
                .destructive("trust", "Run scripts")
                .cancel("cancel", "Cancel")
                .show()
                .await;
            if answer.is_ok_and(|answer| answer.action.as_deref() == Some("trust")) {
                for tab in state.tabs.get().iter().filter(|tab| tab.id == id) {
                    tab.web_trusted.set(true);
                }
            }
        });
    }

    /// Saves the active document through the core's guarded commit. A conflict
    /// is reported, never silently overwritten.
    pub fn save_active(&self) {
        let Some(tab) = self.active_tab() else {
            return;
        };
        if tab.untitled {
            self.save_as_active();
            return;
        }
        self.save_tab(tab.id);
    }

    fn save_tab(&self, id: u64) -> bool {
        let Some(tab) = self.tabs.get().iter().find(|tab| tab.id == id).cloned() else {
            return false;
        };
        if tab.untitled {
            self.status.set(SharedString::from(
                "Save this untitled document with Save As first.",
            ));
            return false;
        }
        let text = tab.text.get().to_string();
        let saved = {
            let loaded = tab.loaded.borrow();
            io::save(&loaded, &text, false)
        };
        match saved {
            Ok(stamp) => {
                tab.loaded.borrow_mut().stamp = stamp;
                tab.dirty.set(false);
                tab.conflict.set(false);
                self.retire_tab_recovery(&tab);
                self.recovery_tracks.borrow_mut().remove(&tab.id);
                self.record_usage(UsageEvent::ArtifactSaved);
                self.status
                    .set(SharedString::from(format!("Saved {}", tab.path.display())));
                true
            }
            Err(SaveError::Conflict) => {
                tab.conflict.set(true);
                self.status.set(SharedString::from(crate::i18n::t(
                    self.settings.get().language,
                    crate::i18n::Key::SaveConflictDetail,
                )));
                false
            }
            Err(error) => {
                self.status
                    .set(SharedString::from(format!("Save failed: {error:?}")));
                false
            }
        }
    }

    /// Ask before discarding every dirty buffer, then close the window. This
    /// is the guarded close path available inside the app: the in-app guard
    /// (`ui::notices`) takes the decision. The platform title bar and Alt+F4
    /// reach the window without it — nsh 0.5.1 can intercept them with
    /// `WindowAttributes::close_on_press = false` and
    /// `WindowRef::on_close_press`, which `app.rs` does not wire yet — so the
    /// periodic checkpoint remains the recovery protection on that path. A
    /// dirty buffer is checkpointed before the guard as a best-effort extra
    /// guard against a failed save.
    pub fn request_quit(&self) {
        let dirty: Vec<u64> = self
            .tabs
            .get()
            .iter()
            .filter(|tab| tab.dirty.get())
            .map(|tab| tab.id)
            .collect();
        if dirty.is_empty() {
            nsh_core::element::desktop::current_window().close();
            return;
        }
        self.checkpoint_dirty_now();
        // The in-app guard takes the decision (`ui::notices`).
        self.open_guard(None);
    }

    /// Translate the active document through the consent-bound provider path.
    /// The editor text is replaced only if the originating tab and source
    /// revision still match after the response returns.
    pub fn translate_active(&self) {
        if self.translation_busy.get() {
            return;
        }
        let Some(tab) = self.active_tab() else {
            return;
        };
        let origin = ReviewOrigin {
            tab_id: tab.id,
            snapshot: SourceSnapshot::new(tab.revision.get(), 0),
        };
        let source = Document::new(Some(tab.path.clone()), tab.text.get().to_string());
        let request = TranslationRequest::prepare(&source, &Scope::Document);
        if request.inputs().is_empty() {
            self.status
                .set(SharedString::from("No translatable text in this document."));
            return;
        }

        let settings = self.settings.get();
        let language = settings.language;
        let target = if settings.translate_to.trim().is_empty() {
            "zh".to_owned()
        } else {
            settings.translate_to.trim().to_owned()
        };
        let prepared = match PreparedTranslation::from_settings(&settings, &self.vault) {
            Ok(prepared) => prepared,
            Err(error) => {
                self.record_translation_failure(&error);
                self.status.set(SharedString::from(format!(
                    "Translation unavailable: {error}"
                )));
                return;
            }
        };
        let prepared: PreparedTranslationRequest = prepared.bind_request(request);
        let disclosure = prepared.disclosure().clone();
        let description = crate::i18n::model_request_disclosure(language, &disclosure);
        let state = self.clone();
        self.spawn_task(async move {
            let answer = alert_dialog()
                .title(crate::i18n::t(language, crate::i18n::Key::Translate).to_owned())
                .description(description)
                .action(
                    "translate",
                    crate::i18n::t(language, crate::i18n::Key::SendToModel).to_owned(),
                )
                .cancel(
                    "cancel",
                    crate::i18n::t(language, crate::i18n::Key::Cancel).to_owned(),
                )
                .show()
                .await;
            if !answer.is_ok_and(|answer| answer.action.as_deref() == Some("translate")) {
                return;
            }

            let mut consent =
                ConsentCapability::from_decision(&disclosure, ConsentDecision::Approve);
            let authorization = match prepared.authorize(&mut consent) {
                Ok(authorization) => authorization,
                Err(error) => {
                    state.status.set(SharedString::from(format!(
                        "Translation unavailable: {error}"
                    )));
                    return;
                }
            };
            state.translation_busy.set(true);
            state.status.set(SharedString::from(
                crate::i18n::t(language, crate::i18n::Key::ModelRequestWaiting).to_owned(),
            ));
            let target_for_request = target.clone();
            let result =
                blocking(move || prepared.execute(authorization, &target_for_request)).await;
            state.translation_busy.set(false);
            match result {
                Ok(Ok(translation)) => {
                    let translated = translation.text;
                    let segments = translation
                        .segments
                        .iter()
                        .filter(|segment| segment.translatable)
                        .count();
                    let Some(tab) = state
                        .tabs
                        .get()
                        .iter()
                        .find(|tab| tab.id == origin.tab_id)
                        .cloned()
                    else {
                        state.record_usage(UsageEvent::StaleResultBlocked);
                        state.status.set(SharedString::from(
                            crate::i18n::t(language, crate::i18n::Key::ModelRequestDocumentClosed)
                                .to_owned(),
                        ));
                        return;
                    };
                    if origin.snapshot.revision != tab.revision.get() {
                        state.record_usage(UsageEvent::StaleResultBlocked);
                        state.status.set(SharedString::from(
                            crate::i18n::t(language, crate::i18n::Key::ModelRequestDocumentChanged)
                                .to_owned(),
                        ));
                        return;
                    }
                    tab.text.set(SharedString::from(translated));
                    tab.dirty.set(true);
                    tab.revision.update(|revision| *revision += 1);
                    state.note_edit(tab.id);
                    state.status.set(SharedString::from(format!(
                        "Translated {segments} segment(s) into {target}."
                    )));
                }
                Ok(Err(error)) => {
                    if let Some(error) = error.downcast_ref::<TranslationError>() {
                        state.record_translation_failure(error);
                    } else {
                        state.record_usage(UsageEvent::ProviderFailure);
                    }
                    state
                        .status
                        .set(SharedString::from(format!("Translation failed: {error}")));
                }
                Err(error) => state.status.set(SharedString::from(format!(
                    "Translation task failed: {error}"
                ))),
            }
        });
    }

    /// Prepare one Review: bind the frozen document request, resolve the
    /// configured endpoint and credential, and show the exact disclosure.
    /// Nothing leaves the machine until [`send_review`](Self::send_review).
    pub fn start_review(&self) {
        // A review that is prepared replaces any review still in flight: its
        // task writes nothing when it comes back, and the loading state it
        // left is not this run's to show — the early returns below must not
        // leave the field saying `Reviewing…`.
        self.review_generation.update(|generation| *generation += 1);
        self.review_busy.set(false);
        let Some(tab) = self.active_tab() else {
            return;
        };
        let origin = ReviewOrigin {
            tab_id: tab.id,
            snapshot: SourceSnapshot::new(tab.revision.get(), 0),
        };
        self.context_open.set(false);
        self.review_view.set(None);
        self.review_unavailable.set(None);
        self.review_consent.set(None);
        self.review_manifest.set(None);
        *self.review_origin.borrow_mut() = None;
        *self.review_binding.borrow_mut() = None;
        self.reset_revision();

        let settings = self.settings.get();
        let language = match settings.language {
            Language::Chinese => ReviewLanguage::SimplifiedChinese,
            Language::English => ReviewLanguage::English,
        };
        let prepared = match PreparedReview::from_settings(&settings, &self.vault) {
            Ok(prepared) => prepared,
            Err(error) => {
                self.record_review_failure(&error);
                self.review_unavailable.set(Some(match error {
                    ReviewError::UnsupportedConfiguredProvider
                    | ReviewError::InvalidEndpoint { .. }
                    | ReviewError::CredentialAccess { .. }
                    | ReviewError::MissingCredential { .. }
                    | ReviewError::NoAvailableCredential => ReviewUnavailable::NotSetUp,
                    _ => ReviewUnavailable::Unpreparable,
                }));
                self.status
                    .set(SharedString::from(format!("Review unavailable: {error}")));
                return;
            }
        };
        let text = tab.text.get().to_string();
        let built = build_review_request(
            ReviewRequestBuildRequest::new(
                ReviewTarget::Document,
                ArtifactLens::infer_from_path(&tab.path),
                Some(&tab.path),
                &text,
                None,
                SourceSnapshot::new(tab.revision.get(), 0),
                tab.dirty.get(),
            )
            .with_skill_origin(tab.loaded.borrow().skill_origin.as_ref()),
        );
        let mut request = match built {
            Ok(built) => built.into_parts().0,
            Err(error) => {
                self.review_unavailable
                    .set(Some(ReviewUnavailable::Unpreparable));
                self.status.set(SharedString::from(format!(
                    "Review request rejected: {error}"
                )));
                return;
            }
        };
        if let Some(context) = self.context_resolved.get() {
            let selected = self.context_selected.get();
            if !selected.is_empty() && context.input.target == tab.path {
                let selected = match context.select_sources(&selected) {
                    Ok(selected) => selected,
                    Err(error) => {
                        self.status.set(SharedString::from(format!(
                            "Context selection rejected: {error}"
                        )));
                        return;
                    }
                };
                request = match request.with_effective_agent_context(selected) {
                    Ok(request) => request,
                    Err(error) => {
                        self.status.set(SharedString::from(format!(
                            "Context request rejected: {error}"
                        )));
                        return;
                    }
                };
            }
        }
        let context_included = request.effective_agent_context.is_some();
        let bound = match prepared.bind_document_request(request, language) {
            Ok(bound) => bound,
            Err(error) => {
                self.status.set(SharedString::from(format!(
                    "Review request rejected: {error}"
                )));
                return;
            }
        };
        let disclosure =
            crate::i18n::model_request_disclosure(settings.language, bound.disclosure());
        let manifest = self.review_manifest_of(
            &bound,
            &tab.path,
            self.excluded_sources(&tab.path, context_included),
        );
        *self.pending_review.borrow_mut() = Some(bound);
        *self.review_origin.borrow_mut() = Some(origin);
        self.review_consent
            .set(Some(SharedString::from(disclosure)));
        self.review_manifest.set(Some(manifest));
        self.review_open.set(true);
    }

    /// Whether the waiting Review consent may also be bound with its selected
    /// Effective Agent Context (the `Document + context` scope).
    pub fn review_consent_offers_context(&self) -> bool {
        self.review_consent_target()
            .is_some_and(|target| self.context_candidate(&target).is_some())
    }

    /// The file the waiting Review consent was bound for, while its tab is
    /// still open.
    fn review_consent_target(&self) -> Option<PathBuf> {
        let origin = self.review_origin.borrow().as_ref().copied()?;
        self.tabs
            .get()
            .iter()
            .find(|tab| tab.id == origin.tab_id)
            .map(|tab| tab.path.clone())
    }

    /// The selected Effective Agent Context a binding for `target` may carry,
    /// when one is resolved and selected for that exact file. `Err` is the
    /// selection the request boundary rejects: the caller says so.
    fn context_selection(&self, target: &Path) -> Result<Option<SelectedContext>, String> {
        let Some(context) = self.context_resolved.get() else {
            return Ok(None);
        };
        if context.input.target != target {
            return Ok(None);
        }
        let selected = self.context_selected.get();
        if selected.is_empty() {
            return Ok(None);
        }
        context
            .select_sources(&selected)
            .map(Some)
            .map_err(|error| error.to_string())
    }

    /// [`Self::context_selection`] without the error: what the UI polls to
    /// know whether the other scope is bindable at all.
    fn context_candidate(&self, target: &Path) -> Option<SelectedContext> {
        self.context_selection(target).ok().flatten()
    }

    /// The frozen Review behind the waiting consent, stripped back to its
    /// pre-context form, with the file it was bound for and its language.
    fn review_base(&self) -> Option<(PathBuf, ReviewRequest, ReviewLanguage)> {
        let target = self.review_consent_target()?;
        let (mut request, language) = {
            let bound = self.pending_review.borrow();
            let bound = bound.as_ref()?;
            (bound.request().clone(), bound.language())
        };
        request.effective_agent_context = None;
        Some((target, request, language))
    }

    /// Re-arm the waiting Review consent for another outbound scope: rebuild
    /// the same frozen request with or without the selected Effective Agent
    /// Context and disclose the new scope. Nothing is authorized; the next
    /// send needs a fresh hold.
    pub fn set_review_consent_scope(&self, with_context: bool) {
        let Some((target, base, language)) = self.review_base() else {
            return;
        };
        let mut request = base;
        if with_context {
            let context = match self.context_selection(&target) {
                Ok(Some(context)) => context,
                Ok(None) => return,
                Err(error) => {
                    self.status.set(SharedString::from(format!(
                        "Context selection rejected: {error}"
                    )));
                    return;
                }
            };
            request = match request.with_effective_agent_context(context) {
                Ok(request) => request,
                Err(error) => {
                    self.status.set(SharedString::from(format!(
                        "Context request rejected: {error}"
                    )));
                    return;
                }
            };
        }
        let settings = self.settings.get();
        let prepared = match PreparedReview::from_settings(&settings, &self.vault) {
            Ok(prepared) => prepared,
            Err(error) => {
                self.record_review_failure(&error);
                self.status
                    .set(SharedString::from(format!("Review unavailable: {error}")));
                self.cancel_review();
                return;
            }
        };
        let context_included = request.effective_agent_context.is_some();
        let bound = match prepared.bind_document_request(request, language) {
            Ok(bound) => bound,
            Err(error) => {
                self.status.set(SharedString::from(format!(
                    "Review request rejected: {error}"
                )));
                self.cancel_review();
                return;
            }
        };
        let disclosure =
            crate::i18n::model_request_disclosure(settings.language, bound.disclosure());
        let manifest = self.review_manifest_of(
            &bound,
            &target,
            self.excluded_sources(&target, context_included),
        );
        *self.pending_review.borrow_mut() = Some(bound);
        self.review_consent
            .set(Some(SharedString::from(disclosure)));
        self.review_manifest.set(Some(manifest));
    }

    /// The resolved Effective Agent Context sources that are not in the
    /// scope: what stays behind, for the consent card's `Stays here`. With the
    /// context left out of the binding, every resolved source stays, the
    /// selected ones included.
    fn excluded_sources(&self, target: &Path, context_included: bool) -> Vec<OutboundItem> {
        let Some(context) = self.context_resolved.get() else {
            return Vec::new();
        };
        if context.input.target != target {
            return Vec::new();
        }
        let selected = self.context_selected.get();
        context
            .sources
            .iter()
            .enumerate()
            .filter(|(index, _)| !context_included || !selected.contains(index))
            .map(|(_, source)| OutboundItem {
                origin: OutboundOrigin::ExcludedSource,
                path: Some(self.display_path(&source.path)),
                reason: None,
                bytes: u64::try_from(source.included_bytes).unwrap_or(u64::MAX),
                content_leaves: false,
            })
            .collect()
    }

    /// The manifest one bound Review discloses: the document and its named
    /// sources, or every file of the Agent Skill package.
    fn review_manifest_of(
        &self,
        bound: &PreparedDocumentReviewRequest,
        path: &Path,
        excluded: Vec<OutboundItem>,
    ) -> OutboundConsent {
        let disclosure = bound.disclosure().clone();
        let (items, mut stays) = self.scope_items(bound.request(), disclosure.scope(), Some(path));
        stays.extend(excluded);
        OutboundConsent {
            disclosure,
            framed_bytes: bound
                .inspection()
                .ok()
                .map(|inspection| inspection.canonical_byte_size()),
            items,
            stays,
        }
    }

    /// What one bound scope discloses: the document and its named sources, or
    /// every file of an Agent Skill package — plus the items that stay.
    fn scope_items(
        &self,
        request: &ReviewRequest,
        scope: &OutboundScope,
        path: Option<&Path>,
    ) -> (Vec<OutboundItem>, Vec<OutboundItem>) {
        let mut items = Vec::new();
        let mut stays = Vec::new();
        if scope.kind() == OutboundScopeKind::AgentSkillPackage {
            let inventory = scope
                .agent_skill_inventory()
                .expect("an Agent Skill scope owns an inventory");
            let package = request.source.package();
            for file in inventory.files() {
                // A binary entry whose raw bytes were not explicitly selected
                // leaves as path, size and hash only.
                let content_leaves = package
                    .and_then(|package| {
                        package
                            .files()
                            .iter()
                            .find(|entry| entry.path == file.normalized_relative_path())
                    })
                    .is_none_or(|entry| {
                        matches!(
                            entry.payload,
                            SkillFilePayload::Utf8 { .. } | SkillFilePayload::RawBinary { .. }
                        )
                    });
                items.push(OutboundItem {
                    origin: OutboundOrigin::PackageFile,
                    path: Some(file.normalized_relative_path().to_owned()),
                    reason: Some(file.inclusion_reason().to_owned()),
                    bytes: file.byte_size(),
                    content_leaves,
                });
            }
            if let Some(package) = package {
                stays.extend(package.omissions().iter().map(|omission| OutboundItem {
                    origin: OutboundOrigin::Omission,
                    path: Some(omission.path.clone()),
                    reason: Some(omission.reason.clone()),
                    bytes: 0,
                    content_leaves: false,
                }));
            }
            // A package reviewed with Effective Agent Context leaves those
            // sources too: they are named beside its files.
            items.extend(self.context_items(request.effective_agent_context.as_ref()));
        } else {
            if let Some(path) = path {
                items.push(OutboundItem {
                    origin: OutboundOrigin::Document,
                    path: Some(self.display_path(path)),
                    reason: None,
                    bytes: scope.primary_content_byte_size(),
                    content_leaves: true,
                });
            }
            items.extend(self.context_items(request.effective_agent_context.as_ref()));
        }
        (items, stays)
    }

    /// The named Effective Agent Context sources of a binding, in order.
    /// Aliases of one content are counted once, on the first occurrence that
    /// carries it.
    fn context_items(&self, context: Option<&SelectedContext>) -> Vec<OutboundItem> {
        let Some(context) = context else {
            return Vec::new();
        };
        let mut counted: Vec<usize> = Vec::new();
        context
            .sources()
            .iter()
            .map(|source| {
                let occurrence = source.occurrence();
                let first = occurrence.content_index.is_some_and(|index| {
                    if counted.contains(&index) {
                        false
                    } else {
                        counted.push(index);
                        true
                    }
                });
                OutboundItem {
                    origin: OutboundOrigin::NamedSource,
                    path: Some(self.display_path(&occurrence.path)),
                    reason: None,
                    bytes: if first {
                        u64::try_from(occurrence.included_bytes).unwrap_or(u64::MAX)
                    } else {
                        0
                    },
                    content_leaves: true,
                }
            })
            .collect()
    }

    /// The manifest one bound Revision discloses: the reviewed document, the
    /// Review it builds on and the answers it carries.
    fn revision_manifest_of(
        &self,
        bound: &PreparedRevisionRequest,
        tab_id: u64,
    ) -> OutboundConsent {
        let disclosure = bound.disclosure().clone();
        let path = self
            .tabs
            .get()
            .iter()
            .find(|tab| tab.id == tab_id)
            .map(|tab| tab.path.clone());
        // A Revision of an Agent Skill package resends the whole package; an
        // ordinary one resends the document the Review covered.
        let mut items = match disclosure.scope().kind() {
            OutboundScopeKind::AgentSkillPackage => {
                self.scope_items(bound.request(), disclosure.scope(), path.as_deref())
                    .0
            }
            _ => vec![OutboundItem {
                origin: OutboundOrigin::Document,
                path: path.as_deref().map(|path| self.display_path(path)),
                reason: None,
                bytes: disclosure.scope().primary_content_byte_size(),
                content_leaves: true,
            }],
        };
        if let Some(details) = disclosure.revision_details() {
            items.push(OutboundItem {
                origin: OutboundOrigin::ReviewContext,
                path: None,
                reason: None,
                bytes: details.review_context_bytes(),
                content_leaves: true,
            });
            items.push(OutboundItem {
                origin: OutboundOrigin::Answers,
                path: None,
                reason: None,
                bytes: details.answers_bytes(),
                content_leaves: true,
            });
        }
        OutboundConsent {
            disclosure,
            // The Revision payload's own framing is built at execute time and
            // is not part of the disclosure, so only the items are known.
            framed_bytes: None,
            items,
            stays: Vec::new(),
        }
    }

    /// A path as a manifest row shows it: relative to the workspace root when
    /// it is inside it, shortened under the home directory, else itself.
    fn display_path(&self, path: &Path) -> String {
        let slash = |path: &Path| path.to_string_lossy().replace('\\', "/");
        if let Some(root) = self.root.get()
            && let Ok(relative) = path.strip_prefix(&root)
        {
            return slash(relative);
        }
        if let Some(home) = dirs::home_dir()
            && let Ok(relative) = path.strip_prefix(&home)
        {
            return format!("~/{}", slash(relative));
        }
        slash(path)
    }

    /// Send the prepared request. Consent is one-shot and bound to the
    /// disclosure the user just saw.
    pub fn send_review(&self) {
        let Some(bound) = self.pending_review.borrow_mut().take() else {
            return;
        };
        let Some(origin) = self.review_origin.borrow_mut().take() else {
            return;
        };
        let binding = origin;
        let revision_request = bound.request().clone();
        let revision_language = bound.language();
        let disclosure = bound.disclosure().clone();
        let mut consent = ConsentCapability::from_decision(&disclosure, ConsentDecision::Approve);
        let authorization = match bound.authorize(&mut consent) {
            Ok(authorization) => authorization,
            Err(error) => {
                self.status.set(SharedString::from(format!(
                    "Review consent rejected: {error}"
                )));
                return;
            }
        };

        self.review_consent.set(None);
        self.review_manifest.set(None);
        self.review_busy.set(true);
        self.record_usage(UsageEvent::review_started(&revision_request));
        let started = Instant::now();
        let state = self.clone();
        let generation = self.review_generation.get_untracked();
        let revision = self
            .tabs
            .get()
            .iter()
            .find(|tab| tab.id == origin.tab_id)
            .map(|tab| tab.revision)
            .unwrap_or_else(|| signal(0));
        self.spawn_task(async move {
            let result = blocking(move || bound.execute(authorization)).await;
            if state.review_generation.get_untracked() != generation {
                // Withdrawn (or replaced) while the request was out: its
                // answer is not this surface's to show, and it must not touch
                // the view, the revision context or the busy flag.
                return;
            }
            state.review_busy.set(false);
            let view = match result {
                Ok(Ok(transport)) => {
                    state.record_usage(UsageEvent::ReviewCompleted(
                        ReviewDurationBucket::from_duration(started.elapsed()),
                    ));
                    let stale = transport.result.snapshot.revision != revision.get();
                    if let Some(output) = transport.result.output.clone() {
                        state.set_revision_context(
                            origin,
                            revision_request,
                            output,
                            revision_language,
                        );
                    }
                    *state.review_binding.borrow_mut() = Some(binding);
                    let mut view = ReviewView::from_result(
                        &transport.result,
                        stale,
                        state.settings.get().language,
                    );
                    view.tab_id = Some(origin.tab_id);
                    view
                }
                Ok(Err(error)) => {
                    state.record_review_failure(&error);
                    ReviewView::from_error(&error, state.settings.get().language)
                }
                Err(error) => ReviewView {
                    status: crate::i18n::t(
                        state.settings.get().language,
                        crate::i18n::Key::ReviewFailed,
                    )
                    .to_owned(),
                    diagnostic: Some(error.to_string()),
                    ..ReviewView::default()
                },
            };
            state.review_view.set(Some(view));
        });
    }

    fn set_revision_context(
        &self,
        origin: ReviewOrigin,
        request: ReviewRequest,
        output: ReviewModelOutput,
        language: ReviewLanguage,
    ) {
        let questions = output
            .clarification_questions
            .iter()
            .enumerate()
            .map(|(ordinal, question)| RevisionQuestionView {
                ordinal,
                question: question.question.as_str().to_owned(),
                priority: format!("{:?}", question.priority),
                answer: signal(String::new()),
            })
            .collect();
        *self.revision_context.borrow_mut() = Some(RevisionContext {
            tab_id: origin.tab_id,
            snapshot: origin.snapshot,
            request,
            output,
            language,
        });
        *self.revision_proposal.borrow_mut() = None;
        self.revision_questions.set(questions);
        self.revision_consent.set(None);
        self.revision_manifest.set(None);
        self.revision_hunks.set(Vec::new());
        self.revision_status.set(None);
    }

    fn reset_revision(&self) {
        *self.revision_context.borrow_mut() = None;
        *self.pending_revision.borrow_mut() = None;
        *self.revision_proposal.borrow_mut() = None;
        self.revision_questions.set(Vec::new());
        self.revision_consent.set(None);
        self.revision_manifest.set(None);
        self.revision_hunks.set(Vec::new());
        self.revision_status.set(None);
        self.revision_busy.set(false);
    }

    /// Build a Revision request from the frozen Review output and the answer
    /// states shown in the panel, then disclose it before any send.
    pub fn start_revision(&self) {
        let context = self.revision_context.borrow().as_ref().map(|context| {
            (
                context.tab_id,
                context.request.clone(),
                context.output.clone(),
                context.language,
            )
        });
        let Some((tab_id, request, output, language)) = context else {
            self.revision_status.set(Some(SharedString::from(
                "Run Review first to prepare a Revision.",
            )));
            return;
        };
        let answers: Vec<RevisionAnswer> = self
            .revision_questions
            .get()
            .iter()
            .map(|question| {
                let answer = question.answer.get();
                if answer.trim().is_empty() {
                    RevisionAnswer::Unanswered
                } else {
                    RevisionAnswer::Answered(answer)
                }
            })
            .collect();
        let answers = match RevisionAnswers::new(answers) {
            Ok(answers) => answers,
            Err(error) => {
                self.revision_status.set(Some(SharedString::from(format!(
                    "Answers rejected: {error}"
                ))));
                return;
            }
        };
        let settings = self.settings.get();
        let prepared = match PreparedReview::from_settings(&settings, &self.vault) {
            Ok(prepared) => prepared,
            Err(error) => {
                self.record_review_failure(&error);
                self.revision_status.set(Some(SharedString::from(format!(
                    "Revision unavailable: {error}"
                ))));
                return;
            }
        };
        let bound = match prepared.bind_revision_request(request, output, answers, language) {
            Ok(bound) => bound,
            Err(error) => {
                self.revision_status.set(Some(SharedString::from(format!(
                    "Revision request rejected: {error}"
                ))));
                return;
            }
        };
        self.revision_status.set(None);
        self.revision_consent.set(Some(SharedString::from(
            crate::i18n::model_request_disclosure(settings.language, bound.disclosure()),
        )));
        let manifest = self.revision_manifest_of(&bound, tab_id);
        self.revision_manifest.set(Some(manifest));
        *self.pending_revision.borrow_mut() = Some(bound);
    }

    pub fn cancel_revision(&self) {
        *self.pending_revision.borrow_mut() = None;
        self.revision_consent.set(None);
        self.revision_manifest.set(None);
        self.revision_busy.set(false);
    }

    /// Send the prepared Revision after one-shot consent, then expose the
    /// validated hunks for explicit per-change approval.
    pub fn send_revision(&self) {
        let Some(bound) = self.pending_revision.borrow_mut().take() else {
            return;
        };
        let disclosure = bound.disclosure().clone();
        let mut consent = ConsentCapability::from_decision(&disclosure, ConsentDecision::Approve);
        let authorization = match bound.authorize(&mut consent) {
            Ok(authorization) => authorization,
            Err(error) => {
                self.revision_status.set(Some(SharedString::from(format!(
                    "Revision consent rejected: {error}"
                ))));
                return;
            }
        };
        self.revision_consent.set(None);
        self.revision_manifest.set(None);
        self.revision_busy.set(true);
        for answer in bound.answers().as_slice() {
            if matches!(answer, RevisionAnswer::Answered(_)) {
                self.record_usage(UsageEvent::ClarificationAnswered);
            }
        }
        let state = self.clone();
        self.spawn_task(async move {
            let result = blocking(move || bound.execute(authorization)).await;
            state.revision_busy.set(false);
            match result {
                Ok(Ok(transport)) => {
                    let proposal = transport.into_proposal();
                    let mut accepted = HashMap::new();
                    let hunks = proposal
                        .hunks()
                        .iter()
                        .enumerate()
                        .map(|(ordinal, hunk)| RevisionHunkView {
                            ordinal,
                            change_id: hunk.change_id().0,
                            rationale: hunk.rationale().to_owned(),
                            before: hunk.expected_source().to_owned(),
                            after: hunk.replacement().to_owned(),
                            accepted: *accepted
                                .entry(hunk.change_id().0)
                                .or_insert_with(|| signal(false)),
                        })
                        .collect();
                    state.revision_hunks.set(hunks);
                    *state.revision_proposal.borrow_mut() = Some(proposal);
                    state.revision_status.set(Some(SharedString::from(
                        "Revision ready. Approve individual changes, then apply.",
                    )));
                }
                Ok(Err(error)) => {
                    if matches!(
                        error,
                        RevisionError::Timeout
                            | RevisionError::TransportUnavailable
                            | RevisionError::RequestFailed
                            | RevisionError::MissingResponseText
                            | RevisionError::ResponseTooLarge
                            | RevisionError::MalformedResponse
                    ) {
                        state.record_usage(UsageEvent::ProviderFailure);
                    }
                    state.revision_status.set(Some(SharedString::from(format!(
                        "Revision failed: {error}"
                    ))));
                }
                Err(error) => state.revision_status.set(Some(SharedString::from(format!(
                    "Revision task failed: {error}"
                )))),
            }
        });
    }

    pub fn toggle_revision_hunk(&self, index: usize) {
        if let Some(hunk) = self.revision_hunks.get().get(index) {
            hunk.accepted.update(|accepted| *accepted = !*accepted);
        }
    }

    /// Compose only explicitly approved changes and apply them to the buffer.
    pub fn apply_revision(&self) {
        let Some(proposal) = self.revision_proposal.borrow().clone() else {
            self.revision_status.set(Some(SharedString::from(
                "Run Review first to prepare a Revision.",
            )));
            return;
        };
        let Some((tab_id, snapshot)) = self
            .revision_context
            .borrow()
            .as_ref()
            .map(|context| (context.tab_id, context.snapshot))
        else {
            self.revision_status.set(Some(SharedString::from(
                "Run Review first to prepare a Revision.",
            )));
            return;
        };
        let Some(tab) = self.active_tab() else {
            return;
        };
        let current_snapshot = SourceSnapshot::new(tab.revision.get(), 0);
        if tab.id != tab_id || !snapshot.matches(current_snapshot) {
            self.record_usage(UsageEvent::StaleResultBlocked);
            self.revision_status.set(Some(SharedString::from(
                "This Revision belongs to another document or source state. Run Review again before applying.",
            )));
            return;
        }
        let mut decisions = HashMap::new();
        for hunk in self.revision_hunks.get() {
            decisions.insert(ChangeId(hunk.change_id), hunk.accepted.get());
        }
        let decisions: Vec<_> = decisions.into_iter().collect();
        let text = match proposal.compose_against(&tab.text.get(), current_snapshot, &decisions) {
            Ok(text) => text,
            Err(error) => {
                self.revision_status.set(Some(SharedString::from(format!(
                    "Revision apply rejected: {error}"
                ))));
                return;
            }
        };
        let hunks = self.revision_hunks.get();
        if text == tab.text.get().as_str() {
            self.record_usage(UsageEvent::RevisionDecision {
                accepted_hunks: 0,
                rejected_hunks: hunks.len() as u64,
            });
            *self.revision_proposal.borrow_mut() = None;
            self.revision_hunks.set(Vec::new());
            self.revision_status
                .set(Some(SharedString::from("No changes approved.")));
            return;
        }
        let accepted_hunks = hunks.iter().filter(|hunk| hunk.accepted.get()).count() as u64;
        self.record_usage(UsageEvent::RevisionDecision {
            accepted_hunks,
            rejected_hunks: hunks.len() as u64 - accepted_hunks,
        });
        tab.text.set(SharedString::from(text));
        tab.dirty.set(true);
        tab.revision.update(|revision| *revision += 1);
        self.note_edit(tab.id);
        self.record_usage(UsageEvent::RevisionApplied);
        *self.revision_proposal.borrow_mut() = None;
        self.revision_hunks.set(Vec::new());
        self.revision_status.set(Some(SharedString::from(
            "Approved changes applied to the buffer.",
        )));
    }

    /// Whether the live Revision proposal's base moved since the Review it was
    /// computed from — an edit to the document, or another document active —
    /// which is exactly what [`apply_revision`](Self::apply_revision) refuses:
    /// the field says so and offers no Accept then. A proposal from no Review
    /// (a synthetic one) is never stale.
    pub fn revision_is_stale(&self) -> bool {
        let context = self.revision_context.borrow();
        let Some(context) = context.as_ref() else {
            return false;
        };
        match self.active_tab() {
            Some(tab) => {
                tab.id != context.tab_id
                    || !context
                        .snapshot
                        .matches(SourceSnapshot::new(tab.revision.get(), 0))
            }
            None => true,
        }
    }

    /// Withdraw the prepared request before it is sent.
    pub fn cancel_review(&self) {
        self.pending_review.borrow_mut().take();
        self.review_origin.borrow_mut().take();
        self.review_consent.set(None);
        self.review_manifest.set(None);
        self.review_busy.set(false);
    }

    /// Withdraw a review whose request is already in flight: the task's
    /// generation becomes stale, so when the model answers it writes nothing
    /// — no view, no revision context, no busy flag. What the user cancelled
    /// stays cancelled.
    pub fn cancel_review_in_flight(&self) {
        self.review_generation.update(|generation| *generation += 1);
        self.cancel_review();
    }

    pub fn close_review(&self) {
        self.cancel_review();
        self.reset_revision();
        self.review_view.set(None);
        self.review_unavailable.set(None);
        self.review_open.set(false);
    }

    pub fn open_settings(&self) {
        self.refresh_usage_counts();
        self.settings_open.set(true);
    }

    pub fn close_settings(&self) {
        self.settings_open.set(false);
        self.save_settings();
    }

    pub fn save_settings(&self) {
        self.settings.get().save();
        self.status.set(SharedString::from(crate::i18n::t(
            self.settings.get().language,
            crate::i18n::Key::SettingsSaved,
        )));
    }

    pub fn set_language(&self, language: Language) {
        self.settings
            .update(|settings| settings.language = language);
        self.save_settings();
    }

    pub fn set_theme(&self, theme: ThemePreference) {
        self.settings.update(|settings| settings.theme = theme);
        set_color_scheme(match theme {
            ThemePreference::System => nsh_core::platform::Appearance::System,
            ThemePreference::Light => nsh_core::platform::Appearance::Light,
            ThemePreference::Dark => nsh_core::platform::Appearance::Dark,
        });
        self.save_settings();
    }

    /// Store a key for this process only. This path never writes a plaintext
    /// key.
    pub fn store_session_credential(&self, value: &str) -> Result<(), String> {
        let target = self.credential_target()?;
        self.vault
            .replace_session(target, value.to_owned())
            .map_err(|error| {
                self.record_usage(UsageEvent::CredentialFailure);
                error.to_string()
            })
    }

    /// Store a key in the platform's secure store. Refused where the platform
    /// provides none, rather than falling back to plaintext.
    pub fn store_persistent_credential(&self, value: &str) -> Result<(), String> {
        if !self.vault.secure_store_supported() {
            self.record_usage(UsageEvent::CredentialFailure);
            return Err("Secure storage is unavailable on this platform; use a session key or an environment variable.".to_owned());
        }
        let target = self.credential_target()?;
        self.vault
            .replace_persistent(&target, value.to_owned())
            .map_err(|error| {
                self.record_usage(UsageEvent::CredentialFailure);
                error.to_string()
            })
    }

    /// How the configured endpoint's credential would resolve right now,
    /// without reading or returning the secret. Mirrors the resolution a model
    /// operation does: this process's session key first, then the secure store,
    /// then the provider's environment variable for an endpoint allowed to use
    /// it.
    ///
    /// The vault's only source-aware query is `resolve`, and resolving reads
    /// the platform's secure store behind a global mutex; callers that draw a
    /// status line should use [`Self::credential_status_cached`] instead.
    pub fn credential_status(&self) -> CredentialStatus {
        let settings = self.settings.get();
        let Some(provider) = mt_core::model::Provider::from_key(settings.model_provider.trim())
        else {
            return CredentialStatus::Unconfigured;
        };
        let base = (!settings.model_base_url.trim().is_empty())
            .then_some(settings.model_base_url.as_str());
        let Ok(endpoint) = mt_core::model::EndpointIdentity::parse(provider, base) else {
            return CredentialStatus::Unconfigured;
        };
        let target = endpoint.credential_target();
        let environment_allowed = endpoint.is_vendor_default()
            || settings.model_environment_key_identity == target.as_str();
        let environment_value = environment_allowed
            .then(|| std::env::var(provider.credential_environment_variable()).ok())
            .flatten();
        match self
            .vault
            .resolve(target.as_str(), environment_value, environment_allowed)
        {
            Ok(Some(resolved)) => match resolved.source() {
                mt_core::credentials::CredentialSource::Session => CredentialStatus::Session,
                mt_core::credentials::CredentialSource::Persistent => CredentialStatus::Stored,
                mt_core::credentials::CredentialSource::Environment => {
                    CredentialStatus::Environment
                }
            },
            Ok(None) => CredentialStatus::Missing,
            Err(_) => CredentialStatus::Unavailable,
        }
    }

    /// [`credential_status`](Self::credential_status), computed again only when
    /// the endpoint or the credential state changes: a status line must not
    /// read the secure store once per binding, nor on every keystroke of a
    /// field the status does not depend on.
    pub(crate) fn credential_status_cached(&self) -> CredentialStatus {
        let settings = self.settings.get();
        let key = CredentialStatusKey {
            provider: settings.model_provider.clone(),
            base_url: settings.model_base_url.clone(),
            environment_identity: settings.model_environment_key_identity.clone(),
            revision: self.settings_credential_revision.get(),
        };
        if let Some((cached, status)) = self.settings_credential_status.get()
            && cached == key
        {
            return status;
        }
        let status = self.credential_status();
        self.settings_credential_status.set(Some((key, status)));
        status
    }

    /// Forget the configured endpoint's key: the secure-store entry and this
    /// process's copy, if either exists.
    pub fn remove_credential(&self) -> Result<(), String> {
        let target = self.credential_target()?;
        self.vault.delete(&target).map_err(|error| {
            self.record_usage(UsageEvent::CredentialFailure);
            error.to_string()
        })
    }

    fn credential_target(&self) -> Result<String, String> {
        let settings = self.settings.get();
        let provider = mt_core::model::Provider::from_key(settings.model_provider.trim())
            .ok_or_else(|| "Choose a supported provider first.".to_owned())?;
        let base = (!settings.model_base_url.trim().is_empty())
            .then_some(settings.model_base_url.as_str());
        let endpoint = mt_core::model::EndpointIdentity::parse(provider, base)
            .map_err(|error| error.to_string())?;
        Ok(endpoint.credential_target().as_str().to_owned())
    }
}

/// The settings [`AppState::credential_status_cached`] depends on: the wire
/// format, the base URL, the endpoint's environment authorization, and the
/// credential revision.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct CredentialStatusKey {
    provider: String,
    base_url: String,
    environment_identity: String,
    revision: u64,
}

/// How the configured endpoint's model credential resolves right now, as the
/// settings page's status line says it. Never carries the key itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CredentialStatus {
    /// No usable endpoint: no wire format chosen, or the base URL is invalid.
    Unconfigured,
    /// A wire format and endpoint are set, but no key is available.
    Missing,
    /// A key held for this process only.
    Session,
    /// A key in the platform's secure store (Windows Credential Manager).
    Stored,
    /// A key from the provider's environment variable.
    Environment,
    /// The secure store could not be read.
    Unavailable,
}

impl ReviewView {
    /// Whether `self` and `other` are the same result: the same understood
    /// sections, findings and questions. The status text and the stale flag
    /// change under one result without it becoming a new arrival.
    pub fn same_result(&self, other: &Self) -> bool {
        self.sections == other.sections
            && self.findings_anchored == other.findings_anchored
            && self.questions_detail == other.questions_detail
    }

    fn from_result(result: &ReviewResult, stale: bool, language: Language) -> Self {
        let mut view = Self {
            status: if stale {
                crate::i18n::t(language, crate::i18n::Key::ReviewStale).to_owned()
            } else {
                match result.status {
                    ReviewStatus::Ready => {
                        crate::i18n::t(language, crate::i18n::Key::ReviewReady).to_owned()
                    }
                    ReviewStatus::Stale => {
                        crate::i18n::t(language, crate::i18n::Key::ReviewStale).to_owned()
                    }
                    ReviewStatus::Diagnostic => {
                        crate::i18n::t(language, crate::i18n::Key::ReviewMalformed).to_owned()
                    }
                }
            },
            diagnostic: result
                .diagnostic
                .as_ref()
                .map(|diagnostic| diagnostic.message.as_str().to_owned()),
            stale: stale || matches!(result.status, ReviewStatus::Stale),
            ..Self::default()
        };
        let Some(output) = &result.output else {
            return view;
        };
        view.sections = sections(&output.understood_intent, language);
        view.findings = output
            .findings
            .iter()
            .map(|finding| {
                let kind = match finding.kind {
                    FindingKind::Source | FindingKind::SourceStatement => {
                        crate::i18n::t(language, crate::i18n::Key::ReviewSourceStatement)
                    }
                    FindingKind::Inference => {
                        crate::i18n::t(language, crate::i18n::Key::ReviewInference)
                    }
                };
                format!("{kind}: {}", finding.text.as_str())
            })
            .collect();
        view.findings_anchored = output
            .findings
            .iter()
            .map(|finding| ReviewFindingView {
                stated: matches!(
                    finding.kind,
                    FindingKind::Source | FindingKind::SourceStatement
                ),
                text: finding.text.as_str().to_owned(),
                location: match &finding.anchor {
                    SourceAnchor::Document { location } => Some(*location),
                    _ => None,
                },
            })
            .collect();
        view.questions = output
            .clarification_questions
            .iter()
            .map(|question| {
                let priority = match question.priority {
                    ClarificationPriority::Critical => {
                        crate::i18n::t(language, crate::i18n::Key::ReviewPriorityCritical)
                    }
                    ClarificationPriority::High => {
                        crate::i18n::t(language, crate::i18n::Key::ReviewPriorityHigh)
                    }
                    ClarificationPriority::Medium => {
                        crate::i18n::t(language, crate::i18n::Key::ReviewPriorityMedium)
                    }
                    ClarificationPriority::Low => {
                        crate::i18n::t(language, crate::i18n::Key::ReviewPriorityLow)
                    }
                };
                match &question.impact {
                    Some(impact) => format!(
                        "[{priority}] {}\n{}",
                        question.question.as_str(),
                        impact.as_str()
                    ),
                    None => format!("[{priority}] {}", question.question.as_str()),
                }
            })
            .collect();
        view.questions_detail = output
            .clarification_questions
            .iter()
            .map(|question| ReviewQuestionView {
                priority: question.priority,
                question: question.question.as_str().to_owned(),
                impact: question
                    .impact
                    .as_ref()
                    .map(|impact| impact.as_str().to_owned()),
            })
            .collect();
        view
    }

    fn from_error(error: &ReviewError, language: Language) -> Self {
        Self {
            status: crate::i18n::t(language, crate::i18n::Key::ReviewFailed).to_owned(),
            diagnostic: Some(error.to_string()),
            failure: Some(match error {
                ReviewError::Timeout { .. } => ReviewFailure::Timeout,
                ReviewError::MalformedResponse { .. }
                | ReviewError::MissingResponseText { .. }
                | ReviewError::ResponseTooLarge { .. } => ReviewFailure::Malformed,
                ReviewError::Cancelled { .. } => ReviewFailure::Cancelled,
                _ => ReviewFailure::Other,
            }),
            ..Self::default()
        }
    }
}

fn sections(intent: &mt_core::review::ReviewSections, language: Language) -> Vec<ReviewSection> {
    let list = |title: crate::i18n::Key, items: &[mt_core::review::StructuredText]| ReviewSection {
        title: crate::i18n::t(language, title).to_owned(),
        items: items.iter().map(|item| item.as_str().to_owned()).collect(),
    };
    [
        Some(ReviewSection {
            title: crate::i18n::t(language, crate::i18n::Key::ReviewStatedGoal).to_owned(),
            items: vec![intent.stated_goal.as_str().to_owned()],
        }),
        Some(list(
            crate::i18n::Key::ReviewContext,
            &intent.relevant_context,
        )),
        Some(list(
            crate::i18n::Key::ReviewConstraints,
            &intent.constraints,
        )),
        Some(list(crate::i18n::Key::ReviewNonGoals, &intent.non_goals)),
        Some(ReviewSection {
            title: crate::i18n::t(language, crate::i18n::Key::ReviewDeliverable).to_owned(),
            items: vec![intent.expected_deliverable.as_str().to_owned()],
        }),
        Some(list(
            crate::i18n::Key::ReviewSuccessEvidence,
            &intent.success_evidence,
        )),
        Some(list(
            crate::i18n::Key::ReviewAssumptions,
            &intent.inferred_assumptions,
        )),
        Some(list(
            crate::i18n::Key::ReviewDecisions,
            &intent.unresolved_decisions,
        )),
    ]
    .into_iter()
    .flatten()
    .filter(|section| !section.items.is_empty())
    .collect()
}

impl Default for AppState {
    fn default() -> Self {
        Self::new()
    }
}

fn flatten(nodes: &[FileNode], depth: usize, out: &mut Vec<Entry>) {
    for node in nodes {
        out.push(Entry {
            path: node.path.clone(),
            name: node.name.clone(),
            depth,
            is_dir: node.is_dir,
        });
        if node.is_dir {
            flatten(&node.children, depth + 1, out);
        }
    }
}

/// The editor language for a path, by extension.
pub fn language_for_path(path: &Path) -> &'static str {
    match path
        .extension()
        .and_then(|extension| extension.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase()
        .as_str()
    {
        "md" | "markdown" => "markdown",
        "mdx" => "markdown",
        "rs" => "rust",
        "toml" => "toml",
        "json" => "json",
        "yaml" | "yml" => "yaml",
        "ts" => "typescript",
        "tsx" => "tsx",
        "js" | "jsx" => "javascript",
        "py" => "python",
        "sh" | "bash" => "shell",
        "html" => "html",
        "css" => "css",
        "go" => "go",
        "c" => "c",
        "cpp" | "cc" | "hpp" => "cpp",
        "java" => "java",
        "sql" => "sql",
        "diff" | "patch" => "diff",
        _ => "plain",
    }
}

/// Rebuild a `LoadedFile` from a recovery record without touching the source.
fn recovered_loaded(record: &RecoveredRecord) -> LoadedFile {
    let metadata = &record.record.metadata;
    LoadedFile {
        path: metadata
            .source_path
            .clone()
            .unwrap_or_else(|| PathBuf::from("recovered.md")),
        text: record.record.text.clone(),
        stamp: metadata.original_stamp.clone(),
        skill_origin: None,
        newline: metadata.newline,
        had_bom: metadata.had_bom,
        encoding: encoding_rs::Encoding::for_label(metadata.encoding_name.as_bytes())
            .unwrap_or(encoding_rs::UTF_8),
        decode_had_errors: metadata.decode_had_errors,
        source_identity: metadata.source_identity.clone(),
    }
}
